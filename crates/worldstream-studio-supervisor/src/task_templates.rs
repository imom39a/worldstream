//! Immutable reusable Task Template revisions and draft-only instantiation.
//!
//! A Task Template snapshots a reviewed draft's exact build choices. This
//! module can create another editable draft, but has no Room creation or daemon
//! mutation capability.

use std::{
    collections::BTreeSet,
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
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use worldstream_protocol::PackReference;
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};

use crate::{
    agent_profiles::{AgentProfileErrorV1, AgentProfileSecretAvailabilityV1, AgentProfileStoreV1},
    room_drafts::{
        AgentAssignmentModeV1, ExactActivityPackDraftValidatorV1, RoomDraftErrorV1,
        RoomDraftFieldErrorV1, RoomDraftSeatReadinessV1, RoomDraftSeatV1, RoomDraftStepV1,
        RoomDraftStoreV1, RoomDraftV1, RoomDraftValidatorV1, validate_draft,
    },
    runner_templates::{CompatibilityV1, RunnerTemplateRegistryV1},
};

const REVISION_SCHEMA_V1: &str = "worldstream/studio-task-template/v1";
const USAGE_SCHEMA_V1: &str = "worldstream/studio-task-template-usage/v1";
const CATALOG_SCHEMA_V1: &str = "worldstream/studio-task-template-catalog/v1";
const MAX_RECORD_BYTES: usize = 256 * 1024;
const MAX_RECORDS: usize = 256;
const MAX_ISSUES: usize = 64;
const MAX_TEXT_BYTES: usize = 256;

/// One immutable Task Template revision copied from a reviewed draft.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTemplateRevisionV1 {
    pub schema: String,
    pub template_id: String,
    pub revision: String,
    pub display_name: String,
    pub source_draft_id: String,
    pub pack: PackReference,
    pub configuration: Value,
    pub seats: Vec<RoomDraftSeatV1>,
    pub readiness: Vec<RoomDraftSeatReadinessV1>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub operator_view: bool,
}

/// Browser request to snapshot one exact reviewed draft under a new revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTemplatePublishRequestV1 {
    pub template_id: String,
    pub revision: String,
    pub display_name: String,
    pub source_draft_id: String,
}

/// Closed dependency assessment for an immutable revision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTemplateDependencyStatusV1 {
    Ready,
    Missing,
    Incompatible,
    Unavailable,
}

/// Safe bounded explanation for one exact dependency problem.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTemplateDependencyIssueV1 {
    pub path: String,
    pub code: String,
    pub message: String,
}

/// Complete current assessment; exact references remain visible when blocked.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTemplateDependencyReportV1 {
    pub status: TaskTemplateDependencyStatusV1,
    pub issues: Vec<TaskTemplateDependencyIssueV1>,
}

impl TaskTemplateDependencyReportV1 {
    #[must_use]
    pub const fn ready() -> Self {
        Self {
            status: TaskTemplateDependencyStatusV1::Ready,
            issues: Vec::new(),
        }
    }
}

/// Current browser-safe view of one immutable revision and its impact.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTemplateRevisionViewV1 {
    pub revision: TaskTemplateRevisionV1,
    pub dependencies: TaskTemplateDependencyReportV1,
    pub used_by_draft_ids: Vec<String>,
}

/// Stable bounded catalog ordered by template and revision identity.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTemplateCatalogV1 {
    pub schema: String,
    pub revisions: Vec<TaskTemplateRevisionViewV1>,
}

/// Request to create one independent draft identity from a revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTemplateInstantiateRequestV1 {
    pub draft_id: String,
}

/// Closed dependency seam used by publication, reuse, and current catalog views.
pub trait TaskTemplateDependencySourceV1: Send + Sync + 'static {
    fn inspect(&self, revision: &TaskTemplateRevisionV1) -> TaskTemplateDependencyReportV1;
}

/// Production dependency source backed by exact installed Pack/Profile/Runner revisions.
#[derive(Clone)]
pub struct InstalledTaskTemplateDependenciesV1 {
    packs: ExactActivityPackDraftValidatorV1,
    profiles: AgentProfileStoreV1,
    runners: RunnerTemplateRegistryV1,
}

impl InstalledTaskTemplateDependenciesV1 {
    #[must_use]
    pub const fn new(
        packs: ExactActivityPackDraftValidatorV1,
        profiles: AgentProfileStoreV1,
        runners: RunnerTemplateRegistryV1,
    ) -> Self {
        Self {
            packs,
            profiles,
            runners,
        }
    }
}

impl TaskTemplateDependencySourceV1 for InstalledTaskTemplateDependenciesV1 {
    fn inspect(&self, revision: &TaskTemplateRevisionV1) -> TaskTemplateDependencyReportV1 {
        let draft = revision.as_draft(revision.source_draft_id.clone(), RoomDraftStepV1::Review);
        let mut issues = match self.packs.validate(&draft) {
            Ok(errors) => errors
                .into_iter()
                .map(|error| dependency_issue(&error.path, &error.code, &error.message))
                .collect::<Vec<_>>(),
            Err(_) => {
                return TaskTemplateDependencyReportV1 {
                    status: TaskTemplateDependencyStatusV1::Unavailable,
                    issues: vec![dependency_issue(
                        "/pack",
                        "dependency_check_unavailable",
                        "The exact Activity Pack dependency could not be checked.",
                    )],
                };
            }
        };
        let installed = self.runners.templates();
        for (index, seat) in revision.seats.iter().enumerate() {
            if seat.principal_kind != Some(worldstream_protocol::PrincipalKind::Agent) {
                continue;
            }
            if let Some(issue) = profile_dependency_issue(&self.profiles, seat, index) {
                issues.push(issue);
            }
            if seat.agent_assignment == Some(AgentAssignmentModeV1::Managed) {
                let runner_path = format!("/seats/{index}/runner_template");
                match &seat.runner_template {
                    Some(runner) => {
                        let exact_installed = installed.iter().any(|manifest| {
                            manifest.template_id == runner.template_id
                                && manifest.revision == runner.revision
                        });
                        if !exact_installed {
                            issues.push(dependency_issue(
                                &runner_path,
                                "runner_template_missing",
                                "The exact Runner Template revision is unavailable.",
                            ));
                        } else if self.runners.compatibility(
                            &runner.template_id,
                            &runner.revision,
                            &revision.pack.id,
                            &revision.pack.version,
                        ) != CompatibilityV1::Compatible
                        {
                            issues.push(dependency_issue(
                                &runner_path,
                                "runner_template_incompatible",
                                "The exact Runner Template revision is incompatible with the pinned Activity Pack.",
                            ));
                        }
                    }
                    None => issues.push(dependency_issue(
                        &runner_path,
                        "runner_template_missing",
                        "An exact Runner Template revision is required for this managed seat.",
                    )),
                }
            }
        }
        dependency_report(issues)
    }
}

fn profile_dependency_issue(
    profiles: &AgentProfileStoreV1,
    seat: &RoomDraftSeatV1,
    index: usize,
) -> Option<TaskTemplateDependencyIssueV1> {
    let path = format!("/seats/{index}/agent_profile");
    let Some(profile) = &seat.agent_profile else {
        return Some(dependency_issue(
            &path,
            "agent_profile_missing",
            "An exact Agent Profile revision is required.",
        ));
    };
    match profiles.revision(&profile.profile_id, &profile.revision) {
        Ok(view)
            if view.secret_settings.iter().all(|setting| {
                setting.availability == AgentProfileSecretAvailabilityV1::Configured
            }) =>
        {
            None
        }
        Ok(view)
            if view.secret_settings.iter().any(|setting| {
                setting.availability == AgentProfileSecretAvailabilityV1::Unavailable
            }) =>
        {
            Some(dependency_issue(
                &path,
                "dependency_check_unavailable",
                "The exact Agent Profile provider settings could not be checked.",
            ))
        }
        Ok(_) => Some(dependency_issue(
            &path,
            "agent_profile_secret_missing",
            "The exact Agent Profile revision has a missing provider setting.",
        )),
        Err(AgentProfileErrorV1::NotFound) => Some(dependency_issue(
            &path,
            "agent_profile_missing",
            "The exact Agent Profile revision is unavailable.",
        )),
        Err(AgentProfileErrorV1::Unavailable) => Some(dependency_issue(
            &path,
            "dependency_check_unavailable",
            "The exact Agent Profile dependency could not be checked.",
        )),
        Err(_) => Some(dependency_issue(
            &path,
            "agent_profile_incompatible",
            "The exact Agent Profile revision is invalid for reuse.",
        )),
    }
}

impl RoomDraftValidatorV1 for InstalledTaskTemplateDependenciesV1 {
    fn validate(
        &self,
        draft: &RoomDraftV1,
    ) -> Result<Vec<RoomDraftFieldErrorV1>, RoomDraftErrorV1> {
        let pack_errors = self.packs.validate(draft)?;
        if !pack_errors.is_empty()
            || draft
                .last_valid_step
                .is_none_or(|step| step < RoomDraftStepV1::Seats)
        {
            return Ok(pack_errors);
        }
        let Some(pack) = draft.pack.clone() else {
            return Ok(pack_errors);
        };
        let report = self.inspect(&TaskTemplateRevisionV1 {
            schema: REVISION_SCHEMA_V1.to_owned(),
            template_id: "draft-validation".to_owned(),
            revision: "draft".to_owned(),
            display_name: "Draft validation".to_owned(),
            source_draft_id: draft.draft_id.clone(),
            pack,
            configuration: draft.configuration.clone(),
            seats: draft.seats.clone(),
            readiness: draft.readiness.clone(),
            operator_view: draft.operator_view,
        });
        if report.status == TaskTemplateDependencyStatusV1::Unavailable {
            return Err(RoomDraftErrorV1::ValidationUnavailable);
        }
        Ok(report
            .issues
            .into_iter()
            .map(|issue| RoomDraftFieldErrorV1 {
                path: issue.path,
                code: issue.code,
                message: issue.message,
            })
            .collect())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct TaskTemplateUsageV1 {
    schema: String,
    template_id: String,
    revision: String,
    draft: RoomDraftV1,
}

/// Closed, pathless Task Template failures.
#[derive(Clone, Debug, Error, PartialEq)]
pub enum TaskTemplateErrorV1 {
    #[error("Task Template request is invalid")]
    Invalid,
    #[error("Task Template source draft is not reviewed")]
    DraftNotReviewed,
    #[error("Task Template revision is immutable")]
    ImmutableRevisionConflict,
    #[error("Task Template draft identity conflicts")]
    DraftConflict,
    #[error("Task Template revision was not found")]
    NotFound,
    #[error("Task Template dependencies are unavailable")]
    DependenciesUnavailable(TaskTemplateDependencyReportV1),
    #[error("Task Template store is unavailable")]
    Unavailable,
}

/// Owner-only immutable revision and usage store.
#[derive(Clone)]
pub struct TaskTemplateStoreV1 {
    revisions: Arc<PathBuf>,
    usages: Arc<PathBuf>,
    drafts: RoomDraftStoreV1,
    dependencies: Arc<dyn TaskTemplateDependencySourceV1>,
    mutation: Arc<Mutex<()>>,
}

impl TaskTemplateStoreV1 {
    /// Opens and validates the complete retained catalog and usage inventory.
    ///
    /// # Errors
    ///
    /// Fails closed for unsafe storage or any malformed retained record.
    pub fn open(
        root: &Path,
        drafts: RoomDraftStoreV1,
        dependencies: impl TaskTemplateDependencySourceV1,
    ) -> Result<Self, TaskTemplateErrorV1> {
        let root = prepare_data_directory(root).map_err(|_| TaskTemplateErrorV1::Unavailable)?;
        let revisions = prepare_data_directory(&root.join("revisions"))
            .map_err(|_| TaskTemplateErrorV1::Unavailable)?;
        let usages = prepare_data_directory(&root.join("usages"))
            .map_err(|_| TaskTemplateErrorV1::Unavailable)?;
        let store = Self {
            revisions: Arc::new(revisions),
            usages: Arc::new(usages),
            drafts,
            dependencies: Arc::new(dependencies),
            mutation: Arc::new(Mutex::new(())),
        };
        store.validate_inventory()?;
        Ok(store)
    }

    /// Publishes an immutable revision from one exact reviewed draft.
    ///
    /// # Errors
    ///
    /// Rejects unreviewed/invalid drafts, dependency problems, changed reuse
    /// of an existing revision identity, or protected-storage failure.
    pub fn publish(
        &self,
        request: &TaskTemplatePublishRequestV1,
    ) -> Result<TaskTemplateRevisionViewV1, TaskTemplateErrorV1> {
        validate_publish_request(request)?;
        let draft = self
            .drafts
            .load(&request.source_draft_id)
            .map_err(|error| map_draft_load_error(&error))?;
        if draft.last_valid_step != Some(RoomDraftStepV1::Review) {
            return Err(TaskTemplateErrorV1::DraftNotReviewed);
        }
        let revision = TaskTemplateRevisionV1 {
            schema: REVISION_SCHEMA_V1.to_owned(),
            template_id: request.template_id.clone(),
            revision: request.revision.clone(),
            display_name: request.display_name.clone(),
            source_draft_id: request.source_draft_id.clone(),
            pack: draft.pack.ok_or(TaskTemplateErrorV1::Invalid)?,
            configuration: draft.configuration,
            seats: draft.seats,
            readiness: draft.readiness,
            operator_view: draft.operator_view,
        };
        validate_revision(&revision)?;
        let report = self.dependencies.inspect(&revision);
        if report.status != TaskTemplateDependencyStatusV1::Ready {
            return Err(TaskTemplateErrorV1::DependenciesUnavailable(report));
        }
        let _guard = self.lock();
        let path = self.revision_path(&revision.template_id, &revision.revision);
        if path.exists() {
            let existing = read_record::<TaskTemplateRevisionV1>(&path)?;
            validate_revision(&existing)?;
            return if existing == revision {
                self.view(existing)
            } else {
                Err(TaskTemplateErrorV1::ImmutableRevisionConflict)
            };
        }
        if self.load_revisions()?.len() >= MAX_RECORDS {
            return Err(TaskTemplateErrorV1::Unavailable);
        }
        persist_new(&path, &revision)?;
        self.view(revision)
    }

    /// Lists every revision with current dependency and usage evidence.
    ///
    /// # Errors
    ///
    /// Fails closed if retained revision or usage storage is malformed.
    pub fn catalog(&self) -> Result<TaskTemplateCatalogV1, TaskTemplateErrorV1> {
        Ok(TaskTemplateCatalogV1 {
            schema: CATALOG_SCHEMA_V1.to_owned(),
            revisions: self
                .load_revisions()?
                .into_iter()
                .map(|revision| self.view(revision))
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    /// Loads one exact revision without fallback or substitution.
    ///
    /// # Errors
    ///
    /// Returns exact not-found or protected-storage failure.
    pub fn revision(
        &self,
        template_id: &str,
        revision: &str,
    ) -> Result<TaskTemplateRevisionViewV1, TaskTemplateErrorV1> {
        validate_id(template_id)?;
        validate_revision_id(revision)?;
        let path = self.revision_path(template_id, revision);
        if !path.exists() {
            return Err(TaskTemplateErrorV1::NotFound);
        }
        let retained = read_record::<TaskTemplateRevisionV1>(&path)?;
        validate_revision(&retained)?;
        if retained.template_id != template_id || retained.revision != revision {
            return Err(TaskTemplateErrorV1::Unavailable);
        }
        self.view(retained)
    }

    /// Creates one independent editable draft and records exact revision usage.
    ///
    /// # Errors
    ///
    /// Blocks missing/incompatible dependencies and never replaces an existing
    /// divergent draft identity.
    pub fn instantiate(
        &self,
        template_id: &str,
        revision_id: &str,
        draft_id: &str,
    ) -> Result<RoomDraftV1, TaskTemplateErrorV1> {
        validate_id(draft_id)?;
        let retained = self.revision(template_id, revision_id)?;
        if retained.dependencies.status != TaskTemplateDependencyStatusV1::Ready {
            return Err(TaskTemplateErrorV1::DependenciesUnavailable(
                retained.dependencies,
            ));
        }
        let draft = retained
            .revision
            .as_draft(draft_id.to_owned(), RoomDraftStepV1::Readiness);
        let usage = TaskTemplateUsageV1 {
            schema: USAGE_SCHEMA_V1.to_owned(),
            template_id: template_id.to_owned(),
            revision: revision_id.to_owned(),
            draft: draft.clone(),
        };
        let _guard = self.lock();
        let usage_path = self.usage_path(draft_id);
        let usage_exists = if usage_path.exists() {
            let existing = read_record::<TaskTemplateUsageV1>(&usage_path)?;
            validate_usage(&existing)?;
            if existing != usage {
                return Err(TaskTemplateErrorV1::DraftConflict);
            }
            true
        } else {
            if self.load_usages()?.len() >= MAX_RECORDS {
                return Err(TaskTemplateErrorV1::Unavailable);
            }
            false
        };
        let created = match self.drafts.create(&draft) {
            Ok(()) => draft,
            Err(RoomDraftErrorV1::Conflict) => {
                let existing = self
                    .drafts
                    .load(draft_id)
                    .map_err(|error| map_draft_load_error(&error))?;
                if existing == draft {
                    existing
                } else {
                    return Err(TaskTemplateErrorV1::DraftConflict);
                }
            }
            Err(RoomDraftErrorV1::ValidationFailed(errors)) => {
                return Err(TaskTemplateErrorV1::DependenciesUnavailable(
                    dependency_report(
                        errors
                            .into_iter()
                            .map(|error| dependency_issue(&error.path, &error.code, &error.message))
                            .collect(),
                    ),
                ));
            }
            Err(_) => return Err(TaskTemplateErrorV1::Unavailable),
        };
        // Publish usage only after the independently durable draft exists. A
        // crash before this point leaves no phantom usage; a crash after draft
        // creation is reconciled by the identical retry above.
        if !usage_exists {
            persist_new(&usage_path, &usage)?;
        }
        Ok(created)
    }

    fn view(
        &self,
        revision: TaskTemplateRevisionV1,
    ) -> Result<TaskTemplateRevisionViewV1, TaskTemplateErrorV1> {
        let dependencies = self.dependencies.inspect(&revision);
        let mut used_by_draft_ids = self
            .load_usages()?
            .into_iter()
            .filter(|usage| {
                usage.template_id == revision.template_id && usage.revision == revision.revision
            })
            .map(|usage| usage.draft.draft_id)
            .collect::<Vec<_>>();
        used_by_draft_ids.sort();
        Ok(TaskTemplateRevisionViewV1 {
            revision,
            dependencies,
            used_by_draft_ids,
        })
    }

    fn validate_inventory(&self) -> Result<(), TaskTemplateErrorV1> {
        let revisions = self.load_revisions()?;
        let mut revision_ids = BTreeSet::new();
        if !revisions.iter().all(|revision| {
            revision_ids.insert((revision.template_id.clone(), revision.revision.clone()))
        }) {
            return Err(TaskTemplateErrorV1::Unavailable);
        }
        let usages = self.load_usages()?;
        let mut draft_ids = BTreeSet::new();
        for usage in usages {
            if !draft_ids.insert(usage.draft.draft_id.clone()) {
                return Err(TaskTemplateErrorV1::Unavailable);
            }
            let Some(revision) = revisions.iter().find(|revision| {
                revision.template_id == usage.template_id && revision.revision == usage.revision
            }) else {
                return Err(TaskTemplateErrorV1::Unavailable);
            };
            if usage.draft
                != revision.as_draft(usage.draft.draft_id.clone(), RoomDraftStepV1::Readiness)
            {
                return Err(TaskTemplateErrorV1::Unavailable);
            }
        }
        Ok(())
    }

    fn load_revisions(&self) -> Result<Vec<TaskTemplateRevisionV1>, TaskTemplateErrorV1> {
        load_records(&self.revisions, validate_revision, |revision| {
            format!(
                "{}--{}.json",
                encode_component(&revision.template_id),
                encode_component(&revision.revision)
            )
        })
    }

    fn load_usages(&self) -> Result<Vec<TaskTemplateUsageV1>, TaskTemplateErrorV1> {
        load_records(&self.usages, validate_usage, |usage| {
            format!("{}.json", encode_component(&usage.draft.draft_id))
        })
    }

    fn revision_path(&self, template_id: &str, revision: &str) -> PathBuf {
        self.revisions.join(format!(
            "{}--{}.json",
            encode_component(template_id),
            encode_component(revision)
        ))
    }

    fn usage_path(&self, draft_id: &str) -> PathBuf {
        self.usages
            .join(format!("{}.json", encode_component(draft_id)))
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.mutation.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl TaskTemplateRevisionV1 {
    fn as_draft(&self, draft_id: String, last_valid_step: RoomDraftStepV1) -> RoomDraftV1 {
        RoomDraftV1 {
            schema: "worldstream/studio-room-draft/v1".to_owned(),
            draft_id,
            pack: Some(self.pack.clone()),
            configuration: self.configuration.clone(),
            seats: self.seats.clone(),
            readiness: self.readiness.clone(),
            operator_view: self.operator_view,
            last_valid_step: Some(last_valid_step),
        }
    }
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if predicates receive a field reference"
)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// Builds bounded catalog, publication, detail, and draft-instantiation routes.
pub fn task_template_router(store: TaskTemplateStoreV1) -> Router {
    Router::new()
        .route(
            "/api/v1/task-templates",
            get(list_templates).post(publish_template),
        )
        .route(
            "/api/v1/task-templates/{template_id}/revisions/{revision}",
            get(get_template),
        )
        .route(
            "/api/v1/task-templates/{template_id}/revisions/{revision}/instantiate",
            post(instantiate_template),
        )
        .layer(DefaultBodyLimit::max(MAX_RECORD_BYTES))
        .with_state(store)
}

async fn list_templates(
    State(store): State<TaskTemplateStoreV1>,
) -> Result<Json<TaskTemplateCatalogV1>, TaskTemplateErrorV1> {
    tokio::task::spawn_blocking(move || store.catalog())
        .await
        .map_err(|_| TaskTemplateErrorV1::Unavailable)?
        .map(Json)
}

async fn publish_template(
    State(store): State<TaskTemplateStoreV1>,
    Json(request): Json<TaskTemplatePublishRequestV1>,
) -> Result<Json<TaskTemplateRevisionViewV1>, TaskTemplateErrorV1> {
    tokio::task::spawn_blocking(move || store.publish(&request))
        .await
        .map_err(|_| TaskTemplateErrorV1::Unavailable)?
        .map(Json)
}

async fn get_template(
    State(store): State<TaskTemplateStoreV1>,
    AxumPath((template_id, revision)): AxumPath<(String, String)>,
) -> Result<Json<TaskTemplateRevisionViewV1>, TaskTemplateErrorV1> {
    tokio::task::spawn_blocking(move || store.revision(&template_id, &revision))
        .await
        .map_err(|_| TaskTemplateErrorV1::Unavailable)?
        .map(Json)
}

async fn instantiate_template(
    State(store): State<TaskTemplateStoreV1>,
    AxumPath((template_id, revision)): AxumPath<(String, String)>,
    Json(request): Json<TaskTemplateInstantiateRequestV1>,
) -> Result<Json<RoomDraftV1>, TaskTemplateErrorV1> {
    tokio::task::spawn_blocking(move || {
        store.instantiate(&template_id, &revision, &request.draft_id)
    })
    .await
    .map_err(|_| TaskTemplateErrorV1::Unavailable)?
    .map(Json)
}

impl IntoResponse for TaskTemplateErrorV1 {
    fn into_response(self) -> Response {
        let (status, code, message, retryable, issues) = match self {
            Self::Invalid => (
                StatusCode::BAD_REQUEST,
                "task_template_invalid",
                "the Task Template request is invalid",
                false,
                Vec::new(),
            ),
            Self::DraftNotReviewed => (
                StatusCode::CONFLICT,
                "task_template_draft_not_reviewed",
                "the source Task draft must be explicitly reviewed",
                false,
                Vec::new(),
            ),
            Self::ImmutableRevisionConflict => (
                StatusCode::CONFLICT,
                "task_template_revision_conflict",
                "the immutable Task Template revision already has different choices",
                false,
                Vec::new(),
            ),
            Self::DraftConflict => (
                StatusCode::CONFLICT,
                "task_template_draft_conflict",
                "the requested independent draft identity already has different content",
                false,
                Vec::new(),
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "task_template_not_found",
                "the exact Task Template revision is unavailable",
                false,
                Vec::new(),
            ),
            Self::DependenciesUnavailable(report) => {
                let unavailable = report.status == TaskTemplateDependencyStatusV1::Unavailable;
                (
                    if unavailable {
                        StatusCode::SERVICE_UNAVAILABLE
                    } else {
                        StatusCode::UNPROCESSABLE_ENTITY
                    },
                    "task_template_dependencies_unavailable",
                    "the exact pinned dependencies block this operation",
                    unavailable,
                    report.issues,
                )
            }
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "task_template_store_unavailable",
                "the protected Task Template store is unavailable",
                true,
                Vec::new(),
            ),
        };
        (
            status,
            Json(serde_json::json!({
                "error": {
                    "code": code,
                    "message": message,
                    "retryable": retryable,
                    "dependency_issues": issues,
                }
            })),
        )
            .into_response()
    }
}

fn validate_publish_request(
    request: &TaskTemplatePublishRequestV1,
) -> Result<(), TaskTemplateErrorV1> {
    validate_id(&request.template_id)?;
    validate_revision_id(&request.revision)?;
    validate_id(&request.source_draft_id)?;
    if !bounded(&request.display_name) {
        return Err(TaskTemplateErrorV1::Invalid);
    }
    Ok(())
}

fn validate_revision(revision: &TaskTemplateRevisionV1) -> Result<(), TaskTemplateErrorV1> {
    validate_id(&revision.template_id)?;
    validate_revision_id(&revision.revision)?;
    validate_id(&revision.source_draft_id)?;
    if revision.schema != REVISION_SCHEMA_V1
        || !bounded(&revision.display_name)
        || revision.pack.id.is_empty()
        || revision.pack.version.is_empty()
        || !is_digest(&revision.pack.digest)
        || revision.seats.len() > 64
        || revision.readiness.len() != revision.seats.len()
        || contains_credential_field(&revision.configuration)
    {
        return Err(TaskTemplateErrorV1::Invalid);
    }
    let draft = revision.as_draft(revision.source_draft_id.clone(), RoomDraftStepV1::Review);
    validate_draft(&draft).map_err(|_| TaskTemplateErrorV1::Invalid)?;
    if draft.seats.iter().any(|seat| {
        seat.principal_kind == Some(worldstream_protocol::PrincipalKind::Agent)
            && seat.agent_profile.is_none()
    }) {
        return Err(TaskTemplateErrorV1::Invalid);
    }
    let mut seat_ids = BTreeSet::new();
    let readiness_matches = revision.readiness.iter().all(|policy| {
        revision.seats.iter().any(|seat| {
            seat.seat_id == policy.seat_id
                && seat.role == policy.role
                && seat.required == policy.required
        })
    });
    if !revision
        .seats
        .iter()
        .all(|seat| seat_ids.insert(seat.seat_id.as_str()))
        || !readiness_matches
    {
        return Err(TaskTemplateErrorV1::Invalid);
    }
    Ok(())
}

fn validate_usage(usage: &TaskTemplateUsageV1) -> Result<(), TaskTemplateErrorV1> {
    if usage.schema != USAGE_SCHEMA_V1 {
        return Err(TaskTemplateErrorV1::Unavailable);
    }
    validate_id(&usage.template_id)?;
    validate_revision_id(&usage.revision)?;
    validate_id(&usage.draft.draft_id)?;
    validate_draft(&usage.draft).map_err(|_| TaskTemplateErrorV1::Unavailable)?;
    if usage.draft.last_valid_step != Some(RoomDraftStepV1::Readiness) {
        return Err(TaskTemplateErrorV1::Unavailable);
    }
    Ok(())
}

fn load_records<T>(
    root: &Path,
    validate: fn(&T) -> Result<(), TaskTemplateErrorV1>,
    expected_file_name: fn(&T) -> String,
) -> Result<Vec<T>, TaskTemplateErrorV1>
where
    T: for<'de> Deserialize<'de>,
{
    let entries = fs::read_dir(root)
        .map_err(|_| TaskTemplateErrorV1::Unavailable)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| TaskTemplateErrorV1::Unavailable)?;
    let mut paths = entries
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    if paths.len() > MAX_RECORDS {
        return Err(TaskTemplateErrorV1::Unavailable);
    }
    paths
        .into_iter()
        .map(|path| {
            let value = read_record::<T>(&path)?;
            validate(&value)?;
            if path.file_name().and_then(|value| value.to_str())
                != Some(expected_file_name(&value).as_str())
            {
                return Err(TaskTemplateErrorV1::Unavailable);
            }
            Ok(value)
        })
        .collect()
}

fn read_record<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, TaskTemplateErrorV1> {
    validate_owner_only_file(path).map_err(|_| TaskTemplateErrorV1::Unavailable)?;
    let metadata = fs::metadata(path).map_err(|_| TaskTemplateErrorV1::Unavailable)?;
    if metadata.len() == 0 || metadata.len() > u64::try_from(MAX_RECORD_BYTES).unwrap_or(u64::MAX) {
        return Err(TaskTemplateErrorV1::Unavailable);
    }
    serde_json::from_slice(&fs::read(path).map_err(|_| TaskTemplateErrorV1::Unavailable)?)
        .map_err(|_| TaskTemplateErrorV1::Unavailable)
}

fn persist_new<T: Serialize>(path: &Path, value: &T) -> Result<(), TaskTemplateErrorV1> {
    let encoded = serde_json::to_vec_pretty(value).map_err(|_| TaskTemplateErrorV1::Unavailable)?;
    if encoded.is_empty() || encoded.len() > MAX_RECORD_BYTES {
        return Err(TaskTemplateErrorV1::Unavailable);
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(TaskTemplateErrorV1::Unavailable)?;
    let temporary = path.with_file_name(format!(".{file_name}.tmp"));
    if temporary.exists() {
        fs::remove_file(&temporary).map_err(|_| TaskTemplateErrorV1::Unavailable)?;
    }
    let mut file =
        create_owner_only_file(&temporary).map_err(|_| TaskTemplateErrorV1::Unavailable)?;
    file.write_all(&encoded)
        .and_then(|()| file.sync_all())
        .map_err(|_| TaskTemplateErrorV1::Unavailable)?;
    drop(file);
    let mut result = fs::hard_link(&temporary, path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            TaskTemplateErrorV1::ImmutableRevisionConflict
        } else {
            TaskTemplateErrorV1::Unavailable
        }
    });
    if result.is_ok() {
        result = sync_directory(path.parent().unwrap_or(Path::new(".")))
            .map_err(|_| TaskTemplateErrorV1::Unavailable);
    }
    let _ = fs::remove_file(temporary);
    result
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
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

fn dependency_report(
    mut issues: Vec<TaskTemplateDependencyIssueV1>,
) -> TaskTemplateDependencyReportV1 {
    issues.truncate(MAX_ISSUES);
    let status = if issues.is_empty() {
        TaskTemplateDependencyStatusV1::Ready
    } else if issues
        .iter()
        .any(|issue| issue.code == "dependency_check_unavailable")
    {
        TaskTemplateDependencyStatusV1::Unavailable
    } else if issues
        .iter()
        .any(|issue| issue.code.ends_with("_missing") || issue.code == "revision_unavailable")
    {
        TaskTemplateDependencyStatusV1::Missing
    } else {
        TaskTemplateDependencyStatusV1::Incompatible
    };
    TaskTemplateDependencyReportV1 { status, issues }
}

fn dependency_issue(path: &str, code: &str, message: &str) -> TaskTemplateDependencyIssueV1 {
    TaskTemplateDependencyIssueV1 {
        path: path.chars().take(512).collect(),
        code: code.chars().take(64).collect(),
        message: message.chars().take(MAX_TEXT_BYTES).collect(),
    }
}

fn map_draft_load_error(error: &RoomDraftErrorV1) -> TaskTemplateErrorV1 {
    match error {
        RoomDraftErrorV1::NotFound => TaskTemplateErrorV1::NotFound,
        RoomDraftErrorV1::InvalidDraft
        | RoomDraftErrorV1::ValidationFailed(_)
        | RoomDraftErrorV1::Conflict => TaskTemplateErrorV1::Invalid,
        RoomDraftErrorV1::ValidationUnavailable | RoomDraftErrorV1::Unavailable => {
            TaskTemplateErrorV1::Unavailable
        }
    }
}

fn validate_id(value: &str) -> Result<(), TaskTemplateErrorV1> {
    if !value.is_empty()
        && value.len() <= 64
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        Ok(())
    } else {
        Err(TaskTemplateErrorV1::Invalid)
    }
}

fn validate_revision_id(value: &str) -> Result<(), TaskTemplateErrorV1> {
    if !value.is_empty()
        && value.len() <= 64
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        Ok(())
    } else {
        Err(TaskTemplateErrorV1::Invalid)
    }
}

fn bounded(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_TEXT_BYTES && !value.contains(['\0', '\r', '\n'])
}

fn is_digest(value: &str) -> bool {
    value.strip_prefix("blake3:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn contains_credential_field(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            let normalized = key.replace('-', "_").to_ascii_lowercase();
            normalized.contains("secret")
                || normalized.contains("bearer")
                || normalized.contains("password")
                || normalized.contains("api_key")
                || normalized.contains("apikey")
                || normalized.contains("token")
                || contains_credential_field(value)
        }),
        Value::Array(values) => values.iter().any(contains_credential_field),
        Value::String(value) => looks_like_credential_value(value),
        _ => false,
    }
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

fn encode_component(value: &str) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.bytes() {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}
