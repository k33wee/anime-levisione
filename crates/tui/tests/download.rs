//! Real-ffmpeg download regression: audio-track selection, progress reports,
//! manifest and offline discovery. Skipped when ffmpeg is not installed.

use anime_levisione::events::Message;
use anime_levisione::playback::{self, DownloadRequest};
use anime_levisione_core::{
    audio::{self, AudioTrack},
    model::Episode,
    provider::{Media, Stream},
    store::Config,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
};

fn fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../android/app/src/androidTest/assets/bilingual.mp4")
}

async fn tool_available(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Minimal HTTP file server: answers every request with the fixture body.
async fn serve_fixture(body: Arc<Vec<u8>>, hits: Arc<AtomicUsize>) -> anyhow::Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let body = body.clone();
            let hits = hits.clone();
            tokio::spawn(async move {
                let mut bytes = [0; 4096];
                let _ = socket.read(&mut bytes).await;
                hits.fetch_add(1, Ordering::Relaxed);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });
    Ok(format!("http://{address}/bilingual.mp4"))
}

fn english() -> AudioTrack {
    AudioTrack {
        language: Some("eng".into()),
        label: "English".into(),
        index: 1,
        default: false,
    }
}

fn request(url: &str, audio: Option<AudioTrack>, duration: Option<f64>) -> DownloadRequest {
    DownloadRequest {
        id: 11,
        stream: Stream {
            url: url.parse().expect("test url"),
            referer: "https://player.example/embed/42".into(),
        },
        anime: serde_json::from_value(serde_json::json!({
            "provider": "animeunity", "id": "synthetic", "title": "Download Test",
        }))
        .expect("anime"),
        episode: Episode {
            id: "9".into(),
            number: "1".into(),
            title: String::new(),
        },
        audio,
        duration,
    }
}

#[tokio::test]
async fn ffmpeg_download_keeps_only_the_chosen_language_and_reports_progress() -> anyhow::Result<()>
{
    if !tool_available("ffmpeg").await || !tool_available("ffprobe").await {
        eprintln!("skipping: ffmpeg/ffprobe not installed");
        return Ok(());
    }
    let directory = tempfile::tempdir()?;
    let body = Arc::new(std::fs::read(fixture())?);
    let hits = Arc::new(AtomicUsize::new(0));
    let url = serve_fixture(body.clone(), hits.clone()).await?;
    let config = Config {
        download_dir: directory.path().to_path_buf(),
        ..Config::default()
    };
    let duration = audio::duration(&Media::Local(fixture())).await?;
    assert!(
        (duration - 30.0).abs() < 0.5,
        "fixture duration: {duration}"
    );
    let (tx, mut rx) = mpsc::unbounded_channel();
    let saved = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        playback::download(
            config.clone(),
            request(&url, Some(english()), Some(duration)),
            tx,
        ),
    )
    .await??;
    let path = Path::new(&saved);
    assert!(path.is_file(), "missing {saved}");
    // The chosen language survives: exactly one audio stream, tagged eng.
    let probe = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream_tags=language",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .await?;
    assert!(probe.status.success());
    let meta: serde_json::Value = serde_json::from_slice(&probe.stdout)?;
    let languages: Vec<&str> = meta["streams"]
        .as_array()
        .expect("audio streams")
        .iter()
        .map(|s| s["tags"]["language"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(languages, ["eng"], "wrong audio in download");
    // Progress reached the UI at least once (tiny files may legitimately
    // report zero bytes before their single final block).
    let mut reported = 0;
    while let Ok(message) = rx.try_recv() {
        if matches!(message, Message::DownloadProgress { .. }) {
            reported += 1;
        }
    }
    assert!(reported >= 1, "no progress reports delivered");
    assert!(hits.load(Ordering::Relaxed) >= 1, "server never hit");
    // The manifest makes the episode discoverable in the offline library.
    let offline = playback::scan_offline(directory.path())?;
    assert_eq!(offline.len(), 1);
    assert_eq!(offline[0].episode.number, "1");
    // A repeat download refuses to overwrite instead of redownloading.
    let (tx, _) = mpsc::unbounded_channel();
    let repeat = playback::download(config, request(&url, None, None), tx).await;
    assert!(
        repeat
            .expect_err("second download")
            .to_string()
            .contains("Already downloaded")
    );
    Ok(())
}
