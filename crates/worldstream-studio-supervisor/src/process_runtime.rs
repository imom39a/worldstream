//! Fixed managed Runtime process adapter; no PID-based adoption or signalling.

use std::{
    fs,
    io::{self, Write as _},
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use worldstream_runtime::{
    CliOverrides, ConfigLoader, SecretSource, create_owner_only_file, validate_data_directory,
};

use crate::{
    local_initialization::validate_initialized_at,
    managed_lifecycle::{LifecycleError, RuntimeControl, RuntimeObservation},
    process_ownership::{ProcessOwnership, ProcessPhase, ProcessRole, ProcessSnapshot},
    protected_publication::{PublicationMode, publish},
    verified_control::VerifiedConnection,
};

const FACTS_FILE: &str = "managed-runtime-launch.v1.json";
const FACTS_SCHEMA: &str = "worldstream/managed-runtime-launch/v1";
const MAX_FACTS_BYTES: usize = 64 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Exact desired launch inputs, not an arbitrary executable/argument manager.
/// Paths need not still exist when inspecting or stopping a retained Runtime.
#[derive(Clone)]
pub struct ManagedRuntimeSpec {
    /// Fixed `WorldStream` daemon executable.
    pub executable: PathBuf,
    /// Explicit configuration selected by the local installation.
    pub config: PathBuf,
    /// Existing protected controller state directory.
    pub state: PathBuf,
    /// Fixed base for configuration resolution and child execution.
    pub working_directory: PathBuf,
    /// Expected literal public Runtime listener.
    pub endpoint: SocketAddr,
    /// Total bounded startup or graceful shutdown budget.
    pub timeout: Duration,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LaunchFacts {
    schema: String,
    generation: String,
    state: PathBuf,
    executable: PathBuf,
    config: PathBuf,
    working_directory: PathBuf,
    data_directory: PathBuf,
    endpoint: SocketAddr,
    policy_digest: String,
}

impl LaunchFacts {
    fn same_policy(&self, other: &Self) -> bool {
        self.state == other.state
            && self.executable == other.executable
            && self.config == other.config
            && self.working_directory == other.working_directory
            && self.data_directory == other.data_directory
            && self.endpoint == other.endpoint
            && self.policy_digest == other.policy_digest
    }
}

/// Process adapter beneath the durable managed lifecycle coordinator.
/// Dropping or reopening it never shuts down the Runtime.
pub struct ProcessRuntimeControl {
    spec: ManagedRuntimeSpec,
    ownership: ProcessOwnership,
    mutation: Mutex<()>,
}

impl ProcessRuntimeControl {
    /// Opens only protected retained state. Does not read current configuration,
    /// validate current executables, create files, or start any process.
    ///
    /// # Errors
    /// Rejects unsafe state, invalid fixed inputs, and malformed launch facts.
    pub fn open(mut spec: ManagedRuntimeSpec) -> Result<Self, LifecycleError> {
        spec.state = validate_data_directory(&spec.state).map_err(|_| LifecycleError::Invalid)?;
        if !spec.executable.is_absolute()
            || !spec.config.is_absolute()
            || !spec.working_directory.is_absolute()
            || !spec.endpoint.ip().is_loopback()
            || spec.endpoint.port() == 0
            || spec.timeout.is_zero()
            || spec.timeout > Duration::from_mins(5)
        {
            return Err(LifecycleError::Invalid);
        }
        let ownership = ProcessOwnership::open(&spec.state).map_err(|_| LifecycleError::Invalid)?;
        let control = Self {
            spec,
            ownership,
            mutation: Mutex::new(()),
        };
        control.read_facts()?;
        Ok(control)
    }

    fn desired_facts(&self) -> Result<LaunchFacts, LifecycleError> {
        let executable =
            fs::canonicalize(&self.spec.executable).map_err(|_| LifecycleError::Invalid)?;
        if !fs::metadata(&executable)
            .map_err(|_| LifecycleError::Invalid)?
            .is_file()
        {
            return Err(LifecycleError::Invalid);
        }
        let config = fs::canonicalize(&self.spec.config).map_err(|_| LifecycleError::Invalid)?;
        let working_directory =
            fs::canonicalize(&self.spec.working_directory).map_err(|_| LifecycleError::Invalid)?;
        let loader =
            ConfigLoader::with_environment(Some(config.clone()), CliOverrides::default(), []);
        let policy = loader
            .preview_at(&working_directory)
            .and_then(|prospective| prospective.installation_toml(&working_directory))
            .map_err(|_| LifecycleError::Invalid)?;
        let effective = validate_initialized_at(&loader, &self.spec.state, &working_directory)
            .map_err(|_| LifecycleError::Invalid)?;
        if effective.server.bind != self.spec.endpoint {
            return Err(LifecycleError::Invalid);
        }
        let data_directory = validate_data_directory(&effective.storage.data_dir)
            .map_err(|_| LifecycleError::Invalid)?;
        Ok(LaunchFacts {
            schema: FACTS_SCHEMA.to_owned(),
            generation: String::new(),
            state: self.spec.state.clone(),
            executable,
            config,
            working_directory,
            data_directory,
            endpoint: effective.server.bind,
            // The normalized declaration includes file references, never resolved secret bytes.
            policy_digest: blake3::hash(policy.as_bytes()).to_hex().to_string(),
        })
    }

    fn read_facts(&self) -> Result<Option<LaunchFacts>, LifecycleError> {
        let path = self.spec.state.join(FACTS_FILE);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(LifecycleError::Unavailable),
            Ok(_) => {}
        }
        let bytes = SecretSource::File(path)
            .read_bounded(MAX_FACTS_BYTES)
            .map_err(|_| LifecycleError::Invalid)?;
        let facts: LaunchFacts =
            serde_json::from_slice(&bytes).map_err(|_| LifecycleError::Invalid)?;
        if facts.schema != FACTS_SCHEMA
            || facts.state != self.spec.state
            || !valid_digest(&facts.generation)
            || !valid_digest(&facts.policy_digest)
            || !facts.executable.is_absolute()
            || !facts.config.is_absolute()
            || !facts.working_directory.is_absolute()
            || !facts.data_directory.is_absolute()
            || !facts.endpoint.ip().is_loopback()
            || facts.endpoint.port() == 0
        {
            return Err(LifecycleError::Invalid);
        }
        Ok(Some(facts))
    }

    fn publish_facts(&self, facts: &LaunchFacts) -> Result<(), LifecycleError> {
        let target = self.spec.state.join(FACTS_FILE);
        let mode = if self.read_facts()?.is_some() {
            PublicationMode::Replace
        } else {
            PublicationMode::CreateNew
        };
        let temporary = self
            .spec
            .state
            .join(format!(".runtime-launch-{}.tmp", facts.generation));
        let mut file = create_owner_only_file(&temporary).map_err(|_| LifecycleError::Invalid)?;
        let bytes = serde_json::to_vec(facts).map_err(|_| LifecycleError::Invalid)?;
        if bytes.len() > MAX_FACTS_BYTES {
            return Err(LifecycleError::Invalid);
        }
        let result = match file.write_all(&bytes) {
            Ok(()) => publish(file, &temporary, &target, mode),
            Err(error) => {
                drop(file);
                Err(error)
            }
        };
        let _ = fs::remove_file(&temporary);
        result.map_err(|_| LifecycleError::PublicationUncertain)
    }

    fn retained(&self) -> Result<Option<(ProcessSnapshot, LaunchFacts)>, LifecycleError> {
        let snapshot = self
            .ownership
            .snapshot(ProcessRole::Runtime)
            .map_err(|_| LifecycleError::Invalid)?;
        let facts = self.read_facts()?;
        match (snapshot, facts) {
            (None, None) => Ok(None),
            (Some(snapshot), Some(facts))
                if snapshot.generation == facts.generation
                    && snapshot
                        .endpoint
                        .is_none_or(|endpoint| endpoint == facts.endpoint) =>
            {
                Ok(Some((snapshot, facts)))
            }
            _ => Err(LifecycleError::Invalid),
        }
    }

    fn ready(&self, budget: Duration) -> bool {
        if !self.retained().is_ok_and(|retained| {
            retained.is_some_and(|(snapshot, _)| snapshot.phase == ProcessPhase::Ready)
        }) {
            return false;
        }
        let response = VerifiedConnection::connect(&self.ownership, ProcessRole::Runtime, budget)
            .and_then(|connection| {
                connection.request_runtime("GET", "/api/v1/control/status", b"")
            });
        let Ok(response) = response else {
            return false;
        };
        response.status == 200
            && serde_json::from_slice::<RuntimeStatus>(&response.body).is_ok_and(|body| {
                body.schema == "worldstream/managed-runtime-status/v1" && body.ready
            })
    }

    fn launch(&self, facts: &LaunchFacts) -> Result<Child, LifecycleError> {
        let mut command = Command::new(&facts.executable);
        for (key, _) in std::env::vars_os() {
            if is_worldstream_override(&key) {
                command.env_remove(key);
            }
        }
        command
            .arg("--config")
            .arg(&facts.config)
            .arg("--managed-state-dir")
            .arg(&self.spec.state)
            .arg("--managed-generation")
            .arg(&facts.generation)
            .current_dir(&facts.working_directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Unix managed worldstreamd calls safe setsid itself, before threads.
        // Making it a process-group leader here would prevent that detachment.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(0x0000_0008 | 0x0000_0200);
        }
        command.spawn().map_err(|_| LifecycleError::Unavailable)
    }
}

impl RuntimeControl for ProcessRuntimeControl {
    fn observe(&self) -> RuntimeObservation {
        let Ok(retained) = self.retained() else {
            return RuntimeObservation::Unmanaged;
        };
        let Ok(held) = self.ownership.is_leased(ProcessRole::Runtime) else {
            return RuntimeObservation::Unmanaged;
        };
        match retained {
            None if !held => stopped_or_foreign(self.spec.endpoint),
            Some((snapshot, facts))
                if !held
                    && matches!(snapshot.phase, ProcessPhase::Stopped | ProcessPhase::Failed) =>
            {
                stopped_or_foreign(facts.endpoint)
            }
            Some((snapshot, _)) if held && snapshot.phase == ProcessPhase::Starting => {
                RuntimeObservation::Starting
            }
            Some((snapshot, _)) if held && snapshot.phase == ProcessPhase::Ready => {
                if self.ready(PROBE_TIMEOUT) {
                    RuntimeObservation::Ready
                } else {
                    RuntimeObservation::Unavailable
                }
            }
            _ => RuntimeObservation::Unmanaged,
        }
    }

    fn start(&self) -> Result<(), LifecycleError> {
        let _mutation = self
            .mutation
            .lock()
            .map_err(|_| LifecycleError::Unavailable)?;
        let mut facts = self.desired_facts()?;
        if self
            .read_facts()?
            .is_some_and(|retained| !retained.same_policy(&facts))
        {
            return Err(LifecycleError::Invalid);
        }
        match self.observe() {
            RuntimeObservation::Ready => return Ok(()),
            RuntimeObservation::Stopped => {}
            RuntimeObservation::Unmanaged => {
                // A process host can die either while a launch is still starting
                // or after the Runtime published Ready. Explicit start may fence
                // that exact generation only while its permanent lease is free
                // and its selected endpoint is closed. The child must claim the
                // lease before storage, so a delayed or surviving child prevents
                // cancellation. Retained PID and phase never imply ownership.
                let abandoned = self
                    .ownership
                    .snapshot(ProcessRole::Runtime)
                    .map_err(|_| LifecycleError::Unavailable)?
                    .ok_or(LifecycleError::Unavailable)?;
                if !recoverable_abandoned_runtime(
                    &abandoned,
                    self.ownership.is_leased(ProcessRole::Runtime),
                    stopped_or_foreign(facts.endpoint),
                ) {
                    return Err(LifecycleError::Unavailable);
                }
                self.ownership
                    .cancel_abandoned(ProcessRole::Runtime, &abandoned.generation)
                    .map_err(|_| LifecycleError::Unavailable)?;
            }
            _ => return Err(LifecycleError::Unavailable),
        }
        let launch = self
            .ownership
            .reserve(ProcessRole::Runtime)
            .map_err(|_| LifecycleError::Unavailable)?;
        launch.generation().clone_into(&mut facts.generation);
        self.publish_facts(&facts)?;
        let mut child = match self.launch(&facts) {
            Ok(child) => child,
            Err(error) => {
                let _ = self
                    .ownership
                    .cancel_abandoned(ProcessRole::Runtime, &facts.generation);
                return Err(error);
            }
        };
        let deadline = Instant::now() + self.spec.timeout;
        loop {
            if child
                .try_wait()
                .map_err(|_| LifecycleError::Unavailable)?
                .is_some()
            {
                let _ = self
                    .ownership
                    .cancel_abandoned(ProcessRole::Runtime, &facts.generation);
                return Err(LifecycleError::Unavailable);
            }
            let Some(remaining) = deadline
                .checked_duration_since(Instant::now())
                .filter(|value| !value.is_zero())
            else {
                let _ = self
                    .ownership
                    .cancel_abandoned(ProcessRole::Runtime, &facts.generation);
                reap(child);
                return Err(LifecycleError::Unavailable);
            };
            if self.ready(remaining.min(PROBE_TIMEOUT)) {
                reap(child);
                return Ok(());
            }
            thread::sleep(POLL_INTERVAL.min(remaining));
        }
    }

    fn stop(&self) -> Result<(), LifecycleError> {
        let _mutation = self
            .mutation
            .lock()
            .map_err(|_| LifecycleError::Unavailable)?;
        if self.observe() == RuntimeObservation::Stopped {
            return Ok(());
        }
        // A retained endpoint and even a valid proof do not repair incomplete
        // installation ownership. Check the permanent lifetime lease before
        // sending any derived control authority or shutdown request.
        if self.ownership.is_leased(ProcessRole::Runtime) != Ok(true) {
            return Err(LifecycleError::Unavailable);
        }
        let (snapshot, _) = self.retained()?.ok_or(LifecycleError::Unavailable)?;
        if snapshot.phase != ProcessPhase::Ready {
            return Err(LifecycleError::Unavailable);
        }
        let response =
            VerifiedConnection::connect(&self.ownership, ProcessRole::Runtime, PROBE_TIMEOUT)
                .and_then(|connection| {
                    connection.request_runtime("POST", "/api/v1/control/stop", b"{}")
                })
                .map_err(|_| LifecycleError::Unavailable)?;
        if response.status != 202
            || !serde_json::from_slice::<StopAccepted>(&response.body).is_ok_and(|body| {
                body.schema == "worldstream/managed-runtime-stop/v1" && body.accepted
            })
        {
            return Err(LifecycleError::Unavailable);
        }
        let deadline = Instant::now() + self.spec.timeout;
        loop {
            if !self
                .ownership
                .is_leased(ProcessRole::Runtime)
                .map_err(|_| LifecycleError::Unavailable)?
            {
                return if self
                    .ownership
                    .snapshot(ProcessRole::Runtime)
                    .map_err(|_| LifecycleError::Invalid)?
                    .is_some_and(|current| {
                        current.generation == snapshot.generation
                            && current.phase == ProcessPhase::Stopped
                    }) {
                    Ok(())
                } else {
                    Err(LifecycleError::Unavailable)
                };
            }
            if Instant::now() >= deadline {
                return Err(LifecycleError::Unavailable);
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeStatus {
    schema: String,
    ready: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StopAccepted {
    schema: String,
    accepted: bool,
}

fn stopped_or_foreign(endpoint: SocketAddr) -> RuntimeObservation {
    match TcpStream::connect_timeout(&endpoint, PROBE_TIMEOUT) {
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
            RuntimeObservation::Stopped
        }
        Ok(_) => RuntimeObservation::Unmanaged,
        Err(_) => RuntimeObservation::Unavailable,
    }
}

fn recoverable_abandoned_runtime(
    snapshot: &ProcessSnapshot,
    leased: Result<bool, crate::process_ownership::OwnershipError>,
    endpoint: RuntimeObservation,
) -> bool {
    matches!(snapshot.phase, ProcessPhase::Starting | ProcessPhase::Ready)
        && leased == Ok(false)
        && endpoint == RuntimeObservation::Stopped
}

fn reap(mut child: Child) {
    // Reap eventual exit without making Runtime lifetime depend on this thread
    // or on Controller survival. No kill-on-drop behavior is installed.
    thread::spawn(move || {
        let _ = child.wait();
    });
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(unix)]
fn is_worldstream_override(key: &std::ffi::OsStr) -> bool {
    use std::os::unix::ffi::OsStrExt as _;
    key.as_bytes() == b"WORLDSTREAM_CONFIG" || key.as_bytes().starts_with(b"WORLDSTREAM__")
}

#[cfg(windows)]
fn is_worldstream_override(key: &std::ffi::OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt as _;
    let uppercase = || {
        key.encode_wide().map(|unit| {
            if (97..=122).contains(&unit) {
                unit - 32
            } else {
                unit
            }
        })
    };
    uppercase().eq("WORLDSTREAM_CONFIG".encode_utf16())
        || uppercase()
            .take("WORLDSTREAM__".len())
            .eq("WORLDSTREAM__".encode_utf16())
}

#[cfg(not(any(unix, windows)))]
fn is_worldstream_override(_: &std::ffi::OsStr) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::{ProcessPhase, ProcessSnapshot, RuntimeObservation, recoverable_abandoned_runtime};
    use crate::process_ownership::OwnershipError;

    fn snapshot(phase: ProcessPhase) -> ProcessSnapshot {
        ProcessSnapshot {
            generation: "a".repeat(64),
            phase,
            pid: Some(42),
            endpoint: Some("127.0.0.1:9410".parse().expect("fixture endpoint")),
        }
    }

    #[test]
    fn released_ready_runtime_with_closed_endpoint_is_recoverable_after_host_restart() {
        assert!(recoverable_abandoned_runtime(
            &snapshot(ProcessPhase::Starting),
            Ok(false),
            RuntimeObservation::Stopped,
        ));
        assert!(recoverable_abandoned_runtime(
            &snapshot(ProcessPhase::Ready),
            Ok(false),
            RuntimeObservation::Stopped,
        ));
        assert!(!recoverable_abandoned_runtime(
            &snapshot(ProcessPhase::Ready),
            Ok(true),
            RuntimeObservation::Stopped,
        ));
        assert!(!recoverable_abandoned_runtime(
            &snapshot(ProcessPhase::Ready),
            Ok(false),
            RuntimeObservation::Unmanaged,
        ));
        assert!(!recoverable_abandoned_runtime(
            &snapshot(ProcessPhase::Ready),
            Err(OwnershipError::Unavailable),
            RuntimeObservation::Stopped,
        ));
        assert!(!recoverable_abandoned_runtime(
            &snapshot(ProcessPhase::Stopped),
            Ok(false),
            RuntimeObservation::Stopped,
        ));
    }
}
