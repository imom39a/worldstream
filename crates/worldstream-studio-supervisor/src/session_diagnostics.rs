//! Secret-free, release-build diagnostics for synchronous hosted session calls.
//! No request, response body, identity, token, or arbitrary error text is accepted.
use std::{
    cell::Cell,
    fs,
    io::{self, ErrorKind, Seek as _, Write as _},
    path::Path,
    sync::{
        Mutex, OnceLock, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use worldstream_runtime::{create_owner_only_file, validate_owner_only_file};

/// Fixed managed-Controller sink name under its protected state directory.
pub const HOSTED_SESSION_DIAGNOSTIC_FILE: &str = "hosted-session-diagnostics.ndjson";
const MAX_DIAGNOSTIC_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Operation {
    Issue,
    Redeem,
    Status,
    Logout,
    StreamTicket,
}

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage {
    Session,
    Authority,
    Membership,
    Client,
    RuntimeTicket,
}

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Detail {
    Complete,
    Connect,
    Write,
    Read,
    Parse,
    Body,
    Validation,
    Http,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Category {
    Ok,
    Invalid,
    Rejected,
    Missing,
    Capacity,
    Unavailable,
    Timeout,
    IoFailure,
    InvalidResponse,
}

#[derive(Clone, Copy, serde::Serialize)]
struct Context {
    call: u64,
    operation: Operation,
    stage: Stage,
}

thread_local! { static CURRENT: Cell<Option<Context>> = const { Cell::new(None) }; }
#[cfg(test)]
thread_local! { static CAPTURE: std::cell::RefCell<Option<Vec<serde_json::Value>>> = const { std::cell::RefCell::new(None) }; }
static NEXT_CALL: AtomicU64 = AtomicU64::new(1);
static FILE_SINK: OnceLock<BoundedFileSink> = OnceLock::new();

struct BoundedFileSink {
    file: Mutex<fs::File>,
    maximum_bytes: u64,
}

impl BoundedFileSink {
    fn open(path: &Path, maximum_bytes: u64) -> io::Result<Self> {
        if maximum_bytes == 0 {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "diagnostic sink bound must be nonzero",
            ));
        }
        let mut file = match fs::symlink_metadata(path) {
            Ok(_) => {
                validate_owner_only_file(path).map_err(io::Error::other)?;
                let file = fs::OpenOptions::new().read(true).write(true).open(path)?;
                validate_owner_only_file(path).map_err(io::Error::other)?;
                file
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                create_owner_only_file(path).map_err(io::Error::other)?
            }
            Err(error) => return Err(error),
        };
        if file.metadata()?.len() > maximum_bytes {
            file.set_len(0)?;
        }
        file.seek(io::SeekFrom::End(0))?;
        Ok(Self {
            file: Mutex::new(file),
            maximum_bytes,
        })
    }

    fn write(&self, event: &serde_json::Value) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(event).map_err(io::Error::other)?;
        bytes.push(b'\n');
        let length = u64::try_from(bytes.len()).map_err(io::Error::other)?;
        if length > self.maximum_bytes {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "diagnostic event exceeds sink bound",
            ));
        }
        let mut file = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        if file.metadata()?.len().saturating_add(length) > self.maximum_bytes {
            file.set_len(0)?;
            file.seek(io::SeekFrom::Start(0))?;
        } else {
            file.seek(io::SeekFrom::End(0))?;
        }
        file.write_all(&bytes)
    }
}

/// Creates or validates the fixed protected sink before a detached Controller launch.
///
/// # Errors
/// Rejects redirected, incorrectly protected, or unavailable files.
pub(crate) fn prepare_file_sink(path: &Path) -> io::Result<()> {
    BoundedFileSink::open(path, MAX_DIAGNOSTIC_FILE_BYTES).map(drop)
}

/// Installs the detached Controller's protected bounded diagnostic sink.
///
/// # Errors
/// Rejects an unsafe or unavailable state directory and repeated initialization.
pub fn configure_managed_file_sink(state_directory: &Path) -> io::Result<()> {
    let path = state_directory.join(HOSTED_SESSION_DIAGNOSTIC_FILE);
    let sink = BoundedFileSink::open(&path, MAX_DIAGNOSTIC_FILE_BYTES)?;
    FILE_SINK.set(sink).map_err(|_| {
        io::Error::new(
            ErrorKind::AlreadyExists,
            "hosted session diagnostic sink is already configured",
        )
    })
}

struct Restore(Option<Context>);
impl Drop for Restore {
    fn drop(&mut self) {
        CURRENT.set(self.0);
    }
}

fn category<T, E: Copy + Into<Category>>(result: &Result<T, E>) -> Category {
    match result {
        Ok(_) => Category::Ok,
        Err(error) => (*error).into(),
    }
}

pub(crate) fn operation<T, E: Copy + Into<Category>>(
    operation: Operation,
    action: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let context = Context {
        call: NEXT_CALL.fetch_add(1, Ordering::Relaxed),
        operation,
        stage: Stage::Session,
    };
    let _restore = Restore(CURRENT.replace(Some(context)));
    let start = Instant::now();
    let result = action();
    emit(
        Detail::Complete,
        category(&result),
        None,
        Some(start.elapsed().as_millis()),
    );
    result
}

pub(crate) fn stage<T, E: Copy + Into<Category>>(
    stage: Stage,
    action: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let prior = CURRENT.get();
    let _restore = Restore(prior);
    CURRENT.set(prior.map(|context| Context { stage, ..context }));
    let start = Instant::now();
    let result = action();
    emit(
        Detail::Complete,
        category(&result),
        None,
        Some(start.elapsed().as_millis()),
    );
    result
}

pub(crate) fn io(detail: Detail, kind: ErrorKind) {
    emit(
        detail,
        if matches!(kind, ErrorKind::TimedOut | ErrorKind::WouldBlock) {
            Category::Timeout
        } else {
            Category::IoFailure
        },
        None,
        None,
    );
}

pub(crate) fn invalid(detail: Detail) {
    emit(detail, Category::InvalidResponse, None, None);
}

pub(crate) fn http(status: u16) {
    emit(
        Detail::Http,
        if (200..300).contains(&status) {
            Category::Ok
        } else {
            Category::Unavailable
        },
        Some(status),
        None,
    );
}

fn record(
    context: Context,
    detail: Detail,
    category: Category,
    status: Option<u16>,
    elapsed_ms: Option<u128>,
) -> serde_json::Value {
    serde_json::json!({
        "event": "hosted_session_diagnostic", "pid": std::process::id(),
        "call": context.call, "operation": context.operation, "stage": context.stage,
        "detail": detail, "category": category, "status": status, "elapsed_ms": elapsed_ms,
    })
}

fn emit(detail: Detail, category: Category, status: Option<u16>, elapsed_ms: Option<u128>) {
    let Some(context) = CURRENT.get() else {
        return;
    };
    // Ignore sink failures. Diagnostic output must never alter admission.
    let event = record(context, detail, category, status, elapsed_ms);
    #[cfg(test)]
    CAPTURE.with_borrow_mut(|capture| {
        if let Some(events) = capture {
            events.push(event.clone());
        }
    });
    if let Some(sink) = FILE_SINK.get() {
        let _ = sink.write(&event);
    } else {
        let _ = writeln!(std::io::stderr().lock(), "{event}");
    }
}

#[cfg(test)]
pub(crate) fn capture<T>(action: impl FnOnce() -> T) -> (T, Vec<serde_json::Value>) {
    CAPTURE.set(Some(Vec::new()));
    let result = action();
    let events = CAPTURE.take().unwrap_or_default();
    (result, events)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    const TEST_SINK_PATH: &str = "WORLDSTREAM_TEST_HOSTED_SESSION_DIAGNOSTIC_LOG";

    #[test]
    fn configured_sink_receives_actual_scoped_events() {
        let directory = tempfile::tempdir().expect("diagnostic test fixture");
        let path = directory.path().join(HOSTED_SESSION_DIAGNOSTIC_FILE);
        let status = std::process::Command::new(
            std::env::current_exe().expect("diagnostic test fixture executable"),
        )
        .args([
            "--ignored",
            "--exact",
            "session_diagnostics::tests::configured_sink_worker",
        ])
        .env(TEST_SINK_PATH, &path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("diagnostic test fixture worker");
        assert!(status.success());
        let events: Vec<serde_json::Value> = fs::read(&path)
            .expect("diagnostic test fixture")
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).expect("complete diagnostic event"))
            .collect();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["stage"], "membership");
        assert_eq!(events[0]["detail"], "http");
        assert_eq!(events[0]["status"], 503);
        assert_eq!(events[1]["stage"], "membership");
        assert_eq!(events[1]["detail"], "complete");
        assert_eq!(events[2]["stage"], "session");
        assert_eq!(events[2]["detail"], "complete");
    }

    #[test]
    #[ignore = "owned subprocess helper for configured diagnostic sink test"]
    fn configured_sink_worker() {
        let path = std::env::var_os(TEST_SINK_PATH).expect("diagnostic test fixture path");
        let state = Path::new(&path)
            .parent()
            .expect("diagnostic test fixture state");
        configure_managed_file_sink(state).expect("configured diagnostic test sink");
        let result: Result<(), Category> = operation(Operation::Status, || {
            stage(Stage::Membership, || {
                http(503);
                Err(Category::Unavailable)
            })
        });
        assert!(result.is_err());
    }

    #[test]
    fn protected_file_sink_retains_only_complete_bounded_events() {
        let directory = tempfile::tempdir().expect("diagnostic test fixture");
        let path = directory.path().join("events.ndjson");
        let sink = BoundedFileSink::open(&path, 512).expect("diagnostic test fixture");
        let context = Context {
            call: 1,
            operation: Operation::Issue,
            stage: Stage::Membership,
        };
        for _ in 0..20 {
            sink.write(&record(
                context,
                Detail::Http,
                Category::Unavailable,
                Some(503),
                Some(12),
            ))
            .expect("diagnostic test fixture");
        }
        let bytes = fs::read(&path).expect("diagnostic test fixture");
        assert!(u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= 512);
        assert!(!bytes.is_empty() && bytes.ends_with(b"\n"));
        for line in bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let event: serde_json::Value =
                serde_json::from_slice(line).expect("complete diagnostic event");
            assert_eq!(event["event"], "hosted_session_diagnostic");
            assert_eq!(event.as_object().map(serde_json::Map::len), Some(9));
        }
        validate_owner_only_file(&path).expect("protected diagnostic sink");
    }

    #[test]
    #[cfg(unix)]
    fn protected_file_sink_rejects_permissive_existing_file() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().expect("diagnostic test fixture");
        let path = directory.path().join("events.ndjson");
        fs::write(&path, b"untrusted\n").expect("diagnostic test fixture");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .expect("diagnostic test fixture");
        assert!(BoundedFileSink::open(&path, 512).is_err());
        assert_eq!(
            fs::read(&path).expect("diagnostic test fixture"),
            b"untrusted\n"
        );
    }

    #[test]
    #[allow(clippy::panic)] // Deliberately exercise unwinding of the diagnostic scope.
    fn fixed_schema_and_scope_do_not_retain_session_or_error_material() {
        let result: Result<(), _> = operation(Operation::StreamTicket, || {
            let call = CURRENT.get().expect("diagnostic test fixture").call;
            stage(Stage::Membership, || {
                let record = record(
                    CURRENT.get().expect("diagnostic test fixture"),
                    Detail::Http,
                    Category::Unavailable,
                    Some(503),
                    Some(12),
                );
                assert_eq!(record["call"], call);
                assert_eq!(record["stage"], "membership");
                assert_eq!(record["status"], 503);
                assert_eq!(
                    record.as_object().expect("diagnostic test fixture").len(),
                    9
                );
                Err(Category::Unavailable)
            })
        });
        assert!(result.is_err());
        assert!(CURRENT.get().is_none());
        let panic = std::panic::catch_unwind(|| {
            operation::<(), Category>(Operation::Status, || panic!("not logged"))
        });
        assert!(panic.is_err());
        assert!(CURRENT.get().is_none());
    }

    #[test]
    fn concurrent_calls_do_not_share_correlation_scope() {
        let ids: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(|| {
                    operation(Operation::Status, || {
                        Ok::<_, Category>(CURRENT.get().expect("diagnostic test fixture").call)
                    })
                    .expect("diagnostic test fixture")
                })
            })
            .collect();
        let unique: std::collections::HashSet<_> = ids
            .into_iter()
            .map(|task| task.join().expect("diagnostic test fixture"))
            .collect();
        assert_eq!(unique.len(), 4);
        assert!(CURRENT.get().is_none());
    }
}
