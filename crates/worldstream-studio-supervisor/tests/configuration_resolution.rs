use serde_json::{Value, json};
use worldstream_core::{agent_heist_lobby_digest, builtin_agent_heist_registry};
use worldstream_studio_supervisor::configuration_resolution::resolve_configuration;

#[test]
fn heist_setup_supplies_fixed_values_without_replacing_explicit_choices()
-> Result<(), Box<dyn std::error::Error>> {
    let registry = builtin_agent_heist_registry()?;
    let digest = agent_heist_lobby_digest();
    let revision = registry.catalog_revision(&digest)?;
    let schema = registry.resolve_schema(&digest, &revision.descriptor.configuration_schema)?;
    let schema: Value = serde_json::from_slice(&schema.to_bytes()?)?;
    let supplied = json!({"roles": ["navigator", "insider", "broker"]});
    let resolved = resolve_configuration(&schema, &supplied)?;
    assert_eq!(
        resolved,
        json!({
            "briefing_duration_seconds":30,
            "commitment_duration_seconds":30,
            "commitment_reminder_seconds_before_deadline":10,
            "maximum_open_offers_per_role":4,
            "maximum_plans":12,
            "negotiation_duration_seconds":90,
            "pack_id":"worldstream.agent-heist",
            "pack_schema":1,
            "result_duration_seconds":20,
            "roles":["navigator","insider","broker"]
        })
    );
    assert!(
        resolve_configuration(
            &schema,
            &json!({
                "roles": ["navigator", "insider", "broker"], "pack_schema": 2
            })
        )
        .is_err()
    );
    assert_eq!(
        supplied,
        json!({"roles": ["navigator", "insider", "broker"]})
    );
    Ok(())
}

#[test]
fn nested_defaults_fill_only_missing_values_and_preserve_editable_choices()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "title": {"type": "string", "default": "Untitled"},
            "options": {
                "type": "object", "default": {}, "additionalProperties": false,
                "properties": {"limit": {"type": "integer", "default": 10}},
                "required": ["limit"]
            }
        },
        "required": ["title", "options"]
    });
    assert_eq!(
        resolve_configuration(&schema, &json!({}))?,
        json!({"title": "Untitled", "options": {"limit": 10}})
    );
    assert_eq!(
        resolve_configuration(
            &schema,
            &json!({
                "title": "Reviewed choice", "options": {"limit": 0}
            })
        )?,
        json!({"title": "Reviewed choice", "options": {"limit": 0}})
    );
    assert!(resolve_configuration(&schema, &json!({"title": null})).is_err());
    Ok(())
}

#[test]
fn validation_rejects_incomplete_unknown_or_unsupported_configuration_contracts()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = json!({
        "type": "object", "additionalProperties": false,
        "properties": {"choice": {"type": "string", "enum": ["first", "second"]}},
        "required": ["choice"]
    });
    assert_eq!(
        resolve_configuration(&schema, &json!({"choice": "second"}))?,
        json!({"choice": "second"})
    );
    for supplied in [
        json!({}),
        json!({"choice": "first", "unexpected": true}),
        json!({"choice": "third"}),
    ] {
        assert!(
            resolve_configuration(&schema, &supplied).is_err(),
            "unreviewed configuration must fail before any setup mutation"
        );
    }
    let unsupported = json!({
        "type": "object", "properties": {"choice": {"type": "string"}},
        "allOf": [{"required": ["other"]}]
    });
    assert!(resolve_configuration(&unsupported, &json!({"choice": "first"})).is_err());
    Ok(())
}

#[test]
fn declared_numeric_and_character_bounds_apply_to_supplied_and_default_values()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "count": {"type": "integer", "minimum": 1, "maximum": 3, "default": 2},
            "label": {"type": "string", "minLength": 1, "maxLength": 2}
        },
        "required": ["count", "label"]
    });
    assert_eq!(
        resolve_configuration(&schema, &json!({"label": "é界"}))?,
        json!({"count": 2, "label": "é界"})
    );
    for supplied in [
        json!({"count": 0, "label": "a"}),
        json!({"count": 4, "label": "a"}),
        json!({"label": ""}),
        json!({"label": "abc"}),
    ] {
        assert!(resolve_configuration(&schema, &supplied).is_err());
    }
    let invalid_default = json!({"type": "object", "properties": {
        "count": {"type": "integer", "minimum": 1, "default": 0}
    }});
    assert!(resolve_configuration(&invalid_default, &json!({})).is_err());
    Ok(())
}

#[test]
fn validation_diagnostics_identify_declared_fields_without_echoing_untrusted_input()
-> Result<(), Box<dyn std::error::Error>> {
    let schema = json!({"type": "object", "additionalProperties": false,
        "properties": {"choice": {"type": "string"}}, "required": ["choice"]});
    let missing = resolve_configuration(&schema, &json!({}))
        .err()
        .ok_or("required field accepted")?;
    assert_eq!(
        serde_json::to_value(&missing)?,
        json!({"path": "/configuration/choice", "code": "required"})
    );
    let unknown = resolve_configuration(
        &schema,
        &json!({"choice": "valid",
        "SECRET_IN_UNKNOWN_KEY": "SECRET_IN_UNKNOWN_VALUE"}),
    )
    .err()
    .ok_or("unknown field accepted")?;
    assert_eq!(
        serde_json::to_value(&unknown)?,
        json!({"path": "/configuration", "code": "additional_properties"})
    );
    assert!(!unknown.to_string().contains("SECRET"));
    Ok(())
}
