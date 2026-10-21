use crate::{
    events::Message,
    model::{Anime, Episode},
    provider::{Stream, http::USER_AGENT},
    store::{Config, Progress},
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

#[derive(Clone)]
pub enum Media {
    Remote(Stream),
    Local(PathBuf),
}
impl Media {
    pub fn source(&self) -> String {
        match self {
            Self::Remote(s) => s.url.to_string(),
            Self::Local(p) => p.to_string_lossy().into_owned(),
        }
    }
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
    crate::model::clean(&String::from_utf8_lossy(&tail))
}
pub async fn play(
    config: Config,
    media: Media,
    anime: Anime,
    episode: Episode,
    resume: f64,
    tx: mpsc::UnboundedSender<Message>,
) -> Result<()> {
    let mpv = is_mpv(&config);
    let temp = tempfile::Builder::new()
        .prefix("anime-levisione-")
        .tempdir()?;
    let socket = temp.path().join("mpv.sock");
    let mut command = Command::new(&config.player);
    command
        .args(&config.player_args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if mpv {
        command
            .arg("--no-terminal")
            .arg("--keep-open=no")
            .arg(format!(
                "--force-media-title={} · Episode {}",
                anime.title, episode.number
            ));
        #[cfg(unix)]
        command.arg(format!("--input-ipc-server={}", socket.display()));
        if resume.is_finite() && resume > 3.0 {
            command.arg(format!("--start={resume:.0}"));
        }
        if let Media::Remote(s) = &media {
            command
                .arg(format!("--referrer={}", s.referer))
                .arg(format!("--user-agent={USER_AGENT}"));
        }
        command.arg("--");
    } else if Path::new(&config.player)
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("vlc"))
    {
        command.arg("--play-and-exit");
        if resume.is_finite() && resume > 3.0 {
            command.arg(format!("--start-time={resume:.0}"));
        }
        if let Media::Remote(s) = &media {
            command
                .arg(format!("--http-referrer={}", s.referer))
                .arg(format!("--http-user-agent={USER_AGENT}"));
        }
        command.arg("--");
    }
    command.arg(media.source());
    let mut child = command.spawn().with_context(|| {
        format!(
            "Cannot start '{}'; install mpv or set player in config.toml",
            config.player
        )
    })?;
    let _ = tx.send(Message::PlaybackStarted);
    let stderr = child.stderr.take().context("Player stderr unavailable")?;
    let diagnostic = tokio::spawn(diagnostic_tail(stderr));
    #[cfg(unix)]
    let monitor = if mpv {
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
    crate::store::atomic_write(&manifest_path, &serde_json::to_vec_pretty(&manifest)?)?;
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
