use std::{net::SocketAddr, path::PathBuf};

use clap::Args;
use worldstream_runtime::{
    CliOverrides, ConfigError, ConfigLoader, EffectiveConfig, StorageProfile,
};

/// Shared config-selection and documented non-secret CLI overrides.
#[derive(Clone, Debug, Args)]
pub struct CommonConfigArgs {
    /// One explicit versioned TOML file. Overrides `WORLDSTREAM_CONFIG`.
    #[arg(long, value_name = "FILE", global = true)]
    pub config: Option<PathBuf>,
    /// Operator listener override.
    #[arg(long, value_name = "ADDRESS", global = true)]
    pub bind: Option<SocketAddr>,
    /// Startup-fixed storage profile override.
    #[arg(long, value_name = "PROFILE", global = true)]
    pub storage_profile: Option<StorageProfile>,
    /// WorldStream-owned runtime directory override.
    #[arg(long, value_name = "DIRECTORY", global = true)]
    pub data_dir: Option<PathBuf>,
}

impl CommonConfigArgs {
    /// Loads the process config without accepting a plaintext secret flag.
    ///
    /// # Errors
    ///
    /// Returns an error when any selected configuration source fails closed.
    pub fn load(&self) -> Result<EffectiveConfig, ConfigError> {
        ConfigLoader::from_process(
            self.config.clone(),
            CliOverrides {
                bind: self.bind,
                storage_profile: self.storage_profile,
                data_dir: self.data_dir.clone(),
            },
        )?
        .load()
    }
}
