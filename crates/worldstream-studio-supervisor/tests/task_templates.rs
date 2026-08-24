use std::{
    collections::BTreeMap,
    fs,
    sync::{Arc, Mutex, PoisonError},
};

use axum::{body::Body, http::Request};
use http_body_util::BodyExt as _;
use serde_json::json;
use tempfile::tempdir;
use tower::ServiceExt as _;
use worldstream_protocol::{ActivityPackCatalogResponse, ActivityPackCatalogRevisionResponse};
use worldstream_studio_supervisor::{
    activity_packs::{ActivityPackProxyErrorV1, DaemonActivityPackSource},
    agent_profiles::{AgentProfileRevisionV1, AgentProfileSecretSettingV1, AgentProfileStoreV1},
    room_drafts::{RoomDraftErrorV1, RoomDraftStoreV1, RoomDraftV1, RoomDraftValidatorV1},
    runner_templates::RunnerTemplateRegistryV1,
    secrets::{FileSecretVaultV1, SecretKindV1},
    task_templates::{
        InstalledTaskTemplateDependenciesV1, TaskTemplateDependencyIssueV1,
        TaskTemplateDependencyReportV1, TaskTemplateDependencySourceV1,
        TaskTemplateDependencyStatusV1, TaskTemplateErrorV1, TaskTemplatePublishRequestV1,
        TaskTemplateRevisionV1, TaskTemplateStoreV1, task_template_router,
    },
};

const DIGEST: &str = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[derive(Clone, Copy)]
struct ValidDraft;

impl RoomDraftValidatorV1 for ValidDraft {
    fn validate(
        &self,
        _draft: &RoomDraftV1,
    ) -> Result<
        Vec<worldstream_studio_supervisor::room_drafts::RoomDraftFieldErrorV1>,
        RoomDraftErrorV1,
    > {
        Ok(Vec::new())
    }
}

fn reviewed_draft() -> RoomDraftV1 {
    serde_json::from_value(json!({
        "schema": "worldstream/studio-room-draft/v1",
        "draft_id": "source-draft",
        "pack": { "id": "counter", "version": "2.0.0", "digest": DIGEST },
        "configuration": { "initial_value": 0, "maximum_value": 8 },
        "seats": [{
            "seat_id": "analyst-1", "role": "analyst", "required": true,
            "display_name": "Analyst", "principal_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "principal_kind": "agent", "agent_assignment": "managed",
            "agent_profile": { "profile_id": "careful-counter", "revision": "profile-r2" },
            "runner_template": { "template_id": "local-runner", "revision": "runner-r3" }
        }],
        "readiness": [{ "seat_id": "analyst-1", "role": "analyst", "required": true }],
        "last_valid_step": "review"
    }))
    .unwrap_or_else(|error| unreachable!("reviewed draft: {error}"))
}

#[derive(Clone, Copy)]
enum DependencyMode {
    Ready,
    Missing,
    Incompatible,
    Unavailable,
}

#[derive(Clone)]
struct Dependencies(Arc<Mutex<DependencyMode>>);

impl TaskTemplateDependencySourceV1 for Dependencies {
    fn inspect(
        &self,
        _revision: &worldstream_studio_supervisor::task_templates::TaskTemplateRevisionV1,
    ) -> TaskTemplateDependencyReportV1 {
        match *self.0.lock().unwrap_or_else(PoisonError::into_inner) {
            DependencyMode::Ready => TaskTemplateDependencyReportV1::ready(),
            DependencyMode::Missing => TaskTemplateDependencyReportV1 {
                status: TaskTemplateDependencyStatusV1::Missing,
                issues: vec![TaskTemplateDependencyIssueV1 {
                    path: "/seats/0/agent_profile".to_owned(),
                    code: "agent_profile_missing".to_owned(),
                    message: "The exact Agent Profile revision is unavailable.".to_owned(),
                }],
            },
            DependencyMode::Incompatible => TaskTemplateDependencyReportV1 {
                status: TaskTemplateDependencyStatusV1::Incompatible,
                issues: vec![TaskTemplateDependencyIssueV1 {
                    path: "/seats/0/runner_template".to_owned(),
                    code: "runner_template_incompatible".to_owned(),
                    message: "The exact Runner Template revision is incompatible.".to_owned(),
                }],
            },
            DependencyMode::Unavailable => TaskTemplateDependencyReportV1 {
                status: TaskTemplateDependencyStatusV1::Unavailable,
                issues: vec![TaskTemplateDependencyIssueV1 {
                    path: "/seats/0/agent_profile".to_owned(),
                    code: "dependency_check_unavailable".to_owned(),
                    message: "The exact Agent Profile dependency could not be checked.".to_owned(),
                }],
            },
        }
    }
}

#[derive(Clone)]
struct FixedPack(ActivityPackCatalogRevisionResponse);

impl DaemonActivityPackSource for FixedPack {
    fn catalog(&self) -> Result<ActivityPackCatalogResponse, ActivityPackProxyErrorV1> {
        Ok(ActivityPackCatalogResponse {
            version: "activity_pack_catalog.v1".to_owned(),
            revisions: vec![self.0.revision.summary.clone()],
        })
    }

    fn revision(
        &self,
        _digest: &str,
    ) -> Result<ActivityPackCatalogRevisionResponse, ActivityPackProxyErrorV1> {
        Ok(self.0.clone())
    }
}

fn production_pack() -> FixedPack {
    FixedPack(
        serde_json::from_value(json!({
            "version": "activity_pack_catalog.v1",
            "revision": {
                "summary": {
                    "pack": { "id": "counter", "version": "2.0.0", "digest": DIGEST },
                    "name": "Counter", "selectable_for_new_rooms": true,
                    "runnable_for_retained_rooms": true
                },
                "roles": [{ "role": "analyst", "minimum": 1, "maximum": 1 }],
                "configuration_schema": {
                    "schema_id": "counter.config.v2",
                    "schema_digest": format!("blake3:{}", "b".repeat(64)),
                    "schema": {
                        "type": "object",
                        "properties": {
                            "initial_value": { "type": "integer", "minimum": 0 },
                            "maximum_value": { "type": "integer", "maximum": 8 }
                        },
                        "required": ["initial_value", "maximum_value"],
                        "additionalProperties": false
                    }
                },
                "actions": []
            }
        }))
        .unwrap_or_else(|error| unreachable!("pack fixture: {error}")),
    )
}

fn runner_registry(root: &std::path::Path, exact_pack_revision: &str) -> RunnerTemplateRegistryV1 {
    let owner = root.join("owner");
    fs::create_dir_all(&owner).unwrap_or_else(|error| unreachable!("owner root: {error}"));
    let executable = owner.join("runner");
    fs::write(&executable, b"approved-runner")
        .unwrap_or_else(|error| unreachable!("runner executable: {error}"));
    let manifest = json!({
        "schema": "worldstream/runner-template/v1",
        "template_id": "local-runner", "revision": "runner-r3",
        "display_name": "Local Runner",
        "executable": {
            "path": executable,
            "blake3": blake3::hash(b"approved-runner").to_hex().to_string()
        },
        "compatibility": [{
            "activity_pack_id": "counter", "exact_revisions": [exact_pack_revision]
        }],
        "capacity": { "maximum_concurrent_invocations": 1 },
        "health": { "path": "/health", "timeout_ms": 50, "stale_after_ms": 1000 },
        "non_secret_environment": {}, "secret_environment": [],
        "instances": [{ "instance_id": "runner-one", "health_address": "127.0.0.1:39001" }]
    });
    fs::write(
        owner.join("local-runner.json"),
        serde_json::to_vec(&manifest).unwrap_or_else(|error| unreachable!("manifest: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("write manifest: {error}"));
    RunnerTemplateRegistryV1::open(&root.join("installed"), &owner)
        .unwrap_or_else(|error| unreachable!("runner registry: {error:?}"))
}

fn publish_request(revision: &str) -> TaskTemplatePublishRequestV1 {
    TaskTemplatePublishRequestV1 {
        template_id: "counter-team".to_owned(),
        revision: revision.to_owned(),
        display_name: "Counter team".to_owned(),
        source_draft_id: "source-draft".to_owned(),
    }
}

fn retained_usage_fixture(
    root: &std::path::Path,
) -> (std::path::PathBuf, RoomDraftStoreV1, Dependencies) {
    let drafts = RoomDraftStoreV1::open(&root.join("drafts"), ValidDraft)
        .unwrap_or_else(|error| unreachable!("draft store: {error:?}"));
    drafts
        .save(&reviewed_draft())
        .unwrap_or_else(|error| unreachable!("save source: {error:?}"));
    let dependencies = Dependencies(Arc::new(Mutex::new(DependencyMode::Ready)));
    let templates = root.join("templates");
    let store = TaskTemplateStoreV1::open(&templates, drafts.clone(), dependencies.clone())
        .unwrap_or_else(|error| unreachable!("template store: {error:?}"));
    store
        .publish(&publish_request("r1"))
        .unwrap_or_else(|error| unreachable!("publish: {error:?}"));
    store
        .instantiate("counter-team", "r1", "retained-draft")
        .unwrap_or_else(|error| unreachable!("instantiate: {error:?}"));
    (templates, drafts, dependencies)
}

fn only_json_file(root: &std::path::Path) -> std::path::PathBuf {
    fs::read_dir(root)
        .unwrap_or_else(|error| unreachable!("record directory: {error}"))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .unwrap_or_else(|| unreachable!("retained JSON record"))
}

#[test]
fn immutable_revision_round_trips_exact_reviewed_choices_after_restart() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let drafts = RoomDraftStoreV1::open(&directory.path().join("drafts"), ValidDraft)
        .unwrap_or_else(|error| unreachable!("draft store: {error:?}"));
    drafts
        .save(&reviewed_draft())
        .unwrap_or_else(|error| unreachable!("save source draft: {error:?}"));
    let dependencies = Dependencies(Arc::new(Mutex::new(DependencyMode::Ready)));
    let root = directory.path().join("templates");
    let store = TaskTemplateStoreV1::open(&root, drafts.clone(), dependencies.clone())
        .unwrap_or_else(|error| unreachable!("template store: {error:?}"));

    let published = store
        .publish(&publish_request("r1"))
        .unwrap_or_else(|error| unreachable!("publish template: {error:?}"));
    assert_eq!(published.revision.pack.digest, DIGEST);
    assert_eq!(published.revision.seats[0].role, "analyst");
    assert_eq!(
        published.revision.seats[0]
            .agent_profile
            .as_ref()
            .map(|profile| profile.revision.as_str()),
        Some("profile-r2")
    );
    assert_eq!(
        published.revision.seats[0]
            .runner_template
            .as_ref()
            .map(|runner| runner.revision.as_str()),
        Some("runner-r3")
    );
    assert_eq!(store.publish(&publish_request("r1")), Ok(published.clone()));
    drop(store);

    let reopened = TaskTemplateStoreV1::open(&root, drafts.clone(), dependencies)
        .unwrap_or_else(|error| unreachable!("reopen template store: {error:?}"));
    assert_eq!(
        reopened
            .revision("counter-team", "r1")
            .unwrap_or_else(|error| unreachable!("load revision: {error:?}")),
        published
    );

    let mut changed = reviewed_draft();
    changed.configuration = json!({ "initial_value": 2, "maximum_value": 8 });
    drafts
        .save(&changed)
        .unwrap_or_else(|error| unreachable!("save changed source: {error:?}"));
    assert_eq!(
        reopened.publish(&publish_request("r1")),
        Err(TaskTemplateErrorV1::ImmutableRevisionConflict)
    );
}

#[test]
fn instantiate_creates_an_independent_editable_draft_and_records_usage_only() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let drafts = RoomDraftStoreV1::open(&directory.path().join("drafts"), ValidDraft)
        .unwrap_or_else(|error| unreachable!("draft store: {error:?}"));
    drafts
        .save(&reviewed_draft())
        .unwrap_or_else(|error| unreachable!("save source: {error:?}"));
    let template_root = directory.path().join("templates");
    let dependencies = Dependencies(Arc::new(Mutex::new(DependencyMode::Ready)));
    let store = TaskTemplateStoreV1::open(&template_root, drafts.clone(), dependencies.clone())
        .unwrap_or_else(|error| unreachable!("template store: {error:?}"));
    let published = store
        .publish(&publish_request("r1"))
        .unwrap_or_else(|error| unreachable!("publish: {error:?}"));

    let instantiated = store
        .instantiate("counter-team", "r1", "independent-draft")
        .unwrap_or_else(|error| unreachable!("instantiate: {error:?}"));
    assert_eq!(instantiated.draft_id, "independent-draft");
    assert_eq!(
        instantiated.last_valid_step,
        Some(worldstream_studio_supervisor::room_drafts::RoomDraftStepV1::Readiness)
    );
    assert_eq!(instantiated.pack, Some(published.revision.pack.clone()));
    assert_eq!(instantiated.seats, published.revision.seats);

    let mut edited = instantiated.clone();
    edited.configuration = json!({ "initial_value": 7, "maximum_value": 8 });
    edited.last_valid_step =
        Some(worldstream_studio_supervisor::room_drafts::RoomDraftStepV1::Activity);
    drafts
        .save(&edited)
        .unwrap_or_else(|error| unreachable!("edit independent draft: {error:?}"));
    let retained = store
        .revision("counter-team", "r1")
        .unwrap_or_else(|error| unreachable!("retained revision: {error:?}"));
    assert_eq!(
        retained.revision.configuration,
        json!({ "initial_value": 0, "maximum_value": 8 })
    );
    assert_eq!(retained.used_by_draft_ids, vec!["independent-draft"]);

    // Simulate a crash after the draft's durable no-replace publication but
    // before usage publication. The identical retry records the one retained
    // revision relationship and survives Supervisor restart.
    let mut recovered = instantiated.clone();
    recovered.draft_id = "recovered-draft".to_owned();
    drafts
        .create(&recovered)
        .unwrap_or_else(|error| unreachable!("pre-publish recovered draft: {error:?}"));
    assert_eq!(
        store.instantiate("counter-team", "r1", "recovered-draft"),
        Ok(recovered)
    );
    drop(store);
    let store = TaskTemplateStoreV1::open(&template_root, drafts.clone(), dependencies)
        .unwrap_or_else(|error| unreachable!("restart template store: {error:?}"));
    assert_eq!(
        store
            .revision("counter-team", "r1")
            .unwrap_or_else(|error| unreachable!("restart usage: {error:?}"))
            .used_by_draft_ids,
        vec!["independent-draft", "recovered-draft"]
    );

    let mut conflicting = reviewed_draft();
    conflicting.draft_id = "conflicting-draft".to_owned();
    conflicting.configuration = json!({ "initial_value": 4, "maximum_value": 8 });
    drafts
        .create(&conflicting)
        .unwrap_or_else(|error| unreachable!("create conflicting draft: {error:?}"));
    assert_eq!(
        store.instantiate("counter-team", "r1", "conflicting-draft"),
        Err(TaskTemplateErrorV1::DraftConflict)
    );
    assert_eq!(
        store
            .revision("counter-team", "r1")
            .unwrap_or_else(|error| unreachable!("usage after conflict: {error:?}"))
            .used_by_draft_ids,
        vec!["independent-draft", "recovered-draft"]
    );
}

#[test]
fn restart_rejects_a_record_whose_filename_does_not_bind_its_exact_identity() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let drafts = RoomDraftStoreV1::open(&directory.path().join("drafts"), ValidDraft)
        .unwrap_or_else(|error| unreachable!("draft store: {error:?}"));
    drafts
        .save(&reviewed_draft())
        .unwrap_or_else(|error| unreachable!("save source: {error:?}"));
    let root = directory.path().join("templates");
    let dependencies = Dependencies(Arc::new(Mutex::new(DependencyMode::Ready)));
    let store = TaskTemplateStoreV1::open(&root, drafts.clone(), dependencies.clone())
        .unwrap_or_else(|error| unreachable!("template store: {error:?}"));
    store
        .publish(&publish_request("r1"))
        .unwrap_or_else(|error| unreachable!("publish: {error:?}"));
    drop(store);
    let revision_directory = root.join("revisions");
    let retained = fs::read_dir(&revision_directory)
        .unwrap_or_else(|error| unreachable!("revision directory: {error}"))
        .next()
        .unwrap_or_else(|| unreachable!("retained revision"))
        .unwrap_or_else(|error| unreachable!("revision entry: {error}"))
        .path();
    fs::copy(retained, revision_directory.join("duplicate.json"))
        .unwrap_or_else(|error| unreachable!("duplicate record: {error}"));

    assert!(matches!(
        TaskTemplateStoreV1::open(&root, drafts, dependencies),
        Err(TaskTemplateErrorV1::Unavailable)
    ));
}

#[test]
fn restart_rejects_wrong_path_duplicate_missing_revision_and_altered_usage_snapshot() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));

    let wrong_path_root = directory.path().join("wrong-path");
    let (templates, drafts, dependencies) = retained_usage_fixture(&wrong_path_root);
    let usage = only_json_file(&templates.join("usages"));
    fs::rename(&usage, usage.with_file_name("wrong-path.json"))
        .unwrap_or_else(|error| unreachable!("rename usage: {error}"));
    assert!(matches!(
        TaskTemplateStoreV1::open(&templates, drafts, dependencies),
        Err(TaskTemplateErrorV1::Unavailable)
    ));

    let duplicate_root = directory.path().join("duplicate");
    let (templates, drafts, dependencies) = retained_usage_fixture(&duplicate_root);
    let usage = only_json_file(&templates.join("usages"));
    fs::copy(&usage, usage.with_file_name("duplicate.json"))
        .unwrap_or_else(|error| unreachable!("duplicate usage: {error}"));
    assert!(matches!(
        TaskTemplateStoreV1::open(&templates, drafts, dependencies),
        Err(TaskTemplateErrorV1::Unavailable)
    ));

    let missing_revision_root = directory.path().join("missing-revision");
    let (templates, drafts, dependencies) = retained_usage_fixture(&missing_revision_root);
    fs::remove_file(only_json_file(&templates.join("revisions")))
        .unwrap_or_else(|error| unreachable!("remove revision: {error}"));
    assert!(matches!(
        TaskTemplateStoreV1::open(&templates, drafts, dependencies),
        Err(TaskTemplateErrorV1::Unavailable)
    ));

    let altered_root = directory.path().join("altered-snapshot");
    let (templates, drafts, dependencies) = retained_usage_fixture(&altered_root);
    let usage = only_json_file(&templates.join("usages"));
    let mut record: serde_json::Value = serde_json::from_slice(
        &fs::read(&usage).unwrap_or_else(|error| unreachable!("read usage: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("parse usage: {error}"));
    record["draft"]["configuration"]["initial_value"] = json!(3);
    fs::write(
        &usage,
        serde_json::to_vec_pretty(&record)
            .unwrap_or_else(|error| unreachable!("encode usage: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("alter usage: {error}"));
    assert!(matches!(
        TaskTemplateStoreV1::open(&templates, drafts, dependencies),
        Err(TaskTemplateErrorV1::Unavailable)
    ));
}

#[test]
fn credential_shaped_configuration_values_are_never_retained() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let drafts = RoomDraftStoreV1::open(&directory.path().join("drafts"), ValidDraft)
        .unwrap_or_else(|error| unreachable!("draft store: {error:?}"));
    let mut source = reviewed_draft();
    source.configuration = json!({
        "initial_value": 0,
        "provider": "sk-live-123456789012345678901234567890"
    });
    assert_eq!(drafts.save(&source), Err(RoomDraftErrorV1::InvalidDraft));
}

fn assert_dependency_issue(
    report: &TaskTemplateDependencyReportV1,
    status: TaskTemplateDependencyStatusV1,
    code: &str,
) {
    assert_eq!(report.status, status);
    assert!(report.issues.iter().any(|issue| issue.code == code));
}

#[test]
fn production_dependencies_enforce_pack_schema_profile_secret_and_runner_compatibility() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let vault_root = directory.path().join("vault");
    let vault = FileSecretVaultV1::open(&vault_root)
        .unwrap_or_else(|error| unreachable!("vault: {error:?}"));
    let reference = vault
        .store(SecretKindV1::ModelProvider, b"provider-credential")
        .unwrap_or_else(|error| unreachable!("provider secret: {error:?}"));
    let profiles = AgentProfileStoreV1::open(&directory.path().join("profiles"), vault.clone())
        .unwrap_or_else(|error| unreachable!("profiles: {error:?}"));
    profiles
        .publish(&AgentProfileRevisionV1 {
            schema: "worldstream/studio-agent-profile/v1".to_owned(),
            profile_id: "careful-counter".to_owned(),
            revision: "profile-r2".to_owned(),
            display_name: "Careful Counter".to_owned(),
            non_secret_configuration: BTreeMap::new(),
            secret_settings: vec![AgentProfileSecretSettingV1 {
                key: "MODEL_PROVIDER_TOKEN".to_owned(),
                kind: SecretKindV1::ModelProvider,
                reference: reference.clone(),
            }],
            host_contract:
                worldstream_studio_supervisor::agent_profiles::AgentHostContractV1::default(),
        })
        .unwrap_or_else(|error| unreachable!("publish profile: {error:?}"));
    let compatible = runner_registry(&directory.path().join("compatible-runner"), "2.0.0");
    let dependencies = InstalledTaskTemplateDependenciesV1::new(
        worldstream_studio_supervisor::room_drafts::ExactActivityPackDraftValidatorV1::new(
            production_pack(),
        ),
        profiles.clone(),
        compatible,
    );
    let source = reviewed_draft();
    let revision = TaskTemplateRevisionV1 {
        schema: "worldstream/studio-task-template/v1".to_owned(),
        template_id: "counter-team".to_owned(),
        revision: "r1".to_owned(),
        display_name: "Counter team".to_owned(),
        source_draft_id: source.draft_id,
        pack: source.pack.unwrap_or_else(|| unreachable!("pack")),
        configuration: source.configuration,
        seats: source.seats,
        readiness: source.readiness,
    };
    assert_eq!(
        dependencies.inspect(&revision),
        TaskTemplateDependencyReportV1::ready()
    );

    let mut invalid_configuration = revision.clone();
    invalid_configuration.configuration = json!({ "initial_value": 0, "maximum_value": 99 });
    let config_report = dependencies.inspect(&invalid_configuration);
    assert_dependency_issue(
        &config_report,
        TaskTemplateDependencyStatusV1::Incompatible,
        "maximum",
    );

    let incompatible = InstalledTaskTemplateDependenciesV1::new(
        worldstream_studio_supervisor::room_drafts::ExactActivityPackDraftValidatorV1::new(
            production_pack(),
        ),
        profiles,
        runner_registry(&directory.path().join("incompatible-runner"), "1.0.0"),
    );
    let runner_report = incompatible.inspect(&revision);
    assert_dependency_issue(
        &runner_report,
        TaskTemplateDependencyStatusV1::Incompatible,
        "runner_template_incompatible",
    );

    let provider_secret = vault_root.join(format!("model-provider-{}.secret", reference.as_str()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(&provider_secret, fs::Permissions::from_mode(0o644))
            .unwrap_or_else(|error| unreachable!("make provider secret unsafe: {error}"));
        let unavailable_report = dependencies.inspect(&revision);
        assert_dependency_issue(
            &unavailable_report,
            TaskTemplateDependencyStatusV1::Unavailable,
            "dependency_check_unavailable",
        );
        fs::set_permissions(&provider_secret, fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|error| unreachable!("restore provider secret: {error}"));
    }
    fs::remove_file(provider_secret)
        .unwrap_or_else(|error| unreachable!("remove provider secret: {error}"));
    let profile_report = dependencies.inspect(&revision);
    assert_dependency_issue(
        &profile_report,
        TaskTemplateDependencyStatusV1::Missing,
        "agent_profile_secret_missing",
    );
}

#[test]
fn missing_and_incompatible_exact_dependencies_stay_visible_and_block_reuse() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let drafts = RoomDraftStoreV1::open(&directory.path().join("drafts"), ValidDraft)
        .unwrap_or_else(|error| unreachable!("draft store: {error:?}"));
    drafts
        .save(&reviewed_draft())
        .unwrap_or_else(|error| unreachable!("save source: {error:?}"));
    let mode = Arc::new(Mutex::new(DependencyMode::Ready));
    let store = TaskTemplateStoreV1::open(
        &directory.path().join("templates"),
        drafts,
        Dependencies(Arc::clone(&mode)),
    )
    .unwrap_or_else(|error| unreachable!("template store: {error:?}"));
    store
        .publish(&publish_request("r1"))
        .unwrap_or_else(|error| unreachable!("publish: {error:?}"));

    *mode.lock().unwrap_or_else(PoisonError::into_inner) = DependencyMode::Missing;
    let visible = store
        .revision("counter-team", "r1")
        .unwrap_or_else(|error| unreachable!("visible missing revision: {error:?}"));
    assert_eq!(
        visible.dependencies.status,
        TaskTemplateDependencyStatusV1::Missing
    );
    assert_eq!(visible.revision.pack.version, "2.0.0");
    assert!(matches!(
        store.instantiate("counter-team", "r1", "blocked-draft"),
        Err(TaskTemplateErrorV1::DependenciesUnavailable(_))
    ));

    *mode.lock().unwrap_or_else(PoisonError::into_inner) = DependencyMode::Incompatible;
    assert!(matches!(
        store.publish(&publish_request("r2")),
        Err(TaskTemplateErrorV1::DependenciesUnavailable(_))
    ));
    assert_eq!(
        store
            .revision("counter-team", "r1")
            .unwrap_or_else(|error| unreachable!("visible incompatible revision: {error:?}"))
            .dependencies
            .status,
        TaskTemplateDependencyStatusV1::Incompatible
    );
}

#[tokio::test]
async fn bounded_http_workflow_publishes_and_instantiates_without_a_room_creation_route() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let drafts = RoomDraftStoreV1::open(&directory.path().join("drafts"), ValidDraft)
        .unwrap_or_else(|error| unreachable!("draft store: {error:?}"));
    drafts
        .save(&reviewed_draft())
        .unwrap_or_else(|error| unreachable!("save source: {error:?}"));
    let mode = Arc::new(Mutex::new(DependencyMode::Ready));
    let router = task_template_router(
        TaskTemplateStoreV1::open(
            &directory.path().join("templates"),
            drafts,
            Dependencies(Arc::clone(&mode)),
        )
        .unwrap_or_else(|error| unreachable!("template store: {error:?}")),
    );

    let published = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/task-templates")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&publish_request("r1"))
                        .unwrap_or_else(|error| unreachable!("publish JSON: {error}")),
                ))
                .unwrap_or_else(|error| unreachable!("publish request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("publish response: {error}"));
    assert_eq!(published.status(), 200);

    let instantiated = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/task-templates/counter-team/revisions/r1/instantiate")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"draft_id":"http-draft"}"#))
                .unwrap_or_else(|error| unreachable!("instantiate request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("instantiate response: {error}"));
    assert_eq!(instantiated.status(), 200);
    let body = instantiated
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| unreachable!("instantiate body: {error}"))
        .to_bytes();
    assert!(
        body.windows("http-draft".len())
            .any(|window| window == b"http-draft")
    );
    for forbidden in ["bearer", "api_key", "secret_reference"] {
        assert!(
            !body
                .windows(forbidden.len())
                .any(|window| window == forbidden.as_bytes())
        );
    }

    let no_room_creation = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/rooms")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("Room request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("Room response: {error}"));
    assert_eq!(no_room_creation.status(), 404);

    *mode.lock().unwrap_or_else(PoisonError::into_inner) = DependencyMode::Unavailable;
    let unavailable = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/task-templates/counter-team/revisions/r1/instantiate")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"draft_id":"unavailable-draft"}"#))
                .unwrap_or_else(|error| unreachable!("unavailable request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("unavailable response: {error}"));
    assert_eq!(unavailable.status(), 503);
    let unavailable_body = unavailable
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| unreachable!("unavailable body: {error}"))
        .to_bytes();
    let unavailable_json: serde_json::Value = serde_json::from_slice(&unavailable_body)
        .unwrap_or_else(|error| unreachable!("unavailable JSON: {error}"));
    assert_eq!(unavailable_json["error"]["retryable"], true);
    assert_eq!(
        unavailable_json["error"]["dependency_issues"][0]["code"],
        "dependency_check_unavailable"
    );
}
