//! An operational deadline for a synchronous storage read, independent of semantic time.
use std::{cell::Cell, time::Instant};

thread_local! {
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Returns the current synchronous storage-read deadline, if one is installed.
#[must_use]
pub fn storage_read_deadline() -> Option<Instant> {
    DEADLINE.get()
}

/// Supplies one absolute deadline to synchronous provider reads, including fallbacks.
/// Providers must enforce it in their SQL and connection operations.
/// This scope cannot terminate arbitrary native code or propagate to another thread.
/// Nested scopes retain the earlier deadline. Unwinding restores the previous scope.
pub fn with_storage_read_deadline<T>(deadline: Instant, read: impl FnOnce() -> T) -> T {
    struct Restore(Option<Instant>);
    impl Drop for Restore {
        fn drop(&mut self) {
            DEADLINE.set(self.0);
        }
    }
    let previous = DEADLINE.get();
    DEADLINE.set(Some(previous.map_or(deadline, |prior| prior.min(deadline))));
    let _restore = Restore(previous);
    read()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        panic::{AssertUnwindSafe, catch_unwind},
        time::Duration,
    };

    #[test]
    fn storage_read_deadline_keeps_the_earlier_nested_scope_and_restores_on_unwind() {
        assert_eq!(storage_read_deadline(), None);
        let outer = Instant::now() + Duration::from_secs(1);
        with_storage_read_deadline(outer, || {
            with_storage_read_deadline(outer + Duration::from_secs(1), || {
                assert_eq!(storage_read_deadline(), Some(outer));
            });
            let expired = Instant::now() - Duration::from_secs(1);
            let caught = catch_unwind(AssertUnwindSafe(|| {
                with_storage_read_deadline(expired, || {
                    assert!(storage_read_deadline().is_some_and(|cut| Instant::now() > cut));
                    std::panic::resume_unwind(Box::new("deadline scope test"));
                });
            }));
            assert!(caught.is_err());
            assert_eq!(storage_read_deadline(), Some(outer));
        });
        assert_eq!(storage_read_deadline(), None);
    }
}
