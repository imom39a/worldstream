use worldstream_activity_client::ExactPackReferenceV1;
use worldstream_protocol::AccessMode;
use worldstream_studio_supervisor::{
    client_bindings::{
        ClientBindingStoreErrorV1, ClientBindingStoreV1, ClientDeploymentTrustPolicyV1,
        ClientSelectionRequestV1, ClientSelectionV1,
    },
    model_provider_credentials::ModelProviderCredentialRegistryV1,
    runner_templates::RunnerTemplateRegistryV1,
    secrets::FileSecretVaultV1,
};

#[test]
fn fresh_installed_catalogs_need_no_source_imports_or_invented_client_approval()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let runners = RunnerTemplateRegistryV1::open_installed(&directory.path().join("runners"))?;
    assert!(runners.templates().is_empty());
    let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))?;
    let providers = ModelProviderCredentialRegistryV1::open_installed(
        &directory.path().join("providers"),
        vault,
    )?;
    assert!(providers.catalog().credentials.is_empty());

    // Until reviewed prerequisite policy is persisted by IMO-142, preview
    // startup explicitly narrows selection to VerifiedOnly. No fallback is
    // invented and opening the registry does not confer any client approval.
    let clients = ClientBindingStoreV1::open_installed(
        &directory.path().join("clients"),
        ClientDeploymentTrustPolicyV1::VerifiedOnly,
    )?;
    let selection = clients.select(
        &ClientSelectionRequestV1 {
            pack: ExactPackReferenceV1 {
                id: "example.heist".to_owned(),
                version: "1.0.0".to_owned(),
                digest: format!("blake3:{}", "a".repeat(64)),
            },
            client_contract: "worldstream/activity-client-protocol/v1".to_owned(),
            access_mode: AccessMode::Participant,
            role: Some("navigator".to_owned()),
        },
        None,
    );
    assert_eq!(selection, Err(ClientBindingStoreErrorV1::Unavailable));
    assert_eq!(
        std::fs::read_dir(directory.path().join("clients/inspector-fallback"))?.count(),
        0
    );
    Ok(())
}

#[test]
fn reopening_client_approvals_preserves_records_and_never_widens_selection_policy()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("clients");
    let configuration =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/clients/catalog");
    // Explicit fixture import represents the separate reviewed prerequisite step,
    // not startup. These tracked declarations intentionally use external trust.
    let imported = ClientBindingStoreV1::open_configured(
        &root,
        &configuration.join("releases"),
        &configuration.join("local-bindings.json"),
    )?;
    let request = ClientSelectionRequestV1 {
        pack: ExactPackReferenceV1 {
            id: "example.unbound".to_owned(),
            version: "1.0.0".to_owned(),
            digest: format!("blake3:{}", "a".repeat(64)),
        },
        client_contract: "worldstream/activity-client-protocol/v1".to_owned(),
        access_mode: AccessMode::Participant,
        role: Some("navigator".to_owned()),
    };
    assert!(matches!(
        imported.select(&request, None)?,
        ClientSelectionV1::InspectorFallback { .. }
    ));
    let before = snapshot(&root)?;
    let narrowed =
        ClientBindingStoreV1::open_installed(&root, ClientDeploymentTrustPolicyV1::VerifiedOnly)?;
    assert_eq!(
        narrowed.select(&request, None),
        Err(ClientBindingStoreErrorV1::Unavailable)
    );
    assert_eq!(snapshot(&root)?, before);
    let reviewed = ClientBindingStoreV1::open_installed(
        &root,
        ClientDeploymentTrustPolicyV1::AllowExternallyTrusted,
    )?;
    assert!(matches!(
        reviewed.select(&request, None)?,
        ClientSelectionV1::InspectorFallback { .. }
    ));
    assert_eq!(snapshot(&root)?, before);

    imported.revoke_deployment("first-party-inspector-web-v2")?;
    imported.disable_binding("agent-heist-0-1-participant-web-v3")?;
    let revoked = snapshot(&root)?;
    assert_ne!(revoked, before);
    let restarted = ClientBindingStoreV1::open_installed(
        &root,
        ClientDeploymentTrustPolicyV1::AllowExternallyTrusted,
    )?;
    assert_eq!(
        restarted.select(&request, None),
        Err(ClientBindingStoreErrorV1::Unavailable)
    );
    assert_eq!(snapshot(&root)?, revoked);
    Ok(())
}

fn snapshot(
    root: &std::path::Path,
) -> Result<std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>, std::io::Error> {
    let mut files = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            files.extend(snapshot(&entry.path())?);
        } else {
            files.insert(entry.path(), std::fs::read(entry.path())?);
        }
    }
    Ok(files)
}

#[test]
fn installed_catalogs_reject_damaged_records_without_repair_or_source_import()
-> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write as _;
    let directory = tempfile::tempdir()?;
    let runners = directory.path().join("runners");
    let providers = directory.path().join("providers");
    let clients = directory.path().join("clients");
    let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))?;
    RunnerTemplateRegistryV1::open_installed(&runners)?;
    ModelProviderCredentialRegistryV1::open_installed(&providers, vault.clone())?;
    ClientBindingStoreV1::open_installed(&clients, ClientDeploymentTrustPolicyV1::VerifiedOnly)?;
    for file in [
        runners.join("invalid.json"),
        providers.join("invalid.json"),
        clients.join("bindings/invalid.json"),
    ] {
        let mut output = worldstream_runtime::create_owner_only_file(&file)?;
        output.write_all(b"{}")?;
        output.sync_all()?;
    }
    let before = snapshot(directory.path())?;
    assert!(RunnerTemplateRegistryV1::open_installed(&runners).is_err());
    assert!(ModelProviderCredentialRegistryV1::open_installed(&providers, vault).is_err());
    assert!(
        ClientBindingStoreV1::open_installed(&clients, ClientDeploymentTrustPolicyV1::VerifiedOnly)
            .is_err()
    );
    assert_eq!(snapshot(directory.path())?, before);
    Ok(())
}
