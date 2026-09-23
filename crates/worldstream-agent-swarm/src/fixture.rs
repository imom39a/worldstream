//! File-backed fixture backend.
//!
//! This module is test scaffolding. It does not create a `WorldStream` Room,
//! issue capabilities, authenticate a Participant, or execute a provider CLI.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    backend::{BackendError, SwarmBackend},
    domain::{SwarmId, SwarmSummary, SwarmView, ValidatedCreateSwarm},
};

const SCHEMA: &str = "worldstream/agent-swarm-fixture-store/v1";
const SOURCE_LABEL: &str = "FIXTURE ONLY — not authoritative WorldStream state";
const MAX_STORE_BYTES: u64 = 4 * 1024 * 1024;

pub struct FixtureFileBackend {
    path: PathBuf,
}

impl FixtureFileBackend {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> Result<FixtureStore, BackendError> {
        match fs::metadata(&self.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FixtureStore::new()),
            Err(_) => Err(BackendError::StorageUnavailable),
            Ok(metadata) if metadata.len() > MAX_STORE_BYTES => Err(BackendError::InvalidData),
            Ok(_) => {
                let bytes = fs::read(&self.path).map_err(|_| BackendError::StorageUnavailable)?;
                let store: FixtureStore =
                    serde_json::from_slice(&bytes).map_err(|_| BackendError::InvalidData)?;
                if store.schema != SCHEMA {
                    return Err(BackendError::InvalidData);
                }
                Ok(store)
            }
        }
    }

    fn write(&self, store: &FixtureStore) -> Result<(), BackendError> {
        let parent = self.path.parent().ok_or(BackendError::StorageUnavailable)?;
        fs::create_dir_all(parent).map_err(|_| BackendError::StorageUnavailable)?;
        let bytes = serde_json::to_vec_pretty(store).map_err(|_| BackendError::InvalidData)?;
        let temporary = self.path.with_extension("tmp");
        fs::write(&temporary, bytes).map_err(|_| BackendError::StorageUnavailable)?;
        // Windows rename does not replace an existing destination. This is a
        // fixture store rather than authoritative history, so a bounded remove
        // before replacement is sufficient for the native smoke contract.
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(BackendError::StorageUnavailable),
        }
        fs::rename(temporary, &self.path).map_err(|_| BackendError::StorageUnavailable)
    }
}

impl SwarmBackend for FixtureFileBackend {
    fn create(&mut self, request: ValidatedCreateSwarm) -> Result<SwarmView, BackendError> {
        let mut store = self.read()?;
        let ordinal = store.next_ordinal;
        store.next_ordinal = ordinal
            .checked_add(1)
            .ok_or(BackendError::IdentityUnavailable)?;
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| BackendError::IdentityUnavailable)?
            .as_nanos();
        let swarm_id = SwarmId::new(format!("fixture-{epoch:x}-{ordinal:x}"))
            .map_err(|_| BackendError::IdentityUnavailable)?;
        let view = SwarmView {
            room_id: format!("fixture-room-{epoch:x}-{ordinal:x}"),
            swarm_id,
            goal: request.goal,
            constraints: request.constraints,
            acceptance_criteria: request.acceptance_criteria,
            working_area: request.working_area,
            roster: request.roster,
            progress_review_interval_seconds: request.progress_review_interval_seconds,
            correction_failure_limit: request.correction_failure_limit,
            source_label: SOURCE_LABEL.to_owned(),
        };
        store.swarms.push(view.clone());
        self.write(&store)?;
        Ok(view)
    }

    fn list(&self) -> Result<Vec<SwarmSummary>, BackendError> {
        Ok(self.read()?.swarms.iter().map(SwarmView::summary).collect())
    }

    fn open(&self, swarm_id: &SwarmId) -> Result<SwarmView, BackendError> {
        self.read()?
            .swarms
            .into_iter()
            .find(|swarm| &swarm.swarm_id == swarm_id)
            .ok_or(BackendError::NotFound)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FixtureStore {
    schema: String,
    next_ordinal: u64,
    swarms: Vec<SwarmView>,
}

impl FixtureStore {
    fn new() -> Self {
        Self {
            schema: SCHEMA.to_owned(),
            next_ordinal: 0,
            swarms: Vec::new(),
        }
    }
}
