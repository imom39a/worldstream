#![allow(clippy::panic)]

//! `PostgreSQL` telemetry boundary tests.
//!
//! The first test is always runnable and exercises the real admin connection
//! failure path.  The live test is opt-in through the same disposable DSN
//! used by the `PostgreSQL` evidence suite; it performs the actual migration
//! and current-schema calls against that provider.

use std::sync::{Arc, Mutex};

use worldstream_postgres::{
    PostgresAdmin, PostgresConnectionConfig, PostgresIntegrityStatusV1, PostgresMigrationPhaseV1,
    PostgresRecoveryPhaseV1, PostgresStorageDiagnosticKindV1, PostgresTelemetryEventV1,
    PostgresTelemetrySink,
};

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

struct FailingSink;

impl PostgresTelemetrySink for FailingSink {
    fn emit(&self, _: PostgresTelemetryEventV1) {
        panic!("telemetry sink failure must not alter PostgreSQL results");
    }
}

fn unreachable_admin() -> PostgresAdmin {
    PostgresAdmin::new(
        PostgresConnectionConfig::direct_admin(
            "host=127.0.0.1 port=1 user=worldstream connect_timeout=1",
        )
        .unwrap_or_else(|error| panic!("test admin config: {error}")),
    )
    .unwrap_or_else(|error| panic!("test admin handle: {error}"))
}

#[test]
fn real_migration_failure_emits_closed_failed_fact_without_unwinding() {
    let sink = RecordingSink::default();
    let events = Arc::clone(&sink.0);
    let result = unreachable_admin().with_telemetry(Arc::new(sink)).migrate();
    assert!(result.is_err());
    assert_eq!(
        events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        [
            PostgresTelemetryEventV1::Migration {
                phase: PostgresMigrationPhaseV1::Started,
                schema_version: 10,
            },
            PostgresTelemetryEventV1::Migration {
                phase: PostgresMigrationPhaseV1::Failed,
                schema_version: 10,
            },
        ]
    );
}

#[test]
fn panicking_sink_cannot_change_real_migration_failure() {
    let result = std::panic::catch_unwind(|| {
        unreachable_admin()
            .with_telemetry(Arc::new(FailingSink))
            .migrate()
    });
    assert!(result.is_ok(), "sink panic escaped the migration boundary");
    assert!(
        result
            .unwrap_or_else(|_| unreachable!("panic was asserted absent"))
            .is_err()
    );
}

#[test]
fn real_recovery_failure_and_storage_error_emit_closed_facts() {
    let sink = RecordingSink::default();
    let events = Arc::clone(&sink.0);
    let result = unreachable_admin()
        .with_telemetry(Arc::new(sink))
        .rebuild_snapshot_cache("01ARZ3NDEKTSV4RRFFQ69G5FAV");
    assert!(result.is_err());
    let events = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(
        events.first(),
        Some(&PostgresTelemetryEventV1::Recovery {
            phase: PostgresRecoveryPhaseV1::Started,
        })
    );
    assert!(
        events.contains(&PostgresTelemetryEventV1::StorageDiagnostic {
            kind: PostgresStorageDiagnosticKindV1::Connection,
        })
    );
    assert_eq!(
        events.last(),
        Some(&PostgresTelemetryEventV1::Recovery {
            phase: PostgresRecoveryPhaseV1::Failed,
        })
    );
}

#[test]
fn event_vocabulary_is_closed_and_contains_no_provider_data() {
    let events = [
        PostgresTelemetryEventV1::Recovery {
            phase: PostgresRecoveryPhaseV1::Completed,
        },
        PostgresTelemetryEventV1::Integrity {
            status: PostgresIntegrityStatusV1::Quarantined,
        },
        PostgresTelemetryEventV1::StorageDiagnostic {
            kind: PostgresStorageDiagnosticKindV1::Connection,
        },
    ];
    let debug = format!("{events:?}");
    assert!(!debug.contains("room_id"));
    assert!(!debug.contains("canonical"));
    assert!(!debug.contains("dsn"));
    assert!(!debug.contains("sql"));
}

#[test]
#[ignore = "requires a disposable PostgreSQL 17.11 provider"]
fn live_admin_migration_and_current_schema_emit_terminal_facts() {
    let dsn = std::env::var("WORLDSTREAM_POSTGRES_TEST_DSN")
        .unwrap_or_else(|error| panic!("WORLDSTREAM_POSTGRES_TEST_DSN: {error}"));
    let config = PostgresConnectionConfig::direct_admin(dsn)
        .unwrap_or_else(|error| panic!("live admin config: {error}"));
    let sink = RecordingSink::default();
    let events = Arc::clone(&sink.0);
    let admin = PostgresAdmin::new(config)
        .unwrap_or_else(|error| panic!("live admin: {error}"))
        .with_telemetry(Arc::new(sink));

    admin
        .migrate()
        .unwrap_or_else(|error| panic!("live migration: {error}"));
    admin
        .migrate()
        .unwrap_or_else(|error| panic!("live current migration: {error}"));

    let events = events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(events.len(), 4);
    assert_eq!(
        events[0],
        PostgresTelemetryEventV1::Migration {
            phase: PostgresMigrationPhaseV1::Started,
            schema_version: 10,
        }
    );
    assert!(matches!(
        events[1],
        PostgresTelemetryEventV1::Migration {
            phase: PostgresMigrationPhaseV1::Applied | PostgresMigrationPhaseV1::AlreadyCurrent,
            schema_version: 10,
        }
    ));
    assert_eq!(
        events[2],
        PostgresTelemetryEventV1::Migration {
            phase: PostgresMigrationPhaseV1::Started,
            schema_version: 10,
        }
    );
    assert_eq!(
        events[3],
        PostgresTelemetryEventV1::Migration {
            phase: PostgresMigrationPhaseV1::AlreadyCurrent,
            schema_version: 10,
        }
    );
}
