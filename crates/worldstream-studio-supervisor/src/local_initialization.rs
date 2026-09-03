//! Explicit local installation planning and protected initialization.

use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use serde::Serialize;
use thiserror::Error;
use worldstream_runtime::{
    CliOverrides, ConfigLoader, EffectiveConfig, ProspectiveConfig, SecretSource,
    create_owner_only_file, prepare_data_directory, validate_data_directory,
    validate_owner_only_file,
};

use crate::{
    control_access::{ControlAccess, ControlAccessError},
    protected_publication::{PublicationMode, publish},
    secrets::FileSecretVaultV1,
    startup_authority::{
        bootstrap_source_for_local_development, establish_host_authority_reference,
        validate_existing_host_authority,
    },
};

/// Captured invocation inputs; initialization never reads ambient environment.
#[derive(Clone)]
pub struct InitializationRequest {
    pub config: Option<PathBuf>,
    pub overrides: CliOverrides,
    pub state_dir: PathBuf,
    pub working_directory: PathBuf,
    pub environment: Vec<(String, String)>,
    pub preview: bool,
}

/// Safe, non-secret outcome of planning or initializing one installation.
#[derive(Debug, Serialize)]
pub struct InitializationReceipt {
    pub mode: String,
    pub config_path: PathBuf,
    pub state_dir: PathBuf,
    pub data_dir: PathBuf,
    pub config_created: bool,
    pub control_created: bool,
    pub services_started: bool,
}

/// Closed error vocabulary; configuration values and secret sources are omitted.
#[derive(Debug, Error)]
#[error(
    "local initialization is unavailable; inspect configuration and protected installation state"
)]
pub struct InitializationError;

struct Plan {
    loader: ConfigLoader,
    prospective: ProspectiveConfig,
    receipt: InitializationReceipt,
    new_config: Option<String>,
    lock_root: PathBuf,
}

#[derive(Clone, serde::Serialize)]
pub(crate) struct ImportInstallation {
    pub config_path: PathBuf,
    pub state_dir: PathBuf,
    pub data_dir: PathBuf,
    pub host_identity: String,
    pub configuration_digest: String,
}

/// Read-only import prerequisite check. No control, bootstrap, DSN or vault
/// secret is opened; actual authority validation belongs to approved apply.
pub(crate) fn import_installation(
    request: &InitializationRequest,
) -> Result<ImportInstallation, InitializationError> {
    use std::io::Read as _;
    let plan = plan(request)?;
    if plan.new_config.is_some() {
        return Err(InitializationError);
    }
    let config_path =
        fs::canonicalize(&plan.receipt.config_path).map_err(|_| InitializationError)?;
    let state_dir =
        validate_data_directory(&plan.receipt.state_dir).map_err(|_| InitializationError)?;
    let data_dir =
        validate_data_directory(&plan.receipt.data_dir).map_err(|_| InitializationError)?;
    let control = state_dir.join("control-access.v1");
    validate_owner_only_file(&control).map_err(|_| InitializationError)?;
    if fs::metadata(control)
        .map_err(|_| InitializationError)?
        .len()
        != 48
    {
        return Err(InitializationError);
    }
    let Some(SecretSource::File(bootstrap)) = plan.prospective.bootstrap_source() else {
        return Err(InitializationError);
    };
    validate_owner_only_file(bootstrap).map_err(|_| InitializationError)?;
    if fs::metadata(bootstrap)
        .map_err(|_| InitializationError)?
        .len()
        != 32
    {
        return Err(InitializationError);
    }
    let host_identity = crate::startup_authority::retained_host_identity(&state_dir)
        .map_err(|_| InitializationError)?;
    let mut digest = blake3::Hasher::new_derive_key("worldstream/initialization-configuration/v1");
    let mut file = fs::File::open(&config_path).map_err(|_| InitializationError)?;
    let mut buffer = [0_u8; 8192];
    loop {
        let count = file.read(&mut buffer).map_err(|_| InitializationError)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let effective = plan
        .prospective
        .installation_toml(&request.working_directory)
        .map_err(|_| InitializationError)?;
    digest.update(&[0]);
    digest.update(effective.as_bytes());
    Ok(ImportInstallation {
        config_path,
        state_dir: fs::canonicalize(state_dir).map_err(|_| InitializationError)?,
        data_dir: fs::canonicalize(data_dir).map_err(|_| InitializationError)?,
        host_identity,
        configuration_digest: digest.finalize().to_hex().to_string(),
    })
}

pub(crate) fn lock_import_installation(
    request: &InitializationRequest,
) -> Result<fs::File, InitializationError> {
    installation_lock(&plan(request)?.lock_root)
}

pub(crate) fn validate_import_authority(
    request: &InitializationRequest,
) -> Result<(), InitializationError> {
    let plan = plan(request)?;
    ControlAccess::open(&plan.receipt.state_dir).map_err(|_| InitializationError)?;
    let effective = plan
        .loader
        .load_at(&request.working_directory)
        .map_err(|_| InitializationError)?;
    validate_existing_host_authority(
        &plan.receipt.state_dir,
        effective.authority.bootstrap_secret.as_ref(),
    )
    .map_err(|_| InitializationError)?;
    Ok(())
}

/// Plans or explicitly initializes local operator state without starting services.
///
/// # Errors
/// Rejects unsafe paths, invalid configuration, or unavailable retained state.
pub fn initialize_local(
    request: &InitializationRequest,
) -> Result<InitializationReceipt, InitializationError> {
    let plan = plan(request)?;
    if request.preview {
        return Ok(plan.receipt);
    }
    let _lock = installation_lock(&plan.lock_root)?;
    // Recheck the selected configuration and freshness after obtaining ownership.
    let mut plan = self::plan(request)?;
    let fresh_state = empty_or_absent(&plan.receipt.state_dir)?;
    let configured = plan
        .prospective
        .bootstrap_source()
        .ok_or(InitializationError)?;
    if let SecretSource::File(path) = configured {
        safe_text(path)?;
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !fresh_state || !empty_or_absent(&plan.receipt.data_dir)? {
                    return Err(InitializationError);
                }
                protected_directory(path.parent().ok_or(InitializationError)?)?;
            }
            Err(_) => return Err(InitializationError),
            Ok(_) => {}
        }
    }
    let bootstrap = bootstrap_source_for_local_development(
        &plan.receipt.state_dir,
        &plan.receipt.data_dir,
        plan.prospective.storage_profile(),
        Some(configured),
    )
    .map_err(|_| InitializationError)?;
    // Final startup validation reads credentials only in explicit apply mode.
    let _effective = plan
        .loader
        .load_at(&request.working_directory)
        .map_err(|_| InitializationError)?;
    if fresh_state {
        let vault = FileSecretVaultV1::open(&plan.receipt.state_dir.join("secrets"))
            .map_err(|_| InitializationError)?;
        establish_host_authority_reference(&plan.receipt.state_dir, &vault, Some(&bootstrap), None)
            .map_err(|_| InitializationError)?;
    } else {
        validate_existing_host_authority(&plan.receipt.state_dir, Some(&bootstrap))
            .map_err(|_| InitializationError)?;
    }
    protected_directory(&plan.receipt.data_dir)?;
    let control_created = match ControlAccess::open(&plan.receipt.state_dir) {
        Ok(_) => false,
        Err(ControlAccessError::NotInitialized) => true,
        Err(_) => return Err(InitializationError),
    };
    // Lock ordering is installation first, control access second.
    ControlAccess::initialize(&plan.receipt.state_dir).map_err(|_| InitializationError)?;
    if let Some(document) = &plan.new_config {
        plan.receipt.config_created =
            publish_config(&plan.receipt.config_path, document.as_bytes())?;
    }
    plan.receipt.control_created = control_created;
    "initialized".clone_into(&mut plan.receipt.mode);
    Ok(plan.receipt)
}

/// Checks an explicitly initialized installation before any startup store opens.
/// No missing directory, authority binding, vault, or control access is repaired.
///
/// # Errors
/// Fails closed when existing configuration or retained authority is unavailable.
pub fn validate_initialized(
    loader: &ConfigLoader,
    state_dir: &Path,
) -> Result<EffectiveConfig, InitializationError> {
    ControlAccess::open(state_dir).map_err(|_| InitializationError)?;
    let effective = loader.load().map_err(|_| InitializationError)?;
    validate_initialized_configuration(state_dir, effective)
}

/// Checks retained initialization using one explicit working directory for the
/// selected configuration and all relative local references. Never repairs state.
///
/// # Errors
/// Fails closed on invalid paths, configuration, or retained authority.
pub fn validate_initialized_at(
    loader: &ConfigLoader,
    state_dir: &Path,
    working_directory: &Path,
) -> Result<EffectiveConfig, InitializationError> {
    if !working_directory.is_absolute() {
        return Err(InitializationError);
    }
    let state_dir = safe_absolute(working_directory, state_dir)?;
    ControlAccess::open(&state_dir).map_err(|_| InitializationError)?;
    let effective = loader
        .load_at(working_directory)
        .map_err(|_| InitializationError)?;
    validate_initialized_configuration(&state_dir, effective)
}

fn validate_initialized_configuration(
    state_dir: &Path,
    effective: EffectiveConfig,
) -> Result<EffectiveConfig, InitializationError> {
    validate_data_directory(&effective.storage.data_dir).map_err(|_| InitializationError)?;
    validate_backup_layout(state_dir, &effective.storage.data_dir)?;
    validate_existing_host_authority(state_dir, effective.authority.bootstrap_secret.as_ref())
        .map_err(|_| InitializationError)?;
    Ok(effective)
}

fn plan(request: &InitializationRequest) -> Result<Plan, InitializationError> {
    let cwd = &request.working_directory;
    if !cwd.is_absolute() {
        return Err(InitializationError);
    }
    let mut environment: BTreeMap<_, _> = request.environment.iter().cloned().collect();
    let selected = request
        .config
        .clone()
        .or_else(|| environment.get("WORLDSTREAM_CONFIG").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(".worldstream/worldstream.toml"));
    let config_path = safe_absolute(cwd, &selected)?;
    let state_dir = safe_absolute(cwd, &request.state_dir)?;
    let existing = match fs::symlink_metadata(&config_path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        _ => return Err(InitializationError),
    };
    environment.remove("WORLDSTREAM_CONFIG");
    if !existing {
        environment
            .entry("WORLDSTREAM__STORAGE__DATA_DIR".to_owned())
            .or_insert(safe_text(&cwd.join(".worldstream/data"))?.to_owned());
        if !environment.contains_key("WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE")
            && !environment.contains_key("WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_HANDLE")
        {
            environment.insert(
                "WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE".to_owned(),
                safe_text(&cwd.join(".worldstream/authority.secret"))?.to_owned(),
            );
        }
    }
    let loader = ConfigLoader::with_environment(
        existing.then(|| config_path.clone()),
        request.overrides.clone(),
        environment,
    );
    let prospective = loader.preview_at(cwd).map_err(|_| InitializationError)?;
    // Managed installation is durable across invocations. Inherited handles
    // belong to the separate foreground loader contract and must not be read
    // or consumed while planning even an already-retained configuration.
    if [
        prospective.bootstrap_source(),
        prospective.postgresql_dsn_source(),
    ]
    .into_iter()
    .flatten()
    .any(|source| matches!(source, SecretSource::InheritedHandle(_)))
    {
        return Err(InitializationError);
    }
    let data_dir = safe_absolute(cwd, prospective.data_directory())?;
    let lock_root = state_dir
        .parent()
        .ok_or(InitializationError)?
        .join(".worldstream-initialization-locks");
    if lock_root.starts_with(&state_dir)
        || lock_root.starts_with(&data_dir)
        || config_path == state_dir
        || config_path == data_dir
    {
        return Err(InitializationError);
    }
    let new_config = if existing {
        None
    } else {
        Some(
            prospective
                .installation_toml(cwd)
                .map_err(|_| InitializationError)?,
        )
    };
    validate_file_roles(
        &config_path,
        existing,
        &state_dir,
        &data_dir,
        &lock_root,
        &prospective,
    )?;
    let receipt = InitializationReceipt {
        mode: "preview".to_owned(),
        config_path,
        state_dir,
        data_dir,
        config_created: false,
        control_created: false,
        services_started: false,
    };
    Ok(Plan {
        loader,
        prospective,
        receipt,
        new_config,
        lock_root,
    })
}

fn empty_or_absent(path: &Path) -> Result<bool, InitializationError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Ok(_) => {
            validate_data_directory(path).map_err(|_| InitializationError)?;
            Ok(fs::read_dir(path)
                .map_err(|_| InitializationError)?
                .next()
                .is_none())
        }
        Err(_) => Err(InitializationError),
    }
}

fn protected_directory(path: &Path) -> Result<PathBuf, InitializationError> {
    match fs::symlink_metadata(path) {
        Ok(_) => validate_data_directory(path).map_err(|_| InitializationError),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            prepare_data_directory(path).map_err(|_| InitializationError)
        }
        Err(_) => Err(InitializationError),
    }
}

fn installation_lock(root: &Path) -> Result<fs::File, InitializationError> {
    protected_directory(root)?;
    // OS aliases of the same sibling directory must lock one inode.
    let path = root.join("initialize.lock");
    match create_owner_only_file(&path) {
        Ok(file) => {
            file.sync_all().map_err(|_| InitializationError)?;
        }
        Err(_) => {
            validate_owner_only_file(&path).map_err(|_| InitializationError)?;
        }
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        options.share_mode(0x0000_0001 | 0x0000_0002);
    }
    let file = options.open(&path).map_err(|_| InitializationError)?;
    validate_owner_only_file(&path).map_err(|_| InitializationError)?;
    // Contention fails immediately; callers may retry without a hidden wait.
    file.try_lock().map_err(|_| InitializationError)?;
    Ok(file)
}

#[derive(Clone, Copy)]
enum FileRole {
    Config,
    Bootstrap,
    Dsn,
    Control,
    Binding,
}

fn validate_file_roles(
    config: &Path,
    config_exists: bool,
    state: &Path,
    data: &Path,
    lock_root: &Path,
    prospective: &ProspectiveConfig,
) -> Result<(), InitializationError> {
    validate_backup_layout(state, data)?;
    let state = projected_path(state)?;
    let data = projected_path(data)?;
    let locks = projected_path(lock_root)?;
    if overlaps(&state, &data) || overlaps(&locks, &data) || overlaps(&locks, &state) {
        return Err(InitializationError);
    }
    let vault = state.join("secrets");
    let mut files = vec![
        (FileRole::Config, projected_path(config)?),
        (
            FileRole::Control,
            projected_path(&state.join("control-access.v1"))?,
        ),
        (
            FileRole::Binding,
            projected_path(&state.join("host-authority-reference.json"))?,
        ),
    ];
    for (role, source) in [
        (FileRole::Bootstrap, prospective.bootstrap_source()),
        (FileRole::Dsn, prospective.postgresql_dsn_source()),
    ] {
        if let Some(SecretSource::File(path)) = source {
            safe_text(path)?;
            let projected = projected_path(path)?;
            // Preserve retained custom sources, but do not populate fresh
            // Supervisor state while preparing a missing source's parent.
            if collision_prefix(&projected, &state) && fs::symlink_metadata(path).is_err() {
                return Err(InitializationError);
            }
            files.push((role, projected));
        }
    }
    for (index, (role, path)) in files.iter().enumerate() {
        if overlaps(path, &data)
            || overlaps(path, &locks)
            || overlaps(path, &vault)
            || collision_prefix(&state, path)
            || (matches!(role, FileRole::Config)
                && !config_exists
                && collision_prefix(path, &state))
        {
            return Err(InitializationError);
        }
        if files
            .iter()
            .skip(index + 1)
            .any(|(_, other)| overlaps(path, other))
        {
            return Err(InitializationError);
        }
    }
    Ok(())
}

fn overlaps(first: &Path, second: &Path) -> bool {
    collision_prefix(first, second) || collision_prefix(second, first)
}

// Conservative rejection only: never rewrite an I/O path or use folded
// spelling to establish that the positive shared-backup layout is valid.
fn collision_prefix(path: &Path, prefix: &Path) -> bool {
    let mut components = path.components();
    prefix.components().all(|expected| {
        components.next().is_some_and(|actual| {
            #[cfg(any(target_os = "macos", windows))]
            {
                actual
                    .as_os_str()
                    .as_encoded_bytes()
                    .eq_ignore_ascii_case(expected.as_os_str().as_encoded_bytes())
            }
            #[cfg(not(any(target_os = "macos", windows)))]
            {
                actual == expected
            }
        })
    })
}

fn validate_backup_layout(state: &Path, data: &Path) -> Result<(), InitializationError> {
    let state = projected_path(state)?;
    let data = projected_path(data)?;
    if overlaps(&state, &data) {
        return Err(InitializationError);
    }
    let expected_state = projected_path(&data.parent().ok_or(InitializationError)?.join("studio"))?;
    if state != expected_state {
        return Err(InitializationError);
    }
    let supervisor_backups = projected_path(&state.join("backups"))?;
    let daemon_backups = projected_path(&expected_state.join("backups"))?;
    if supervisor_backups != daemon_backups {
        return Err(InitializationError);
    }
    match fs::symlink_metadata(state.join("backups")) {
        Ok(_) => {
            validate_data_directory(&state.join("backups")).map_err(|_| InitializationError)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Err(InitializationError),
    }
    Ok(())
}

// Resolve existing ancestors without creation. Original paths still flow to
// protected filesystem operations for authoritative symlink/ownership checks.
fn projected_path(path: &Path) -> Result<PathBuf, InitializationError> {
    let mut ancestor = path;
    let mut suffix = Vec::new();
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => {
                let mut canonical = fs::canonicalize(ancestor).map_err(|_| InitializationError)?;
                for component in suffix.into_iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let component = ancestor.file_name().ok_or(InitializationError)?;
                #[cfg(any(target_os = "macos", windows))]
                if !component.as_encoded_bytes().is_ascii() {
                    return Err(InitializationError);
                }
                #[cfg(windows)]
                if component
                    .as_encoded_bytes()
                    .last()
                    .is_some_and(|last| matches!(*last, b'.' | b' '))
                {
                    return Err(InitializationError);
                }
                suffix.push(component.to_os_string());
                ancestor = ancestor.parent().ok_or(InitializationError)?;
            }
            Err(_) => return Err(InitializationError),
        }
    }
}

fn publish_config(path: &Path, bytes: &[u8]) -> Result<bool, InitializationError> {
    let parent = path.parent().ok_or(InitializationError)?;
    if !parent.exists() {
        protected_directory(parent)?;
    }
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| InitializationError)?;
    let temporary_directory = parent.join(format!(
        ".worldstream-init-{}",
        blake3::hash(&nonce).to_hex()
    ));
    protected_directory(&temporary_directory)?;
    let temporary = temporary_directory.join("config.tmp");
    let result = (|| {
        let mut file = create_owner_only_file(&temporary).map_err(|_| InitializationError)?;
        file.write_all(bytes).map_err(|_| InitializationError)?;
        match publish(file, &temporary, path, PublicationMode::CreateNew) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                validate_owner_only_file(path).map_err(|_| InitializationError)?;
                let metadata = fs::metadata(path).map_err(|_| InitializationError)?;
                if metadata.len() != u64::try_from(bytes.len()).map_err(|_| InitializationError)?
                    || fs::read(path).map_err(|_| InitializationError)? != bytes
                {
                    return Err(InitializationError);
                }
                Ok(false)
            }
            Err(_) => Err(InitializationError),
        }
    })();
    let _ = fs::remove_file(&temporary);
    let _ = fs::remove_dir(&temporary_directory);
    result
}

fn safe_text(path: &Path) -> Result<&str, InitializationError> {
    path.to_str()
        .filter(|text| {
            !text.is_empty() && text.len() <= 4096 && !text.chars().any(char::is_control)
        })
        .ok_or(InitializationError)
}

fn safe_absolute(cwd: &Path, path: &Path) -> Result<PathBuf, InitializationError> {
    safe_text(path)?;
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    safe_text(&absolute)?;
    if absolute
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(InitializationError);
    }
    Ok(absolute)
}
