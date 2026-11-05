use anime_levisione_core::{
    diagnostics::{self, DiagnosticEvent, DiagnosticFailure, DiagnosticOperation},
    provider::http::Http,
};
use std::{
    io::{Read, Write},
    net::TcpListener,
};

#[tokio::test]
async fn real_requests_cancellations_and_panics_never_persist_sensitive_text() -> anyhow::Result<()>
{
    let temp = tempfile::tempdir()?;
    diagnostics::initialize(temp.path())?;
    let secret = "SENSITIVE-title-alib1.connection-token";
    let server = TcpListener::bind("127.0.0.1:0")?;
    let address = server.local_addr()?;
    let worker = std::thread::spawn(move || -> std::io::Result<()> {
        // One retry, then success. Request URL and response body must not enter logs.
        for status in [503, 200] {
            let (mut socket, _) = server.accept()?;
            socket.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
            let mut request = [0; 4096];
            let _ = socket.read(&mut request)?;
            write!(
                socket,
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{secret}",
                secret.len()
            )?;
        }
        Ok(())
    });
    let http = Http::new()?;
    assert_eq!(
        http.text(
            http.client
                .get(format!("http://{address}/{secret}?token={secret}"))
        )
        .await?,
        secret
    );
    worker.join().expect("fixture server")?;

    let (started, receive) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async {
        let _span = diagnostics::Operation::new(DiagnosticOperation::Playback, None);
        let _ = started.send(());
        std::future::pending::<()>().await;
    });
    receive.await?;
    task.abort();
    assert!(task.await.is_err());
    assert!(std::panic::catch_unwind(|| panic!("{secret}")).is_err());
    diagnostics::record(DiagnosticEvent::CoreFailure {
        kind: DiagnosticFailure::Other,
    });
    let output = diagnostics::snapshot()?;
    assert!(!output.contains(secret));
    assert!(!output.contains("127.0.0.1"));
    assert!(!output.contains("http://"));
    assert!(output.contains("\"status\":503"));
    assert!(output.contains("\"retry\":true"));
    assert!(output.contains("\"status\":200"));
    assert!(output.contains("\"outcome\":\"cancelled\""));
    assert!(output.contains("\"event\":\"panic\""));
    for line in output.lines() {
        serde_json::from_str::<serde_json::Value>(line)?;
    }
    Ok(())
}
