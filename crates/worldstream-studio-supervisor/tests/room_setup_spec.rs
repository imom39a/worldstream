use serde_json::{Value, json};
use worldstream_core::{agent_heist_lobby_digest, builtin_agent_heist_registry};
use worldstream_protocol::ActivityPackCatalogRevisionResponse;
use worldstream_studio_supervisor::room_setup_spec::{
    generate_setup_example, resolve_setup_specification,
};

fn heist_catalog() -> Result<ActivityPackCatalogRevisionResponse, Box<dyn std::error::Error>> {
    let registry = builtin_agent_heist_registry()?;
    let digest = agent_heist_lobby_digest();
    let revision = registry.catalog_revision(&digest)?;
    let reference = &revision.descriptor.configuration_schema;
    let schema: Value =
        serde_json::from_slice(&registry.resolve_schema(&digest, reference)?.to_bytes()?)?;
    Ok(serde_json::from_value(json!({
        "version": "activity_pack_catalog.v1", "revision": {
            "summary": {"pack": {"id": "worldstream.agent-heist", "version": "0.2.0", "digest": digest.to_string()},
                "name": "Agent Heist", "selectable_for_new_rooms": true, "runnable_for_retained_rooms": true},
            "roles": revision.descriptor.roles.iter().map(|role| json!({"role": role.role,
                "minimum": role.minimum, "maximum": role.maximum})).collect::<Vec<_>>(),
            "configuration_schema": {"schema_id": reference.schema_id,
                "schema_digest": reference.schema_digest.to_string(), "schema": schema},
            "actions": []
        }
    }))?)
}

fn negotiate_catalog() -> Result<ActivityPackCatalogRevisionResponse, Box<dyn std::error::Error>> {
    use worldstream_core::Blake3DigestV1;
    use worldstream_pack_bundle::PackBundleVerifierV1;
    let bytes = include_bytes!(
        "../../../packs/negotiate/releases/0.2.0/worldstream-negotiate-83453ea9641f8b16e9b96bf536c5ee932611611817458f130d8b77c7b93ff9a8.wspack"
    );
    let bundle = PackBundleVerifierV1.inspect(std::sync::Arc::from(bytes.as_slice()))?;
    let descriptor = bundle.descriptor();
    let reference = &descriptor.configuration_schema;
    // Bind this literal schema to the exact document in the verified retained bundle.
    assert_eq!(
        reference.schema_digest,
        Blake3DigestV1::hash(br#"{"type":"object"}"#)
    );
    Ok(serde_json::from_value(
        json!({"version": "activity_pack_catalog.v1", "revision": {
            "summary": {"pack": {"id": descriptor.pack_id, "version": descriptor.explanatory_version,
                "digest": bundle.revision_digest().to_string()}, "name": descriptor.name,
                "selectable_for_new_rooms": true, "runnable_for_retained_rooms": true},
            "roles": descriptor.roles.iter().map(|role| json!({"role": role.role,
                "minimum": role.minimum, "maximum": role.maximum})).collect::<Vec<_>>(),
            "configuration_schema": {"schema_id": reference.schema_id,
                "schema_digest": reference.schema_digest.to_string(), "schema": {"type": "object"}},
            "actions": []
        }}),
    )?)
}

fn setup_input(catalog: &ActivityPackCatalogRevisionResponse) -> Value {
    json!({
        "schema": "worldstream/room-setup/v1", "pack": catalog.revision.summary.pack,
        "configuration": {"roles": ["navigator", "insider", "broker"]},
        "seats": [
            {"label": "navigator", "role": "navigator", "required": true, "display_name": "Navigator",
                "principal": {"reference": "alice", "kind": "human"}},
            {"label": "insider", "role": "insider", "required": true, "display_name": "Insider",
                "principal": {"reference": "bob", "kind": "human"}},
            {"label": "broker", "role": "broker", "required": false, "display_name": "Broker"}
        ]
    })
}

#[test]
fn reusable_setup_resolves_exact_pack_values_and_keeps_public_seat_intent()
-> Result<(), Box<dyn std::error::Error>> {
    let catalog = heist_catalog()?;
    let bytes = serde_json::to_vec(&setup_input(&catalog))?;
    let resolved = resolve_setup_specification(&bytes, &catalog)?;
    let output = serde_json::to_value(&resolved)?;
    assert_eq!(
        output["configuration"]["pack_id"],
        "worldstream.agent-heist"
    );
    assert_eq!(output["configuration"]["maximum_plans"], 12);
    assert_eq!(
        output["seats"][0]["principal"],
        json!({"reference": "alice", "kind": "human"})
    );
    assert!(output["seats"][2].get("principal").is_none());
    assert_eq!(output["operator_view"], false);
    assert!(output.get("draft_id").is_none());
    assert!(output.get("readiness").is_none());
    Ok(())
}

#[test]
fn setup_rejects_ambiguous_principals_and_incompatible_seat_assignment_intent()
-> Result<(), Box<dyn std::error::Error>> {
    let catalog = heist_catalog()?;
    for invalid in [
        ("/seats/1/principal/reference", json!("alice")),
        ("/seats/0/principal", Value::Null),
        ("/seats/2/assignment", json!({"mode": "external"})),
        ("/seats/0/assignment", json!({"mode": "external"})),
        ("/seats/0/principal/kind", json!("agent")),
        ("/seats/0/role", json!("unlisted_role")),
        ("/seats/1/label", json!("navigator")),
        ("/seats/1/label", json!("Insider_Agent")),
    ] {
        let mut input = setup_input(&catalog);
        let (parent, key) = invalid.0.rsplit_once('/').ok_or("fixture pointer")?;
        input
            .pointer_mut(parent)
            .and_then(Value::as_object_mut)
            .ok_or("fixture object")?
            .insert(key.to_owned(), invalid.1);
        assert!(resolve_setup_specification(&serde_json::to_vec(&input)?, &catalog).is_err());
    }
    let mut agent = setup_input(&catalog);
    agent["seats"][0]["principal"]["kind"] = json!("agent");
    agent["seats"][0]["assignment"] = json!({"mode": "external"});
    assert!(resolve_setup_specification(&serde_json::to_vec(&agent)?, &catalog).is_ok());
    agent["seats"][0]["assignment"] = json!({"mode": "managed"});
    assert!(resolve_setup_specification(&serde_json::to_vec(&agent)?, &catalog).is_err());
    Ok(())
}

#[test]
fn managed_agent_setup_requires_an_explicit_profile_but_external_agents_do_not()
-> Result<(), Box<dyn std::error::Error>> {
    let catalog = heist_catalog()?;
    let mut input = setup_input(&catalog);
    input["seats"][0]["principal"]["kind"] = json!("agent");
    input["seats"][0]["assignment"] = json!({"mode": "external"});
    assert!(resolve_setup_specification(&serde_json::to_vec(&input)?, &catalog).is_ok());
    input["seats"][0]["assignment"] = json!({
        "mode": "managed",
        "runner_template": {"template_id": "approved-runner", "revision": "v1"}
    });
    let error = resolve_setup_specification(&serde_json::to_vec(&input)?, &catalog)
        .err()
        .ok_or("managed Agent without an explicit profile was accepted")?;
    assert_eq!(
        serde_json::to_value(error)?,
        json!({
            "path": "/seats/0/assignment/agent_profile", "code": "required"
        })
    );
    Ok(())
}

#[test]
fn role_cardinality_counts_declared_and_required_seats_not_only_filled_seats()
-> Result<(), Box<dyn std::error::Error>> {
    let catalog = heist_catalog()?;
    let mut optional_required_role = setup_input(&catalog);
    optional_required_role["seats"][0]["required"] = json!(false);
    assert!(
        resolve_setup_specification(&serde_json::to_vec(&optional_required_role)?, &catalog)
            .is_err()
    );

    let mut too_many_optional = setup_input(&catalog);
    too_many_optional["seats"].as_array_mut().ok_or("fixture seats")?.push(json!({
        "label": "extra-broker", "role": "broker", "required": false, "display_name": "Extra broker"
    }));
    assert!(
        resolve_setup_specification(&serde_json::to_vec(&too_many_optional)?, &catalog).is_err()
    );
    Ok(())
}

#[test]
fn public_setup_rejects_unbounded_references_executable_expressions_and_credentials()
-> Result<(), Box<dyn std::error::Error>> {
    let catalog = heist_catalog()?;
    for value in [
        "${USER}".to_owned(),
        String::new(),
        "a".repeat(129),
        "alice/other".to_owned(),
    ] {
        let mut input = setup_input(&catalog);
        input["seats"][0]["principal"]["reference"] = json!(value);
        assert!(resolve_setup_specification(&serde_json::to_vec(&input)?, &catalog).is_err());
    }
    let mut credential = setup_input(&catalog);
    credential["seats"][0]["display_name"] = json!("Bearer NEVER_PRINT_THIS_SECRET");
    let error = resolve_setup_specification(&serde_json::to_vec(&credential)?, &catalog)
        .err()
        .ok_or("credential accepted")?;
    assert!(!serde_json::to_string(&error)?.contains("NEVER_PRINT"));

    let mut permissive = catalog.clone();
    permissive.revision.configuration_schema.schema = json!({"type": "object"});
    let mut credential = setup_input(&permissive);
    credential["configuration"] = json!({"api_key": "NEVER_PRINT_THIS_SECRET"});
    assert!(resolve_setup_specification(&serde_json::to_vec(&credential)?, &permissive).is_err());
    let ordinary = serde_json::to_string(&setup_input(&catalog))?;
    let duplicate = ordinary.replace("\"roles\":", "\"roles\":[\"navigator\"],\"roles\":");
    assert!(resolve_setup_specification(duplicate.as_bytes(), &catalog).is_err());
    Ok(())
}

#[test]
fn reviewed_examples_resolve_through_the_same_exact_catalog_path()
-> Result<(), Box<dyn std::error::Error>> {
    let heist = generate_setup_example(&heist_catalog()?)?;
    assert_eq!(heist.configuration["maximum_plans"], 12);
    assert_eq!(heist.seats.len(), 3);
    let negotiate = generate_setup_example(&negotiate_catalog()?)?;
    assert_eq!(
        negotiate.configuration["formation_deadline"],
        4_102_444_800_u64
    );
    assert_eq!(negotiate.seats.len(), 4);
    assert!(!heist.operator_view && !negotiate.operator_view);
    Ok(())
}
