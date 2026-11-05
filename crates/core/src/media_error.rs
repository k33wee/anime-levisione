//! Read subprocess diagnostics in memory, but expose only fixed categories/codes.
//! In particular VLC can return exit code 0 after failing to open its input.
use crate::diagnostics::{self, DiagnosticEvent, DiagnosticFailure, DiagnosticOperation};
use std::fmt;
use tokio::io::{AsyncRead, AsyncReadExt};

const MAX_TAIL: usize = 4096;

#[derive(Clone, Copy, Debug)]
pub struct MediaError {
    pub kind: DiagnosticFailure,
    pub status: Option<u16>,
}
impl MediaError {
    pub fn retryable(&self) -> bool {
        match self.kind {
            DiagnosticFailure::Timeout | DiagnosticFailure::Connect => true,
            DiagnosticFailure::Http => self.status.is_none_or(|status| {
                matches!(status, 401 | 403 | 404 | 408 | 410 | 419 | 429 | 500..=599)
            }),
            _ => false,
        }
    }
    pub fn record(&self, operation: DiagnosticOperation) {
        diagnostics::record(DiagnosticEvent::MediaFailure {
            operation,
            kind: self.kind,
            status: self.status,
        });
    }
}
impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            DiagnosticFailure::Http => match self.status {
                Some(status) => write!(
                    f,
                    "Media server returned HTTP {status}; retry the episode or change provider"
                ),
                None => f.write_str(
                    "Media server rejected the request; retry the episode or change provider",
                ),
            },
            DiagnosticFailure::Timeout => f.write_str("Media server timed out; retry the episode"),
            DiagnosticFailure::Connect => {
                f.write_str("Cannot connect to the media server; retry the episode")
            }
            DiagnosticFailure::Data => f.write_str("Could not inspect audio metadata"),
            _ => {
                f.write_str("Player could not open the media; retry the episode or change provider")
            }
        }
    }
}
impl std::error::Error for MediaError {}

// Deliberately neither Debug nor Serialize: tail can contain a signed media URL.
#[derive(Default)]
pub struct Output {
    pub tail: String,
    pub failure: Option<MediaError>,
    input_failed: bool,
}
impl Output {
    pub fn vlc_failure(&self) -> Option<MediaError> {
        self.input_failed
            .then_some(self.failure.unwrap_or(MediaError {
                kind: DiagnosticFailure::Io,
                status: None,
            }))
    }
}

fn classify(raw: &str) -> Option<MediaError> {
    // Match technical prefixes, not arbitrary numbers appearing in a URL/token.
    http_status(raw)
        .map(|status| MediaError {
            kind: DiagnosticFailure::Http,
            status: Some(status),
        })
        .or_else(|| {
            if raw.contains("Server returned 4XX") || raw.contains("Server returned 5XX") {
                Some(MediaError {
                    kind: DiagnosticFailure::Http,
                    status: None,
                })
            } else if raw.contains("Connection timed out") || raw.contains("Operation timed out") {
                Some(MediaError {
                    kind: DiagnosticFailure::Timeout,
                    status: None,
                })
            } else if raw.contains("Connection refused")
                || raw.contains("Failed to resolve hostname")
                || raw.contains("No route to host")
            {
                Some(MediaError {
                    kind: DiagnosticFailure::Connect,
                    status: None,
                })
            } else {
                None
            }
        })
}
fn http_status(raw: &str) -> Option<u16> {
    for marker in [
        "Server returned ",
        "HTTP ",
        "HTTP/1.0 ",
        "HTTP/1.1 ",
        "HTTP/2 ",
        "HTTP error ",
    ] {
        for (offset, prefix) in raw.match_indices(marker) {
            let rest = &raw[offset + prefix.len()..];
            if let Some(code) = rest
                .get(..3)
                .and_then(|s| s.parse::<u16>().ok())
                .filter(|code| (400..600).contains(code))
                && rest.as_bytes().get(3).is_none_or(|b| !b.is_ascii_digit())
            {
                return Some(code);
            }
        }
    }
    None
}

fn vlc_input_failed(raw: &str) -> bool {
    raw.lines().any(|line| {
        let Some((_, message)) = line.split_once("main input error:") else {
            return false;
        };
        // VLC translates its error dialog, but retains the technical MRL label.
        // Only inspect the message before a URI, never an MRL-like URL/token.
        message
            .trim_start()
            .starts_with("Your input can't be opened")
            || message
                .split("://")
                .next()
                .is_some_and(|prefix| prefix.contains("MRL"))
    })
}

/// Always drain the pipe; retain only a small tail and safe findings. Findings
/// survive long/localized stderr and markers split across read boundaries.
pub async fn read(reader: impl AsyncRead + Unpin) -> Output {
    let mut output = Output::default();
    drain(reader, &mut output).await;
    output
}

/// Own both futures: cancellation closes stderr, with no detached reader task.
/// A helper process may inherit stderr after the player exits; never wait for it
/// indefinitely, but retain safe findings already observed before the deadline.
pub async fn capture<T>(
    reader: impl AsyncRead + Unpin,
    work: impl std::future::Future<Output = T>,
) -> (T, Output) {
    let mut output = Output::default();
    let result = {
        let drain = drain(reader, &mut output);
        tokio::pin!(drain, work);
        tokio::select! {
            result = &mut work => {
                let _ = tokio::time::timeout(std::time::Duration::from_millis(500), &mut drain).await;
                result
            }
            () = &mut drain => work.await,
        }
    };
    (result, output)
}

async fn drain(reader: impl AsyncRead + Unpin, output: &mut Output) {
    let mut reader = reader;
    let mut chunk = [0; 1024];
    let mut tail = Vec::new();
    while let Ok(count) = reader.read(&mut chunk).await {
        if count == 0 {
            break;
        }
        tail.extend_from_slice(&chunk[..count]);
        let text = String::from_utf8_lossy(&tail);
        if let Some(error) = classify(&text)
            && (error.status.is_some() || output.failure.is_none())
        {
            output.failure = Some(error);
        }
        output.input_failed |= vlc_input_failed(&text);
        if tail.len() > MAX_TAIL {
            tail.drain(..tail.len() - MAX_TAIL);
        }
    }
    output.tail = crate::model::clean(&String::from_utf8_lossy(&tail));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn vlc_input_failure_is_not_a_success_even_with_exit_zero() {
        let text = b"[id] access stream error: HTTP 503 error\n[id] main input error: VLC is unable to open the MRL 'https://private.example/?token=SECRET'\n";
        let output = read(&text[..]).await;
        let error = output.vlc_failure().expect("fatal input");
        assert_eq!(error.status, Some(503));
        assert!(error.retryable());
        assert!(!error.to_string().contains("SECRET"));
        assert!(!error.to_string().contains("private.example"));
    }
    #[tokio::test]
    async fn recoverable_decoder_and_http_warnings_are_not_fatal_input_errors() {
        let text = b"HTTP 503 error\nmain decoder error: buffer deadlock prevented\nmain blend error: blending failed\n";
        assert!(read(&text[..]).await.vlc_failure().is_none());
        assert!(read(&b""[..]).await.vlc_failure().is_none());
    }
    #[tokio::test]
    async fn findings_survive_truncation_and_the_entire_pipe_is_drained() {
        let mut text =
            b"HTTP 403 error\n[id] main input error: Your input can't be opened\n".to_vec();
        text.extend(vec![b'x'; 128 * 1024]);
        let output = read(text.as_slice()).await;
        assert_eq!(output.vlc_failure().expect("input error").status, Some(403));
        assert!(output.tail.len() <= MAX_TAIL);
    }
    #[tokio::test]
    async fn localized_vlc_errors_and_split_markers_are_recognized() {
        let mut text = vec![b'x'; 1019];
        text.extend_from_slice(b"\nHTTP/1.1 503 Service Unavailable\n[id] main input error: VLC non riesce ad aprire il MRL 'https://private.example/'\n");
        assert_eq!(
            read(text.as_slice())
                .await
                .vlc_failure()
                .expect("Italian input error")
                .status,
            Some(503)
        );
        assert!(!vlc_input_failed(
            "main input error: subtitle unavailable at https://private.example/MRL"
        ));
    }

    #[tokio::test]
    async fn inherited_open_stderr_does_not_hide_a_failure_or_hang_completion() {
        use tokio::io::AsyncWriteExt;
        let (mut writer, reader) = tokio::io::duplex(4096);
        writer
            .write_all(b"HTTP 503 error\nmain input error: Your input can't be opened\n")
            .await
            .expect("write");
        let started = std::time::Instant::now();
        let (_, output) = capture(reader, std::future::ready(())).await;
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(
            output.vlc_failure().expect("retained failure").status,
            Some(503)
        );
        assert!(writer.write_all(b"closed").await.is_err());
    }

    #[tokio::test]
    async fn cancelling_capture_closes_the_pipe_without_a_detached_reader() {
        use tokio::io::AsyncWriteExt;
        let (mut writer, reader) = tokio::io::duplex(4096);
        let task = tokio::spawn(capture(reader, std::future::pending::<()>()));
        tokio::task::yield_now().await;
        task.abort();
        assert!(matches!(task.await, Err(e) if e.is_cancelled()));
        assert!(writer.write_all(b"closed").await.is_err());
    }

    #[test]
    fn ffprobe_status_families_and_url_digits_are_safe() {
        assert_eq!(
            classify("Server returned 403 Forbidden")
                .expect("HTTP")
                .status,
            Some(403)
        );
        assert!(matches!(
            classify("Server returned 5XX Server Error reply")
                .expect("HTTP")
                .kind,
            DiagnosticFailure::Http
        ));
        assert!(classify("https://example/403?token=503&HTTP%20500").is_none());
        assert!(classify("HTTP 5030").is_none());
    }
}
