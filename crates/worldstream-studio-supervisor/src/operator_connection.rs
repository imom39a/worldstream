//! Explicit Controller startup and proof-bound operator requests.

use crate::{
    control_access::ControlAccess,
    local_initialization::validate_initialized_at,
    process_ownership::{ProcessOwnership, ProcessPhase, ProcessRole},
    protected_publication::{PublicationMode, publish},
    session_diagnostics::{HOSTED_SESSION_DIAGNOSTIC_FILE, prepare_file_sink},
    verified_control::{ControlResponse, VerifiedConnection},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write as _},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;
use worldstream_runtime::{
    ConfigLoader, SecretSource, create_owner_only_file, validate_data_directory,
    validate_owner_only_file,
};

/// Closed local control failure; process output and authority never escape.
#[derive(Clone, Copy, Debug, Error)]
pub enum OperatorConnectionError {
    #[error("the protected local Controller is unavailable")]
    Unavailable,
    #[error("managed Controller configuration or ownership is invalid")]
    Invalid,
    #[error("managed startup publication or completion is uncertain")]
    Incomplete,
}

/// Read-only connection selection. Construction never creates state or services.
pub struct OperatorConnection {
    state: PathBuf,
    endpoint: SocketAddr,
    timeout: Duration,
    participant_console_origin: Option<String>,
}

/// Fixed shipped executables; not an arbitrary command launcher.
pub struct ControllerExecutables {
    pub controller: PathBuf,
    pub runtime: PathBuf,
    pub assignment_mcp: PathBuf,
}

const SELECTION_FILE: &str = "managed-controller-config.v1.json";

#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ConfigurationSelection {
    schema: String,
    state: PathBuf,
    original: PathBuf,
    working_directory: PathBuf,
    policy_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    participant_console_origin: Option<String>,
}

const DEFAULT_PARTICIPANT_CONSOLE_ORIGIN: &str = "http://127.0.0.1:5173";

impl OperatorConnection {
    /// Opens existing protected installation state without starting processes.
    ///
    /// # Errors
    /// Rejects unsafe paths, non-loopback endpoints and unbounded timeouts.
    pub fn open(
        state: &Path,
        endpoint: SocketAddr,
        timeout: Duration,
    ) -> Result<Self, OperatorConnectionError> {
        if !endpoint.ip().is_loopback()
            || endpoint.port() == 0
            || timeout.is_zero()
            || timeout > Duration::from_mins(5)
        {
            return Err(OperatorConnectionError::Invalid);
        }
        let state = validate_data_directory(state).map_err(|_| OperatorConnectionError::Invalid)?;
        Ok(Self {
            state,
            endpoint,
            timeout,
            participant_console_origin: None,
        })
    }

    /// Selects an explicit participant browser origin for Controller startup.
    /// Omission reuses the retained selection, or the default on first startup.
    ///
    /// # Errors
    /// Rejects origins outside the existing exact local HTTP origin contract.
    pub fn with_participant_console_origin(
        mut self,
        origin: Option<&str>,
    ) -> Result<Self, OperatorConnectionError> {
        if origin.is_some_and(|value| !worldstream_runtime::is_managed_participant_origin(value)) {
            return Err(OperatorConnectionError::Invalid);
        }
        self.participant_console_origin = origin.map(str::to_owned);
        Ok(self)
    }

    /// Sends one request on the exact socket whose generation was proved.
    ///
    /// # Errors
    /// No credential is sent to an unverified process or retried on another socket.
    pub fn request(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<ControlResponse, OperatorConnectionError> {
        self.request_with_timeout(method, path, body, self.timeout)
    }

    fn request_with_timeout(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
        timeout: Duration,
    ) -> Result<ControlResponse, OperatorConnectionError> {
        let ownership =
            ProcessOwnership::open(&self.state).map_err(|_| OperatorConnectionError::Invalid)?;
        let snapshot = ownership
            .snapshot(ProcessRole::Controller)
            .map_err(|_| OperatorConnectionError::Invalid)?
            .ok_or(OperatorConnectionError::Unavailable)?;
        if snapshot.endpoint != Some(self.endpoint)
            || !ownership
                .is_leased(ProcessRole::Controller)
                .map_err(|_| OperatorConnectionError::Invalid)?
        {
            return Err(OperatorConnectionError::Unavailable);
        }
        VerifiedConnection::connect(&ownership, ProcessRole::Controller, timeout)
            .and_then(|connection| {
                connection.request(method, path, body, || {
                    ControlAccess::open(&self.state)?.authorization_header()
                })
            })
            .map_err(|_| OperatorConnectionError::Unavailable)
    }

    /// Selects the original file, or its exact retained normalized snapshot when
    /// that original file is absent. An invalid or different explicit file never
    /// silently falls back. This read-only operation starts no process.
    ///
    /// # Errors
    /// Rejects damaged protected metadata, changed selection, or a missing snapshot.
    pub fn start_config_path(
        &self,
        requested: &Path,
        working_directory: &Path,
    ) -> Result<PathBuf, OperatorConnectionError> {
        let original = selected_path(requested, working_directory)?;
        match fs::symlink_metadata(&original) {
            Ok(_) => return Ok(original),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err(OperatorConnectionError::Invalid),
        }
        let selection = self
            .read_selection()?
            .ok_or(OperatorConnectionError::Invalid)?;
        if selection.original != original
            || selection.working_directory
                != fs::canonicalize(working_directory)
                    .map_err(|_| OperatorConnectionError::Invalid)?
        {
            return Err(OperatorConnectionError::Invalid);
        }
        let snapshot = self.state.join("managed-runtime.toml");
        let bytes = SecretSource::File(snapshot.clone())
            .read_bounded(1024 * 1024)
            .map_err(|_| OperatorConnectionError::Invalid)?;
        if blake3::hash(&bytes).to_hex().as_str() != selection.policy_digest {
            return Err(OperatorConnectionError::Invalid);
        }
        Ok(snapshot)
    }

    fn read_selection(&self) -> Result<Option<ConfigurationSelection>, OperatorConnectionError> {
        let path = self.state.join(SELECTION_FILE);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(OperatorConnectionError::Invalid),
            Ok(_) => {}
        }
        let bytes = SecretSource::File(path)
            .read_bounded(32 * 1024)
            .map_err(|_| OperatorConnectionError::Invalid)?;
        let selection: ConfigurationSelection =
            serde_json::from_slice(&bytes).map_err(|_| OperatorConnectionError::Invalid)?;
        if selection.schema != "worldstream/controller-configuration/v1"
            || selection.state != self.state
            || !selection.original.is_absolute()
            || !selection.working_directory.is_absolute()
            || selection.policy_digest.len() != 64
            || !selection
                .policy_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || selection
                .participant_console_origin
                .as_deref()
                .is_some_and(|origin| !worldstream_runtime::is_managed_participant_origin(origin))
        {
            return Err(OperatorConnectionError::Invalid);
        }
        Ok(Some(selection))
    }

    /// Explicitly starts or reuses the configured Controller, never the Runtime.
    /// The caller must then request the managed Runtime start operation.
    ///
    /// # Errors
    /// Invalid/drifted configuration, foreign listeners, damaged ownership, child
    /// failure or timeout remain errors. No PID-based adoption or termination occurs.
    #[allow(
        clippy::too_many_lines,
        reason = "keep explicit startup validation and publication order visible"
    )]
    pub fn ensure_started(
        &self,
        loader: &ConfigLoader,
        requested_config: &Path,
        working_directory: &Path,
        executables: &ControllerExecutables,
    ) -> Result<(), OperatorConnectionError> {
        let started = Instant::now();
        let effective = validate_initialized_at(loader, &self.state, working_directory)
            .map_err(|_| OperatorConnectionError::Invalid)?;
        if !effective.server.bind.ip().is_loopback() || effective.server.bind.port() == 0 {
            return Err(OperatorConnectionError::Invalid);
        }
        // Serialize launchers until the child claims its lifetime lease and
        // proves readiness. A delayed child is not an abandoned launch while
        // another caller still holds this distinct, permanent startup lock.
        let _startup = self.lock_startup(started)?;
        let normalized = loader
            .preview_at(working_directory)
            .and_then(|config| config.installation_toml(working_directory))
            .map_err(|_| OperatorConnectionError::Invalid)?;
        let config = self.state.join("managed-runtime.toml");
        let retained_selection = self.read_selection()?;
        let retained_origin = retained_selection.as_ref().map(|selection| {
            selection
                .participant_console_origin
                .as_deref()
                .unwrap_or(DEFAULT_PARTICIPANT_CONSOLE_ORIGIN)
        });
        if self
            .participant_console_origin
            .as_deref()
            .is_some_and(|requested| retained_origin.is_some_and(|retained| requested != retained))
        {
            return Err(OperatorConnectionError::Invalid);
        }
        let origin = self
            .participant_console_origin
            .as_deref()
            .or(retained_origin)
            .unwrap_or(DEFAULT_PARTICIPANT_CONSOLE_ORIGIN);
        let selection = ConfigurationSelection {
            schema: "worldstream/controller-configuration/v1".into(),
            state: self.state.clone(),
            original: selected_path(requested_config, working_directory)?,
            working_directory: fs::canonicalize(working_directory)
                .map_err(|_| OperatorConnectionError::Invalid)?,
            policy_digest: blake3::hash(normalized.as_bytes()).to_hex().to_string(),
            participant_console_origin: retained_selection.as_ref().map_or_else(
                || Some(origin.to_owned()),
                |selection| selection.participant_console_origin.clone(),
            ),
        };
        if retained_selection
            .as_ref()
            .is_some_and(|retained| retained != &selection)
        {
            return Err(OperatorConnectionError::Invalid);
        }
        let existing = match fs::symlink_metadata(&config) {
            Ok(_) => Some(
                SecretSource::File(config.clone())
                    .read_bounded(1024 * 1024)
                    .map_err(|_| OperatorConnectionError::Invalid)?,
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(_) => return Err(OperatorConnectionError::Invalid),
        };
        if existing
            .as_ref()
            .is_some_and(|bytes| bytes.as_slice() != normalized.as_bytes())
        {
            return Err(OperatorConnectionError::Invalid);
        }
        if self
            .request_with_timeout(
                "GET",
                "/api/v1/control/server/status",
                b"",
                Duration::from_secs(1),
            )
            .is_ok_and(|response| response.status == 200)
        {
            return Ok(());
        }
        let ownership =
            ProcessOwnership::open(&self.state).map_err(|_| OperatorConnectionError::Invalid)?;
        let record = ownership
            .snapshot(ProcessRole::Controller)
            .map_err(|_| OperatorConnectionError::Invalid)?;
        if record.is_some() && (existing.is_none() || retained_selection.is_none()) {
            return Err(OperatorConnectionError::Invalid);
        }
        let leased = ownership
            .is_leased(ProcessRole::Controller)
            .map_err(|_| OperatorConnectionError::Invalid)?;
        if leased {
            return self.wait_ready(started);
        }
        // A free lifetime lease and a refused endpoint permit explicit recovery
        // after a crash; retained Ready metadata alone must not block forever.
        // Never fence a generation while its selected endpoint is occupied.
        match TcpStream::connect_timeout(&self.endpoint, Duration::from_millis(250)) {
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {}
            _ => return Err(OperatorConnectionError::Unavailable),
        }
        if let Some(record) = &record {
            if matches!(record.phase, ProcessPhase::Starting | ProcessPhase::Ready) {
                ownership
                    .cancel_abandoned(ProcessRole::Controller, &record.generation)
                    .map_err(|_| OperatorConnectionError::Invalid)?;
            } else if !matches!(record.phase, ProcessPhase::Stopped | ProcessPhase::Failed) {
                return Err(OperatorConnectionError::Invalid);
            }
        }
        let controller = fixed_executable(&executables.controller)?;
        let runtime = fixed_executable(&executables.runtime)?;
        let assignment_mcp = fixed_executable(&executables.assignment_mcp)?;
        let Ok(claim) = ownership.reserve(ProcessRole::Controller) else {
            return self.wait_ready(started);
        };
        let generation = claim.generation().to_owned();
        if retained_selection.is_none() {
            let temporary = self
                .state
                .join(format!(".controller-selection-{generation}.tmp"));
            let publication = (|| {
                let bytes = serde_json::to_vec(&selection).map_err(io::Error::other)?;
                let mut file = create_owner_only_file(&temporary)
                    .map_err(|_| io::Error::other("protected selection publication failed"))?;
                file.write_all(&bytes)?;
                publish(
                    file,
                    &temporary,
                    &self.state.join(SELECTION_FILE),
                    PublicationMode::CreateNew,
                )
            })();
            let _ = fs::remove_file(&temporary);
            if publication.is_err() {
                let _ = ownership.cancel_abandoned(ProcessRole::Controller, &generation);
                return Err(OperatorConnectionError::Incomplete);
            }
        }
        if existing.is_none() {
            let temporary = self.state.join(format!(".managed-config-{generation}.tmp"));
            let publication = (|| {
                let mut file = create_owner_only_file(&temporary)
                    .map_err(|_| io::Error::other("protected configuration publication failed"))?;
                file.write_all(normalized.as_bytes())?;
                publish(file, &temporary, &config, PublicationMode::CreateNew)
            })();
            let _ = fs::remove_file(&temporary);
            if publication.is_err() {
                let _ = ownership.cancel_abandoned(ProcessRole::Controller, &generation);
                return Err(OperatorConnectionError::Incomplete);
            }
        }
        let diagnostic_log = self.state.join(HOSTED_SESSION_DIAGNOSTIC_FILE);
        if prepare_file_sink(&diagnostic_log).is_err() {
            let _ = ownership.cancel_abandoned(ProcessRole::Controller, &generation);
            return Err(OperatorConnectionError::Incomplete);
        }
        let mut command = Command::new(controller);
        for (key, _) in std::env::vars_os() {
            // Config has already been resolved once, including explicit overrides.
            if key
                .to_str()
                .is_some_and(|key| key == "WORLDSTREAM_CONFIG" || key.starts_with("WORLDSTREAM__"))
            {
                command.env_remove(key);
            }
        }
        command
            .arg("--bind")
            .arg(self.endpoint.to_string())
            .arg("--participant-console-origin")
            .arg(origin)
            .arg("--daemon")
            .arg(effective.server.bind.to_string())
            .arg("--daemon-executable")
            .arg(runtime)
            .arg("--daemon-config")
            .arg(&config)
            .arg("--state-dir")
            .arg(&self.state)
            .arg("--assignment-mcp-executable")
            .arg(assignment_mcp)
            .arg("--managed-generation")
            .arg(&generation)
            .arg("--graceful-stop-timeout-ms")
            .arg(self.timeout.as_millis().to_string())
            .arg("--hosted-session-diagnostic-log")
            .arg(&diagnostic_log)
            .current_dir(&self.state)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(0x0000_0008 | 0x0000_0200);
        }
        let Ok(mut child) = command.spawn() else {
            let _ = ownership.cancel_abandoned(ProcessRole::Controller, &generation);
            return Err(OperatorConnectionError::Incomplete);
        };
        // Reap only our exact child. Exiting this CLI leaves the detached child alive.
        thread::spawn(move || {
            let _ = child.wait();
        });
        self.wait_ready(started)
    }

    fn lock_startup(&self, started: Instant) -> Result<fs::File, OperatorConnectionError> {
        let path = self.state.join("managed-controller-start.lock");
        match fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let ownership = ProcessOwnership::open(&self.state)
                    .map_err(|_| OperatorConnectionError::Invalid)?;
                let retained = ownership
                    .snapshot(ProcessRole::Controller)
                    .map_err(|_| OperatorConnectionError::Invalid)?
                    .is_some()
                    || self.read_selection()?.is_some();
                // A concurrent initializer may have published the permanent
                // lock and its first generation since our initial observation.
                if retained && fs::symlink_metadata(&path).is_err() {
                    return Err(OperatorConnectionError::Invalid);
                }
                let mut nonce = [0_u8; 16];
                getrandom::fill(&mut nonce).map_err(|_| OperatorConnectionError::Unavailable)?;
                let temporary = self.state.join(format!(
                    ".controller-start-{}.tmp",
                    blake3::hash(&nonce).to_hex()
                ));
                let file = create_owner_only_file(&temporary)
                    .map_err(|_| OperatorConnectionError::Invalid)?;
                let result = publish(file, &temporary, &path, PublicationMode::CreateNew);
                let _ = fs::remove_file(&temporary);
                match result {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(_) => return Err(OperatorConnectionError::Incomplete),
                }
            }
            Err(_) => return Err(OperatorConnectionError::Invalid),
        }
        validate_owner_only_file(&path).map_err(|_| OperatorConnectionError::Invalid)?;
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            options.share_mode(0x0000_0001 | 0x0000_0002);
        }
        let lock = options
            .open(&path)
            .map_err(|_| OperatorConnectionError::Unavailable)?;
        validate_owner_only_file(&path).map_err(|_| OperatorConnectionError::Invalid)?;
        loop {
            match lock.try_lock() {
                Ok(()) => return Ok(lock),
                Err(fs::TryLockError::WouldBlock) if started.elapsed() < self.timeout => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(_) => return Err(OperatorConnectionError::Incomplete),
            }
        }
    }

    fn wait_ready(&self, started: Instant) -> Result<(), OperatorConnectionError> {
        while started.elapsed() < self.timeout {
            if self
                .request_with_timeout(
                    "GET",
                    "/api/v1/control/server/status",
                    b"",
                    Duration::from_secs(1),
                )
                .is_ok_and(|response| response.status == 200)
            {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(25));
        }
        Err(OperatorConnectionError::Incomplete)
    }

    /// Verifies Controller-only shutdown completed for the same generation.
    ///
    /// # Errors
    /// Refuses to report success while ownership remains held or has changed.
    pub fn stop_controller(&self) -> Result<(), OperatorConnectionError> {
        let ownership =
            ProcessOwnership::open(&self.state).map_err(|_| OperatorConnectionError::Invalid)?;
        let before = ownership
            .snapshot(ProcessRole::Controller)
            .map_err(|_| OperatorConnectionError::Invalid)?
            .ok_or(OperatorConnectionError::Unavailable)?;
        let started = Instant::now();
        let response = self.request("POST", "/api/v1/control/controller-stop", b"")?;
        if response.status != 202 {
            return Err(OperatorConnectionError::Unavailable);
        }
        while started.elapsed() < self.timeout {
            let after = ownership
                .snapshot(ProcessRole::Controller)
                .map_err(|_| OperatorConnectionError::Invalid)?
                .ok_or(OperatorConnectionError::Invalid)?;
            if after.generation != before.generation {
                return Err(OperatorConnectionError::Invalid);
            }
            if after.phase == ProcessPhase::Stopped
                && !ownership
                    .is_leased(ProcessRole::Controller)
                    .map_err(|_| OperatorConnectionError::Invalid)?
            {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(25));
        }
        Err(OperatorConnectionError::Incomplete)
    }
}

fn selected_path(
    requested: &Path,
    working_directory: &Path,
) -> Result<PathBuf, OperatorConnectionError> {
    if requested.as_os_str().is_empty() || !working_directory.is_absolute() {
        return Err(OperatorConnectionError::Invalid);
    }
    let path = working_directory.join(requested);
    match fs::symlink_metadata(&path) {
        Ok(_) => fs::canonicalize(path).map_err(|_| OperatorConnectionError::Invalid),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or(OperatorConnectionError::Invalid)?;
            let name = path.file_name().ok_or(OperatorConnectionError::Invalid)?;
            Ok(fs::canonicalize(parent)
                .map_err(|_| OperatorConnectionError::Invalid)?
                .join(name))
        }
        Err(_) => Err(OperatorConnectionError::Invalid),
    }
}

fn fixed_executable(path: &Path) -> Result<PathBuf, OperatorConnectionError> {
    let path = fs::canonicalize(path).map_err(|_| OperatorConnectionError::Invalid)?;
    if !path.is_file() {
        return Err(OperatorConnectionError::Invalid);
    }
    Ok(path)
}
