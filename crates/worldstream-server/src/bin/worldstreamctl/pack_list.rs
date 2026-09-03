//! Read-only installed intent and proof-bound frozen Runtime Pack facts.

use super::cli_contract::CommandOptions;
use serde::Serialize;
use std::{env, io, path::PathBuf, time::Duration};
use worldstream_pack_bundle::{InstalledPackBundleV1, PackBundleStoreV1, PackInstallStateV1};
use worldstream_runtime::{CliOverrides, ConfigLoader};
use worldstream_server::{CommonConfigArgs, StartupPackFactsV1};
use worldstream_studio_supervisor::operator_connection::OperatorConnection;

#[derive(Debug, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum InstalledInventory {
    Available {
        evidence: InventoryEvidence,
        entries: Vec<InstalledPackBundleV1>,
    },
    Unavailable,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryEvidence {
    UnverifiedMetadata,
}

#[derive(Debug, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum RunningInventory {
    Available { facts: StartupPackFactsV1 },
    Unavailable,
    Stale,
}

#[derive(Debug, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum NextStartInventory {
    Available {
        evidence: InventoryEvidence,
        selectable_bundle_digests: Vec<String>,
        readiness_verified: bool,
    },
    Unavailable,
}

#[derive(Debug, Serialize)]
pub struct PackListSummary {
    pub installed: InstalledInventory,
    pub next_start: NextStartInventory,
    pub running: RunningInventory,
    /// None means the independent sources could not both be read.
    pub pending_changes: Option<bool>,
}

impl PackListSummary {
    #[must_use]
    pub const fn complete(&self) -> bool {
        matches!(self.installed, InstalledInventory::Available { .. })
            && matches!(self.running, RunningInventory::Available { .. })
    }

    /// Prints copyable selectors only from authenticated running facts.
    pub fn write_human(&self, output: &mut impl io::Write) -> io::Result<()> {
        match &self.installed {
            InstalledInventory::Available { entries, .. } => {
                writeln!(
                    output,
                    "Installed portable Bundles: {} (unverified metadata)",
                    entries.len()
                )?;
                for entry in entries {
                    writeln!(
                        output,
                        "  {}  revision={}  next-start={}",
                        entry.bundle_digest,
                        entry.revision_digest,
                        selection_label(entry.install_state)
                    )?;
                }
            }
            InstalledInventory::Unavailable => {
                writeln!(output, "Installed inventory: unavailable")?;
            }
        }
        match &self.next_start {
            NextStartInventory::Available { .. } => writeln!(
                output,
                "Next-start selection is retained intent; readiness was not verified."
            )?,
            NextStartInventory::Unavailable => {
                writeln!(output, "Next-start selection: unavailable")?;
            }
        }
        match &self.running {
            RunningInventory::Available { facts } => {
                writeln!(output, "Running Packs (ID@version selectors):")?;
                for pack in &facts.embedded_revisions {
                    writeln!(
                        output,
                        "  {}@{}  source=embedded  revision={}",
                        pack.id, pack.version, pack.digest
                    )?;
                }
                for pack in &facts.installed {
                    writeln!(
                        output,
                        "  {}@{}  source=portable  {}  revision={}  bundle={}",
                        pack.pack_id,
                        pack.explanatory_version,
                        selection_label(pack.install_state),
                        pack.revision_digest,
                        pack.bundle_digest
                    )?;
                }
                if facts.embedded_revisions.is_empty() && facts.installed.is_empty() {
                    writeln!(output, "  (empty)")?;
                }
            }
            RunningInventory::Unavailable => writeln!(
                output,
                "Running Packs: unavailable; no selectors inferred from disk metadata."
            )?,
            RunningInventory::Stale => writeln!(
                output,
                "Running Packs: stale or invalid evidence; no selectors inferred."
            )?,
        }
        if let Some(pending) = self.pending_changes {
            writeln!(
                output,
                "Installed intent differs from running inventory: {pending}"
            )?;
        }
        Ok(())
    }
}

const fn selection_label(state: PackInstallStateV1) -> &'static str {
    match state {
        PackInstallStateV1::Selectable => "selectable",
        PackInstallStateV1::RetainedOnly => "retained-only",
    }
}

pub fn execute(options: &CommandOptions, config: &CommonConfigArgs) -> PackListSummary {
    let installed = local_inventory(config).map_or(InstalledInventory::Unavailable, |entries| {
        InstalledInventory::Available {
            evidence: InventoryEvidence::UnverifiedMetadata,
            entries,
        }
    });
    let next_start = match &installed {
        InstalledInventory::Available { entries, .. } => NextStartInventory::Available {
            evidence: InventoryEvidence::UnverifiedMetadata,
            selectable_bundle_digests: entries
                .iter()
                .filter(|entry| entry.install_state == PackInstallStateV1::Selectable)
                .map(|entry| entry.bundle_digest.to_string())
                .collect(),
            readiness_verified: false,
        },
        InstalledInventory::Unavailable => NextStartInventory::Unavailable,
    };
    let running = running_inventory(options);
    let pending_changes = match (&installed, &running) {
        (InstalledInventory::Available { entries, .. }, RunningInventory::Available { facts }) => {
            Some(
                entries.len() != facts.installed.len()
                    || entries
                        .iter()
                        .zip(&facts.installed)
                        .any(|(desired, loaded)| {
                            desired.bundle_digest.to_string() != loaded.bundle_digest
                                || desired.revision_digest.to_string() != loaded.revision_digest
                                || desired.install_state != loaded.install_state
                        }),
            )
        }
        _ => None,
    };
    PackListSummary {
        installed,
        next_start,
        running,
        pending_changes,
    }
}

fn local_inventory(config: &CommonConfigArgs) -> Option<Vec<InstalledPackBundleV1>> {
    let selected = config
        .config
        .clone()
        .or_else(|| env::var_os("WORLDSTREAM_CONFIG").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(".worldstream/worldstream.toml"));
    let loader = ConfigLoader::from_process(
        Some(selected),
        CliOverrides {
            bind: config.bind,
            storage_profile: config.storage_profile,
            data_dir: config.data_dir.clone(),
        },
    )
    .ok()?;
    let configuration = loader
        .preview_at(&env::current_dir().ok()?)
        .ok()?
        .redacted();
    // Validation observes existing paths only; missing state is not initialized.
    let data =
        worldstream_runtime::validate_data_directory(&configuration.storage.data_dir).ok()?;
    PackBundleStoreV1::read_inventory_metadata(&data.join("activity-packs")).ok()
}

fn running_inventory(options: &CommandOptions) -> RunningInventory {
    let response = OperatorConnection::open(
        &options.state_dir,
        options.controller,
        Duration::from_secs(u64::from(options.timeout_seconds)),
    )
    .and_then(|connection| connection.request("GET", "/api/v1/control/packs", b""));
    let Ok(response) = response else {
        return RunningInventory::Unavailable;
    };
    if response.status != 200 {
        return RunningInventory::Unavailable;
    }
    let Ok(facts) = serde_json::from_slice::<StartupPackFactsV1>(&response.body) else {
        return RunningInventory::Stale;
    };
    if facts.schema != "worldstream/startup-pack-facts/v1"
        || facts.installed.len() > worldstream_pack_bundle::MAX_INSTALLED_BUNDLE_COUNT
        || facts.embedded_revisions.len() > 256
        || facts
            .inventory_digest
            .parse::<worldstream_core::Blake3DigestV1>()
            .is_err()
        || facts
            .installed
            .windows(2)
            .any(|pair| pair[0].bundle_digest >= pair[1].bundle_digest)
    {
        return RunningInventory::Stale;
    }
    RunningInventory::Available { facts }
}
