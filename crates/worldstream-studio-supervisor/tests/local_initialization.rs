#![cfg(feature = "cli-operator-preview")]
#![allow(
    clippy::manual_assert_eq,
    reason = "Retained-state comparisons must not print secret bytes or their hashes on failure."
)]

use std::{
    collections::BTreeMap,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};
use worldstream_runtime::{CliOverrides, ConfigLoader};
use worldstream_studio_supervisor::{
    control_access::ControlAccess,
    local_initialization::{
        InitializationRequest, initialize_local, validate_initialized, validate_initialized_at,
    },
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn initialized_validation_resolves_all_relative_paths_against_captured_working_directory()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let initialized = initialize_local(&request(directory.path()))?;
    fs::write(
        &initialized.config_path,
        "config_version = 1\n[storage]\nprofile = 'sqlite-bundled'\ndata_dir = '.worldstream/data'\n[authority.bootstrap]\nsecret_file = '.worldstream/authority.secret'\n",
    )?;
    let loader = ConfigLoader::with_environment(
        Some(PathBuf::from(".worldstream/worldstream.toml")),
        CliOverrides::default(),
        Vec::new(),
    );
    let before = files(directory.path())?;
    let effective =
        validate_initialized_at(&loader, Path::new(".worldstream/studio"), directory.path())?;
    assert_eq!(
        fs::canonicalize(effective.storage.data_dir)?,
        fs::canonicalize(directory.path().join(".worldstream/data"))?
    );
    assert!(files(directory.path())? == before);
    Ok(())
}

fn request(root: &Path) -> InitializationRequest {
    InitializationRequest {
        config: None,
        overrides: CliOverrides::default(),
        state_dir: root.join(".worldstream/studio"),
        working_directory: root.to_path_buf(),
        environment: Vec::new(),
        preview: false,
    }
}

fn files(root: &Path) -> Result<BTreeMap<PathBuf, blake3::Hash>, Box<dyn std::error::Error>> {
    let mut snapshot = BTreeMap::new();
    if !root.exists() {
        return Ok(snapshot);
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            snapshot.extend(files(&entry.path())?);
        } else {
            snapshot.insert(entry.path(), blake3::hash(&fs::read(entry.path())?));
        }
    }
    Ok(snapshot)
}

#[test]
#[cfg(unix)]
fn managed_initialization_rejects_retained_inherited_sources_without_reading_or_mutating()
-> TestResult {
    use std::{io::Seek as _, os::fd::AsRawFd as _};

    for bootstrap_handle in [true, false] {
        let directory = tempfile::tempdir()?;
        let root = directory.path().join("installation");
        worldstream_runtime::prepare_data_directory(&root)?;
        let source_path = root.join("authority.secret");
        let mut source = worldstream_runtime::create_owner_only_file(&source_path)?;
        source.write_all(&[0x29; 32])?;
        drop(source);
        let mut inherited = fs::File::open(&source_path)?;
        let handle = inherited.as_raw_fd();
        let config = root.join("worldstream.toml");
        let source_config = if bootstrap_handle {
            format!("[authority.bootstrap]\nsecret_handle = {handle}\n")
        } else {
            format!(
                "[storage.postgresql]\ndsn_handle = {handle}\n[authority.bootstrap]\nsecret_file = 'authority.secret'\n"
            )
        };
        let profile = if bootstrap_handle {
            "sqlite-bundled"
        } else {
            "postgres-primary"
        };
        fs::write(
            &config,
            format!(
                "config_version = 1\n[storage]\nprofile = '{profile}'\ndata_dir = '.worldstream/data'\n{source_config}"
            ),
        )?;
        let mut invocation = request(&root);
        invocation.config = Some(config);
        let before = files(directory.path())?;
        for preview in [true, false] {
            invocation.preview = preview;
            assert!(initialize_local(&invocation).is_err());
            assert_eq!(inherited.stream_position()?, 0);
            assert!(files(directory.path())? == before);
            assert!(!root.join(".worldstream").exists());
        }
    }
    Ok(())
}

#[test]
#[cfg(any(target_os = "macos", windows))]
fn case_alias_config_collision_never_creates_installation_state() -> TestResult {
    let directory = tempfile::tempdir()?;
    let probe = directory.path().join("case-probe");
    fs::write(&probe, b"case probe")?;
    let case_insensitive = directory.path().join("CASE-PROBE").exists();
    fs::remove_file(probe)?;
    if !case_insensitive {
        return Ok(());
    }
    let mut invocation = request(directory.path());
    invocation.config = Some(directory.path().join(".worldstream/AUTHORITY.SECRET"));
    assert!(initialize_local(&invocation).is_err());
    assert_eq!(fs::read_dir(directory.path())?.count(), 0);
    Ok(())
}

#[test]
fn lexical_state_alias_cannot_bypass_an_existing_installation_lock() -> TestResult {
    let directory = tempfile::tempdir()?;
    let original = request(directory.path());
    initialize_local(&original)?;
    let lock_root = directory
        .path()
        .join(".worldstream/.worldstream-initialization-locks");
    let entry = fs::read_dir(lock_root)?
        .next()
        .ok_or("missing emitted lock")??;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(entry.path())?;
    lock.lock()?;
    let before = files(directory.path())?;
    let mut alias = original;
    alias.state_dir = directory.path().join(".worldstream/./studio");
    assert!(initialize_local(&alias).is_err());
    assert!(files(directory.path())? == before);
    Ok(())
}

#[test]
fn conflicting_config_file_roles_are_rejected_without_creating_authority() -> TestResult {
    for selected in [
        ".worldstream/authority.secret",
        ".worldstream/studio/control-access.v1",
        ".worldstream/studio/host-authority-reference.json",
        ".worldstream/data/worldstream.toml",
    ] {
        for preview in [false, true] {
            let directory = tempfile::tempdir()?;
            let mut invocation = request(directory.path());
            invocation.config = Some(directory.path().join(selected));
            invocation.preview = preview;
            assert!(
                initialize_local(&invocation).is_err(),
                "conflicting role was accepted"
            );
            assert_eq!(
                fs::read_dir(directory.path())?.count(),
                0,
                "collision mutated the installation"
            );
        }
    }
    Ok(())
}

#[test]
fn unsupported_backup_layout_fails_before_creating_any_installation_paths() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut invocation = request(directory.path());
    invocation.overrides.data_dir = Some(directory.path().join("other/data"));
    assert!(initialize_local(&invocation).is_err());
    assert_eq!(fs::read_dir(directory.path())?.count(), 0);
    invocation.preview = true;
    assert!(initialize_local(&invocation).is_err());
    assert_eq!(fs::read_dir(directory.path())?.count(), 0);
    Ok(())
}

#[test]
fn secret_file_ancestors_and_runtime_data_roles_never_mutate_the_installation() -> TestResult {
    for (config, bootstrap) in [
        (
            ".worldstream/config",
            ".worldstream/config/authority.secret",
        ),
        (
            ".worldstream/worldstream.toml",
            ".worldstream/data/worldstream.sqlite3",
        ),
        (
            ".worldstream/worldstream.toml",
            ".worldstream/studio/control-access.v1",
        ),
    ] {
        let directory = tempfile::tempdir()?;
        let mut invocation = request(directory.path());
        invocation.config = Some(directory.path().join(config));
        invocation.environment.push((
            "WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE".to_owned(),
            directory
                .path()
                .join(bootstrap)
                .to_str()
                .ok_or("fixture path")?
                .to_owned(),
        ));
        assert!(initialize_local(&invocation).is_err());
        assert_eq!(fs::read_dir(directory.path())?.count(), 0);
    }
    Ok(())
}

#[test]
fn retained_config_inside_controller_state_supports_control_only_migration() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut invocation = request(directory.path());
    let first = initialize_local(&invocation)?;
    let retained_config = first.state_dir.join("custom-daemon.toml");
    fs::rename(&first.config_path, &retained_config)?;
    invocation.config = Some(retained_config);
    let control = first.state_dir.join("control-access.v1");
    fs::remove_file(&control)?;
    let before = files(directory.path())?;
    let migrated = initialize_local(&invocation)?;
    assert!(!migrated.config_created && migrated.control_created);
    let mut after = files(directory.path())?;
    after.remove(&control);
    assert!(after == before);
    let loader =
        ConfigLoader::with_environment(invocation.config, CliOverrides::default(), Vec::new());
    validate_initialized(&loader, &first.state_dir)?;
    Ok(())
}

#[test]
fn repeat_and_explicit_control_migration_preserve_all_other_retained_bytes() -> TestResult {
    let directory = tempfile::tempdir()?;
    let invocation = request(directory.path());
    let first = initialize_local(&invocation)?;
    let before = files(directory.path())?;
    let repeat = initialize_local(&invocation)?;
    assert!(!repeat.config_created && !repeat.control_created);
    assert!(files(directory.path())? == before);
    let control = first.state_dir.join("control-access.v1");
    fs::remove_file(&control)?;
    let before_migration = files(directory.path())?;
    let loader = ConfigLoader::with_environment(
        Some(first.config_path),
        CliOverrides::default(),
        Vec::new(),
    );
    assert!(validate_initialized(&loader, &first.state_dir).is_err());
    assert!(files(directory.path())? == before_migration);
    let migrated = initialize_local(&invocation)?;
    assert!(!migrated.config_created && migrated.control_created);
    let mut after = files(directory.path())?;
    after.remove(&control);
    assert!(after == before_migration);
    Ok(())
}

#[test]
fn missing_retained_bootstrap_or_binding_never_triggers_authority_repair() -> TestResult {
    for missing in [
        ".worldstream/authority.secret",
        ".worldstream/studio/host-authority-reference.json",
    ] {
        let directory = tempfile::tempdir()?;
        let invocation = request(directory.path());
        initialize_local(&invocation)?;
        fs::remove_file(directory.path().join(missing))?;
        let before = files(directory.path())?;
        assert!(initialize_local(&invocation).is_err());
        assert!(files(directory.path())? == before);
    }
    Ok(())
}

#[test]
fn existing_daemon_bootstrap_onboards_without_changing_retained_data() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join(".worldstream");
    worldstream_runtime::prepare_data_directory(&root.join("data"))?;
    let retained = root.join("data/opaque-retained.db");
    fs::write(&retained, b"retained daemon data canary")?;
    let bootstrap = root.join("authority.secret");
    let mut source = worldstream_runtime::create_owner_only_file(&bootstrap)?;
    source.write_all(&[0x73; 32])?;
    drop(source);
    let initialized = initialize_local(&request(directory.path()))?;
    assert!(initialized.config_created && initialized.control_created);
    assert!(fs::read(&bootstrap)? == [0x73; 32]);
    assert!(fs::read(&retained)? == b"retained daemon data canary");
    Ok(())
}

#[test]
fn preview_does_not_require_or_open_postgres_and_bootstrap_secret_sources() -> TestResult {
    let directory = tempfile::tempdir()?;
    let config = directory.path().join("worldstream.toml");
    fs::write(
        &config,
        "config_version = 1\n[storage]\nprofile = 'postgres-primary'\ndata_dir = '.worldstream/data'\n[storage.postgresql]\ndsn_file = 'missing-dsn.secret'\n[authority.bootstrap]\nsecret_file = 'missing-bootstrap.secret'\n",
    )?;
    let mut invocation = request(directory.path());
    invocation.config = Some(config);
    invocation.preview = true;
    let before = files(directory.path())?;
    let preview = initialize_local(&invocation)?;
    assert_eq!(preview.mode, "preview");
    assert!(files(directory.path())? == before);
    assert!(!directory.path().join(".worldstream").exists());
    Ok(())
}

#[test]
fn captured_working_directory_resolves_existing_config_secret_and_data_references() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("installation");
    worldstream_runtime::prepare_data_directory(&root)?;
    // The unique basename is absent from the real process CWD; the old loader
    // fails before any write because its mistaken relative parent is empty.
    let name = format!(
        "{}-bootstrap.secret",
        directory
            .path()
            .file_name()
            .ok_or("temp name")?
            .to_string_lossy()
    );
    let mut bootstrap = worldstream_runtime::create_owner_only_file(&root.join(&name))?;
    bootstrap.write_all(&[0x39; 32])?;
    drop(bootstrap);
    let config = root.join("worldstream.toml");
    let original = format!(
        "config_version = 1\n[storage]\ndata_dir = 'data'\n[authority.bootstrap]\nsecret_file = '{name}'\n"
    );
    fs::write(&config, &original)?;
    let request = InitializationRequest {
        config: Some(PathBuf::from("worldstream.toml")),
        overrides: CliOverrides::default(),
        state_dir: PathBuf::from("studio"),
        working_directory: root.clone(),
        environment: Vec::new(),
        preview: false,
    };
    let receipt = initialize_local(&request)?;
    assert_eq!(receipt.data_dir, root.join("data"));
    assert!(!receipt.config_created && receipt.control_created);
    assert!(fs::read(&config)? == original.as_bytes());
    assert!(fs::read(root.join(name))? == [0x39; 32]);
    assert!(
        ControlAccess::open(&receipt.state_dir)?
            .authorization_header()?
            .is_sensitive()
    );
    Ok(())
}

#[test]
fn fresh_apply_creates_a_protected_installation_usable_by_readonly_startup() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let request = InitializationRequest {
        config: None,
        overrides: CliOverrides {
            data_dir: Some(root.join(".worldstream/selected-data")),
            bind: Some("127.0.0.1:9511".parse()?),
            ..CliOverrides::default()
        },
        state_dir: root.join(".worldstream/studio"),
        working_directory: root.to_path_buf(),
        environment: Vec::new(),
        preview: false,
    };
    let receipt = initialize_local(&request)?;
    assert_eq!(receipt.mode, "initialized");
    assert!(receipt.config_created && receipt.control_created);
    assert!(!receipt.services_started);
    let loader = ConfigLoader::with_environment(
        Some(receipt.config_path.clone()),
        CliOverrides::default(),
        Vec::new(),
    );
    let configuration = validate_initialized(&loader, &receipt.state_dir)?;
    assert_eq!(
        configuration.storage.data_dir,
        root.join(".worldstream/selected-data")
    );
    assert_eq!(configuration.server.bind, "127.0.0.1:9511".parse()?);
    assert!(
        ControlAccess::open(&receipt.state_dir)?
            .authorization_header()?
            .is_sensitive()
    );
    assert_eq!(fs::read_dir(&receipt.data_dir)?.count(), 0);
    worldstream_runtime::validate_owner_only_file(&receipt.config_path)?;
    Ok(())
}

#[test]
fn preview_fresh_installation_creates_nothing() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path().to_path_buf();
    let request = InitializationRequest {
        config: None,
        overrides: CliOverrides::default(),
        state_dir: PathBuf::from(".worldstream/studio"),
        working_directory: root.clone(),
        environment: Vec::new(),
        preview: true,
    };
    let receipt = initialize_local(&request)?;
    assert_eq!(receipt.mode, "preview");
    assert_eq!(
        receipt.config_path,
        root.join(".worldstream/worldstream.toml")
    );
    assert_eq!(receipt.data_dir, root.join(".worldstream/data"));
    assert!(!receipt.config_created && !receipt.control_created && !receipt.services_started);
    assert_eq!(fs::read_dir(&root)?.count(), 0);
    Ok(())
}
