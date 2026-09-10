use std::mem::size_of;

use worldstream_core::{
    CoreReducerV1, PackRegistryErrorV1, PreparedCoreStateV1, PreparedRoomTransitionV1,
    RoomTransitionPreparerV1, RoomTransitionStateV1, TimerRequestV1, VerifiedCoreStateV1,
    agent_heist_clock_safe_digest, agent_heist_lobby_digest, agent_heist_schema_safe_digest,
    builtin_worldstream_registry, counter_v1_digest, counter_v2_digest, counter_v3_digest,
    counter_v4_digest,
};

#[test]
fn deep_reduction_and_timer_types_are_reachable_downstream() {
    assert!(size_of::<CoreReducerV1>() > 0);
    assert!(size_of::<PreparedCoreStateV1>() > 0);
    assert!(size_of::<RoomTransitionPreparerV1>() > 0);
    assert!(size_of::<RoomTransitionStateV1>() > 0);
    assert!(size_of::<PreparedRoomTransitionV1>() > 0);
    assert!(size_of::<VerifiedCoreStateV1>() > 0);
    assert!(size_of::<TimerRequestV1>() > 0);
}

#[test]
fn activity_pack_catalog_preserves_exact_revision_identity_and_schema_bytes() {
    let registry = builtin_worldstream_registry()
        .unwrap_or_else(|error| unreachable!("built-in registry: {error}"));

    let revisions = registry.catalog_revisions().collect::<Vec<_>>();
    assert_eq!(revisions.len(), 9);
    let schema_safe = registry
        .catalog_revision(&agent_heist_schema_safe_digest())
        .unwrap_or_else(|error| unreachable!("schema-safe revision: {error}"));
    assert!(schema_safe.selectable_for_new_rooms);
    assert!(schema_safe.runnable_for_retained_rooms);
    assert_eq!(schema_safe.descriptor.explanatory_version, "0.5.0");
    let clock_safe = registry
        .catalog_revision(&agent_heist_clock_safe_digest())
        .unwrap_or_else(|error| unreachable!("clock-safe revision: {error}"));
    assert!(clock_safe.selectable_for_new_rooms);
    assert!(clock_safe.runnable_for_retained_rooms);
    assert_eq!(clock_safe.descriptor.explanatory_version, "0.3.0");
    assert_ne!(clock_safe.revision_digest, agent_heist_lobby_digest());
    assert!(
        revisions
            .iter()
            .any(|revision| revision.revision_digest == agent_heist_lobby_digest())
    );
    assert!(
        revisions.windows(2).all(|pair| {
            pair[0].revision_digest.to_string() < pair[1].revision_digest.to_string()
        })
    );

    let retained = registry
        .catalog_revision(&counter_v1_digest())
        .unwrap_or_else(|error| unreachable!("retained revision: {error}"));
    assert!(!retained.selectable_for_new_rooms);
    assert!(retained.runnable_for_retained_rooms);

    let selectable = registry
        .catalog_revision(&counter_v2_digest())
        .unwrap_or_else(|error| unreachable!("selectable revision: {error}"));
    assert!(selectable.selectable_for_new_rooms);
    assert!(selectable.runnable_for_retained_rooms);
    assert_ne!(retained.revision_digest, selectable.revision_digest);

    let attention_retained = registry
        .catalog_revision(&counter_v3_digest())
        .unwrap_or_else(|error| unreachable!("attention revision: {error}"));
    assert!(!attention_retained.selectable_for_new_rooms);
    assert!(attention_retained.runnable_for_retained_rooms);
    assert_ne!(
        selectable.revision_digest,
        attention_retained.revision_digest
    );

    let attention_selectable = registry
        .catalog_revision(&counter_v4_digest())
        .unwrap_or_else(|error| unreachable!("target-qualified attention revision: {error}"));
    assert!(attention_selectable.selectable_for_new_rooms);
    assert!(attention_selectable.runnable_for_retained_rooms);
    assert_ne!(
        attention_retained.revision_digest,
        attention_selectable.revision_digest
    );

    let schema = registry
        .resolve_schema(
            &selectable.revision_digest,
            &selectable.descriptor.configuration_schema,
        )
        .unwrap_or_else(|error| unreachable!("configuration schema: {error}"));
    assert_eq!(
        schema.to_bytes().unwrap_or_else(|error| unreachable!("schema bytes: {error}")),
        br#"{"additionalProperties":false,"properties":{"initial_value":{"maximum":16,"minimum":0,"type":"integer"},"maximum_value":{"maximum":16,"minimum":1,"type":"integer"}},"required":["initial_value","maximum_value"],"type":"object"}"#
    );
}

#[test]
fn activity_pack_catalog_never_substitutes_an_unknown_revision() {
    let registry = builtin_worldstream_registry()
        .unwrap_or_else(|error| unreachable!("built-in registry: {error}"));
    let unknown = "blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        .parse()
        .unwrap_or_else(|error| unreachable!("digest: {error}"));

    assert!(matches!(
        registry.catalog_revision(&unknown),
        Err(PackRegistryErrorV1::MissingRevision(actual)) if actual == unknown
    ));
}
