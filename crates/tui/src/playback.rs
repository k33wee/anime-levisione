use crate::events::Message;
#[cfg(unix)]
use anime_levisione_core::store::Progress;
use anime_levisione_core::{
    audio::AudioTrack,
    model::{Anime, Episode},
    provider::{Stream, http::USER_AGENT},
    store::Config,
};
use anyhow::{Context, Result, bail};
#[cfg(unix)]
use std::time::Duration;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::{io::AsyncRead, process::Command, sync::mpsc};

// Re-exported so player-side code can keep using `playback::Media`.
pub use anime_levisione_core::provider::Media;
pub struct PlayRequest {
    pub id: u64,
    pub media: Media,
    pub anime: Anime,
    pub episode: Episode,
    pub resume: f64,
    pub audio: Option<AudioTrack>,
    pub audio_warning: Option<String>,
}

fn is_vlc(config: &Config) -> bool {
    Path::new(&config.player)
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("vlc"))
}
pub fn supports_audio_choice(config: &Config) -> bool {
    is_mpv(config) || is_vlc(config)
}
pub fn is_mpv(config: &Config) -> bool {
    Path::new(&config.player)
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("mpv"))
}
/// Drain stderr so children cannot block; keep only a small diagnostic tail.
pub async fn diagnostic_tail(reader: impl AsyncRead + Unpin) -> String {
    use tokio::io::AsyncReadExt;
    let mut reader = reader;
    let mut chunk = [0u8; 1024];
    let mut tail = Vec::new();
    while let Ok(n) = reader.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        tail.extend_from_slice(&chunk[..n]);
        if tail.len() > 4096 {
            tail.drain(..tail.len() - 4096);
        }
    }
    anime_levisione_core::model::clean(&String::from_utf8_lossy(&tail))
}
fn player_command(config: &Config, request: &PlayRequest, _socket: &Path) -> Command {
    let PlayRequest {
        media,
        anime,
        episode,
        resume,
        audio,
        ..
    } = request;
    let mut command = Command::new(&config.player);
    command
        .args(&config.player_args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if is_mpv(config) {
        command
            .arg("--no-terminal")
            .arg("--keep-open=no")
            .arg(format!(
                "--force-media-title={} · Episode {}",
                anime.title, episode.number
            ));
        #[cfg(unix)]
        command.arg(format!("--input-ipc-server={}", _socket.display()));
        if resume.is_finite() && *resume > 3.0 {
            command.arg(format!("--start={resume:.0}"));
        }
        if let Media::Remote(s) = &media {
            command
                .arg(format!("--referrer={}", s.referer))
                .arg(format!("--user-agent={USER_AGENT}"));
        }
        if let Some(track) = audio {
            if let Some(language) = &track.language {
                command.arg("--aid=auto").arg(format!("--alang={language}"));
            } else {
                command
                    .arg("--alang=")
                    .arg(format!("--aid={}", track.index + 1));
            }
        }
        command.arg("--");
    } else if is_vlc(config) {
        command.arg("--play-and-exit");
        if resume.is_finite() && *resume > 3.0 {
            command.arg(format!("--start-time={resume:.0}"));
        }
        if let Media::Remote(s) = &media {
            command
                .arg(format!("--http-referrer={}", s.referer))
                .arg(format!("--http-user-agent={USER_AGENT}"));
        }
        if let Some(track) = audio {
            // A startup language survives VLC's HLS track-ID recreation. Clear
            // numeric overrides in player_args before applying this choice.
            command.arg("--audio-track-id=-1");
            if let Some(language) = &track.language {
                command
                    .arg("--audio-track=-1")
                    .arg(format!("--audio-language={language}"));
            } else {
                command
                    .arg("--audio-language=any")
                    .arg(format!("--audio-track={}", track.index));
            }
        }
        command.arg("--");
    }
    command.arg(media.source());
    command
}

pub async fn play(
    config: Config,
    request: PlayRequest,
    tx: mpsc::UnboundedSender<Message>,
) -> Result<()> {
    let temp = tempfile::Builder::new()
        .prefix("anime-levisione-")
        .tempdir()?;
    let socket = temp.path().join("mpv.sock");
    let mut command = player_command(&config, &request, &socket);
    let PlayRequest {
        id,
        media: _,
        anime,
        episode,
        resume,
        audio,
        audio_warning,
    } = request;
    let mut child = command.spawn().with_context(|| {
        format!(
            "Cannot start '{}'; install mpv or set player in config.toml",
            config.player
        )
    })?;
    let _ = tx.send(Message::PlaybackStarted {
        request_id: id,
        audio: audio.map(|track| track.label),
        warning: audio_warning,
    });
    let stderr = child.stderr.take().context("Player stderr unavailable")?;
    let diagnostic = tokio::spawn(diagnostic_tail(stderr));
    #[cfg(unix)]
    let monitor = if is_mpv(&config) {
        Some(tokio::spawn(monitor(
            socket,
            anime,
            episode,
            resume,
            tx.clone(),
        )))
    } else {
        None
    };
    #[cfg(not(unix))]
    let _ = (socket, anime, episode, tx, resume);
    let status = child.wait().await?;
    #[cfg(unix)]
    if let Some(monitor) = monitor {
        // EOF normally reaches the observer first; allow its final buffered update.
        let _ = tokio::time::timeout(Duration::from_millis(400), monitor).await;
    }
    let tail = diagnostic.await.unwrap_or_default();
    if !status.success() {
        bail!(
            "Player exited with {status}. {}",
            if tail.is_empty() {
                "Check player installation and retry the episode."
            } else {
                &tail
            }
        );
    }
    Ok(())
}
#[cfg(unix)]
async fn monitor(
    socket: PathBuf,
    anime: Anime,
    episode: Episode,
    resume: f64,
    tx: mpsc::UnboundedSender<Message>,
) {
    use tokio::{io::AsyncWriteExt, net::UnixStream};
    let mut stream = None;
    for _ in 0..80 {
        if let Ok(s) = UnixStream::connect(&socket).await {
            stream = Some(s);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let Some(mut stream) = stream else {
        return;
    };
    for (id, name) in [(1, "time-pos"), (2, "duration"), (3, "eof-reached")] {
        let cmd = serde_json::json!({"command": ["observe_property", id, name]}).to_string() + "\n";
        if stream.write_all(cmd.as_bytes()).await.is_err() {
            return;
        }
    }
    let mut progress = Progress {
        episode,
        seconds: resume,
        duration: 0.0,
        completed: false,
    };
    let mut lines = BufReader::new(stream).lines();
    let mut last_sent: Option<tokio::time::Instant> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v["event"] != "property-change" {
            continue;
        }
        match v["name"].as_str() {
            Some("time-pos") => {
                if let Some(seconds) = v["data"].as_f64() {
                    progress.seconds = seconds;
                }
            }
            Some("duration") => {
                if let Some(duration) = v["data"].as_f64() {
                    progress.duration = duration;
                }
            }
            Some("eof-reached") if v["data"] == true => progress.completed = true,
            _ => {}
        }
        progress.completed |=
            progress.duration > 0.0 && progress.seconds / progress.duration >= 0.90;
        if last_sent.is_none_or(|t| t.elapsed() >= Duration::from_secs(2)) || progress.completed {
            let _ = tx.send(Message::Progress {
                anime: anime.clone(),
                progress: progress.clone(),
            });
            last_sent = Some(tokio::time::Instant::now());
        }
    }
    let _ = tx.send(Message::Progress { anime, progress });
}

#[derive(Debug, Clone)]
pub struct OfflineEpisode {
    pub anime: Anime,
    pub episode: Episode,
    pub path: PathBuf,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct DownloadManifest {
    anime: Anime,
    episode: Episode,
    file: String,
}
pub fn safe_name(value: &str) -> String {
    let value: String = value
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " .-_".contains(c) {
                c
            } else {
                '_'
            }
        })
        .take(100)
        .collect();
    let value = value.trim_matches([' ', '.']);
    if value.is_empty() {
        "anime".into()
    } else {
        value.into()
    }
}
pub async fn download(
    config: Config,
    stream: Stream,
    anime: Anime,
    episode: Episode,
) -> Result<String> {
    let directory = config.download_dir.join(format!(
        "{} [{}-{}]",
        safe_name(&anime.title),
        anime.provider.short(),
        safe_name(&anime.id)
    ));
    tokio::fs::create_dir_all(&directory).await?;
    let name = format!("Episode {}.mkv", safe_name(&episode.number));
    let destination = directory.join(&name);
    let manifest_path = destination.with_extension("json");
    if destination.exists() {
        bail!("Already downloaded: {}", destination.display());
    }
    // ffmpeg handles both direct files and HLS correctly; never save a playlist as .mp4.
    let temporary = tempfile::Builder::new()
        .prefix(".")
        .suffix(".part.mkv")
        .tempfile_in(&directory)?;
    let output = Command::new("ffmpeg")
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-user_agent",
            USER_AGENT,
            "-referer",
            &stream.referer,
            "-i",
            stream.url.as_str(),
            "-map",
            "0:v?",
            "-map",
            "0:a?",
            "-map",
            "0:s?",
            "-c",
            "copy",
        ])
        .arg(temporary.path())
        .spawn()
        .context("Cannot start ffmpeg; install it to download episodes")?;
    let mut child = output;
    let diagnostic = tokio::spawn(diagnostic_tail(
        child.stderr.take().context("ffmpeg stderr unavailable")?,
    ));
    let status = child.wait().await?;
    let tail = diagnostic.await.unwrap_or_default();
    if !status.success() {
        bail!("Download failed: {tail}");
    }
    if temporary.as_file().metadata()?.len() == 0 {
        bail!("Download returned an empty file");
    }
    // Persist manifest first: a crash never leaves a complete video invisible to the scanner.
    let manifest = DownloadManifest {
        anime,
        episode,
        file: name,
    };
    anime_levisione_core::store::atomic_write(
        &manifest_path,
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    temporary
        .persist_noclobber(&destination)
        .context("Cannot finalize download; existing file was not overwritten")?;
    Ok(destination.display().to_string())
}
pub fn scan_offline(root: &Path) -> Result<Vec<OfflineEpisode>> {
    let mut result = Vec::new();
    let dirs = match std::fs::read_dir(root) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(e) => return Err(e.into()),
    };
    for dir in dirs {
        let dir = dir?;
        if !dir.file_type()?.is_dir() {
            continue;
        }
        for file in std::fs::read_dir(dir.path())? {
            let file = file?;
            if file.path().extension().is_none_or(|e| e != "json") {
                continue;
            }
            let bytes = std::fs::read(file.path())?;
            let manifest: DownloadManifest = serde_json::from_slice(&bytes)
                .with_context(|| format!("Invalid download manifest {}", file.path().display()))?;
            // Treat manifests as untrusted: never resolve paths outside their own directory.
            let relative = Path::new(&manifest.file);
            if relative.components().count() != 1
                || !matches!(
                    relative.components().next(),
                    Some(std::path::Component::Normal(_))
                )
            {
                bail!("Unsafe path in download manifest {}", file.path().display());
            }
            let path = dir.path().join(relative);
            if path.is_file() {
                result.push(OfflineEpisode {
                    anime: manifest.anime,
                    episode: manifest.episode,
                    path,
                });
            }
        }
    }
    result.sort_by(|a, b| {
        a.anime.title.cmp(&b.anime.title).then_with(|| {
            a.episode
                .number
                .parse::<f64>()
                .unwrap_or(0.0)
                .total_cmp(&b.episode.number.parse::<f64>().unwrap_or(0.0))
        })
    });
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(audio: Option<AudioTrack>) -> PlayRequest {
        PlayRequest {
            id: 7,
            media: Media::Remote(Stream {
                url: "https://cdn.example/playlist/42?token=signed"
                    .parse()
                    .expect("url"),
                referer: "https://player.example/embed/42".into(),
            }),
            anime: serde_json::from_value(serde_json::json!({
                "provider": "streamingcommunity", "id": "42-test", "title": "Test",
            }))
            .expect("anime"),
            episode: Episode {
                id: "9".into(),
                number: "3".into(),
                title: String::new(),
            },
            resume: 42.0,
            audio,
            audio_warning: None,
        }
    }
    fn args(config: Config, request: &PlayRequest) -> Vec<String> {
        player_command(&config, request, Path::new("/tmp/test.sock"))
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }
    fn english() -> AudioTrack {
        AudioTrack {
            language: Some("eng".into()),
            label: "English".into(),
            index: 1,
            default: false,
        }
    }

    #[test]
    fn selected_vlc_language_overrides_preferences_without_losing_resume_or_headers() {
        let request = request(Some(english()));
        let args = args(
            Config {
                player: "vlc".into(),
                player_args: vec!["--audio-language=ita".into(), "--audio-track=0".into()],
                ..Config::default()
            },
            &request,
        );
        assert!(args.contains(&"--audio-track-id=-1".into()));
        assert_eq!(
            args.iter()
                .rfind(|arg| arg.starts_with("--audio-language=")),
            Some(&"--audio-language=eng".into())
        );
        assert_eq!(
            args.iter().rfind(|arg| arg.starts_with("--audio-track=")),
            Some(&"--audio-track=-1".into())
        );
        assert!(args.contains(&"--start-time=42".into()));
        assert!(args.contains(&"--http-referrer=https://player.example/embed/42".into()));
        assert_eq!(args.last(), Some(&request.media.source()));
        assert_eq!(args[args.len() - 2], "--");
    }

    #[test]
    fn mpv_uses_language_and_unknown_file_tracks_use_audio_ordinals() {
        let config = Config {
            player: "mpv".into(),
            ..Config::default()
        };
        let selected = args(config.clone(), &request(Some(english())));
        assert!(selected.contains(&"--aid=auto".into()));
        assert!(selected.contains(&"--alang=eng".into()));
        assert!(selected.contains(&"--start=42".into()));
        let unknown = AudioTrack {
            language: None,
            ..english()
        };
        assert!(args(config, &request(Some(unknown.clone()))).contains(&"--aid=2".into()));
        let vlc = args(
            Config {
                player: "vlc".into(),
                ..Config::default()
            },
            &request(Some(unknown)),
        );
        assert!(vlc.contains(&"--audio-track=1".into()));
    }

    #[test]
    fn no_choice_preserves_the_configured_player_audio_defaults() {
        let config = Config {
            player: "vlc".into(),
            player_args: vec!["--audio-language=ita".into()],
            ..Config::default()
        };
        let args = args(config, &request(None));
        assert_eq!(
            args.iter()
                .filter(|arg| arg.starts_with("--audio-"))
                .count(),
            1
        );
        assert!(args.contains(&"--audio-language=ita".into()));
    }

    #[test]
    fn file_names_cannot_escape_directory() {
        assert_eq!(safe_name("../../evil/name\n"), "_.._evil_name_");
        assert_eq!(safe_name("..."), "anime");
        assert!(!safe_name("a/b\\c").contains('/'));
    }
    #[test]
    fn missing_offline_folder_is_empty() -> Result<()> {
        let dir = tempfile::tempdir()?;
        assert!(scan_offline(&dir.path().join("missing"))?.is_empty());
        Ok(())
    }
}
