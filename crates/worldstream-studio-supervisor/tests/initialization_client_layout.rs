//! Recovery of interrupted creation of an otherwise empty client catalog layout.

use std::path::PathBuf;
use worldstream_runtime::{CliOverrides, prepare_data_directory};
use worldstream_studio_supervisor::{
    client_bindings::{ClientBindingStoreV1, ClientDeploymentTrustPolicyV1},
    initialization_imports::{InitializationImportRequest, apply_imports, preview_imports},
    local_initialization::{InitializationRequest, initialize_local},
};

#[test]
fn exact_client_retry_completes_an_interrupted_empty_catalog_layout()
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
    // Explicit tracked non-secret declarations are read-only; all state is owned
    // by the temporary installation below.
    let declaration = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/clients/catalog/cli-import.json");
    let mut request = InitializationImportRequest {
        installation,
        runner_templates: Vec::new(),
        provider_declarations: Vec::new(),
        agent_profiles: Vec::new(),
        client_declarations: vec![declaration],
        approval: None,
    };
    request.approval = Some(preview_imports(&request)?.digest);
    // A crash in layout creation precedes every record publication: one empty
    // protected child exists; the remaining children have not been created.
    let root = request.installation.state_dir.join("client-bindings");
    prepare_data_directory(&root)?;
    prepare_data_directory(&root.join("releases"))?;
    let applied = apply_imports(&request);
    assert!(
        applied.is_ok(),
        "exact retry must complete a provably empty partial layout"
    );
    let applied = applied?;
    assert_eq!(applied.created_client_declarations.len(), 1);
    assert!(!applied.services_started);
    let policy = ClientBindingStoreV1::installed_policy(&root)?;
    assert_eq!(
        policy,
        ClientDeploymentTrustPolicyV1::AllowExternallyTrusted
    );
    ClientBindingStoreV1::open_installed(&root, policy)?;
    Ok(())
}
