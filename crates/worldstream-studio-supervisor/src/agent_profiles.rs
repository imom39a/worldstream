//! Immutable external Agent Profile revisions and exact seat assignments.
//!
//! Profiles contain bounded policy configuration and opaque, kind-bound secret
//! references. They do not execute models, retain agent-private memory, or
//! grant Membership Action or Runner control authority.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_protocol::UlidString;
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};

use crate::{
    room_drafts::AgentProfileRevisionReferenceV1,
    secrets::{FileSecretVaultV1, SecretAvailabilityV1, SecretKindV1, SecretReferenceV1},
};

const PROFILE_SCHEMA_V1: &str = "worldstream/studio-agent-profile/v1";
const ASSIGNMENT_SCHEMA_V1: &str = "worldstream/studio-agent-profile-assignment/v1";
const CATALOG_SCHEMA_V1: &str = "worldstream/studio-agent-profile-catalog/v1";
const ASSIGNMENT_CATALOG_SCHEMA_V1: &str = "worldstream/studio-agent-profile-assignment-catalog/v1";
const MAX_RECORD_BYTES: u64 = 64 * 1024;
const MAX_RECORDS: usize = 256;
const MAX_TEXT_BYTES: usize = 256;
const MAX_SETTING_BYTES: usize = 4096;

/// One secret-bearing Agent Profile setting retained only as an opaque ref.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProfileSecretSettingV1 {
    pub key: String,
    pub kind: SecretKindV1,
    pub reference: SecretReferenceV1,
}

/// Immutable owner-published Agent Profile revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProfileRevisionV1 {
    pub schema: String,
    pub profile_id: String,
    pub revision: String,
    pub display_name: String,
    pub non_secret_configuration: BTreeMap<String, String>,
    pub secret_settings: Vec<AgentProfileSecretSettingV1>,
}

/// Browser-safe state of one required secret setting.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProfileSecretAvailabilityV1 {
    Configured,
    Missing,
    Unavailable,
}

/// Browser-safe secret requirement without its retained reference.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProfileSecretSettingViewV1 {
    pub key: String,
    pub kind: SecretKindV1,
    pub availability: AgentProfileSecretAvailabilityV1,
}

/// Browser-safe exact profile revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProfileRevisionViewV1 {
    pub profile_id: String,
    pub revision: String,
    pub display_name: String,
    pub non_secret_configuration: BTreeMap<String, String>,
    pub secret_settings: Vec<AgentProfileSecretSettingViewV1>,
}

/// Bounded exact revision catalog.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProfileCatalogV1 {
    pub schema: String,
    pub profiles: Vec<AgentProfileRevisionViewV1>,
}

/// Exact Membership identity produced for one reviewed Agent seat.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProfileMembershipBindingV1 {
    pub room_id: String,
    pub member_id: String,
    pub principal_id: String,
    pub role: String,
}

/// Separate execution assignment. This is never participant Action authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentExecutionBindingV1 {
    External,
    Managed { runner_id: String },
}

/// Immutable exact mapping from one reviewed seat to its resulting Membership.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProfileSeatAssignmentV1 {
    pub schema: String,
    pub assignment_id: String,
    pub draft_id: String,
    pub seat_id: String,
    pub profile: AgentProfileRevisionReferenceV1,
    pub membership: AgentProfileMembershipBindingV1,
    pub execution: AgentExecutionBindingV1,
}

/// Bounded browser-safe assignment catalog.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProfileAssignmentCatalogV1 {
    pub schema: String,
    pub assignments: Vec<AgentProfileSeatAssignmentV1>,
}

/// Closed profile-store failures without paths or credential material.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AgentProfileErrorV1 {
    #[error("Agent Profile revision is invalid")]
    InvalidProfile,
    #[error("Agent Profile revision is immutable")]
    ImmutableRevisionConflict,
    #[error("Agent Profile assignment is invalid")]
    InvalidAssignment,
    #[error("Agent Profile assignment is immutable")]
    ImmutableAssignmentConflict,
    #[error("Agent Profile record was not found")]
    NotFound,
    #[error("Agent Profile store is unavailable")]
    Unavailable,
}

/// Owner-only immutable store, reproducible after Supervisor restart.
#[derive(Clone)]
pub struct AgentProfileStoreV1 {
    profiles: Arc<PathBuf>,
    assignments: Arc<PathBuf>,
    vault: FileSecretVaultV1,
    mutation: Arc<Mutex<()>>,
}

impl AgentProfileStoreV1 {
    /// Opens or creates one protected store and validates every retained record.
    ///
    /// # Errors
    ///
    /// Fails closed for unsafe storage, corrupt records, or excessive catalogs.
    pub fn open(root: &Path, vault: FileSecretVaultV1) -> Result<Self, AgentProfileErrorV1> {
        let root = prepare_data_directory(root).map_err(|_| AgentProfileErrorV1::Unavailable)?;
        let profiles = prepare_data_directory(&root.join("revisions"))
            .map_err(|_| AgentProfileErrorV1::Unavailable)?;
        let assignments = prepare_data_directory(&root.join("assignments"))
            .map_err(|_| AgentProfileErrorV1::Unavailable)?;
        let store = Self {
            profiles: Arc::new(profiles),
            assignments: Arc::new(assignments),
            vault,
            mutation: Arc::new(Mutex::new(())),
        };
        store.load_revisions()?;
        store.load_assignments()?;
        Ok(store)
    }

    /// Publishes a new immutable revision, or returns the identical existing one.
    ///
    /// # Errors
    ///
    /// Rejects malformed configuration, unavailable/wrong-kind secret refs, or
    /// a changed payload under an existing exact identity.
    pub fn publish(
        &self,
        revision: &AgentProfileRevisionV1,
    ) -> Result<AgentProfileRevisionViewV1, AgentProfileErrorV1> {
        validate_revision(revision)?;
        let _guard = self.lock();
        let target = self.profile_path(&revision.profile_id, &revision.revision);
        if target.exists() {
            let existing = read_record::<AgentProfileRevisionV1>(&target)?;
            return if existing == *revision {
                Ok(self.view(&existing))
            } else {
                Err(AgentProfileErrorV1::ImmutableRevisionConflict)
            };
        }
        for setting in &revision.secret_settings {
            if setting.kind != SecretKindV1::ModelProvider
                || self
                    .vault
                    .inspect(setting.kind, &setting.reference)
                    .availability
                    != SecretAvailabilityV1::Configured
            {
                return Err(AgentProfileErrorV1::InvalidProfile);
            }
        }
        if self.load_revisions()?.len() >= MAX_RECORDS {
            return Err(AgentProfileErrorV1::Unavailable);
        }
        let profile_directory =
            prepare_data_directory(&self.profile_directory(&revision.profile_id))
                .map_err(|_| AgentProfileErrorV1::Unavailable)?;
        sync_directory(&self.profiles).map_err(|_| AgentProfileErrorV1::Unavailable)?;
        let target =
            profile_directory.join(format!("{}.json", encode_component(&revision.revision)));
        persist_new(&target, revision)?;
        Ok(self.view(revision))
    }

    /// Lists browser-safe exact revisions in stable identity order.
    ///
    /// # Errors
    ///
    /// Fails closed if protected records cannot be validated.
    pub fn catalog(&self) -> Result<AgentProfileCatalogV1, AgentProfileErrorV1> {
        Ok(AgentProfileCatalogV1 {
            schema: CATALOG_SCHEMA_V1.to_owned(),
            profiles: self
                .load_revisions()?
                .iter()
                .map(|revision| self.view(revision))
                .collect(),
        })
    }

    /// Loads one exact browser-safe profile revision without substitution.
    ///
    /// # Errors
    ///
    /// Returns not-found or a closed store failure.
    pub fn revision(
        &self,
        profile_id: &str,
        revision: &str,
    ) -> Result<AgentProfileRevisionViewV1, AgentProfileErrorV1> {
        validate_profile_id(profile_id)?;
        validate_revision_id(revision)?;
        let path = self.profile_path(profile_id, revision);
        if !path.exists() {
            return Err(AgentProfileErrorV1::NotFound);
        }
        let record = read_record::<AgentProfileRevisionV1>(&path)?;
        validate_revision(&record)?;
        if record.profile_id != profile_id || record.revision != revision {
            return Err(AgentProfileErrorV1::Unavailable);
        }
        Ok(self.view(&record))
    }

    /// Persists one immutable exact seat-to-Membership assignment.
    ///
    /// # Errors
    ///
    /// Rejects unknown profile revisions, malformed identities, or rebinding an
    /// existing assignment identity to another Membership or Runner.
    pub fn bind_membership(
        &self,
        assignment: &AgentProfileSeatAssignmentV1,
    ) -> Result<AgentProfileSeatAssignmentV1, AgentProfileErrorV1> {
        validate_assignment(assignment)?;
        self.revision(&assignment.profile.profile_id, &assignment.profile.revision)?;
        let _guard = self.lock();
        let target = self.assignment_path(&assignment.assignment_id);
        if target.exists() {
            let existing = read_record::<AgentProfileSeatAssignmentV1>(&target)?;
            return if existing == *assignment {
                Ok(existing)
            } else {
                Err(AgentProfileErrorV1::ImmutableAssignmentConflict)
            };
        }
        let existing = self.load_assignments()?;
        if existing.len() >= MAX_RECORDS {
            return Err(AgentProfileErrorV1::Unavailable);
        }
        if existing.iter().any(|value| {
            (value.draft_id == assignment.draft_id && value.seat_id == assignment.seat_id)
                || (value.membership.room_id == assignment.membership.room_id
                    && value.membership.member_id == assignment.membership.member_id)
        }) {
            return Err(AgentProfileErrorV1::ImmutableAssignmentConflict);
        }
        persist_new(&target, assignment)?;
        Ok(assignment.clone())
    }

    /// Reconciles the exact immutable seat assignment after an interrupted
    /// setup checkpoint. A missing record is recreated and an identical
    /// retained record is returned; any competing binding fails closed.
    ///
    /// # Errors
    ///
    /// Rejects invalid or conflicting assignments and unavailable storage.
    pub fn reconcile_membership(
        &self,
        assignment: &AgentProfileSeatAssignmentV1,
    ) -> Result<AgentProfileSeatAssignmentV1, AgentProfileErrorV1> {
        self.bind_membership(assignment)
    }

    /// Lists exact browser-safe assignments in stable identity order.
    ///
    /// # Errors
    ///
    /// Fails closed if any retained assignment is corrupt.
    pub fn assignments(&self) -> Result<AgentProfileAssignmentCatalogV1, AgentProfileErrorV1> {
        Ok(AgentProfileAssignmentCatalogV1 {
            schema: ASSIGNMENT_CATALOG_SCHEMA_V1.to_owned(),
            assignments: self.load_assignments()?,
        })
    }

    /// Loads one exact immutable seat assignment for an internal bounded helper.
    ///
    /// # Errors
    ///
    /// Returns not-found or a closed store validation failure.
    pub fn assignment(
        &self,
        assignment_id: &str,
    ) -> Result<AgentProfileSeatAssignmentV1, AgentProfileErrorV1> {
        if assignment_id.parse::<UlidString>().is_err() {
            return Err(AgentProfileErrorV1::InvalidAssignment);
        }
        let path = self.assignment_path(assignment_id);
        if !path.exists() {
            return Err(AgentProfileErrorV1::NotFound);
        }
        let assignment = read_record::<AgentProfileSeatAssignmentV1>(&path)?;
        validate_assignment(&assignment)?;
        if assignment.assignment_id != assignment_id {
            return Err(AgentProfileErrorV1::Unavailable);
        }
        let profile = read_record::<AgentProfileRevisionV1>(
            &self.profile_path(&assignment.profile.profile_id, &assignment.profile.revision),
        )?;
        validate_revision(&profile)?;
        if profile.profile_id != assignment.profile.profile_id
            || profile.revision != assignment.profile.revision
        {
            return Err(AgentProfileErrorV1::Unavailable);
        }
        Ok(assignment)
    }

    fn view(&self, revision: &AgentProfileRevisionV1) -> AgentProfileRevisionViewV1 {
        AgentProfileRevisionViewV1 {
            profile_id: revision.profile_id.clone(),
            revision: revision.revision.clone(),
            display_name: revision.display_name.clone(),
            non_secret_configuration: revision.non_secret_configuration.clone(),
            secret_settings: revision
                .secret_settings
                .iter()
                .map(|setting| {
                    let availability = match self
                        .vault
                        .inspect(setting.kind, &setting.reference)
                        .availability
                    {
                        SecretAvailabilityV1::Configured => {
                            AgentProfileSecretAvailabilityV1::Configured
                        }
                        SecretAvailabilityV1::Missing => AgentProfileSecretAvailabilityV1::Missing,
                        SecretAvailabilityV1::Unavailable => {
                            AgentProfileSecretAvailabilityV1::Unavailable
                        }
                    };
                    AgentProfileSecretSettingViewV1 {
                        key: setting.key.clone(),
                        kind: setting.kind,
                        availability,
                    }
                })
                .collect(),
        }
    }

    fn load_revisions(&self) -> Result<Vec<AgentProfileRevisionV1>, AgentProfileErrorV1> {
        let mut records = Vec::new();
        for path in profile_json_files(&self.profiles)? {
            let record = read_record::<AgentProfileRevisionV1>(&path)?;
            validate_revision(&record)?;
            if path != self.profile_path(&record.profile_id, &record.revision) {
                return Err(AgentProfileErrorV1::Unavailable);
            }
            records.push(record);
        }
        records.sort_by(|left, right| {
            (&left.profile_id, &left.revision).cmp(&(&right.profile_id, &right.revision))
        });
        Ok(records)
    }

    fn load_assignments(&self) -> Result<Vec<AgentProfileSeatAssignmentV1>, AgentProfileErrorV1> {
        let mut records = Vec::new();
        let mut seats = BTreeSet::new();
        let mut memberships = BTreeSet::new();
        for path in json_files(&self.assignments)? {
            let record = read_record::<AgentProfileSeatAssignmentV1>(&path)?;
            validate_assignment(&record)?;
            if path != self.assignment_path(&record.assignment_id)
                || !self
                    .profile_path(&record.profile.profile_id, &record.profile.revision)
                    .exists()
                || !seats.insert((record.draft_id.clone(), record.seat_id.clone()))
                || !memberships.insert((
                    record.membership.room_id.clone(),
                    record.membership.member_id.clone(),
                ))
            {
                return Err(AgentProfileErrorV1::Unavailable);
            }
            records.push(record);
        }
        records.sort_by(|left, right| left.assignment_id.cmp(&right.assignment_id));
        Ok(records)
    }

    fn profile_path(&self, profile_id: &str, revision: &str) -> PathBuf {
        self.profile_directory(profile_id)
            .join(format!("{}.json", encode_component(revision)))
    }

    fn profile_directory(&self, profile_id: &str) -> PathBuf {
        self.profiles.join(encode_component(profile_id))
    }

    fn assignment_path(&self, assignment_id: &str) -> PathBuf {
        self.assignments.join(format!("{assignment_id}.json"))
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.mutation.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Builds the bounded browser-safe profile publication and catalog surface.
pub fn agent_profile_router(store: AgentProfileStoreV1) -> Router {
    Router::new()
        .route(
            "/api/v1/agent-profiles",
            get(profile_catalog).post(profile_publish),
        )
        .route(
            "/api/v1/agent-profiles/{profile_id}/revisions/{revision}",
            get(profile_revision),
        )
        .route(
            "/api/v1/agent-profile-assignments",
            get(profile_assignments),
        )
        .with_state(store)
}

async fn profile_publish(
    State(store): State<AgentProfileStoreV1>,
    Json(revision): Json<AgentProfileRevisionV1>,
) -> Result<Json<AgentProfileRevisionViewV1>, AgentProfileErrorV1> {
    tokio::task::spawn_blocking(move || store.publish(&revision))
        .await
        .map_err(|_| AgentProfileErrorV1::Unavailable)?
        .map(Json)
}

async fn profile_catalog(
    State(store): State<AgentProfileStoreV1>,
) -> Result<Json<AgentProfileCatalogV1>, AgentProfileErrorV1> {
    tokio::task::spawn_blocking(move || store.catalog())
        .await
        .map_err(|_| AgentProfileErrorV1::Unavailable)?
        .map(Json)
}

async fn profile_revision(
    State(store): State<AgentProfileStoreV1>,
    AxumPath((profile_id, revision)): AxumPath<(String, String)>,
) -> Result<Json<AgentProfileRevisionViewV1>, AgentProfileErrorV1> {
    tokio::task::spawn_blocking(move || store.revision(&profile_id, &revision))
        .await
        .map_err(|_| AgentProfileErrorV1::Unavailable)?
        .map(Json)
}

async fn profile_assignments(
    State(store): State<AgentProfileStoreV1>,
) -> Result<Json<AgentProfileAssignmentCatalogV1>, AgentProfileErrorV1> {
    tokio::task::spawn_blocking(move || store.assignments())
        .await
        .map_err(|_| AgentProfileErrorV1::Unavailable)?
        .map(Json)
}

impl IntoResponse for AgentProfileErrorV1 {
    fn into_response(self) -> Response {
        let status = match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::InvalidProfile | Self::InvalidAssignment => StatusCode::BAD_REQUEST,
            Self::ImmutableRevisionConflict | Self::ImmutableAssignmentConflict => {
                StatusCode::CONFLICT
            }
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        };
        let code = match self {
            Self::InvalidProfile => "agent_profile_invalid",
            Self::ImmutableRevisionConflict => "agent_profile_revision_conflict",
            Self::InvalidAssignment => "agent_profile_assignment_invalid",
            Self::ImmutableAssignmentConflict => "agent_profile_assignment_conflict",
            Self::NotFound => "agent_profile_not_found",
            Self::Unavailable => "agent_profile_store_unavailable",
        };
        (
            status,
            Json(serde_json::json!({ "error": { "code": code } })),
        )
            .into_response()
    }
}

fn validate_revision(revision: &AgentProfileRevisionV1) -> Result<(), AgentProfileErrorV1> {
    if revision.schema != PROFILE_SCHEMA_V1
        || !valid_id(&revision.profile_id)
        || !valid_revision(&revision.revision)
        || !bounded(&revision.display_name, MAX_TEXT_BYTES)
        || revision.non_secret_configuration.len() > 64
        || revision.secret_settings.len() > 64
    {
        return Err(AgentProfileErrorV1::InvalidProfile);
    }
    let mut keys = BTreeSet::new();
    for (key, value) in &revision.non_secret_configuration {
        if !valid_configuration_key(key)
            || sensitive_key(key)
            || !bounded(value, MAX_SETTING_BYTES)
            || credential_shaped_value(value)
            || !keys.insert(key.as_str())
        {
            return Err(AgentProfileErrorV1::InvalidProfile);
        }
    }
    for setting in &revision.secret_settings {
        if setting.kind != SecretKindV1::ModelProvider
            || !valid_setting_key(&setting.key)
            || !sensitive_key(&setting.key)
            || !keys.insert(setting.key.as_str())
        {
            return Err(AgentProfileErrorV1::InvalidProfile);
        }
    }
    Ok(())
}

fn validate_assignment(
    assignment: &AgentProfileSeatAssignmentV1,
) -> Result<(), AgentProfileErrorV1> {
    if assignment.schema != ASSIGNMENT_SCHEMA_V1
        || assignment.assignment_id.parse::<UlidString>().is_err()
        || !valid_id(&assignment.draft_id)
        || !valid_id(&assignment.seat_id)
        || !valid_id(&assignment.profile.profile_id)
        || !valid_revision(&assignment.profile.revision)
        || assignment.membership.room_id.parse::<UlidString>().is_err()
        || assignment
            .membership
            .member_id
            .parse::<UlidString>()
            .is_err()
        || assignment
            .membership
            .principal_id
            .parse::<UlidString>()
            .is_err()
        || !bounded(&assignment.membership.role, MAX_TEXT_BYTES)
        || matches!(
            &assignment.execution,
            AgentExecutionBindingV1::Managed { runner_id }
                if runner_id.parse::<UlidString>().is_err()
        )
    {
        return Err(AgentProfileErrorV1::InvalidAssignment);
    }
    Ok(())
}

fn json_files(root: &Path) -> Result<Vec<PathBuf>, AgentProfileErrorV1> {
    let mut files = fs::read_dir(root)
        .map_err(|_| AgentProfileErrorV1::Unavailable)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    if files.len() > MAX_RECORDS {
        return Err(AgentProfileErrorV1::Unavailable);
    }
    files.sort();
    Ok(files)
}

fn profile_json_files(root: &Path) -> Result<Vec<PathBuf>, AgentProfileErrorV1> {
    let entries = fs::read_dir(root)
        .map_err(|_| AgentProfileErrorV1::Unavailable)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AgentProfileErrorV1::Unavailable)?;
    if entries.len() > MAX_RECORDS {
        return Err(AgentProfileErrorV1::Unavailable);
    }
    let mut files = Vec::new();
    for entry in entries {
        if !entry
            .file_type()
            .map_err(|_| AgentProfileErrorV1::Unavailable)?
            .is_dir()
        {
            return Err(AgentProfileErrorV1::Unavailable);
        }
        let directory =
            prepare_data_directory(&entry.path()).map_err(|_| AgentProfileErrorV1::Unavailable)?;
        files.extend(json_files(&directory)?);
        if files.len() > MAX_RECORDS {
            return Err(AgentProfileErrorV1::Unavailable);
        }
    }
    files.sort();
    Ok(files)
}

fn read_record<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, AgentProfileErrorV1> {
    validate_owner_only_file(path).map_err(|_| AgentProfileErrorV1::Unavailable)?;
    let metadata = fs::metadata(path).map_err(|_| AgentProfileErrorV1::Unavailable)?;
    if metadata.len() == 0 || metadata.len() > MAX_RECORD_BYTES {
        return Err(AgentProfileErrorV1::Unavailable);
    }
    serde_json::from_slice(&fs::read(path).map_err(|_| AgentProfileErrorV1::Unavailable)?)
        .map_err(|_| AgentProfileErrorV1::Unavailable)
}

fn persist_new<T: Serialize>(path: &Path, value: &T) -> Result<(), AgentProfileErrorV1> {
    let encoded = serde_json::to_vec_pretty(value).map_err(|_| AgentProfileErrorV1::Unavailable)?;
    if encoded.is_empty() || u64::try_from(encoded.len()).unwrap_or(u64::MAX) > MAX_RECORD_BYTES {
        return Err(AgentProfileErrorV1::Unavailable);
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(AgentProfileErrorV1::Unavailable)?;
    let temporary = path.with_file_name(format!(".{file_name}.tmp"));
    if temporary.exists() {
        fs::remove_file(&temporary).map_err(|_| AgentProfileErrorV1::Unavailable)?;
    }
    let mut file =
        create_owner_only_file(&temporary).map_err(|_| AgentProfileErrorV1::Unavailable)?;
    file.write_all(&encoded)
        .and_then(|()| file.sync_all())
        .map_err(|_| AgentProfileErrorV1::Unavailable)?;
    fs::rename(&temporary, path).map_err(|_| AgentProfileErrorV1::Unavailable)?;
    sync_directory(path.parent().ok_or(AgentProfileErrorV1::Unavailable)?)
        .map_err(|_| AgentProfileErrorV1::Unavailable)
}

fn validate_profile_id(value: &str) -> Result<(), AgentProfileErrorV1> {
    if valid_id(value) {
        Ok(())
    } else {
        Err(AgentProfileErrorV1::InvalidProfile)
    }
}

fn validate_revision_id(value: &str) -> Result<(), AgentProfileErrorV1> {
    if valid_revision(value) {
        Ok(())
    } else {
        Err(AgentProfileErrorV1::InvalidProfile)
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

fn valid_revision(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn valid_setting_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_uppercase() || *byte == b'_')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn valid_configuration_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
}

fn sensitive_key(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    ["SECRET", "TOKEN", "PASSWORD", "KEY", "CREDENTIAL"]
        .iter()
        .any(|marker| value.contains(&marker.to_ascii_lowercase()))
}

fn credential_shaped_value(value: &str) -> bool {
    let trimmed = value.trim();
    let lowercase = trimmed.to_ascii_lowercase();
    lowercase.starts_with("bearer ")
        || lowercase.starts_with("bearer=")
        || lowercase.starts_with("basic ")
        || lowercase.starts_with("sk-")
        || lowercase.starts_with("sk_")
        || lowercase.starts_with("api_key=")
        || lowercase.starts_with("apikey=")
        || lowercase.starts_with("token=")
        || lowercase.starts_with("secret=")
        || lowercase.starts_with("password=")
        || lowercase.starts_with("credential=")
}

fn bounded(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.contains(['\0', '\r', '\n'])
}

fn encode_component(value: &str) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.bytes() {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}
