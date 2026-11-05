//! Explicit real-player regression: never opens VLC windows or uses user settings.
use anime_levisione::playback::{self, PlayRequest};
use anime_levisione_core::{
    diagnostics,
    media_error::MediaError,
    model::Episode,
    provider::{Media, Stream},
    store::Config,
};
use std::sync::{
    Arc,
    atomic::{AtomicU16, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
};

fn request(media: Media, id: u64) -> anyhow::Result<PlayRequest> {
    Ok(PlayRequest {
        id,
        media,
        anime: serde_json::from_value(
            serde_json::json!({"provider":"animeunity","id":"synthetic","title":"Private title sentinel"}),
        )?,
        episode: Episode {
            id: "private-episode".into(),
            number: "1".into(),
            title: String::new(),
        },
        resume: 0.0,
        audio: None,
        audio_warning: None,
    })
}

#[tokio::test]
#[ignore = "requires installed VLC; set ANIME_LEVISIONE_TEST_VLC to its executable"]
async fn real_vlc_http_failures_are_not_successes_and_never_log_the_media_url() -> anyhow::Result<()>
{
    let directory = tempfile::tempdir()?;
    diagnostics::initialize(directory.path())?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let status = Arc::new(AtomicU16::new(403));
    let reply_status = status.clone();
    let server = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let code = reply_status.load(Ordering::Relaxed);
            tokio::spawn(async move {
                let mut bytes = [0; 4096];
                let _ = socket.read(&mut bytes).await;
                let reply = format!(
                    "HTTP/1.1 {code} Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            });
        }
    });
    let config = Config {
        player: std::env::var("ANIME_LEVISIONE_TEST_VLC").unwrap_or_else(|_| "vlc".into()),
        player_args: vec![
            "--ignore-config".into(),
            "--intf=dummy".into(),
            "--no-video".into(),
            "--aout=dummy".into(),
        ],
        ..Config::default()
    };
    for code in [403, 503] {
        status.store(code, Ordering::Relaxed);
        let request = request(
            Media::Remote(Stream {
                url: format!("http://{address}/private.mp4?token=SENSITIVE_SENTINEL").parse()?,
                referer: "https://private.example/".into(),
            }),
            u64::from(code),
        )?;
        let (tx, _) = mpsc::unbounded_channel();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            playback::play(config.clone(), request, tx),
        )
        .await;
        let error =
            result?.expect_err("VLC must not turn a failed source into a successful playback");
        assert_eq!(
            error
                .downcast_ref::<MediaError>()
                .expect("safe media error")
                .status,
            Some(code)
        );
    }
    server.abort();
    let mut valid_config = config;
    valid_config.player_args.push("--stop-time=1".into());
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../android/app/src/androidTest/assets/bilingual.mp4");
    let (tx, _) = mpsc::unbounded_channel();
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        playback::play(valid_config, request(Media::Local(fixture), 7)?, tx),
    )
    .await??;
    let logs = diagnostics::snapshot()?;
    for sentinel in [
        "SENSITIVE_SENTINEL",
        "private.mp4",
        "private.example",
        "Private title sentinel",
        "private-episode",
        "127.0.0.1",
    ] {
        assert!(!logs.contains(sentinel), "sensitive data in diagnostics");
    }
    let failures = logs
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        failures
            .iter()
            .filter(|e| e["event"] == "media_failure")
            .count(),
        2
    );
    Ok(())
}
