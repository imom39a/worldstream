#![cfg(feature = "cli-operator-preview")]

use std::{collections::BTreeMap, fs, io::Write as _, path::Path};
use worldstream_studio_supervisor::{
    process_ownership::{ProcessOwnership, ProcessRole},
    retained_server_inspection::inspect_retained_server,
};

fn snapshot(root: &Path) -> std::io::Result<BTreeMap<String, Vec<u8>>> {
    fs::read_dir(root)?
        .map(|entry| {
            let entry = entry?;
            Ok((
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path())?,
            ))
        })
        .collect()
}

#[test]
fn unavailable_controller_preserves_retained_runtime_and_checkpoint_without_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let state = worldstream_runtime::prepare_data_directory(&temporary.path().join("state"))?;
    let ownership = ProcessOwnership::open(&state)?;
    ownership.reserve(ProcessRole::Controller)?;
    // Owned filesystem fault: this component is unavailable, not silently absent.
    fs::write(state.join("managed-controller.v1"), b"damaged")?;
    let runtime = ownership.reserve(ProcessRole::Runtime)?;
    let lease = ownership.claim(ProcessRole::Runtime, runtime.generation())?;
    let mut checkpoint =
        worldstream_runtime::create_owner_only_file(&state.join("managed-lifecycle.v1.json"))?;
    checkpoint.write_all(br#"{"schema":"worldstream/managed-lifecycle-operation/v1","operation_id":7,"action":"restart","stage":"runtime_restart","captured_running":[],"restore_remaining":[]}"#)?;
    drop(checkpoint);
    let before = snapshot(&state)?;
    let inspection = serde_json::to_value(inspect_retained_server(&state))?;
    assert_eq!(
        inspection["schema"],
        "worldstream/retained-server-inspection/v1"
    );
    assert_eq!(inspection["evidence"], "retained_only");
    assert_eq!(inspection["controller"]["availability"], "unavailable");
    assert_eq!(inspection["runtime"]["availability"], "available");
    assert_eq!(inspection["runtime"]["phase"], "starting");
    assert_eq!(inspection["runtime"]["lease"], "held");
    assert_eq!(inspection["runtime"]["generation"], runtime.generation());
    assert_eq!(inspection["operation"]["availability"], "available");
    assert_eq!(inspection["operation"]["checkpoint"]["operation_id"], 7);
    assert_eq!(
        inspection["operation"]["checkpoint"]["stage"],
        "runtime_restart"
    );
    assert!(snapshot(&state)? == before);
    drop(lease);
    Ok(())
}
