//! Native owned-process boundary.
//!
//! A small guard process owns each provider process group. The supervisor keeps
//! the guard's control pipe open; supervisor death closes that pipe and causes
//! the guard to terminate and reap the provider group. On Windows both guard
//! and provider groups use kill-on-close Job Objects through `command-group`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufRead as _, Read, Write},
    path::{Path, PathBuf},
    process::{ChildStdin, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use command_group::{CommandGroup as _, GroupChild};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::provider::{
    EphemeralProviderProfile, OutputContract, PreparedInvocation, ProviderBlocker,
    codex_app_server, digest_executable,
};
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_data_directory,
    validate_owner_only_file,
};

const MAX_GUARD_REQUEST_BYTES: usize = 8 * 1024 * 1024;
const MAX_ARGUMENTS: usize = 512;
const DEFAULT_CAPTURE_BYTES: usize = 4 * 1024 * 1024;
const MAX_PROFILE_FILES: usize = 8;
const MAX_PROFILE_BYTES: usize = 1024 * 1024;
const GUARD_READY_LINE: &str = "worldstream-agent-swarm-process-guard-ready-v1\n";
// The guard hashes the exact native executable before and after spawn. A real
// macOS Codex binary takes over ten seconds for both hashes in a debug build.
// Keep this bounded while allowing identity verification to finish.
const GUARD_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GuardLaunch {
    program: PathBuf,
    expected_executable_digest: String,
    arguments: Vec<String>,
    working_area: PathBuf,
    stdin: String,
    #[serde(default)]
    codex_app_server: bool,
    #[serde(default)]
    clear_environment: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    environment: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cleanup_directory: Option<PathBuf>,
}

/// Exact non-provider command executed through the same owned process guard.
/// The child receives only the explicitly supplied environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedCommand {
    pub program: PathBuf,
    /// Exact BLAKE3 identity authorized by the caller before guard launch.
    pub expected_executable_digest: String,
    pub arguments: Vec<String>,
    pub working_area: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub stdin: String,
}

/// Native owned-process failures. Arguments, environment and output are never
/// included in the error text.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ProcessError {
    #[error("owned process request is invalid")]
    InvalidRequest,
    #[error("owned process could not be started")]
    SpawnUnavailable,
    #[error("owned process communication failed")]
    Communication,
    #[error("owned process output is invalid")]
    InvalidOutput,
    #[error("owned process tree could not be proven stopped")]
    ContainmentUnresolved,
    #[error("provider configuration is blocked")]
    ProviderBlocked,
}

impl From<ProviderBlocker> for ProcessError {
    fn from(_value: ProviderBlocker) -> Self {
        Self::ProviderBlocked
    }
}

/// Bounded terminal process observation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessExit {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

/// Spawns provider invocations through an exact guard executable.
#[derive(Clone, Debug)]
pub struct NativeProcessSpawner {
    guard_executable: PathBuf,
    capture_bytes: usize,
    profile_root: Option<PathBuf>,
}

impl NativeProcessSpawner {
    /// Opens the native process boundary around one exact guard executable.
    ///
    /// # Errors
    /// Rejects relative, absent, or non-file guard paths and unbounded capture.
    pub fn new(guard_executable: &Path, capture_bytes: usize) -> Result<Self, ProcessError> {
        let guard_executable = exact_file(guard_executable)?;
        if capture_bytes == 0 || capture_bytes > 64 * 1024 * 1024 {
            return Err(ProcessError::InvalidRequest);
        }
        Ok(Self {
            guard_executable,
            capture_bytes,
            profile_root: None,
        })
    }

    /// Uses the standard bounded transcript size.
    ///
    /// # Errors
    /// Rejects an unavailable guard path.
    pub fn with_default_capture(guard_executable: &Path) -> Result<Self, ProcessError> {
        Self::new(guard_executable, DEFAULT_CAPTURE_BYTES)
    }

    /// Adds an owner-protected root for invocation-local provider profiles.
    /// Existing children are crash remnants from the previous sole owner and
    /// are removed before this spawner can launch work.
    ///
    /// # Errors
    /// Rejects unsafe storage or an unprovable stale entry.
    pub fn with_profile_root(mut self, root: &Path) -> Result<Self, ProcessError> {
        let root = prepare_data_directory(root).map_err(|_| ProcessError::InvalidRequest)?;
        cleanup_stale_profiles(&root)?;
        self.profile_root = Some(root);
        Ok(self)
    }

    /// Starts one spawnable provider request. The returned handle is the only
    /// authority to stop the owned process tree; its PID is diagnostic only.
    ///
    /// # Errors
    /// Fails closed for blocked configuration, invalid paths, or guard startup.
    pub fn spawn(&self, prepared: &PreparedInvocation) -> Result<OwnedInvocation, ProcessError> {
        prepared.ensure_spawnable()?;
        if let Some(profile) = prepared
            .qualification
            .as_ref()
            .and_then(|binding| binding.local_codex_profile.as_ref())
        {
            profile
                .validate_guard(&self.guard_executable)
                .map_err(|_| ProcessError::InvalidRequest)?;
        }
        if prepared.output_contract == OutputContract::CodexAppServer {
            codex_app_server::Request::validate_prepared(prepared)
                .map_err(|_| ProcessError::InvalidRequest)?;
        }
        let program = exact_file(&prepared.program)?;
        if program != prepared.program
            || digest_executable(&program).as_ref() != Ok(&prepared.executable_digest)
        {
            return Err(ProcessError::InvalidRequest);
        }
        let working_area = prepared
            .working_area
            .canonicalize()
            .map_err(|_| ProcessError::InvalidRequest)?;
        if !working_area.is_dir()
            || prepared.arguments.len() > MAX_ARGUMENTS
            || prepared.stdin.len() > MAX_GUARD_REQUEST_BYTES
            || prepared.arguments.iter().any(|value| value.contains('\0'))
        {
            return Err(ProcessError::InvalidRequest);
        }
        let profile = match &prepared.ephemeral_profile {
            Some(profile) => Some(self.materialize_profile(&prepared.invocation_id, profile)?),
            None => None,
        };
        let mut environment_set = prepared.environment_set.clone();
        if let Some(profile) = &profile
            && environment_set
                .insert(
                    profile.environment_variable.clone(),
                    profile.directory().display().to_string(),
                )
                .is_some()
        {
            cleanup_profile(profile.directory());
            return Err(ProcessError::InvalidRequest);
        }
        let request = GuardLaunch {
            program,
            expected_executable_digest: prepared.executable_digest.clone(),
            arguments: prepared.arguments.clone(),
            working_area,
            stdin: prepared.stdin.clone(),
            codex_app_server: prepared.output_contract == OutputContract::CodexAppServer,
            clear_environment: false,
            environment: BTreeMap::new(),
            cleanup_directory: profile
                .as_ref()
                .map(|profile| profile.directory().to_path_buf()),
        };
        self.spawn_request(
            &request,
            &prepared.environment_remove,
            &environment_set,
            profile,
        )
    }

    /// Starts an exact local command through the native owned-descendant
    /// guard. Unlike provider launches, the child environment is cleared.
    ///
    /// # Errors
    ///
    /// Rejects unsafe paths, oversized input, invalid environment entries, or
    /// an unavailable process guard.
    pub fn spawn_command(&self, request: &OwnedCommand) -> Result<OwnedInvocation, ProcessError> {
        let program = exact_file(&request.program)?;
        if digest_executable(&program).as_ref() != Ok(&request.expected_executable_digest) {
            return Err(ProcessError::InvalidRequest);
        }
        let working_area = request
            .working_area
            .canonicalize()
            .map_err(|_| ProcessError::InvalidRequest)?;
        if !working_area.is_dir()
            || request.arguments.len() > MAX_ARGUMENTS
            || request.stdin.len() > MAX_GUARD_REQUEST_BYTES
            || request.arguments.iter().any(|value| value.contains('\0'))
            || request.environment.iter().any(|(name, value)| {
                name.is_empty()
                    || name.contains(['=', '\0'])
                    || value.contains('\0')
                    || name.len() > MAX_GUARD_REQUEST_BYTES
                    || value.len() > MAX_GUARD_REQUEST_BYTES
            })
        {
            return Err(ProcessError::InvalidRequest);
        }
        let launch = GuardLaunch {
            program,
            expected_executable_digest: request.expected_executable_digest.clone(),
            arguments: request.arguments.clone(),
            working_area,
            stdin: request.stdin.clone(),
            codex_app_server: false,
            clear_environment: true,
            environment: request.environment.clone(),
            cleanup_directory: None,
        };
        self.spawn_request(&launch, &BTreeSet::new(), &BTreeMap::new(), None)
    }

    fn spawn_request(
        &self,
        request: &GuardLaunch,
        environment_remove: &BTreeSet<String>,
        environment_set: &BTreeMap<String, String>,
        profile: Option<MaterializedProfile>,
    ) -> Result<OwnedInvocation, ProcessError> {
        let bytes = serde_json::to_vec(request).map_err(|_| ProcessError::InvalidRequest)?;
        if bytes.len() > MAX_GUARD_REQUEST_BYTES {
            return Err(ProcessError::InvalidRequest);
        }
        let mut command = Command::new(&self.guard_executable);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for variable in environment_remove {
            command.env_remove(variable);
        }
        for (key, value) in environment_set {
            command.env(key, value);
        }
        let mut child = match spawn_group(&mut command) {
            Ok(child) => child,
            Err(error) => {
                if let Some(profile) = profile {
                    cleanup_profile(profile.directory());
                }
                return Err(error);
            }
        };
        let mut control = child
            .inner()
            .stdin
            .take()
            .ok_or(ProcessError::Communication)?;
        let stdout = child
            .inner()
            .stdout
            .take()
            .ok_or(ProcessError::Communication)?;
        let stderr = child
            .inner()
            .stderr
            .take()
            .ok_or(ProcessError::Communication)?;
        if control.write_all(&bytes).is_err()
            || control.write_all(b"\n").is_err()
            || control.flush().is_err()
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ProcessError::Communication);
        }
        let stdout = match guard_handshake(stdout) {
            Ok(stdout) => stdout,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        Ok(OwnedInvocation {
            child: Some(child),
            control: Some(control),
            stdout: Some(spawn_capture(stdout, self.capture_bytes)),
            stderr: Some(spawn_capture(stderr, self.capture_bytes)),
            terminal: None,
            profile_directory: profile.map(MaterializedProfile::into_directory),
        })
    }

    fn materialize_profile(
        &self,
        invocation_id: &str,
        profile: &EphemeralProviderProfile,
    ) -> Result<MaterializedProfile, ProcessError> {
        validate_profile(profile)?;
        let root = self
            .profile_root
            .as_ref()
            .ok_or(ProcessError::InvalidRequest)?;
        let digest = blake3::hash(invocation_id.as_bytes()).to_hex().to_string();
        let directory = root.join(format!("invocation-{digest}"));
        if directory.exists() {
            return Err(ProcessError::InvalidRequest);
        }
        prepare_data_directory(&directory).map_err(|_| ProcessError::InvalidRequest)?;
        for entry in &profile.files {
            let path = directory.join(&entry.name);
            let Ok(mut file) = create_owner_only_file(&path) else {
                cleanup_profile(&directory);
                return Err(ProcessError::InvalidRequest);
            };
            if file.write_all(entry.contents.as_bytes()).is_err()
                || file.sync_all().is_err()
                || validate_owner_only_file(&path).is_err()
            {
                cleanup_profile(&directory);
                return Err(ProcessError::InvalidRequest);
            }
        }
        Ok(MaterializedProfile {
            directory,
            environment_variable: profile.root_environment_variable.clone(),
            cleanup_on_drop: true,
        })
    }
}

struct MaterializedProfile {
    directory: PathBuf,
    environment_variable: String,
    cleanup_on_drop: bool,
}

impl MaterializedProfile {
    fn directory(&self) -> &Path {
        &self.directory
    }

    fn into_directory(mut self) -> PathBuf {
        self.cleanup_on_drop = false;
        self.directory.clone()
    }
}

impl Drop for MaterializedProfile {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            cleanup_profile(&self.directory);
        }
    }
}

/// Live owned provider tree.
pub struct OwnedInvocation {
    child: Option<GroupChild>,
    control: Option<ChildStdin>,
    stdout: Option<JoinHandle<Result<Captured, ProcessError>>>,
    stderr: Option<JoinHandle<Result<Captured, ProcessError>>>,
    terminal: Option<ProcessExit>,
    profile_directory: Option<PathBuf>,
}

impl std::fmt::Debug for OwnedInvocation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OwnedInvocation")
            .field("pid", &self.pid())
            .field("terminal", &self.terminal.is_some())
            .finish_non_exhaustive()
    }
}

impl OwnedInvocation {
    /// Diagnostic leader PID. It is never accepted back as signalling authority.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(GroupChild::id)
    }

    /// Polls for a fully drained owned process tree.
    ///
    /// # Errors
    /// Returns a closed containment/output error and retains no success claim.
    pub fn poll(&mut self) -> Result<Option<ProcessExit>, ProcessError> {
        if let Some(exit) = &self.terminal {
            return Ok(Some(exit.clone()));
        }
        let Some(child) = &mut self.child else {
            return Err(ProcessError::ContainmentUnresolved);
        };
        match child.try_wait() {
            Ok(Some(status)) => self.finish(status).map(Some),
            Ok(None) => Ok(None),
            Err(_) => Err(ProcessError::ContainmentUnresolved),
        }
    }

    /// Waits until the complete owned process tree and its output pipes are
    /// drained. Dropping the caller still closes the guard control pipe and
    /// requests descendant cleanup.
    ///
    /// # Errors
    ///
    /// Returns a closed containment/output error and never fabricates an exit.
    pub fn wait(&mut self) -> Result<ProcessExit, ProcessError> {
        loop {
            if let Some(exit) = self.poll()? {
                return Ok(exit);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Requests guard-mediated shutdown, then forcibly terminates the guard
    /// group only if the trusted guard misses the bounded deadline.
    ///
    /// # Errors
    /// Never reports success until the guard process group and output pipes are
    /// conclusively drained.
    pub fn stop(&mut self, grace: Duration) -> Result<ProcessExit, ProcessError> {
        if grace.is_zero() || grace > Duration::from_mins(1) {
            return Err(ProcessError::InvalidRequest);
        }
        if let Some(exit) = &self.terminal {
            return Ok(exit.clone());
        }
        drop(self.control.take());
        let deadline = Instant::now()
            .checked_add(grace)
            .ok_or(ProcessError::ContainmentUnresolved)?;
        loop {
            if let Some(exit) = self.poll()? {
                return Ok(exit);
            }
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let child = self
            .child
            .as_mut()
            .ok_or(ProcessError::ContainmentUnresolved)?;
        if let Err(error) = child.kill()
            && error.kind() != io::ErrorKind::InvalidInput
        {
            return Err(ProcessError::ContainmentUnresolved);
        }
        let forced_deadline = Instant::now()
            .checked_add(grace)
            .ok_or(ProcessError::ContainmentUnresolved)?;
        loop {
            if self.poll()?.is_some() {
                // The guard owns a separate provider group. If it missed the
                // control deadline and had to be killed, that group's drain
                // can no longer be proven from this process.
                return Err(ProcessError::ContainmentUnresolved);
            }
            if Instant::now() >= forced_deadline {
                return Err(ProcessError::ContainmentUnresolved);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn finish(&mut self, status: ExitStatus) -> Result<ProcessExit, ProcessError> {
        self.control.take();
        let stdout = join_capture(self.stdout.take())?;
        let stderr = join_capture(self.stderr.take())?;
        self.child.take();
        if let Some(directory) = self.profile_directory.take() {
            cleanup_profile(&directory);
        }
        let exit = ProcessExit {
            success: status.success(),
            code: status.code(),
            stdout: stdout.bytes,
            stderr: stderr.bytes,
            stdout_truncated: stdout.truncated,
            stderr_truncated: stderr.truncated,
        };
        self.terminal = Some(exit.clone());
        Ok(exit)
    }
}

impl Drop for OwnedInvocation {
    fn drop(&mut self) {
        // Closing control is the crash/owner-loss signal. Do not synchronously
        // join here: Drop must remain bounded even if the host is unwinding.
        drop(self.control.take());
    }
}

#[derive(Debug)]
struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}

fn spawn_capture(
    mut reader: impl Read + Send + 'static,
    limit: usize,
) -> JoinHandle<Result<Captured, ProcessError>> {
    thread::spawn(move || {
        let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
        let mut truncated = false;
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            let count = reader
                .read(&mut buffer)
                .map_err(|_| ProcessError::InvalidOutput)?;
            if count == 0 {
                break;
            }
            let remaining = limit.saturating_sub(bytes.len());
            let retained = remaining.min(count);
            bytes.extend_from_slice(&buffer[..retained]);
            truncated |= retained != count;
        }
        Ok(Captured { bytes, truncated })
    })
}

fn guard_handshake(
    stdout: impl Read + Send + 'static,
) -> Result<impl Read + Send + 'static, ProcessError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut reader = io::BufReader::new(stdout);
        let mut line = String::new();
        let valid = reader
            .read_line(&mut line)
            .is_ok_and(|count| count == GUARD_READY_LINE.len() && line == GUARD_READY_LINE);
        let _ = sender.send((reader, valid));
    });
    let (reader, valid) = receiver
        .recv_timeout(GUARD_HANDSHAKE_TIMEOUT)
        .map_err(|_| ProcessError::Communication)?;
    if !valid {
        return Err(ProcessError::SpawnUnavailable);
    }
    Ok(reader)
}

fn join_capture(
    handle: Option<JoinHandle<Result<Captured, ProcessError>>>,
) -> Result<Captured, ProcessError> {
    handle
        .ok_or(ProcessError::InvalidOutput)?
        .join()
        .map_err(|_| ProcessError::InvalidOutput)?
}

fn exact_file(path: &Path) -> Result<PathBuf, ProcessError> {
    if !path.is_absolute() {
        return Err(ProcessError::InvalidRequest);
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| ProcessError::InvalidRequest)?;
    if !canonical
        .metadata()
        .map_err(|_| ProcessError::InvalidRequest)?
        .is_file()
    {
        return Err(ProcessError::InvalidRequest);
    }
    Ok(canonical)
}

#[cfg(windows)]
fn spawn_group(command: &mut Command) -> Result<GroupChild, ProcessError> {
    command
        .group()
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| ProcessError::SpawnUnavailable)
}

#[cfg(not(windows))]
fn spawn_group(command: &mut Command) -> Result<GroupChild, ProcessError> {
    command
        .group_spawn()
        .map_err(|_| ProcessError::SpawnUnavailable)
}

/// Process-guard entry point used only by the fixed companion binary.
///
/// # Errors
/// Returns a closed error after best-effort provider-tree cleanup.
#[doc(hidden)]
#[allow(
    clippy::too_many_lines,
    reason = "keep the guard's validated launch, handshake, monitoring, and cleanup sequence visibly linear"
)]
pub fn run_guard_stdio() -> Result<i32, ProcessError> {
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|_| ProcessError::Communication)?;
    if line.is_empty() || line.len() > MAX_GUARD_REQUEST_BYTES {
        return Err(ProcessError::InvalidRequest);
    }
    let request: GuardLaunch =
        serde_json::from_str(&line).map_err(|_| ProcessError::InvalidRequest)?;
    let program = exact_file(&request.program)?;
    if digest_executable(&program).as_ref() != Ok(&request.expected_executable_digest) {
        if let Some(directory) = request.cleanup_directory.as_deref() {
            cleanup_profile(directory);
        }
        return Err(ProcessError::InvalidRequest);
    }
    let working_area = request
        .working_area
        .canonicalize()
        .map_err(|_| ProcessError::InvalidRequest)?;
    if !working_area.is_dir()
        || request.arguments.len() > MAX_ARGUMENTS
        || request.stdin.len() > MAX_GUARD_REQUEST_BYTES
        || request.arguments.iter().any(|value| value.contains('\0'))
    {
        return Err(ProcessError::InvalidRequest);
    }
    let app_server_request = request
        .codex_app_server
        .then(|| codex_app_server::Request::parse(&request.stdin))
        .transpose()
        .map_err(|_| ProcessError::InvalidRequest)?;
    let mut command = Command::new(&program);
    command
        .args(request.arguments)
        .current_dir(working_area)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if request.clear_environment {
        command.env_clear();
    }
    command.envs(request.environment);
    let mut provider = spawn_group(&mut command)?;
    // Re-read the executable identity after native spawn. This detects a
    // replacement in the last pathname-based launch window and fails closed;
    // the owned group is terminated before any result can be accepted.
    if digest_executable(&program).as_ref() != Ok(&request.expected_executable_digest) {
        let _ = provider.kill();
        let _ = provider.wait();
        if let Some(directory) = request.cleanup_directory.as_deref() {
            cleanup_profile(directory);
        }
        return Err(ProcessError::InvalidRequest);
    }
    io::stdout()
        .write_all(GUARD_READY_LINE.as_bytes())
        .and_then(|()| io::stdout().flush())
        .map_err(|_| ProcessError::Communication)?;
    let mut provider_stdin = provider
        .inner()
        .stdin
        .take()
        .ok_or(ProcessError::Communication)?;
    let provider_stdout = provider
        .inner()
        .stdout
        .take()
        .ok_or(ProcessError::Communication)?;
    let provider_stderr = provider
        .inner()
        .stderr
        .take()
        .ok_or(ProcessError::Communication)?;
    let (protocol_sender, protocol_receiver) = mpsc::sync_channel(1);
    let stdout_pump = if let Some(app_server_request) = app_server_request {
        thread::spawn(move || {
            let result = codex_app_server::run(
                &app_server_request,
                provider_stdout,
                provider_stdin,
                io::stdout(),
            )
            .map_err(|_| ProcessError::InvalidOutput);
            let _ = protocol_sender.send(result.is_ok());
            result
        })
    } else {
        if provider_stdin.write_all(request.stdin.as_bytes()).is_err()
            || provider_stdin.flush().is_err()
        {
            let _ = provider.kill();
            let _ = provider.wait();
            return Err(ProcessError::Communication);
        }
        drop(provider_stdin);
        thread::spawn(move || pump(provider_stdout, io::stdout()))
    };
    let stderr_pump = thread::spawn(move || pump(provider_stderr, io::stderr()));
    let (control_sender, control_receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut sink = [0_u8; 64];
        loop {
            match reader.read(&mut sink) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let _ = control_sender.send(());
    });
    let (observed_status, stopping, protocol_success) = loop {
        if control_receiver.try_recv().is_ok() {
            break (None, true, None);
        }
        if let Ok(success) = protocol_receiver.try_recv() {
            break (None, false, Some(success));
        }
        match observe_group_leader(&mut provider) {
            Ok(Some(status)) => break (Some(status), false, None),
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => return Err(ProcessError::ContainmentUnresolved),
        }
    };
    let status = terminate_group_and_reap(&mut provider, observed_status)?;
    let stdout_result = stdout_pump
        .join()
        .map_err(|_| ProcessError::InvalidOutput)?;
    let stderr_result = stderr_pump
        .join()
        .map_err(|_| ProcessError::InvalidOutput)?;
    if let Some(directory) = request.cleanup_directory {
        cleanup_profile(&directory);
    }
    stdout_result?;
    stderr_result?;
    if stopping {
        Ok(1)
    } else if let Some(success) = protocol_success {
        Ok(i32::from(!success))
    } else {
        Ok(status.code().unwrap_or(1))
    }
}

#[cfg(unix)]
fn observe_group_leader(provider: &mut GroupChild) -> io::Result<Option<ExitStatus>> {
    use std::os::unix::process::ExitStatusExt as _;

    let pid = rustix::process::Pid::from_child(provider.inner());
    let options = rustix::process::WaitIdOptions::EXITED
        | rustix::process::WaitIdOptions::NOHANG
        | rustix::process::WaitIdOptions::NOWAIT;
    rustix::process::waitid(rustix::process::WaitId::Pid(pid), options)
        .map(|status| {
            status.and_then(|status| {
                if let Some(code) = status.exit_status() {
                    return code.checked_shl(8).map(ExitStatus::from_raw);
                }
                status.terminating_signal().map(|signal| {
                    let core_dumped = if status.dumped() { 0x80 } else { 0 };
                    ExitStatus::from_raw(signal | core_dumped)
                })
            })
        })
        .map_err(io::Error::from)
}

#[cfg(windows)]
fn observe_group_leader(provider: &mut GroupChild) -> io::Result<Option<ExitStatus>> {
    provider.try_wait()
}

fn terminate_group_and_reap(
    provider: &mut GroupChild,
    mut observed_status: Option<ExitStatus>,
) -> Result<ExitStatus, ProcessError> {
    if let Err(error) = provider.kill() {
        let possibly_gone = matches!(
            error.kind(),
            io::ErrorKind::InvalidInput | io::ErrorKind::NotFound
        ) || is_no_such_process(&error)
            || is_darwin_zombie_group(&error);
        if !possibly_gone {
            return Err(ProcessError::ContainmentUnresolved);
        }
        // Closing app-server stdin starts exit. Darwin can report EPERM from
        // killpg before waitid can observe that terminal state. Allow only a
        // bounded observation grace; never interpret the signal error itself
        // as proof of termination or accept a still-running leader.
        if observed_status.is_none() {
            observed_status = terminal_observation_grace(|| observe_group_leader(provider))?;
        }
        if observed_status.is_none() {
            return Err(ProcessError::ContainmentUnresolved);
        }
    }
    let status = provider
        .wait()
        .map_err(|_| ProcessError::ContainmentUnresolved)?;
    Ok(observed_status.unwrap_or(status))
}

fn terminal_observation_grace(
    mut observe: impl FnMut() -> io::Result<Option<ExitStatus>>,
) -> Result<Option<ExitStatus>, ProcessError> {
    for attempt in 0..25 {
        let status = observe().map_err(|_| ProcessError::ContainmentUnresolved)?;
        if status.is_some() {
            return Ok(status);
        }
        if attempt < 24 {
            thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(None)
}

#[cfg(all(test, unix))]
mod terminal_observation_tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt as _;

    #[test]
    fn exit_becoming_observable_after_signal_race_is_required() -> Result<(), ProcessError> {
        let mut observations = 0;
        let status = terminal_observation_grace(|| {
            observations += 1;
            Ok((observations == 3).then(|| ExitStatus::from_raw(0)))
        })?;
        assert_eq!(observations, 3);
        assert!(status.is_some_and(|value| value.success()));
        assert!(terminal_observation_grace(|| Ok(None))?.is_none());
        assert!(terminal_observation_grace(|| Err(io::Error::other("wait failed"))).is_err());
        Ok(())
    }
}

#[cfg(unix)]
fn is_no_such_process(error: &io::Error) -> bool {
    error.raw_os_error() == Some(rustix::io::Errno::SRCH.raw_os_error())
}

#[cfg(target_os = "macos")]
fn is_darwin_zombie_group(error: &io::Error) -> bool {
    // Darwin can return EPERM for killpg after the WNOWAIT-observed leader is
    // a zombie and no signalable member remains in its group.
    error.raw_os_error() == Some(rustix::io::Errno::PERM.raw_os_error())
}

#[cfg(all(unix, not(target_os = "macos")))]
const fn is_darwin_zombie_group(_error: &io::Error) -> bool {
    false
}

#[cfg(windows)]
const fn is_no_such_process(_error: &io::Error) -> bool {
    false
}

#[cfg(windows)]
const fn is_darwin_zombie_group(_error: &io::Error) -> bool {
    false
}

fn pump(mut reader: impl Read, mut writer: impl Write) -> Result<(), ProcessError> {
    io::copy(&mut reader, &mut writer).map_err(|_| ProcessError::InvalidOutput)?;
    writer.flush().map_err(|_| ProcessError::InvalidOutput)
}

fn validate_profile(profile: &EphemeralProviderProfile) -> Result<(), ProcessError> {
    let valid_environment = !profile.root_environment_variable.is_empty()
        && profile.root_environment_variable.len() <= 128
        && profile
            .root_environment_variable
            .bytes()
            .enumerate()
            .all(|(index, byte)| {
                byte == b'_' || byte.is_ascii_uppercase() || (index > 0 && byte.is_ascii_digit())
            });
    let mut total = 0_usize;
    if !valid_environment || profile.files.is_empty() || profile.files.len() > MAX_PROFILE_FILES {
        return Err(ProcessError::InvalidRequest);
    }
    for file in &profile.files {
        total = total.saturating_add(file.contents.len());
        let path = Path::new(&file.name);
        if file.name.is_empty()
            || file.name.len() > 255
            || path.file_name() != Some(path.as_os_str())
            || file.name.contains(['/', '\\', '\0'])
            || total > MAX_PROFILE_BYTES
        {
            return Err(ProcessError::InvalidRequest);
        }
    }
    Ok(())
}

fn cleanup_stale_profiles(root: &Path) -> Result<(), ProcessError> {
    let root = validate_data_directory(root).map_err(|_| ProcessError::InvalidRequest)?;
    for entry in fs::read_dir(&root).map_err(|_| ProcessError::InvalidRequest)? {
        let entry = entry.map_err(|_| ProcessError::InvalidRequest)?;
        let kind = entry
            .file_type()
            .map_err(|_| ProcessError::InvalidRequest)?;
        if !kind.is_dir() || kind.is_symlink() {
            return Err(ProcessError::InvalidRequest);
        }
        validate_data_directory(&entry.path()).map_err(|_| ProcessError::InvalidRequest)?;
        cleanup_profile(&entry.path());
        if entry.path().exists() {
            return Err(ProcessError::InvalidRequest);
        }
    }
    Ok(())
}

fn cleanup_profile(directory: &Path) {
    let Ok(directory) = validate_data_directory(directory) else {
        return;
    };
    let _ = fs::remove_dir_all(directory);
}
