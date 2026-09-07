use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use worldstream_runtime::CliOverrides;
use worldstream_studio_supervisor::{
    initialization_imports::{InitializationImportRequest, apply_imports, preview_imports},
    local_initialization::{InitializationRequest, initialize_local},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn runner_import_rejects_distinct_identities_with_the_same_retained_filename() -> TestResult {
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
    fs::write(directory.path().join("never-executed.bin"), [0_u8])?;
    let mut runners = Vec::new();
    for (template, revision, instance) in [("a--b", "c", "one"), ("a", "b--c", "two")] {
        let path = directory.path().join(format!("{instance}.json"));
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "schema": "worldstream/runner-template/v1",
                "template_id": template,
                "revision": revision,
                "display_name": "Filename collision fixture",
                "executable": {"path": "never-executed.bin", "blake3": "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213"},
                "compatibility": [{"activity_pack_id": "worldstream.counter", "exact_revisions": ["4.0.0"]}],
                "capacity": {"maximum_concurrent_invocations": 1},
                "health": {"path": "/health", "timeout_ms": 1000, "stale_after_ms": 5000},
                "non_secret_environment": {}, "secret_environment": [],
                "instances": [{"instance_id": instance, "health_address": "127.0.0.1:9511"}]
            }))?,
        )?;
        runners.push(path);
    }
    let request = InitializationImportRequest {
        installation,
        runner_templates: runners,
        provider_declarations: Vec::new(),
        agent_profiles: Vec::new(),
        client_declarations: Vec::new(),
        approval: None,
    };
    let before = snapshot(directory.path())?;
    assert!(
        preview_imports(&request).is_err(),
        "review must reject colliding retained filenames before partial publication"
    );
    assert!(snapshot(directory.path())? == before);
    Ok(())
}

#[test]
fn provider_import_rejects_an_aggregate_that_exceeds_retained_catalog_capacity() -> TestResult {
    use std::io::Write as _;
    use worldstream_studio_supervisor::{
        model_provider_credentials::ModelProviderCredentialRegistryV1, secrets::FileSecretVaultV1,
    };
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
    let private = directory.path().join("private");
    worldstream_runtime::prepare_data_directory(&private)?;
    let mut source = worldstream_runtime::create_owner_only_file(&private.join("model-token"))?;
    source.write_all(b"owned-capacity-provider-canary")?;
    drop(source);
    let mut request = InitializationImportRequest {
        installation,
        runner_templates: Vec::new(),
        provider_declarations: Vec::new(),
        agent_profiles: Vec::new(),
        client_declarations: Vec::new(),
        approval: None,
    };
    // Populate the supported boundary through the same approved public import
    // path, in bounded batches; never fabricate retained records or vault IDs.
    for batch in 0..16 {
        request.provider_declarations.clear();
        for offset in 0..16 {
            let id = format!("model-{}", batch * 16 + offset);
            let path = directory.path().join(format!("{id}.json"));
            fs::write(
                &path,
                serde_json::to_vec(&serde_json::json!({
                    "schema": "worldstream/model-provider-credential-import/v1",
                    "credential_id": id,
                    "display_name": "Capacity fixture",
                    "provider": "open_ai_compatible",
                    "secret_file": "private/model-token"
                }))?,
            )?;
            request.provider_declarations.push(path);
        }
        request.approval = Some(preview_imports(&request)?.digest);
        assert_eq!(
            apply_imports(&request)?.created_provider_credentials.len(),
            16
        );
    }
    assert_eq!(
        apply_imports(&request)?.reused_provider_credentials.len(),
        16
    );
    let extra = directory.path().join("extra.json");
    fs::write(&extra, br#"{"schema":"worldstream/model-provider-credential-import/v1","credential_id":"model-extra","display_name":"Overflow fixture","provider":"open_ai_compatible","secret_file":"private/model-token"}"#)?;
    request.provider_declarations = vec![extra];
    request.approval = None;
    let before = snapshot(directory.path())?;
    assert!(
        preview_imports(&request).is_err(),
        "review must reject record 257 before any publication"
    );
    assert!(snapshot(directory.path())? == before);
    let vault = FileSecretVaultV1::open_existing(&request.installation.state_dir.join("secrets"))?;
    ModelProviderCredentialRegistryV1::open_installed(
        &request
            .installation
            .state_dir
            .join("model-provider-credentials/installed"),
        vault,
    )?;
    Ok(())
}

#[test]
fn preview_does_not_authenticate_or_hash_installation_credentials() -> TestResult {
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
    let profile = directory.path().join("external-profile.json");
    fs::write(
        &profile,
        include_bytes!("../../../config/initialization/external-agent.json"),
    )?;
    let mut request = InitializationImportRequest {
        installation,
        runner_templates: Vec::new(),
        provider_declarations: Vec::new(),
        agent_profiles: vec![profile],
        client_declarations: Vec::new(),
        approval: None,
    };
    let reviewed = preview_imports(&request)?;
    // Replace only owned fixture credential bytes, preserving protected paths
    // and lengths. Metadata review must not authenticate them or bind their bytes.
    fs::write(
        request.installation.state_dir.join("control-access.v1"),
        [0_u8; 48],
    )?;
    fs::write(
        directory.path().join(".worldstream/authority.secret"),
        [0_u8; 32],
    )?;
    let before = snapshot(directory.path())?;
    assert_eq!(preview_imports(&request)?.digest, reviewed.digest);
    assert!(snapshot(directory.path())? == before);
    request.approval = Some(reviewed.digest);
    assert!(
        apply_imports(&request).is_err(),
        "apply must authenticate retained authority before publishing"
    );
    assert!(snapshot(directory.path())? == before);
    Ok(())
}

#[test]
fn repeated_provider_import_never_rotates_a_named_secret() -> TestResult {
    use std::io::Write as _;
    use worldstream_studio_supervisor::{
        agent_profiles::ManagedReferenceProviderV1,
        model_provider_credentials::ModelProviderCredentialRegistryV1,
        secrets::{FileSecretVaultV1, SecretKindV1},
    };
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
    let private = directory.path().join("private");
    worldstream_runtime::prepare_data_directory(&private)?;
    let secret_path = private.join("model-token");
    let mut source = worldstream_runtime::create_owner_only_file(&secret_path)?;
    source.write_all(b"owned-original-provider-canary")?;
    drop(source);
    let provider = directory.path().join("provider.json");
    fs::write(&provider, br#"{"schema":"worldstream/model-provider-credential-import/v1","credential_id":"local-model","display_name":"Local model","provider":"open_ai_compatible","secret_file":"private/model-token"}"#)?;
    let mut request = InitializationImportRequest {
        installation,
        runner_templates: Vec::new(),
        provider_declarations: vec![provider],
        agent_profiles: Vec::new(),
        client_declarations: Vec::new(),
        approval: None,
    };
    request.approval = Some(preview_imports(&request)?.digest);
    let applied = apply_imports(&request)?;
    let vault = FileSecretVaultV1::open_existing(&applied.state_dir.join("secrets"))?;
    let registry = ModelProviderCredentialRegistryV1::open_installed(
        &applied
            .state_dir
            .join("model-provider-credentials/installed"),
        vault.clone(),
    )?;
    let reference =
        registry.resolve("local-model", ManagedReferenceProviderV1::OpenAiCompatible)?;
    fs::write(&secret_path, b"owned-replacement-provider-canary")?;
    // Review binds the explicit protected source, never a hash of token bytes.
    assert_eq!(Some(preview_imports(&request)?.digest), request.approval);
    let before = snapshot(directory.path())?;
    assert!(apply_imports(&request).is_err());
    assert!(snapshot(directory.path())? == before);
    assert!(
        vault
            .resolve(SecretKindV1::ModelProvider, &reference)?
            .as_bytes()
            == b"owned-original-provider-canary"
    );
    Ok(())
}

#[test]
fn replacing_a_retained_provider_dependency_invalidates_profile_approval() -> TestResult {
    use std::io::Write as _;
    use worldstream_studio_supervisor::secrets::{FileSecretVaultV1, SecretKindV1};
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
    fs::write(directory.path().join("never-executed.bin"), [0_u8])?;
    let runner = directory.path().join("runner.json");
    fs::write(&runner, br#"{"schema":"worldstream/runner-template/v1","template_id":"counter-reference","revision":"1","display_name":"Counter reference","executable":{"path":"never-executed.bin","blake3":"2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213"},"compatibility":[{"activity_pack_id":"worldstream.counter","exact_revisions":["4.0.0"]}],"capacity":{"maximum_concurrent_invocations":1},"health":{"path":"/health","timeout_ms":1000,"stale_after_ms":5000},"non_secret_environment":{},"secret_environment":[],"instances":[{"instance_id":"counter-local","health_address":"127.0.0.1:9511"}]}"#)?;
    let private = directory.path().join("private");
    worldstream_runtime::prepare_data_directory(&private)?;
    let mut source = worldstream_runtime::create_owner_only_file(&private.join("model-token"))?;
    source.write_all(b"owned-original-provider-canary")?;
    drop(source);
    let provider = directory.path().join("provider.json");
    fs::write(&provider, br#"{"schema":"worldstream/model-provider-credential-import/v1","credential_id":"local-model","display_name":"Local model","provider":"open_ai_compatible","secret_file":"private/model-token"}"#)?;
    let mut request = InitializationImportRequest {
        installation,
        runner_templates: vec![runner],
        provider_declarations: vec![provider],
        agent_profiles: Vec::new(),
        client_declarations: Vec::new(),
        approval: None,
    };
    request.approval = Some(preview_imports(&request)?.digest);
    apply_imports(&request)?;
    let profile = directory.path().join("managed-profile.json");
    fs::write(&profile, br#"{"schema":"worldstream/studio-agent-profile-publish/v2","profile_id":"counter-agent","revision":"1","display_name":"Managed Counter","non_secret_configuration":{},"host_contract":{"kind":"managed_reference","host_contract_revision":"v1","runner_template":{"template_id":"counter-reference","revision":"1"},"provider":"open_ai_compatible","provider_address":"127.0.0.1:11434","model_id":"local-model"},"managed_provider_credential_id":"local-model"}"#)?;
    request.runner_templates.clear();
    request.provider_declarations.clear();
    request.agent_profiles = vec![profile];
    request.approval = Some(preview_imports(&request)?.digest);
    let vault = FileSecretVaultV1::open_existing(&request.installation.state_dir.join("secrets"))?;
    let replacement = vault.store(
        SecretKindV1::ModelProvider,
        b"owned-replacement-provider-canary",
    )?;
    // Simulate restored/replaced retained metadata inside this owned fixture.
    // The production immutable registry API deliberately does not offer this mutation.
    fs::write(
        request
            .installation
            .state_dir
            .join("model-provider-credentials/installed/local-model.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema": "worldstream/model-provider-credential/v1",
            "credential_id": "local-model",
            "display_name": "Local model",
            "provider": "open_ai_compatible",
            "secret": {"kind": "model_provider", "reference": replacement.as_str()}
        }))?,
    )?;
    let before = snapshot(directory.path())?;
    assert!(
        apply_imports(&request).is_err(),
        "stale approval must not adopt replacement provider authority"
    );
    assert!(snapshot(directory.path())? == before);
    let new_review = preview_imports(&request)?;
    assert_ne!(
        request.approval.as_deref(),
        Some(new_review.digest.as_str())
    );
    let output = serde_json::to_string(&new_review)?;
    assert!(!output.contains(replacement.as_str()));
    assert!(!output.contains("owned-replacement-provider-canary"));
    Ok(())
}

#[test]
fn reviewed_client_declarations_install_exact_targets_and_reuse_selection_policy() -> TestResult {
    use worldstream_activity_client::ExactPackReferenceV1;
    use worldstream_protocol::AccessMode;
    use worldstream_studio_supervisor::client_bindings::{
        ClientBindingStoreV1, ClientDeploymentTrustPolicyV1, ClientSelectionRequestV1,
        ClientSelectionV1,
    };
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
    for (name, bytes) in [
        (
            "inspector.json",
            include_bytes!("../../../config/activity-clients/releases/inspector-web-v2.json")
                .as_slice(),
        ),
        (
            "heist.json",
            include_bytes!("../../../config/activity-clients/releases/agent-heist-web-v3.json")
                .as_slice(),
        ),
        (
            "negotiate.json",
            include_bytes!("../../../config/activity-clients/releases/negotiate-web.json")
                .as_slice(),
        ),
        (
            "negotiate-v3.json",
            include_bytes!("../../../config/activity-clients/releases/negotiate-web-v3.json")
                .as_slice(),
        ),
        (
            "bindings.json",
            include_bytes!("../../../config/activity-clients/local-bindings.json").as_slice(),
        ),
    ] {
        fs::write(directory.path().join(name), bytes)?;
    }
    let declaration = directory.path().join("clients.json");
    fs::write(&declaration, br#"{"schema":"worldstream/client-declaration-import/v1","release_files":["inspector.json","heist.json","negotiate.json","negotiate-v3.json"],"bindings_file":"bindings.json"}"#)?;
    let mut request = InitializationImportRequest {
        installation,
        runner_templates: Vec::new(),
        provider_declarations: Vec::new(),
        agent_profiles: Vec::new(),
        client_declarations: vec![declaration],
        approval: None,
    };
    let before = snapshot(directory.path())?;
    let review = preview_imports(&request)?;
    assert_eq!(review.client_declarations.len(), 1);
    assert_eq!(
        review.client_declarations[0].deployment_trust_policy,
        ClientDeploymentTrustPolicyV1::AllowExternallyTrusted
    );
    assert!(
        review.client_declarations[0]
            .deployments
            .iter()
            .any(
                |deployment| deployment.deployment_id == "first-party-negotiate-web-v3"
                    && deployment
                        .surfaces
                        .iter()
                        .any(|surface| surface.launch_url == "http://127.0.0.1:5173/negotiate-v3/")
            )
    );
    assert!(snapshot(directory.path())? == before);
    request.approval = Some(review.digest);
    let applied = apply_imports(&request)?;
    assert_eq!(applied.created_client_declarations.len(), 1);
    assert!(!applied.services_started);
    let root = applied.state_dir.join("client-bindings");
    let policy = ClientBindingStoreV1::installed_policy(&root)?;
    assert_eq!(
        policy,
        ClientDeploymentTrustPolicyV1::AllowExternallyTrusted
    );
    let store = ClientBindingStoreV1::open_installed(&root, policy)?;
    for (version, digest, binding, launch_url) in [
        (
            "0.1.0",
            "blake3:a62585c88ffebe0b2222f5f93e17de1e9cbb003593eca4891225f75dca985589",
            "negotiate-0-1-spectator-web",
            "http://127.0.0.1:5173/negotiate/",
        ),
        (
            "0.2.0",
            "blake3:651a04711a61bbdb263da5869a48d9829bc37b3be315042404587b00d52c127c",
            "negotiate-0-2-spectator-web-v3",
            "http://127.0.0.1:5173/negotiate-v3/",
        ),
    ] {
        let selected = store.select(
            &ClientSelectionRequestV1 {
                pack: ExactPackReferenceV1 {
                    id: "worldstream.negotiate".to_owned(),
                    version: version.to_owned(),
                    digest: digest.to_owned(),
                },
                client_contract: "worldstream/activity-client-protocol/v1".to_owned(),
                access_mode: AccessMode::Spectator,
                role: None,
            },
            None,
        )?;
        assert!(matches!(selected, ClientSelectionV1::Selected { candidate }
            if candidate.candidate_id == binding
                && candidate.launch_url == launch_url));
    }
    let after = snapshot(directory.path())?;
    assert_eq!(apply_imports(&request)?.reused_client_declarations.len(), 1);
    assert!(snapshot(directory.path())? == after);
    Ok(())
}

#[test]
fn managed_profile_resolves_template_and_provider_from_the_same_approved_batch() -> TestResult {
    use std::io::Write as _;
    use worldstream_studio_supervisor::{
        agent_profiles::{AgentHostContractV1, AgentProfileStoreV1, ManagedReferenceProviderV1},
        model_provider_credentials::ModelProviderCredentialRegistryV1,
        secrets::FileSecretVaultV1,
    };
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
    fs::write(directory.path().join("never-executed.bin"), [0_u8])?;
    let runner = directory.path().join("runner.json");
    fs::write(&runner, br#"{"schema":"worldstream/runner-template/v1","template_id":"counter-reference","revision":"1","display_name":"Counter reference","executable":{"path":"never-executed.bin","blake3":"2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213"},"compatibility":[{"activity_pack_id":"worldstream.counter","exact_revisions":["4.0.0"]}],"capacity":{"maximum_concurrent_invocations":1},"health":{"path":"/health","timeout_ms":1000,"stale_after_ms":5000},"non_secret_environment":{},"secret_environment":[],"instances":[{"instance_id":"counter-local","health_address":"127.0.0.1:9511"}]}"#)?;
    let private = directory.path().join("private");
    worldstream_runtime::prepare_data_directory(&private)?;
    let mut source = worldstream_runtime::create_owner_only_file(&private.join("model-token"))?;
    source.write_all(b"owned-managed-profile-provider-canary")?;
    drop(source);
    let provider = directory.path().join("provider.json");
    fs::write(&provider, br#"{"schema":"worldstream/model-provider-credential-import/v1","credential_id":"local-model","display_name":"Local model","provider":"open_ai_compatible","secret_file":"private/model-token"}"#)?;
    let profile = directory.path().join("managed-profile.json");
    fs::write(&profile, br#"{"schema":"worldstream/studio-agent-profile-publish/v2","profile_id":"counter-agent","revision":"1","display_name":"Managed Counter","non_secret_configuration":{},"host_contract":{"kind":"managed_reference","host_contract_revision":"v1","runner_template":{"template_id":"counter-reference","revision":"1"},"provider":"open_ai_compatible","provider_address":"127.0.0.1:11434","model_id":"local-model"},"managed_provider_credential_id":"local-model"}"#)?;
    let mut request = InitializationImportRequest {
        installation,
        runner_templates: vec![runner],
        provider_declarations: vec![provider],
        agent_profiles: vec![profile],
        client_declarations: Vec::new(),
        approval: None,
    };
    let before = snapshot(directory.path())?;
    let review = preview_imports(&request)?;
    assert_eq!(review.agent_profiles[0].profile_id, "counter-agent");
    assert!(snapshot(directory.path())? == before);
    request.approval = Some(review.digest);
    let applied = apply_imports(&request)?;
    assert_eq!(applied.created_agent_profiles.len(), 1);
    let vault = FileSecretVaultV1::open_existing(&applied.state_dir.join("secrets"))?;
    let credentials = ModelProviderCredentialRegistryV1::open_installed(
        &applied
            .state_dir
            .join("model-provider-credentials/installed"),
        vault.clone(),
    )?;
    let reference =
        credentials.resolve("local-model", ManagedReferenceProviderV1::OpenAiCompatible)?;
    assert!(!serde_json::to_string(&applied)?.contains(reference.as_str()));
    let profiles = AgentProfileStoreV1::open(&applied.state_dir.join("agent-profiles"), vault)?;
    let installed = profiles.revision("counter-agent", "1")?;
    assert!(
        matches!(installed.host_contract, AgentHostContractV1::ManagedReference { runner_template, .. }
        if runner_template.template_id == "counter-reference" && runner_template.revision == "1")
    );
    assert_eq!(installed.secret_settings.len(), 1);
    let after = snapshot(directory.path())?;
    assert_eq!(apply_imports(&request)?.reused_agent_profiles.len(), 1);
    assert!(snapshot(directory.path())? == after);
    Ok(())
}

#[test]
fn named_provider_import_reads_only_explicit_protected_input_and_reuses_authority() -> TestResult {
    use std::io::Write as _;
    use worldstream_studio_supervisor::{
        agent_profiles::ManagedReferenceProviderV1,
        model_provider_credentials::ModelProviderCredentialRegistryV1,
        secrets::{FileSecretVaultV1, SecretKindV1},
    };
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
    let private = directory.path().join("private");
    worldstream_runtime::prepare_data_directory(&private)?;
    let secret = b"owned-provider-token-canary";
    let mut file = worldstream_runtime::create_owner_only_file(&private.join("model-token"))?;
    file.write_all(secret)?;
    drop(file);
    let declaration = directory.path().join("provider.json");
    fs::write(&declaration, br#"{"schema":"worldstream/model-provider-credential-import/v1","credential_id":"local-model","display_name":"Local model","provider":"open_ai_compatible","secret_file":"private/model-token"}"#)?;
    let mut request = InitializationImportRequest {
        installation,
        runner_templates: Vec::new(),
        provider_declarations: vec![declaration],
        agent_profiles: Vec::new(),
        client_declarations: Vec::new(),
        approval: None,
    };
    let before = snapshot(directory.path())?;
    let review = preview_imports(&request)?;
    assert_eq!(review.provider_credentials.len(), 1);
    assert!(snapshot(directory.path())? == before);
    assert!(
        !serde_json::to_vec(&review)?
            .windows(secret.len())
            .any(|window| window == secret)
    );
    request.approval = Some(review.digest);
    let applied = apply_imports(&request)?;
    assert_eq!(applied.created_provider_credentials, ["local-model"]);
    assert!(
        !serde_json::to_vec(&applied)?
            .windows(secret.len())
            .any(|window| window == secret)
    );
    let vault = FileSecretVaultV1::open_existing(&applied.state_dir.join("secrets"))?;
    let registry = ModelProviderCredentialRegistryV1::open_installed(
        &applied
            .state_dir
            .join("model-provider-credentials/installed"),
        vault.clone(),
    )?;
    let reference =
        registry.resolve("local-model", ManagedReferenceProviderV1::OpenAiCompatible)?;
    assert!(
        vault
            .resolve(SecretKindV1::ModelProvider, &reference)?
            .as_bytes()
            == secret
    );
    let after = snapshot(directory.path())?;
    let reused = apply_imports(&request)?;
    assert_eq!(reused.reused_provider_credentials, ["local-model"]);
    assert!(snapshot(directory.path())? == after);
    assert!(!reused.services_started);
    Ok(())
}

#[test]
fn verified_runner_template_preview_and_apply_pins_exact_executable() -> TestResult {
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
    let executable = directory.path().join("never-executed.bin");
    // Official BLAKE3 test_vectors.json input_len=1: literal [0], first 32 output bytes.
    // https://github.com/BLAKE3-team/BLAKE3/blob/master/test_vectors/test_vectors.json
    let expected_digest = "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213";
    fs::write(&executable, [0_u8])?;
    let declaration = directory.path().join("runner.json");
    fs::write(
        &declaration,
        format!(
            r#"{{"schema":"worldstream/runner-template/v1","template_id":"counter-reference","revision":"1","display_name":"Counter reference","executable":{{"path":"never-executed.bin","blake3":"{expected_digest}"}},"compatibility":[{{"activity_pack_id":"worldstream.counter","exact_revisions":["4.0.0"]}}],"capacity":{{"maximum_concurrent_invocations":1}},"health":{{"path":"/health","timeout_ms":1000,"stale_after_ms":5000}},"non_secret_environment":{{}},"secret_environment":[],"instances":[{{"instance_id":"counter-local","health_address":"127.0.0.1:9511"}}]}}"#
        ),
    )?;
    let mut request = InitializationImportRequest {
        installation,
        runner_templates: vec![declaration],
        provider_declarations: Vec::new(),
        agent_profiles: Vec::new(),
        client_declarations: Vec::new(),
        approval: None,
    };
    let before = snapshot(directory.path())?;
    let review = preview_imports(&request)?;
    assert_eq!(review.runner_templates.len(), 1);
    assert_eq!(
        review.runner_templates[0].executable.path,
        fs::canonicalize(executable)?
    );
    assert_eq!(
        review.runner_templates[0].executable.blake3,
        expected_digest
    );
    assert!(snapshot(directory.path())? == before);
    request.approval = Some(review.digest);
    let applied = apply_imports(&request)?;
    assert_eq!(applied.created_runner_templates.len(), 1);
    assert!(!applied.services_started);
    let registry =
        worldstream_studio_supervisor::runner_templates::RunnerTemplateRegistryV1::open_installed(
            &applied.state_dir.join("runner-templates/installed"),
        )?;
    assert_eq!(registry.templates()[0].executable.blake3, expected_digest);
    let after = snapshot(directory.path())?;
    let reused = apply_imports(&request)?;
    assert_eq!(reused.reused_runner_templates.len(), 1);
    assert!(snapshot(directory.path())? == after);
    Ok(())
}

#[test]
fn exact_approval_publishes_one_external_profile_and_reuses_it() -> TestResult {
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
    let profile = directory.path().join("external-profile.json");
    fs::write(&profile, br#"{"schema":"worldstream/studio-agent-profile-publish/v2","profile_id":"external-agent","revision":"1","display_name":"External agent","non_secret_configuration":{},"host_contract":{"kind":"generic_mcp"}}"#)?;
    let mut request = InitializationImportRequest {
        installation,
        runner_templates: Vec::new(),
        provider_declarations: Vec::new(),
        agent_profiles: vec![profile],
        client_declarations: Vec::new(),
        approval: None,
    };
    let review = preview_imports(&request)?;
    let before = snapshot(directory.path())?;
    assert!(apply_imports(&request).is_err());
    assert!(snapshot(directory.path())? == before);
    request.approval = Some(review.digest.clone());
    let applied = apply_imports(&request)?;
    assert_eq!(applied.created_agent_profiles.len(), 1);
    assert!(applied.reused_agent_profiles.is_empty());
    assert!(!applied.services_started);
    let after = snapshot(directory.path())?;
    let repeat = apply_imports(&request)?;
    assert!(repeat.created_agent_profiles.is_empty());
    assert_eq!(repeat.reused_agent_profiles.len(), 1);
    assert!(snapshot(directory.path())? == after);
    let vault = worldstream_studio_supervisor::secrets::FileSecretVaultV1::open_existing(
        &review.state_dir.join("secrets"),
    )?;
    let profiles = worldstream_studio_supervisor::agent_profiles::AgentProfileStoreV1::open(
        &review.state_dir.join("agent-profiles"),
        vault,
    )?;
    let installed = profiles.revision("external-agent", "1")?;
    assert_eq!(installed.display_name, "External agent");
    assert!(installed.secret_settings.is_empty());
    Ok(())
}

fn snapshot(
    root: &Path,
) -> Result<BTreeMap<PathBuf, Option<blake3::Hash>>, Box<dyn std::error::Error>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            result.insert(entry.path(), None);
            result.extend(snapshot(&entry.path())?);
        } else {
            result.insert(entry.path(), Some(blake3::hash(&fs::read(entry.path())?)));
        }
    }
    Ok(result)
}

#[test]
fn external_profile_preview_is_nonmutating_and_stable() -> TestResult {
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
    let profile = directory.path().join("external-profile.json");
    fs::write(&profile, br#"{"schema":"worldstream/studio-agent-profile-publish/v2","profile_id":"external-agent","revision":"1","display_name":"External agent","non_secret_configuration":{},"host_contract":{"kind":"generic_mcp"}}"#)?;
    let request = InitializationImportRequest {
        installation,
        runner_templates: Vec::new(),
        provider_declarations: Vec::new(),
        agent_profiles: vec![profile],
        client_declarations: Vec::new(),
        approval: None,
    };
    let before = snapshot(directory.path())?;
    let first = preview_imports(&request)?;
    let second = preview_imports(&request)?;
    assert_eq!(first.schema, "worldstream/initialization-import-review/v1");
    assert_eq!(first.digest, second.digest);
    assert!(first.digest.starts_with("blake3:"));
    assert_eq!(first.digest.len(), 71);
    assert_eq!(first.agent_profiles.len(), 1);
    assert!(!first.services_started);
    assert!(snapshot(directory.path())? == before);
    Ok(())
}
