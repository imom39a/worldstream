//! Pack listing reads retained intent without re-verifying or creating inventory.

use std::{fs, path::Path};
use worldstream_pack_bundle::{PackBundleStoreV1, PackInstallStateV1};
use worldstream_server::operator_packs::{approve_pack, install_pack, set_pack_selectable};

#[test]
fn installed_pack_metadata_preserves_exact_identity_and_next_start_selection()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let absent = directory.path().join("absent");
    assert!(PackBundleStoreV1::read_inventory_metadata(&absent)?.is_empty());
    assert!(!absent.exists());
    let data = worldstream_runtime::prepare_data_directory(&directory.path().join("data"))?;
    let bundle = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/packs/negotiate/releases/0.1.0/worldstream-negotiate-9033a1aa10ca37c301660b7427d79c4e71d7af59006bc7d51edc4b470c8c2db5.wspack");
    approve_pack(
        &data,
        &bundle,
        "pack-list-test".into(),
        "2026-09-03T00:00:00Z".into(),
    )?;
    let installed = install_pack(&data, &bundle, "2026-09-03T00:00:01Z".into())?;
    set_pack_selectable(&data, &installed.bundle_digest, true)?;
    let root = data.join("activity-packs");
    let rows = PackBundleStoreV1::read_inventory_metadata(&root)?;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].bundle_digest.to_string(),
        "blake3:9033a1aa10ca37c301660b7427d79c4e71d7af59006bc7d51edc4b470c8c2db5"
    );
    assert_eq!(
        rows[0].revision_digest.to_string(),
        "blake3:a62585c88ffebe0b2222f5f93e17de1e9cbb003593eca4891225f75dca985589"
    );
    assert_eq!(rows[0].install_state, PackInstallStateV1::Selectable);
    // An unavailable original object must not turn this metadata read into an
    // offline verifier. The caller must label these records unverified.
    let objects = root.join("objects");
    let retained = root.join("objects-test-retained");
    fs::rename(&objects, &retained)?;
    let observed = PackBundleStoreV1::read_inventory_metadata(&root);
    fs::rename(&retained, &objects)?;
    assert_eq!(observed?, rows);
    Ok(())
}
