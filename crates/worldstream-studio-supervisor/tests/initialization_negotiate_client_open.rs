//! The reviewed CLI import resolves Negotiate 0.2 to its specialized client.

use std::{path::PathBuf, time::Duration};

use axum::{body::Body, http::Request};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;
use worldstream_activity_client::ExactPackReferenceV1;
use worldstream_protocol::{AccessMode, PackReference, SealedCapabilityBearerV1};
use worldstream_runtime::CliOverrides;
use worldstream_studio_supervisor::{
    client_bindings::{ClientBindingStoreV1, ClientSelectionRequestV1, ClientSelectionV1},
    control_access::ControlAccess,
    control_admission::protect_operator_routes,
    initialization_imports::{InitializationImportRequest, apply_imports, preview_imports},
    local_initialization::{InitializationRequest, initialize_local},
    participant_handoff::{
        CurrentMembershipSnapshotV1, HumanSeatAuthorityV1, ParticipantActionRequestV1,
        ParticipantConsoleGatewayErrorV1, ParticipantConsoleGatewayV1,
        ParticipantConsoleObservationV1, ParticipantHandoffAuthorityErrorV1,
        ParticipantHandoffAuthoritySourceV1, ParticipantHandoffBrokerV1,
        operator_client_handoff_router,
    },
};

const ROOM_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const MEMBER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const BEARER: &str = "wsb1:abababababababababababababababababababababababababababababababab";
const NEGOTIATE_0_2: &str =
    "blake3:651a04711a61bbdb263da5869a48d9829bc37b3be315042404587b00d52c127c";

fn negotiate_pack() -> PackReference {
    PackReference {
        id: "worldstream.negotiate".to_owned(),
        version: "0.2.0".to_owned(),
        digest: NEGOTIATE_0_2.to_owned(),
    }
}

#[derive(Clone, Copy)]
struct NegotiateHumanSeat;

impl ParticipantHandoffAuthoritySourceV1 for NegotiateHumanSeat {
    fn resolve_provisioned_human_seat(
        &self,
        operation: &str,
        seat: &str,
    ) -> Result<HumanSeatAuthorityV1, ParticipantHandoffAuthorityErrorV1> {
        if operation != "negotiate-live" || seat != "buyer-approver" {
            return Err(ParticipantHandoffAuthorityErrorV1::SeatNotFound);
        }
        HumanSeatAuthorityV1::new(
            ROOM_ID,
            MEMBER_ID,
            negotiate_pack(),
            AccessMode::Participant,
            Some("buyer_approver".to_owned()),
            SealedCapabilityBearerV1::parse(BEARER.to_owned())
                .map_err(|_| ParticipantHandoffAuthorityErrorV1::Unavailable)?,
        )
    }
}

#[derive(Clone, Copy)]
struct NegotiateGateway;

impl ParticipantConsoleGatewayV1 for NegotiateGateway {
    fn current_membership(
        &self,
        _: &HumanSeatAuthorityV1,
        _: Option<u64>,
    ) -> Result<CurrentMembershipSnapshotV1, ParticipantConsoleGatewayErrorV1> {
        Ok(CurrentMembershipSnapshotV1 {
            pack: negotiate_pack(),
            access_mode: AccessMode::Participant,
            role: Some("buyer_approver".to_owned()),
        })
    }

    fn observe(
        &self,
        _: &HumanSeatAuthorityV1,
        _: Option<u64>,
    ) -> Result<ParticipantConsoleObservationV1, ParticipantConsoleGatewayErrorV1> {
        Err(ParticipantConsoleGatewayErrorV1::Unavailable)
    }

    fn act(
        &self,
        _: &HumanSeatAuthorityV1,
        _: Option<u64>,
        _: &ParticipantActionRequestV1,
    ) -> Result<Value, ParticipantConsoleGatewayErrorV1> {
        Err(ParticipantConsoleGatewayErrorV1::Unavailable)
    }
}

#[tokio::test]
async fn reviewed_import_selects_and_opens_negotiate_0_2_client()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let installation = InitializationRequest {
        config: None,
        overrides: CliOverrides::default(),
        state_dir: directory.path().join(".worldstream/studio"),
        working_directory: directory.path().to_path_buf(),
        environment: Vec::new(),
        preview: false,
    };
    initialize_local(&installation)?;
    let declaration = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/activity-clients/cli-import.json");
    let mut request = InitializationImportRequest {
        installation,
        runner_templates: Vec::new(),
        provider_declarations: Vec::new(),
        agent_profiles: Vec::new(),
        client_declarations: vec![declaration],
        approval: None,
    };
    request.approval = Some(preview_imports(&request)?.digest);
    let applied = apply_imports(&request)?;
    let root = applied.state_dir.join("client-bindings");
    let clients = ClientBindingStoreV1::open_installed(
        &root,
        ClientBindingStoreV1::installed_policy(&root)?,
    )?;

    let selected = clients.select(
        &ClientSelectionRequestV1 {
            pack: ExactPackReferenceV1 {
                id: "worldstream.negotiate".to_owned(),
                version: "0.2.0".to_owned(),
                digest: NEGOTIATE_0_2.to_owned(),
            },
            client_contract: "worldstream/activity-client-protocol/v1".to_owned(),
            access_mode: AccessMode::Participant,
            role: Some("buyer_approver".to_owned()),
        },
        None,
    )?;
    assert!(matches!(selected, ClientSelectionV1::Selected { candidate }
        if candidate.candidate_id == "negotiate-0-2-participant-web"
            && candidate.client_id == "worldstream.negotiate.web"
            && candidate.launch_url == "http://127.0.0.1:5173/negotiate-v2/"));

    let broker = ParticipantHandoffBrokerV1::new(
        "http://127.0.0.1:5174",
        "http://127.0.0.1:5173",
        Duration::from_secs(30),
        8,
        NegotiateHumanSeat,
        NegotiateGateway,
        clients,
    )
    .map_err(|_| "bounded Negotiate browser broker fixture is invalid")?;
    let control = ControlAccess::initialize(&directory.path().join("controller"))?;
    let authorization = control.authorization_header()?;
    let router = protect_operator_routes(operator_client_handoff_router(broker), control);
    let response = router
        .oneshot(
            Request::post(
                "/api/v1/room-setup-operations/negotiate-live/seats/buyer-approver/client-handoff",
            )
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(Body::from("{}"))?,
        )
        .await?;
    assert_eq!(response.status(), 201);
    let body: Value = serde_json::from_slice(&response.into_body().collect().await?.to_bytes())?;
    assert_eq!(body["state"], "ready");
    assert!(
        body["client_url"]
            .as_str()
            .is_some_and(|url| url.starts_with("http://127.0.0.1:5173/negotiate-v2/#handoff=wsh1:"))
    );
    assert!(!body.to_string().contains(BEARER));
    Ok(())
}
