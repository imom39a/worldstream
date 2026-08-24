#[path = "../src/secrets.rs"]
mod secrets;

mod room_drafts {
    pub use worldstream_studio_supervisor::room_drafts::AgentProfileRevisionReferenceV1;
}

#[path = "../src/agent_profiles.rs"]
mod agent_profiles;

use std::collections::BTreeMap;

use agent_profiles::{
    AgentExecutionBindingV1, AgentProfileErrorV1, AgentProfileMembershipBindingV1,
    AgentProfileRevisionV1, AgentProfileSeatAssignmentV1, AgentProfileSecretAvailabilityV1,
    AgentProfileSecretSettingV1, AgentProfileStoreV1, agent_profile_router,
};
use axum::{
    body::Body,
    http::{Method, Request},
};
use http_body_util::BodyExt as _;
use room_drafts::AgentProfileRevisionReferenceV1;
use secrets::{FileSecretVaultV1, SecretKindV1};
use tempfile::tempdir;
use tower::ServiceExt as _;

fn revision(reference: secrets::SecretReferenceV1, revision: &str) -> AgentProfileRevisionV1 {
    AgentProfileRevisionV1 {
        schema: "worldstream/studio-agent-profile/v1".to_owned(),
        profile_id: "careful-counter".to_owned(),
        revision: revision.to_owned(),
        display_name: "Careful Counter".to_owned(),
        non_secret_configuration: BTreeMap::from([
            ("policy".to_owned(), "deliberate".to_owned()),
            ("temperature".to_owned(), "0.2".to_owned()),
        ]),
        secret_settings: vec![AgentProfileSecretSettingV1 {
            key: "MODEL_PROVIDER_TOKEN".to_owned(),
            kind: SecretKindV1::ModelProvider,
            reference,
        }],
    }
}

fn assignment(revision: &str) -> AgentProfileSeatAssignmentV1 {
    AgentProfileSeatAssignmentV1 {
        schema: "worldstream/studio-agent-profile-assignment/v1".to_owned(),
        assignment_id: "01ARZ3NDEKTSV4RRFFQ69G5FB0".to_owned(),
        draft_id: "launch-alpha".to_owned(),
        seat_id: "counter-1".to_owned(),
        profile: AgentProfileRevisionReferenceV1 {
            profile_id: "careful-counter".to_owned(),
            revision: revision.to_owned(),
        },
        membership: AgentProfileMembershipBindingV1 {
            room_id: "01ARZ3NDEKTSV4RRFFQ69G5FB1".to_owned(),
            member_id: "01ARZ3NDEKTSV4RRFFQ69G5FB2".to_owned(),
            principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FB3".to_owned(),
            role: "counter".to_owned(),
        },
        execution: AgentExecutionBindingV1::Managed {
            runner_id: "01ARZ3NDEKTSV4RRFFQ69G5FB4".to_owned(),
        },
    }
}

#[tokio::test]
async fn immutable_revisions_restart_exactly_and_browser_projection_never_exposes_secret_refs() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))
        .unwrap_or_else(|error| unreachable!("secret vault: {error:?}"));
    let secret = vault
        .store(SecretKindV1::ModelProvider, b"provider-private-value")
        .unwrap_or_else(|error| unreachable!("store secret: {error:?}"));
    let secret_text = secret.as_str().to_owned();
    let root = directory.path().join("agent-profiles");
    let store = AgentProfileStoreV1::open(&root, vault.clone())
        .unwrap_or_else(|error| unreachable!("profile store: {error:?}"));

    let first = revision(secret.clone(), "1");
    let first_view = store
        .publish(&first)
        .unwrap_or_else(|error| unreachable!("publish profile: {error:?}"));
    assert_eq!(first_view.revision, "1");
    assert_eq!(
        first_view.secret_settings[0].availability,
        AgentProfileSecretAvailabilityV1::Configured
    );
    assert_eq!(store.publish(&first), Ok(first_view.clone()));
    let mut changed = first.clone();
    changed.display_name = "Changed in place".to_owned();
    assert_eq!(
        store.publish(&changed),
        Err(AgentProfileErrorV1::ImmutableRevisionConflict)
    );
    store
        .publish(&revision(secret, "2"))
        .unwrap_or_else(|error| unreachable!("publish new revision: {error:?}"));
    drop(store);

    let reopened = AgentProfileStoreV1::open(&root, vault)
        .unwrap_or_else(|error| unreachable!("reopen profile store: {error:?}"));
    let response = agent_profile_router(reopened)
        .oneshot(
            Request::builder()
                .uri("/api/v1/agent-profiles")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("catalog request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("catalog response: {error}"));
    assert_eq!(response.status(), 200);
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| unreachable!("catalog body: {error}"))
        .to_bytes();
    let json: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or_else(|error| unreachable!("catalog JSON: {error}"));
    assert_eq!(json["profiles"].as_array().map(Vec::len), Some(2));
    assert!(
        !body
            .windows(secret_text.len())
            .any(|window| window == secret_text.as_bytes())
    );
    assert!(
        !body
            .windows(22)
            .any(|window| window == b"provider-private-value")
    );
    assert!(!body.windows(16).any(|window| window == b"secret_reference"));
}

#[test]
fn publication_rejects_raw_secret_configuration_and_wrong_kind_reference() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))
        .unwrap_or_else(|error| unreachable!("secret vault: {error:?}"));
    let runner_reference = vault
        .store(SecretKindV1::RunnerAuthority, b"runner-only")
        .unwrap_or_else(|error| unreachable!("runner secret: {error:?}"));
    let store = AgentProfileStoreV1::open(&directory.path().join("profiles"), vault)
        .unwrap_or_else(|error| unreachable!("profile store: {error:?}"));
    let wrong_kind = revision(runner_reference, "1");
    assert_eq!(
        store.publish(&wrong_kind),
        Err(AgentProfileErrorV1::InvalidProfile)
    );

    let vault = FileSecretVaultV1::open(&directory.path().join("other-secrets"))
        .unwrap_or_else(|error| unreachable!("other vault: {error:?}"));
    let reference = vault
        .store(SecretKindV1::ModelProvider, b"provider")
        .unwrap_or_else(|error| unreachable!("provider secret: {error:?}"));
    let store = AgentProfileStoreV1::open(&directory.path().join("other-profiles"), vault)
        .unwrap_or_else(|error| unreachable!("other store: {error:?}"));
    let mut unsafe_profile = revision(reference.clone(), "1");
    unsafe_profile.non_secret_configuration =
        BTreeMap::from([("api_token".to_owned(), "raw-secret".to_owned())]);
    assert_eq!(
        store.publish(&unsafe_profile),
        Err(AgentProfileErrorV1::InvalidProfile)
    );

    let mut bearer_value = revision(reference, "1");
    bearer_value.non_secret_configuration = BTreeMap::from([(
        "provider_header".to_owned(),
        "Bearer eyJhbGciOiJIUzI1NiJ9.private-signature".to_owned(),
    )]);
    assert_eq!(
        store.publish(&bearer_value),
        Err(AgentProfileErrorV1::InvalidProfile)
    );

    let missing = secrets::SecretReferenceV1::parse("a".repeat(64))
        .unwrap_or_else(|error| unreachable!("valid absent reference: {error:?}"));
    assert_eq!(
        store.publish(&revision(missing, "2")),
        Err(AgentProfileErrorV1::InvalidProfile)
    );
}

#[tokio::test]
async fn build_api_publishes_exact_revision_and_requires_a_new_revision_for_changes() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))
        .unwrap_or_else(|error| unreachable!("secret vault: {error:?}"));
    let reference = vault
        .store(SecretKindV1::ModelProvider, b"provider-private-value")
        .unwrap_or_else(|error| unreachable!("provider secret: {error:?}"));
    let store = AgentProfileStoreV1::open(&directory.path().join("profiles"), vault)
        .unwrap_or_else(|error| unreachable!("profile store: {error:?}"));
    let first = revision(reference.clone(), "1");

    let response = agent_profile_router(store.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/agent-profiles")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&first).unwrap_or_else(
                    |error| unreachable!("publish request JSON: {error}"),
                )))
                .unwrap_or_else(|error| unreachable!("publish request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("publish response: {error}"));
    assert_eq!(response.status(), 200);
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| unreachable!("publish body: {error}"))
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body)
        .unwrap_or_else(|error| unreachable!("publish response JSON: {error}"));
    assert_eq!(json["revision"], "1");
    assert!(json.get("schema").is_none());
    assert!(json["secret_settings"][0].get("reference").is_none());

    let mut changed_in_place = first;
    changed_in_place.display_name = "Changed in place".to_owned();
    let conflict = agent_profile_router(store.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/agent-profiles")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&changed_in_place)
                        .unwrap_or_else(|error| unreachable!("conflict request JSON: {error}")),
                ))
                .unwrap_or_else(|error| unreachable!("conflict request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("conflict response: {error}"));
    assert_eq!(conflict.status(), 409);

    changed_in_place.revision = "2".to_owned();
    let next = agent_profile_router(store)
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/agent-profiles")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&changed_in_place)
                        .unwrap_or_else(|error| unreachable!("next request JSON: {error}")),
                ))
                .unwrap_or_else(|error| unreachable!("next request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("next response: {error}"));
    assert_eq!(next.status(), 200);
}

#[test]
fn exact_revision_paths_do_not_alias_delimiter_shaped_identities() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))
        .unwrap_or_else(|error| unreachable!("secret vault: {error:?}"));
    let reference = vault
        .store(SecretKindV1::ModelProvider, b"provider")
        .unwrap_or_else(|error| unreachable!("provider secret: {error:?}"));
    let root = directory.path().join("profiles");
    let store = AgentProfileStoreV1::open(&root, vault.clone())
        .unwrap_or_else(|error| unreachable!("profile store: {error:?}"));
    let mut left = revision(reference.clone(), "beta--gamma");
    left.profile_id = "alpha".to_owned();
    left.display_name = "Left".to_owned();
    let mut right = revision(reference, "gamma");
    right.profile_id = "alpha--beta".to_owned();
    right.display_name = "Right".to_owned();

    store
        .publish(&left)
        .unwrap_or_else(|error| unreachable!("publish left: {error:?}"));
    store
        .publish(&right)
        .unwrap_or_else(|error| unreachable!("publish right: {error:?}"));
    drop(store);

    let reopened = AgentProfileStoreV1::open(&root, vault)
        .unwrap_or_else(|error| unreachable!("reopen profile store: {error:?}"));
    assert_eq!(
        reopened
            .revision("alpha", "beta--gamma")
            .unwrap_or_else(|error| unreachable!("load left: {error:?}"))
            .display_name,
        "Left"
    );
    assert_eq!(
        reopened
            .revision("alpha--beta", "gamma")
            .unwrap_or_else(|error| unreachable!("load right: {error:?}"))
            .display_name,
        "Right"
    );
}

#[tokio::test]
async fn exact_seat_membership_assignment_is_idempotent_restartable_and_keeps_runner_separate() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))
        .unwrap_or_else(|error| unreachable!("secret vault: {error:?}"));
    let reference = vault
        .store(SecretKindV1::ModelProvider, b"provider")
        .unwrap_or_else(|error| unreachable!("provider secret: {error:?}"));
    let root = directory.path().join("profiles");
    let store = AgentProfileStoreV1::open(&root, vault.clone())
        .unwrap_or_else(|error| unreachable!("profile store: {error:?}"));
    store
        .publish(&revision(reference, "1"))
        .unwrap_or_else(|error| unreachable!("publish profile: {error:?}"));
    let exact = assignment("1");
    assert_eq!(store.bind_membership(&exact), Ok(exact.clone()));
    assert_eq!(store.bind_membership(&exact), Ok(exact.clone()));
    let mut conflict = exact.clone();
    conflict.membership.member_id = "01ARZ3NDEKTSV4RRFFQ69G5FB5".to_owned();
    assert_eq!(
        store.bind_membership(&conflict),
        Err(AgentProfileErrorV1::ImmutableAssignmentConflict)
    );
    drop(store);

    let reopened = AgentProfileStoreV1::open(&root, vault)
        .unwrap_or_else(|error| unreachable!("reopen profile store: {error:?}"));
    let response = agent_profile_router(reopened)
        .oneshot(
            Request::builder()
                .uri("/api/v1/agent-profile-assignments")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("assignment request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("assignment response: {error}"));
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| unreachable!("assignment body: {error}"))
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body)
        .unwrap_or_else(|error| unreachable!("assignment JSON: {error}"));
    assert_eq!(json["assignments"][0]["membership"]["role"], "counter");
    assert_eq!(json["assignments"][0]["execution"]["kind"], "managed");
    assert!(
        json["assignments"][0]["membership"]
            .get("runner_id")
            .is_none()
    );
    assert!(
        json["assignments"][0]["execution"]
            .get("member_id")
            .is_none()
    );
}

#[test]
fn assignment_reconcile_recreates_a_missing_record_and_keeps_seats_isolated() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))
        .unwrap_or_else(|error| unreachable!("secret vault: {error:?}"));
    let reference = vault
        .store(SecretKindV1::ModelProvider, b"provider")
        .unwrap_or_else(|error| unreachable!("provider secret: {error:?}"));
    let root = directory.path().join("profiles");
    let store = AgentProfileStoreV1::open(&root, vault)
        .unwrap_or_else(|error| unreachable!("profile store: {error:?}"));
    store
        .publish(&revision(reference, "1"))
        .unwrap_or_else(|error| unreachable!("publish profile: {error:?}"));

    let first = assignment("1");
    let mut second = first.clone();
    second.assignment_id = "01ARZ3NDEKTSV4RRFFQ69G5FB5".to_owned();
    second.seat_id = "counter-2".to_owned();
    second.membership.member_id = "01ARZ3NDEKTSV4RRFFQ69G5FB6".to_owned();
    store
        .bind_membership(&first)
        .unwrap_or_else(|error| unreachable!("bind first seat: {error:?}"));
    store
        .bind_membership(&second)
        .unwrap_or_else(|error| unreachable!("bind second seat: {error:?}"));
    assert_eq!(
        store.assignments().map(|value| value.assignments.len()),
        Ok(2)
    );

    std::fs::remove_file(
        root.join("assignments")
            .join(format!("{}.json", first.assignment_id)),
    )
    .unwrap_or_else(|error| unreachable!("remove interrupted assignment: {error}"));
    assert_eq!(store.reconcile_membership(&first), Ok(first.clone()));
    assert_eq!(store.reconcile_membership(&first), Ok(first));
    assert_eq!(
        store.assignments().map(|value| value.assignments.len()),
        Ok(2)
    );
}
