//! Offline protected metadata inspection. Retained phase and lease observations
//! are not authenticated endpoint evidence and never authorize process control.

use std::{net::SocketAddr, path::Path};

use serde::Serialize;

use crate::{
    managed_lifecycle::{LifecycleOperation, read_operation_record},
    process_ownership::{ProcessOwnership, ProcessPhase, ProcessRole},
};

/// Closed per-process evidence; absent is not proof of a stopped process.
#[derive(Debug, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum RetainedProcessInspection {
    Available {
        generation: String,
        phase: RetainedPhase,
        pid: Option<u32>,
        endpoint: Option<SocketAddr>,
        lease: RetainedLease,
    },
    Absent,
    Unavailable,
}

/// Historical phase, never present liveness.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetainedPhase {
    Starting,
    Ready,
    Stopped,
    Failed,
}

/// Momentary existing lease observation, not process identity proof.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetainedLease {
    Held,
    Released,
}

/// A validated checkpoint can remain visible when another component is corrupt.
#[derive(Debug, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum RetainedOperationInspection {
    Available { checkpoint: LifecycleOperation },
    Absent,
    Unavailable,
}

/// Non-live provenance is explicit in every serialized inspection.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetainedEvidence {
    RetainedOnly,
}

/// Safe diagnostics only; contains no credentials, raw records, or control grant.
#[derive(Debug, Serialize)]
pub struct RetainedServerInspection {
    pub schema: &'static str,
    pub evidence: RetainedEvidence,
    pub controller: RetainedProcessInspection,
    pub runtime: RetainedProcessInspection,
    pub operation: RetainedOperationInspection,
}

/// Reads existing protected metadata without initialization, store construction,
/// repair, network contact, or process startup. Components fail independently.
#[must_use]
pub fn inspect_retained_server(state: &Path) -> RetainedServerInspection {
    let ownership = ProcessOwnership::open(state);
    let controller = ownership
        .as_ref()
        .map_or(RetainedProcessInspection::Unavailable, |owner| {
            inspect_process(owner, ProcessRole::Controller)
        });
    let runtime = ownership
        .as_ref()
        .map_or(RetainedProcessInspection::Unavailable, |owner| {
            inspect_process(owner, ProcessRole::Runtime)
        });
    let operation = match worldstream_runtime::validate_data_directory(state) {
        Ok(state) => match read_operation_record(&state.join("managed-lifecycle.v1.json")) {
            Ok(Some(checkpoint)) => RetainedOperationInspection::Available { checkpoint },
            Ok(None) => RetainedOperationInspection::Absent,
            Err(_) => RetainedOperationInspection::Unavailable,
        },
        Err(_) => RetainedOperationInspection::Unavailable,
    };
    RetainedServerInspection {
        schema: "worldstream/retained-server-inspection/v1",
        evidence: RetainedEvidence::RetainedOnly,
        controller,
        runtime,
        operation,
    }
}

fn inspect_process(owner: &ProcessOwnership, role: ProcessRole) -> RetainedProcessInspection {
    let evidence = (|| {
        let before = owner.snapshot(role)?;
        let leased = owner.is_leased(role)?;
        let after = owner.snapshot(role)?;
        if before != after {
            return Err(crate::process_ownership::OwnershipError::Unavailable);
        }
        Ok((after, leased))
    })();
    match evidence {
        Ok((Some(record), leased)) => RetainedProcessInspection::Available {
            generation: record.generation,
            phase: match record.phase {
                ProcessPhase::Starting => RetainedPhase::Starting,
                ProcessPhase::Ready => RetainedPhase::Ready,
                ProcessPhase::Stopped => RetainedPhase::Stopped,
                ProcessPhase::Failed => RetainedPhase::Failed,
            },
            pid: record.pid,
            endpoint: record.endpoint,
            lease: if leased {
                RetainedLease::Held
            } else {
                RetainedLease::Released
            },
        },
        Ok((None, false)) => RetainedProcessInspection::Absent,
        Ok((None, true)) | Err(_) => RetainedProcessInspection::Unavailable,
    }
}
