#[path = "../src/room_drafts.rs"]
mod room_drafts;

mod activity_packs {
    pub use worldstream_studio_supervisor::activity_packs::*;
}

use axum::{body::Body, http::Request};
use http_body_util::BodyExt as _;
use room_drafts::{
    ExactActivityPackDraftValidatorV1, RoomDraftErrorV1, RoomDraftResponseV1, RoomDraftStepV1,
    RoomDraftStoreV1, RoomDraftV1, RoomDraftValidatorV1, room_draft_router,
};
use serde_json::{Value, json};
use tempfile::tempdir;
use tower::ServiceExt as _;
use worldstream_protocol::{ActivityPackCatalogResponse, ActivityPackCatalogRevisionResponse};

const DIGEST: &str = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn draft() -> Value {
    json!({
        "schema": "worldstream/studio-room-draft/v1",
        "draft_id": "launch-alpha",
        "pack": { "id": "counter", "version": "1.0.0", "digest": DIGEST },
        "configuration": { "initial_value": 0, "maximum_value": 8 },
        "seats": [
            { "seat_id": "player-1", "role": "player", "required": true, "display_name": "Player 1", "principal_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV", "principal_kind": "human" },
            { "seat_id": "observer-1", "role": "observer", "required": false, "display_name": "Observer 1" }
        ],
        "readiness": [
            { "seat_id": "player-1", "role": "player", "required": true },
            { "seat_id": "observer-1", "role": "observer", "required": false }
        ],
        "last_valid_step": "readiness"
    })
}

#[derive(Clone, Copy)]
struct ValidDraft;

impl RoomDraftValidatorV1 for ValidDraft {
    fn validate(
        &self,
        _draft: &RoomDraftV1,
    ) -> Result<Vec<room_drafts::RoomDraftFieldErrorV1>, RoomDraftErrorV1> {
        Ok(Vec::new())
    }
}

#[derive(Clone)]
struct FixedPackSource(ActivityPackCatalogRevisionResponse);

impl activity_packs::DaemonActivityPackSource for FixedPackSource {
    fn catalog(
        &self,
    ) -> Result<ActivityPackCatalogResponse, activity_packs::ActivityPackProxyErrorV1> {
        Ok(ActivityPackCatalogResponse {
            version: "activity_pack_catalog.v1".to_owned(),
            revisions: vec![self.0.revision.summary.clone()],
        })
    }

    fn revision(
        &self,
        _digest: &str,
    ) -> Result<ActivityPackCatalogRevisionResponse, activity_packs::ActivityPackProxyErrorV1> {
        Ok(self.0.clone())
    }
}

fn exact_validator() -> ExactActivityPackDraftValidatorV1 {
    let detail = serde_json::from_value(json!({
        "version": "activity_pack_catalog.v1",
        "revision": {
            "summary": {
                "pack": { "id": "counter", "version": "1.0.0", "digest": DIGEST },
                "name": "Counter",
                "selectable_for_new_rooms": true,
                "runnable_for_retained_rooms": true
            },
            "roles": [
                { "role": "player", "minimum": 1, "maximum": 2 },
                { "role": "observer", "minimum": 0, "maximum": 1 }
            ],
            "configuration_schema": {
                "schema_id": "counter.config.v1",
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
    .unwrap_or_else(|error| unreachable!("detail fixture: {error}"));
    ExactActivityPackDraftValidatorV1::new(FixedPackSource(detail))
}

#[tokio::test]
async fn owner_only_draft_resumes_after_restart_at_last_valid_step_with_exact_pack_pin() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary store: {error}"));
    let root = directory.path().join("drafts");
    let store = RoomDraftStoreV1::open(&root, ValidDraft)
        .unwrap_or_else(|error| unreachable!("open draft store: {error:?}"));
    let put = room_draft_router(store)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/room-drafts/launch-alpha")
                .header("content-type", "application/json")
                .body(Body::from(draft().to_string()))
                .unwrap_or_else(|error| unreachable!("PUT request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("PUT response: {error}"));
    assert_eq!(put.status(), 200);

    let reopened = RoomDraftStoreV1::open(&root, ValidDraft)
        .unwrap_or_else(|error| unreachable!("reopen draft store: {error:?}"));
    let get = room_draft_router(reopened)
        .oneshot(
            Request::builder()
                .uri("/api/v1/room-drafts/launch-alpha")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("GET request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("GET response: {error}"));
    assert_eq!(get.status(), 200);
    let body = get
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| unreachable!("GET body: {error}"))
        .to_bytes();
    let resumed: RoomDraftResponseV1 = serde_json::from_slice(&body)
        .unwrap_or_else(|error| unreachable!("draft response JSON: {error}"));

    assert_eq!(
        resumed.draft.last_valid_step,
        Some(RoomDraftStepV1::Readiness)
    );
    assert_eq!(
        resumed.draft.pack.as_ref().map(|pack| pack.digest.as_str()),
        Some(DIGEST)
    );
    assert_eq!(resumed.review.pack, resumed.draft.pack);
    assert_eq!(resumed.review.readiness, resumed.draft.readiness);
}

#[tokio::test]
async fn unavailable_exact_revision_remains_inspectable_and_is_never_migrated() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary store: {error}"));
    let store = RoomDraftStoreV1::open(&directory.path().join("drafts"), ValidDraft)
        .unwrap_or_else(|error| unreachable!("open draft store: {error:?}"));
    let response = room_draft_router(store)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/room-drafts/launch-alpha")
                .header("content-type", "application/json")
                .body(Body::from(draft().to_string()))
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| unreachable!("body: {error}"))
        .to_bytes();
    let saved: RoomDraftResponseV1 = serde_json::from_slice(&body)
        .unwrap_or_else(|error| unreachable!("response JSON: {error}"));

    assert_eq!(
        saved.draft.pack.as_ref().map(|pack| pack.version.as_str()),
        Some("1.0.0")
    );
    assert_eq!(
        saved.draft.pack.as_ref().map(|pack| pack.digest.as_str()),
        Some(DIGEST)
    );
}

#[tokio::test]
async fn draft_validation_rejects_secret_material_and_misaligned_seat_readiness() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary store: {error}"));
    let store = RoomDraftStoreV1::open(&directory.path().join("drafts"), ValidDraft)
        .unwrap_or_else(|error| unreachable!("open draft store: {error:?}"));
    let mut unsafe_draft = draft();
    unsafe_draft["configuration"]["api_key"] = json!("must-not-persist");
    unsafe_draft["readiness"][0]["role"] = json!("substitute-role");
    let response = room_draft_router(store)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/room-drafts/launch-alpha")
                .header("content-type", "application/json")
                .body(Body::from(unsafe_draft.to_string()))
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), 400);
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| unreachable!("body: {error}"))
        .to_bytes();
    assert!(!body.windows(16).any(|window| window == b"must-not-persist"));
}

#[tokio::test]
async fn supervisor_enforces_exact_pack_schema_with_bounded_pointer_errors() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary store: {error}"));
    let store = RoomDraftStoreV1::open(&directory.path().join("drafts"), exact_validator())
        .unwrap_or_else(|error| unreachable!("open draft store: {error:?}"));
    let mut invalid = draft();
    invalid["configuration"]["maximum_value"] = json!(99);
    let response = room_draft_router(store)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/room-drafts/launch-alpha")
                .header("content-type", "application/json")
                .body(Body::from(invalid.to_string()))
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), 422);
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| unreachable!("body: {error}"))
        .to_bytes();
    let error: Value = serde_json::from_slice(&body)
        .unwrap_or_else(|error| unreachable!("validation JSON: {error}"));

    assert_eq!(
        error["error"]["field_errors"][0]["path"],
        "/configuration/maximum_value"
    );
    assert_eq!(error["error"]["field_errors"][0]["code"], "maximum");
    assert!(!body.windows(2).any(|window| window == b"99"));
}

#[tokio::test]
async fn supervisor_enforces_declared_roles_and_cardinality_before_persisting() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary store: {error}"));
    let store = RoomDraftStoreV1::open(&directory.path().join("drafts"), exact_validator())
        .unwrap_or_else(|error| unreachable!("open draft store: {error:?}"));
    let mut invalid = draft();
    invalid["seats"][0]["role"] = json!("administrator");
    invalid["readiness"][0]["role"] = json!("administrator");
    let response = room_draft_router(store)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/room-drafts/launch-alpha")
                .header("content-type", "application/json")
                .body(Body::from(invalid.to_string()))
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), 422);
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| unreachable!("body: {error}"))
        .to_bytes();
    let error: Value = serde_json::from_slice(&body)
        .unwrap_or_else(|error| unreachable!("validation JSON: {error}"));

    assert_eq!(error["error"]["field_errors"][0]["path"], "/seats/0/role");
    assert_eq!(error["error"]["field_errors"][0]["code"], "undeclared_role");
    assert!(
        error["error"]["field_errors"]
            .as_array()
            .is_some_and(|errors| { errors.iter().any(|field| field["code"] == "role_minimum") })
    );
}

#[tokio::test]
async fn partial_draft_persists_only_through_its_last_valid_step() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary store: {error}"));
    let root = directory.path().join("drafts");
    let store = RoomDraftStoreV1::open(&root, exact_validator())
        .unwrap_or_else(|error| unreachable!("open draft store: {error:?}"));
    let mut partial = draft();
    partial["configuration"] = json!({});
    partial["seats"] = json!([]);
    partial["readiness"] = json!([]);
    partial["last_valid_step"] = json!("activity");
    let response = room_draft_router(store)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/room-drafts/launch-alpha")
                .header("content-type", "application/json")
                .body(Body::from(partial.to_string()))
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(response.status(), 200);

    let reopened = RoomDraftStoreV1::open(&root, exact_validator())
        .unwrap_or_else(|error| unreachable!("reopen draft store: {error:?}"));
    let resumed = reopened
        .load("launch-alpha")
        .unwrap_or_else(|error| unreachable!("resume partial draft: {error:?}"));
    assert_eq!(resumed.last_valid_step, Some(RoomDraftStepV1::Activity));
    assert!(resumed.seats.is_empty());
}

#[tokio::test]
async fn draft_surface_has_no_room_or_authority_creation_route() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary store: {error}"));
    let store = RoomDraftStoreV1::open(&directory.path().join("drafts"), ValidDraft)
        .unwrap_or_else(|error| unreachable!("open draft store: {error:?}"));
    for path in ["/api/v1/rooms", "/api/v1/authorities", "/api/v1/principals"] {
        let response = room_draft_router(store.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), 404);
    }
}

#[tokio::test]
async fn agent_seat_retains_exact_profile_revision_but_human_seat_rejects_it() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary store: {error}"));
    let root = directory.path().join("drafts");
    let store = RoomDraftStoreV1::open(&root, ValidDraft)
        .unwrap_or_else(|error| unreachable!("open draft store: {error:?}"));
    let mut agent = draft();
    agent["seats"][0]["principal_kind"] = json!("agent");
    agent["seats"][0]["agent_assignment"] = json!("external");
    agent["seats"][0]["agent_profile"] = json!({
        "profile_id": "careful-counter",
        "revision": "2"
    });
    let saved = room_draft_router(store.clone())
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/room-drafts/launch-alpha")
                .header("content-type", "application/json")
                .body(Body::from(agent.to_string()))
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(saved.status(), 200);
    let reopened = RoomDraftStoreV1::open(&root, ValidDraft)
        .unwrap_or_else(|error| unreachable!("reopen draft store: {error:?}"));
    let retained = reopened
        .load("launch-alpha")
        .unwrap_or_else(|error| unreachable!("load draft: {error:?}"));
    assert_eq!(
        retained.seats[0]
            .agent_profile
            .as_ref()
            .map(|profile| (profile.profile_id.as_str(), profile.revision.as_str())),
        Some(("careful-counter", "2"))
    );

    let mut human = draft();
    human["seats"][0]["agent_profile"] = json!({
        "profile_id": "careful-counter",
        "revision": "2"
    });
    let rejected = room_draft_router(store)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/room-drafts/launch-alpha")
                .header("content-type", "application/json")
                .body(Body::from(human.to_string()))
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
    assert_eq!(rejected.status(), 400);
}
