use std::{
    fs,
    io::Write as _,
    path::Path,
    sync::{Arc, Barrier},
    thread,
};

use tempfile::{TempDir, tempdir};
use worldstream_runtime::{SecretSource, StorageProfile, create_owner_only_file};
use worldstream_studio_supervisor::{
    secrets::{FileSecretVaultV1, SecretKindV1},
    startup_authority::{
        bootstrap_source_for_local_development, establish_host_authority_reference,
    },
};

const AUTHORITY: [u8; 32] = [0xa7; 32];

fn test_directory() -> TempDir {
    let directory = tempdir().unwrap_or_else(|error| panic!("temporary state: {error}"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("secure temporary root: {error}"));
    }
    directory
}

fn bootstrap_source_at(root: &Path, name: &str, bytes: &[u8]) -> SecretSource {
    let path = root.join(name);
    let mut file = create_owner_only_file(&path)
        .unwrap_or_else(|error| panic!("create bootstrap secret: {error}"));
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .unwrap_or_else(|error| panic!("write bootstrap secret: {error}"));
    SecretSource::File(path)
}

fn bootstrap_source(root: &Path, bytes: &[u8]) -> SecretSource {
    bootstrap_source_at(root, "authority.secret", bytes)
}

#[test]
fn fresh_local_development_creates_an_owner_only_bootstrap_source_before_importing_it() {
    let directory = test_directory();
    let state = directory.path().join("studio");
    let data = directory.path().join("data");
    let source_path = directory.path().join("authority.secret");
    let configured = SecretSource::File(source_path.clone());

    let source = bootstrap_source_for_local_development(
        &state,
        &data,
        StorageProfile::SqliteBundled,
        Some(&configured),
    )
    .unwrap_or_else(|error| panic!("prepare fresh bootstrap source: {error}"));
    let vault = FileSecretVaultV1::open(&state.join("secrets"))
        .unwrap_or_else(|error| panic!("open vault: {error}"));
    let reference = establish_host_authority_reference(&state, &vault, Some(&source), None)
        .unwrap_or_else(|error| panic!("establish imported authority: {error}"));

    assert_eq!(
        vault
            .resolve(SecretKindV1::HostAuthority, &reference)
            .unwrap_or_else(|error| panic!("resolve imported authority: {error}"))
            .as_bytes()
            .len(),
        32
    );
    assert!(
        !fs::read(source_path)
            .unwrap_or_else(|error| panic!("read generated source: {error}"))
            .windows(b"Bearer ".len())
            .any(|window| window == b"Bearer ")
    );
}

#[test]
fn interrupted_temporary_bootstrap_publication_recovers_on_the_next_fresh_start() {
    let directory = test_directory();
    let state = directory.path().join("studio");
    let data = directory.path().join("data");
    let source_path = directory.path().join("authority.secret");
    let configured = SecretSource::File(source_path.clone());
    let temporary = directory
        .path()
        .join(".authority-bootstrap-interrupted.tmp");
    let mut file = create_owner_only_file(&temporary)
        .unwrap_or_else(|error| panic!("create interrupted temporary source: {error}"));
    file.write_all(b"partial")
        .and_then(|()| file.sync_all())
        .unwrap_or_else(|error| panic!("write interrupted temporary source: {error}"));

    let first = bootstrap_source_for_local_development(
        &state,
        &data,
        StorageProfile::SqliteBundled,
        Some(&configured),
    )
    .unwrap_or_else(|error| panic!("recover bootstrap publication: {error}"));
    let first_material = first
        .read_exact_256()
        .unwrap_or_else(|error| panic!("read recovered bootstrap source: {error}"));
    let repeat = bootstrap_source_for_local_development(
        &state,
        &data,
        StorageProfile::SqliteBundled,
        Some(&configured),
    )
    .unwrap_or_else(|error| panic!("repeat bootstrap publication: {error}"));

    assert_eq!(
        repeat
            .read_exact_256()
            .unwrap_or_else(|error| panic!("read repeated bootstrap source: {error}")),
        first_material
    );
    assert!(temporary.is_file());
}

#[test]
fn concurrent_fresh_bootstrap_initializers_reuse_one_published_source() {
    let directory = test_directory();
    let source_path = directory.path().join("authority.secret");
    let barrier = Arc::new(Barrier::new(2));
    let mut workers = Vec::new();

    for worker in 0..2 {
        let barrier = Arc::clone(&barrier);
        let configured = SecretSource::File(source_path.clone());
        let state = directory.path().join(format!("studio-{worker}"));
        let data = directory.path().join(format!("data-{worker}"));
        workers.push(thread::spawn(move || {
            barrier.wait();
            bootstrap_source_for_local_development(
                &state,
                &data,
                StorageProfile::SqliteBundled,
                Some(&configured),
            )
        }));
    }

    let first = workers
        .remove(0)
        .join()
        .unwrap_or_else(|_| panic!("first initializer panicked"))
        .unwrap_or_else(|error| panic!("first initializer: {error}"));
    let second = workers
        .remove(0)
        .join()
        .unwrap_or_else(|_| panic!("second initializer panicked"))
        .unwrap_or_else(|error| panic!("second initializer: {error}"));

    assert_eq!(first, second);
    assert_eq!(
        fs::metadata(source_path)
            .unwrap_or_else(|error| panic!("published bootstrap metadata: {error}"))
            .len(),
        32
    );
}

#[test]
fn missing_bootstrap_source_with_existing_local_state_never_rotates_authority() {
    let directory = test_directory();
    let state = directory.path().join("studio");
    fs::create_dir_all(&state).unwrap_or_else(|error| panic!("create state: {error}"));
    fs::write(state.join("existing-state"), b"retained")
        .unwrap_or_else(|error| panic!("write state marker: {error}"));
    let configured = SecretSource::File(directory.path().join("missing.secret"));

    let error = bootstrap_source_for_local_development(
        &state,
        &directory.path().join("data"),
        StorageProfile::SqliteBundled,
        Some(&configured),
    )
    .expect_err("existing state with a missing source must fail closed");

    assert!(error.to_string().contains("bootstrap authority"));
    assert!(!error.to_string().contains("missing.secret"));
    assert!(!error.to_string().contains("delete"));
}

#[cfg(unix)]
#[test]
fn inaccessible_existing_local_state_never_allows_bootstrap_generation() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = test_directory();
    let state = directory.path().join("studio");
    fs::create_dir(&state).unwrap_or_else(|error| panic!("create retained state: {error}"));
    fs::set_permissions(&state, fs::Permissions::from_mode(0o000))
        .unwrap_or_else(|error| panic!("make retained state inaccessible: {error}"));
    let configured = SecretSource::File(directory.path().join("missing.secret"));

    let result = bootstrap_source_for_local_development(
        &state,
        &directory.path().join("data"),
        StorageProfile::SqliteBundled,
        Some(&configured),
    );
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| panic!("restore retained state permissions: {error}"));

    let error = result.expect_err("inaccessible retained state must fail closed");
    assert!(error.to_string().contains("bootstrap authority"));
}

#[test]
fn missing_postgres_bootstrap_source_requires_an_owner_provided_secret() {
    let directory = test_directory();
    let configured = SecretSource::File(directory.path().join("missing.secret"));

    let error = bootstrap_source_for_local_development(
        &directory.path().join("studio"),
        &directory.path().join("data"),
        StorageProfile::PostgresPrimary,
        Some(&configured),
    )
    .expect_err("postgres startup must not create host authority material");

    assert!(error.to_string().contains("bootstrap authority"));
}

#[test]
fn first_startup_imports_the_configured_host_authority_behind_one_opaque_reference() {
    let directory = test_directory();
    let state = directory.path().join("studio");
    let vault = FileSecretVaultV1::open(&state.join("secrets"))
        .unwrap_or_else(|error| panic!("open vault: {error}"));

    let reference = establish_host_authority_reference(
        &state,
        &vault,
        Some(&bootstrap_source(directory.path(), &AUTHORITY)),
        None,
    )
    .unwrap_or_else(|error| panic!("establish host authority: {error}"));

    assert_eq!(
        vault
            .resolve(SecretKindV1::HostAuthority, &reference)
            .unwrap_or_else(|error| panic!("resolve imported authority: {error}"))
            .as_bytes(),
        AUTHORITY
    );
    let binding = fs::read(state.join("host-authority-reference.json"))
        .unwrap_or_else(|error| panic!("read retained binding: {error}"));
    assert!(
        !binding
            .windows(b"\xa7\xa7".len())
            .any(|window| window == b"\xa7\xa7")
    );
    assert!(
        !binding
            .windows(b"Bearer ".len())
            .any(|window| window == b"Bearer ")
    );
}

#[test]
fn repeat_startup_and_supervisor_restart_reuse_the_same_host_authority_reference() {
    let directory = test_directory();
    let state = directory.path().join("studio");
    let source = bootstrap_source(directory.path(), &AUTHORITY);
    let vault = FileSecretVaultV1::open(&state.join("secrets"))
        .unwrap_or_else(|error| panic!("open vault: {error}"));
    let first = establish_host_authority_reference(&state, &vault, Some(&source), None)
        .unwrap_or_else(|error| panic!("first startup: {error}"));
    let repeat = establish_host_authority_reference(&state, &vault, Some(&source), None)
        .unwrap_or_else(|error| panic!("repeat startup: {error}"));
    drop(vault);

    let restarted_vault = FileSecretVaultV1::open(&state.join("secrets"))
        .unwrap_or_else(|error| panic!("reopen vault: {error}"));
    let restarted =
        establish_host_authority_reference(&state, &restarted_vault, Some(&source), None)
            .unwrap_or_else(|error| panic!("restart startup: {error}"));

    assert_eq!(first, repeat);
    assert_eq!(repeat, restarted);
}

#[test]
fn interrupted_first_import_recovers_only_the_exact_matching_single_retained_authority() {
    let directory = test_directory();
    let state = directory.path().join("studio");
    let source = bootstrap_source(directory.path(), &AUTHORITY);
    let vault = FileSecretVaultV1::open(&state.join("secrets"))
        .unwrap_or_else(|error| panic!("open vault: {error}"));
    let imported = vault
        .store(SecretKindV1::HostAuthority, &AUTHORITY)
        .unwrap_or_else(|error| panic!("simulate interrupted import: {error}"));
    let temporary = state.join(".host-authority-reference-interrupted.tmp");
    let mut file = create_owner_only_file(&temporary)
        .unwrap_or_else(|error| panic!("create interrupted binding: {error}"));
    file.write_all(b"{")
        .and_then(|()| file.sync_all())
        .unwrap_or_else(|error| panic!("write interrupted binding: {error}"));

    let recovered = establish_host_authority_reference(&state, &vault, Some(&source), None)
        .unwrap_or_else(|error| panic!("recover interrupted import: {error}"));

    assert_eq!(recovered, imported);
    assert!(state.join("host-authority-reference.json").is_file());
}

#[test]
fn malformed_published_binding_fails_closed_without_repointing_authority() {
    let directory = test_directory();
    let state = directory.path().join("studio");
    let source = bootstrap_source(directory.path(), &AUTHORITY);
    let vault = FileSecretVaultV1::open(&state.join("secrets"))
        .unwrap_or_else(|error| panic!("open vault: {error}"));
    let retained = vault
        .store(SecretKindV1::HostAuthority, &AUTHORITY)
        .unwrap_or_else(|error| panic!("store authority: {error}"));
    let binding = state.join("host-authority-reference.json");
    let mut file = create_owner_only_file(&binding)
        .unwrap_or_else(|error| panic!("create malformed binding: {error}"));
    file.write_all(b"{")
        .and_then(|()| file.sync_all())
        .unwrap_or_else(|error| panic!("write malformed binding: {error}"));

    let error = establish_host_authority_reference(&state, &vault, Some(&source), None)
        .expect_err("malformed published binding must stop startup");

    assert!(error.to_string().contains("malformed"));
    assert!(!error.to_string().contains(retained.as_str()));
}

#[test]
fn missing_or_malformed_bootstrap_authority_fails_closed_without_disclosure() {
    let directory = test_directory();
    let state = directory.path().join("studio");
    let vault = FileSecretVaultV1::open(&state.join("secrets"))
        .unwrap_or_else(|error| panic!("open vault: {error}"));
    let missing = SecretSource::File(directory.path().join("missing.secret"));

    let missing_error = establish_host_authority_reference(&state, &vault, Some(&missing), None)
        .expect_err("missing bootstrap source must stop startup");
    assert!(missing_error.to_string().contains("bootstrap authority"));
    assert!(!missing_error.to_string().contains("missing.secret"));
    assert!(!missing_error.to_string().contains("delete"));

    let malformed = bootstrap_source(directory.path(), &[0xa7; 31]);
    let malformed_error =
        establish_host_authority_reference(&state, &vault, Some(&malformed), None)
            .expect_err("malformed bootstrap source must stop startup");
    assert!(malformed_error.to_string().contains("bootstrap authority"));
    assert!(!malformed_error.to_string().contains("a7"));
}

#[test]
fn supplied_reference_must_match_the_retained_authority_and_never_repoints_it() {
    let directory = test_directory();
    let state = directory.path().join("studio");
    let source = bootstrap_source(directory.path(), &AUTHORITY);
    let vault = FileSecretVaultV1::open(&state.join("secrets"))
        .unwrap_or_else(|error| panic!("open vault: {error}"));
    let retained = establish_host_authority_reference(&state, &vault, Some(&source), None)
        .unwrap_or_else(|error| panic!("first startup: {error}"));
    let mismatched = vault
        .store(SecretKindV1::HostAuthority, &[0xb8; 32])
        .unwrap_or_else(|error| panic!("store distinct authority: {error}"));

    let error =
        establish_host_authority_reference(&state, &vault, Some(&source), Some(&mismatched))
            .expect_err("different supplied reference must not repoint retained authority");

    assert!(error.to_string().contains("does not match"));
    assert!(!error.to_string().contains(mismatched.as_str()));
    let changed_source =
        bootstrap_source_at(directory.path(), "changed-authority.secret", &[0xc9; 32]);
    let changed_error =
        establish_host_authority_reference(&state, &vault, Some(&changed_source), None)
            .expect_err("changed bootstrap authority must not replace retained authority");
    assert!(changed_error.to_string().contains("does not match"));
    assert!(!changed_error.to_string().contains("c9"));
    let still_retained =
        establish_host_authority_reference(&state, &vault, Some(&source), Some(&retained))
            .unwrap_or_else(|error| panic!("matching supplied reference: {error}"));
    assert_eq!(still_retained, retained);
}
