//! Closed, storage-owned PostgreSQL telemetry facts.
//!
//! This module intentionally has no dependency on the server exporter.  The
//! PostgreSQL adapter emits only bounded facts through [`PostgresTelemetrySink`];
//! the server may map those facts to its existing versioned queue.  The sink
//! is observational and is never allowed to affect database truth.

use std::panic::{AssertUnwindSafe, catch_unwind};

/// A migration lifecycle fact from a real PostgreSQL administration call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostgresMigrationPhaseV1 {
    /// The direct-admin migration call started.
    Started,
    /// One or more forward migrations committed successfully.
    Applied,
    /// The database already had the reviewed migration prefix.
    AlreadyCurrent,
    /// The migration call failed or was interrupted.
    Failed,
}

/// A recovery lifecycle fact from a real PostgreSQL recovery/repair call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostgresRecoveryPhaseV1 {
    /// Recovery inspection or repair started.
    Started,
    /// Recovery completed and its result was installed/verified.
    Completed,
    /// Recovery failed closed.
    Failed,
}

/// A durable PostgreSQL integrity disposition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostgresIntegrityStatusV1 {
    /// The Room was marked faulted by a guarded integrity operation.
    Faulted,
    /// The Room was quarantined by a guarded integrity operation.
    Quarantined,
}

/// A closed storage diagnostic vocabulary shared with the server telemetry
/// contract.  It contains no SQLSTATE, SQL text, endpoint, Room, or payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostgresStorageDiagnosticKindV1 {
    Connection,
    Integrity,
    Query,
    Lock,
}

/// Vendor-neutral facts emitted by storage-owned PostgreSQL boundaries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostgresTelemetryEventV1 {
    Migration {
        phase: PostgresMigrationPhaseV1,
        schema_version: u64,
    },
    Recovery {
        phase: PostgresRecoveryPhaseV1,
    },
    Integrity {
        status: PostgresIntegrityStatusV1,
    },
    StorageDiagnostic {
        kind: PostgresStorageDiagnosticKindV1,
    },
}

/// Best-effort, non-blocking observation hook for PostgreSQL diagnostics.
/// Implementations must not panic or make PostgreSQL correctness depend on
/// delivery.  The adapter additionally catches panics so embedding code and
/// tests cannot unwind a migration or commit call through this hook.
pub trait PostgresTelemetrySink: Send + Sync + 'static {
    fn emit(&self, event: PostgresTelemetryEventV1);
}

/// Delivers one PostgreSQL diagnostic without allowing an observability bug to
/// unwind a storage operation.
pub(crate) fn emit_postgres_telemetry(
    telemetry: Option<&dyn PostgresTelemetrySink>,
    event: PostgresTelemetryEventV1,
) {
    if let Some(telemetry) = telemetry {
        let _ = catch_unwind(AssertUnwindSafe(|| telemetry.emit(event)));
    }
}

/// Emits a migration failure on scope exit unless the caller records a
/// terminal success.  This covers connection, SQL, capability, verification,
/// and transaction-commit failures without duplicating error branches.
pub(crate) struct MigrationTelemetryGuard<'a> {
    telemetry: Option<&'a dyn PostgresTelemetrySink>,
    target_version: u64,
    completed: bool,
}

impl<'a> MigrationTelemetryGuard<'a> {
    pub(crate) fn new(
        telemetry: Option<&'a dyn PostgresTelemetrySink>,
        target_version: u64,
    ) -> Self {
        Self {
            telemetry,
            target_version,
            completed: false,
        }
    }

    pub(crate) fn started(&self) {
        emit_postgres_telemetry(
            self.telemetry,
            PostgresTelemetryEventV1::Migration {
                phase: PostgresMigrationPhaseV1::Started,
                schema_version: self.target_version,
            },
        );
    }

    pub(crate) fn complete(&mut self, phase: PostgresMigrationPhaseV1) {
        emit_postgres_telemetry(
            self.telemetry,
            PostgresTelemetryEventV1::Migration {
                phase,
                schema_version: self.target_version,
            },
        );
        self.completed = true;
    }
}

impl Drop for MigrationTelemetryGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            emit_postgres_telemetry(
                self.telemetry,
                PostgresTelemetryEventV1::Migration {
                    phase: PostgresMigrationPhaseV1::Failed,
                    schema_version: self.target_version,
                },
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct RecordingSink(Arc<Mutex<Vec<PostgresTelemetryEventV1>>>);

    impl PostgresTelemetrySink for RecordingSink {
        fn emit(&self, event: PostgresTelemetryEventV1) {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(event);
        }
    }

    struct PanickingSink;

    impl PostgresTelemetrySink for PanickingSink {
        fn emit(&self, _: PostgresTelemetryEventV1) {
            panic!("PostgreSQL telemetry must not unwind storage");
        }
    }

    #[test]
    fn migration_guard_emits_closed_terminal_facts() {
        let sink = RecordingSink::default();
        {
            let mut guard = MigrationTelemetryGuard::new(Some(&sink), 8);
            guard.started();
            guard.complete(PostgresMigrationPhaseV1::Applied);
        }
        {
            let mut guard = MigrationTelemetryGuard::new(Some(&sink), 8);
            guard.started();
            guard.complete(PostgresMigrationPhaseV1::AlreadyCurrent);
        }
        {
            let guard = MigrationTelemetryGuard::new(Some(&sink), 8);
            guard.started();
        }
        let events = sink
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(
            events,
            vec![
                PostgresTelemetryEventV1::Migration {
                    phase: PostgresMigrationPhaseV1::Started,
                    schema_version: 8,
                },
                PostgresTelemetryEventV1::Migration {
                    phase: PostgresMigrationPhaseV1::Applied,
                    schema_version: 8,
                },
                PostgresTelemetryEventV1::Migration {
                    phase: PostgresMigrationPhaseV1::Started,
                    schema_version: 8,
                },
                PostgresTelemetryEventV1::Migration {
                    phase: PostgresMigrationPhaseV1::AlreadyCurrent,
                    schema_version: 8,
                },
                PostgresTelemetryEventV1::Migration {
                    phase: PostgresMigrationPhaseV1::Started,
                    schema_version: 8,
                },
                PostgresTelemetryEventV1::Migration {
                    phase: PostgresMigrationPhaseV1::Failed,
                    schema_version: 8,
                },
            ]
        );
    }

    #[test]
    fn panicking_sink_isolated_from_producer() {
        let result = std::panic::catch_unwind(|| {
            emit_postgres_telemetry(
                Some(&PanickingSink),
                PostgresTelemetryEventV1::StorageDiagnostic {
                    kind: PostgresStorageDiagnosticKindV1::Query,
                },
            );
        });
        assert!(result.is_ok());
    }
}
