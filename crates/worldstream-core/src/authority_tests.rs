use std::{fmt::Debug, str::FromStr, sync::Arc};

use super::*;

const ROOM_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const ROOM_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const MEMBER_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const MEMBER_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
const PRINCIPAL_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const PRINCIPAL_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";
const HOST: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB2";
const CAP_MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
const CAP_HOST: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC1";
const CAP_RUNNER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC2";
const RUNNER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
const ACTION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FE0";
const CHANGE_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FF0";
const CHANGE_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FF1";
const CHANGE_C: &str = "01ARZ3NDEKTSV4RRFFQ69G5FF2";
const CHANGE_D: &str = "01ARZ3NDEKTSV4RRFFQ69G5FF3";
const CHANGE_E: &str = "01ARZ3NDEKTSV4RRFFQ69G5FF4";
const CHANGE_F: &str = "01ARZ3NDEKTSV4RRFFQ69G5FF5";
const CHANGE_G: &str = "01ARZ3NDEKTSV4RRFFQ69G5FF6";
const REQUEST_HASH: &str =
    "blake3:1111111111111111111111111111111111111111111111111111111111111111";

fn parsed<T>(value: &str) -> T
where
    T: FromStr,
    T::Err: Debug,
{
    match value.parse() {
        Ok(value) => value,
        Err(error) => unreachable!("fixture parse failed: {error:?}"),
    }
}

fn fixture<T, E: Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => unreachable!("fixture construction failed: {error:?}"),
    }
}

fn assert_authority_error(
    result: Result<AuthorityGrantV1, AuthorityErrorV1>,
    expected: AuthorityErrorV1,
) {
    match result {
        Err(actual) => assert_eq!(actual, expected),
        Ok(grant) => unreachable!("expected {expected:?}, received {grant:?}"),
    }
}

fn generation() -> AuthorityGenerationV1 {
    fixture(AuthorityGenerationV1::new(1))
}

fn principal_generation() -> PrincipalGenerationV1 {
    fixture(PrincipalGenerationV1::new(1))
}

fn membership_generation() -> MembershipGenerationV1 {
    fixture(MembershipGenerationV1::new(1))
}

fn runner_generation() -> RunnerGenerationV1 {
    fixture(RunnerGenerationV1::new(1))
}

fn scopes(values: &[CapabilityScopeV1]) -> CapabilityScopeSetV1 {
    fixture(CapabilityScopeSetV1::new(values.iter().copied()))
}

fn principal(id: &str, kind: PrincipalKindV1) -> PrincipalAuthoritySnapshotV1 {
    PrincipalAuthoritySnapshotV1::new(
        parsed(id),
        kind,
        PrincipalAuthorityStatusV1::Enabled,
        principal_generation(),
    )
}

fn membership(
    room_id: &str,
    member_id: &str,
    principal_id: &str,
    principal_kind: PrincipalKindV1,
    standing: MembershipStandingV1,
    access_mode: AccessModeV1,
) -> MembershipAuthoritySnapshotV1 {
    let role = (access_mode == AccessModeV1::Participant).then(|| "fixture-role".to_owned());
    MembershipAuthoritySnapshotV1::new(
        parsed(room_id),
        fixture(MembershipV1::new(
            parsed(member_id),
            parsed(principal_id),
            principal_kind,
            standing,
            access_mode,
            role,
        )),
        membership_generation(),
    )
}

struct CapabilityFixture<'a> {
    capability_id: &'a str,
    principal_id: &'a str,
    token_byte: u8,
    profile: CapabilityProfileV1,
    scopes: CapabilityScopeSetV1,
    expires_at: Option<&'a str>,
    revoked_at: Option<&'a str>,
}

fn capability(input: CapabilityFixture<'_>) -> CapabilityAuthoritySnapshotV1 {
    fixture(CapabilityAuthoritySnapshotV1::new(
        CapabilityAuthoritySnapshotPartsV1 {
            capability_id: parsed(input.capability_id),
            token_hash: CapabilityBearerV1::from_bytes([input.token_byte; 32]).token_hash(),
            principal_id: parsed(input.principal_id),
            profile: input.profile,
            scopes: input.scopes,
            generation: generation(),
            expires_at: input.expires_at.map(parsed),
            revoked_at: input.revoked_at.map(parsed),
        },
    ))
}

fn presented_capability(capability_id: &str, token_byte: u8) -> PresentedCapabilityV1 {
    PresentedCapabilityV1::new(
        parsed(capability_id),
        CapabilityBearerV1::from_bytes([token_byte; 32]),
    )
}

fn member_use(room_id: &str, action_type: &str) -> AuthorityUseV1 {
    AuthorityUseV1::Member {
        room_id: parsed(room_id),
        member_id: parsed(MEMBER_A),
        operation: MemberAuthorityUseV1::SubmitAction {
            identity: ParticipantActionOperationIdentityV1 {
                room_id: parsed(room_id),
                member_id: parsed(MEMBER_A),
                action_id: parsed(ACTION),
            },
            request_hash: parsed(REQUEST_HASH),
            action_type: action_type.to_owned(),
        },
    }
}

fn action_request(room_id: &str, payload: &[u8]) -> ParticipantActionRequestV1 {
    ParticipantActionRequestV1::new(
        parsed(room_id),
        parsed(MEMBER_A),
        parsed(ACTION),
        fixture(RoomSequenceV1::new(0)),
        "pass",
        fixture(CanonicalJsonV1::parse(payload)),
    )
}

fn timer_request(room_id: &str, payload: &[u8]) -> TimerFiredRequestV1 {
    TimerFiredRequestV1::new(
        parsed(room_id),
        parsed("01ARZ3NDEKTSV4RRFFQ69G5FE1"),
        fixture(TimerGenerationV1::new(1)),
        parsed("2026-08-15T12:30:00Z"),
        fixture(CanonicalJsonV1::parse(payload)),
    )
}

fn member_authority(
    standing: MembershipStandingV1,
    access_mode: AccessModeV1,
    capability_scopes: &[CapabilityScopeV1],
    expires_at: Option<&str>,
) -> (
    Arc<InMemoryAuthorityStoreV1>,
    AuthorityV1,
    PresentedCapabilityV1,
) {
    let store = Arc::new(InMemoryAuthorityStoreV1::new());
    fixture(store.seed_principal(principal(PRINCIPAL_A, PrincipalKindV1::Agent)));
    fixture(store.seed_membership(membership(
        ROOM_A,
        MEMBER_A,
        PRINCIPAL_A,
        PrincipalKindV1::Agent,
        standing,
        access_mode,
    )));
    fixture(store.seed_capability(capability(CapabilityFixture {
        capability_id: CAP_MEMBER,
        principal_id: PRINCIPAL_A,
        token_byte: 7,
        profile: CapabilityProfileV1::RoomMember {
            room_id: parsed(ROOM_A),
            member_id: parsed(MEMBER_A),
        },
        scopes: scopes(capability_scopes),
        expires_at,
        revoked_at: None,
    })));
    let authority = AuthorityV1::new(store.clone());
    (store, authority, presented_capability(CAP_MEMBER, 7))
}

#[test]
fn external_scope_spellings_are_exact_bounded_and_profile_sealed() {
    let all = scopes(&[
        CapabilityScopeV1::RoomAct,
        CapabilityScopeV1::OperatorRoomAdmin,
    ]);
    let encoded = fixture(serde_json::to_string(&all));
    assert_eq!(encoded, r#"["room:act","operator:room_admin"]"#);
    assert!(serde_json::from_str::<CapabilityScopeV1>(r#""operator:diagnostics""#).is_err());
    assert_eq!(
        CapabilityScopeSetV1::new([CapabilityScopeV1::RoomAct, CapabilityScopeV1::RoomAct,]),
        Err(AuthorityShapeErrorV1::DuplicateScope)
    );

    let incompatible = CapabilityAuthoritySnapshotV1::new(CapabilityAuthoritySnapshotPartsV1 {
        capability_id: parsed(CAP_MEMBER),
        token_hash: CapabilityBearerV1::from_bytes([3; 32]).token_hash(),
        principal_id: parsed(PRINCIPAL_A),
        profile: CapabilityProfileV1::RoomMember {
            room_id: parsed(ROOM_A),
            member_id: parsed(MEMBER_A),
        },
        scopes: scopes(&[CapabilityScopeV1::OperatorRoomAdmin]),
        generation: generation(),
        expires_at: None,
        revoked_at: None,
    });
    assert_eq!(
        incompatible,
        Err(AuthorityShapeErrorV1::IncompatibleScopeProfile)
    );
}

#[test]
fn action_authority_has_one_stable_disabled_branch_and_forbids_read_only_members() {
    let (_, enabled, presented) = member_authority(
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        &[CapabilityScopeV1::RoomAct],
        None,
    );
    assert!(matches!(
        fixture(enabled.authorize(
            &presented,
            member_use(ROOM_A, "pack-role-is-checked-later"),
            parsed("2026-08-15T12:00:00Z"),
        )),
        AuthorityGrantV1::ParticipantAction(ParticipantActionAuthorityV1::EnabledParticipant(_))
    ));

    for standing in [
        MembershipStandingV1::Suspended,
        MembershipStandingV1::Departed,
    ] {
        let (_, authority, presented) = member_authority(
            standing,
            AccessModeV1::Participant,
            &[CapabilityScopeV1::RoomAct],
            None,
        );
        assert!(matches!(
            fixture(authority.authorize(
                &presented,
                member_use(ROOM_A, "pass"),
                parsed("2026-08-15T12:00:00Z"),
            )),
            AuthorityGrantV1::ParticipantAction(
                ParticipantActionAuthorityV1::StableMembershipNotEnabled(_)
            )
        ));
    }

    for access_mode in [AccessModeV1::Spectator, AccessModeV1::Operator] {
        for standing in [
            MembershipStandingV1::Enabled,
            MembershipStandingV1::Suspended,
            MembershipStandingV1::Departed,
        ] {
            let (_, authority, presented) =
                member_authority(standing, access_mode, &[CapabilityScopeV1::RoomAct], None);
            assert_authority_error(
                authority.authorize(
                    &presented,
                    member_use(ROOM_A, "pass"),
                    parsed("2026-08-15T12:00:00Z"),
                ),
                AuthorityErrorV1::Forbidden,
            );
        }
    }
}

#[test]
fn action_denials_precede_any_durable_mutation() {
    let (_, authority, presented) = member_authority(
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        &[CapabilityScopeV1::RoomAttach],
        None,
    );
    assert_authority_error(
        authority.authorize(
            &presented,
            member_use(ROOM_A, "pass"),
            parsed("2026-08-15T12:00:00Z"),
        ),
        AuthorityErrorV1::Forbidden,
    );

    let (_, authority, presented) = member_authority(
        MembershipStandingV1::Suspended,
        AccessModeV1::Participant,
        &[CapabilityScopeV1::RoomAct],
        None,
    );
    assert_authority_error(
        authority.authorize(
            &presented,
            member_use(ROOM_B, "pass"),
            parsed("2026-08-15T12:00:00Z"),
        ),
        AuthorityErrorV1::Forbidden,
    );

    let (_, authority, valid_presented) = member_authority(
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        &[CapabilityScopeV1::RoomAct],
        Some("2026-08-15T12:05:00Z"),
    );
    assert_authority_error(
        authority.authorize(
            &valid_presented,
            member_use(ROOM_A, "pass"),
            parsed("2026-08-15T12:05:00Z"),
        ),
        AuthorityErrorV1::Unauthenticated,
    );
    assert_authority_error(
        authority.authorize(
            &presented_capability(CAP_MEMBER, 8),
            member_use(ROOM_A, "pass"),
            parsed("2026-08-15T12:00:00Z"),
        ),
        AuthorityErrorV1::Unauthenticated,
    );
}

#[test]
fn typed_action_authority_derives_and_seals_the_exact_request_purpose() {
    let (_, authority, presented) = member_authority(
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        &[CapabilityScopeV1::RoomAct],
        None,
    );
    let request = action_request(ROOM_A, br#"{"private":"one"}"#);
    let request_hash = fixture(request.canonical_request_hash());
    let expected_use = AuthorityUseV1::Member {
        room_id: parsed(ROOM_A),
        member_id: parsed(MEMBER_A),
        operation: MemberAuthorityUseV1::SubmitAction {
            identity: ParticipantActionOperationIdentityV1 {
                room_id: parsed(ROOM_A),
                member_id: parsed(MEMBER_A),
                action_id: parsed(ACTION),
            },
            request_hash,
            action_type: "pass".to_owned(),
        },
    };
    let grant =
        fixture(authority.authorize_action(&presented, &request, parsed("2026-08-15T12:00:00Z")));
    let ParticipantActionAuthorityV1::EnabledParticipant(grant) = grant else {
        unreachable!("expected enabled typed Action authority");
    };
    let fence = grant.into_fence_facts();
    assert!(fence.binds_use(&expected_use));

    let changed_request = action_request(ROOM_A, br#"{"private":"two"}"#);
    let changed_use = AuthorityUseV1::Member {
        room_id: parsed(ROOM_A),
        member_id: parsed(MEMBER_A),
        operation: MemberAuthorityUseV1::SubmitAction {
            identity: ParticipantActionOperationIdentityV1 {
                room_id: parsed(ROOM_A),
                member_id: parsed(MEMBER_A),
                action_id: parsed(ACTION),
            },
            request_hash: fixture(changed_request.canonical_request_hash()),
            action_type: "pass".to_owned(),
        },
    };
    assert!(!fence.binds_use(&changed_use));
    assert_authority_error(
        authority
            .authorize_action(
                &presented,
                &action_request(ROOM_B, br#"{"private":"one"}"#),
                parsed("2026-08-15T12:00:00Z"),
            )
            .map(AuthorityGrantV1::ParticipantAction),
        AuthorityErrorV1::Forbidden,
    );
}

#[test]
fn spectator_and_operator_memberships_select_distinct_read_only_viewers() {
    let (_, spectator, spectator_capability) = member_authority(
        MembershipStandingV1::Enabled,
        AccessModeV1::Spectator,
        &[CapabilityScopeV1::RoomObservePublic],
        None,
    );
    let spectator_grant = fixture(spectator.authorize_member_read(
        &spectator_capability,
        parsed(ROOM_A),
        parsed(MEMBER_A),
        MemberReadOperationV1::CurrentProjection,
        parsed("2026-08-15T12:00:00Z"),
    ));
    assert_eq!(spectator_grant.room_id(), &parsed::<RoomId>(ROOM_A));
    assert_eq!(
        spectator_grant.operation(),
        MemberReadOperationV1::CurrentProjection
    );

    let (_, operator, operator_capability) = member_authority(
        MembershipStandingV1::Enabled,
        AccessModeV1::Operator,
        &[CapabilityScopeV1::RoomObserveMember],
        None,
    );
    let grant = fixture(operator.authorize_member_read(
        &operator_capability,
        parsed(ROOM_A),
        parsed(MEMBER_A),
        MemberReadOperationV1::CatchUp,
        parsed("2026-08-15T12:00:00Z"),
    ));
    assert_eq!(format!("{grant:?}"), "AuthorizedViewerV1([OPAQUE])");
}

#[test]
fn diagnostic_facade_binds_target_operation_and_consuming_adapter_fence() {
    let (store, authority, capability) = host_authority(None);
    let grant = fixture(authority.authorize_diagnostic(
        &capability,
        DiagnosticTargetV1::Room(parsed(ROOM_A)),
        DiagnosticOperationV1::RawExport,
        parsed("2026-08-15T12:00:00Z"),
    ));
    assert!(matches!(
        grant.target(),
        DiagnosticTargetV1::Room(room_id) if room_id == &parsed::<RoomId>(ROOM_A)
    ));
    assert_eq!(grant.operation(), DiagnosticOperationV1::RawExport);
    let input = grant.into_adapter_input();
    let snapshot = fixture(store.snapshot(&input.authority_snapshot_query()))
        .unwrap_or_else(|| unreachable!("fixture diagnostic snapshot"));
    fixture(input.revalidate_current(&snapshot, &parsed("2026-08-15T12:00:01Z")));
    assert!(matches!(
        input.target(),
        DiagnosticTargetV1::Room(room_id) if room_id == &parsed::<RoomId>(ROOM_A)
    ));
    assert_eq!(format!("{input:?}"), "DiagnosticAdapterInputV1([OPAQUE])");
}

fn host_authority(
    room_id: Option<&str>,
) -> (
    Arc<InMemoryAuthorityStoreV1>,
    AuthorityV1,
    PresentedCapabilityV1,
) {
    let store = Arc::new(InMemoryAuthorityStoreV1::new());
    fixture(store.seed_principal(principal(HOST, PrincipalKindV1::Human)));
    fixture(store.seed_capability(capability(CapabilityFixture {
        capability_id: CAP_HOST,
        principal_id: HOST,
        token_byte: 9,
        profile: CapabilityProfileV1::HostOperator {
            room_id: room_id.map(parsed),
        },
        scopes: scopes(&[
            CapabilityScopeV1::OperatorRoomAdmin,
            CapabilityScopeV1::OperatorBackup,
        ]),
        expires_at: None,
        revoked_at: None,
    })));
    let authority = AuthorityV1::new(store.clone());
    (store, authority, presented_capability(CAP_HOST, 9))
}

#[test]
fn in_memory_capability_token_hashes_are_globally_unique() {
    let store = InMemoryAuthorityStoreV1::new();
    let first = capability(CapabilityFixture {
        capability_id: CAP_HOST,
        principal_id: HOST,
        token_byte: 9,
        profile: CapabilityProfileV1::HostOperator { room_id: None },
        scopes: scopes(&[CapabilityScopeV1::OperatorRoomAdmin]),
        expires_at: None,
        revoked_at: None,
    });
    fixture(store.seed_capability(first));
    let duplicate_bearer = capability(CapabilityFixture {
        capability_id: CAP_MEMBER,
        principal_id: HOST,
        token_byte: 9,
        profile: CapabilityProfileV1::HostOperator { room_id: None },
        scopes: scopes(&[CapabilityScopeV1::OperatorRoomAdmin]),
        expires_at: None,
        revoked_at: None,
    });
    assert_eq!(
        store.seed_capability(duplicate_bearer),
        Err(AuthorityStoreErrorV1::Conflict)
    );

    let (_store, authority, presented) = host_authority(None);
    let duplicate_bearer = fixture(NewCapabilityV1::new(
        parsed(CAP_MEMBER),
        CapabilityBearerV1::from_bytes([9; 32]).token_hash(),
        parsed(HOST),
        CapabilityProfileV1::HostOperator { room_id: None },
        scopes(&[CapabilityScopeV1::OperatorRoomAdmin]),
        None,
    ));
    assert_eq!(
        authority.change(
            &presented,
            AuthorityChangeV1::RegisterCapability {
                change_id: parsed(CHANGE_A),
                capability: duplicate_bearer,
            },
            parsed("2026-08-15T12:00:00Z"),
        ),
        Err(AuthorityErrorV1::Conflict)
    );
}

fn administration_identity() -> AdministrationOperationIdentityV1 {
    AdministrationOperationIdentityV1 {
        authenticated_principal: parsed(HOST),
        versioned_operation_kind: "worldstream/core-proposed/v1".to_owned(),
        idempotency_key: "fixture".to_owned(),
    }
}

fn core_administration_request(room_id: &str, reason: &str) -> CoreAdministrationRequestV1 {
    fixture(CoreAdministrationRequestV1::new(
        parsed(room_id),
        AdministrationOperationIdentityV1 {
            authenticated_principal: parsed(HOST),
            versioned_operation_kind: CORE_OPERATION_KIND.to_owned(),
            idempotency_key: "typed-authority-fixture".to_owned(),
        },
        CoreProposedKindV1::Archive,
        fixture(RoomSequenceV1::new(0)),
        reason,
        CoreChangeSetV1::archive(RoomStatusV1::Active),
    ))
}

fn bootstrap_request(
    change_id: &str,
    principal_id: &str,
    capability_id: &str,
    token_byte: u8,
    expires_at: Option<&str>,
) -> AuthorityBootstrapV1 {
    fixture(AuthorityBootstrapV1::new(
        parsed(change_id),
        parsed(principal_id),
        PrincipalKindV1::Human,
        parsed(capability_id),
        CapabilityBearerV1::from_bytes([token_byte; 32]).token_hash(),
        expires_at.map(parsed),
    ))
}

#[test]
fn bootstrap_is_exact_atomic_idempotent_and_one_time() {
    let store = Arc::new(InMemoryAuthorityStoreV1::new());
    fixture(store.set_commit_checked_at(parsed("2026-08-15T12:00:01Z")));
    let authority = AuthorityV1::new(store.clone());
    let request = bootstrap_request(CHANGE_A, HOST, CAP_HOST, 9, None);
    assert_eq!(
        request.capability().profile(),
        &CapabilityProfileV1::HostOperator { room_id: None }
    );
    assert_eq!(
        request.capability().scopes().iter().collect::<Vec<_>>(),
        vec![CapabilityScopeV1::OperatorRoomAdmin]
    );

    let receipt = fixture(authority.bootstrap(request.clone(), parsed("2026-08-15T12:00:00Z")));
    assert_eq!(
        receipt.result(),
        AuthorityChangeResultV1::AuthorityBootstrapped
    );
    assert_eq!(receipt.resulting_generation(), 1);
    assert_eq!(receipt.changed_at().as_str(), "2026-08-15T12:00:01Z");
    assert_eq!(
        receipt.target(),
        &AuthorityChangeTargetV1::Bootstrap {
            principal_id: parsed(HOST),
            capability_id: parsed(CAP_HOST),
        }
    );
    assert_eq!(
        fixture(authority.bootstrap(request, parsed("2026-08-15T12:00:00Z"))),
        receipt
    );

    assert_eq!(
        authority.bootstrap(
            bootstrap_request(CHANGE_A, PRINCIPAL_A, CAP_RUNNER, 8, None),
            parsed("2026-08-15T12:00:00Z"),
        ),
        Err(AuthorityErrorV1::Conflict)
    );
    assert_eq!(
        authority.bootstrap(
            bootstrap_request(CHANGE_B, HOST, CAP_HOST, 9, None),
            parsed("2026-08-15T12:00:00Z"),
        ),
        Err(AuthorityErrorV1::Conflict)
    );

    assert!(matches!(
        fixture(authority.authorize(
            &presented_capability(CAP_HOST, 9),
            AuthorityUseV1::CreateRoom {
                identity: administration_identity(),
                request_hash: parsed(REQUEST_HASH),
            },
            parsed("2026-08-15T12:00:02Z"),
        )),
        AuthorityGrantV1::RoomCreation(_)
    ));
}

#[test]
fn bootstrap_rejects_backwards_or_expired_commit_without_partial_install() {
    let store = Arc::new(InMemoryAuthorityStoreV1::new());
    let authority = AuthorityV1::new(store.clone());
    let expiring = bootstrap_request(CHANGE_A, HOST, CAP_HOST, 9, Some("2026-08-15T12:05:00Z"));

    fixture(store.set_commit_checked_at(parsed("2026-08-15T11:59:59Z")));
    assert_eq!(
        authority.bootstrap(expiring.clone(), parsed("2026-08-15T12:00:00Z")),
        Err(AuthorityErrorV1::InvalidAuthorityRequest)
    );
    fixture(store.set_commit_checked_at(parsed("2026-08-15T12:05:00Z")));
    assert_eq!(
        authority.bootstrap(expiring.clone(), parsed("2026-08-15T12:00:00Z")),
        Err(AuthorityErrorV1::InvalidAuthorityRequest)
    );

    fixture(store.set_commit_checked_at(parsed("2026-08-15T12:04:59Z")));
    assert_eq!(
        fixture(authority.bootstrap(expiring, parsed("2026-08-15T12:00:00Z"))).result(),
        AuthorityChangeResultV1::AuthorityBootstrapped
    );
}

#[test]
fn room_bound_and_global_host_authority_have_distinct_attribution() {
    let classified = ClassifiedCoreAdministrationV1::new(
        CoreProposedKindV1::Archive,
        CoreAdministrationClassV1::Mandatory,
        0,
    );
    let (_, global, global_capability) = host_authority(None);
    let global_grant = fixture(global.authorize(
        &global_capability,
        AuthorityUseV1::CoreAdministration {
            room_id: parsed(ROOM_A),
            classified: classified.clone(),
            identity: administration_identity(),
            request_hash: parsed(REQUEST_HASH),
        },
        parsed("2026-08-15T12:00:00Z"),
    ));
    let AuthorityGrantV1::CoreAdministration(global_grant) = global_grant else {
        unreachable!("expected Core administration grant");
    };
    assert_eq!(
        global_grant.attribution().authority_kind,
        CoreAuthorityKindV1::HostOperator
    );

    let (_, room_admin, room_capability) = host_authority(Some(ROOM_A));
    let room_grant = fixture(room_admin.authorize(
        &room_capability,
        AuthorityUseV1::CoreAdministration {
            room_id: parsed(ROOM_A),
            classified,
            identity: administration_identity(),
            request_hash: parsed(REQUEST_HASH),
        },
        parsed("2026-08-15T12:00:00Z"),
    ));
    let AuthorityGrantV1::CoreAdministration(room_grant) = room_grant else {
        unreachable!("expected room administration grant");
    };
    assert_eq!(
        room_grant.attribution().authority_kind,
        CoreAuthorityKindV1::RoomAdministrator
    );
    assert_authority_error(
        room_admin.authorize(
            &room_capability,
            AuthorityUseV1::CreateRoom {
                identity: administration_identity(),
                request_hash: parsed(REQUEST_HASH),
            },
            parsed("2026-08-15T12:00:00Z"),
        ),
        AuthorityErrorV1::Forbidden,
    );
}

#[test]
fn typed_timer_authority_binds_exact_room_root_and_request_hash() {
    let request = timer_request(ROOM_A, br#"{"kind":"deadline"}"#);
    let (_, global, global_capability) = host_authority(None);
    let grant = fixture(global.authorize_timer_fired(
        &global_capability,
        &request,
        parsed("2026-08-15T12:00:00Z"),
    ));
    assert_eq!(format!("{grant:?}"), "AuthorizedTimerFiredV1([OPAQUE])");
    assert_eq!(grant.room_id(), &parsed::<RoomId>(ROOM_A));
    assert_eq!(
        grant.request_hash(),
        &fixture(request.canonical_request_hash())
    );

    let (_, room_admin, room_capability) = host_authority(Some(ROOM_A));
    fixture(room_admin.authorize_timer_fired(
        &room_capability,
        &request,
        parsed("2026-08-15T12:00:00Z"),
    ));
    assert_eq!(
        room_admin
            .authorize_timer_fired(
                &room_capability,
                &timer_request(ROOM_B, br#"{"kind":"deadline"}"#),
                parsed("2026-08-15T12:00:00Z"),
            )
            .err(),
        Some(AuthorityErrorV1::Forbidden)
    );
}

#[test]
fn typed_core_administration_derives_class_hash_and_exact_room() {
    let (_, global, global_capability) = host_authority(None);
    let request = core_administration_request(ROOM_A, "private-reason-one");
    let expected_use = AuthorityUseV1::CoreAdministration {
        room_id: request.room_id().clone(),
        classified: fixture(request.classified()),
        identity: request.operation_identity().clone(),
        request_hash: fixture(request.canonical_request_hash()),
    };
    let grant = fixture(global.authorize_core_administration(
        &global_capability,
        &request,
        parsed("2026-08-15T12:00:00Z"),
    ));
    assert_eq!(
        grant.classified().class(),
        CoreAdministrationClassV1::Mandatory
    );
    let fence = grant.into_fence_facts();
    assert!(fence.binds_use(&expected_use));

    let changed = core_administration_request(ROOM_A, "private-reason-two");
    let changed_use = AuthorityUseV1::CoreAdministration {
        room_id: changed.room_id().clone(),
        classified: fixture(changed.classified()),
        identity: changed.operation_identity().clone(),
        request_hash: fixture(changed.canonical_request_hash()),
    };
    assert!(!fence.binds_use(&changed_use));

    let (_, room_admin, room_capability) = host_authority(Some(ROOM_A));
    assert_authority_error(
        room_admin
            .authorize_core_administration(
                &room_capability,
                &core_administration_request(ROOM_B, "cross-room"),
                parsed("2026-08-15T12:00:00Z"),
            )
            .map(AuthorityGrantV1::CoreAdministration),
        AuthorityErrorV1::Forbidden,
    );
}

#[test]
fn receipt_read_grant_retains_identity_hash_and_closed_target_policy() {
    let (_, authority, host_capability) = host_authority(None);
    let identity = OperationIdentityV1::Administration(Box::new(administration_identity()));
    let request_hash: CanonicalRequestHashV1 = parsed(REQUEST_HASH);
    let exact_use = AuthorityUseV1::ReadRoomOperationResult {
        identity: identity.clone(),
        request_hash: request_hash.clone(),
        target_room_id: Some(parsed(ROOM_A)),
    };
    let grant = fixture(authority.authorize(
        &host_capability,
        exact_use.clone(),
        parsed("2026-08-15T12:00:00Z"),
    ));
    let AuthorityGrantV1::ReceiptRead(grant) = grant else {
        unreachable!("expected exact receipt-read grant");
    };
    let parts = grant.into_adapter_input();
    assert_eq!(parts.identity, identity);
    assert_eq!(parts.request_hash, request_hash);
    assert!(matches!(
        parts.target_policy,
        crate::authority::ReceiptReadTargetPolicyV1::ExactRoom(ref room_id)
            if room_id == &parsed::<RoomId>(ROOM_A)
    ));
    assert!(parts.fence.binds_use(&exact_use));

    assert_authority_error(
        authority.authorize(
            &host_capability,
            AuthorityUseV1::ReadRoomOperationResult {
                identity: OperationIdentityV1::Administration(Box::new(administration_identity())),
                request_hash: parsed(REQUEST_HASH),
                target_room_id: None,
            },
            parsed("2026-08-15T12:00:00Z"),
        ),
        AuthorityErrorV1::InvalidAuthorityRequest,
    );

    let create_identity = AdministrationOperationIdentityV1 {
        authenticated_principal: parsed(HOST),
        versioned_operation_kind: CREATE_ROOM_OPERATION_KIND.to_owned(),
        idempotency_key: "create-receipt".to_owned(),
    };
    let create_use = AuthorityUseV1::ReadRoomOperationResult {
        identity: OperationIdentityV1::Administration(Box::new(create_identity.clone())),
        request_hash: parsed(REQUEST_HASH),
        target_room_id: None,
    };
    let create_grant =
        fixture(authority.authorize(&host_capability, create_use, parsed("2026-08-15T12:00:00Z")));
    let AuthorityGrantV1::ReceiptRead(create_grant) = create_grant else {
        unreachable!("expected global Create receipt-read grant");
    };
    assert!(matches!(
        create_grant.into_adapter_input().target_policy,
        crate::authority::ReceiptReadTargetPolicyV1::GlobalCreate
    ));
    assert_authority_error(
        authority.authorize(
            &host_capability,
            AuthorityUseV1::ReadRoomOperationResult {
                identity: OperationIdentityV1::Administration(Box::new(create_identity)),
                request_hash: parsed(REQUEST_HASH),
                target_room_id: Some(parsed(ROOM_A)),
            },
            parsed("2026-08-15T12:00:00Z"),
        ),
        AuthorityErrorV1::InvalidAuthorityRequest,
    );

    let (_, room_admin, room_capability) = host_authority(Some(ROOM_A));
    assert_authority_error(
        room_admin.authorize(
            &room_capability,
            AuthorityUseV1::ReadRoomOperationResult {
                identity: OperationIdentityV1::Administration(Box::new(administration_identity())),
                request_hash: parsed(REQUEST_HASH),
                target_room_id: Some(parsed(ROOM_B)),
            },
            parsed("2026-08-15T12:00:00Z"),
        ),
        AuthorityErrorV1::Forbidden,
    );
}

#[test]
fn replay_grant_retains_exact_address_sequence_and_purpose() {
    let (_, authority, presented) = member_authority(
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        &[CapabilityScopeV1::RoomReplay],
        None,
    );
    let use_ = AuthorityUseV1::Replay {
        room_id: parsed(ROOM_A),
        member_id: parsed(MEMBER_A),
        at_room_seq: fixture(RoomSequenceV1::new(7)),
        projection_kind: ReplayProjectionKindV1::HistoricalMembership,
    };
    let grant =
        fixture(authority.authorize(&presented, use_.clone(), parsed("2026-08-15T12:00:00Z")));
    let AuthorityGrantV1::Replay(grant) = grant else {
        unreachable!("expected Replay grant");
    };
    assert_eq!(grant.room_id(), &parsed::<RoomId>(ROOM_A));
    assert_eq!(grant.member_id(), &parsed::<MemberId>(MEMBER_A));
    assert_eq!(grant.at_room_seq(), fixture(RoomSequenceV1::new(7)));
    let input = grant.into_adapter_input();
    assert!(input.fence.binds_use(&use_));
    assert!(!input.fence.binds_use(&AuthorityUseV1::Replay {
        room_id: parsed(ROOM_A),
        member_id: parsed(MEMBER_A),
        at_room_seq: fixture(RoomSequenceV1::new(8)),
        projection_kind: ReplayProjectionKindV1::HistoricalMembership,
    }));
    assert_authority_error(
        authority.authorize(
            &presented,
            AuthorityUseV1::Replay {
                room_id: parsed(ROOM_B),
                member_id: parsed(MEMBER_A),
                at_room_seq: fixture(RoomSequenceV1::new(7)),
                projection_kind: ReplayProjectionKindV1::HistoricalMembership,
            },
            parsed("2026-08-15T12:00:00Z"),
        ),
        AuthorityErrorV1::Forbidden,
    );
}

#[test]
fn final_reveal_is_forbidden_from_every_public_replay_grant_path() {
    let (_, authority, presented) = member_authority(
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        &[CapabilityScopeV1::RoomReplay],
        None,
    );
    assert_authority_error(
        authority.authorize(
            &presented,
            AuthorityUseV1::Replay {
                room_id: parsed(ROOM_A),
                member_id: parsed(MEMBER_A),
                at_room_seq: fixture(RoomSequenceV1::new(7)),
                projection_kind: ReplayProjectionKindV1::FinalReveal,
            },
            parsed("2026-08-15T12:00:00Z"),
        ),
        AuthorityErrorV1::Forbidden,
    );
    assert_authority_error(
        authority
            .authorize_replay(
                &presented,
                parsed(ROOM_A),
                parsed(MEMBER_A),
                fixture(RoomSequenceV1::new(7)),
                ReplayProjectionKindV1::FinalReveal,
                parsed("2026-08-15T12:00:00Z"),
            )
            .map(AuthorityGrantV1::Replay),
        AuthorityErrorV1::Forbidden,
    );
}

#[test]
fn runner_control_is_separate_and_may_target_another_agent_principal() {
    let store = Arc::new(InMemoryAuthorityStoreV1::new());
    fixture(store.seed_principal(principal(HOST, PrincipalKindV1::Human)));
    fixture(store.seed_membership(membership(
        ROOM_A,
        MEMBER_B,
        PRINCIPAL_B,
        PrincipalKindV1::Agent,
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
    )));
    fixture(store.seed_runner(RunnerAuthoritySnapshotV1::new(
        parsed(RUNNER),
        parsed(HOST),
        RunnerAuthorityStatusV1::Enabled,
        runner_generation(),
    )));
    let target = RoomMembershipKeyV1 {
        room_id: parsed(ROOM_A),
        member_id: parsed(MEMBER_B),
    };
    fixture(store.seed_capability(capability(CapabilityFixture {
        capability_id: CAP_RUNNER,
        principal_id: HOST,
        token_byte: 5,
        profile: CapabilityProfileV1::RunnerControl {
            runner_id: parsed(RUNNER),
            permitted_memberships: fixture(RunnerMembershipSetV1::new([target.clone()])),
        },
        scopes: scopes(&[
            CapabilityScopeV1::ActivationClaim,
            CapabilityScopeV1::ActivationComplete,
        ]),
        expires_at: None,
        revoked_at: None,
    })));
    let authority = AuthorityV1::new(store.clone());
    let runner_capability = presented_capability(CAP_RUNNER, 5);
    let grant = fixture(authority.authorize_runner_control(
        &runner_capability,
        parsed(RUNNER),
        RunnerControlOperationV1::Claim,
        target,
        parsed("2026-08-15T12:00:00Z"),
    ));
    assert_eq!(grant.runner_id(), &parsed::<RunnerId>(RUNNER));
    let input = grant.into_adapter_input();
    let snapshot = fixture(store.snapshot(&input.authority_snapshot_query()))
        .unwrap_or_else(|| unreachable!("fixture Runner snapshot"));
    fixture(input.revalidate_current(&snapshot, &parsed("2026-08-15T12:00:01Z")));
    let revoked_same_generation = RunnerAuthoritySnapshotV1::new(
        parsed(RUNNER),
        parsed(HOST),
        RunnerAuthorityStatusV1::Revoked,
        snapshot
            .runner()
            .unwrap_or_else(|| unreachable!("fixture Runner"))
            .generation(),
    );
    let corrupt_snapshot = fixture(AuthoritySnapshotV1::new(
        snapshot.capability().clone(),
        snapshot.principal().clone(),
        snapshot.membership().cloned(),
        Some(revoked_same_generation),
    ));
    assert_eq!(
        input.revalidate_current(&corrupt_snapshot, &parsed("2026-08-15T12:00:01Z")),
        Err(AuthorityErrorV1::StaleAuthorityGeneration)
    );
    assert_authority_error(
        authority.authorize(
            &runner_capability,
            member_use(ROOM_A, "pass"),
            parsed("2026-08-15T12:00:00Z"),
        ),
        AuthorityErrorV1::Forbidden,
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn runner_control_can_be_provisioned_and_immediately_revoked() {
    let (store, authority, host_capability) = host_authority(None);
    fixture(store.set_commit_checked_at(parsed("2026-08-15T12:00:01Z")));
    fixture(authority.change(
        &host_capability,
        AuthorityChangeV1::CreatePrincipal {
            change_id: parsed(CHANGE_A),
            principal_id: parsed(PRINCIPAL_B),
            kind: PrincipalKindV1::Agent,
        },
        parsed("2026-08-15T12:00:00Z"),
    ));
    let registered_runner = fixture(authority.change(
        &host_capability,
        AuthorityChangeV1::RegisterRunner {
            change_id: parsed(CHANGE_B),
            runner_id: parsed(RUNNER),
            owner_principal_id: parsed(HOST),
        },
        parsed("2026-08-15T12:00:00Z"),
    ));
    assert_eq!(
        registered_runner.result(),
        AuthorityChangeResultV1::RunnerRegistered
    );
    assert_eq!(registered_runner.resulting_generation(), 1);

    fixture(store.seed_membership(membership(
        ROOM_A,
        MEMBER_B,
        PRINCIPAL_B,
        PrincipalKindV1::Agent,
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
    )));
    let target = RoomMembershipKeyV1 {
        room_id: parsed(ROOM_A),
        member_id: parsed(MEMBER_B),
    };
    let runner_token = CapabilityBearerV1::from_bytes([5; 32]);
    let runner_capability = fixture(NewCapabilityV1::new(
        parsed(CAP_RUNNER),
        runner_token.token_hash(),
        parsed(HOST),
        CapabilityProfileV1::RunnerControl {
            runner_id: parsed(RUNNER),
            permitted_memberships: fixture(RunnerMembershipSetV1::new([target.clone()])),
        },
        scopes(&[CapabilityScopeV1::ActivationClaim]),
        None,
    ));
    assert_eq!(
        fixture(authority.change(
            &host_capability,
            AuthorityChangeV1::RegisterCapability {
                change_id: parsed(CHANGE_C),
                capability: runner_capability,
            },
            parsed("2026-08-15T12:00:00Z"),
        ))
        .result(),
        AuthorityChangeResultV1::CapabilityRegistered
    );
    let presented_runner = PresentedCapabilityV1::new(parsed(CAP_RUNNER), runner_token);
    let runner_use = AuthorityUseV1::RunnerControl {
        runner_id: parsed(RUNNER),
        operation: RunnerControlOperationV1::Claim,
        target: Some(target),
    };
    assert!(matches!(
        fixture(authority.authorize(
            &presented_runner,
            runner_use.clone(),
            parsed("2026-08-15T12:00:00Z"),
        )),
        AuthorityGrantV1::RunnerControl(_)
    ));

    fixture(store.set_commit_checked_at(parsed("2026-08-15T12:00:02Z")));
    let revoke = AuthorityChangeV1::RevokeRunner {
        change_id: parsed(CHANGE_D),
        runner_id: parsed(RUNNER),
        expected_generation: runner_generation(),
        reason_code: fixture(AuthorityReasonCodeV1::new("fixture.runner_revoke")),
    };
    let revoked = fixture(authority.change(
        &host_capability,
        revoke.clone(),
        parsed("2026-08-15T12:00:01Z"),
    ));
    assert_eq!(revoked.result(), AuthorityChangeResultV1::RunnerRevoked);
    assert_eq!(revoked.resulting_generation(), 2);
    assert_eq!(
        fixture(authority.change(&host_capability, revoke, parsed("2026-08-15T12:00:01Z"),)),
        revoked
    );
    assert_authority_error(
        authority.authorize(
            &presented_runner,
            runner_use,
            parsed("2026-08-15T12:00:02Z"),
        ),
        AuthorityErrorV1::Forbidden,
    );
    assert_eq!(
        authority.change(
            &host_capability,
            AuthorityChangeV1::RevokeRunner {
                change_id: parsed(CHANGE_E),
                runner_id: parsed(RUNNER),
                expected_generation: runner_generation(),
                reason_code: fixture(AuthorityReasonCodeV1::new("fixture.stale_runner_revoke")),
            },
            parsed("2026-08-15T12:00:01Z"),
        ),
        Err(AuthorityErrorV1::StaleAuthorityGeneration)
    );
}

#[test]
fn authority_change_uses_store_owned_commit_time_for_expiry_and_ordering() {
    let store = Arc::new(InMemoryAuthorityStoreV1::new());
    fixture(store.seed_principal(principal(HOST, PrincipalKindV1::Human)));
    fixture(store.seed_capability(capability(CapabilityFixture {
        capability_id: CAP_HOST,
        principal_id: HOST,
        token_byte: 9,
        profile: CapabilityProfileV1::HostOperator { room_id: None },
        scopes: scopes(&[CapabilityScopeV1::OperatorRoomAdmin]),
        expires_at: Some("2026-08-15T12:05:00Z"),
        revoked_at: None,
    })));
    let authority = AuthorityV1::new(store.clone());
    let host_capability = presented_capability(CAP_HOST, 9);
    let create = AuthorityChangeV1::CreatePrincipal {
        change_id: parsed(CHANGE_A),
        principal_id: parsed(PRINCIPAL_A),
        kind: PrincipalKindV1::Agent,
    };

    fixture(store.set_commit_checked_at(parsed("2026-08-15T12:05:00Z")));
    assert_eq!(
        authority.change(
            &host_capability,
            create.clone(),
            parsed("2026-08-15T12:04:00Z"),
        ),
        Err(AuthorityErrorV1::StaleAuthorityGeneration)
    );

    fixture(store.set_commit_checked_at(parsed("2026-08-15T12:03:59Z")));
    assert_eq!(
        authority.change(
            &host_capability,
            create.clone(),
            parsed("2026-08-15T12:04:00Z"),
        ),
        Err(AuthorityErrorV1::InvalidAuthorityRequest)
    );

    fixture(store.set_commit_checked_at(parsed("2026-08-15T12:04:59Z")));
    let receipt =
        fixture(authority.change(&host_capability, create, parsed("2026-08-15T12:04:00Z")));
    assert_eq!(receipt.changed_at().as_str(), "2026-08-15T12:04:59Z");
    assert_eq!(receipt.result(), AuthorityChangeResultV1::PrincipalCreated);
}

#[test]
fn sealed_action_fence_binds_purpose_and_revalidates_time_and_generation() {
    let (store, authority, presented) = member_authority(
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        &[CapabilityScopeV1::RoomAct],
        Some("2026-08-15T12:10:00Z"),
    );
    let use_ = member_use(ROOM_A, "pass");
    let grant =
        fixture(authority.authorize(&presented, use_.clone(), parsed("2026-08-15T12:00:00Z")));
    let AuthorityGrantV1::ParticipantAction(ParticipantActionAuthorityV1::EnabledParticipant(
        grant,
    )) = grant
    else {
        unreachable!("expected enabled Action grant");
    };
    let fence = grant.into_fence_facts();
    assert!(fence.binds_use(&use_));
    assert!(!fence.binds_use(&member_use(ROOM_A, "different")));
    let snapshot = fixture(store.snapshot(&fence.snapshot_query()))
        .unwrap_or_else(|| unreachable!("fixture snapshot must exist"));
    fixture(fence.revalidate_current(&snapshot, &parsed("2026-08-15T12:09:59Z")));
    assert_eq!(
        fence.revalidate_current(&snapshot, &parsed("2026-08-15T11:59:59Z")),
        Err(AuthorityErrorV1::InvalidAuthorityRequest)
    );
    assert_eq!(
        fence.revalidate_current(&snapshot, &parsed("2026-08-15T12:10:00Z")),
        Err(AuthorityErrorV1::StaleAuthorityGeneration)
    );

    let changed_membership = MembershipAuthoritySnapshotV1::new(
        parsed(ROOM_A),
        snapshot.membership().map_or_else(
            || unreachable!("fixture Membership must exist"),
            |membership| membership.membership().clone(),
        ),
        fixture(MembershipGenerationV1::new(2)),
    );
    let changed_snapshot = fixture(AuthoritySnapshotV1::new(
        snapshot.capability().clone(),
        snapshot.principal().clone(),
        Some(changed_membership),
        None,
    ));
    assert_eq!(
        fence.revalidate_current(&changed_snapshot, &parsed("2026-08-15T12:05:00Z")),
        Err(AuthorityErrorV1::StaleAuthorityGeneration)
    );

    let current_membership = snapshot
        .membership()
        .unwrap_or_else(|| unreachable!("fixture Membership"));
    let changed_membership = MembershipV1::new(
        current_membership.membership().member_id().clone(),
        current_membership.membership().principal_id().clone(),
        current_membership.membership().principal_kind(),
        MembershipStandingV1::Suspended,
        current_membership.membership().access_mode(),
        current_membership.membership().role().map(str::to_owned),
    )
    .unwrap_or_else(|error| unreachable!("fixture changed Membership: {error}"));
    let same_generation_membership = MembershipAuthoritySnapshotV1::new(
        parsed(ROOM_A),
        changed_membership,
        current_membership.generation(),
    );
    let corrupt_snapshot = fixture(AuthoritySnapshotV1::new(
        snapshot.capability().clone(),
        snapshot.principal().clone(),
        Some(same_generation_membership),
        None,
    ));
    assert_eq!(
        fence.revalidate_current(&corrupt_snapshot, &parsed("2026-08-15T12:05:00Z")),
        Err(AuthorityErrorV1::StaleAuthorityGeneration)
    );

    let changed_principal = PrincipalAuthoritySnapshotV1::new(
        snapshot.principal().principal_id().clone(),
        PrincipalKindV1::Human,
        PrincipalAuthorityStatusV1::Enabled,
        snapshot.principal().generation(),
    );
    let corrupt_snapshot = fixture(AuthoritySnapshotV1::new(
        snapshot.capability().clone(),
        changed_principal,
        snapshot.membership().cloned(),
        None,
    ));
    assert_eq!(
        fence.revalidate_current(&corrupt_snapshot, &parsed("2026-08-15T12:05:00Z")),
        Err(AuthorityErrorV1::StaleAuthorityGeneration)
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn in_memory_changes_are_idempotent_narrow_only_and_immediately_revocable() {
    let (store, authority, host_capability) = host_authority(None);
    let create = AuthorityChangeV1::CreatePrincipal {
        change_id: parsed(CHANGE_A),
        principal_id: parsed(PRINCIPAL_A),
        kind: PrincipalKindV1::Agent,
    };
    let created = fixture(authority.change(
        &host_capability,
        create.clone(),
        parsed("2026-08-15T12:00:00Z"),
    ));
    assert_eq!(created.result(), AuthorityChangeResultV1::PrincipalCreated);
    assert_eq!(created.resulting_generation(), 1);
    assert_eq!(
        fixture(authority.change(&host_capability, create, parsed("2026-08-15T12:00:00Z"),)),
        created
    );
    let conflicting = AuthorityChangeV1::CreatePrincipal {
        change_id: parsed(CHANGE_A),
        principal_id: parsed(PRINCIPAL_B),
        kind: PrincipalKindV1::Agent,
    };
    assert_eq!(
        authority.change(
            &host_capability,
            conflicting,
            parsed("2026-08-15T12:00:00Z"),
        ),
        Err(AuthorityErrorV1::Conflict)
    );

    fixture(store.seed_membership(membership(
        ROOM_A,
        MEMBER_A,
        PRINCIPAL_A,
        PrincipalKindV1::Agent,
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
    )));
    let new_token = CapabilityBearerV1::from_bytes([4; 32]);
    let new_capability = fixture(NewCapabilityV1::new(
        parsed(CAP_MEMBER),
        new_token.token_hash(),
        parsed(PRINCIPAL_A),
        CapabilityProfileV1::RoomMember {
            room_id: parsed(ROOM_A),
            member_id: parsed(MEMBER_A),
        },
        scopes(&[CapabilityScopeV1::RoomAttach, CapabilityScopeV1::RoomAct]),
        Some(parsed("2026-08-15T14:00:00Z")),
    ));
    let registered = fixture(authority.change(
        &host_capability,
        AuthorityChangeV1::RegisterCapability {
            change_id: parsed(CHANGE_B),
            capability: new_capability,
        },
        parsed("2026-08-15T12:01:00Z"),
    ));
    assert_eq!(
        registered.result(),
        AuthorityChangeResultV1::CapabilityRegistered
    );

    let narrowed = fixture(authority.change(
        &host_capability,
        AuthorityChangeV1::NarrowCapability {
            change_id: parsed(CHANGE_C),
            capability_id: parsed(CAP_MEMBER),
            expected_generation: generation(),
            scopes: scopes(&[CapabilityScopeV1::RoomAct]),
            expires_at: Some(parsed("2026-08-15T13:00:00Z")),
            reason_code: fixture(AuthorityReasonCodeV1::new("fixture.narrow")),
        },
        parsed("2026-08-15T12:02:00Z"),
    ));
    assert_eq!(narrowed.resulting_generation(), 2);
    assert_eq!(
        authority.change(
            &host_capability,
            AuthorityChangeV1::NarrowCapability {
                change_id: parsed(CHANGE_D),
                capability_id: parsed(CAP_MEMBER),
                expected_generation: fixture(AuthorityGenerationV1::new(2)),
                scopes: scopes(&[CapabilityScopeV1::RoomAct]),
                expires_at: Some(parsed("2026-08-15T13:00:00Z")),
                reason_code: fixture(AuthorityReasonCodeV1::new("fixture.noop")),
            },
            parsed("2026-08-15T12:03:00Z"),
        ),
        Err(AuthorityErrorV1::InvalidAuthorityRequest)
    );

    let disabled = fixture(authority.change(
        &host_capability,
        AuthorityChangeV1::SetPrincipalStatus {
            change_id: parsed(CHANGE_E),
            principal_id: parsed(PRINCIPAL_A),
            expected_generation: principal_generation(),
            status: PrincipalAuthorityStatusV1::Disabled,
            reason_code: fixture(AuthorityReasonCodeV1::new("fixture.disable")),
        },
        parsed("2026-08-15T12:04:00Z"),
    ));
    assert_eq!(disabled.resulting_generation(), 2);
    assert_authority_error(
        authority.authorize(
            &PresentedCapabilityV1::new(
                parsed(CAP_MEMBER),
                CapabilityBearerV1::from_bytes([4; 32]),
            ),
            member_use(ROOM_A, "pass"),
            parsed("2026-08-15T12:04:00Z"),
        ),
        AuthorityErrorV1::Unauthenticated,
    );
    let enabled = fixture(authority.change(
        &host_capability,
        AuthorityChangeV1::SetPrincipalStatus {
            change_id: parsed(CHANGE_F),
            principal_id: parsed(PRINCIPAL_A),
            expected_generation: fixture(PrincipalGenerationV1::new(2)),
            status: PrincipalAuthorityStatusV1::Enabled,
            reason_code: fixture(AuthorityReasonCodeV1::new("fixture.enable")),
        },
        parsed("2026-08-15T12:05:00Z"),
    ));
    assert_eq!(enabled.resulting_generation(), 3);

    let revoked = fixture(authority.change(
        &host_capability,
        AuthorityChangeV1::RevokeCapability {
            change_id: parsed(CHANGE_G),
            capability_id: parsed(CAP_MEMBER),
            expected_generation: fixture(AuthorityGenerationV1::new(2)),
            reason_code: fixture(AuthorityReasonCodeV1::new("fixture.revoke")),
        },
        parsed("2026-08-15T12:06:00Z"),
    ));
    assert_eq!(revoked.resulting_generation(), 3);
    assert_authority_error(
        authority.authorize(
            &PresentedCapabilityV1::new(parsed(CAP_MEMBER), new_token),
            member_use(ROOM_A, "pass"),
            parsed("2026-08-15T12:06:00Z"),
        ),
        AuthorityErrorV1::Unauthenticated,
    );
}

#[test]
fn debug_and_errors_do_not_expose_credentials_scopes_or_request_payloads() {
    let (store, authority, valid_presented) = member_authority(
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        &[CapabilityScopeV1::RoomAct],
        None,
    );
    let bearer_debug = format!("{:?}", CapabilityBearerV1::from_bytes([0xab; 32]));
    assert_eq!(bearer_debug, "CapabilityBearerV1([REDACTED])");
    let token_debug = format!(
        "{:?}",
        CapabilityBearerV1::from_bytes([0xcd; 32]).token_hash()
    );
    assert_eq!(token_debug, "CapabilityTokenHashV1([REDACTED])");
    let use_ = member_use(ROOM_A, "private-action-type");
    assert!(!format!("{use_:?}").contains("private-action-type"));
    let grant =
        fixture(authority.authorize(&valid_presented, use_, parsed("2026-08-15T12:00:00Z")));
    let grant_debug = format!("{grant:?}");
    assert_eq!(grant_debug, "AuthorityGrantV1::ParticipantAction([OPAQUE])");
    let AuthorityGrantV1::ParticipantAction(ParticipantActionAuthorityV1::EnabledParticipant(
        grant,
    )) = grant
    else {
        unreachable!("expected enabled Action grant");
    };
    let fence = grant.into_fence_facts();
    let fence_debug = format!("{fence:?}");
    assert!(!fence_debug.contains(ROOM_A));
    assert!(!fence_debug.contains(MEMBER_A));
    assert!(!fence_debug.contains("room:act"));
    let snapshot = fixture(store.snapshot(&fence.snapshot_query()))
        .unwrap_or_else(|| unreachable!("fixture snapshot must exist"));
    let snapshot_debug = format!("{:?}", snapshot.capability());
    assert!(!snapshot_debug.contains(ROOM_A));
    assert!(!snapshot_debug.contains(MEMBER_A));
    assert!(!snapshot_debug.contains("room:act"));
    let error = authority.authorize(
        &presented_capability(CAP_MEMBER, 99),
        member_use(ROOM_A, "private-action-type"),
        parsed("2026-08-15T12:00:00Z"),
    );
    assert_eq!(format!("{:?}", error.err()), "Some(Unauthenticated)");
}
