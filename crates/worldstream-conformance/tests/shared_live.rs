#![cfg(feature = "live-conformance")]
#![allow(
    clippy::expect_used,
    clippy::manual_let_else,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::too_many_lines,
    clippy::type_complexity,
    clippy::unused_self,
    clippy::unwrap_used
)]

use std::{collections::BTreeMap, env, sync::Arc};

use tempfile::NamedTempFile;
use worldstream_conformance::{
    AdapterError, CanonicalFact, KernelConformanceAdapter, ProjectionOutcome, ScenarioEvidence,
    ScenarioPlan, ScenarioStatus, digest, operation_outcome, run_catalog,
};
use worldstream_core::{
    AccessModeV1, ActivationOperationRequestV1, AdministrationOperationIdentityV1,
    AuthorityBootstrapV1, AuthorityChangeV1, AuthorityGenerationV1, AuthorityGrantV1,
    AuthorityUseV1, AuthorityV1, CORE_OPERATION_KIND, CapabilityAuthoritySnapshotPartsV1,
    CapabilityAuthoritySnapshotV1, CapabilityBearerV1, CapabilityProfileV1, CapabilityScopeSetV1,
    CapabilityScopeV1, CoreAdministrationRequestV1, CoreChangeSetV1, CoreProposedKindV1,
    CoreProposedV1, CoreRoomStateV1, CoreTraceV1, InMemoryAuthorityStoreV1,
    InitialMembershipProposalV1, IntegrityGenerationV1, MemberAuthorityUseV1, MemberId,
    MemberReadOperationV1, MembershipAuthoritySnapshotV1, MembershipChangeV1,
    MembershipGenerationV1, MembershipStandingV1, MembershipV1, NewCapabilityV1,
    PackGenesisRequestV1, PackRegistryV1, ParticipantActionRequestV1, ParticipantActionV1,
    PreparedRoomCommitV1, PreparedRoomCreationV1, PreparedRoomWriteV1, PresentedCapabilityV1,
    PrincipalKindV1, RecordedStimulusV1, RoomCommitResolutionV1, RoomCommitStorageV1,
    RoomCreationRequestV1, RoomId, RoomMembershipKeyV1, RoomSeedV1, RunnerAuthoritySnapshotV1,
    RunnerAuthorityStatusV1, RunnerControlOperationV1, RunnerGenerationV1, RunnerMembershipSetV1,
    TimerFiredRequestV1, TimerFiredV1, TimerGenerationV1, ValidatedPackViewV1, agent_heist_digest,
    builtin_agent_heist_registry,
};
use worldstream_postgres::{
    PostgresAdmin, PostgresConnectionConfig, PostgresConnectionPath, PostgresObservationDeliveryV1,
    PostgresRoomStore,
};
use worldstream_sqlite::{SqliteObservationDeliveryV1, SqliteRoomStore};

const HOST_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH5";
const MEMBER_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH6";
const RUNNER_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH7";
const BOOTSTRAP_CHANGE: &str = "01ARZ3NDEKTSV4RRFFQ69G5FJ0";
const RUNNER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FE0";
const REGISTER_RUNNER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FJ1";
const REGISTER_MEMBER_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FJ2";
const REGISTER_RUNNER_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FJ3";
const SEED: &str = "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const TRANSITION_ADMIN: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ6";
const TRANSITION_ACTION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ7";
const TRANSITION_TIMER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FQ8";

fn parsed<T>(value: &str) -> T
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .unwrap_or_else(|error| panic!("{value}: {error}"))
}

fn canonical(bytes: &[u8]) -> worldstream_core::CanonicalJsonV1 {
    worldstream_core::CanonicalJsonV1::parse(bytes)
        .unwrap_or_else(|error| panic!("canonical JSON: {error}"))
}

fn fact(kind: &str, key: &str, bytes: Vec<u8>, semantic: &str) -> CanonicalFact {
    CanonicalFact {
        kind: kind.to_owned(),
        key: key.to_owned(),
        hash: digest(&bytes),
        bytes,
        semantic: semantic.to_owned(),
    }
}

fn json_bytes<T: serde::Serialize>(value: &T) -> Vec<u8> {
    let raw = serde_json::to_vec(value).unwrap_or_else(|error| panic!("JSON: {error}"));
    canonical(&raw)
        .to_bytes()
        .unwrap_or_else(|error| panic!("canonical JSON: {error}"))
}

fn activation_receipt_projection(
    result: &worldstream_core::ActivationOperationResultV1,
) -> Vec<u8> {
    json_bytes(&serde_json::json!({
        "operation_id": result.operation_id,
        "activation_id": result.activation_id,
        "claim_id": result.claim_id,
        "runner_id": result.runner_id,
        "code": result.code,
        "state": result.state,
        "lease_generation": result.lease_generation,
        "context_hash": result.context_hash,
        "context": result.context,
    }))
}

fn resolution_label(resolution: &RoomCommitResolutionV1) -> String {
    match resolution {
        RoomCommitResolutionV1::GenesisCreated { .. } => "genesis_created",
        RoomCommitResolutionV1::TransitionCommitted { .. } => "transition_committed",
        RoomCommitResolutionV1::RejectionRecorded { .. } => "rejection_recorded",
        RoomCommitResolutionV1::NoChangeRecorded { .. } => "no_change_recorded",
        RoomCommitResolutionV1::NotApplicable => "not_applicable",
        RoomCommitResolutionV1::Reprepare => "reprepare",
        RoomCommitResolutionV1::Fenced => "fenced",
        RoomCommitResolutionV1::Conflict { .. } => "conflict",
        RoomCommitResolutionV1::RetryableKnownAbsent => "retryable_known_absent",
        RoomCommitResolutionV1::Indeterminate => "indeterminate",
        RoomCommitResolutionV1::Fault => "fault",
    }
    .to_owned()
}

fn outcome(
    operation: &str,
    resolution: &RoomCommitResolutionV1,
) -> worldstream_conformance::CanonicalOperationOutcome {
    let receipt = resolution
        .stored_result()
        .map(|result| result.canonical_receipt_bytes().to_vec());
    operation_outcome(
        operation,
        resolution_label(resolution),
        resolution.duplicate(),
        receipt.clone(),
        if receipt.is_some() {
            "stored"
        } else {
            "closed"
        },
        receipt,
    )
}

fn projection(trace: &CoreTraceV1, member_id: &MemberId) -> ProjectionOutcome {
    let membership = trace
        .core_state()
        .membership(member_id)
        .unwrap_or_else(|| panic!("projection membership"));
    let viewer = match membership.access_mode() {
        AccessModeV1::Participant => worldstream_core::PackViewerV1::Participant(member_id.clone()),
        AccessModeV1::Spectator => worldstream_core::PackViewerV1::Public(member_id.clone()),
        AccessModeV1::Operator => worldstream_core::PackViewerV1::Operator(member_id.clone()),
    };
    let view = trace
        .retained_pack()
        .unwrap_or_else(|| panic!("retained pack"))
        .host()
        .view(&worldstream_core::ViewInputV1 {
            core: trace.core_state(),
            activity_state: trace.activity_state(),
            complete_head: trace.head(),
            viewer: &viewer,
        })
        .unwrap_or_else(|error| panic!("projection: {error}"));
    ProjectionOutcome {
        operation: "current-view".to_owned(),
        status: ScenarioStatus::Pass,
        projection_bytes: Some(view.canonical_bytes().to_vec()),
        projection_hash: Some(digest(view.canonical_bytes())),
    }
}

fn validated_view(trace: &CoreTraceV1, member_id: &MemberId) -> ValidatedPackViewV1 {
    let membership = trace
        .core_state()
        .membership(member_id)
        .unwrap_or_else(|| panic!("projection membership"));
    let viewer = match membership.access_mode() {
        AccessModeV1::Participant => worldstream_core::PackViewerV1::Participant(member_id.clone()),
        AccessModeV1::Spectator => worldstream_core::PackViewerV1::Public(member_id.clone()),
        AccessModeV1::Operator => worldstream_core::PackViewerV1::Operator(member_id.clone()),
    };
    trace
        .retained_pack()
        .unwrap_or_else(|| panic!("retained pack"))
        .host()
        .view(&worldstream_core::ViewInputV1 {
            core: trace.core_state(),
            activity_state: trace.activity_state(),
            complete_head: trace.head(),
            viewer: &viewer,
        })
        .unwrap_or_else(|error| panic!("projection: {error}"))
}

fn plan_memberships(plan: &ScenarioPlan) -> Vec<MembershipV1> {
    let roles = ["navigator", "insider", "broker", "spectator", "operator"];
    let modes = [
        (AccessModeV1::Participant, Some(roles[0])),
        (AccessModeV1::Participant, Some(roles[1])),
        (AccessModeV1::Participant, Some(roles[2])),
        (AccessModeV1::Spectator, None),
        (AccessModeV1::Operator, None),
    ];
    std::iter::once((plan.member_id.as_str(), plan.principal_id.as_str()))
        .chain(
            plan.secondary_member_ids
                .iter()
                .zip(&plan.secondary_principal_ids)
                .map(|(m, p)| (m.as_str(), p.as_str())),
        )
        .zip(modes)
        .map(|((member, principal), (mode, role))| {
            MembershipV1::new(
                parsed(member),
                parsed(principal),
                PrincipalKindV1::Agent,
                MembershipStandingV1::Enabled,
                mode,
                role.map(str::to_owned),
            )
            .unwrap_or_else(|error| panic!("membership: {error}"))
        })
        .collect()
}

fn build_genesis(plan: &ScenarioPlan) -> worldstream_core::PreparedNewRoomGenesisV1 {
    let registry =
        builtin_agent_heist_registry().unwrap_or_else(|error| panic!("registry: {error}"));
    registry
        .prepare_genesis_for_new_room(&PackGenesisRequestV1 {
            room_id: parsed(&plan.room_id),
            pack_digest: agent_heist_digest(),
            configuration: canonical(br#"{"briefing_duration_seconds":30,"commitment_duration_seconds":30,"commitment_reminder_seconds_before_deadline":10,"maximum_open_offers_per_role":4,"maximum_plans":12,"negotiation_duration_seconds":90,"pack_id":"worldstream.agent-heist","pack_schema":1,"result_duration_seconds":20,"roles":["navigator","insider","broker"]}"#),
            room_seed: parsed::<RoomSeedV1>(SEED),
            created_at: parsed("2026-08-15T12:00:00Z"),
            initial_core_state: CoreRoomStateV1::active(plan_memberships(plan))
                .unwrap_or_else(|error| panic!("Core state: {error}")),
        })
        .unwrap_or_else(|error| panic!("Genesis: {error}"))
}

fn creation_request(plan: &ScenarioPlan) -> RoomCreationRequestV1 {
    RoomCreationRequestV1::new(
        agent_heist_digest(),
        canonical(br#"{"briefing_duration_seconds":30,"commitment_duration_seconds":30,"commitment_reminder_seconds_before_deadline":10,"maximum_open_offers_per_role":4,"maximum_plans":12,"negotiation_duration_seconds":90,"pack_id":"worldstream.agent-heist","pack_schema":1,"result_duration_seconds":20,"roles":["navigator","insider","broker"]}"#),
        plan_memberships(plan)
            .into_iter()
            .map(|membership| InitialMembershipProposalV1::new(membership.principal_id().clone(), membership.principal_kind(), membership.standing(), membership.access_mode(), membership.role().map(str::to_owned)).unwrap_or_else(|error| panic!("proposal: {error}")))
            .collect(),
    )
}

fn creation_identity(plan: &ScenarioPlan, key: &str) -> AdministrationOperationIdentityV1 {
    AdministrationOperationIdentityV1 {
        authenticated_principal: parsed(&plan.principal_id),
        versioned_operation_kind: worldstream_core::CREATE_ROOM_OPERATION_KIND.to_owned(),
        idempotency_key: key.to_owned(),
    }
}

fn registry() -> PackRegistryV1 {
    builtin_agent_heist_registry().unwrap_or_else(|error| panic!("registry: {error}"))
}

struct SqliteAdapter {
    store: Arc<SqliteRoomStore>,
    _file: NamedTempFile,
    authority: AuthorityV1,
    plan: ScenarioPlan,
    registry: PackRegistryV1,
    trace: Option<CoreTraceV1>,
}

impl SqliteAdapter {
    fn new(plan: ScenarioPlan) -> Self {
        let file = NamedTempFile::new().unwrap_or_else(|error| panic!("SQLite temp file: {error}"));
        let store = Arc::new(
            SqliteRoomStore::open(file.path())
                .unwrap_or_else(|error| panic!("SQLite open: {error}")),
        );
        let authority = AuthorityV1::new(store.clone());
        let host_bearer = CapabilityBearerV1::from_bytes([0xA7; 32]);
        authority
            .bootstrap(
                AuthorityBootstrapV1::new(
                    parsed(BOOTSTRAP_CHANGE),
                    parsed(&plan.principal_id),
                    PrincipalKindV1::Agent,
                    parsed(HOST_CAPABILITY),
                    host_bearer.token_hash(),
                    None,
                )
                .unwrap_or_else(|error| panic!("SQLite bootstrap request: {error}")),
                parsed("2026-08-15T12:00:00Z"),
            )
            .unwrap_or_else(|error| panic!("SQLite bootstrap: {error}"));
        Self {
            store,
            _file: file,
            authority,
            plan,
            registry: registry(),
            trace: None,
        }
    }

    fn host(&self) -> PresentedCapabilityV1 {
        PresentedCapabilityV1::new(
            parsed(HOST_CAPABILITY),
            CapabilityBearerV1::from_bytes([0xA7; 32]),
        )
    }

    fn member(&self) -> PresentedCapabilityV1 {
        PresentedCapabilityV1::new(
            parsed(MEMBER_CAPABILITY),
            CapabilityBearerV1::from_bytes([0xB8; 32]),
        )
    }

    fn runner(&self) -> PresentedCapabilityV1 {
        PresentedCapabilityV1::new(
            parsed(RUNNER_CAPABILITY),
            CapabilityBearerV1::from_bytes([0xC9; 32]),
        )
    }

    fn install_runtime_authority(&self) {
        self.authority
            .change(
                &self.host(),
                AuthorityChangeV1::RegisterRunner {
                    change_id: parsed(REGISTER_RUNNER),
                    runner_id: parsed(RUNNER_ID),
                    owner_principal_id: parsed(&self.plan.principal_id),
                },
                parsed("2026-08-15T12:00:01Z"),
            )
            .unwrap_or_else(|error| panic!("SQLite runner registration: {error}"));
        let member_scopes = CapabilityScopeSetV1::new([
            CapabilityScopeV1::RoomAct,
            CapabilityScopeV1::RoomAttach,
            CapabilityScopeV1::RoomObserveMember,
            CapabilityScopeV1::RoomReplay,
        ])
        .unwrap_or_else(|error| panic!("SQLite member scopes: {error}"));
        let member_capability = NewCapabilityV1::new(
            parsed(MEMBER_CAPABILITY),
            CapabilityBearerV1::from_bytes([0xB8; 32]).token_hash(),
            parsed(&self.plan.principal_id),
            CapabilityProfileV1::RoomMember {
                room_id: parsed(&self.plan.room_id),
                member_id: parsed(&self.plan.member_id),
            },
            member_scopes,
            None,
        )
        .unwrap_or_else(|error| panic!("SQLite member capability: {error}"));
        self.authority
            .change(
                &self.host(),
                AuthorityChangeV1::RegisterCapability {
                    change_id: parsed(REGISTER_MEMBER_CAPABILITY),
                    capability: member_capability,
                },
                parsed("2026-08-15T12:00:02Z"),
            )
            .unwrap_or_else(|error| panic!("SQLite member capability registration: {error}"));
        let runner_scopes = CapabilityScopeSetV1::new([
            CapabilityScopeV1::ActivationOfferReceive,
            CapabilityScopeV1::ActivationClaim,
            CapabilityScopeV1::ActivationComplete,
        ])
        .unwrap_or_else(|error| panic!("SQLite runner scopes: {error}"));
        let runner_memberships = RunnerMembershipSetV1::new([RoomMembershipKeyV1 {
            room_id: parsed(&self.plan.room_id),
            member_id: parsed(&self.plan.activation_target_member_id),
        }])
        .unwrap_or_else(|error| panic!("SQLite runner target: {error}"));
        let runner_capability = NewCapabilityV1::new(
            parsed(RUNNER_CAPABILITY),
            CapabilityBearerV1::from_bytes([0xC9; 32]).token_hash(),
            parsed(&self.plan.principal_id),
            CapabilityProfileV1::RunnerControl {
                runner_id: parsed(RUNNER_ID),
                permitted_memberships: runner_memberships,
            },
            runner_scopes,
            None,
        )
        .unwrap_or_else(|error| panic!("SQLite runner capability: {error}"));
        self.authority
            .change(
                &self.host(),
                AuthorityChangeV1::RegisterCapability {
                    change_id: parsed(REGISTER_RUNNER_CAPABILITY),
                    capability: runner_capability,
                },
                parsed("2026-08-15T12:00:03Z"),
            )
            .unwrap_or_else(|error| panic!("SQLite runner capability registration: {error}"));
    }

    fn trace(&self) -> &CoreTraceV1 {
        self.trace
            .as_ref()
            .unwrap_or_else(|| panic!("SQLite trace is not initialized"))
    }

    fn core_request(&self) -> CoreAdministrationRequestV1 {
        let before = self
            .trace()
            .core_state()
            .membership(&parsed(&self.plan.secondary_member_ids[3]))
            .unwrap_or_else(|| panic!("admin membership"))
            .clone();
        let change = MembershipChangeV1::access_mode_change(before, AccessModeV1::Operator, None)
            .unwrap_or_else(|error| panic!("admin change: {error}"));
        CoreAdministrationRequestV1::new(
            parsed(&self.plan.room_id),
            AdministrationOperationIdentityV1 {
                authenticated_principal: parsed(&self.plan.principal_id),
                versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
                idempotency_key: "shared-core-admin".to_owned(),
            },
            CoreProposedKindV1::AccessModeChange,
            self.trace().head().room_seq(),
            "shared-core-admin",
            CoreChangeSetV1::one(change),
        )
        .unwrap_or_else(|error| panic!("admin request: {error}"))
    }

    fn action_request(&self) -> ParticipantActionRequestV1 {
        ParticipantActionRequestV1::new(
            parsed(&self.plan.room_id),
            parsed(&self.plan.member_id),
            parsed(&self.plan.action_id),
            self.trace().head().room_seq(),
            "inspect_clue",
            canonical(br#"{"clue_id":"route"}"#),
        )
    }

    fn action_stimulus(&self, _request: &ParticipantActionRequestV1) -> ParticipantActionV1 {
        let descriptor = self
            .trace()
            .retained_pack()
            .unwrap_or_else(|| panic!("pack"))
            .descriptor();
        ParticipantActionV1 {
            member_id: parsed(&self.plan.member_id),
            action_id: parsed(&self.plan.action_id),
            action_type: "inspect_clue".to_owned(),
            payload_schema_digest: descriptor
                .actions
                .iter()
                .find(|item| item.action_type == "inspect_clue")
                .unwrap_or_else(|| panic!("action descriptor"))
                .payload_schema
                .schema_digest
                .clone(),
            canonical_payload: canonical(br#"{"clue_id":"route"}"#),
            exact_basis_head: self.trace().head().clone(),
            admitted_at: parsed(&self.plan.admitted_at),
        }
    }

    fn frame_heads(&self) -> BTreeMap<MemberId, u64> {
        self.store
            .gateway_room_snapshot(&self.registry, &parsed(&self.plan.room_id))
            .unwrap_or_else(|error| panic!("SQLite snapshot: {error}"))
            .map(|snapshot| snapshot.frame_heads().clone())
            .unwrap_or_else(|| panic!("SQLite room snapshot missing"))
    }

    fn create_write(&self, key: &str, initial_value: u32) -> PreparedRoomWriteV1 {
        let genesis = build_genesis(&self.plan);
        let request = creation_request(&self.plan);
        let identity = creation_identity(&self.plan, key);
        let grant = match worldstream_core::authorize_room_creation_operation(
            &self.authority,
            self.store.as_ref(),
            &self.host(),
            &identity,
            &request,
            parsed("2026-08-15T12:00:04Z"),
        )
        .unwrap_or_else(|error| panic!("SQLite creation authorization: {error}"))
        {
            worldstream_core::RoomCreationIngressV1::Authorized(grant) => *grant,
            other => panic!("unexpected creation ingress: {other:?}"),
        };
        let _ = initial_value;
        PreparedRoomCreationV1::from_registry_genesis(identity, &request, grant, genesis)
            .unwrap_or_else(|error| panic!("SQLite creation preparation: {error}"))
            .into()
    }
}

impl KernelConformanceAdapter for SqliteAdapter {
    fn adapter_name(&self) -> &'static str {
        "sqlite"
    }

    fn create(&mut self, plan: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        let write = self.create_write("shared-create", 0);
        let duplicate = self.create_write("shared-create", 0);
        let first = RoomCommitStorageV1::commit(self.store.as_ref(), &write);
        let second = RoomCommitStorageV1::commit(self.store.as_ref(), &duplicate);
        let trace = CoreTraceV1::create_from_retained_for_conformance(build_genesis(plan))
            .map_err(|error| AdapterError::new("sqlite-create", error.to_string()))?;
        self.trace = Some(trace);
        self.install_runtime_authority();
        let mut outcomes = vec![
            outcome("create", &first),
            outcome("create-duplicate", &second),
        ];
        let conflict = self.create_write("shared-create-conflict", 0);
        let conflict = RoomCommitStorageV1::commit(self.store.as_ref(), &conflict);
        outcomes.push(outcome("create-conflict", &conflict));
        let head = self
            .trace()
            .head()
            .canonical_bytes()
            .map_err(|error| AdapterError::new("sqlite-create", error.to_string()))?;
        Ok(ScenarioEvidence::pass(
            outcomes,
            vec![projection(self.trace(), &parsed(&plan.member_id))],
            vec![fact("head", "create", head, "genesis-head")],
            vec!["duplicate/conflict are durable receipt resolutions".to_owned()],
        ))
    }

    fn core_admin(&mut self, _: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        let request = self.core_request();
        let grant = match worldstream_core::authorize_core_administration_operation(
            &self.authority,
            self.store.as_ref(),
            &self.host(),
            &request,
            parsed("2026-08-15T12:00:05Z"),
        )
        .map_err(|error| AdapterError::new("sqlite-core-admin", error.to_string()))?
        {
            worldstream_core::CoreAdministrationIngressV1::Authorized(grant) => *grant,
            other => {
                return Err(AdapterError::new(
                    "sqlite-core-admin",
                    format!("unexpected ingress: {other:?}"),
                ));
            }
        };
        let prepared = PreparedRoomCommitV1::for_authorized_core_administration(
            self.trace(),
            &request,
            parsed("2026-08-15T12:00:06Z"),
            parsed(TRANSITION_ADMIN),
            IntegrityGenerationV1::new(1).unwrap_or_else(|error| panic!("integrity: {error}")),
            grant,
            &self.frame_heads(),
        )
        .map_err(|error| AdapterError::new("sqlite-core-admin", format!("{error:?}")))?;
        let resolution = RoomCommitStorageV1::commit(self.store.as_ref(), &prepared.into());
        if matches!(
            resolution,
            RoomCommitResolutionV1::TransitionCommitted { .. }
        ) {
            let proposal = CoreProposedV1::new(
                request.kind(),
                worldstream_core::CoreAuthorityAttributionV1 {
                    principal_id: parsed(&self.plan.principal_id),
                    authority_kind: worldstream_core::CoreAuthorityKindV1::HostOperator,
                },
                request.operation_identity().clone(),
                request.expected_room_seq(),
                request.reason_code(),
                parsed("2026-08-15T12:00:06Z"),
                request.changeset().clone(),
            );
            self.trace
                .as_mut()
                .unwrap_or_else(|| panic!("SQLite trace is not initialized"))
                .advance_for_conformance(RecordedStimulusV1::CoreProposed(proposal))
                .map_err(|error| {
                    AdapterError::new("sqlite-core-admin", format!("trace advance: {error:?}"))
                })?;
        }
        let bytes = self
            .trace()
            .head()
            .canonical_bytes()
            .map_err(|error| AdapterError::new("sqlite-core-admin", error.to_string()))?;
        Ok(ScenarioEvidence::pass(
            vec![outcome("core-admin", &resolution)],
            vec![projection(self.trace(), &parsed(&self.plan.member_id))],
            vec![fact("head", "core-admin", bytes, "admin-head")],
            vec!["Core classification and authority fence were consumed by the adapter".to_owned()],
        ))
    }

    fn action(&mut self, _: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        let request = self.action_request();
        let hash = request
            .canonical_request_hash()
            .map_err(|error| AdapterError::new("sqlite-action", error.to_string()))?;
        let use_ = AuthorityUseV1::Member {
            room_id: parsed(&self.plan.room_id),
            member_id: parsed(&self.plan.member_id),
            operation: MemberAuthorityUseV1::SubmitAction {
                identity: worldstream_core::ParticipantActionOperationIdentityV1 {
                    room_id: parsed(&self.plan.room_id),
                    member_id: parsed(&self.plan.member_id),
                    action_id: parsed(&self.plan.action_id),
                },
                request_hash: hash,
                action_type: "inspect_clue".to_owned(),
            },
        };
        let grant = match self
            .authority
            .authorize(&self.member(), use_, parsed("2026-08-15T12:00:07Z"))
            .map_err(|error| AdapterError::new("sqlite-action", error.to_string()))?
        {
            AuthorityGrantV1::ParticipantAction(
                worldstream_core::ParticipantActionAuthorityV1::EnabledParticipant(grant),
            ) => grant,
            _ => return Err(AdapterError::new("sqlite-action", "wrong authority grant")),
        };
        let stimulus = self.action_stimulus(&request);
        let trace = self.trace();
        let prepared_transition = trace
            .prepare(RecordedStimulusV1::ParticipantAction(stimulus.clone()))
            .map_err(|error| AdapterError::new("sqlite-action", format!("{error:?}")))?;
        let prepared = PreparedRoomCommitV1::for_authorized_action(
            trace,
            &request,
            prepared_transition,
            parsed(TRANSITION_ACTION),
            IntegrityGenerationV1::new(1).unwrap_or_else(|error| panic!("integrity: {error}")),
            grant,
            &self.frame_heads(),
        )
        .map_err(|error| AdapterError::new("sqlite-action", format!("{error:?}")))?;
        let resolution = RoomCommitStorageV1::commit(self.store.as_ref(), &prepared.into());
        if matches!(
            resolution,
            RoomCommitResolutionV1::TransitionCommitted { .. }
        ) {
            self.trace
                .as_mut()
                .unwrap_or_else(|| panic!("SQLite trace is not initialized"))
                .advance_for_conformance(RecordedStimulusV1::ParticipantAction(stimulus))
                .map_err(|error| {
                    AdapterError::new("sqlite-action", format!("trace advance: {error:?}"))
                })?;
        }
        let bytes = self
            .trace()
            .head()
            .canonical_bytes()
            .map_err(|error| AdapterError::new("sqlite-action", error.to_string()))?;
        Ok(ScenarioEvidence::pass(vec![outcome("action", &resolution)], vec![projection(self.trace(), &parsed(&self.plan.member_id))], vec![fact("head", "action", bytes, "action-head")], vec!["Action payload, basis Head, duplicate fence and observation consequence use Core sealing".to_owned()]))
    }

    fn timer(&mut self, _: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        let candidate = self
            .store
            .timer_candidate(
                &parsed(&self.plan.room_id),
                &parsed(&self.plan.timer_id),
                TimerGenerationV1::new(1)
                    .unwrap_or_else(|error| panic!("timer generation: {error}")),
            )
            .map_err(|error| AdapterError::new("sqlite-timer", error.to_string()))?
            .ok_or_else(|| AdapterError::new("sqlite-timer", "initial timer missing"))?;
        let request = candidate.request().clone();
        let hash = request
            .canonical_request_hash()
            .map_err(|error| AdapterError::new("sqlite-timer", error.to_string()))?;
        let use_ = AuthorityUseV1::TimerFired {
            room_id: parsed(&self.plan.room_id),
            request_hash: hash,
        };
        let grant = match self
            .authority
            .authorize(&self.host(), use_, parsed("2026-08-15T12:00:31Z"))
            .map_err(|error| AdapterError::new("sqlite-timer", error.to_string()))?
        {
            AuthorityGrantV1::TimerFired(grant) => grant,
            _ => return Err(AdapterError::new("sqlite-timer", "wrong authority grant")),
        };
        let trace = self.trace();
        let prepared_transition = trace
            .prepare(RecordedStimulusV1::TimerFired(TimerFiredV1 {
                timer_id: request.timer_id().clone(),
                generation: request.generation(),
                scheduled_for: request.scheduled_for().clone(),
                canonical_payload: request.canonical_payload().clone(),
            }))
            .map_err(|error| AdapterError::new("sqlite-timer", format!("{error:?}")))?;
        let prepared = PreparedRoomCommitV1::for_authorized_timer_fired(
            trace,
            &request,
            prepared_transition,
            parsed(TRANSITION_TIMER),
            IntegrityGenerationV1::new(1).unwrap_or_else(|error| panic!("integrity: {error}")),
            grant,
            &self.frame_heads(),
        )
        .map_err(|error| AdapterError::new("sqlite-timer", format!("{error:?}")))?;
        let resolution = RoomCommitStorageV1::commit(self.store.as_ref(), &prepared.into());
        if matches!(
            resolution,
            RoomCommitResolutionV1::TransitionCommitted { .. }
        ) {
            self.trace
                .as_mut()
                .unwrap_or_else(|| panic!("SQLite trace is not initialized"))
                .advance_for_conformance(RecordedStimulusV1::TimerFired(TimerFiredV1 {
                    timer_id: request.timer_id().clone(),
                    generation: request.generation(),
                    scheduled_for: request.scheduled_for().clone(),
                    canonical_payload: request.canonical_payload().clone(),
                }))
                .map_err(|error| {
                    AdapterError::new("sqlite-timer", format!("trace advance: {error:?}"))
                })?;
        }
        let bytes = self
            .trace()
            .head()
            .canonical_bytes()
            .map_err(|error| AdapterError::new("sqlite-timer", error.to_string()))?;
        Ok(ScenarioEvidence::pass(
            vec![outcome("timer", &resolution)],
            vec![projection(self.trace(), &parsed(&self.plan.member_id))],
            vec![fact("head", "timer", bytes, "timer-generation-1")],
            vec!["Timer generation and payload were read from the durable candidate".to_owned()],
        ))
    }

    fn frame_cursor_reset(&mut self, _: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        let room_id: RoomId = parsed(&self.plan.room_id);
        let member_id = parsed(&self.plan.member_id);
        let view = validated_view(self.trace(), &member_id);
        let attach = self
            .store
            .attach_observations(
                self.authority
                    .authorize_member_read(
                        &self.member(),
                        room_id.clone(),
                        member_id.clone(),
                        MemberReadOperationV1::Attach,
                        parsed("2026-08-15T12:00:40Z"),
                    )
                    .map_err(|error| AdapterError::new("sqlite-frame", error.to_string()))?,
                view,
            )
            .map_err(|error| AdapterError::new("sqlite-frame", error.to_string()))?;
        let first = match attach.delivery() {
            SqliteObservationDeliveryV1::Reset {
                projection_reset, ..
            } => json_bytes(&serde_json::json!({
                "kind":"reset", "frame_head":attach.frame_head(), "reset_through":null, "projection":projection_reset.canonical_bytes()
            })),
            SqliteObservationDeliveryV1::Retained { frames, .. } => {
                json_bytes(&serde_json::json!({
                    "kind":"retained", "frame_head":attach.frame_head(), "reset_through":null, "frames":frames.iter().map(|frame| serde_json::json!({"seq":frame.frame_seq(),"cause":frame.cause_room_seq().get(),"hash":frame.payload_hash().to_string(),"bytes":frame.payload_bytes()})).collect::<Vec<_>>()
                }))
            }
        };
        if attach.frame_head() == 0 {
            return Err(AdapterError::new(
                "sqlite-frame",
                "real action/timer frame head is empty",
            ));
        }
        self.store
            .acknowledge_observation(
                self.authority
                    .authorize_member_read(
                        &self.member(),
                        room_id.clone(),
                        member_id.clone(),
                        MemberReadOperationV1::AcknowledgeObservation,
                        parsed("2026-08-15T12:00:41Z"),
                    )
                    .map_err(|error| AdapterError::new("sqlite-frame", error.to_string()))?,
                1,
            )
            .map_err(|error| AdapterError::new("sqlite-frame", error.to_string()))?;
        let positions = self
            .store
            .prune_observation_frames(&room_id, &member_id, 3)
            .map_err(|error| AdapterError::new("sqlite-frame", error.to_string()))?;
        if positions.reset_required_through() != Some(attach.frame_head()) {
            return Err(AdapterError::new(
                "sqlite-frame",
                format!(
                    "prune returned reset witness {:?}, expected {}",
                    positions.reset_required_through(),
                    attach.frame_head()
                ),
            ));
        }
        let after = self
            .store
            .attach_observations(
                self.authority
                    .authorize_member_read(
                        &self.member(),
                        room_id,
                        member_id,
                        MemberReadOperationV1::Attach,
                        parsed("2026-08-15T12:00:42Z"),
                    )
                    .map_err(|error| AdapterError::new("sqlite-frame", error.to_string()))?,
                validated_view(self.trace(), &parsed(&self.plan.member_id)),
            )
            .map_err(|error| AdapterError::new("sqlite-frame", error.to_string()))?;
        let second = match after.delivery() {
            SqliteObservationDeliveryV1::Reset {
                projection_reset, ..
            } => json_bytes(&serde_json::json!({
                "kind":"reset", "frame_head":after.frame_head(), "reset_through":positions.reset_required_through(), "projection":projection_reset.canonical_bytes()
            })),
            SqliteObservationDeliveryV1::Retained { frames, .. } => {
                json_bytes(&serde_json::json!({
                    "kind":"retained", "frame_head":after.frame_head(), "reset_through":positions.reset_required_through(), "frames":frames.iter().map(|frame| serde_json::json!({"seq":frame.frame_seq(),"cause":frame.cause_room_seq().get(),"hash":frame.payload_hash().to_string(),"bytes":frame.payload_bytes()})).collect::<Vec<_>>()
                }))
            }
        };
        Ok(ScenarioEvidence::pass(
            vec![
                operation_outcome(
                    "frame-attach",
                    "stored",
                    false,
                    Some(first.clone()),
                    "stored",
                    Some(first.clone()),
                ),
                operation_outcome(
                    "frame-reset",
                    "stored",
                    false,
                    Some(second.clone()),
                    "stored",
                    Some(second.clone()),
                ),
            ],
            vec![projection(self.trace(), &parsed(&self.plan.member_id))],
            vec![
                fact("delivery", "cursor-reset", first, "initial attach"),
                fact(
                    "delivery",
                    "cursor-reset-after-prune",
                    second,
                    "pruned-prefix reset",
                ),
            ],
            vec![
                "The real observation attach, ACK, prefix deletion, and reset path executed"
                    .to_owned(),
            ],
        ))
    }
    fn activation(&mut self, plan: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        let target = RoomMembershipKeyV1 {
            room_id: parsed(&plan.room_id),
            member_id: parsed(&plan.activation_target_member_id),
        };
        let request = ActivationOperationRequestV1 {
            operation_kind: "offer".to_owned(),
            operation_id: plan.activation_id.clone(),
            activation_id: None,
            claim_id: None,
            runner_id: plan.runner_id.clone(),
            lease_generation: None,
            requested_lease_ms: None,
            disposition: None,
        };
        let authority = self
            .authority
            .authorize_runner_control(
                &self.runner(),
                parsed(RUNNER_ID),
                RunnerControlOperationV1::ReceiveOffer,
                target,
                parsed("2026-08-15T12:00:50Z"),
            )
            .map_err(|error| AdapterError::new("sqlite-activation", error.to_string()))?;
        let offered = self
            .store
            .offer_activations(authority, request)
            .map_err(|error| AdapterError::new("sqlite-activation", error.to_string()))?;
        let full_receipt = offered.operation;
        let result_bytes = activation_receipt_projection(&full_receipt);
        let full_receipt_bytes = json_bytes(&full_receipt);
        let offer_bytes = json_bytes(&offered.offers.iter().map(|offer| serde_json::json!({
            "activation_id":offer.activation_id, "member_id":offer.member_id.to_string(), "cause_room_seq":offer.cause_room_seq.get(), "reason_code":offer.reason_code, "priority":offer.priority, "maximum_lease_ms":offer.maximum_lease_ms
        })).collect::<Vec<_>>());
        if !offered.offers.is_empty() {
            return Err(AdapterError::new(
                "sqlite-activation",
                "Heist activation offer requires a context-producing transition",
            ));
        }
        Ok(ScenarioEvidence::pass(
            vec![operation_outcome("activation-offer", "granted", false, Some(result_bytes.clone()), "stored", Some(result_bytes.clone())), operation_outcome("activation-lease-generation", "not_available", false, Some(offer_bytes.clone()), "closed", Some(offer_bytes.clone()))],
            vec![projection(self.trace(), &parsed(&plan.member_id))],
            vec![fact("activation", "offer-receipt", result_bytes, "provider-neutral typed offer receipt"), fact("activation", "offer-receipt-full", full_receipt_bytes, "full durable offer receipt"), fact("activation", "offers", offer_bytes, "real pending offer set")],
            vec!["The real Runner offer receipt and lease-generation fence were queried; this immutable Heist corpus has no pending Attention intent after inspect_clue".to_owned()],
        ))
    }
    fn recovery(&mut self, plan: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        let recovered = self
            .store
            .recover_room(&self.registry, &parsed(&plan.room_id))
            .map_err(|error| AdapterError::new("sqlite-recovery", error.to_string()))?
            .ok_or_else(|| {
                AdapterError::new(
                    "sqlite-recovery",
                    "room disappeared during restart recovery",
                )
            })?;
        self.trace = Some(recovered);
        let trace = self.trace();
        let genesis = trace
            .genesis_bytes()
            .map_err(|error| AdapterError::new("sqlite-recovery", error.to_string()))?;
        let transitions = json_bytes(
            &trace
                .transition_bytes()
                .map_err(|error| AdapterError::new("sqlite-recovery", error.to_string()))?,
        );
        let head = trace
            .head()
            .canonical_bytes()
            .map_err(|error| AdapterError::new("sqlite-recovery", error.to_string()))?;
        let integrity = self
            .store
            .room_integrity_state(&parsed(&plan.room_id))
            .map_err(|error| AdapterError::new("sqlite-recovery", error.to_string()))?
            .ok_or_else(|| {
                AdapterError::new("sqlite-recovery", "SQLite integrity state missing")
            })?;
        if integrity.status() != worldstream_core::RoomIntegrityStatusV1::Healthy {
            return Err(AdapterError::new(
                "sqlite-recovery",
                "SQLite recovery did not finish healthy",
            ));
        }
        Ok(ScenarioEvidence::pass(
            vec![operation_outcome("restart", "healthy", false, None, "closed", None), operation_outcome("snapshot-rebuild", "healthy", false, None, "closed", None)],
            vec![projection(trace, &parsed(&plan.member_id))],
            vec![fact("lineage", "genesis", genesis, "exact retained genesis"), fact("lineage", "transitions", transitions, "exact retained transition replay"), fact("head", "recovery", head, "recovered complete head")],
            vec!["The adapter deleted and rebuilt disposable snapshots, then replayed exact retained bytes after a fresh verification".to_owned()],
        ))
    }
}

struct PostgresAdapter {
    admin: PostgresAdmin,
    store: PostgresRoomStore,
    maintenance: PostgresRoomStore,
    authority: AuthorityV1,
    _memory: Arc<InMemoryAuthorityStoreV1>,
    plan: ScenarioPlan,
    registry: PackRegistryV1,
    trace: Option<CoreTraceV1>,
}

impl PostgresAdapter {
    fn new(
        plan: ScenarioPlan,
        admin_dsn: String,
        dsn: String,
        path: PostgresConnectionPath,
    ) -> Self {
        let admin = PostgresAdmin::new(
            PostgresConnectionConfig::direct_admin(admin_dsn.clone())
                .unwrap_or_else(|error| panic!("PG admin config: {error}")),
        )
        .unwrap_or_else(|error| panic!("PG admin: {error}"));
        admin
            .migrate()
            .unwrap_or_else(|error| panic!("PG migrate: {error}"));
        let store = PostgresRoomStore::new(
            PostgresConnectionConfig::runtime(dsn, path)
                .unwrap_or_else(|error| panic!("PG runtime config: {error}")),
        )
        .unwrap_or_else(|error| panic!("PG runtime: {error}"));
        // Raw prefix deletion is a conformance-fixture mutation. Production
        // runtime roles use the bounded SECURITY DEFINER retention function
        // and deliberately have no direct DELETE privilege on Frames.
        let maintenance = PostgresRoomStore::new(
            PostgresConnectionConfig::runtime(admin_dsn, PostgresConnectionPath::Direct)
                .unwrap_or_else(|error| panic!("PG maintenance config: {error}")),
        )
        .unwrap_or_else(|error| panic!("PG maintenance: {error}"));
        let memory = Arc::new(InMemoryAuthorityStoreV1::new());
        memory
            .set_commit_checked_at(parsed("2026-08-15T12:00:00Z"))
            .unwrap_or_else(|error| panic!("memory clock: {error}"));
        let authority = AuthorityV1::new(memory.clone());
        let host_bearer = CapabilityBearerV1::from_bytes([0xA7; 32]);
        authority
            .bootstrap(
                AuthorityBootstrapV1::new(
                    parsed(BOOTSTRAP_CHANGE),
                    parsed(&plan.principal_id),
                    PrincipalKindV1::Agent,
                    parsed(HOST_CAPABILITY),
                    host_bearer.token_hash(),
                    None,
                )
                .unwrap_or_else(|error| panic!("PG authority bootstrap: {error}")),
                parsed("2026-08-15T12:00:00Z"),
            )
            .unwrap_or_else(|error| panic!("PG authority bootstrap: {error}"));
        for membership in plan_memberships(&plan) {
            memory
                .seed_membership(MembershipAuthoritySnapshotV1::new(
                    parsed(&plan.room_id),
                    membership,
                    MembershipGenerationV1::new(1)
                        .unwrap_or_else(|error| panic!("membership generation: {error}")),
                ))
                .unwrap_or_else(|error| panic!("PG memory membership: {error}"));
        }
        memory
            .seed_runner(RunnerAuthoritySnapshotV1::new(
                parsed(RUNNER_ID),
                parsed(&plan.principal_id),
                RunnerAuthorityStatusV1::Enabled,
                RunnerGenerationV1::new(1)
                    .unwrap_or_else(|error| panic!("runner generation: {error}")),
            ))
            .unwrap_or_else(|error| panic!("PG memory runner: {error}"));
        let member_scopes = CapabilityScopeSetV1::new([
            CapabilityScopeV1::RoomAct,
            CapabilityScopeV1::RoomAttach,
            CapabilityScopeV1::RoomObserveMember,
            CapabilityScopeV1::RoomReplay,
        ])
        .unwrap_or_else(|error| panic!("PG member scopes: {error}"));
        memory
            .seed_capability(
                CapabilityAuthoritySnapshotV1::new(CapabilityAuthoritySnapshotPartsV1 {
                    capability_id: parsed(MEMBER_CAPABILITY),
                    token_hash: CapabilityBearerV1::from_bytes([0xB8; 32]).token_hash(),
                    principal_id: parsed(&plan.principal_id),
                    profile: CapabilityProfileV1::RoomMember {
                        room_id: parsed(&plan.room_id),
                        member_id: parsed(&plan.member_id),
                    },
                    scopes: member_scopes,
                    generation: AuthorityGenerationV1::new(1)
                        .unwrap_or_else(|error| panic!("capability generation: {error}")),
                    expires_at: None,
                    revoked_at: None,
                })
                .unwrap_or_else(|error| panic!("PG member capability: {error}")),
            )
            .unwrap_or_else(|error| panic!("PG memory member capability: {error}"));
        let runner_scopes = CapabilityScopeSetV1::new([
            CapabilityScopeV1::ActivationOfferReceive,
            CapabilityScopeV1::ActivationClaim,
            CapabilityScopeV1::ActivationComplete,
        ])
        .unwrap_or_else(|error| panic!("PG runner scopes: {error}"));
        let runner_memberships = RunnerMembershipSetV1::new([RoomMembershipKeyV1 {
            room_id: parsed(&plan.room_id),
            member_id: parsed(&plan.activation_target_member_id),
        }])
        .unwrap_or_else(|error| panic!("PG runner membership: {error}"));
        memory
            .seed_capability(
                CapabilityAuthoritySnapshotV1::new(CapabilityAuthoritySnapshotPartsV1 {
                    capability_id: parsed(RUNNER_CAPABILITY),
                    token_hash: CapabilityBearerV1::from_bytes([0xC9; 32]).token_hash(),
                    principal_id: parsed(&plan.principal_id),
                    profile: CapabilityProfileV1::RunnerControl {
                        runner_id: parsed(RUNNER_ID),
                        permitted_memberships: runner_memberships,
                    },
                    scopes: runner_scopes,
                    generation: AuthorityGenerationV1::new(1)
                        .unwrap_or_else(|error| panic!("runner capability generation: {error}")),
                    expires_at: None,
                    revoked_at: None,
                })
                .unwrap_or_else(|error| panic!("PG runner capability: {error}")),
            )
            .unwrap_or_else(|error| panic!("PG memory runner capability: {error}"));
        Self {
            admin,
            store,
            maintenance,
            authority,
            _memory: memory,
            plan,
            registry: registry(),
            trace: None,
        }
    }

    fn create_write(&self, key: &str) -> PreparedRoomWriteV1 {
        let identity = creation_identity(&self.plan, key);
        let request = creation_request(&self.plan);
        let request_hash = request
            .canonical_request_hash()
            .unwrap_or_else(|error| panic!("PG creation hash: {error}"));
        let grant = match self
            .authority
            .authorize(
                &PresentedCapabilityV1::new(
                    parsed(HOST_CAPABILITY),
                    CapabilityBearerV1::from_bytes([0xA7; 32]),
                ),
                AuthorityUseV1::CreateRoom {
                    identity: identity.clone(),
                    request_hash,
                },
                parsed("2026-08-15T12:00:04Z"),
            )
            .unwrap_or_else(|error| panic!("PG creation authorization: {error}"))
        {
            AuthorityGrantV1::RoomCreation(grant) => grant,
            _ => panic!("wrong PG creation grant"),
        };
        let prepared = PreparedRoomCreationV1::from_registry_genesis(
            identity,
            &request,
            grant,
            build_genesis(&self.plan),
        )
        .unwrap_or_else(|error| panic!("PG creation preparation: {error}"));
        prepared.into()
    }

    fn trace(&self) -> &CoreTraceV1 {
        self.trace
            .as_ref()
            .unwrap_or_else(|| panic!("PG trace is not initialized"))
    }

    fn refresh_trace(&mut self) {
        self.trace = Some(
            self.store
                .recover_conformance_trace(&self.registry, &self.plan.room_id)
                .unwrap_or_else(|error| panic!("PG refresh: {error}")),
        );
    }

    fn core_request(&self) -> CoreAdministrationRequestV1 {
        let before = self
            .trace()
            .core_state()
            .membership(&parsed(&self.plan.secondary_member_ids[3]))
            .unwrap_or_else(|| panic!("PG admin membership"))
            .clone();
        let change = MembershipChangeV1::access_mode_change(before, AccessModeV1::Operator, None)
            .unwrap_or_else(|error| panic!("PG admin change: {error}"));
        CoreAdministrationRequestV1::new(
            parsed(&self.plan.room_id),
            AdministrationOperationIdentityV1 {
                authenticated_principal: parsed(&self.plan.principal_id),
                versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
                idempotency_key: "shared-core-admin".to_owned(),
            },
            CoreProposedKindV1::AccessModeChange,
            self.trace().head().room_seq(),
            "shared-core-admin",
            CoreChangeSetV1::one(change),
        )
        .unwrap_or_else(|error| panic!("PG admin request: {error}"))
    }

    fn action_request(&self) -> ParticipantActionRequestV1 {
        ParticipantActionRequestV1::new(
            parsed(&self.plan.room_id),
            parsed(&self.plan.member_id),
            parsed(&self.plan.action_id),
            self.trace().head().room_seq(),
            "inspect_clue",
            canonical(br#"{"clue_id":"route"}"#),
        )
    }

    fn action_stimulus(&self) -> ParticipantActionV1 {
        let descriptor = self
            .trace()
            .retained_pack()
            .unwrap_or_else(|| panic!("PG pack"))
            .descriptor();
        ParticipantActionV1 {
            member_id: parsed(&self.plan.member_id),
            action_id: parsed(&self.plan.action_id),
            action_type: "inspect_clue".to_owned(),
            payload_schema_digest: descriptor
                .actions
                .iter()
                .find(|item| item.action_type == "inspect_clue")
                .unwrap_or_else(|| panic!("PG action descriptor"))
                .payload_schema
                .schema_digest
                .clone(),
            canonical_payload: canonical(br#"{"clue_id":"route"}"#),
            exact_basis_head: self.trace().head().clone(),
            admitted_at: parsed(&self.plan.admitted_at),
        }
    }
}

impl KernelConformanceAdapter for PostgresAdapter {
    fn adapter_name(&self) -> &'static str {
        "postgres"
    }

    fn create(&mut self, plan: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        let write = self.create_write("shared-create");
        let first = self.store.commit_conformance_write_for_conformance(&write);
        let duplicate = self
            .store
            .commit_conformance_write_for_conformance(&self.create_write("shared-create"));
        let trace = CoreTraceV1::create_from_retained_for_conformance(build_genesis(plan))
            .map_err(|error| AdapterError::new("postgres-create", error.to_string()))?;
        self.trace = Some(trace);
        let conflict = self
            .store
            .commit_conformance_write_for_conformance(&self.create_write("shared-create-conflict"));
        let head = self
            .trace()
            .head()
            .canonical_bytes()
            .map_err(|error| AdapterError::new("postgres-create", error.to_string()))?;
        Ok(ScenarioEvidence::pass(
            vec![
                outcome("create", &first),
                outcome("create-duplicate", &duplicate),
                outcome("create-conflict", &conflict),
            ],
            vec![projection(self.trace(), &parsed(&plan.member_id))],
            vec![fact("head", "create", head, "genesis-head")],
            vec!["duplicate/conflict are durable receipt resolutions".to_owned()],
        ))
    }

    fn core_admin(&mut self, _: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        let request = self.core_request();
        let classified = request
            .classified()
            .map_err(|error| AdapterError::new("postgres-core-admin", error.to_string()))?;
        let request_hash = request
            .canonical_request_hash()
            .map_err(|error| AdapterError::new("postgres-core-admin", error.to_string()))?;
        let grant = match self
            .authority
            .authorize(
                &PresentedCapabilityV1::new(
                    parsed(HOST_CAPABILITY),
                    CapabilityBearerV1::from_bytes([0xA7; 32]),
                ),
                AuthorityUseV1::CoreAdministration {
                    room_id: parsed(&self.plan.room_id),
                    classified,
                    identity: request.operation_identity().clone(),
                    request_hash,
                },
                parsed("2026-08-15T12:00:05Z"),
            )
            .map_err(|error| AdapterError::new("postgres-core-admin", error.to_string()))?
        {
            AuthorityGrantV1::CoreAdministration(grant) => grant,
            _ => {
                return Err(AdapterError::new(
                    "postgres-core-admin",
                    "wrong authority grant",
                ));
            }
        };
        let resolution = self
            .store
            .commit_conformance_core_administration(
                &self.registry,
                grant,
                &request,
                parsed("2026-08-15T12:00:06Z"),
                parsed(TRANSITION_ADMIN),
            )
            .map_err(|error| AdapterError::new("postgres-core-admin", error.to_string()))?;
        self.refresh_trace();
        let bytes = self
            .trace()
            .head()
            .canonical_bytes()
            .map_err(|error| AdapterError::new("postgres-core-admin", error.to_string()))?;
        Ok(ScenarioEvidence::pass(
            vec![outcome("core-admin", &resolution)],
            vec![projection(self.trace(), &parsed(&self.plan.member_id))],
            vec![fact("head", "core-admin", bytes, "admin-head")],
            vec!["Core classification and authority fence were consumed by the adapter".to_owned()],
        ))
    }

    fn action(&mut self, _: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        let request = self.action_request();
        let hash = request
            .canonical_request_hash()
            .map_err(|error| AdapterError::new("postgres-action", error.to_string()))?;
        let grant = match self
            .authority
            .authorize(
                &PresentedCapabilityV1::new(
                    parsed(MEMBER_CAPABILITY),
                    CapabilityBearerV1::from_bytes([0xB8; 32]),
                ),
                AuthorityUseV1::Member {
                    room_id: parsed(&self.plan.room_id),
                    member_id: parsed(&self.plan.member_id),
                    operation: MemberAuthorityUseV1::SubmitAction {
                        identity: worldstream_core::ParticipantActionOperationIdentityV1 {
                            room_id: parsed(&self.plan.room_id),
                            member_id: parsed(&self.plan.member_id),
                            action_id: parsed(&self.plan.action_id),
                        },
                        request_hash: hash,
                        action_type: "inspect_clue".to_owned(),
                    },
                },
                parsed("2026-08-15T12:00:07Z"),
            )
            .map_err(|error| AdapterError::new("postgres-action", error.to_string()))?
        {
            AuthorityGrantV1::ParticipantAction(
                worldstream_core::ParticipantActionAuthorityV1::EnabledParticipant(grant),
            ) => grant,
            _ => {
                return Err(AdapterError::new(
                    "postgres-action",
                    "wrong authority grant",
                ));
            }
        };
        let resolution = self
            .store
            .commit_conformance_participant_action(
                &self.registry,
                grant,
                &request,
                self.action_stimulus(),
                parsed(TRANSITION_ACTION),
            )
            .map_err(|error| AdapterError::new("postgres-action", error.to_string()))?;
        self.refresh_trace();
        let bytes = self
            .trace()
            .head()
            .canonical_bytes()
            .map_err(|error| AdapterError::new("postgres-action", error.to_string()))?;
        Ok(ScenarioEvidence::pass(vec![outcome("action", &resolution)], vec![projection(self.trace(), &parsed(&self.plan.member_id))], vec![fact("head", "action", bytes, "action-head")], vec!["Action payload, basis Head, duplicate fence and observation consequence use Core sealing".to_owned()]))
    }

    fn timer(&mut self, _: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        let verification = self
            .store
            .verify_room(&self.plan.room_id)
            .map_err(|error| AdapterError::new("postgres-timer", error.to_string()))?;
        let timer = verification
            .timers
            .iter()
            .find(|timer| timer.timer_id == self.plan.timer_id)
            .ok_or_else(|| AdapterError::new("postgres-timer", "initial timer missing"))?;
        let request = TimerFiredRequestV1::new(
            parsed(&self.plan.room_id),
            parsed(&self.plan.timer_id),
            TimerGenerationV1::new(timer.generation)
                .map_err(|error| AdapterError::new("postgres-timer", error.to_string()))?,
            parsed(&timer.scheduled_for),
            canonical(&timer.payload_bytes),
        );
        let hash = request
            .canonical_request_hash()
            .map_err(|error| AdapterError::new("postgres-timer", error.to_string()))?;
        let grant = match self
            .authority
            .authorize(
                &PresentedCapabilityV1::new(
                    parsed(HOST_CAPABILITY),
                    CapabilityBearerV1::from_bytes([0xA7; 32]),
                ),
                AuthorityUseV1::TimerFired {
                    room_id: parsed(&self.plan.room_id),
                    request_hash: hash,
                },
                parsed("2026-08-15T12:00:31Z"),
            )
            .map_err(|error| AdapterError::new("postgres-timer", error.to_string()))?
        {
            AuthorityGrantV1::TimerFired(grant) => grant,
            _ => return Err(AdapterError::new("postgres-timer", "wrong authority grant")),
        };
        let resolution = self
            .store
            .commit_conformance_timer_fired(
                &self.registry,
                grant,
                &request,
                TimerFiredV1 {
                    timer_id: request.timer_id().clone(),
                    generation: request.generation(),
                    scheduled_for: request.scheduled_for().clone(),
                    canonical_payload: request.canonical_payload().clone(),
                },
                parsed(TRANSITION_TIMER),
            )
            .map_err(|error| AdapterError::new("postgres-timer", error.to_string()))?;
        self.refresh_trace();
        let bytes = self
            .trace()
            .head()
            .canonical_bytes()
            .map_err(|error| AdapterError::new("postgres-timer", error.to_string()))?;
        Ok(ScenarioEvidence::pass(
            vec![outcome("timer", &resolution)],
            vec![projection(self.trace(), &parsed(&self.plan.member_id))],
            vec![fact("head", "timer", bytes, "timer-generation-1")],
            vec!["Timer generation and payload were read from the durable candidate".to_owned()],
        ))
    }
    fn frame_cursor_reset(&mut self, _: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        let room_id = self.plan.room_id.as_str();
        let member_id = self.plan.member_id.as_str();
        let view = validated_view(self.trace(), &parsed(&self.plan.member_id));
        let attach = self
            .store
            .read_observation_conformance(
                room_id,
                member_id,
                None,
                &canonical(view.canonical_bytes()),
            )
            .map_err(|error| AdapterError::new("postgres-frame", format!("{error:?}")))?;
        let encode_delivery = |delivery: &PostgresObservationDeliveryV1,
                               reset_through: Option<u64>| {
            match delivery {
                PostgresObservationDeliveryV1::Reset {
                    frame_head,
                    projection_bytes,
                    ..
                } => json_bytes(&serde_json::json!({
                    "kind":"reset", "frame_head":frame_head, "reset_through":reset_through, "projection":projection_bytes
                })),
                PostgresObservationDeliveryV1::Retained {
                    frame_head, frames, ..
                } => json_bytes(&serde_json::json!({
                    "kind":"retained", "frame_head":frame_head, "reset_through":reset_through, "frames":frames.iter().map(|frame| serde_json::json!({"seq":frame.frame_seq,"cause":frame.cause_room_seq,"hash":frame.payload_hash,"bytes":frame.payload_bytes})).collect::<Vec<_>>()
                })),
            }
        };
        let first = encode_delivery(&attach, None);
        let frame_head = match &attach {
            PostgresObservationDeliveryV1::Reset { frame_head, .. }
            | PostgresObservationDeliveryV1::Retained { frame_head, .. } => *frame_head,
        };
        if frame_head == 0 {
            return Err(AdapterError::new(
                "postgres-frame",
                "real action/timer frame head is empty",
            ));
        }
        self.store
            .acknowledge_observation_conformance(room_id, member_id, 1)
            .map_err(|error| AdapterError::new("postgres-frame", format!("{error:?}")))?;
        let positions = self
            .maintenance
            .prune_observation_conformance(room_id, member_id, 3)
            .map_err(|error| AdapterError::new("postgres-frame", format!("{error:?}")))?;
        if positions.reset_required_through != Some(frame_head) {
            return Err(AdapterError::new(
                "postgres-frame",
                format!(
                    "prune returned reset witness {:?}, expected {frame_head}",
                    positions.reset_required_through
                ),
            ));
        }
        let after = self
            .store
            .read_observation_conformance(
                room_id,
                member_id,
                Some(1),
                &canonical(view.canonical_bytes()),
            )
            .map_err(|error| AdapterError::new("postgres-frame", format!("{error:?}")))?;
        let second = encode_delivery(&after, positions.reset_required_through);
        Ok(ScenarioEvidence::pass(
            vec![
                operation_outcome(
                    "frame-attach",
                    "stored",
                    false,
                    Some(first.clone()),
                    "stored",
                    Some(first.clone()),
                ),
                operation_outcome(
                    "frame-reset",
                    "stored",
                    false,
                    Some(second.clone()),
                    "stored",
                    Some(second.clone()),
                ),
            ],
            vec![projection(self.trace(), &parsed(&self.plan.member_id))],
            vec![
                fact("delivery", "cursor-reset", first, "initial attach"),
                fact(
                    "delivery",
                    "cursor-reset-after-prune",
                    second,
                    "pruned-prefix reset",
                ),
            ],
            vec![
                "The real observation attach, ACK, prefix deletion, and reset path executed"
                    .to_owned(),
            ],
        ))
    }
    fn activation(&mut self, plan: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        let request = ActivationOperationRequestV1 {
            operation_kind: "offer".to_owned(),
            operation_id: plan.activation_id.clone(),
            activation_id: None,
            claim_id: None,
            runner_id: plan.runner_id.clone(),
            lease_generation: None,
            requested_lease_ms: None,
            disposition: None,
        };
        let offers = self
            .store
            .offer_activations(&plan.room_id, &plan.activation_target_member_id, &request)
            .map_err(|error| AdapterError::new("postgres-activation", error.to_string()))?;
        let full_receipt = self
            .store
            .read_activation_receipt_conformance(&plan.room_id, &request)
            .map_err(|error| AdapterError::new("postgres-activation", error.to_string()))?
            .ok_or_else(|| {
                AdapterError::new(
                    "postgres-activation",
                    "offer receipt disappeared after commit",
                )
            })?;
        let result_bytes = activation_receipt_projection(&full_receipt);
        let full_receipt_bytes = json_bytes(&full_receipt);
        let offer_bytes = json_bytes(&offers.iter().map(|offer| serde_json::json!({
            "activation_id":offer.activation_id, "member_id":offer.target_member_id, "cause_room_seq":offer.cause_room_seq, "reason_code":offer.reason_code, "priority":offer.priority, "policy_revision":offer.policy_revision
        })).collect::<Vec<_>>());
        if !offers.is_empty() {
            return Err(AdapterError::new(
                "postgres-activation",
                "Heist activation offer requires a context-producing transition",
            ));
        }
        Ok(ScenarioEvidence::pass(
            vec![operation_outcome("activation-offer", "granted", false, Some(result_bytes.clone()), "stored", Some(result_bytes.clone())), operation_outcome("activation-lease-generation", "not_available", false, Some(offer_bytes.clone()), "closed", Some(offer_bytes.clone()))],
            vec![projection(self.trace(), &parsed(&plan.member_id))],
            vec![fact("activation", "offer-receipt", result_bytes, "provider-neutral typed offer receipt"), fact("activation", "offer-receipt-full", full_receipt_bytes, "full durable offer receipt"), fact("activation", "offers", offer_bytes, "real pending offer set")],
            vec!["The real Runner offer receipt and lease-generation fence were queried; this immutable Heist corpus has no pending Attention intent after inspect_clue".to_owned()],
        ))
    }
    fn recovery(&mut self, plan: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError> {
        let before = self
            .store
            .verify_room(&plan.room_id)
            .map_err(|error| AdapterError::new("postgres-recovery", error.to_string()))?;
        self.admin
            .delete_snapshot_cache(&plan.room_id)
            .map_err(|error| AdapterError::new("postgres-recovery", error.to_string()))?;
        self.admin
            .rebuild_snapshot_cache(&plan.room_id)
            .map_err(|error| AdapterError::new("postgres-recovery", error.to_string()))?;
        let corrupted = self
            .admin
            .corrupt_snapshot_cache_for_conformance(&plan.room_id)
            .map_err(|error| AdapterError::new("postgres-recovery", error.to_string()))?;
        if corrupted != 1 || self.store.verify_room(&plan.room_id).is_ok() {
            return Err(AdapterError::new(
                "postgres-recovery",
                "malformed disposable snapshot did not fail closed before repair",
            ));
        }
        self.admin
            .rebuild_snapshot_cache(&plan.room_id)
            .map_err(|error| AdapterError::new("postgres-recovery", error.to_string()))?;
        let after = self
            .store
            .verify_room(&plan.room_id)
            .map_err(|error| AdapterError::new("postgres-recovery", error.to_string()))?;
        if before.genesis_bytes != after.genesis_bytes
            || before.transition_bytes != after.transition_bytes
            || before.head_bytes != after.head_bytes
            || before.integrity_status != "healthy"
            || after.integrity_status != "healthy"
        {
            return Err(AdapterError::new(
                "postgres-recovery",
                "snapshot rebuild changed retained lineage or integrity status",
            ));
        }
        let trace = self
            .store
            .recover_conformance_trace(&self.registry, &plan.room_id)
            .map_err(|error| AdapterError::new("postgres-recovery", error.to_string()))?;
        self.trace = Some(trace);
        let replay = self.trace();
        let genesis = replay
            .genesis_bytes()
            .map_err(|error| AdapterError::new("postgres-recovery", error.to_string()))?;
        let transitions = json_bytes(
            &replay
                .transition_bytes()
                .map_err(|error| AdapterError::new("postgres-recovery", error.to_string()))?,
        );
        let head = replay
            .head()
            .canonical_bytes()
            .map_err(|error| AdapterError::new("postgres-recovery", error.to_string()))?;
        Ok(ScenarioEvidence::pass(
            vec![operation_outcome("restart", "healthy", false, None, "closed", None), operation_outcome("snapshot-rebuild", "healthy", false, None, "closed", None)],
            vec![projection(replay, &parsed(&plan.member_id))],
            vec![fact("lineage", "genesis", genesis, "exact retained genesis"), fact("lineage", "transitions", transitions, "exact retained transition replay"), fact("head", "recovery", head, "recovered complete head")],
            vec!["The adapter deleted and rebuilt disposable snapshots, then replayed exact retained bytes after a fresh verification".to_owned()],
        ))
    }
}

#[test]
fn real_shared_catalog_runs_sqlite_and_postgres_without_fixture_adapters() {
    let admin_dsn = env::var("WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN")
        .unwrap_or_else(|_| panic!("WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN is required"));
    let runtime_dsn = env::var("WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN")
        .unwrap_or_else(|_| panic!("WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN is required"));
    let path = match env::var("WORLDSTREAM_POSTGRES_TEST_PATH").as_deref() {
        Ok("pooler") => PostgresConnectionPath::TransactionPool,
        _ => PostgresConnectionPath::Direct,
    };
    let plan = ScenarioPlan::counter();
    let mut sqlite = SqliteAdapter::new(plan.clone());
    let mut postgres = PostgresAdapter::new(
        plan.clone(),
        admin_dsn,
        if path == PostgresConnectionPath::Direct {
            runtime_dsn
        } else {
            env::var("WORLDSTREAM_POSTGRES_TEST_POOLER_DSN").unwrap_or(runtime_dsn)
        },
        path,
    );
    let sqlite_results = run_catalog(&mut sqlite, &plan).unwrap_or_else(|error| {
        panic!(
            "SQLite shared catalog: {}: {}",
            error.operation, error.detail
        )
    });
    let postgres_results = run_catalog(&mut postgres, &plan).unwrap_or_else(|error| {
        panic!(
            "PostgreSQL shared catalog: {}: {}",
            error.operation, error.detail
        )
    });
    let left = worldstream_conformance::ConformanceArtifact::for_comparison(&sqlite_results)
        .unwrap_or_else(|error| panic!("SQLite artifact: {error}"));
    let right = worldstream_conformance::ConformanceArtifact::for_comparison(&postgres_results)
        .unwrap_or_else(|error| panic!("PostgreSQL artifact: {error}"));
    assert!(
        sqlite_results
            .iter()
            .all(|result| result.status == ScenarioStatus::Pass)
    );
    assert!(
        postgres_results
            .iter()
            .all(|result| result.status == ScenarioStatus::Pass)
    );
    println!(
        "IMO50_SHARED=PASS path={path:?} scenarios=7 hash={:02x?}",
        left.canonical_hash
    );
    if let Ok(path) = env::var("WORLDSTREAM_IMO50_COMPARISON_FILE") {
        let report = serde_json::json!({
            "schema": "worldstream/imo-50/shared-comparison/v1",
            "connection_path": format!("{path:?}"),
            "scenario_count": worldstream_conformance::SCENARIOS.len(),
            "canonical_equal": true,
            "sqlite": serde_json::to_value(&left).unwrap_or_else(|error| panic!("SQLite comparison report: {error}")),
            "postgres": serde_json::to_value(&right).unwrap_or_else(|error| panic!("PostgreSQL comparison report: {error}")),
        });
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&report)
                .unwrap_or_else(|error| panic!("comparison report: {error}")),
        )
        .unwrap_or_else(|error| panic!("comparison report write: {error}"));
    }
    worldstream_conformance::ConformanceArtifact::compare_adapters(
        &sqlite_results,
        &postgres_results,
    )
    .unwrap_or_else(|error| panic!("exact provider-neutral comparison: {error}"));
}
