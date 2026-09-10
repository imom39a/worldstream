//! Secret-free, release-build diagnostics for synchronous hosted session calls.
//! No request, response body, identity, token, or arbitrary error text is accepted.
use std::{
    cell::Cell,
    io::{ErrorKind, Write as _},
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

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
    #[cfg(test)]
    CAPTURE.with_borrow_mut(|capture| {
        if let Some(events) = capture {
            events.push(record(context, detail, category, status, elapsed_ms));
        }
    });
    let _ = writeln!(
        std::io::stderr().lock(),
        "{}",
        record(context, detail, category, status, elapsed_ms)
    );
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
