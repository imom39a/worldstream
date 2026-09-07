//! Fixed-configuration lifecycle control for the local `worldstreamd` process.

use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::{DaemonConnectivityV1, DaemonStatusSource};

const LIFECYCLE_SCHEMA_V1: &str = "worldstream/studio-daemon-lifecycle/v1";
const POLL_INTERVAL: Duration = Duration::from_millis(25);

#[cfg(test)]
type StopCompletionGate = Arc<(Mutex<bool>, std::sync::Condvar)>;

/// The complete closed lifecycle vocabulary exposed to Studio.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonLifecycleStateV1 {
    /// No configured daemon process is live.
    Stopped,
    /// One configured start is in progress.
    Starting,
    /// A managed or reconciled configured daemon is live.
    Running,
    /// A graceful stop is in progress.
    Stopping,
    /// The last lifecycle operation failed safely.
    Failed,
    /// Fixed lifecycle configuration is not available.
    Unavailable,
}

/// Bounded remediation safe for display in the operator portal.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DaemonLifecycleFailureV1 {
    /// Stable machine-readable failure code.
    pub code: String,
    /// Credential-free explanation suitable for a host operator.
    pub explanation: String,
    /// Safe next action suitable for a host operator.
    pub next_action: String,
}

/// One typed lifecycle snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DaemonLifecycleV1 {
    /// Stable response schema identifier.
    pub schema: String,
    /// Current configured-daemon process state.
    pub state: DaemonLifecycleStateV1,
    /// Identity of the most recently accepted lifecycle operation.
    pub operation_id: u64,
    /// Whether the Supervisor owns the process and can safely stop it.
    pub managed_by_supervisor: bool,
    /// Bounded failure and remediation, only for failed or unavailable states.
    pub failure: Option<DaemonLifecycleFailureV1>,
}

/// Fixed lifecycle capability injected into the versioned Supervisor routes.
pub trait DaemonLifecycleControl: Send + Sync + 'static {
    /// Reconciles and returns the current lifecycle snapshot.
    fn lifecycle(&self) -> DaemonLifecycleV1;
    /// Idempotently requests a configured start.
    fn start(&self) -> DaemonLifecycleV1;
    /// Idempotently requests a graceful configured stop.
    fn stop(&self) -> DaemonLifecycleV1;
    /// Requests a graceful stop followed by the same configured start.
    fn restart(&self) -> DaemonLifecycleV1;
}

/// Builds the Supervisor's fixed, bodyless lifecycle routes.
pub fn lifecycle_router(control: impl DaemonLifecycleControl) -> Router {
    let control: Arc<dyn DaemonLifecycleControl> = Arc::new(control);
    Router::new()
        .route("/api/v1/daemon/lifecycle", get(lifecycle))
        .route("/api/v1/daemon/start", post(start))
        .route("/api/v1/daemon/stop", post(stop))
        .route("/api/v1/daemon/restart", post(restart))
        .with_state(control)
}

async fn lifecycle(
    State(control): State<Arc<dyn DaemonLifecycleControl>>,
) -> Json<DaemonLifecycleV1> {
    lifecycle_operation(control, DaemonLifecycleControl::lifecycle).await
}

async fn start(State(control): State<Arc<dyn DaemonLifecycleControl>>) -> Json<DaemonLifecycleV1> {
    lifecycle_operation(control, DaemonLifecycleControl::start).await
}

async fn stop(State(control): State<Arc<dyn DaemonLifecycleControl>>) -> Json<DaemonLifecycleV1> {
    lifecycle_operation(control, DaemonLifecycleControl::stop).await
}

async fn restart(
    State(control): State<Arc<dyn DaemonLifecycleControl>>,
) -> Json<DaemonLifecycleV1> {
    lifecycle_operation(control, DaemonLifecycleControl::restart).await
}

async fn lifecycle_operation(
    control: Arc<dyn DaemonLifecycleControl>,
    operation: fn(&dyn DaemonLifecycleControl) -> DaemonLifecycleV1,
) -> Json<DaemonLifecycleV1> {
    let fallback = unavailable("operation_failed");
    let snapshot = tokio::task::spawn_blocking(move || operation(control.as_ref()))
        .await
        .unwrap_or(fallback);
    Json(snapshot)
}

/// Process lifecycle controller constrained to one executable and config file.
#[derive(Clone)]
pub struct ConfiguredDaemonLifecycle {
    inner: Arc<Mutex<LifecycleInner>>,
    executable: Arc<PathBuf>,
    config: Arc<PathBuf>,
    graceful_timeout: Duration,
    status_source: Arc<dyn DaemonStatusSource>,
    launcher: Arc<dyn ProcessLauncher>,
    #[cfg(test)]
    stop_completion_gate: Option<StopCompletionGate>,
}

struct LifecycleInner {
    state: DaemonLifecycleStateV1,
    operation_id: u64,
    managed: bool,
    failure: Option<DaemonLifecycleFailureV1>,
    child: Option<Box<dyn ManagedDaemon>>,
}

impl ConfiguredDaemonLifecycle {
    /// Configures control for exactly one daemon executable and config file.
    pub fn new(
        executable: PathBuf,
        config: PathBuf,
        graceful_timeout: Duration,
        status_source: impl DaemonStatusSource,
    ) -> Self {
        Self::with_launcher(
            executable,
            config,
            graceful_timeout,
            status_source,
            OsProcessLauncher,
        )
    }

    fn with_launcher(
        executable: PathBuf,
        config: PathBuf,
        graceful_timeout: Duration,
        status_source: impl DaemonStatusSource,
        launcher: impl ProcessLauncher,
    ) -> Self {
        let configured = executable.is_file() && config.is_file();
        Self {
            inner: Arc::new(Mutex::new(LifecycleInner {
                state: if configured {
                    DaemonLifecycleStateV1::Stopped
                } else {
                    DaemonLifecycleStateV1::Unavailable
                },
                operation_id: 0,
                managed: false,
                failure: (!configured).then(configuration_unavailable),
                child: None,
            })),
            executable: Arc::new(executable),
            config: Arc::new(config),
            graceful_timeout,
            status_source: Arc::new(status_source),
            launcher: Arc::new(launcher),
            #[cfg(test)]
            stop_completion_gate: None,
        }
    }

    fn lock(&self) -> MutexGuard<'_, LifecycleInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn snapshot(inner: &LifecycleInner) -> DaemonLifecycleV1 {
        DaemonLifecycleV1 {
            schema: LIFECYCLE_SCHEMA_V1.to_owned(),
            state: inner.state,
            operation_id: inner.operation_id,
            managed_by_supervisor: inner.managed,
            failure: inner.failure.clone(),
        }
    }

    fn reconcile(&self) {
        {
            let mut inner = self.lock();
            // The accepted operation owns child completion while in flight.
            // A status read must not reap an exit and discard a pending restart.
            if matches!(
                inner.state,
                DaemonLifecycleStateV1::Starting | DaemonLifecycleStateV1::Stopping
            ) {
                return;
            }
            if let Some(child) = inner.child.as_mut() {
                match child.try_wait() {
                    Ok(Some(_)) => {
                        inner.child = None;
                        inner.managed = false;
                        if inner.state != DaemonLifecycleStateV1::Stopped {
                            inner.state = DaemonLifecycleStateV1::Failed;
                            inner.failure = Some(unexpected_exit());
                        }
                    }
                    Ok(None) => return,
                    Err(()) => {
                        inner.state = DaemonLifecycleStateV1::Failed;
                        inner.failure = Some(process_state_failed());
                        return;
                    }
                }
            }
        }

        if !self.executable.is_file() || !self.config.is_file() {
            let mut inner = self.lock();
            inner.state = DaemonLifecycleStateV1::Unavailable;
            inner.managed = false;
            inner.failure = Some(configuration_unavailable());
            return;
        }

        let connected = self.status_source.status().connectivity == DaemonConnectivityV1::Connected;
        let mut inner = self.lock();
        if inner.child.is_some()
            || matches!(
                inner.state,
                DaemonLifecycleStateV1::Starting | DaemonLifecycleStateV1::Stopping
            )
        {
            return;
        }
        if connected {
            inner.state = DaemonLifecycleStateV1::Running;
            inner.managed = false;
            inner.failure = None;
        } else if inner.state == DaemonLifecycleStateV1::Running && !inner.managed {
            inner.state = DaemonLifecycleStateV1::Stopped;
            inner.failure = None;
        }
    }

    fn begin_start(&self) -> DaemonLifecycleV1 {
        self.reconcile();
        let operation_id;
        let snapshot;
        {
            let mut inner = self.lock();
            if matches!(
                inner.state,
                DaemonLifecycleStateV1::Starting
                    | DaemonLifecycleStateV1::Running
                    | DaemonLifecycleStateV1::Stopping
                    | DaemonLifecycleStateV1::Unavailable
            ) || inner.child.is_some()
            {
                return Self::snapshot(&inner);
            }
            inner.operation_id = inner.operation_id.saturating_add(1);
            operation_id = inner.operation_id;
            inner.state = DaemonLifecycleStateV1::Starting;
            inner.managed = true;
            inner.failure = None;
            snapshot = Self::snapshot(&inner);
        }
        let control = self.clone();
        thread::spawn(move || control.complete_start(operation_id));
        snapshot
    }

    fn complete_start(&self, operation_id: u64) {
        let launched = self.launcher.launch(&self.executable, &self.config);
        let mut inner = self.lock();
        match launched {
            Ok(child)
                if inner.operation_id == operation_id
                    && inner.state == DaemonLifecycleStateV1::Starting =>
            {
                inner.child = Some(child);
                inner.state = DaemonLifecycleStateV1::Running;
                inner.managed = true;
                inner.failure = None;
            }
            Ok(child) => {
                drop(inner);
                self.stop_cancelled_start(child);
            }
            Err(()) if inner.operation_id == operation_id => {
                inner.state = DaemonLifecycleStateV1::Failed;
                inner.managed = false;
                inner.failure = Some(spawn_failed());
            }
            Err(()) if inner.state == DaemonLifecycleStateV1::Stopping => {
                inner.state = DaemonLifecycleStateV1::Stopped;
                inner.managed = false;
                inner.failure = None;
            }
            Err(()) => {}
        }
    }

    fn stop_cancelled_start(&self, mut child: Box<dyn ManagedDaemon>) {
        let operation_id = {
            let inner = self.lock();
            inner.operation_id
        };
        if child.request_graceful_stop().is_err() {
            self.finish_stop_failure(operation_id, Some(child), stop_failed());
            return;
        }
        match child.wait_for_exit(self.graceful_timeout) {
            Ok(Some(_)) => {
                let mut inner = self.lock();
                if inner.operation_id == operation_id
                    && inner.state == DaemonLifecycleStateV1::Stopping
                {
                    inner.state = DaemonLifecycleStateV1::Stopped;
                    inner.managed = false;
                    inner.failure = None;
                }
            }
            Ok(None) => self.finish_stop_failure(operation_id, Some(child), stop_timeout()),
            Err(()) => self.finish_stop_failure(operation_id, Some(child), stop_failed()),
        }
    }

    fn begin_stop(&self) -> DaemonLifecycleV1 {
        self.reconcile();
        let operation_id;
        let snapshot;
        {
            let mut inner = self.lock();
            match inner.state {
                DaemonLifecycleStateV1::Stopped
                | DaemonLifecycleStateV1::Stopping
                | DaemonLifecycleStateV1::Unavailable => return Self::snapshot(&inner),
                DaemonLifecycleStateV1::Running if !inner.managed => {
                    inner.state = DaemonLifecycleStateV1::Failed;
                    inner.failure = Some(not_managed());
                    return Self::snapshot(&inner);
                }
                DaemonLifecycleStateV1::Starting if inner.child.is_none() => {
                    inner.operation_id = inner.operation_id.saturating_add(1);
                    inner.state = DaemonLifecycleStateV1::Stopping;
                    inner.managed = true;
                    inner.failure = None;
                    return Self::snapshot(&inner);
                }
                DaemonLifecycleStateV1::Failed if inner.child.is_none() => {
                    return Self::snapshot(&inner);
                }
                _ => {}
            }
            inner.operation_id = inner.operation_id.saturating_add(1);
            operation_id = inner.operation_id;
            inner.state = DaemonLifecycleStateV1::Stopping;
            inner.failure = None;
            snapshot = Self::snapshot(&inner);
        }
        let control = self.clone();
        thread::spawn(move || control.complete_stop(operation_id, false));
        snapshot
    }

    fn begin_restart(&self) -> DaemonLifecycleV1 {
        self.reconcile();
        let operation_id;
        let snapshot;
        {
            let mut inner = self.lock();
            if inner.state == DaemonLifecycleStateV1::Running && !inner.managed {
                inner.state = DaemonLifecycleStateV1::Failed;
                inner.failure = Some(not_managed());
                return Self::snapshot(&inner);
            }
            if inner.state != DaemonLifecycleStateV1::Running || inner.child.is_none() {
                drop(inner);
                return self.begin_start();
            }
            inner.operation_id = inner.operation_id.saturating_add(1);
            operation_id = inner.operation_id;
            inner.state = DaemonLifecycleStateV1::Stopping;
            inner.failure = None;
            snapshot = Self::snapshot(&inner);
        }
        let control = self.clone();
        thread::spawn(move || control.complete_stop(operation_id, true));
        snapshot
    }

    fn complete_stop(&self, operation_id: u64, restart: bool) {
        #[cfg(test)]
        if let Some(gate) = &self.stop_completion_gate {
            let (lock, wake) = &**gate;
            let released = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (released, _) = wake
                .wait_timeout_while(released, Duration::from_secs(5), |released| !*released)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(*released, "test stop completion gate exceeded its deadline");
        }
        let Some(mut child) = ({
            let mut inner = self.lock();
            if inner.operation_id != operation_id || inner.state != DaemonLifecycleStateV1::Stopping
            {
                return;
            }
            inner.child.take()
        }) else {
            self.finish_stop_failure(operation_id, None, stop_failed());
            return;
        };

        if child.request_graceful_stop().is_err() {
            self.finish_stop_failure(operation_id, Some(child), stop_failed());
            return;
        }
        match child.wait_for_exit(self.graceful_timeout) {
            Ok(Some(_)) if restart => {
                {
                    let mut inner = self.lock();
                    if inner.operation_id != operation_id {
                        return;
                    }
                    inner.state = DaemonLifecycleStateV1::Starting;
                    inner.managed = true;
                    inner.failure = None;
                }
                self.complete_start(operation_id);
            }
            Ok(Some(_)) => {
                let mut inner = self.lock();
                if inner.operation_id == operation_id {
                    inner.state = DaemonLifecycleStateV1::Stopped;
                    inner.managed = false;
                    inner.failure = None;
                }
            }
            Ok(None) => self.finish_stop_failure(operation_id, Some(child), stop_timeout()),
            Err(()) => self.finish_stop_failure(operation_id, Some(child), stop_failed()),
        }
    }

    fn finish_stop_failure(
        &self,
        operation_id: u64,
        child: Option<Box<dyn ManagedDaemon>>,
        failure: DaemonLifecycleFailureV1,
    ) {
        let mut inner = self.lock();
        if inner.operation_id == operation_id {
            inner.child = child;
            inner.state = DaemonLifecycleStateV1::Failed;
            inner.managed = inner.child.is_some();
            inner.failure = Some(failure);
        }
    }
}

impl DaemonLifecycleControl for ConfiguredDaemonLifecycle {
    fn lifecycle(&self) -> DaemonLifecycleV1 {
        self.reconcile();
        Self::snapshot(&self.lock())
    }

    fn start(&self) -> DaemonLifecycleV1 {
        self.begin_start()
    }

    fn stop(&self) -> DaemonLifecycleV1 {
        self.begin_stop()
    }

    fn restart(&self) -> DaemonLifecycleV1 {
        self.begin_restart()
    }
}

trait ProcessLauncher: Send + Sync + 'static {
    fn launch(&self, executable: &Path, config: &Path) -> Result<Box<dyn ManagedDaemon>, ()>;
}

trait ManagedDaemon: Send + 'static {
    fn try_wait(&mut self) -> Result<Option<ProcessExit>, ()>;
    fn request_graceful_stop(&mut self) -> Result<(), ()>;
    fn wait_for_exit(&mut self, timeout: Duration) -> Result<Option<ProcessExit>, ()>;
}

#[derive(Clone, Copy)]
struct ProcessExit;

struct OsProcessLauncher;

impl ProcessLauncher for OsProcessLauncher {
    fn launch(&self, executable: &Path, config: &Path) -> Result<Box<dyn ManagedDaemon>, ()> {
        Command::new(executable)
            .arg("--config")
            .arg(config)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .map(|child| Box::new(OsManagedDaemon { child }) as Box<dyn ManagedDaemon>)
            .map_err(|_| ())
    }
}

struct OsManagedDaemon {
    child: Child,
}

impl ManagedDaemon for OsManagedDaemon {
    fn try_wait(&mut self) -> Result<Option<ProcessExit>, ()> {
        self.child
            .try_wait()
            .map(|status| status.map(|_| ProcessExit))
            .map_err(|_| ())
    }

    fn request_graceful_stop(&mut self) -> Result<(), ()> {
        request_graceful_stop(&mut self.child)
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> Result<Option<ProcessExit>, ()> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(exit) = self.try_wait()? {
                return Ok(Some(exit));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
        }
    }
}

#[cfg(unix)]
fn request_graceful_stop(child: &mut Child) -> Result<(), ()> {
    let raw_pid = i32::try_from(child.id()).map_err(|_| ())?;
    let pid = rustix::process::Pid::from_raw(raw_pid).ok_or(())?;
    rustix::process::kill_process(pid, rustix::process::Signal::INT).map_err(|_| ())
}

#[cfg(windows)]
fn request_graceful_stop(child: &mut Child) -> Result<(), ()> {
    Command::new("taskkill.exe")
        .arg("/PID")
        .arg(child.id().to_string())
        .status()
        .map_err(|_| ())?
        .success()
        .then_some(())
        .ok_or(())
}

fn unavailable(code: &str) -> DaemonLifecycleV1 {
    DaemonLifecycleV1 {
        schema: LIFECYCLE_SCHEMA_V1.to_owned(),
        state: DaemonLifecycleStateV1::Unavailable,
        operation_id: 0,
        managed_by_supervisor: false,
        failure: Some(DaemonLifecycleFailureV1 {
            code: code.to_owned(),
            explanation: "The Supervisor could not complete the lifecycle operation.".to_owned(),
            next_action: "Retry the operation or restart Studio Supervisor.".to_owned(),
        }),
    }
}

fn configuration_unavailable() -> DaemonLifecycleFailureV1 {
    DaemonLifecycleFailureV1 {
        code: "configuration_unavailable".to_owned(),
        explanation: "The configured daemon executable or configuration is unavailable.".to_owned(),
        next_action: "Restore the configured installation files, then retry.".to_owned(),
    }
}

fn spawn_failed() -> DaemonLifecycleFailureV1 {
    DaemonLifecycleFailureV1 {
        code: "start_failed".to_owned(),
        explanation: "The configured daemon could not be started.".to_owned(),
        next_action: "Check the local installation and configuration, then retry.".to_owned(),
    }
}

fn stop_failed() -> DaemonLifecycleFailureV1 {
    DaemonLifecycleFailureV1 {
        code: "stop_failed".to_owned(),
        explanation: "The graceful stop request could not be completed.".to_owned(),
        next_action: "Check the daemon process, then retry the stop.".to_owned(),
    }
}

fn stop_timeout() -> DaemonLifecycleFailureV1 {
    DaemonLifecycleFailureV1 {
        code: "stop_timeout".to_owned(),
        explanation: "The daemon did not exit before the graceful stop timeout.".to_owned(),
        next_action: "Allow current work to finish, then retry the stop.".to_owned(),
    }
}

fn not_managed() -> DaemonLifecycleFailureV1 {
    DaemonLifecycleFailureV1 {
        code: "not_managed".to_owned(),
        explanation: "The live daemon was not started by this Supervisor session.".to_owned(),
        next_action: "Stop it from its original process owner, then retry here.".to_owned(),
    }
}

fn unexpected_exit() -> DaemonLifecycleFailureV1 {
    DaemonLifecycleFailureV1 {
        code: "unexpected_exit".to_owned(),
        explanation: "The managed daemon exited outside a requested stop.".to_owned(),
        next_action: "Review daemon diagnostics, then retry the start.".to_owned(),
    }
}

fn process_state_failed() -> DaemonLifecycleFailureV1 {
    DaemonLifecycleFailureV1 {
        code: "process_state_failed".to_owned(),
        explanation: "The Supervisor could not determine the managed daemon state.".to_owned(),
        next_action: "Restart Studio Supervisor before retrying lifecycle control.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        sync::{
            Arc, Condvar, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    use super::{
        ConfiguredDaemonLifecycle, DaemonLifecycleStateV1, DaemonLifecycleV1, ManagedDaemon,
        ProcessExit, ProcessLauncher, StopCompletionGate, lifecycle_router,
    };
    use crate::{
        DaemonConnectivityV1, DaemonHealthV1, DaemonReadinessV1, DaemonStatusSource,
        DaemonStatusV1, DaemonVersionV1,
    };

    static NEXT_FIXTURE_ID: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone)]
    struct FixedStatus(DaemonStatusV1);

    impl DaemonStatusSource for FixedStatus {
        fn status(&self) -> DaemonStatusV1 {
            self.0.clone()
        }
    }

    #[derive(Clone, Default)]
    struct FakeLauncher {
        launches: Arc<AtomicUsize>,
        launch_gate: Arc<(Mutex<bool>, Condvar)>,
        exit_gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl FakeLauncher {
        fn allow_launch(&self) {
            let (lock, wake) = &*self.launch_gate;
            *lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            wake.notify_all();
        }

        fn allow_exit(&self) {
            let (lock, wake) = &*self.exit_gate;
            *lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            wake.notify_all();
        }
    }

    impl ProcessLauncher for FakeLauncher {
        fn launch(&self, executable: &Path, config: &Path) -> Result<Box<dyn ManagedDaemon>, ()> {
            assert!(executable.ends_with("worldstreamd"));
            assert!(config.ends_with("worldstream.toml"));
            let launch_number = self.launches.fetch_add(1, Ordering::SeqCst);
            let (lock, wake) = &*self.launch_gate;
            let mut allowed = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while !*allowed {
                allowed = wake
                    .wait(allowed)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if launch_number > 0 {
                *self
                    .exit_gate
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
            }
            Ok(Box::new(FakeDaemon {
                exit_gate: Arc::clone(&self.exit_gate),
            }))
        }
    }

    struct FakeDaemon {
        exit_gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl ManagedDaemon for FakeDaemon {
        fn try_wait(&mut self) -> Result<Option<ProcessExit>, ()> {
            let exited = *self
                .exit_gate
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Ok(exited.then_some(ProcessExit))
        }

        fn request_graceful_stop(&mut self) -> Result<(), ()> {
            Ok(())
        }

        fn wait_for_exit(&mut self, timeout: Duration) -> Result<Option<ProcessExit>, ()> {
            let (lock, wake) = &*self.exit_gate;
            let exited = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (exited, _) = wake
                .wait_timeout_while(exited, timeout, |exited| !*exited)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Ok((*exited).then_some(ProcessExit))
        }
    }

    #[tokio::test]
    async fn repeated_start_is_idempotent_and_uses_only_the_fixed_configuration() {
        let fixture = Fixture::new(DaemonStatusV1::unavailable());
        let first = fixture.post("/api/v1/daemon/start").await;
        let second = fixture
            .post_with_body(
                "/api/v1/daemon/start",
                r#"{"executable":"/bin/sh","config":"/tmp/hostile"}"#,
            )
            .await;

        assert_eq!(first.state, DaemonLifecycleStateV1::Starting);
        assert_eq!(second.state, DaemonLifecycleStateV1::Starting);
        assert_eq!(second.operation_id, first.operation_id);
        fixture.launcher.allow_launch();
        fixture.wait_for(DaemonLifecycleStateV1::Running).await;
        assert_eq!(fixture.launcher.launches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stop_during_start_tracks_and_gracefully_reaps_the_created_process() {
        let fixture = Fixture::new(DaemonStatusV1::unavailable());
        assert_eq!(
            fixture.post("/api/v1/daemon/start").await.state,
            DaemonLifecycleStateV1::Starting
        );

        let stopping = fixture.post("/api/v1/daemon/stop").await;
        assert_eq!(stopping.state, DaemonLifecycleStateV1::Stopping);
        fixture.launcher.allow_exit();
        fixture.launcher.allow_launch();
        fixture.wait_for(DaemonLifecycleStateV1::Stopped).await;
        assert_eq!(fixture.launcher.launches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn graceful_stop_and_restart_expose_intermediate_states() {
        let fixture = Fixture::new(DaemonStatusV1::unavailable());
        fixture.launcher.allow_launch();
        fixture.post("/api/v1/daemon/start").await;
        fixture.wait_for(DaemonLifecycleStateV1::Running).await;

        let stopping = fixture.post("/api/v1/daemon/restart").await;
        assert_eq!(stopping.state, DaemonLifecycleStateV1::Stopping);
        assert!(stopping.managed_by_supervisor);
        fixture.launcher.allow_exit();
        fixture.wait_for(DaemonLifecycleStateV1::Running).await;
        assert_eq!(fixture.launcher.launches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn lifecycle_read_cannot_consume_an_exit_owned_by_restart() {
        let pause = StopCompletionPause(Arc::new((Mutex::new(false), Condvar::new())));
        let fixture = Fixture::with_stop_completion_gate(
            DaemonStatusV1::unavailable(),
            Duration::from_secs(1),
            Some(Arc::clone(&pause.0)),
        );
        fixture.launcher.allow_launch();
        fixture.post("/api/v1/daemon/start").await;
        fixture.wait_for(DaemonLifecycleStateV1::Running).await;

        let restarting = fixture.post("/api/v1/daemon/restart").await;
        assert_eq!(restarting.state, DaemonLifecycleStateV1::Stopping);
        fixture.launcher.allow_exit();
        // Force a read after exit but before the restart worker can take the
        // child. Polling must not consume the worker's accepted operation.
        let observed = fixture.get().await;
        assert_eq!(observed.state, DaemonLifecycleStateV1::Stopping);
        assert_eq!(observed.operation_id, restarting.operation_id);
        assert!(observed.managed_by_supervisor);
        for route in ["/api/v1/daemon/stop", "/api/v1/daemon/restart"] {
            assert_eq!(fixture.post(route).await, observed);
        }
        assert_eq!(fixture.launcher.launches.load(Ordering::SeqCst), 1);
        pause.release();
        let running = fixture.wait_for(DaemonLifecycleStateV1::Running).await;
        assert_eq!(running.operation_id, restarting.operation_id);
        assert_eq!(fixture.launcher.launches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn exit_without_a_stop_operation_is_still_unexpected() {
        let fixture = Fixture::new(DaemonStatusV1::unavailable());
        fixture.launcher.allow_launch();
        fixture.post("/api/v1/daemon/start").await;
        fixture.wait_for(DaemonLifecycleStateV1::Running).await;

        fixture.launcher.allow_exit();
        let failed = fixture.get().await;
        assert_eq!(failed.state, DaemonLifecycleStateV1::Failed);
        assert!(!failed.managed_by_supervisor);
        assert_eq!(
            failed.failure.as_ref().map(|failure| failure.code.as_str()),
            Some("unexpected_exit")
        );
        assert_eq!(fixture.launcher.launches.load(Ordering::SeqCst), 1);
    }

    struct StopCompletionPause(StopCompletionGate);

    impl StopCompletionPause {
        fn release(&self) {
            let (lock, wake) = &*self.0;
            *lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            wake.notify_all();
        }
    }

    impl Drop for StopCompletionPause {
        fn drop(&mut self) {
            self.release();
        }
    }

    #[tokio::test]
    async fn supervisor_restart_reconciles_a_live_daemon_without_duplicate_or_unsafe_control() {
        let fixture = Fixture::new(connected_status());

        let running = fixture.get().await;
        assert_eq!(running.state, DaemonLifecycleStateV1::Running);
        assert!(!running.managed_by_supervisor);
        assert_eq!(fixture.post("/api/v1/daemon/start").await, running);
        assert_eq!(fixture.launcher.launches.load(Ordering::SeqCst), 0);

        let failed = fixture.post("/api/v1/daemon/stop").await;
        assert_eq!(failed.state, DaemonLifecycleStateV1::Failed);
        let failure = failed.failure.unwrap_or_else(|| unreachable!("failure"));
        assert_eq!(failure.code, "not_managed");
        assert!(!failure.explanation.contains('/'));
        assert!(!failure.next_action.is_empty());
    }

    #[tokio::test]
    async fn graceful_stop_timeout_fails_safely_without_forcing_the_daemon() {
        let fixture =
            Fixture::with_timeout(DaemonStatusV1::unavailable(), Duration::from_millis(20));
        fixture.launcher.allow_launch();
        fixture.post("/api/v1/daemon/start").await;
        fixture.wait_for(DaemonLifecycleStateV1::Running).await;

        let stopping = fixture.post("/api/v1/daemon/stop").await;
        assert_eq!(stopping.state, DaemonLifecycleStateV1::Stopping);
        let failed = fixture.wait_for(DaemonLifecycleStateV1::Failed).await;
        assert!(failed.managed_by_supervisor);
        let failure = failed.failure.unwrap_or_else(|| unreachable!("failure"));
        assert_eq!(failure.code, "stop_timeout");
        assert!(!failure.next_action.is_empty());
    }

    #[tokio::test]
    async fn missing_fixed_configuration_is_unavailable_without_leaking_paths() {
        let control = ConfiguredDaemonLifecycle::with_launcher(
            Path::new("/missing/private/worldstreamd").to_path_buf(),
            Path::new("/missing/private/worldstream.toml").to_path_buf(),
            Duration::from_secs(1),
            FixedStatus(DaemonStatusV1::unavailable()),
            FakeLauncher::default(),
        );
        let router = lifecycle_router(control);
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/daemon/lifecycle")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let lifecycle: DaemonLifecycleV1 = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("lifecycle JSON: {error}"));

        assert_eq!(lifecycle.state, DaemonLifecycleStateV1::Unavailable);
        assert!(!String::from_utf8_lossy(&body).contains("/missing/private"));
    }

    #[tokio::test]
    async fn arbitrary_command_and_path_routes_remain_absent() {
        let fixture = Fixture::new(DaemonStatusV1::unavailable());
        for route in [
            "/api/v1/command",
            "/api/v1/daemon/exec",
            "/api/v1/daemon/path",
        ] {
            let response = fixture
                .router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(route)
                        .body(Body::empty())
                        .unwrap_or_else(|error| unreachable!("request: {error}")),
                )
                .await
                .unwrap_or_else(|error| unreachable!("response: {error}"));
            assert_eq!(response.status(), 404);
        }
    }

    struct Fixture {
        router: axum::Router,
        launcher: FakeLauncher,
    }

    fn connected_status() -> DaemonStatusV1 {
        DaemonStatusV1 {
            schema: "worldstream/studio-daemon-status/v1".to_owned(),
            connectivity: DaemonConnectivityV1::Connected,
            health: DaemonHealthV1::Live,
            readiness: DaemonReadinessV1::Ready,
            version: Some(DaemonVersionV1 {
                product: "0.1.0".to_owned(),
                build_version: "0.1.0".to_owned(),
                source_revision: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            }),
            unavailable_reason: None,
        }
    }

    impl Fixture {
        fn new(status: DaemonStatusV1) -> Self {
            Self::with_timeout(status, Duration::from_secs(1))
        }

        fn with_timeout(status: DaemonStatusV1, timeout: Duration) -> Self {
            Self::with_stop_completion_gate(status, timeout, None)
        }

        fn with_stop_completion_gate(
            status: DaemonStatusV1,
            timeout: Duration,
            stop_completion_gate: Option<StopCompletionGate>,
        ) -> Self {
            let suffix = format!(
                "{}-{}",
                std::process::id(),
                NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
            );
            let root = std::env::temp_dir().join(format!("worldstream-lifecycle-{suffix}"));
            fs::create_dir_all(&root)
                .unwrap_or_else(|error| unreachable!("create fixture: {error}"));
            let executable = root.join("worldstreamd");
            let config = root.join("worldstream.toml");
            fs::write(&executable, b"fixture")
                .unwrap_or_else(|error| unreachable!("write executable fixture: {error}"));
            fs::write(&config, b"schema_version = 1")
                .unwrap_or_else(|error| unreachable!("write config fixture: {error}"));
            let launcher = FakeLauncher::default();
            let mut control = ConfiguredDaemonLifecycle::with_launcher(
                executable,
                config,
                timeout,
                FixedStatus(status),
                launcher.clone(),
            );
            control.stop_completion_gate = stop_completion_gate;
            Self {
                router: lifecycle_router(control),
                launcher,
            }
        }

        async fn get(&self) -> DaemonLifecycleV1 {
            self.request("GET", "/api/v1/daemon/lifecycle", "").await
        }

        async fn post(&self, path: &str) -> DaemonLifecycleV1 {
            self.post_with_body(path, "").await
        }

        async fn post_with_body(&self, path: &str, body: &str) -> DaemonLifecycleV1 {
            self.request("POST", path, body).await
        }

        async fn request(&self, method: &str, path: &str, body: &str) -> DaemonLifecycleV1 {
            let response = self
                .router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::from(body.to_owned()))
                        .unwrap_or_else(|error| unreachable!("request: {error}")),
                )
                .await
                .unwrap_or_else(|error| unreachable!("response: {error}"));
            assert_eq!(response.status(), 200);
            let body = response
                .into_body()
                .collect()
                .await
                .unwrap_or_else(|error| unreachable!("body: {error}"))
                .to_bytes();
            serde_json::from_slice(&body)
                .unwrap_or_else(|error| unreachable!("lifecycle JSON: {error}"))
        }

        async fn wait_for(&self, state: DaemonLifecycleStateV1) -> DaemonLifecycleV1 {
            for _ in 0..100 {
                let lifecycle = self.get().await;
                if lifecycle.state == state {
                    return lifecycle;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            unreachable!("lifecycle did not reach {state:?}")
        }
    }
}
