//! Bounded, local diagnostics. The schema deliberately accepts no free-form text:
//! never pass URLs, error messages, credentials, media metadata or library records.
use crate::model::ProviderId;
use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const MAX_BYTES: u64 = 256 * 1024;
const FILE_COUNT: usize = 4;
static LOG: OnceLock<Mutex<RotatingLog>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum DiagnosticOperation {
    Catalog,
    Search,
    Episodes,
    Resolve,
    DomainProbe,
    AudioProbe,
    Playback,
    Download,
    Sync,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum DiagnosticFailure {
    Timeout,
    Connect,
    Http,
    Io,
    Data,
    State,
    Argument,
    Memory,
    Security,
    Other,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum DiagnosticOutcome {
    Success,
    Failed,
    Cancelled,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum DiagnosticComponent {
    Ui,
    Provider,
    Audio,
    Store,
    Sync,
    Playback,
    Catalog,
    Repository,
    Offline,
    Main,
    Diagnostics,
    Other,
}

/// Only fixed enums, booleans and technical counters cross this privacy boundary.
#[derive(Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum DiagnosticEvent {
    SessionStarted,
    SessionEnded,
    Panic {
        component: DiagnosticComponent,
        line: u32,
    },
    AndroidEnvironment {
        api: u32,
    },
    AndroidCrash {
        kind: DiagnosticFailure,
    },
    AndroidCrashFrame {
        component: DiagnosticComponent,
        line: u32,
    },
    OperationStarted {
        id: u64,
        operation: DiagnosticOperation,
        provider: Option<ProviderId>,
    },
    OperationFinished {
        id: u64,
        outcome: DiagnosticOutcome,
        elapsed_ms: u64,
        failure: Option<DiagnosticFailure>,
    },
    HttpResponse {
        id: u64,
        provider: Option<ProviderId>,
        attempt: u32,
        status: u16,
        elapsed_ms: u64,
        retry: bool,
    },
    HttpFailure {
        id: u64,
        provider: Option<ProviderId>,
        attempt: u32,
        kind: DiagnosticFailure,
        elapsed_ms: u64,
        retry: bool,
    },
    ProviderDeadline,
    CoreFailure {
        kind: DiagnosticFailure,
    },
    PlayerOpened {
        session: u64,
    },
    PlayerClosed {
        session: u64,
    },
    PlayerProcessExit {
        session: u64,
        code: Option<i32>,
        success: bool,
    },
    PlayerBuffering {
        session: u64,
        buffered_ms: u64,
    },
    PlayerReady {
        session: u64,
        stall_ms: u64,
    },
    PlayerFailure {
        session: u64,
        code: u32,
        kind: DiagnosticFailure,
    },
    MediaLoadFailure {
        session: u64,
        status: u16,
        kind: DiagnosticFailure,
        cancelled: bool,
    },
    Bandwidth {
        session: u64,
        bits_per_second: u64,
    },
    ExportRequested,
    MediaFailure {
        operation: DiagnosticOperation,
        kind: DiagnosticFailure,
        status: Option<u16>,
    },
}

pub fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Call once per process, before requests. Logging failure must not stop the app.
pub fn initialize(directory: &Path) -> io::Result<()> {
    let logger = RotatingLog::new(directory.to_path_buf(), MAX_BYTES)?;
    LOG.set(Mutex::new(logger)).map_err(|_| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Diagnostics already initialized",
        )
    })?;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        record(DiagnosticEvent::Panic {
            component: panic_component(info.location().map_or("", |l| l.file())),
            line: info.location().map_or(0, |l| l.line()),
        });
        // Preserve normal terminal/system crash reporting, but never persist the payload.
        previous(info);
    }));
    record(DiagnosticEvent::SessionStarted);
    Ok(())
}

fn panic_component(file: &str) -> DiagnosticComponent {
    // Match fixed source suffixes, never persist local build paths or panic payloads.
    let file = file.replace('\\', "/");
    if file.ends_with("crates/tui/src/ui.rs") {
        DiagnosticComponent::Ui
    } else if file.ends_with("crates/tui/src/app.rs") {
        DiagnosticComponent::Catalog
    } else if file.ends_with("crates/tui/src/main.rs") {
        DiagnosticComponent::Main
    } else if file.ends_with("crates/tui/src/playback.rs") {
        DiagnosticComponent::Playback
    } else if file.ends_with("crates/core/src/audio.rs") {
        DiagnosticComponent::Audio
    } else if file.ends_with("crates/core/src/store.rs") {
        DiagnosticComponent::Store
    } else if file.ends_with("crates/core/src/sync.rs") {
        DiagnosticComponent::Sync
    } else if file.contains("crates/core/src/provider/") {
        DiagnosticComponent::Provider
    } else if file.ends_with("crates/core/src/api.rs") {
        DiagnosticComponent::Repository
    } else if file.ends_with("crates/core/src/diagnostics.rs") {
        DiagnosticComponent::Diagnostics
    } else {
        DiagnosticComponent::Other
    }
}

pub fn record(event: DiagnosticEvent) {
    if let Some(log) = LOG.get() {
        // try_lock prevents a panic in the writer from deadlocking the panic hook.
        // Normal writers serialize; only the panic hook uses the nonblocking path below.
        if let Some(mut log) = if matches!(event, DiagnosticEvent::Panic { .. }) {
            log.try_lock().ok()
        } else {
            log.lock().ok()
        } {
            let _ = log.append(&event);
        }
    }
}

pub fn snapshot() -> io::Result<String> {
    let log = LOG
        .get()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Diagnostics unavailable"))?;
    log.lock()
        .map_err(|_| io::Error::other("Diagnostics unavailable"))?
        .snapshot()
}

/// Read-only CLI export; does not need the library lock or parse user metadata.
/// Across processes this is a best-effort snapshot if the writer rotates mid-read.
pub fn read_snapshot(directory: &Path) -> io::Result<String> {
    RotatingLog {
        directory: directory.to_path_buf(),
        max_bytes: MAX_BYTES,
    }
    .snapshot()
}

pub fn failure(error: &anyhow::Error) -> DiagnosticFailure {
    for cause in error.chain() {
        if let Some(e) = cause.downcast_ref::<crate::media_error::MediaError>() {
            return e.kind;
        }
        if let Some(e) = cause.downcast_ref::<reqwest::Error>() {
            return if e.is_timeout() {
                DiagnosticFailure::Timeout
            } else if e.is_connect() {
                DiagnosticFailure::Connect
            } else if e.is_status() {
                DiagnosticFailure::Http
            } else if e.is_decode() {
                DiagnosticFailure::Data
            } else {
                DiagnosticFailure::Io
            };
        }
        if let Some(e) = cause.downcast_ref::<io::Error>() {
            return if e.kind() == io::ErrorKind::TimedOut {
                DiagnosticFailure::Timeout
            } else {
                DiagnosticFailure::Io
            };
        }
        if cause.is::<serde_json::Error>() {
            return DiagnosticFailure::Data;
        }
    }
    DiagnosticFailure::Other
}

/// A dropped future records cancellation, including rapid player replacement.
pub struct Operation {
    id: u64,
    start: Instant,
    finished: bool,
}
impl Operation {
    pub fn new(operation: DiagnosticOperation, provider: Option<ProviderId>) -> Self {
        let id = next_id();
        record(DiagnosticEvent::OperationStarted {
            id,
            operation,
            provider,
        });
        Self {
            id,
            start: Instant::now(),
            finished: false,
        }
    }
    pub fn finish<T>(&mut self, result: &anyhow::Result<T>) {
        self.finished = true;
        record(DiagnosticEvent::OperationFinished {
            id: self.id,
            outcome: if result.is_ok() {
                DiagnosticOutcome::Success
            } else {
                DiagnosticOutcome::Failed
            },
            elapsed_ms: self.start.elapsed().as_millis() as u64,
            failure: result.as_ref().err().map(failure),
        });
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        if !self.finished {
            record(DiagnosticEvent::OperationFinished {
                id: self.id,
                outcome: DiagnosticOutcome::Cancelled,
                elapsed_ms: self.start.elapsed().as_millis() as u64,
                failure: None,
            });
        }
    }
}
pub async fn observe<T>(
    operation: DiagnosticOperation,
    provider: Option<ProviderId>,
    future: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    let mut span = Operation::new(operation, provider);
    let result = future.await;
    span.finish(&result);
    result
}

struct RotatingLog {
    directory: PathBuf,
    max_bytes: u64,
}
impl RotatingLog {
    fn new(directory: PathBuf, max_bytes: u64) -> io::Result<Self> {
        fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
        let log = Self {
            directory,
            max_bytes,
        };
        // Detect unwritable storage at initialization, not after the first event.
        log.open()?.flush()?;
        Ok(log)
    }
    fn path(&self, index: usize) -> PathBuf {
        self.directory.join(if index == 0 {
            "diagnostics.jsonl".into()
        } else {
            format!("diagnostics.{index}.jsonl")
        })
    }
    fn open(&self) -> io::Result<fs::File> {
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(self.path(0))
    }
    fn append(&mut self, event: &DiagnosticEvent) -> io::Result<()> {
        #[derive(Serialize)]
        struct Entry<'a> {
            schema: u32,
            timestamp_ms: u64,
            process: u32,
            version: &'static str,
            os: &'static str,
            arch: &'static str,
            #[serde(flatten)]
            event: &'a DiagnosticEvent,
        }
        let mut line = serde_json::to_vec(&Entry {
            schema: 1,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            process: std::process::id(),
            version: env!("CARGO_PKG_VERSION"),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            event,
        })?;
        line.push(b'\n');
        if line.len() as u64 > self.max_bytes {
            return Err(io::Error::other("Diagnostic entry too large"));
        }
        let size = match fs::metadata(self.path(0)) {
            Ok(metadata) => metadata.len(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e),
        };
        if size + line.len() as u64 > self.max_bytes {
            for index in (1..FILE_COUNT).rev() {
                let destination = self.path(index);
                match fs::remove_file(&destination) {
                    Ok(()) => (),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e),
                }
                match fs::rename(self.path(index - 1), destination) {
                    Ok(()) => (),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e),
                }
            }
        }
        // No retained handle: rotation also works on Windows. Each event is flushed
        // to the OS immediately, so an application crash does not lose a queue.
        self.open()?.write_all(&line)
    }
    fn snapshot(&self) -> io::Result<String> {
        let mut bytes = Vec::new();
        for index in (0..FILE_COUNT).rev() {
            match fs::File::open(self.path(index)) {
                Ok(file) => {
                    file.take(self.max_bytes).read_to_end(&mut bytes)?;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
            }
        }
        String::from_utf8(bytes).map_err(|_| io::Error::other("Invalid diagnostics"))
    }
}

#[cfg(feature = "uniffi")]
#[uniffi::export]
pub fn diagnostics_initialize(directory: String) -> crate::api::ApiResult<()> {
    initialize(Path::new(&directory)).map_err(|_| anyhow::anyhow!("Diagnostics unavailable").into())
}
#[cfg(feature = "uniffi")]
#[uniffi::export]
pub fn diagnostics_record(event: DiagnosticEvent) {
    record(event);
}
#[cfg(feature = "uniffi")]
#[uniffi::export]
pub fn diagnostics_snapshot() -> crate::api::ApiResult<String> {
    snapshot().map_err(|_| anyhow::anyhow!("Diagnostics unavailable").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_is_bounded_chronological_and_survives_reopen() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let mut log = RotatingLog::new(temp.path().to_path_buf(), 600)?;
        for session in 0..100 {
            log.append(&DiagnosticEvent::PlayerOpened { session })?;
        }
        let snapshot = log.snapshot()?;
        let entries = snapshot
            .lines()
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            entries.last().map(|v| v["session"].as_u64()),
            Some(Some(99))
        );
        assert!(
            entries
                .windows(2)
                .all(|p| p[0]["session"].as_u64() < p[1]["session"].as_u64())
        );
        assert_eq!(fs::read_dir(temp.path())?.count(), FILE_COUNT);
        for file in fs::read_dir(temp.path())? {
            assert!(file?.metadata()?.len() <= 600);
        }
        assert!(snapshot.len() <= FILE_COUNT * 600);
        let reopened = RotatingLog::new(temp.path().to_path_buf(), 600)?;
        assert_eq!(snapshot, reopened.snapshot()?);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(temp.path())?.permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(log.path(0))?.permissions().mode() & 0o777,
                0o600
            );
        }
        Ok(())
    }

    #[test]
    fn sensitive_error_payloads_never_enter_the_schema() -> anyhow::Result<()> {
        let secret =
            "https://secret.example/film?token=123 alib1.private /Users/person library title";
        let error = anyhow::anyhow!(secret);
        let temp = tempfile::tempdir()?;
        let mut log = RotatingLog::new(temp.path().to_path_buf(), MAX_BYTES)?;
        log.append(&DiagnosticEvent::CoreFailure {
            kind: failure(&error),
        })?;
        let snapshot = log.snapshot()?;
        assert!(!snapshot.contains(secret));
        for fragment in [
            "https:",
            "alib1",
            "/Users/",
            "secret.example",
            "library title",
        ] {
            assert!(!snapshot.contains(fragment));
        }
        assert!(snapshot.contains("other"));
        Ok(())
    }

    #[test]
    fn unavailable_storage_is_nonfatal_and_snapshot_ignores_unrelated_files() -> anyhow::Result<()>
    {
        let temp = tempfile::tempdir()?;
        let blocked = temp.path().join("file");
        fs::write(&blocked, b"not a directory")?;
        assert!(RotatingLog::new(blocked, MAX_BYTES).is_err());
        let mut log = RotatingLog::new(temp.path().join("logs"), MAX_BYTES)?;
        fs::write(log.directory.join("credentials.json"), b"secret")?;
        log.append(&DiagnosticEvent::SessionStarted)?;
        assert!(!log.snapshot()?.contains("secret"));
        // Transient I/O failures are recoverable on the next event.
        fs::remove_file(log.path(0))?;
        fs::create_dir(log.path(0))?;
        assert!(log.append(&DiagnosticEvent::SessionStarted).is_err());
        fs::remove_dir(log.path(0))?;
        log.append(&DiagnosticEvent::SessionEnded)?;
        assert!(log.snapshot()?.contains("session_ended"));
        Ok(())
    }

    #[test]
    fn concurrent_writes_and_snapshots_remain_bounded_valid_json() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let log = std::sync::Arc::new(Mutex::new(RotatingLog::new(
            temp.path().to_path_buf(),
            1024,
        )?));
        std::thread::scope(|scope| {
            let mut tasks = Vec::new();
            for _ in 0..8 {
                let log = log.clone();
                tasks.push(scope.spawn(move || {
                    for session in 0..100 {
                        log.lock()
                            .expect("writer")
                            .append(&DiagnosticEvent::PlayerOpened { session })
                            .expect("append");
                        let snapshot = log.lock().expect("reader").snapshot().expect("snapshot");
                        assert!(snapshot.len() <= FILE_COUNT * 1024);
                        for line in snapshot.lines() {
                            serde_json::from_str::<serde_json::Value>(line).expect("valid JSON");
                        }
                    }
                }));
            }
            for task in tasks {
                task.join().expect("worker");
            }
        });
        Ok(())
    }
}
