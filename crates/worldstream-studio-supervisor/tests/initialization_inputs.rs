use worldstream_studio_supervisor::initialization_inputs::{
    parse_agent_profile, parse_client_declaration, parse_provider_declaration,
    parse_runner_template,
};

const RUNNER: &[u8] = include_bytes!("fixtures/initialization-inputs/runner.json");
const PROVIDER: &[u8] = include_bytes!("fixtures/initialization-inputs/provider.json");
const PROFILE: &[u8] = include_bytes!("fixtures/initialization-inputs/profile.json");
const CLIENT: &[u8] = include_bytes!("fixtures/initialization-inputs/client.json");
type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn profile_host_variants_reject_nested_unknown_fields_and_keep_named_credentials() -> TestResult {
    let managed: serde_json::Value = serde_json::from_slice(PROFILE)?;
    let mut generic = managed.clone();
    generic["host_contract"] = serde_json::json!({"kind": "generic_mcp"});
    generic["managed_provider_credential_id"] = serde_json::Value::Null;
    assert!(parse_agent_profile(&serde_json::to_vec(&generic)?).is_ok());
    generic["host_contract"]["secret"] = serde_json::json!("private-canary");
    let error = parse_agent_profile(&serde_json::to_vec(&generic)?)
        .err()
        .ok_or("generic host accepted an inline secret")?;
    assert_eq!(error.to_string(), "initialization declaration is invalid");
    for pointer in ["/host_contract", "/host_contract/runner_template"] {
        let mut invalid = managed.clone();
        invalid
            .pointer_mut(pointer)
            .ok_or("fixture object missing")?["secret"] = serde_json::json!("private-canary");
        assert!(parse_agent_profile(&serde_json::to_vec(&invalid)?).is_err());
    }
    let mut missing = managed;
    missing["managed_provider_credential_id"] = serde_json::Value::Null;
    assert!(parse_agent_profile(&serde_json::to_vec(&missing)?).is_err());
    Ok(())
}

fn accepts(kind: usize, bytes: &[u8]) -> bool {
    match kind {
        0 => parse_runner_template(bytes).is_ok(),
        1 => parse_provider_declaration(bytes).is_ok(),
        2 => parse_agent_profile(bytes).is_ok(),
        3 => parse_client_declaration(bytes).is_ok(),
        _ => false,
    }
}

#[test]
fn each_import_flag_accepts_only_its_own_versioned_declaration() {
    for (kind, fixture) in [RUNNER, PROVIDER, PROFILE, CLIENT].into_iter().enumerate() {
        for parser in 0..4 {
            assert_eq!(accepts(parser, fixture), parser == kind);
        }
    }
}

#[test]
fn closed_inputs_reject_wrong_schema_unknown_fields_inline_secrets_and_oversize() -> TestResult {
    for (kind, fixture) in [RUNNER, PROVIDER, PROFILE, CLIENT].into_iter().enumerate() {
        let value: serde_json::Value = serde_json::from_slice(fixture)?;
        for (field, replacement) in [
            ("schema", serde_json::json!("wrong/v1")),
            ("unexpected", serde_json::json!(true)),
            ("secret", serde_json::json!("do-not-expose")),
        ] {
            let mut invalid = value.clone();
            invalid[field] = replacement;
            assert!(!accepts(kind, &serde_json::to_vec(&invalid)?));
        }
        for malformed in [b"".as_slice(), b"null", b"[]", b"{", b"{}"] {
            assert!(!accepts(kind, malformed));
        }
        let maximum = if kind == 3 { 256 * 1024 } else { 64 * 1024 };
        let mut oversized = fixture.to_vec();
        oversized.resize(maximum + 1, b' ');
        assert!(!accepts(kind, &oversized));
    }
    Ok(())
}

#[test]
fn nested_runner_unknown_fields_and_vault_references_are_rejected() -> TestResult {
    for field in ["executable", "capacity", "health"] {
        let mut value: serde_json::Value = serde_json::from_slice(RUNNER)?;
        value[field]["secret"] = serde_json::json!("do-not-expose");
        assert!(parse_runner_template(&serde_json::to_vec(&value)?).is_err());
    }
    for field in ["compatibility", "instances"] {
        let mut value: serde_json::Value = serde_json::from_slice(RUNNER)?;
        value[field][0]["extra"] = serde_json::json!(true);
        assert!(parse_runner_template(&serde_json::to_vec(&value)?).is_err());
    }
    let mut value: serde_json::Value = serde_json::from_slice(RUNNER)?;
    value["secret_environment"] = serde_json::json!([{"key":"TOKEN", "kind":"runner", "reference":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]);
    assert!(parse_runner_template(&serde_json::to_vec(&value)?).is_err());
    Ok(())
}

#[test]
fn paths_and_client_expansion_are_bounded_and_never_opened() -> TestResult {
    for path in ["".to_owned(), "a\nb".to_owned(), "x".repeat(4097)] {
        let mut value: serde_json::Value = serde_json::from_slice(PROVIDER)?;
        value["secret_file"] = serde_json::json!(path);
        assert!(parse_provider_declaration(&serde_json::to_vec(&value)?).is_err());
    }
    for count in [0, 17] {
        let mut value: serde_json::Value = serde_json::from_slice(CLIENT)?;
        value["release_files"] = serde_json::json!(vec!["release.json"; count]);
        assert!(parse_client_declaration(&serde_json::to_vec(&value)?).is_err());
    }
    Ok(())
}

#[test]
fn provider_import_names_a_file_without_reading_secret_bytes() -> TestResult {
    let input = parse_provider_declaration(include_bytes!(
        "fixtures/initialization-inputs/provider.json"
    ))?;
    assert_eq!(input.credential_id, "local-openai");
    assert_eq!(input.secret_file.to_str(), Some("secrets/provider-token"));
    Ok(())
}
