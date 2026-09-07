//! Exact retry of incomplete client publication without restoring lost trust state.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use worldstream_activity_client::ExactPackReferenceV1;
use worldstream_protocol::AccessMode;
use worldstream_runtime::CliOverrides;
use worldstream_studio_supervisor::{
    client_bindings::{ClientBindingStoreV1, ClientSelectionRequestV1, ClientSelectionV1},
    initialization_imports::{InitializationImportRequest, apply_imports, preview_imports},
    local_initialization::{InitializationRequest, initialize_local},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn exact_client_retry_completes_orphan_status_but_never_recreates_lost_status() -> TestResult {
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
    let declarations = directory.path().join("declarations");
    fs::create_dir_all(declarations.join("releases"))?;
    for (name, bytes) in [
        (
            "releases/inspector-web.json",
            include_bytes!("../../../config/activity-clients/releases/inspector-web.json")
                .as_slice(),
        ),
        (
            "releases/agent-heist-web-v2.json",
            include_bytes!("../../../config/activity-clients/releases/agent-heist-web-v2.json")
                .as_slice(),
        ),
        (
            "releases/negotiate-web-v2.json",
            include_bytes!("../../../config/activity-clients/releases/negotiate-web-v2.json")
                .as_slice(),
        ),
        (
            "releases/inspector-web-v2.json",
            include_bytes!("../../../config/activity-clients/releases/inspector-web-v2.json")
                .as_slice(),
        ),
        (
            "releases/agent-heist-web.json",
            include_bytes!("../../../config/activity-clients/releases/agent-heist-web.json")
                .as_slice(),
        ),
        (
            "releases/agent-heist-web-v3.json",
            include_bytes!("../../../config/activity-clients/releases/agent-heist-web-v3.json")
                .as_slice(),
        ),
        (
            "releases/negotiate-web.json",
            include_bytes!("../../../config/activity-clients/releases/negotiate-web.json")
                .as_slice(),
        ),
        (
            "releases/negotiate-web-v3.json",
            include_bytes!("../../../config/activity-clients/releases/negotiate-web-v3.json")
                .as_slice(),
        ),
        (
            "local-bindings.json",
            include_bytes!("../../../config/activity-clients/local-bindings.json").as_slice(),
        ),
        (
            "cli-import.json",
            include_bytes!("../../../config/activity-clients/cli-import.json").as_slice(),
        ),
    ] {
        fs::write(declarations.join(name), bytes)?;
    }
    let mut request = InitializationImportRequest {
        installation,
        runner_templates: Vec::new(),
        provider_declarations: Vec::new(),
        agent_profiles: Vec::new(),
        client_declarations: vec![declarations.join("cli-import.json")],
        approval: None,
    };
    request.approval = Some(preview_imports(&request)?.digest);
    let applied = apply_imports(&request)?;
    let root = applied.state_dir.join("client-bindings");
    let store = ClientBindingStoreV1::open_installed(
        &root,
        ClientBindingStoreV1::installed_policy(&root)?,
    )?;
    store.disable_binding("negotiate-0-1-spectator-web")?;
    store.revoke_deployment("first-party-agent-heist-web-v3")?;
    // Owned filesystem fault: publication stopped with a retained status but
    // without its immutable binding. Existing status must not be reset on retry.
    fs::remove_file(root.join("bindings/negotiate-0-1-spectator-web.json"))?;
    let recovered = apply_imports(&request);
    assert!(
        recovered.is_ok(),
        "exact orphan-status retry must complete publication"
    );
    let recovered = recovered?;
    assert!(!recovered.services_started);
    for (id, version, digest) in [
        (
            "worldstream.negotiate",
            "0.1.0",
            "blake3:a62585c88ffebe0b2222f5f93e17de1e9cbb003593eca4891225f75dca985589",
        ),
        (
            "worldstream.agent-heist",
            "0.2.0",
            "blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820",
        ),
    ] {
        let selection = store.select(
            &ClientSelectionRequestV1 {
                pack: ExactPackReferenceV1 {
                    id: id.to_owned(),
                    version: version.to_owned(),
                    digest: digest.to_owned(),
                },
                client_contract: "worldstream/activity-client-protocol/v1".to_owned(),
                access_mode: AccessMode::Spectator,
                role: None,
            },
            None,
        )?;
        assert!(matches!(
            selection,
            ClientSelectionV1::InspectorFallback { .. }
        ));
    }
    // Losing a status beside a retained identity is different: recreating
    // Approved could silently undo a previous disable, so this must fail closed.
    fs::remove_file(root.join("binding-status/negotiate-0-1-spectator-web.json"))?;
    let before = snapshot(directory.path())?;
    assert!(apply_imports(&request).is_err());
    assert!(snapshot(directory.path())? == before);
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
