//! Owner-only persisted Studio Room-creation drafts.
//!
//! This module stores planning state only. It has no daemon Room, principal,
//! capability, or authority creation capability.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use worldstream_protocol::{PackReference, PrincipalKind, UlidString};
use worldstream_runtime::{
    create_owner_only_file, create_owner_only_renameable_file, prepare_data_directory,
    validate_owner_only_file,
};

use crate::activity_packs::{ActivityPackProxyErrorV1, DaemonActivityPackSource};

const DRAFT_SCHEMA_V1: &str = "worldstream/studio-room-draft/v1";
const DRAFT_RESPONSE_VERSION_V1: &str = "studio_room_draft.v1";
const MAX_DRAFT_BYTES: usize = 128 * 1024;
const MAX_CONFIGURATION_BYTES: usize = 64 * 1024;
const MAX_SEATS: usize = 64;
const MAX_TEXT_BYTES: usize = 256;

/// The five stable wizard steps.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomDraftStepV1 {
    Activity,
    Configuration,
    Seats,
    Readiness,
    Review,
}

/// Execution assignment for an Agent participant. It remains separate from an
/// exact Agent Profile revision and from Membership Action authority.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAssignmentModeV1 {
    External,
    Managed,
}

/// Exact immutable Agent Profile revision selected for one Agent seat.
///
/// This is planning metadata only. It grants neither Membership Action
/// authority nor Runner control authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProfileRevisionReferenceV1 {
    pub profile_id: String,
    pub revision: String,
}

/// Exact immutable Runner Template revision selected for a managed Agent seat.
///
/// This is assignment intent only. It carries no Runner authority or process
/// identity and cannot authorize an Invocation or a domain Action.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerTemplateRevisionReferenceV1 {
    pub template_id: String,
    pub revision: String,
}

/// One stable seat derived from an exact pack revision's declared Role.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomDraftSeatV1 {
    pub seat_id: String,
    pub role: String,
    pub required: bool,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_kind: Option<PrincipalKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_assignment: Option<AgentAssignmentModeV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_profile: Option<AgentProfileRevisionReferenceV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_template: Option<RunnerTemplateRevisionReferenceV1>,
}

/// Pre-Room readiness policy for one stable seat. This is not live presence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomDraftSeatReadinessV1 {
    pub seat_id: String,
    pub role: String,
    pub required: bool,
}

/// One bounded schema or exact-pack validation error safe for Studio.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomDraftFieldErrorV1 {
    pub path: String,
    pub code: String,
    pub message: String,
}

/// Complete editable persisted draft. The pack reference is always exact.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomDraftV1 {
    pub schema: String,
    pub draft_id: String,
    pub pack: Option<PackReference>,
    pub configuration: Value,
    pub seats: Vec<RoomDraftSeatV1>,
    pub readiness: Vec<RoomDraftSeatReadinessV1>,
    pub last_valid_step: Option<RoomDraftStepV1>,
}

/// Owned review projection. It cannot mutate the editable draft it snapshots.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomDraftReviewV1 {
    pub pack: Option<PackReference>,
    pub configuration: Value,
    pub seats: Vec<RoomDraftSeatV1>,
    pub readiness: Vec<RoomDraftSeatReadinessV1>,
}

/// Versioned persisted draft plus an immutable snapshot-shaped review value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomDraftResponseV1 {
    pub version: String,
    pub draft: RoomDraftV1,
    pub review: RoomDraftReviewV1,
}

impl RoomDraftResponseV1 {
    pub(crate) fn from_draft(draft: RoomDraftV1) -> Self {
        let review = RoomDraftReviewV1 {
            pack: draft.pack.clone(),
            configuration: draft.configuration.clone(),
            seats: draft.seats.clone(),
            readiness: draft.readiness.clone(),
        };
        Self {
            version: DRAFT_RESPONSE_VERSION_V1.to_owned(),
            draft,
            review,
        }
    }
}

/// Closed, pathless draft-store failures.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum RoomDraftErrorV1 {
    #[error("Room draft is invalid")]
    InvalidDraft,
    #[error("Room draft validation failed")]
    ValidationFailed(Vec<RoomDraftFieldErrorV1>),
    #[error("Room draft validation is unavailable")]
    ValidationUnavailable,
    #[error("Room draft was not found")]
    NotFound,
    #[error("Room draft identity already exists")]
    Conflict,
    #[error("Room draft store is unavailable")]
    Unavailable,
}

/// Exact-pack configuration validator injected into the draft persistence seam.
pub trait RoomDraftValidatorV1: Send + Sync + 'static {
    /// Validates the exact pinned pack and configuration without substitution.
    ///
    /// # Errors
    ///
    /// Returns bounded field errors or a closed validation-unavailable error.
    fn validate(&self, draft: &RoomDraftV1)
    -> Result<Vec<RoomDraftFieldErrorV1>, RoomDraftErrorV1>;
}

/// Concrete validator backed by the daemon's exact-digest catalog detail API.
#[derive(Clone)]
pub struct ExactActivityPackDraftValidatorV1 {
    source: Arc<dyn DaemonActivityPackSource>,
}

impl ExactActivityPackDraftValidatorV1 {
    /// Binds draft validation to the same bounded exact-pack source as Studio.
    #[must_use]
    pub fn new(source: impl DaemonActivityPackSource) -> Self {
        Self {
            source: Arc::new(source),
        }
    }
}

impl RoomDraftValidatorV1 for ExactActivityPackDraftValidatorV1 {
    fn validate(
        &self,
        draft: &RoomDraftV1,
    ) -> Result<Vec<RoomDraftFieldErrorV1>, RoomDraftErrorV1> {
        let Some(pack) = &draft.pack else {
            return Ok(vec![field_error(
                "/pack",
                "required",
                "Select an exact Activity Pack revision.",
            )]);
        };
        let detail = match self.source.revision(&pack.digest) {
            Ok(detail) => detail,
            Err(
                ActivityPackProxyErrorV1::InvalidRevision
                | ActivityPackProxyErrorV1::RevisionUnavailable,
            ) => {
                return Ok(vec![field_error(
                    "/pack/digest",
                    "revision_unavailable",
                    "The exact Activity Pack revision is unavailable.",
                )]);
            }
            Err(
                ActivityPackProxyErrorV1::AuthorityUnavailable
                | ActivityPackProxyErrorV1::DaemonUnavailable
                | ActivityPackProxyErrorV1::InvalidResponse,
            ) => return Err(RoomDraftErrorV1::ValidationUnavailable),
        };
        let exact = &detail.revision.summary.pack;
        if exact.id != pack.id || exact.version != pack.version || exact.digest != pack.digest {
            return Ok(vec![field_error(
                "/pack",
                "revision_mismatch",
                "The saved Activity Pack reference does not match the exact installed revision.",
            )]);
        }
        if !detail.revision.summary.selectable_for_new_rooms {
            return Ok(vec![field_error(
                "/pack/digest",
                "revision_not_selectable",
                "The exact Activity Pack revision is not selectable for a new Room.",
            )]);
        }
        let mut errors = Vec::new();
        if draft
            .last_valid_step
            .is_some_and(|step| step >= RoomDraftStepV1::Configuration)
        {
            validate_json_schema(
                &detail.revision.configuration_schema.schema,
                &draft.configuration,
                "/configuration",
                &mut errors,
            );
        }
        if draft
            .last_valid_step
            .is_some_and(|step| step >= RoomDraftStepV1::Seats)
        {
            validate_declared_seats(&detail.revision.roles, &draft.seats, &mut errors);
        }
        Ok(bound_field_errors(errors))
    }
}

/// Owner-only durable draft store, safe to reopen after Supervisor restart.
#[derive(Clone)]
pub struct RoomDraftStoreV1 {
    root: Arc<PathBuf>,
    mutation: Arc<Mutex<()>>,
    validator: Arc<dyn RoomDraftValidatorV1>,
}

impl RoomDraftStoreV1 {
    /// Opens or creates the owner-only draft directory.
    ///
    /// # Errors
    ///
    /// Returns a pathless unavailable error when the directory is unsafe.
    pub fn open(
        root: &Path,
        validator: impl RoomDraftValidatorV1,
    ) -> Result<Self, RoomDraftErrorV1> {
        let root = prepare_data_directory(root).map_err(|_| RoomDraftErrorV1::Unavailable)?;
        Ok(Self {
            root: Arc::new(root),
            mutation: Arc::new(Mutex::new(())),
            validator: Arc::new(validator),
        })
    }

    /// Loads one exact draft identifier without fallback.
    ///
    /// # Errors
    ///
    /// Returns not-found, invalid-draft, or unavailable without exposing paths.
    pub fn load(&self, draft_id: &str) -> Result<RoomDraftV1, RoomDraftErrorV1> {
        validate_identifier(draft_id)?;
        let path = self.draft_path(draft_id);
        if !path.exists() {
            return Err(RoomDraftErrorV1::NotFound);
        }
        validate_owner_only_file(&path).map_err(|_| RoomDraftErrorV1::Unavailable)?;
        let metadata = fs::metadata(&path).map_err(|_| RoomDraftErrorV1::Unavailable)?;
        if metadata.len() > u64::try_from(MAX_DRAFT_BYTES).unwrap_or(u64::MAX) {
            return Err(RoomDraftErrorV1::InvalidDraft);
        }
        let encoded = fs::read(path).map_err(|_| RoomDraftErrorV1::Unavailable)?;
        let draft: RoomDraftV1 =
            serde_json::from_slice(&encoded).map_err(|_| RoomDraftErrorV1::InvalidDraft)?;
        validate_draft(&draft)?;
        if draft.draft_id != draft_id {
            return Err(RoomDraftErrorV1::InvalidDraft);
        }
        Ok(draft)
    }

    /// Loads one exact draft and revalidates its current exact dependencies
    /// immediately before an effectful operation.
    pub(crate) fn load_for_operation(
        &self,
        draft_id: &str,
    ) -> Result<RoomDraftV1, RoomDraftErrorV1> {
        let draft = self.load(draft_id)?;
        let field_errors = self.validator.validate(&draft)?;
        if field_errors.is_empty() {
            Ok(draft)
        } else {
            Err(RoomDraftErrorV1::ValidationFailed(bound_field_errors(
                field_errors,
            )))
        }
    }

    /// Atomically persists one validated editable draft.
    ///
    /// # Errors
    ///
    /// Rejects invalid shapes, raw credential fields, inconsistent readiness,
    /// or unsafe local storage through a closed pathless error.
    pub fn save(&self, draft: &RoomDraftV1) -> Result<(), RoomDraftErrorV1> {
        validate_draft(draft)?;
        let field_errors = self.validator.validate(draft)?;
        if !field_errors.is_empty() {
            return Err(RoomDraftErrorV1::ValidationFailed(bound_field_errors(
                field_errors,
            )));
        }
        let encoded =
            serde_json::to_vec_pretty(draft).map_err(|_| RoomDraftErrorV1::InvalidDraft)?;
        if encoded.len() > MAX_DRAFT_BYTES {
            return Err(RoomDraftErrorV1::InvalidDraft);
        }
        let _guard = self.lock();
        let target = self.draft_path(&draft.draft_id);
        let temporary = self.root.join(format!(".{}.tmp", draft.draft_id));
        if temporary.exists() {
            fs::remove_file(&temporary).map_err(|_| RoomDraftErrorV1::Unavailable)?;
        }
        let mut file = create_owner_only_renameable_file(&temporary)
            .map_err(|_| RoomDraftErrorV1::Unavailable)?;
        file.write_all(&encoded)
            .and_then(|()| file.sync_all())
            .map_err(|_| RoomDraftErrorV1::Unavailable)?;
        drop(file);
        let result = replace_file(&temporary, &target)
            .and_then(|()| sync_directory(self.root.as_ref()))
            .map_err(|_| RoomDraftErrorV1::Unavailable);
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    /// Publishes one new independent draft without replacing an existing identity.
    ///
    /// # Errors
    ///
    /// Rejects invalid/unsafe content, unavailable dependencies, an existing
    /// draft identity, or protected-storage failure.
    pub fn create(&self, draft: &RoomDraftV1) -> Result<(), RoomDraftErrorV1> {
        validate_draft(draft)?;
        let field_errors = self.validator.validate(draft)?;
        if !field_errors.is_empty() {
            return Err(RoomDraftErrorV1::ValidationFailed(bound_field_errors(
                field_errors,
            )));
        }
        let encoded =
            serde_json::to_vec_pretty(draft).map_err(|_| RoomDraftErrorV1::InvalidDraft)?;
        if encoded.len() > MAX_DRAFT_BYTES {
            return Err(RoomDraftErrorV1::InvalidDraft);
        }
        let _guard = self.lock();
        let target = self.draft_path(&draft.draft_id);
        if target.exists() {
            return Err(RoomDraftErrorV1::Conflict);
        }
        let temporary = self.root.join(format!(".{}.create.tmp", draft.draft_id));
        if temporary.exists() {
            fs::remove_file(&temporary).map_err(|_| RoomDraftErrorV1::Unavailable)?;
        }
        let mut file =
            create_owner_only_file(&temporary).map_err(|_| RoomDraftErrorV1::Unavailable)?;
        file.write_all(&encoded)
            .and_then(|()| file.sync_all())
            .map_err(|_| RoomDraftErrorV1::Unavailable)?;
        drop(file);
        let mut result = fs::hard_link(&temporary, &target).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                RoomDraftErrorV1::Conflict
            } else {
                RoomDraftErrorV1::Unavailable
            }
        });
        if result.is_ok() {
            result = sync_directory(self.root.as_ref()).map_err(|_| RoomDraftErrorV1::Unavailable);
        }
        let _ = fs::remove_file(temporary);
        result
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.mutation.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn draft_path(&self, draft_id: &str) -> PathBuf {
        self.root.join(format!("{draft_id}.json"))
    }
}

/// Builds the bounded draft-only API.
pub fn room_draft_router(store: RoomDraftStoreV1) -> Router {
    Router::new()
        .route(
            "/api/v1/room-drafts/{draft_id}",
            get(load_draft).put(save_draft),
        )
        .layer(DefaultBodyLimit::max(MAX_DRAFT_BYTES))
        .with_state(store)
}

async fn load_draft(
    State(store): State<RoomDraftStoreV1>,
    AxumPath(draft_id): AxumPath<String>,
) -> Result<Json<RoomDraftResponseV1>, RoomDraftErrorV1> {
    store
        .load(&draft_id)
        .map(RoomDraftResponseV1::from_draft)
        .map(Json)
}

async fn save_draft(
    State(store): State<RoomDraftStoreV1>,
    AxumPath(draft_id): AxumPath<String>,
    Json(draft): Json<RoomDraftV1>,
) -> Result<Json<RoomDraftResponseV1>, RoomDraftErrorV1> {
    if draft.draft_id != draft_id {
        return Err(RoomDraftErrorV1::InvalidDraft);
    }
    store.save(&draft)?;
    Ok(Json(RoomDraftResponseV1::from_draft(draft)))
}

#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
    retryable: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    field_errors: Vec<RoomDraftFieldErrorV1>,
}

impl IntoResponse for RoomDraftErrorV1 {
    fn into_response(self) -> Response {
        let (status, code, message, retryable, field_errors) = match self {
            Self::InvalidDraft => (
                StatusCode::BAD_REQUEST,
                "room_draft_invalid",
                "the Room draft is invalid",
                false,
                Vec::new(),
            ),
            Self::ValidationFailed(errors) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "room_draft_validation_failed",
                "the Room draft has invalid fields",
                false,
                bound_field_errors(errors),
            ),
            Self::ValidationUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "room_draft_validation_unavailable",
                "the exact Activity Pack configuration validator is unavailable",
                true,
                Vec::new(),
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "room_draft_not_found",
                "the exact Room draft is unavailable",
                false,
                Vec::new(),
            ),
            Self::Conflict => (
                StatusCode::CONFLICT,
                "room_draft_identity_conflict",
                "the Room draft identity already exists",
                false,
                Vec::new(),
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "room_draft_store_unavailable",
                "the protected Room draft store is unavailable",
                true,
                Vec::new(),
            ),
        };
        (
            status,
            Json(ErrorEnvelope {
                error: ErrorBody {
                    code,
                    message,
                    retryable,
                    field_errors,
                },
            }),
        )
            .into_response()
    }
}

pub(crate) fn validate_draft(draft: &RoomDraftV1) -> Result<(), RoomDraftErrorV1> {
    if draft.schema != DRAFT_SCHEMA_V1 {
        return Err(RoomDraftErrorV1::InvalidDraft);
    }
    validate_identifier(&draft.draft_id)?;
    if let Some(pack) = &draft.pack
        && (!is_bounded_text(&pack.id)
            || !is_bounded_text(&pack.version)
            || !is_digest(&pack.digest))
    {
        return Err(RoomDraftErrorV1::InvalidDraft);
    }
    let configuration_bytes =
        serde_json::to_vec(&draft.configuration).map_err(|_| RoomDraftErrorV1::InvalidDraft)?;
    if configuration_bytes.len() > MAX_CONFIGURATION_BYTES
        || contains_credential_field(&draft.configuration)
        || draft.seats.len() > MAX_SEATS
        || draft.readiness.len() > MAX_SEATS
    {
        return Err(RoomDraftErrorV1::InvalidDraft);
    }
    let mut seats = BTreeMap::new();
    let mut principals = BTreeSet::new();
    for seat in &draft.seats {
        validate_identifier(&seat.seat_id)?;
        if !is_bounded_text(&seat.role)
            || !is_bounded_text(&seat.display_name)
            || seats.insert(seat.seat_id.as_str(), seat).is_some()
            || seat.principal_id.is_some() != seat.principal_kind.is_some()
            || match seat.principal_kind {
                Some(PrincipalKind::Agent) => {
                    seat.agent_assignment.is_none()
                        || seat.agent_profile.as_ref().is_some_and(|profile| {
                            !valid_profile_id(&profile.profile_id)
                                || !valid_profile_revision(&profile.revision)
                        })
                        || seat.runner_template.as_ref().is_some_and(|runner| {
                            !valid_profile_id(&runner.template_id)
                                || !valid_profile_revision(&runner.revision)
                        })
                        || (seat.agent_assignment == Some(AgentAssignmentModeV1::Managed))
                            != seat.runner_template.is_some()
                }
                Some(PrincipalKind::Human) | None => {
                    seat.agent_assignment.is_some()
                        || seat.agent_profile.is_some()
                        || seat.runner_template.is_some()
                }
            }
        {
            return Err(RoomDraftErrorV1::InvalidDraft);
        }
        if let Some(principal_id) = &seat.principal_id {
            principal_id
                .parse::<UlidString>()
                .map_err(|_| RoomDraftErrorV1::InvalidDraft)?;
            if !principals.insert(principal_id.as_str()) {
                return Err(RoomDraftErrorV1::InvalidDraft);
            }
        }
    }
    let mut readiness_ids = BTreeSet::new();
    for readiness in &draft.readiness {
        let Some(seat) = seats.get(readiness.seat_id.as_str()) else {
            return Err(RoomDraftErrorV1::InvalidDraft);
        };
        if !readiness_ids.insert(readiness.seat_id.as_str())
            || readiness.role != seat.role
            || readiness.required != seat.required
        {
            return Err(RoomDraftErrorV1::InvalidDraft);
        }
    }
    if readiness_ids.len() != seats.len() {
        return Err(RoomDraftErrorV1::InvalidDraft);
    }
    if draft
        .last_valid_step
        .is_some_and(|step| step >= RoomDraftStepV1::Activity)
        && draft.pack.is_none()
    {
        return Err(RoomDraftErrorV1::InvalidDraft);
    }
    if draft
        .last_valid_step
        .is_some_and(|step| step >= RoomDraftStepV1::Readiness)
        && draft
            .seats
            .iter()
            .any(|seat| seat.required && seat.principal_id.is_none())
    {
        return Err(RoomDraftErrorV1::InvalidDraft);
    }
    Ok(())
}

fn valid_profile_id(value: &str) -> bool {
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

fn valid_profile_revision(value: &str) -> bool {
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

fn bound_field_errors(errors: Vec<RoomDraftFieldErrorV1>) -> Vec<RoomDraftFieldErrorV1> {
    errors
        .into_iter()
        .take(64)
        .map(|error| RoomDraftFieldErrorV1 {
            path: bounded_error_text(error.path, 512, "/configuration"),
            code: bounded_error_text(error.code, 64, "invalid"),
            message: bounded_error_text(error.message, 256, "the field is invalid"),
        })
        .collect()
}

fn bounded_error_text(value: String, maximum: usize, fallback: &str) -> String {
    if value.is_empty() || value.len() > maximum {
        fallback.to_owned()
    } else {
        value
    }
}

fn validate_json_schema(
    schema: &Value,
    instance: &Value,
    path: &str,
    errors: &mut Vec<RoomDraftFieldErrorV1>,
) {
    if errors.len() >= 64 {
        return;
    }
    let Some(schema) = schema.as_object() else {
        errors.push(field_error(
            path,
            "schema_unsupported",
            "The installed configuration schema is unsupported.",
        ));
        return;
    };
    if let Some(expected) = schema.get("const")
        && expected != instance
    {
        errors.push(field_error(
            path,
            "const",
            "The field must use the declared fixed value.",
        ));
    }
    if let Some(choices) = schema.get("enum").and_then(Value::as_array)
        && !choices.contains(instance)
    {
        errors.push(field_error(
            path,
            "enum",
            "The field must use one of the declared values.",
        ));
    }
    let Some(expected_type) = schema.get("type").and_then(Value::as_str) else {
        errors.push(field_error(
            path,
            "schema_unsupported",
            "The installed configuration schema is unsupported.",
        ));
        return;
    };
    if !matches_type(instance, expected_type) {
        errors.push(field_error(
            path,
            "type",
            "The field has the wrong value type.",
        ));
        return;
    }
    match expected_type {
        "object" => validate_object_schema(schema, instance, path, errors),
        "array" => validate_array_schema(schema, instance, path, errors),
        "string" => validate_string_schema(schema, instance, path, errors),
        "number" | "integer" => validate_number_schema(schema, instance, path, errors),
        "boolean" | "null" => {}
        _ => errors.push(field_error(
            path,
            "schema_unsupported",
            "The installed configuration schema is unsupported.",
        )),
    }
}

fn validate_object_schema(
    schema: &serde_json::Map<String, Value>,
    instance: &Value,
    path: &str,
    errors: &mut Vec<RoomDraftFieldErrorV1>,
) {
    let Some(object) = instance.as_object() else {
        return;
    };
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for field in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(field) {
                errors.push(field_error(
                    &child_pointer(path, field),
                    "required",
                    "The field is required.",
                ));
            }
        }
    }
    if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
        for field in object.keys() {
            if !properties.contains_key(field) {
                errors.push(field_error(
                    &child_pointer(path, field),
                    "additional_property",
                    "The field is not declared by this exact revision.",
                ));
            }
        }
    }
    for (field, field_schema) in properties {
        if let Some(value) = object.get(&field) {
            validate_json_schema(&field_schema, value, &child_pointer(path, &field), errors);
        }
    }
}

fn validate_array_schema(
    schema: &serde_json::Map<String, Value>,
    instance: &Value,
    path: &str,
    errors: &mut Vec<RoomDraftFieldErrorV1>,
) {
    let Some(values) = instance.as_array() else {
        return;
    };
    if schema
        .get("minItems")
        .and_then(Value::as_u64)
        .is_some_and(|minimum| u64::try_from(values.len()).unwrap_or(u64::MAX) < minimum)
    {
        errors.push(field_error(
            path,
            "min_items",
            "The field has too few items.",
        ));
    }
    if schema
        .get("maxItems")
        .and_then(Value::as_u64)
        .is_some_and(|maximum| u64::try_from(values.len()).unwrap_or(u64::MAX) > maximum)
    {
        errors.push(field_error(
            path,
            "max_items",
            "The field has too many items.",
        ));
    }
    if let Some(item_schema) = schema.get("items") {
        for (index, value) in values.iter().enumerate() {
            validate_json_schema(item_schema, value, &format!("{path}/{index}"), errors);
        }
    }
}

fn validate_string_schema(
    schema: &serde_json::Map<String, Value>,
    instance: &Value,
    path: &str,
    errors: &mut Vec<RoomDraftFieldErrorV1>,
) {
    let Some(value) = instance.as_str() else {
        return;
    };
    if schema
        .get("minLength")
        .and_then(Value::as_u64)
        .is_some_and(|minimum| u64::try_from(value.chars().count()).unwrap_or(u64::MAX) < minimum)
    {
        errors.push(field_error(
            path,
            "min_length",
            "The field is shorter than allowed.",
        ));
    }
    if schema
        .get("maxLength")
        .and_then(Value::as_u64)
        .is_some_and(|maximum| u64::try_from(value.chars().count()).unwrap_or(u64::MAX) > maximum)
    {
        errors.push(field_error(
            path,
            "max_length",
            "The field is longer than allowed.",
        ));
    }
}

fn validate_number_schema(
    schema: &serde_json::Map<String, Value>,
    instance: &Value,
    path: &str,
    errors: &mut Vec<RoomDraftFieldErrorV1>,
) {
    let Some(value) = instance.as_f64() else {
        return;
    };
    if schema
        .get("minimum")
        .and_then(Value::as_f64)
        .is_some_and(|minimum| value < minimum)
    {
        errors.push(field_error(
            path,
            "minimum",
            "The field is below the declared minimum.",
        ));
    }
    if schema
        .get("maximum")
        .and_then(Value::as_f64)
        .is_some_and(|maximum| value > maximum)
    {
        errors.push(field_error(
            path,
            "maximum",
            "The field is above the declared maximum.",
        ));
    }
}

fn matches_type(value: &Value, expected: &str) -> bool {
    match expected {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}

fn child_pointer(parent: &str, field: &str) -> String {
    format!("{parent}/{}", field.replace('~', "~0").replace('/', "~1"))
}

fn field_error(path: &str, code: &str, message: &str) -> RoomDraftFieldErrorV1 {
    RoomDraftFieldErrorV1 {
        path: path.to_owned(),
        code: code.to_owned(),
        message: message.to_owned(),
    }
}

fn validate_declared_seats(
    roles: &[worldstream_protocol::ActivityPackCatalogRole],
    seats: &[RoomDraftSeatV1],
    errors: &mut Vec<RoomDraftFieldErrorV1>,
) {
    let declared = roles
        .iter()
        .map(|role| (role.role.as_str(), role))
        .collect::<BTreeMap<_, _>>();
    for (index, seat) in seats.iter().enumerate() {
        if !declared.contains_key(seat.role.as_str()) {
            errors.push(field_error(
                &format!("/seats/{index}/role"),
                "undeclared_role",
                "The seat Role is not declared by this exact revision.",
            ));
        }
    }
    for role in roles {
        let matching = seats.iter().filter(|seat| seat.role == role.role);
        let count = u32::try_from(matching.clone().count()).unwrap_or(u32::MAX);
        let required =
            u32::try_from(matching.filter(|seat| seat.required).count()).unwrap_or(u32::MAX);
        if count < role.minimum {
            errors.push(field_error(
                "/seats",
                "role_minimum",
                "A declared Role has fewer seats than its minimum.",
            ));
        }
        if count > role.maximum {
            errors.push(field_error(
                "/seats",
                "role_maximum",
                "A declared Role has more seats than its maximum.",
            ));
        }
        if required != role.minimum {
            errors.push(field_error(
                "/seats",
                "required_cardinality",
                "Required seats must match the declared Role minimum.",
            ));
        }
    }
}

fn validate_identifier(value: &str) -> Result<(), RoomDraftErrorV1> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || !value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return Err(RoomDraftErrorV1::InvalidDraft);
    }
    Ok(())
}

fn is_bounded_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_TEXT_BYTES
}

fn is_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("blake3:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn contains_credential_field(value: &Value) -> bool {
    const CREDENTIAL_KEYS: [&str; 7] = [
        "api_key",
        "authorization",
        "bearer",
        "credential",
        "password",
        "secret",
        "token",
    ];
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            let normalized = key.replace('-', "_").to_ascii_lowercase();
            CREDENTIAL_KEYS
                .iter()
                .any(|credential| normalized.contains(credential))
                || contains_credential_field(value)
        }),
        Value::Array(values) => values.iter().any(contains_credential_field),
        Value::String(value) => looks_like_credential_value(value),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(unix)]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(source, target)
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    let source = source.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "source path is not Unicode",
        )
    })?;
    let target = target.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "target path is not Unicode",
        )
    })?;
    winsafe::MoveFileEx(
        source,
        Some(target),
        winsafe::co::MOVEFILE::REPLACE_EXISTING,
    )
    .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(windows)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt as _};

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
}

fn looks_like_credential_value(value: &str) -> bool {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("bearer ")
        || lower.starts_with("sk-")
        || lower.starts_with("sk_")
        || lower.starts_with("ghp_")
        || lower.starts_with("github_pat_")
        || lower.starts_with("xoxb-")
        || lower.starts_with("xoxp-")
        || value.starts_with("AKIA")
        || (value.starts_with("eyJ") && value.matches('.').count() == 2)
    {
        return true;
    }
    value.len() >= 32
        && !value.bytes().any(|byte| byte.is_ascii_whitespace())
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b'+' | b'=')
        })
        && value.bytes().any(|byte| byte.is_ascii_alphabetic())
        && value.bytes().any(|byte| byte.is_ascii_digit())
}
