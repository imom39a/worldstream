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
const MAX_TELEMETRY_ENDPOINT_BYTES: usize = 256;
const MAX_DEPLOYMENT_LINEAGE_BYTES: usize = 128;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Whether a browser origin is an exact local HTTP origin admitted by the
/// participant handoff contract (without a path or query).
#[must_use]
pub fn is_exact_loopback_origin(value: &str) -> bool {
    let Ok(uri) = value.parse::<http::Uri>() else {
        return false;
    };
    uri.scheme_str() == Some("http")
        && uri.path() == "/"
        && uri.query().is_none()
        && uri.authority().is_some()
        && matches!(
            uri.host(),
            Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
        )
}

/// Whether a managed participant origin can be passed unchanged to the browser
/// and the current Controller, whose legacy Studio origin remains reserved.
#[must_use]
pub fn is_managed_participant_origin(value: &str) -> bool {
    if !is_exact_loopback_origin(value) || value == "http://127.0.0.1:5174" {
        return false;
    }
    let Ok(uri) = value.parse::<http::Uri>() else {
        return false;
    };
    let Some(host) = uri.host() else {
        return false;
    };
    let canonical = match uri.port_u16() {
        Some(port) if port != 80 => format!("http://{host}:{port}"),
        _ => format!("http://{host}"),
    };
    value == canonical
}

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

    /// Reads exactly one 256-bit secret without exposing its source or bytes
    /// in diagnostics. The source is revalidated immediately before opening
    /// a file or duplicating an inherited handle.
    ///
    /// # Errors
    ///
    /// Returns a pathless error when the source is unavailable, unreadable, or
    /// does not contain exactly 32 bytes.
    pub fn read_exact_256(&self) -> Result<[u8; 32], SecretValidationError> {
        match self {
            Self::File(path) => {
                validate_owner_only_file(path).map_err(SecretValidationError::from)?;
                let file =
                    fs::File::open(path).map_err(|_| SecretValidationError::MaterialUnavailable)?;
                read_exact_256_from(file)
            }
            Self::InheritedHandle(handle) => {
                validate_inherited_secret_handle(*handle)?;
                let descriptor = inherited_secret_descriptor(*handle)?;
                let duplicate = FileDescriptor::dup(&descriptor)
                    .map_err(|_| SecretValidationError::InheritedHandleNotOpen)?;
                read_exact_256_from(duplicate)
            }
        }
    }

    /// Reads a non-empty secret payload up to the caller's explicit byte
    /// bound. This is intended for variable-length process secrets such as a
    /// database DSN; diagnostics never include the source or material.
    ///
    /// # Errors
    ///
    /// Returns a pathless error when the source is unavailable, unreadable,
    /// empty, or larger than `maximum_bytes`.
    pub fn read_bounded(&self, maximum_bytes: usize) -> Result<Vec<u8>, SecretValidationError> {
        if maximum_bytes == 0 {
            return Err(SecretValidationError::MaterialLength);
        }
        match self {
            Self::File(path) => {
                validate_owner_only_file(path).map_err(SecretValidationError::from)?;
                let file =
                    fs::File::open(path).map_err(|_| SecretValidationError::MaterialUnavailable)?;
                read_bounded_from(file, maximum_bytes)
            }
            Self::InheritedHandle(handle) => {
                validate_inherited_secret_handle(*handle)?;
                let descriptor = inherited_secret_descriptor(*handle)?;
                let duplicate = FileDescriptor::dup(&descriptor)
                    .map_err(|_| SecretValidationError::InheritedHandleNotOpen)?;
                read_bounded_from(duplicate, maximum_bytes)
            }
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

/// Operator-supplied deployment lineage used by canonical storage metadata.
///
/// The value is an opaque, bounded ASCII identity. It is intentionally not
/// rendered by `Debug` or redacted configuration output because it may be
/// high-cardinality deployment identity.
#[derive(Clone, Eq, PartialEq)]
pub struct DeploymentLineageV1(String);

impl DeploymentLineageV1 {
    /// Parses the bounded canonical lineage shape.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is empty, too long, non-ASCII, or
    /// contains a character outside the canonical identity alphabet.
    pub fn parse(value: &str) -> Result<Self, ConfigError> {
        let bytes = value.as_bytes();
        if bytes.is_empty()
            || bytes.len() > MAX_DEPLOYMENT_LINEAGE_BYTES
            || !bytes[0].is_ascii_alphanumeric()
            || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
            || bytes.iter().any(|byte| {
                !(byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'/'))
            })
        {
            return Err(ConfigError::InvalidDeploymentLineage);
        }
        Ok(Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for DeploymentLineageV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeploymentLineageV1([REDACTED])")
    }
}

/// Operator-supplied, nonzero JavaScript-safe storage epoch.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct StorageEpochV1(u64);

impl StorageEpochV1 {
    /// Constructs an epoch in the supported nonzero safe-integer range.
    ///
    /// # Errors
    ///
    /// Returns an error for zero or a value above the JavaScript-safe integer
    /// maximum.
    pub const fn new(value: u64) -> Result<Self, ConfigError> {
        if value == 0 || value > MAX_SAFE_INTEGER {
            Err(ConfigError::InvalidStorageEpoch)
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Debug for StorageEpochV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StorageEpochV1([REDACTED])")
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
    /// Explicit deployment lineage for canonical export metadata.
    pub deployment_lineage: Option<DeploymentLineageV1>,
    /// Explicit storage epoch for canonical export metadata.
    pub storage_epoch: Option<StorageEpochV1>,
}

/// Startup authority configuration. The bearer is delivered out of band and
/// is never represented as a TOML, CLI, environment, or effective-config
/// value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorityConfig {
    /// Owner-readable secret source used only for the first host authority.
    pub bootstrap_secret: Option<SecretSource>,
}

/// Optional, vendor-neutral OTLP/HTTP destination. The daemon may choose a
/// transport for this endpoint; configuration validation never performs a
/// network request and the endpoint is never rendered in redacted output.
#[derive(Clone, Eq, PartialEq)]
pub struct TelemetryEndpointV1(String);

impl TelemetryEndpointV1 {
    /// Validates an OTLP/HTTP endpoint without accepting credentials or
    /// query/fragment material that could carry secrets.
    pub fn parse(value: &str) -> Result<Self, ConfigError> {
        if value.is_empty()
            || value.len() > MAX_TELEMETRY_ENDPOINT_BYTES
            || value
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
            || !(value.starts_with("http://") || value.starts_with("https://"))
            || value.contains('?')
            || value.contains('#')
        {
            return Err(ConfigError::InvalidTelemetryEndpoint);
        }
        let authority = value
            .split_once("://")
            .and_then(|(_, rest)| {
                rest.split_once('/')
                    .map_or(Some(rest), |(host, _)| Some(host))
            })
            .ok_or(ConfigError::InvalidTelemetryEndpoint)?;
        if authority.is_empty()
            || authority.contains('@')
            || authority.contains('%')
            || authority.contains('\\')
        {
            return Err(ConfigError::InvalidTelemetryEndpoint);
        }
        validate_telemetry_authority(authority)?;
        Ok(Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for TelemetryEndpointV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TelemetryEndpointV1([REDACTED])")
    }
}

/// A non-secret startup diagnostic for telemetry configuration.
///
/// Telemetry is deliberately best-effort: an invalid endpoint must select the
/// bounded structured-log path rather than prevent the process from starting.
/// The rejected value is never retained, rendered, or included in an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TelemetryConfigDiagnosticV1 {
    InvalidEndpoint,
}

impl TelemetryConfigDiagnosticV1 {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidEndpoint => "invalid_endpoint",
        }
    }
}

/// Startup telemetry options. An absent endpoint selects the local structured
/// log path; this config object does not imply collector delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelemetryConfig {
    pub otlp_endpoint: Option<TelemetryEndpointV1>,
    /// Safe diagnostic explaining why an optional exporter was not selected.
    pub diagnostic: Option<TelemetryConfigDiagnosticV1>,
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
    /// Startup authority configuration.
    pub authority: AuthorityConfig,
    /// Optional post-commit telemetry destination.
    pub telemetry: TelemetryConfig,
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
                deployment_lineage: None,
                storage_epoch: None,
            },
            authority: AuthorityConfig {
                bootstrap_secret: None,
            },
            telemetry: TelemetryConfig {
                otlp_endpoint: None,
                diagnostic: None,
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
                deployment_lineage: self
                    .storage
                    .deployment_lineage
                    .as_ref()
                    .map(|_| "[CONFIGURED]"),
                storage_epoch: self.storage.storage_epoch.map(|_| "[CONFIGURED]"),
            },
            authority: RedactedAuthorityConfig {
                bootstrap_secret: self.authority.bootstrap_secret.as_ref().map(|secret| {
                    RedactedSecretSource {
                        source: secret.kind(),
                        value: "[REDACTED]",
                    }
                }),
            },
            telemetry: RedactedTelemetryConfig {
                otlp_endpoint: self
                    .telemetry
                    .otlp_endpoint
                    .as_ref()
                    .map(|_| "[CONFIGURED]"),
                diagnostic: self
                    .telemetry
                    .diagnostic
                    .map(TelemetryConfigDiagnosticV1::code),
            },
        }
    }

    fn validate(&self) -> Result<(), ConfigError> {
        self.validate_without_bootstrap_secret()?;

        if let Some(source) = &self.authority.bootstrap_secret {
            validate_bootstrap_secret(source, "authority.bootstrap.secret_handle")?;
        }

        Ok(())
    }

    fn validate_without_bootstrap_secret(&self) -> Result<(), ConfigError> {
        self.validate_with_dsn_check(|source| {
            validate_secret(source, "storage.postgresql.dsn_handle")
        })
    }

    fn validate_structure(&self) -> Result<(), ConfigError> {
        self.validate_with_dsn_check(|_| Ok(()))
    }

    fn validate_with_dsn_check(
        &self,
        check_dsn: impl FnOnce(&SecretSource) -> Result<(), ConfigError>,
    ) -> Result<(), ConfigError> {
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
            (StorageProfile::PostgresPrimary, Some(source)) => check_dsn(source)?,
            (StorageProfile::SqliteBundled, None) => {}
        }

        match (
            &self.storage.deployment_lineage,
            &self.storage.storage_epoch,
        ) {
            (Some(_), Some(_)) | (None, None) => {}
            _ => return Err(ConfigError::PartialDeploymentMetadata),
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
    /// Redacted authority section.
    pub authority: RedactedAuthorityConfig,
    /// Redacted optional telemetry destination.
    pub telemetry: RedactedTelemetryConfig,
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
    /// Constant marker when an explicit deployment lineage was supplied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployment_lineage: Option<&'static str>,
    /// Constant marker when an explicit storage epoch was supplied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_epoch: Option<&'static str>,
}

/// Authority output without the bootstrap path, handle, or bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RedactedAuthorityConfig {
    /// Redacted first-host authority secret source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bootstrap_secret: Option<RedactedSecretSource>,
}

/// Telemetry output without endpoint authority/path or credentials.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RedactedTelemetryConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otlp_endpoint: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<&'static str>,
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

/// Structurally checked planning configuration, not authorized for startup.
/// Secret files and inherited handles have not been opened or validated.
#[derive(Clone, Debug)]
pub struct ProspectiveConfig(EffectiveConfig);

impl ProspectiveConfig {
    /// Serializes a new durable installation configuration, not a diagnostic view.
    /// File references are retained but secret bytes are never opened or copied.
    /// Inherited sources cannot be persisted for a later process.
    ///
    /// # Errors
    /// Rejects inherited sources, unsafe base paths, or serialization failures.
    pub fn installation_toml(&self, working_directory: &Path) -> Result<String, ConfigError> {
        if !working_directory.is_absolute() {
            return Err(ConfigError::InitializationPersistence);
        }
        let absolute = |path: &Path| {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                working_directory.join(path)
            }
        };
        let file_reference = |source: &SecretSource| match source {
            SecretSource::File(path) => Ok(absolute(path)),
            SecretSource::InheritedHandle(_) => Err(ConfigError::InitializationPersistence),
        };
        let effective = &self.0;
        let bootstrap = effective
            .authority
            .bootstrap_secret
            .as_ref()
            .map(file_reference)
            .transpose()?;
        let dsn = effective
            .storage
            .postgresql_dsn
            .as_ref()
            .map(file_reference)
            .transpose()?;
        let file = FileConfig {
            config_version: effective.config_version,
            server: Some(FileServerConfig {
                bind: Some(effective.server.bind),
            }),
            storage: Some(FileStorageConfig {
                profile: Some(effective.storage.profile),
                data_dir: Some(absolute(&effective.storage.data_dir)),
                deployment_lineage: effective
                    .storage
                    .deployment_lineage
                    .as_ref()
                    .map(|lineage| lineage.as_str().to_owned()),
                storage_epoch: effective.storage.storage_epoch.map(StorageEpochV1::get),
                postgresql: dsn.map(|path| FilePostgresqlConfig {
                    dsn_file: Some(path),
                    dsn_handle: None,
                }),
            }),
            authority: bootstrap.map(|path| FileAuthorityConfig {
                bootstrap: Some(FileBootstrapConfig {
                    secret_file: Some(path),
                    secret_handle: None,
                }),
            }),
            telemetry: effective.telemetry.otlp_endpoint.as_ref().map(|endpoint| {
                FileTelemetryConfig {
                    otlp: Some(FileOtlpConfig {
                        endpoint: endpoint.0.clone(),
                    }),
                }
            }),
        };
        toml::to_string(&file).map_err(|_| ConfigError::InitializationPersistence)
    }
    /// Planned non-secret runtime directory.
    #[must_use]
    pub fn data_directory(&self) -> &Path {
        &self.0.storage.data_dir
    }

    /// Planned storage profile.
    #[must_use]
    pub const fn storage_profile(&self) -> StorageProfile {
        self.0.storage.profile
    }

    /// Unvalidated bootstrap reference; never its secret material.
    #[must_use]
    pub const fn bootstrap_source(&self) -> Option<&SecretSource> {
        self.0.authority.bootstrap_secret.as_ref()
    }

    /// Unvalidated `PostgreSQL` DSN reference, never its secret material.
    #[must_use]
    pub const fn postgresql_dsn_source(&self) -> Option<&SecretSource> {
        self.0.storage.postgresql_dsn.as_ref()
    }

    /// Safe configuration inspection without opening secret sources.
    #[must_use]
    pub fn redacted(&self) -> RedactedConfig {
        self.0.redacted()
    }
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
            .field(
                "environment_bootstrap_file_present",
                &self
                    .environment
                    .contains_key("WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE"),
            )
            .field(
                "environment_bootstrap_handle_present",
                &self
                    .environment
                    .contains_key("WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_HANDLE"),
            )
            .field(
                "environment_deployment_lineage_present",
                &self
                    .environment
                    .contains_key("WORLDSTREAM__STORAGE__DEPLOYMENT_LINEAGE"),
            )
            .field(
                "environment_storage_epoch_present",
                &self
                    .environment
                    .contains_key("WORLDSTREAM__STORAGE__STORAGE_EPOCH"),
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
    /// Returns an error for any unsupported, insecure, inactive, missing,
    /// unknown, or out-of-range value. Optional telemetry endpoint syntax is
    /// intentionally a typed fallback diagnostic, because telemetry cannot
    /// make the authoritative process fail to start.
    pub fn load(&self) -> Result<EffectiveConfig, ConfigError> {
        let effective = self.load_before_secret_validation()?;
        effective.validate()?;
        Ok(effective)
    }

    /// Plans all configuration layers without opening any secret source.
    ///
    /// # Errors
    /// Returns file, syntax, precedence, or structural configuration errors.
    pub fn preview(&self) -> Result<ProspectiveConfig, ConfigError> {
        let effective = self.load_before_secret_validation()?;
        effective.validate_structure()?;
        Ok(ProspectiveConfig(effective))
    }

    /// Plans configuration relative to an explicitly captured working directory.
    /// Selected config, data, bootstrap, and DSN file paths use the same base.
    /// No secret source is opened. Legacy `preview` and `load` are unchanged.
    ///
    /// # Errors
    /// Rejects a non-absolute base or invalid configuration structure.
    pub fn preview_at(&self, working_directory: &Path) -> Result<ProspectiveConfig, ConfigError> {
        let effective = self.load_before_secret_validation_at(Some(working_directory))?;
        effective.validate_structure()?;
        Ok(ProspectiveConfig(effective))
    }

    /// Fully validates configuration using an explicit working-directory base.
    /// Unlike planning, this opens and validates configured secret sources.
    ///
    /// # Errors
    /// Rejects an invalid base, configuration, or unavailable secret source.
    pub fn load_at(&self, working_directory: &Path) -> Result<EffectiveConfig, ConfigError> {
        let effective = self.load_before_secret_validation_at(Some(working_directory))?;
        effective.validate()?;
        Ok(effective)
    }

    /// Loads all configuration layers, prepares a configured bootstrap source,
    /// and then performs complete final validation.
    ///
    /// Trusted local launchers use this to create an owner-only first-install
    /// bootstrap file without duplicating configuration precedence. The
    /// callback sees layered configuration only after non-secret invariants
    /// pass, cannot alter it, and never bypasses final validation.
    ///
    /// # Errors
    ///
    /// Returns configuration validation errors or the callback's preparation
    /// error. Bootstrap material is always validated before success.
    pub fn load_with_bootstrap_preparation(
        &self,
        prepare: impl FnOnce(&EffectiveConfig) -> Result<(), ConfigError>,
    ) -> Result<EffectiveConfig, ConfigError> {
        let effective = self.load_before_secret_validation()?;
        effective.validate_without_bootstrap_secret()?;
        prepare(&effective)?;
        effective.validate()?;
        Ok(effective)
    }

    fn load_before_secret_validation(&self) -> Result<EffectiveConfig, ConfigError> {
        self.load_before_secret_validation_at(None)
    }

    fn load_before_secret_validation_at(
        &self,
        working_directory: Option<&Path>,
    ) -> Result<EffectiveConfig, ConfigError> {
        if working_directory.is_some_and(|base| !base.is_absolute()) {
            return Err(ConfigError::InvalidWorkingDirectory);
        }
        let mut effective = EffectiveConfig::default();
        if let Some(path) = self.selected_config_path()? {
            let resolved = working_directory.map(|base| base.join(path));
            apply_file(&mut effective, resolved.as_deref().unwrap_or(path))?;
        }
        apply_environment(&mut effective, &self.environment)?;
        apply_cli(&mut effective, &self.cli_overrides);
        if let Some(base) = working_directory {
            // Keep empty paths empty so normal validation still rejects them.
            let resolve = |path: &mut PathBuf| {
                if !path.as_os_str().is_empty() && path.is_relative() {
                    *path = base.join(&*path);
                }
            };
            resolve(&mut effective.storage.data_dir);
            for source in [
                &mut effective.storage.postgresql_dsn,
                &mut effective.authority.bootstrap_secret,
            ] {
                if let Some(SecretSource::File(path)) = source {
                    resolve(path);
                }
            }
        }
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

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    config_version: u32,
    #[serde(default)]
    server: Option<FileServerConfig>,
    #[serde(default)]
    storage: Option<FileStorageConfig>,
    #[serde(default)]
    authority: Option<FileAuthorityConfig>,
    #[serde(default)]
    telemetry: Option<FileTelemetryConfig>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FileTelemetryConfig {
    #[serde(default)]
    otlp: Option<FileOtlpConfig>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FileOtlpConfig {
    endpoint: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FileAuthorityConfig {
    #[serde(default)]
    bootstrap: Option<FileBootstrapConfig>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FileBootstrapConfig {
    #[serde(default)]
    secret_file: Option<PathBuf>,
    #[serde(default)]
    secret_handle: Option<u64>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FileServerConfig {
    #[serde(default)]
    bind: Option<SocketAddr>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FileStorageConfig {
    #[serde(default)]
    profile: Option<StorageProfile>,
    #[serde(default)]
    data_dir: Option<PathBuf>,
    #[serde(default)]
    deployment_lineage: Option<String>,
    #[serde(default)]
    storage_epoch: Option<u64>,
    #[serde(default)]
    postgresql: Option<FilePostgresqlConfig>,
}

#[derive(Deserialize, Serialize)]
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
        if let Some(deployment_lineage) = storage.deployment_lineage {
            effective.storage.deployment_lineage =
                Some(DeploymentLineageV1::parse(&deployment_lineage)?);
        }
        if let Some(storage_epoch) = storage.storage_epoch {
            effective.storage.storage_epoch = Some(StorageEpochV1::new(storage_epoch)?);
        }
        if let Some(postgresql) = storage.postgresql {
            effective.storage.postgresql_dsn = secret_from_parts(
                postgresql.dsn_file,
                postgresql.dsn_handle,
                "storage.postgresql",
            )?;
        }
    }
    if let Some(authority) = file.authority
        && let Some(bootstrap) = authority.bootstrap
    {
        effective.authority.bootstrap_secret = secret_from_parts(
            bootstrap.secret_file,
            bootstrap.secret_handle,
            "authority.bootstrap",
        )?;
    }
    if let Some(telemetry) = file.telemetry
        && let Some(otlp) = telemetry.otlp
    {
        let (endpoint, diagnostic) = telemetry_config_value(&otlp.endpoint);
        effective.telemetry.otlp_endpoint = endpoint;
        effective.telemetry.diagnostic = diagnostic;
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
    let mut bootstrap_file = None;
    let mut bootstrap_handle = None;
    let mut touched_bootstrap = false;
    let mut telemetry_endpoint = None;
    let mut telemetry_diagnostic = None;
    let mut touched_telemetry = false;
    let mut deployment_lineage = None;
    let mut storage_epoch = None;

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
            "WORLDSTREAM__STORAGE__DEPLOYMENT_LINEAGE" => {
                deployment_lineage = Some(DeploymentLineageV1::parse(value)?);
            }
            "WORLDSTREAM__STORAGE__STORAGE_EPOCH" => {
                storage_epoch = Some(StorageEpochV1::new(parse_environment(key, value)?)?);
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
            "WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE" => {
                touched_bootstrap = true;
                if value.is_empty() {
                    return Err(ConfigError::InvalidEnvironment {
                        key: key.clone(),
                        value: "empty".to_owned(),
                    });
                }
                bootstrap_file = Some(PathBuf::from(value));
            }
            "WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_HANDLE" => {
                touched_bootstrap = true;
                bootstrap_handle = Some(parse_environment(key, value)?);
            }
            "WORLDSTREAM__TELEMETRY__OTLP__ENDPOINT" => {
                touched_telemetry = true;
                (telemetry_endpoint, telemetry_diagnostic) = telemetry_config_value(value);
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
    if touched_bootstrap {
        effective.authority.bootstrap_secret = secret_from_parts(
            bootstrap_file,
            bootstrap_handle,
            "WORLDSTREAM__AUTHORITY__BOOTSTRAP",
        )?;
    }
    if let Some(deployment_lineage) = deployment_lineage {
        effective.storage.deployment_lineage = Some(deployment_lineage);
    }
    if let Some(storage_epoch) = storage_epoch {
        effective.storage.storage_epoch = Some(storage_epoch);
    }
    if touched_telemetry {
        effective.telemetry.otlp_endpoint = telemetry_endpoint;
        effective.telemetry.diagnostic = telemetry_diagnostic;
    }
    Ok(())
}

fn telemetry_config_value(
    value: &str,
) -> (
    Option<TelemetryEndpointV1>,
    Option<TelemetryConfigDiagnosticV1>,
) {
    if let Ok(endpoint) = TelemetryEndpointV1::parse(value) {
        (Some(endpoint), None)
    } else {
        (None, Some(TelemetryConfigDiagnosticV1::InvalidEndpoint))
    }
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

fn validate_telemetry_authority(authority: &str) -> Result<(), ConfigError> {
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, suffix) = rest
            .split_once(']')
            .ok_or(ConfigError::InvalidTelemetryEndpoint)?;
        if host.is_empty() || !host.contains(':') {
            return Err(ConfigError::InvalidTelemetryEndpoint);
        }
        let port = match suffix {
            "" => None,
            suffix => Some(
                suffix
                    .strip_prefix(':')
                    .ok_or(ConfigError::InvalidTelemetryEndpoint)?,
            ),
        };
        (host, port)
    } else if authority.matches(':').count() > 1 {
        return Err(ConfigError::InvalidTelemetryEndpoint);
    } else {
        authority
            .split_once(':')
            .map_or((authority, None), |(host, port)| (host, Some(port)))
    };
    if host.is_empty() || host.starts_with('.') || host.ends_with('.') {
        return Err(ConfigError::InvalidTelemetryEndpoint);
    }
    if let Some(port) = port {
        let port = port
            .parse::<u16>()
            .map_err(|_| ConfigError::InvalidTelemetryEndpoint)?;
        if port == 0 {
            return Err(ConfigError::InvalidTelemetryEndpoint);
        }
    }
    Ok(())
}

fn validate_secret(source: &SecretSource, handle_key: &'static str) -> Result<(), ConfigError> {
    match source {
        SecretSource::File(path) => validate_owner_only_file(path)
            .map_err(SecretValidationError::from)
            .map_err(ConfigError::Secret)?,
        SecretSource::InheritedHandle(handle) if *handle < 3 => {
            return Err(ConfigError::OutOfRange {
                key: handle_key,
                value: "reserved standard handle".to_owned(),
            });
        }
        SecretSource::InheritedHandle(handle) => {
            validate_inherited_secret_handle(*handle).map_err(ConfigError::Secret)?;
        }
    }
    Ok(())
}

fn validate_bootstrap_secret(
    source: &SecretSource,
    handle_key: &'static str,
) -> Result<(), ConfigError> {
    validate_secret(source, handle_key)?;
    source
        .read_exact_256()
        .map(|_| ())
        .map_err(ConfigError::Secret)
}

fn read_exact_256_from(mut reader: impl Read) -> Result<[u8; 32], SecretValidationError> {
    let mut bytes = [0_u8; 33];
    let mut filled = 0;
    while filled < bytes.len() {
        match reader.read(&mut bytes[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(_) => {
                bytes.fill(0);
                return Err(SecretValidationError::MaterialUnavailable);
            }
        }
    }
    if filled != 32 {
        bytes.fill(0);
        return Err(SecretValidationError::MaterialLength);
    }
    let mut secret = [0_u8; 32];
    secret.copy_from_slice(&bytes[..32]);
    bytes.fill(0);
    Ok(secret)
}

fn read_bounded_from(
    reader: impl Read,
    maximum_bytes: usize,
) -> Result<Vec<u8>, SecretValidationError> {
    let capacity = maximum_bytes
        .checked_add(1)
        .ok_or(SecretValidationError::MaterialLength)?;
    let mut bytes = Vec::with_capacity(capacity.min(16 * 1024 + 1));
    reader
        .take(u64::try_from(capacity).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|_| SecretValidationError::MaterialUnavailable)?;
    if bytes.is_empty() || bytes.len() > maximum_bytes {
        bytes.fill(0);
        return Err(SecretValidationError::MaterialLength);
    }
    Ok(bytes)
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
    /// Explicit initialization bases cannot depend on the process CWD.
    #[error("captured working directory must be absolute")]
    InvalidWorkingDirectory,
    /// New durable config cannot preserve an inherited descriptor across processes.
    #[error("new installation configuration requires durable file secret references")]
    InitializationPersistence,
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
    /// The optional OTLP endpoint is not a safe HTTP(S) authority.
    #[error("invalid telemetry OTLP endpoint")]
    InvalidTelemetryEndpoint,
    /// A required value is absent.
    #[error("missing configuration: {0}")]
    Missing(&'static str),
    /// Deployment lineage was not in the bounded canonical shape.
    #[error("deployment lineage is not in the bounded canonical shape")]
    InvalidDeploymentLineage,
    /// Storage epoch was not a nonzero safe integer.
    #[error("storage epoch must be between 1 and 9007199254740991")]
    InvalidStorageEpoch,
    /// Exactly one deployment metadata field was supplied.
    #[error("deployment lineage and storage epoch must be supplied together")]
    PartialDeploymentMetadata,
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
    #[error("{0} must select exactly one secret file or inherited handle")]
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
    /// The source could not be read without retaining source details.
    #[error("secret material is unavailable or unreadable")]
    MaterialUnavailable,
    /// Bootstrap material must be exactly one 256-bit value.
    #[error("secret material must contain exactly 32 bytes")]
    MaterialLength,
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
            | FilesystemError::UnsafePath { .. }
            | FilesystemError::FilesystemIdentityUnavailable { .. }
            | FilesystemError::UnsupportedFilesystem { .. } => Self::FileType,
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

    use crate::{create_owner_only_file, prepare_data_directory};

    use super::{
        CliOverrides, ConfigError, ConfigLoader, SecretSource, SecretValidationError,
        StorageProfile, TelemetryConfigDiagnosticV1, TelemetryEndpointV1,
    };

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
        assert!(config.telemetry.otlp_endpoint.is_none());
        assert!(config.storage.deployment_lineage.is_none());
        assert!(config.storage.storage_epoch.is_none());
    }

    #[test]
    fn deployment_metadata_layers_from_versioned_toml_and_environment() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let config_path = directory.path().join("deployment.toml");
        fs::write(
            &config_path,
            "config_version = 1\n[storage]\ndeployment_lineage = \"deployment/from-file\"\nstorage_epoch = 7\n",
        )
        .unwrap_or_else(|error| unreachable!("write config: {error}"));
        let config = ConfigLoader::with_environment(
            Some(config_path),
            CliOverrides::default(),
            environment(&[(
                "WORLDSTREAM__STORAGE__DEPLOYMENT_LINEAGE",
                "deployment/from-environment",
            )]),
        )
        .load()
        .unwrap_or_else(|error| unreachable!("valid deployment metadata: {error}"));

        assert_eq!(
            config
                .storage
                .deployment_lineage
                .as_ref()
                .map(super::DeploymentLineageV1::as_str),
            Some("deployment/from-environment")
        );
        assert_eq!(
            config.storage.storage_epoch.map(super::StorageEpochV1::get),
            Some(7)
        );
    }

    #[test]
    fn deployment_metadata_rejects_invalid_and_partial_values() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let config_path = directory.path().join("partial.toml");
        fs::write(
            &config_path,
            "config_version = 1\n[storage]\ndeployment_lineage = \"deployment/only\"\n",
        )
        .unwrap_or_else(|error| unreachable!("write config: {error}"));
        assert!(matches!(
            ConfigLoader::with_environment(
                Some(config_path),
                CliOverrides::default(),
                BTreeMap::new()
            )
            .load(),
            Err(ConfigError::PartialDeploymentMetadata)
        ));

        let invalid_lineage = ConfigLoader::with_environment(
            None,
            CliOverrides::default(),
            environment(&[(
                "WORLDSTREAM__STORAGE__DEPLOYMENT_LINEAGE",
                "deployment with spaces",
            )]),
        )
        .load();
        assert!(matches!(
            invalid_lineage,
            Err(ConfigError::InvalidDeploymentLineage)
        ));

        let invalid_epoch = ConfigLoader::with_environment(
            None,
            CliOverrides::default(),
            environment(&[("WORLDSTREAM__STORAGE__STORAGE_EPOCH", "0")]),
        )
        .load();
        assert!(matches!(
            invalid_epoch,
            Err(ConfigError::InvalidStorageEpoch)
        ));
    }

    #[test]
    fn deployment_metadata_debug_and_redacted_output_never_echo_identity() {
        let lineage = "deployment/secret-high-cardinality-identity";
        let config = ConfigLoader::with_environment(
            None,
            CliOverrides::default(),
            environment(&[
                ("WORLDSTREAM__STORAGE__DEPLOYMENT_LINEAGE", lineage),
                ("WORLDSTREAM__STORAGE__STORAGE_EPOCH", "9007199254740991"),
            ]),
        )
        .load()
        .unwrap_or_else(|error| unreachable!("valid deployment metadata: {error}"));

        assert!(!format!("{config:?}").contains(lineage));
        let redacted = serde_json::to_string(&config.redacted())
            .unwrap_or_else(|error| unreachable!("redacted deployment metadata: {error}"));
        assert!(redacted.contains("[CONFIGURED]"));
        assert!(!redacted.contains(lineage));
    }

    #[test]
    fn telemetry_endpoint_is_layered_and_redacted() {
        let loader = ConfigLoader::with_environment(
            None,
            CliOverrides::default(),
            environment(&[(
                "WORLDSTREAM__TELEMETRY__OTLP__ENDPOINT",
                "http://127.0.0.1:4318/v1/logs",
            )]),
        );
        let config = loader
            .load()
            .unwrap_or_else(|error| unreachable!("valid telemetry config: {error}"));
        assert_eq!(
            config
                .telemetry
                .otlp_endpoint
                .as_ref()
                .map(TelemetryEndpointV1::as_str),
            Some("http://127.0.0.1:4318/v1/logs")
        );
        assert!(!format!("{config:?}").contains("v1/logs"));
        let redacted = serde_json::to_string(&config.redacted())
            .unwrap_or_else(|error| unreachable!("redacted telemetry config: {error}"));
        assert!(redacted.contains("[CONFIGURED]"));
        assert!(!redacted.contains("v1/logs"));
    }

    #[test]
    fn telemetry_endpoint_rejects_credentials_queries_bad_ports_and_unsupported_schemes() {
        for value in [
            "ftp://collector:4318",
            "http://user:password@collector:4318",
            "http://collector:4318?token=secret",
            "http://collector:0",
            "http://collector:65536",
            "http://::1:4318",
        ] {
            let config = ConfigLoader::with_environment(
                None,
                CliOverrides::default(),
                environment(&[("WORLDSTREAM__TELEMETRY__OTLP__ENDPOINT", value)]),
            )
            .load()
            .unwrap_or_else(|error| unreachable!("telemetry fallback must load: {error}"));
            assert_eq!(config.telemetry.otlp_endpoint, None, "{value}");
            assert_eq!(
                config.telemetry.diagnostic,
                Some(TelemetryConfigDiagnosticV1::InvalidEndpoint),
                "{value}"
            );
            let redacted = serde_json::to_string(&config.redacted())
                .unwrap_or_else(|error| unreachable!("redacted telemetry config: {error}"));
            assert!(redacted.contains("invalid_endpoint"));
            assert!(!redacted.contains(value));
        }
        assert!(TelemetryEndpointV1::parse("http://[::1]:4318/v1/logs").is_ok());
    }

    #[test]
    fn malformed_file_telemetry_config_falls_back_without_retaining_endpoint() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let sentinel = "http://user:password@collector:4318/v1/logs?token=secret-marker";
        let config_path = directory.path().join("malformed-telemetry.toml");
        fs::write(
            &config_path,
            format!("config_version = 1\n[telemetry.otlp]\nendpoint = \"{sentinel}\"\n"),
        )
        .unwrap_or_else(|error| unreachable!("write malformed telemetry config: {error}"));
        let config = ConfigLoader::with_environment(
            Some(config_path),
            CliOverrides::default(),
            BTreeMap::new(),
        )
        .load()
        .unwrap_or_else(|error| unreachable!("telemetry fallback must load: {error}"));
        assert!(config.telemetry.otlp_endpoint.is_none());
        assert_eq!(
            config.telemetry.diagnostic,
            Some(TelemetryConfigDiagnosticV1::InvalidEndpoint)
        );
        assert!(!format!("{config:?}").contains(sentinel));
        let redacted = serde_json::to_string(&config.redacted())
            .unwrap_or_else(|error| unreachable!("redacted telemetry config: {error}"));
        assert!(redacted.contains("invalid_endpoint"));
        assert!(!redacted.contains(sentinel));
        assert!(!redacted.contains("password"));
    }

    #[test]
    fn telemetry_endpoint_can_be_loaded_from_versioned_toml() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let config_path = directory.path().join("telemetry.toml");
        fs::write(
            &config_path,
            "config_version = 1\n[telemetry.otlp]\nendpoint = \"http://collector:4318/v1/logs\"\n",
        )
        .unwrap_or_else(|error| unreachable!("write telemetry config: {error}"));
        let config = ConfigLoader::with_environment(
            Some(config_path),
            CliOverrides::default(),
            BTreeMap::new(),
        )
        .load()
        .unwrap_or_else(|error| unreachable!("valid telemetry TOML: {error}"));
        assert_eq!(
            config
                .telemetry
                .otlp_endpoint
                .as_ref()
                .map(TelemetryEndpointV1::as_str),
            Some("http://collector:4318/v1/logs")
        );
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

    #[cfg(unix)]
    #[test]
    fn authority_bootstrap_source_is_redacted_and_reads_exact_material() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let path = directory.path().join("authority-bootstrap");
        let path_text = path.to_string_lossy().into_owned();
        fs::write(&path, [0x5a_u8; 32])
            .unwrap_or_else(|error| unreachable!("authority secret: {error}"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|error| unreachable!("authority permissions: {error}"));
        let config = ConfigLoader::with_environment(
            None,
            CliOverrides::default(),
            environment(&[("WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE", &path_text)]),
        )
        .load()
        .unwrap_or_else(|error| unreachable!("valid authority config: {error}"));
        let source = config
            .authority
            .bootstrap_secret
            .as_ref()
            .unwrap_or_else(|| unreachable!("authority source"));
        assert_eq!(source.kind(), "owner_readable_secret_file");
        assert_eq!(
            source
                .read_exact_256()
                .unwrap_or_else(|error| unreachable!("authority secret: {error}")),
            [0x5a_u8; 32]
        );

        let redacted = serde_json::to_string(&config.redacted())
            .unwrap_or_else(|error| unreachable!("redacted config: {error}"));
        assert!(redacted.contains("owner_readable_secret_file"));
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains(&path_text));
        assert!(!redacted.contains("5a5a5a"));

        let direct = SecretSource::File(path);
        assert_eq!(direct.kind(), "owner_readable_secret_file");
    }

    #[cfg(unix)]
    #[test]
    fn bootstrap_preparation_can_create_a_missing_configured_source_before_final_validation() {
        use std::io::Write as _;

        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let source_directory = prepare_data_directory(&directory.path().join("bootstrap-source"))
            .unwrap_or_else(|error| unreachable!("prepare bootstrap source directory: {error}"));
        let path = source_directory.join("fresh-authority-bootstrap");
        let path_text = path.to_string_lossy().into_owned();
        let loader = ConfigLoader::with_environment(
            None,
            CliOverrides::default(),
            environment(&[("WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE", &path_text)]),
        );

        let config = loader
            .load_with_bootstrap_preparation(|effective| {
                let source = effective
                    .authority
                    .bootstrap_secret
                    .as_ref()
                    .unwrap_or_else(|| unreachable!("configured bootstrap source"));
                let SecretSource::File(source_path) = source else {
                    unreachable!("configured bootstrap source must be a file");
                };
                assert_eq!(source_path, &path);
                let mut file = create_owner_only_file(source_path)
                    .unwrap_or_else(|error| unreachable!("create bootstrap source: {error}"));
                file.write_all(&[0x4f; 32])
                    .and_then(|()| file.sync_all())
                    .unwrap_or_else(|error| unreachable!("write bootstrap source: {error}"));
                Ok(())
            })
            .unwrap_or_else(|error| unreachable!("prepared config: {error}"));

        assert_eq!(
            config
                .authority
                .bootstrap_secret
                .as_ref()
                .unwrap_or_else(|| unreachable!("prepared bootstrap source"))
                .read_exact_256()
                .unwrap_or_else(|error| unreachable!("prepared bootstrap material: {error}")),
            [0x4f; 32]
        );
    }

    #[cfg(unix)]
    #[test]
    fn bootstrap_preparation_still_rejects_malformed_material_during_final_validation() {
        use std::io::Write as _;

        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let source_directory = prepare_data_directory(&directory.path().join("bootstrap-source"))
            .unwrap_or_else(|error| unreachable!("prepare bootstrap source directory: {error}"));
        let path = source_directory.join("malformed-fresh-authority-bootstrap");
        let path_text = path.to_string_lossy().into_owned();
        let loader = ConfigLoader::with_environment(
            None,
            CliOverrides::default(),
            environment(&[("WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE", &path_text)]),
        );

        let error = loader
            .load_with_bootstrap_preparation(|effective| {
                let source = effective
                    .authority
                    .bootstrap_secret
                    .as_ref()
                    .unwrap_or_else(|| unreachable!("configured bootstrap source"));
                let SecretSource::File(source_path) = source else {
                    unreachable!("configured bootstrap source must be a file");
                };
                assert_eq!(source_path, &path);
                let mut file = create_owner_only_file(source_path)
                    .unwrap_or_else(|error| unreachable!("create bootstrap source: {error}"));
                file.write_all(&[0x51; 31])
                    .and_then(|()| file.sync_all())
                    .unwrap_or_else(|error| unreachable!("write bootstrap source: {error}"));
                Ok(())
            })
            .err()
            .unwrap_or_else(|| unreachable!("malformed prepared source must fail"));

        assert!(matches!(
            error,
            ConfigError::Secret(SecretValidationError::MaterialLength)
        ));
    }

    #[test]
    fn invalid_non_secret_configuration_never_invokes_bootstrap_preparation() {
        let loader = ConfigLoader::with_environment(
            None,
            CliOverrides::default(),
            environment(&[("WORLDSTREAM__STORAGE__PROFILE", "unsupported")]),
        );
        let mut invoked = false;

        let error = loader
            .load_with_bootstrap_preparation(|_| {
                invoked = true;
                Ok(())
            })
            .err()
            .unwrap_or_else(|| unreachable!("invalid storage profile must fail"));

        assert!(matches!(error, ConfigError::UnsupportedValue { .. }));
        assert!(!invoked);
    }

    #[cfg(unix)]
    #[test]
    fn authority_bootstrap_inherited_handle_reads_exact_material() {
        use std::os::fd::AsRawFd;

        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let path = directory.path().join("authority-bootstrap-handle");
        fs::write(&path, [0x6b_u8; 32])
            .unwrap_or_else(|error| unreachable!("authority secret: {error}"));
        let file = fs::File::open(&path)
            .unwrap_or_else(|error| unreachable!("authority secret handle: {error}"));
        let source = SecretSource::InheritedHandle(
            u64::try_from(file.as_raw_fd())
                .unwrap_or_else(|error| unreachable!("nonnegative file descriptor: {error}")),
        );
        assert_eq!(
            source
                .read_exact_256()
                .unwrap_or_else(|error| unreachable!("authority secret: {error}")),
            [0x6b_u8; 32]
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_bootstrap_secret_with_wrong_material_length_at_config_load() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let path = directory.path().join("short-authority-bootstrap");
        let path_text = path.to_string_lossy().into_owned();
        fs::write(&path, [0x2a_u8; 31])
            .unwrap_or_else(|error| unreachable!("authority secret: {error}"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|error| unreachable!("authority permissions: {error}"));

        let error = ConfigLoader::with_environment(
            None,
            CliOverrides::default(),
            environment(&[("WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE", &path_text)]),
        )
        .load()
        .err()
        .unwrap_or_else(|| unreachable!("wrong-length bootstrap secret must fail closed"));
        assert!(matches!(
            error,
            ConfigError::Secret(SecretValidationError::MaterialLength)
        ));
        assert!(!error_chain(&error).contains(&path_text));
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
