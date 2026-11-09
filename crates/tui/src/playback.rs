use crate::events::Message;
#[cfg(unix)]
use anime_levisione_core::store::Progress;
use anime_levisione_core::{
    audio::AudioTrack,
    diagnostics::{self, DiagnosticEvent, DiagnosticOperation},
    media_error,
    model::{Anime, Episode},
    provider::{Stream, http::USER_AGENT},
    store::Config,
};
use anyhow::{Context, Result, bail};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::Command,
    sync::mpsc,
};

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
    media_error::read(reader).await.tail
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
            "Cannot start '{}'; install VLC or mpv, or set player in config.toml",
            config.player
        )
    })?;
    diagnostics::record(DiagnosticEvent::PlayerOpened { session: id });
    let _ = tx.send(Message::PlaybackStarted {
        request_id: id,
        audio: audio.map(|track| track.label),
        warning: audio_warning,
    });
    let stderr = child.stderr.take().context("Player stderr unavailable")?;
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
    let (status, output) = media_error::capture(stderr, child.wait()).await;
    let status = status?;
    diagnostics::record(DiagnosticEvent::PlayerProcessExit {
        session: id,
        code: status.code(),
        success: status.success(),
    });
    #[cfg(unix)]
    if let Some(monitor) = monitor {
        // EOF normally reaches the observer first; allow its final buffered update.
        let _ = tokio::time::timeout(Duration::from_millis(400), monitor).await;
    }
    // VLC reports a successful process exit even for an unreadable media URL.
    // Require its explicit fatal-input marker: decoder/HTTP fallback warnings alone
    // do not mean playback failed.
    let failure = if is_vlc(&config) {
        output.vlc_failure()
    } else if !status.success() {
        output.failure
    } else {
        None
    };
    if let Some(error) = failure {
        error.record(DiagnosticOperation::Playback);
        return Err(error.into());
    }
    let tail = output.tail;
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
/// Directory every episode of a title is downloaded into, under the configured
/// download directory. Shared with the UI so the target can be shown before
/// ffmpeg starts.
pub fn download_directory(config: &Config, anime: &Anime) -> PathBuf {
    config.download_dir.join(format!(
        "{} [{}-{}]",
        safe_name(&anime.title),
        anime.provider.short(),
        safe_name(&anime.id)
    ))
}
/// Final file an episode download is saved to.
pub fn destination(config: &Config, anime: &Anime, episode: &Episode) -> PathBuf {
    download_directory(config, anime).join(format!("Episode {}.mkv", safe_name(&episode.number)))
}

pub struct DownloadRequest {
    pub id: u64,
    pub stream: Stream,
    pub anime: Anime,
    pub episode: Episode,
    /// Chosen audio track: its language tag (preferred) or ordinal selects the
    /// ffmpeg audio mapping. `None` keeps every audio stream.
    pub audio: Option<AudioTrack>,
    /// Best-effort media duration used to estimate the final file size.
    pub duration: Option<f64>,
}

/// ffmpeg `-map` selector for the chosen audio: by language metadata when the
/// track has one (VLC/mpv-compatible normalized code), else by audio ordinal.
fn audio_map(track: &AudioTrack) -> String {
    match &track.language {
        Some(language) => format!("0:a:m:language:{language}"),
        None => format!("0:a:{}", track.index),
    }
}

fn ffmpeg_args(stream: &Stream, audio_selector: &str, output: &Path) -> Vec<String> {
    vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-nostats".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-y".into(),
        "-user_agent".into(),
        USER_AGENT.into(),
        "-referer".into(),
        stream.referer.clone(),
        "-i".into(),
        stream.url.to_string(),
        "-map".into(),
        "0:v?".into(),
        "-map".into(),
        audio_selector.into(),
        "-map".into(),
        "0:s?".into(),
        "-c".into(),
        "copy".into(),
        output.display().to_string(),
    ]
}

/// One ffmpeg run. Reports size/speed/estimate through the message channel
/// at most twice a second; returns (success, stderr tail).
async fn run_ffmpeg(
    args: &[String],
    request_id: u64,
    duration: Option<f64>,
    tx: &mpsc::UnboundedSender<Message>,
) -> Result<(bool, String)> {
    let mut child = Command::new("ffmpeg")
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args(args)
        .spawn()
        .context("Cannot start ffmpeg; install it to download episodes")?;
    let tail = tokio::spawn(diagnostic_tail(
        child.stderr.take().context("ffmpeg stderr unavailable")?,
    ));
    let progress = tokio::spawn(report_progress(
        child
            .stdout
            .take()
            .context("ffmpeg progress pipe unavailable")?,
        request_id,
        duration,
        tx.clone(),
    ));
    let status = child.wait().await?;
    let _ = progress.await;
    let tail = tail.await.unwrap_or_default();
    Ok((status.success(), tail))
}

/// Estimated final size: observed bytes scaled by the media duration over
/// the already-processed seconds. Returns `None` without a usable duration,
/// and only after enough media has been processed for a sane extrapolation
/// (early samples are mostly container headers and skew the total).
fn size_estimate(bytes: u64, done_seconds: Option<f64>, duration: Option<f64>) -> Option<u64> {
    duration
        .filter(|total| *total > 0.0)
        .zip(done_seconds)
        .and_then(|(total, done)| {
            (done >= 5.0).then(|| (bytes as f64 * total / done).round() as u64)
        })
}

/// Parse ffmpeg `-progress pipe:1` reports and forward them as messages.
/// Each block ends with a `progress=` key, so reports are emitted once per
/// complete block (all fields present) and at most ~2.5 times a second.
async fn report_progress<R: AsyncRead + Unpin>(
    pipe: R,
    request_id: u64,
    duration: Option<f64>,
    tx: mpsc::UnboundedSender<Message>,
) {
    let mut lines = BufReader::new(pipe).lines();
    let mut bytes: Option<u64> = None;
    let mut done_seconds: Option<f64> = None;
    let mut last_sample: Option<(std::time::Instant, u64)> = None;
    let mut last_sent: Option<std::time::Instant> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "total_size" => bytes = value.parse().ok(),
            "out_time_us" => done_seconds = value.parse::<f64>().ok().map(|us| us / 1_000_000.0),
            "progress" => {
                let end = value == "end";
                // The final block always reports so the last byte count lands.
                if (end || last_sent.is_none_or(|at| at.elapsed() >= Duration::from_millis(400)))
                    && let Some(bytes) = bytes
                {
                    let speed = last_sample.map(|(at, previous)| {
                        let elapsed = at.elapsed().as_secs_f64();
                        if elapsed > 0.0 {
                            ((bytes.saturating_sub(previous)) as f64 / elapsed) as u64
                        } else {
                            0
                        }
                    });
                    last_sample = Some((std::time::Instant::now(), bytes));
                    let _ = tx.send(Message::DownloadProgress {
                        request_id,
                        bytes,
                        total: size_estimate(bytes, done_seconds, duration),
                        speed: speed.filter(|speed| *speed > 0),
                    });
                    last_sent = Some(std::time::Instant::now());
                }
                if end {
                    break;
                }
            }
            _ => continue,
        }
    }
}

pub async fn download(
    config: Config,
    request: DownloadRequest,
    tx: mpsc::UnboundedSender<Message>,
) -> Result<String> {
    let DownloadRequest {
        id,
        stream,
        anime,
        episode,
        audio,
        duration,
    } = request;
    let destination = destination(&config, &anime, &episode);
    let directory = download_directory(&config, &anime);
    tokio::fs::create_dir_all(&directory).await?;
    if destination.exists() {
        bail!("Already downloaded: {}", destination.display());
    }
    // ffmpeg handles both direct files and HLS correctly; never save a playlist as .mp4.
    let mut temporary = tempfile::Builder::new()
        .prefix(".")
        .suffix(".part.mkv")
        .tempfile_in(&directory)?;
    // Selector escalation: the chosen mapping first, then the audio ordinal
    // (probe and ffmpeg stream numbering can drift on HLS masters), then every
    // audio stream. The user is told whenever the explicit choice is dropped.
    let mut selectors = vec![
        audio
            .as_ref()
            .map(audio_map)
            .unwrap_or_else(|| "0:a?".into()),
    ];
    if let Some(track) = &audio
        && track.language.is_some()
        && !selectors
            .iter()
            .any(|s| *s == format!("0:a:{}", track.index))
    {
        selectors.push(format!("0:a:{}", track.index));
    }
    if selectors.last() != Some(&"0:a?".to_string()) {
        selectors.push("0:a?".into());
    }
    let mut result = None;
    for (attempt, selector) in selectors.iter().enumerate() {
        if attempt > 0 {
            let _ = tx.send(Message::Notice {
                text: format!(
                    "Audio track not found in the stream; retrying download with {}",
                    if selector == "0:a?" {
                        "all audio languages".to_string()
                    } else {
                        format!("audio track {selector}")
                    }
                ),
                error: true,
            });
            temporary = tempfile::Builder::new()
                .prefix(".")
                .suffix(".part.mkv")
                .tempfile_in(&directory)?;
        }
        result = Some(
            run_ffmpeg(
                &ffmpeg_args(&stream, selector, temporary.path()),
                id,
                duration,
                &tx,
            )
            .await,
        );
        let stop = match &result {
            Some(Ok((_, tail))) => !tail.contains("matches no streams"),
            Some(Err(_)) => true,
            None => true,
        };
        if stop {
            break;
        }
    }
    let (success, tail) = result.expect("at least one ffmpeg attempt")?;
    if !success {
        bail!("Download failed: {tail}");
    }
    if temporary.as_file().metadata()?.len() == 0 {
        bail!("Download returned an empty file");
    }
    // Persist manifest first: a crash never leaves a complete video invisible to the scanner.
    let manifest = DownloadManifest {
        anime,
        episode,
        file: destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string(),
    };
    anime_levisione_core::store::atomic_write(
        &destination.with_extension("json"),
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

    #[cfg(unix)]
    #[tokio::test]
    async fn vlc_exit_zero_with_a_fatal_input_error_is_reported_as_failure() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir()?;
        let executable = directory.path().join("vlc");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf '%s\\n' 'HTTP 503 error' \"main input error: VLC is unable to open the MRL https://private.example/?token=SECRET\" >&2\nexit 0\n",
        )?;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;
        let config = Config {
            player: executable.to_string_lossy().into_owned(),
            ..Config::default()
        };
        let (tx, _) = mpsc::unbounded_channel();
        let error = play(config.clone(), request(None), tx)
            .await
            .expect_err("VLC lied about success");
        assert_eq!(
            error
                .downcast_ref::<media_error::MediaError>()
                .expect("typed error")
                .status,
            Some(503)
        );
        assert!(!format!("{error:#}").contains("SECRET"));
        std::fs::write(
            &executable,
            "#!/bin/sh\necho 'main decoder error: buffer deadlock prevented' >&2\nexit 0\n",
        )?;
        let (tx, _) = mpsc::unbounded_channel();
        play(config, request(None), tx).await?;
        Ok(())
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
    fn download_destination_lives_under_the_configured_directory() {
        let config = Config::default();
        let anime = serde_json::from_value(serde_json::json!({
            "provider": "animeworld", "id": "op", "title": "One Piece",
        }))
        .expect("anime");
        let episode = Episode {
            id: "9".into(),
            number: "12.5".into(),
            title: String::new(),
        };
        let target = destination(&config, &anime, &episode);
        assert_eq!(
            target,
            config
                .download_dir
                .join("One Piece [AW-op]")
                .join("Episode 12.5.mkv")
        );
    }

    fn download_request(audio: Option<AudioTrack>) -> DownloadRequest {
        DownloadRequest {
            id: 4,
            stream: Stream {
                url: "https://cdn.example/playlist/42?token=signed"
                    .parse()
                    .expect("url"),
                referer: "https://player.example/embed/42".into(),
            },
            anime: serde_json::from_value(serde_json::json!({
                "provider": "streamingcommunity", "id": "42-test", "title": "Test",
            }))
            .expect("anime"),
            episode: Episode {
                id: "9".into(),
                number: "3".into(),
                title: String::new(),
            },
            audio,
            duration: Some(1440.0),
        }
    }

    #[test]
    fn chosen_audio_selects_the_ffmpeg_stream_by_language_or_ordinal() {
        let request = download_request(Some(english()));
        let args = ffmpeg_args(
            &request.stream,
            &audio_map(request.audio.as_ref().expect("track")),
            Path::new("/tmp/out.mkv"),
        );
        let join = |args: &[String]| args.join(" ");
        assert!(join(&args).contains("-map 0:v? -map 0:a:m:language:eng -map 0:s?"));
        assert!(join(&args).contains("-progress pipe:1"));
        assert!(join(&args).contains("-nostats"));
        assert!(join(&args).ends_with("/tmp/out.mkv"));
        let unknown = AudioTrack {
            language: None,
            ..english()
        };
        assert_eq!(audio_map(&unknown), "0:a:1");
    }

    #[test]
    fn size_estimates_need_a_duration_and_processed_time() {
        assert_eq!(size_estimate(50, Some(5.0), Some(20.0)), Some(200));
        assert_eq!(size_estimate(50, Some(5.0), None), None);
        assert_eq!(size_estimate(50, None, Some(20.0)), None);
        // Early samples are mostly headers: no extrapolation before ~5 s.
        assert_eq!(size_estimate(50, Some(4.9), Some(20.0)), None);
        assert_eq!(size_estimate(50, Some(0.5), Some(20.0)), None);
        assert_eq!(size_estimate(50, Some(5.0), Some(0.0)), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ffmpeg_progress_reports_are_parsed_and_rate_limited() -> Result<()> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let script = "printf 'frame=1\ntotal_size=1200000\nout_time_us=6000000\nprogress=continue\nframe=2\ntotal_size=1400000\nout_time_us=7000000\nprogress=end\n'";
        let mut command = tokio::process::Command::new("/bin/sh");
        command
            .args(["-c", script])
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        let stdout = child.stdout.take().expect("stdout");
        report_progress(stdout, 4, Some(10.0), tx).await;
        let _ = child.wait().await;
        let first = rx.try_recv().expect("first block sends immediately");
        assert!(matches!(
            first,
            Message::DownloadProgress {
                request_id: 4,
                bytes: 1_200_000,
                total: Some(2_000_000),
                speed: None,
            }
        ));
        // The final block bypasses the throttle so the last size always lands.
        let final_report = rx.try_recv().expect("final block reports");
        assert!(matches!(
            final_report,
            Message::DownloadProgress {
                request_id: 4,
                bytes: 1_400_000,
                total: Some(2_000_000),
                speed: _,
            }
        ));
        assert!(rx.try_recv().is_err(), "nothing after the final block");
        Ok(())
    }
    #[test]
    fn missing_offline_folder_is_empty() -> Result<()> {
        let dir = tempfile::tempdir()?;
        assert!(scan_offline(&dir.path().join("missing"))?.is_empty());
        Ok(())
    }
}
