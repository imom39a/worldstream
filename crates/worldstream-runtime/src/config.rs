use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fmt, fs,
    io::Read,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
};

use directories::ProjectDirs;
use filedescriptor::{AsRawFileDescriptor, FileDescriptor, RawFileDescriptor};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{FilesystemError, ManifestError, embedded_manifest, validate_owner_only_file};

const CONFIG_VERSION: u32 = 1;
const MAX_CONFIG_BYTES: u64 = 256 * 1024;
const ENV_PREFIX: &str = "WORLDSTREAM__";
const ENV_CONFIG_PATH: &str = "WORLDSTREAM_CONFIG";

/// Startup storage profile. Selection is fixed until process termination.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum StorageProfile {
    /// Exact release-bundled `SQLite` profile (adapter added in a later issue).
    #[serde(rename = "sqlite-bundled")]
    SqliteBundled,
    /// One hosted or self-managed `PostgreSQL` 17 writable primary.
    #[serde(rename = "postgres-primary")]
    PostgresPrimary,
}

impl StorageProfile {
    /// Canonical compatibility-manifest selector.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SqliteBundled => "sqlite-bundled",
            Self::PostgresPrimary => "postgres-primary",
        }
    }
}

impl fmt::Display for StorageProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for StorageProfile {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "sqlite-bundled" => Ok(Self::SqliteBundled),
            "postgres-primary" => Ok(Self::PostgresPrimary),
            other => Err(ConfigError::UnsupportedValue {
                key: "storage.profile",
                value: other.to_owned(),
            }),
        }
    }
}

/// A reference to secret bytes. The bytes themselves never enter config.
#[derive(Clone, Eq, PartialEq)]
pub enum SecretSource {
    /// Path to an owner-readable, non-symlink regular file.
    File(PathBuf),
    /// Numeric inherited descriptor/handle supplied by the process launcher.
    InheritedHandle(u64),
}

impl SecretSource {
    /// Stable non-secret source discriminator.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::File(_) => "owner_readable_secret_file",
            Self::InheritedHandle(_) => "inherited_handle",
        }
    }
}

impl fmt::Debug for SecretSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretSource")
            .field("kind", &self.kind())
            .field("value", &"[REDACTED]")
            .finish()
    }
}

/// Listener configuration for the operator-only shell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerConfig {
    /// Listener address. The compiled default is loopback.
    pub bind: SocketAddr,
}

/// Startup-fixed storage selection and local runtime directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageConfig {
    /// Frozen profile selector.
    pub profile: StorageProfile,
    /// WorldStream-owned runtime directory.
    pub data_dir: PathBuf,
    /// Runtime `PostgreSQL` DSN secret reference, present only for that profile.
    pub postgresql_dsn: Option<SecretSource>,
}

/// Fully layered, validated process configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveConfig {
    /// Version of this configuration contract.
    pub config_version: u32,
    /// Operator listener configuration.
    pub server: ServerConfig,
    /// Startup-fixed storage configuration.
    pub storage: StorageConfig,
}

impl Default for EffectiveConfig {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            server: ServerConfig {
                bind: SocketAddr::from(([127, 0, 0, 1], 9410)),
            },
            storage: StorageConfig {
                profile: StorageProfile::SqliteBundled,
                data_dir: default_data_directory(),
                postgresql_dsn: None,
            },
        }
    }
}

fn default_data_directory() -> PathBuf {
    ProjectDirs::from("dev", "WorldStream", "WorldStream").map_or_else(
        || {
            #[cfg(windows)]
            {
                PathBuf::from(r"C:\ProgramData\WorldStream")
            }
            #[cfg(not(windows))]
            {
                PathBuf::from("/var/lib/worldstream")
            }
        },
        |directories| directories.data_local_dir().to_path_buf(),
    )
}

impl EffectiveConfig {
    /// Returns a serializable view that never reveals secret paths or handles.
    #[must_use]
    pub fn redacted(&self) -> RedactedConfig {
        RedactedConfig {
            config_version: self.config_version,
            server: RedactedServerConfig {
                bind: self.server.bind,
            },
            storage: RedactedStorageConfig {
                profile: self.storage.profile,
                data_dir: self.storage.data_dir.clone(),
                postgresql_dsn: self.storage.postgresql_dsn.as_ref().map(|secret| {
                    RedactedSecretSource {
                        source: secret.kind(),
                        value: "[REDACTED]",
                    }
                }),
            },
        }
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.config_version != CONFIG_VERSION {
            return Err(ConfigError::UnsupportedConfigVersion(self.config_version));
        }
        if self.server.bind.port() == 0 {
            return Err(ConfigError::OutOfRange {
                key: "server.bind",
                value: self.server.bind.to_string(),
            });
        }
        if self.storage.data_dir.as_os_str().is_empty() {
            return Err(ConfigError::OutOfRange {
                key: "storage.data_dir",
                value: "empty".to_owned(),
            });
        }

        let manifest = embedded_manifest()?;
        if !manifest.supports_storage_profile(self.storage.profile.as_str()) {
            return Err(ConfigError::UnsupportedValue {
                key: "storage.profile",
                value: self.storage.profile.to_string(),
            });
        }

        match (self.storage.profile, &self.storage.postgresql_dsn) {
            (StorageProfile::SqliteBundled, Some(_)) => {
                return Err(ConfigError::InactiveBackend(
                    "storage.postgresql is configured while storage.profile is sqlite-bundled",
                ));
            }
            (StorageProfile::PostgresPrimary, None) => {
                return Err(ConfigError::Missing(
                    "postgres-primary requires exactly one DSN secret file or inherited handle",
                ));
            }
            (StorageProfile::PostgresPrimary, Some(source)) => validate_secret(source)?,
            (StorageProfile::SqliteBundled, None) => {}
        }

        Ok(())
    }
}

/// JSON/TOML-safe effective config output with secret references redacted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RedactedConfig {
    /// Config schema version.
    pub config_version: u32,
    /// Redacted listener section.
    pub server: RedactedServerConfig,
    /// Redacted storage section.
    pub storage: RedactedStorageConfig,
}

/// Non-secret listener output.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RedactedServerConfig {
    /// Effective listener address.
    pub bind: SocketAddr,
}

/// Storage output without DSN path, handle, or bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RedactedStorageConfig {
    /// Effective storage profile.
    pub profile: StorageProfile,
    /// Effective non-secret data path.
    pub data_dir: PathBuf,
    /// Redacted DSN source when `PostgreSQL` is selected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub postgresql_dsn: Option<RedactedSecretSource>,
}

/// Redacted secret reference emitted by `config effective`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RedactedSecretSource {
    /// Secret delivery mechanism.
    pub source: &'static str,
    /// Constant placeholder, never a path, descriptor, handle, or secret.
    pub value: &'static str,
}

/// Documented command-line overrides. No secret has a CLI representation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CliOverrides {
    /// Listener override.
    pub bind: Option<SocketAddr>,
    /// Storage profile override.
    pub storage_profile: Option<StorageProfile>,
    /// Data-directory override.
    pub data_dir: Option<PathBuf>,
}

/// Deterministic configuration loader with injectable environment for tests.
#[derive(Clone)]
pub struct ConfigLoader {
    cli_config: Option<PathBuf>,
    cli_overrides: CliOverrides,
    environment: BTreeMap<String, String>,
}

impl fmt::Debug for ConfigLoader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigLoader")
            .field("cli_config", &self.cli_config.as_ref().map(|_| "[PRESENT]"))
            .field("cli_overrides", &self.cli_overrides)
            .field("environment_key_count", &self.environment.len())
            .field(
                "environment_config_path_present",
                &self.environment.contains_key(ENV_CONFIG_PATH),
            )
            .field(
                "environment_dsn_file_present",
                &self
                    .environment
                    .contains_key("WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE"),
            )
            .field(
                "environment_dsn_handle_present",
                &self
                    .environment
                    .contains_key("WORLDSTREAM__STORAGE__POSTGRESQL__DSN_HANDLE"),
            )
            .finish_non_exhaustive()
    }
}

impl ConfigLoader {
    /// Captures the process environment. Non-Unicode `WorldStream` values fail.
    ///
    /// # Errors
    ///
    /// Returns an error if a relevant environment key or value is not Unicode.
    pub fn from_process(
        cli_config: Option<PathBuf>,
        cli_overrides: CliOverrides,
    ) -> Result<Self, ConfigError> {
        let environment = collect_process_environment()?;
        Ok(Self {
            cli_config,
            cli_overrides,
            environment,
        })
    }

    /// Creates a deterministic loader from explicit key/value pairs.
    #[must_use]
    pub fn with_environment(
        cli_config: Option<PathBuf>,
        cli_overrides: CliOverrides,
        environment: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        Self {
            cli_config,
            cli_overrides,
            environment: environment.into_iter().collect(),
        }
    }

    /// Applies defaults, at most one explicit versioned file, environment, and CLI.
    ///
    /// # Errors
    ///
    /// Returns an error for any unsupported, malformed, insecure, inactive,
    /// missing, unknown, or out-of-range value.
    pub fn load(&self) -> Result<EffectiveConfig, ConfigError> {
        let mut effective = EffectiveConfig::default();
        if let Some(path) = self.selected_config_path()? {
            apply_file(&mut effective, path)?;
        }
        apply_environment(&mut effective, &self.environment)?;
        apply_cli(&mut effective, &self.cli_overrides);
        effective.validate()?;
        Ok(effective)
    }

    fn selected_config_path(&self) -> Result<Option<&Path>, ConfigError> {
        if let Some(path) = self.cli_config.as_deref() {
            if path.as_os_str().is_empty() {
                return Err(ConfigError::EmptyConfigPath);
            }
            return Ok(Some(path));
        }

        match self.environment.get(ENV_CONFIG_PATH) {
            Some(path) if path.is_empty() => Err(ConfigError::EmptyConfigPath),
            Some(path) => Ok(Some(Path::new(path))),
            None => Ok(None),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    config_version: u32,
    #[serde(default)]
    server: Option<FileServerConfig>,
    #[serde(default)]
    storage: Option<FileStorageConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileServerConfig {
    #[serde(default)]
    bind: Option<SocketAddr>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileStorageConfig {
    #[serde(default)]
    profile: Option<StorageProfile>,
    #[serde(default)]
    data_dir: Option<PathBuf>,
    #[serde(default)]
    postgresql: Option<FilePostgresqlConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FilePostgresqlConfig {
    #[serde(default)]
    dsn_file: Option<PathBuf>,
    #[serde(default)]
    dsn_handle: Option<u64>,
}

fn apply_file(effective: &mut EffectiveConfig, path: &Path) -> Result<(), ConfigError> {
    let metadata = fs::metadata(path).map_err(|source| ConfigError::ReadConfig {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(ConfigError::ConfigTooLarge {
            path: path.to_path_buf(),
            bytes: metadata.len(),
        });
    }
    let source = fs::read_to_string(path).map_err(|source| ConfigError::ReadConfig {
        path: path.to_path_buf(),
        source,
    })?;
    let file: FileConfig = toml::from_str(&source)
        .map_err(|error| ConfigError::Toml(SanitizedTomlError::new(&error, &source)))?;
    effective.config_version = file.config_version;
    if let Some(server) = file.server
        && let Some(bind) = server.bind
    {
        effective.server.bind = bind;
    }
    if let Some(storage) = file.storage {
        if let Some(profile) = storage.profile {
            effective.storage.profile = profile;
        }
        if let Some(data_dir) = storage.data_dir {
            effective.storage.data_dir = data_dir;
        }
        if let Some(postgresql) = storage.postgresql {
            effective.storage.postgresql_dsn = secret_from_parts(
                postgresql.dsn_file,
                postgresql.dsn_handle,
                "storage.postgresql",
            )?;
        }
    }
    Ok(())
}

fn apply_environment(
    effective: &mut EffectiveConfig,
    environment: &BTreeMap<String, String>,
) -> Result<(), ConfigError> {
    let mut dsn_file = None;
    let mut dsn_handle = None;
    let mut touched_postgresql = false;

    for (key, value) in environment {
        match key.as_str() {
            "WORLDSTREAM__CONFIG_VERSION" => {
                effective.config_version = parse_environment(key, value)?;
            }
            "WORLDSTREAM__SERVER__BIND" => {
                effective.server.bind = parse_environment(key, value)?;
            }
            "WORLDSTREAM__STORAGE__PROFILE" => {
                effective.storage.profile = value.parse()?;
            }
            "WORLDSTREAM__STORAGE__DATA_DIR" => {
                if value.is_empty() {
                    return Err(ConfigError::InvalidEnvironment {
                        key: key.clone(),
                        value: "empty".to_owned(),
                    });
                }
                effective.storage.data_dir = PathBuf::from(value);
            }
            "WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE" => {
                touched_postgresql = true;
                if value.is_empty() {
                    return Err(ConfigError::InvalidEnvironment {
                        key: key.clone(),
                        value: "empty".to_owned(),
                    });
                }
                dsn_file = Some(PathBuf::from(value));
            }
            "WORLDSTREAM__STORAGE__POSTGRESQL__DSN_HANDLE" => {
                touched_postgresql = true;
                dsn_handle = Some(parse_environment(key, value)?);
            }
            unknown if unknown.starts_with(ENV_PREFIX) => {
                return Err(ConfigError::UnknownEnvironmentKey(unknown.to_owned()));
            }
            _ => {}
        }
    }

    if touched_postgresql {
        effective.storage.postgresql_dsn =
            secret_from_parts(dsn_file, dsn_handle, "WORLDSTREAM__STORAGE__POSTGRESQL")?;
    }
    Ok(())
}

fn apply_cli(effective: &mut EffectiveConfig, cli: &CliOverrides) {
    if let Some(bind) = cli.bind {
        effective.server.bind = bind;
    }
    if let Some(profile) = cli.storage_profile {
        effective.storage.profile = profile;
    }
    if let Some(data_dir) = &cli.data_dir {
        effective.storage.data_dir.clone_from(data_dir);
    }
}

fn parse_environment<T>(key: &str, value: &str) -> Result<T, ConfigError>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    value
        .parse()
        .map_err(|error: T::Err| ConfigError::InvalidEnvironment {
            key: key.to_owned(),
            value: error.to_string(),
        })
}

fn secret_from_parts(
    file: Option<PathBuf>,
    handle: Option<u64>,
    section: &'static str,
) -> Result<Option<SecretSource>, ConfigError> {
    match (file, handle) {
        (Some(_), Some(_)) => Err(ConfigError::MutuallyExclusiveSecrets(section)),
        (Some(path), None) => Ok(Some(SecretSource::File(path))),
        (None, Some(handle)) => Ok(Some(SecretSource::InheritedHandle(handle))),
        (None, None) => Ok(None),
    }
}

fn validate_secret(source: &SecretSource) -> Result<(), ConfigError> {
    match source {
        SecretSource::File(path) => validate_owner_only_file(path)
            .map_err(SecretValidationError::from)
            .map_err(ConfigError::Secret)?,
        SecretSource::InheritedHandle(handle) if *handle < 3 => {
            return Err(ConfigError::OutOfRange {
                key: "storage.postgresql.dsn_handle",
                value: "reserved standard handle".to_owned(),
            });
        }
        SecretSource::InheritedHandle(handle) => {
            validate_inherited_secret_handle(*handle).map_err(ConfigError::Secret)?;
        }
    }
    Ok(())
}

struct InheritedSecretDescriptor(RawFileDescriptor);

impl AsRawFileDescriptor for InheritedSecretDescriptor {
    fn as_raw_file_descriptor(&self) -> RawFileDescriptor {
        self.0
    }
}

fn validate_inherited_secret_handle(handle: u64) -> Result<(), SecretValidationError> {
    let descriptor = inherited_secret_descriptor(handle)?;
    let mut duplicate = FileDescriptor::dup(&descriptor)
        .map_err(|_| SecretValidationError::InheritedHandleNotOpen)?;

    // A zero-byte OS read checks the duplicated handle's access mode without
    // consuming, copying, or retaining any secret bytes.
    let bytes_read = duplicate
        .read(&mut [])
        .map_err(|_| SecretValidationError::InheritedHandleNotReadable)?;
    if bytes_read != 0 {
        return Err(SecretValidationError::InheritedHandleNotReadable);
    }
    Ok(())
}

#[cfg(unix)]
fn inherited_secret_descriptor(
    handle: u64,
) -> Result<InheritedSecretDescriptor, SecretValidationError> {
    let raw = i32::try_from(handle).map_err(|_| SecretValidationError::InheritedHandleNotOpen)?;
    Ok(InheritedSecretDescriptor(raw))
}

#[cfg(windows)]
fn inherited_secret_descriptor(
    handle: u64,
) -> Result<InheritedSecretDescriptor, SecretValidationError> {
    let raw = usize::try_from(handle).map_err(|_| SecretValidationError::InheritedHandleNotOpen)?;
    Ok(InheritedSecretDescriptor(raw as RawFileDescriptor))
}

fn collect_process_environment() -> Result<BTreeMap<String, String>, ConfigError> {
    let mut environment = BTreeMap::new();
    for (key, value) in env::vars_os() {
        if relevant_environment_key(&key) {
            let key = key
                .into_string()
                .map_err(|_| ConfigError::NonUnicodeEnvironment)?;
            let value = value
                .into_string()
                .map_err(|_| ConfigError::NonUnicodeEnvironment)?;
            environment.insert(key, value);
        }
    }
    Ok(environment)
}

fn relevant_environment_key(key: &OsString) -> bool {
    key.to_str()
        .is_some_and(|key| key == ENV_CONFIG_PATH || key.starts_with(ENV_PREFIX))
}

/// Strict configuration errors. Values never include secret bytes.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Selected config file could not be read.
    #[error("cannot read config file {path}: {source}")]
    ReadConfig {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// Config files are bounded before parsing.
    #[error("config file {path} is {bytes} bytes; maximum is {MAX_CONFIG_BYTES}")]
    ConfigTooLarge { path: PathBuf, bytes: u64 },
    /// TOML syntax, duplicates, unknown fields, and wrong types are fatal.
    #[error("invalid versioned TOML configuration: {0}")]
    Toml(SanitizedTomlError),
    /// The configuration contract is unsupported.
    #[error("unsupported configuration version: {0}")]
    UnsupportedConfigVersion(u32),
    /// A manifest value is unsupported by this binary.
    #[error("unsupported value for {key}: {value}")]
    UnsupportedValue { key: &'static str, value: String },
    /// A key for an inactive backend was provided.
    #[error("inactive backend configuration: {0}")]
    InactiveBackend(&'static str),
    /// A required value is absent.
    #[error("missing configuration: {0}")]
    Missing(&'static str),
    /// A numeric or address value is outside its supported range.
    #[error("out-of-range value for {key}: {value}")]
    OutOfRange { key: &'static str, value: String },
    /// A prefixed environment key is unknown.
    #[error("unknown environment configuration key: {0}")]
    UnknownEnvironmentKey(String),
    /// An environment value has a wrong type or shape.
    #[error("invalid environment value for {key}: {value}")]
    InvalidEnvironment { key: String, value: String },
    /// A relevant environment key/value cannot be decoded as UTF-8.
    #[error("WorldStream environment contains a non-Unicode key or value")]
    NonUnicodeEnvironment,
    /// The selected config path was explicitly empty.
    #[error("the explicit configuration path is empty")]
    EmptyConfigPath,
    /// Both allowed secret delivery mechanisms were selected.
    #[error("{0} must select exactly one of dsn_file or dsn_handle")]
    MutuallyExclusiveSecrets(&'static str),
    /// Secret source ownership, permission, or handle validation failed.
    #[error("secret source is not safely owner-readable: {0}")]
    Secret(#[source] SecretValidationError),
    /// The embedded compatibility artifact failed closed.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
}

/// Pathless secret diagnostics safe for stderr, logs, and error-chain debug output.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum SecretValidationError {
    /// A secret file could not be inspected without retaining its location.
    #[error("secret file is missing or inaccessible")]
    FileInaccessible,
    /// A secret path was empty, a symlink/reparse point, or not a regular file.
    #[error("secret path must name a non-symlink regular file")]
    FileType,
    /// The file is not owned by the process identity.
    #[error("secret file owner does not match the process identity")]
    FileOwner,
    /// The POSIX mode or Windows DACL permits unsafe access.
    #[error("secret file permissions are not owner-only")]
    FilePermissions,
    /// The configured raw descriptor/handle is not open in this process.
    #[error("inherited secret handle is not open in this process")]
    InheritedHandleNotOpen,
    /// The configured descriptor/handle cannot perform a zero-byte read probe.
    #[error("inherited secret handle is not readable")]
    InheritedHandleNotReadable,
    /// The platform has no fail-closed secret-source validation policy.
    #[error("secret-source validation is unsupported on this platform")]
    UnsupportedPlatform,
}

impl From<FilesystemError> for SecretValidationError {
    fn from(error: FilesystemError) -> Self {
        match error {
            FilesystemError::Io { .. } => Self::FileInaccessible,
            FilesystemError::Symlink(_)
            | FilesystemError::ReparsePoint(_)
            | FilesystemError::WrongType { .. }
            | FilesystemError::UnsafePath { .. } => Self::FileType,
            FilesystemError::Owner { .. } => Self::FileOwner,
            FilesystemError::Permissions { .. } | FilesystemError::WindowsAcl { .. } => {
                Self::FilePermissions
            }
            FilesystemError::UnsupportedPlatform => Self::UnsupportedPlatform,
        }
    }
}

/// Redacted TOML error metadata that never retains the authored source line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SanitizedTomlError {
    category: &'static str,
    line: Option<usize>,
    column: Option<usize>,
}

impl SanitizedTomlError {
    fn new(error: &toml::de::Error, source: &str) -> Self {
        let message = error.message();
        let category = if message.contains("duplicate key") || message.contains("duplicate field") {
            "duplicate key"
        } else if message.contains("unknown field") {
            "unknown key"
        } else if message.contains("invalid type") {
            "wrong value type"
        } else if message.contains("missing field") {
            "missing required key"
        } else {
            "syntax or value shape"
        };
        let (line, column) = error.span().map_or((None, None), |span| {
            let offset = span.start.min(source.len());
            let prefix = &source.as_bytes()[..offset];
            let line = prefix.split(|byte| *byte == b'\n').count();
            let column = prefix
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(prefix.len() + 1, |newline| prefix.len() - newline);
            (Some(line), Some(column))
        });
        Self {
            category,
            line,
            column,
        }
    }
}

impl fmt::Display for SanitizedTomlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.line, self.column) {
            (Some(line), Some(column)) => {
                write!(
                    formatter,
                    "{} at line {line}, column {column}",
                    self.category
                )
            }
            _ => formatter.write_str(self.category),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap, error::Error, fmt::Write as _, fs, net::SocketAddr, path::PathBuf,
    };

    use tempfile::tempdir;

    use super::{CliOverrides, ConfigError, ConfigLoader, SecretValidationError, StorageProfile};

    fn environment(items: &[(&str, &str)]) -> BTreeMap<String, String> {
        items
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[cfg(unix)]
    fn inherited_handle(file: &fs::File) -> u64 {
        use std::os::fd::AsRawFd;

        u64::try_from(file.as_raw_fd())
            .unwrap_or_else(|error| unreachable!("nonnegative test descriptor: {error}"))
    }

    #[cfg(windows)]
    fn inherited_handle(file: &fs::File) -> u64 {
        use std::os::windows::io::AsRawHandle;

        file.as_raw_handle() as usize as u64
    }

    fn error_chain(error: &ConfigError) -> String {
        let mut rendered = String::new();
        let mut current: Option<&(dyn Error + 'static)> = Some(error);
        while let Some(item) = current {
            writeln!(&mut rendered, "{item}\n{item:?}")
                .unwrap_or_else(|_| unreachable!("writing to a String cannot fail"));
            current = item.source();
        }
        rendered
    }

    #[test]
    fn compiled_default_listener_is_loopback() {
        let config = super::EffectiveConfig::default();
        assert!(config.server.bind.ip().is_loopback());
        assert_ne!(config.server.bind.port(), 0);
    }

    #[test]
    fn precedence_is_defaults_then_file_then_environment_then_cli() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let config_path = directory.path().join("config.toml");
        fs::write(
            &config_path,
            "config_version = 1\n[server]\nbind = \"127.0.0.1:9500\"\n",
        )
        .unwrap_or_else(|error| unreachable!("write fixture: {error}"));

        let cli_bind = "127.0.0.1:9700"
            .parse::<SocketAddr>()
            .unwrap_or_else(|error| unreachable!("valid socket: {error}"));
        let loader = ConfigLoader::with_environment(
            Some(config_path),
            CliOverrides {
                bind: Some(cli_bind),
                ..CliOverrides::default()
            },
            environment(&[("WORLDSTREAM__SERVER__BIND", "127.0.0.1:9600")]),
        );
        let config = loader
            .load()
            .unwrap_or_else(|error| unreachable!("valid config: {error}"));
        assert_eq!(config.server.bind, cli_bind);
        assert_eq!(config.storage.profile, StorageProfile::SqliteBundled);
    }

    #[test]
    fn cli_config_path_wins_worldstream_config() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let config_path = directory.path().join("config.toml");
        fs::write(&config_path, "config_version = 1\n")
            .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        let loader = ConfigLoader::with_environment(
            Some(config_path),
            CliOverrides::default(),
            environment(&[("WORLDSTREAM_CONFIG", "/definitely/not/present")]),
        );
        assert!(loader.load().is_ok());
    }

    #[test]
    fn rejects_unknown_duplicate_and_wrong_type_toml() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        for (name, source) in [
            ("unknown", "config_version = 1\nsurprise = true\n"),
            ("duplicate", "config_version = 1\nconfig_version = 1\n"),
            ("wrong-type", "config_version = 1\n[server]\nbind = 42\n"),
        ] {
            let config_path = directory.path().join(format!("{name}.toml"));
            fs::write(&config_path, source)
                .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
            let result = ConfigLoader::with_environment(
                Some(config_path),
                CliOverrides::default(),
                BTreeMap::new(),
            )
            .load();
            assert!(matches!(result, Err(ConfigError::Toml(_))), "{name}");
        }
    }

    #[test]
    fn toml_errors_never_echo_an_illegal_plaintext_dsn() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let config_path = directory.path().join("secret.toml");
        let sentinel = "postgres://SENTINEL_SHOULD_NEVER_ESCAPE";
        fs::write(
            &config_path,
            format!(
                "config_version = 1\n[storage]\nprofile = \"postgres-primary\"\n[storage.postgresql]\ndsn = \"{sentinel}\"\n"
            ),
        )
        .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        let error = ConfigLoader::with_environment(
            Some(config_path),
            CliOverrides::default(),
            BTreeMap::new(),
        )
        .load()
        .err()
        .unwrap_or_else(|| unreachable!("plaintext dsn must be rejected"));
        assert!(!error.to_string().contains(sentinel));
        assert!(!format!("{error:?}").contains(sentinel));
    }

    #[test]
    fn secret_file_errors_never_retain_or_echo_the_path() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let sentinel = "SENTINEL_SECRET_FILE_PATH";
        let path = directory.path().join(sentinel);
        let path_text = path.to_string_lossy().into_owned();
        let error = ConfigLoader::with_environment(
            None,
            CliOverrides::default(),
            environment(&[
                ("WORLDSTREAM__STORAGE__PROFILE", "postgres-primary"),
                ("WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE", &path_text),
            ]),
        )
        .load()
        .err()
        .unwrap_or_else(|| unreachable!("missing secret file must fail"));

        let rendered = error_chain(&error);
        assert!(rendered.contains("secret file is missing or inaccessible"));
        assert!(!rendered.contains(sentinel));
        assert!(!rendered.contains(&path_text));
    }

    #[test]
    fn loader_debug_never_echoes_secret_references() {
        let path_sentinel = "SENTINEL_SECRET_FILE_PATH";
        let handle_sentinel = "987654321";
        let loader = ConfigLoader::with_environment(
            None,
            CliOverrides::default(),
            environment(&[
                ("WORLDSTREAM__STORAGE__PROFILE", "postgres-primary"),
                ("WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE", path_sentinel),
                (
                    "WORLDSTREAM__STORAGE__POSTGRESQL__DSN_HANDLE",
                    handle_sentinel,
                ),
            ]),
        );

        let rendered = format!("{loader:?}");
        assert!(!rendered.contains(path_sentinel));
        assert!(!rendered.contains(handle_sentinel));
        assert!(rendered.contains("environment_dsn_file_present: true"));
        assert!(rendered.contains("environment_dsn_handle_present: true"));
    }

    #[test]
    fn rejects_unknown_prefixed_environment_key() {
        let result = ConfigLoader::with_environment(
            None,
            CliOverrides::default(),
            environment(&[("WORLDSTREAM__SERVER__TYPO", "true")]),
        )
        .load();
        assert!(matches!(result, Err(ConfigError::UnknownEnvironmentKey(_))));
    }

    #[test]
    fn rejects_backend_options_when_profile_is_inactive() {
        let result = ConfigLoader::with_environment(
            None,
            CliOverrides::default(),
            environment(&[("WORLDSTREAM__STORAGE__POSTGRESQL__DSN_HANDLE", "7")]),
        )
        .load();
        assert!(matches!(result, Err(ConfigError::InactiveBackend(_))));
    }

    #[test]
    fn redacted_output_never_contains_secret_path_or_handle() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let path = directory.path().join("dsn");
        fs::write(&path, "not-a-real-dsn")
            .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        let file = fs::File::open(&path)
            .unwrap_or_else(|error| unreachable!("open readable fixture: {error}"));
        let handle = inherited_handle(&file).to_string();
        let loader = ConfigLoader::with_environment(
            None,
            CliOverrides::default(),
            environment(&[
                ("WORLDSTREAM__STORAGE__PROFILE", "postgres-primary"),
                ("WORLDSTREAM__STORAGE__POSTGRESQL__DSN_HANDLE", &handle),
            ]),
        );
        let config = loader
            .load()
            .unwrap_or_else(|error| unreachable!("valid config: {error}"));
        let redacted = serde_json::to_value(config.redacted())
            .unwrap_or_else(|error| unreachable!("serialize redacted: {error}"));
        assert_eq!(
            redacted["storage"]["postgresql_dsn"]["source"],
            "inherited_handle"
        );
        assert_eq!(redacted["storage"]["postgresql_dsn"]["value"], "[REDACTED]");
    }

    #[test]
    fn rejects_an_inherited_handle_that_is_not_open() {
        let sentinel = "12345";
        let error = ConfigLoader::with_environment(
            None,
            CliOverrides::default(),
            environment(&[
                ("WORLDSTREAM__STORAGE__PROFILE", "postgres-primary"),
                ("WORLDSTREAM__STORAGE__POSTGRESQL__DSN_HANDLE", sentinel),
            ]),
        )
        .load()
        .err()
        .unwrap_or_else(|| unreachable!("unopened handle must fail"));
        assert!(matches!(
            &error,
            ConfigError::Secret(SecretValidationError::InheritedHandleNotOpen)
        ));
        assert!(!error_chain(&error).contains(sentinel));
    }

    #[test]
    fn rejects_an_inherited_handle_without_read_access() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let path = directory.path().join("write-only-dsn");
        fs::write(&path, "not-a-real-dsn")
            .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        let file = fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap_or_else(|error| unreachable!("open write-only fixture: {error}"));
        let handle = inherited_handle(&file).to_string();
        let error = ConfigLoader::with_environment(
            None,
            CliOverrides::default(),
            environment(&[
                ("WORLDSTREAM__STORAGE__PROFILE", "postgres-primary"),
                ("WORLDSTREAM__STORAGE__POSTGRESQL__DSN_HANDLE", &handle),
            ]),
        )
        .load()
        .err()
        .unwrap_or_else(|| unreachable!("write-only handle must fail"));
        assert!(matches!(
            &error,
            ConfigError::Secret(SecretValidationError::InheritedHandleNotReadable)
        ));
    }

    #[test]
    fn cli_profile_override_can_make_file_backend_options_inactive() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let config_path = directory.path().join("config.toml");
        fs::write(
            &config_path,
            "config_version = 1\n[storage]\nprofile = \"postgres-primary\"\n[storage.postgresql]\ndsn_handle = 9\n",
        )
        .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        let result = ConfigLoader::with_environment(
            Some(config_path),
            CliOverrides {
                storage_profile: Some(StorageProfile::SqliteBundled),
                data_dir: Some(PathBuf::from("test-data")),
                ..CliOverrides::default()
            },
            BTreeMap::new(),
        )
        .load();
        assert!(matches!(result, Err(ConfigError::InactiveBackend(_))));
    }
}
