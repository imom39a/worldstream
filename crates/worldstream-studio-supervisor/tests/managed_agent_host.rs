#![allow(dead_code)]

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use tempfile::tempdir;

use worldstream_studio_supervisor::agent_profiles::ManagedReferenceProviderV1;
use worldstream_studio_supervisor::managed_agent_host::{
    ManagedAgentHostErrorV1, ManagedAgentHostLaunchPlanV1, ManagedAgentHostOperationsV1,
    ManagedAgentHostPreparedLaunchV1, ManagedAgentHostProcessLauncherV1, ManagedAgentHostProcessV1,
    ManagedAgentHostProfileV1, ManagedAgentHostStartSourceV1, OsManagedAgentHostProcessLauncherV1,
};

const ASSIGNMENT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const SECOND_ASSIGNMENT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const SECRET_REFERENCE: &str = "abababababababababababababababababababababababababababababababab";

fn provider_address() -> SocketAddr {
    "127.0.0.1:11434"
        .parse()
        .unwrap_or_else(|error| unreachable!("provider address: {error}"))
}

#[derive(Clone)]
struct FixedStartSource {
    root: std::path::PathBuf,
    capacity: u32,
    stale_after: Duration,
}

impl FixedStartSource {
    fn new(root: std::path::PathBuf) -> Self {
        Self {
            root,
            capacity: 1,
            stale_after: Duration::from_secs(30),
        }
    }
}

impl ManagedAgentHostStartSourceV1 for FixedStartSource {
    fn prepare(
        &self,
        assignment_id: &str,
    ) -> Result<ManagedAgentHostPreparedLaunchV1, ManagedAgentHostErrorV1> {
        let profile = ManagedAgentHostProfileV1::new(
            assignment_id,
            "reference-agent-host",
            "r1",
            "openai-compatible",
            provider_address(),
            "test-model",
            SECRET_REFERENCE,
            self.capacity,
            self.stale_after,
        )?;
        Ok(ManagedAgentHostPreparedLaunchV1::new(
            profile,
            ManagedAgentHostLaunchPlanV1::new(
                &self.root.join("worldstream-assignment-mcp"),
                &self.root,
                SECRET_REFERENCE,
                &self.root.join("reference-agent-host"),
                "openai-compatible",
                provider_address(),
                "test-model",
            )?,
            zeroize::Zeroizing::new(b"provider-secret".to_vec()),
        ))
    }
}

#[derive(Clone, Default)]
struct RecordingProcessLauncher(Arc<AtomicUsize>);

impl ManagedAgentHostProcessLauncherV1 for RecordingProcessLauncher {
    fn launch_bridged(
        &self,
        _plan: &ManagedAgentHostLaunchPlanV1,
        credential: &zeroize::Zeroizing<Vec<u8>>,
    ) -> Result<Box<dyn ManagedAgentHostProcessV1>, ManagedAgentHostErrorV1> {
        assert_eq!(credential.as_slice(), b"provider-secret");
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FixedProcess {
            observed_at_ms: test_now_ms(),
        }))
    }
}

struct FixedProcess {
    observed_at_ms: u64,
}

#[derive(Clone, Copy)]
struct ExitingProcessLauncher;

impl ManagedAgentHostProcessLauncherV1 for ExitingProcessLauncher {
    fn launch_bridged(
        &self,
        _plan: &ManagedAgentHostLaunchPlanV1,
        _credential: &zeroize::Zeroizing<Vec<u8>>,
    ) -> Result<Box<dyn ManagedAgentHostProcessV1>, ManagedAgentHostErrorV1> {
        Ok(Box::new(ExitedProcess))
    }
}

struct ExitedProcess;

impl ManagedAgentHostProcessV1 for ExitedProcess {
    fn try_wait(&mut self) -> Result<Option<i32>, ManagedAgentHostErrorV1> {
        Ok(Some(17))
    }

    fn stop(&mut self) -> Result<(), ManagedAgentHostErrorV1> {
        Ok(())
    }
}

impl ManagedAgentHostProcessV1 for FixedProcess {
    fn try_wait(&mut self) -> Result<Option<i32>, ManagedAgentHostErrorV1> {
        Ok(None)
    }

    fn last_activity_at_ms(&self) -> Option<u64> {
        Some(self.observed_at_ms)
    }

    fn stop(&mut self) -> Result<(), ManagedAgentHostErrorV1> {
        Ok(())
    }
}

#[derive(Clone)]
struct RetryableStopLauncher(Arc<AtomicUsize>);

impl ManagedAgentHostProcessLauncherV1 for RetryableStopLauncher {
    fn launch_bridged(
        &self,
        _plan: &ManagedAgentHostLaunchPlanV1,
        _credential: &zeroize::Zeroizing<Vec<u8>>,
    ) -> Result<Box<dyn ManagedAgentHostProcessV1>, ManagedAgentHostErrorV1> {
        Ok(Box::new(RetryableStopProcess(Arc::clone(&self.0))))
    }
}

struct RetryableStopProcess(Arc<AtomicUsize>);

impl ManagedAgentHostProcessV1 for RetryableStopProcess {
    fn try_wait(&mut self) -> Result<Option<i32>, ManagedAgentHostErrorV1> {
        Ok(None)
    }

    fn last_activity_at_ms(&self) -> Option<u64> {
        Some(test_now_ms())
    }

    fn stop(&mut self) -> Result<(), ManagedAgentHostErrorV1> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(ManagedAgentHostErrorV1::Unavailable)
        } else {
            Ok(())
        }
    }
}

#[test]
fn failed_stop_retains_the_owned_agent_host_for_a_later_retry()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempdir()?;
    let root = directory.path().canonicalize()?;
    let stop_calls = Arc::new(AtomicUsize::new(0));
    let operations = ManagedAgentHostOperationsV1::open_with(
        &root.join("operations"),
        FixedStartSource::new(root),
        RetryableStopLauncher(Arc::clone(&stop_calls)),
    )?;
    operations.start(ASSIGNMENT)?;
    assert!(matches!(
        operations.stop(ASSIGNMENT),
        Err(ManagedAgentHostErrorV1::Unavailable)
    ));
    assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
    let stopped = operations.stop(ASSIGNMENT)?;
    assert_eq!(stop_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        stopped.state,
        worldstream_studio_supervisor::managed_agent_host::ManagedAgentHostStateV1::Stopped
    );
    assert!(!stopped.ready);
    Ok(())
}

#[test]
fn stop_without_an_owned_handle_does_not_claim_that_an_unresolved_host_exited()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempdir()?;
    let root = directory.path().canonicalize()?;
    let source = FixedStartSource::new(root.clone());
    let launcher = RecordingProcessLauncher::default();
    let original = ManagedAgentHostOperationsV1::open_with(
        &root.join("operations"),
        source.clone(),
        launcher.clone(),
    )?;
    original.start(ASSIGNMENT)?;
    drop(original);
    let reopened =
        ManagedAgentHostOperationsV1::open_with(&root.join("operations"), source, launcher)?;
    assert!(matches!(
        reopened.stop(ASSIGNMENT),
        Err(ManagedAgentHostErrorV1::Unavailable)
    ));
    let status = reopened.status(ASSIGNMENT)?;
    assert_eq!(
        status.state,
        worldstream_studio_supervisor::managed_agent_host::ManagedAgentHostStateV1::NeedsAttention
    );
    assert!(!status.ready);
    Ok(())
}

mod coordinated_lifecycle {
    use super::*;
    use std::sync::{Mutex, atomic::AtomicBool};
    use worldstream_runtime::prepare_data_directory;
    use worldstream_studio_supervisor::{
        managed_agent_host::ManagedAgentHostStateV1,
        managed_lifecycle::{
            LifecycleError, LifecycleStage, ManagedLifecycle, RuntimeControl, RuntimeObservation,
        },
        runner_templates::{RunnerSupervisorV1, RunnerTemplateRegistryV1},
        secrets::FileSecretVaultV1,
    };

    // Only the OS process seam is substituted. The operation stores, Runner
    // registry, managed-host lifecycle and restart coordination remain real.
    #[derive(Clone)]
    struct Processes {
        active_hosts: Arc<AtomicUsize>,
        runtime_running: Arc<AtomicBool>,
        fail_runtime_starts: Arc<AtomicUsize>,
        stop_gate: Option<Arc<StopGate>>,
    }

    struct StopGate {
        entered: std::sync::mpsc::SyncSender<()>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl ManagedAgentHostProcessLauncherV1 for Processes {
        fn launch_bridged(
            &self,
            _plan: &ManagedAgentHostLaunchPlanV1,
            _credential: &zeroize::Zeroizing<Vec<u8>>,
        ) -> Result<Box<dyn ManagedAgentHostProcessV1>, ManagedAgentHostErrorV1> {
            if !self.runtime_running.load(Ordering::SeqCst) {
                return Err(ManagedAgentHostErrorV1::Unavailable);
            }
            self.active_hosts.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(HostProcess {
                processes: self.clone(),
                stopped: false,
            }))
        }
    }

    struct HostProcess {
        processes: Processes,
        stopped: bool,
    }

    impl ManagedAgentHostProcessV1 for HostProcess {
        fn try_wait(&mut self) -> Result<Option<i32>, ManagedAgentHostErrorV1> {
            Ok(self.stopped.then_some(0))
        }

        fn last_activity_at_ms(&self) -> Option<u64> {
            Some(test_now_ms())
        }

        fn stop(&mut self) -> Result<(), ManagedAgentHostErrorV1> {
            if !self.stopped {
                self.processes.active_hosts.fetch_sub(1, Ordering::SeqCst);
                self.stopped = true;
            }
            Ok(())
        }
    }

    impl RuntimeControl for Processes {
        fn observe(&self) -> RuntimeObservation {
            if self.runtime_running.load(Ordering::SeqCst) {
                RuntimeObservation::Ready
            } else {
                RuntimeObservation::Stopped
            }
        }

        fn start(&self) -> Result<(), LifecycleError> {
            if self
                .fail_runtime_starts
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(LifecycleError::Unavailable);
            }
            self.runtime_running.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn stop(&self) -> Result<(), LifecycleError> {
            if let Some(gate) = &self.stop_gate {
                gate.entered
                    .send(())
                    .map_err(|_| LifecycleError::Unavailable)?;
                gate.release
                    .lock()
                    .map_err(|_| LifecycleError::Unavailable)?
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|_| LifecycleError::Unavailable)?;
            }
            if self.active_hosts.load(Ordering::SeqCst) != 0 {
                return Err(LifecycleError::Unavailable);
            }
            self.runtime_running.store(false, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn restart_stops_owned_runners_before_runtime_and_restores_only_the_running_set()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let state = prepare_data_directory(&temporary.path().join("state"))?;
        let processes = Processes {
            active_hosts: Arc::new(AtomicUsize::new(0)),
            runtime_running: Arc::new(AtomicBool::new(true)),
            fail_runtime_starts: Arc::new(AtomicUsize::new(0)),
            stop_gate: None,
        };
        let mut source = FixedStartSource::new(state.clone());
        source.capacity = 2;
        let hosts = ManagedAgentHostOperationsV1::open_with(
            &state.join("managed-hosts"),
            source,
            processes.clone(),
        )?;
        hosts.start(ASSIGNMENT)?;
        hosts.start(SECOND_ASSIGNMENT)?;
        hosts.stop(SECOND_ASSIGNMENT)?;
        let runners = RunnerSupervisorV1::open(
            RunnerTemplateRegistryV1::open_installed(&state.join("runner-templates"))?,
            &state.join("runner-runtime"),
            FileSecretVaultV1::open(&state.join("secrets"))?,
            Duration::from_secs(1),
        )?;
        let lifecycle =
            ManagedLifecycle::open(&state, processes.clone(), runners.clone(), hosts.clone())?;
        let result = lifecycle.restart()?;
        assert_eq!(result.stage, LifecycleStage::Complete);
        assert!(result.restore_remaining.is_empty());
        assert_eq!(processes.active_hosts.load(Ordering::SeqCst), 1);
        assert!(hosts.status(ASSIGNMENT)?.ready);
        assert_eq!(
            hosts.status(SECOND_ASSIGNMENT)?.state,
            ManagedAgentHostStateV1::Stopped
        );
        drop(lifecycle);
        let reopened = ManagedLifecycle::open(&state, processes, runners, hosts)?;
        let retained = reopened
            .status()?
            .operation
            .ok_or("missing retained lifecycle operation")?;
        assert_eq!(retained.operation_id, result.operation_id);
        assert_eq!(retained.stage, LifecycleStage::Complete);
        Ok(())
    }

    #[test]
    fn retry_after_runtime_start_failure_preserves_the_original_restore_set()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let state = prepare_data_directory(&temporary.path().join("state"))?;
        let processes = Processes {
            active_hosts: Arc::new(AtomicUsize::new(0)),
            runtime_running: Arc::new(AtomicBool::new(true)),
            fail_runtime_starts: Arc::new(AtomicUsize::new(1)),
            stop_gate: None,
        };
        let hosts = ManagedAgentHostOperationsV1::open_with(
            &state.join("managed-hosts"),
            FixedStartSource::new(state.clone()),
            processes.clone(),
        )?;
        hosts.start(ASSIGNMENT)?;
        let runners = RunnerSupervisorV1::open(
            RunnerTemplateRegistryV1::open_installed(&state.join("runner-templates"))?,
            &state.join("runner-runtime"),
            FileSecretVaultV1::open(&state.join("secrets"))?,
            Duration::from_secs(1),
        )?;
        let lifecycle =
            ManagedLifecycle::open(&state, processes.clone(), runners.clone(), hosts.clone())?;
        assert!(matches!(
            lifecycle.restart(),
            Err(LifecycleError::Unavailable)
        ));
        let checkpoint = lifecycle
            .status()?
            .operation
            .ok_or("missing partial restart")?;
        assert_eq!(checkpoint.stage, LifecycleStage::RuntimeRestart);
        assert_eq!(checkpoint.restore_remaining.len(), 1);
        assert_eq!(processes.active_hosts.load(Ordering::SeqCst), 0);
        drop(lifecycle);
        let reopened = ManagedLifecycle::open(&state, processes.clone(), runners, hosts.clone())?;
        let resumed = reopened.restart()?;
        assert_eq!(resumed.operation_id, checkpoint.operation_id);
        assert_eq!(resumed.stage, LifecycleStage::Complete);
        assert_eq!(processes.active_hosts.load(Ordering::SeqCst), 1);
        assert!(hosts.status(ASSIGNMENT)?.ready);
        Ok(())
    }

    #[test]
    fn new_managed_start_cannot_race_with_the_captured_restart_set()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let state = prepare_data_directory(&temporary.path().join("state"))?;
        let (entered, reached_stop) = std::sync::mpsc::sync_channel(1);
        let (release, proceed) = std::sync::mpsc::sync_channel(1);
        let processes = Processes {
            active_hosts: Arc::new(AtomicUsize::new(0)),
            runtime_running: Arc::new(AtomicBool::new(true)),
            fail_runtime_starts: Arc::new(AtomicUsize::new(0)),
            stop_gate: Some(Arc::new(StopGate {
                entered,
                release: Mutex::new(proceed),
            })),
        };
        let mut source = FixedStartSource::new(state.clone());
        source.capacity = 2;
        let hosts = ManagedAgentHostOperationsV1::open_with(
            &state.join("managed-hosts"),
            source,
            processes.clone(),
        )?;
        hosts.start(ASSIGNMENT)?;
        let runners = RunnerSupervisorV1::open(
            RunnerTemplateRegistryV1::open_installed(&state.join("runner-templates"))?,
            &state.join("runner-runtime"),
            FileSecretVaultV1::open(&state.join("secrets"))?,
            Duration::from_secs(1),
        )?;
        let lifecycle = ManagedLifecycle::open(&state, processes.clone(), runners, hosts.clone())?;
        let restart = std::thread::spawn(move || lifecycle.restart());
        reached_stop.recv_timeout(Duration::from_secs(5))?;
        let unexpected_start = hosts.start(SECOND_ASSIGNMENT);
        release.send(())?;
        let restarted = restart.join().map_err(|_| "restart thread failed")?;
        assert!(matches!(
            unexpected_start,
            Err(ManagedAgentHostErrorV1::Unavailable)
        ));
        assert_eq!(restarted?.stage, LifecycleStage::Complete);
        assert_eq!(processes.active_hosts.load(Ordering::SeqCst), 1);
        assert!(hosts.status(ASSIGNMENT)?.ready);
        assert!(
            hosts.start(SECOND_ASSIGNMENT)?.ready,
            "starts reopen after completion"
        );
        Ok(())
    }

    #[test]
    fn explicit_stop_then_start_does_not_restore_previously_stopped_hosts()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let state = prepare_data_directory(&temporary.path().join("state"))?;
        let processes = Processes {
            active_hosts: Arc::new(AtomicUsize::new(0)),
            runtime_running: Arc::new(AtomicBool::new(true)),
            fail_runtime_starts: Arc::new(AtomicUsize::new(0)),
            stop_gate: None,
        };
        let hosts = ManagedAgentHostOperationsV1::open_with(
            &state.join("managed-hosts"),
            FixedStartSource::new(state.clone()),
            processes.clone(),
        )?;
        hosts.start(ASSIGNMENT)?;
        let runners = RunnerSupervisorV1::open(
            RunnerTemplateRegistryV1::open_installed(&state.join("runner-templates"))?,
            &state.join("runner-runtime"),
            FileSecretVaultV1::open(&state.join("secrets"))?,
            Duration::from_secs(1),
        )?;
        let lifecycle =
            ManagedLifecycle::open(&state, processes.clone(), runners.clone(), hosts.clone())?;
        let stopped = lifecycle.stop()?;
        assert_eq!(stopped.stage, LifecycleStage::Complete);
        assert_eq!(lifecycle.status()?.runtime, RuntimeObservation::Stopped);
        assert_eq!(processes.active_hosts.load(Ordering::SeqCst), 0);
        assert!(hosts.start(ASSIGNMENT).is_err());
        drop(lifecycle);
        let reopened = ManagedLifecycle::open(&state, processes.clone(), runners, hosts.clone())?;
        assert_eq!(reopened.stop()?.stage, LifecycleStage::Complete);
        assert_eq!(reopened.start()?.stage, LifecycleStage::Complete);
        assert_eq!(reopened.status()?.runtime, RuntimeObservation::Ready);
        assert_eq!(
            hosts.status(ASSIGNMENT)?.state,
            ManagedAgentHostStateV1::Stopped
        );
        assert_eq!(processes.active_hosts.load(Ordering::SeqCst), 0);
        assert!(hosts.start(ASSIGNMENT)?.ready);
        Ok(())
    }

    #[test]
    fn unowned_retained_host_blocks_runtime_stop_and_restart()
    -> Result<(), Box<dyn std::error::Error>> {
        for restart in [false, true] {
            let temporary = tempdir()?;
            let state = prepare_data_directory(&temporary.path().join("state"))?;
            let processes = Processes {
                active_hosts: Arc::new(AtomicUsize::new(0)),
                runtime_running: Arc::new(AtomicBool::new(true)),
                fail_runtime_starts: Arc::new(AtomicUsize::new(0)),
                stop_gate: None,
            };
            let hosts = ManagedAgentHostOperationsV1::open_with(
                &state.join("managed-hosts"),
                FixedStartSource::new(state.clone()),
                processes.clone(),
            )?;
            hosts.start(ASSIGNMENT)?;
            drop(hosts);
            let reopened = ManagedAgentHostOperationsV1::open_with(
                &state.join("managed-hosts"),
                FixedStartSource::new(state.clone()),
                processes.clone(),
            )?;
            // The Runtime's OS-control boundary cannot know about a Controller's
            // lost child handles. Coordination must reject unresolved ownership.
            let runtime = Processes {
                active_hosts: Arc::new(AtomicUsize::new(0)),
                ..processes.clone()
            };
            let runners = RunnerSupervisorV1::open(
                RunnerTemplateRegistryV1::open_installed(&state.join("runner-templates"))?,
                &state.join("runner-runtime"),
                FileSecretVaultV1::open(&state.join("secrets"))?,
                Duration::from_secs(1),
            )?;
            let lifecycle = ManagedLifecycle::open(&state, runtime, runners, reopened)?;
            let outcome = if restart {
                lifecycle.restart()
            } else {
                lifecycle.stop()
            };
            assert!(matches!(outcome, Err(LifecycleError::Unavailable)));
            assert!(processes.runtime_running.load(Ordering::SeqCst));
            assert_eq!(processes.active_hosts.load(Ordering::SeqCst), 1);
            assert!(lifecycle.status()?.operation.is_none());
        }
        Ok(())
    }

    #[test]
    fn retained_lifecycle_logs_are_bounded_ordered_and_read_without_starting()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let state = prepare_data_directory(&temporary.path().join("state"))?;
        let processes = Processes {
            active_hosts: Arc::new(AtomicUsize::new(0)),
            runtime_running: Arc::new(AtomicBool::new(true)),
            fail_runtime_starts: Arc::new(AtomicUsize::new(0)),
            stop_gate: None,
        };
        let hosts = ManagedAgentHostOperationsV1::open_with(
            &state.join("managed-hosts"),
            FixedStartSource::new(state.clone()),
            processes.clone(),
        )?;
        let runners = RunnerSupervisorV1::open(
            RunnerTemplateRegistryV1::open_installed(&state.join("runner-templates"))?,
            &state.join("runner-runtime"),
            FileSecretVaultV1::open(&state.join("secrets"))?,
            Duration::from_secs(1),
        )?;
        let lifecycle =
            ManagedLifecycle::open(&state, processes.clone(), runners.clone(), hosts.clone())?;
        assert!(lifecycle.logs(100)?.is_empty());
        for _ in 0..20 {
            lifecycle.start()?;
            lifecycle.stop()?;
        }
        let expected = lifecycle.logs(3)?;
        assert_eq!(expected.len(), 3);
        assert_eq!(
            expected.last().ok_or("missing final log")?.stage,
            LifecycleStage::Complete
        );
        assert_eq!(expected.last().ok_or("missing final log")?.operation_id, 40);
        assert!(
            expected
                .windows(2)
                .all(|pair| pair[0].sequence < pair[1].sequence)
        );
        assert!(lifecycle.logs(1001).is_err());
        drop(lifecycle);
        let reopened = ManagedLifecycle::open(&state, processes.clone(), runners, hosts)?;
        assert_eq!(reopened.logs(3)?, expected);
        assert!(!processes.runtime_running.load(Ordering::SeqCst));
        assert_eq!(processes.active_hosts.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[cfg(unix)]
    fn owned_runner_fixture(
        state: &std::path::Path,
    ) -> Result<RunnerSupervisorV1, Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt as _;
        let executable = state.join("owned-runner.sh");
        let script = b"#!/bin/sh\ntrap 'exit 0' INT TERM\nwhile :; do /bin/sleep 1; done\n";
        std::fs::write(&executable, script)?;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;
        let declarations = prepare_data_directory(&state.join("runner-declarations"))?;
        let health = std::net::TcpListener::bind("127.0.0.1:0")?;
        std::fs::write(
            declarations.join("runner.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema":"worldstream/runner-template/v1", "template_id":"owned-runner", "revision":"1",
                "display_name":"Owned runner", "executable":{"path":executable,"blake3":blake3::hash(script).to_hex().to_string()},
                "compatibility":[{"activity_pack_id":"worldstream.counter","exact_revisions":["4.0.0"]}],
                "capacity":{"maximum_concurrent_invocations":1},
                "health":{"path":"/health", "timeout_ms":100,"stale_after_ms":1000},
                "non_secret_environment":{}, "secret_environment":[],
                "instances":[{"instance_id":"owned-local", "health_address":health.local_addr()?.to_string()}]
            }))?,
        )?;
        drop(health);
        Ok(RunnerSupervisorV1::open(
            RunnerTemplateRegistryV1::open(&state.join("runner-templates"), &declarations)?,
            &state.join("runner-runtime"),
            FileSecretVaultV1::open(&state.join("secrets"))?,
            Duration::from_secs(5),
        )?)
    }

    #[cfg(unix)]
    #[test]
    fn completed_installation_stop_blocks_new_managed_runner_starts()
    -> Result<(), Box<dyn std::error::Error>> {
        use worldstream_studio_supervisor::runner_templates::RunnerInstanceStateV1;
        let temporary = tempdir()?;
        let state = prepare_data_directory(&temporary.path().join("state"))?;
        let runners = owned_runner_fixture(&state)?;
        let processes = Processes {
            active_hosts: Arc::new(AtomicUsize::new(0)),
            runtime_running: Arc::new(AtomicBool::new(true)),
            fail_runtime_starts: Arc::new(AtomicUsize::new(0)),
            stop_gate: None,
        };
        let hosts = ManagedAgentHostOperationsV1::open_with(
            &state.join("managed-hosts"),
            FixedStartSource::new(state.clone()),
            processes.clone(),
        )?;
        let lifecycle = ManagedLifecycle::open(&state, processes, runners.clone(), hosts)?;
        assert_eq!(lifecycle.stop()?.stage, LifecycleStage::Complete);
        let attempted = runners.start("owned-local").ok_or("missing owned Runner")?;
        // Cleanup remains with the original public owner even when the assertion fails.
        let cleanup = runners
            .stop("owned-local")
            .ok_or("missing Runner cleanup")?;
        assert_eq!(cleanup.instances[0].state, RunnerInstanceStateV1::Stopped);
        assert_eq!(attempted.instances[0].state, RunnerInstanceStateV1::Stopped);
        assert!(!attempted.instances[0].managed_by_supervisor);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unresolved_retained_runner_blocks_runtime_shutdown_without_touching_its_process()
    -> Result<(), Box<dyn std::error::Error>> {
        use worldstream_studio_supervisor::runner_templates::RunnerInstanceStateV1;
        for restart in [false, true] {
            let temporary = tempdir()?;
            let state = prepare_data_directory(&temporary.path().join("state"))?;
            let original = owned_runner_fixture(&state)?;
            let started = original
                .start("owned-local")
                .ok_or("missing owned Runner")?;
            assert_eq!(started.instances[0].state, RunnerInstanceStateV1::Running);
            // A new Controller has retained intent, but not the old child handle.
            // Keep the original owner solely for test cleanup.
            let reopened = RunnerSupervisorV1::open(
                RunnerTemplateRegistryV1::open_installed(&state.join("runner-templates"))?,
                &state.join("runner-runtime"),
                FileSecretVaultV1::open(&state.join("secrets"))?,
                Duration::from_secs(5),
            )?;
            let processes = Processes {
                active_hosts: Arc::new(AtomicUsize::new(0)),
                runtime_running: Arc::new(AtomicBool::new(true)),
                fail_runtime_starts: Arc::new(AtomicUsize::new(0)),
                stop_gate: None,
            };
            let hosts = ManagedAgentHostOperationsV1::open_with(
                &state.join("managed-hosts"),
                FixedStartSource::new(state.clone()),
                processes.clone(),
            )?;
            let lifecycle = ManagedLifecycle::open(&state, processes.clone(), reopened, hosts)?;
            let outcome = if restart {
                lifecycle.restart()
            } else {
                lifecycle.stop()
            };
            let still_running =
                original.statuses().instances[0].state == RunnerInstanceStateV1::Running;
            let cleanup = original
                .stop("owned-local")
                .ok_or("missing cleanup owner")?;
            assert_eq!(cleanup.instances[0].state, RunnerInstanceStateV1::Stopped);
            assert!(matches!(outcome, Err(LifecycleError::Unavailable)));
            assert!(still_running);
            assert!(processes.runtime_running.load(Ordering::SeqCst));
            assert!(lifecycle.status()?.operation.is_none());
        }
        Ok(())
    }
}

#[cfg(unix)]
#[derive(Clone)]
struct MakeOperationDirectoryReadOnlyLauncher {
    operations: std::path::PathBuf,
}

#[cfg(unix)]
impl ManagedAgentHostProcessLauncherV1 for MakeOperationDirectoryReadOnlyLauncher {
    fn launch_bridged(
        &self,
        _plan: &ManagedAgentHostLaunchPlanV1,
        _credential: &zeroize::Zeroizing<Vec<u8>>,
    ) -> Result<Box<dyn ManagedAgentHostProcessV1>, ManagedAgentHostErrorV1> {
        use std::os::unix::fs::PermissionsExt as _;

        std::fs::set_permissions(&self.operations, std::fs::Permissions::from_mode(0o500))
            .unwrap_or_else(|error| unreachable!("make operation directory read-only: {error}"));
        Ok(Box::new(FixedProcess {
            observed_at_ms: test_now_ms(),
        }))
    }
}

#[test]
fn two_child_plan_gives_storage_only_to_the_fixed_helper_and_stdio_only_to_the_host() {
    let plan = ManagedAgentHostLaunchPlanV1::new(
        std::path::Path::new("/approved/worldstream-assignment-mcp"),
        std::path::Path::new("/owner/worldstream-state"),
        SECRET_REFERENCE,
        std::path::Path::new("/approved/reference-agent-host"),
        "openai-compatible",
        provider_address(),
        "test-model",
    )
    .unwrap_or_else(|error| unreachable!("launch plan: {error:?}"));

    assert_eq!(
        plan.helper_arguments(),
        [
            "--state-dir",
            "/owner/worldstream-state",
            "--launch-reference",
            SECRET_REFERENCE
        ]
    );
    assert_eq!(
        plan.host_arguments(),
        vec![
            "--transport".to_owned(),
            "stdio".to_owned(),
            "--provider".to_owned(),
            "openai-compatible".to_owned(),
            "--provider-address".to_owned(),
            "127.0.0.1:11434".to_owned(),
            "--model".to_owned(),
            "test-model".to_owned()
        ]
    );
    assert!(!plan.host_arguments().iter().any(|argument| {
        argument.contains("state")
            || argument.contains(SECRET_REFERENCE)
            || argument.contains("bearer")
    }));
    assert_eq!(plan.model(), "test-model");
}

#[test]
fn durable_post_setup_operation_surfaces_restart_recovery_without_private_material() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let root = directory
        .path()
        .canonicalize()
        .unwrap_or_else(|error| unreachable!("root: {error}"));
    let operations_root = root.join("operations");
    let source = FixedStartSource::new(root.clone());
    let launches = RecordingProcessLauncher::default();
    let operations =
        ManagedAgentHostOperationsV1::open_with(&operations_root, source.clone(), launches.clone())
            .unwrap_or_else(|error| unreachable!("host operations: {error:?}"));
    let running = operations
        .start(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("start host: {error:?}"));
    assert!(running.ready);
    assert_eq!(launches.0.load(Ordering::SeqCst), 1);
    drop(operations);

    let restarted =
        ManagedAgentHostOperationsV1::open_with(&operations_root, source, launches.clone())
            .unwrap_or_else(|error| unreachable!("restart host operations: {error:?}"));
    let attention = restarted
        .status(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("restart status: {error:?}"));
    assert!(!attention.ready);
    assert_eq!(
        attention.failure.as_ref().map(|value| value.code.as_str()),
        Some("host_restart_required")
    );
    let recovered = restarted
        .start(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("retry same host: {error:?}"));
    assert!(recovered.ready);
    assert_eq!(launches.0.load(Ordering::SeqCst), 2);
    let safe = serde_json::to_string(&recovered)
        .unwrap_or_else(|error| unreachable!("serialize status: {error}"));
    for prohibited in [
        SECRET_REFERENCE,
        "provider-secret",
        "launch_reference",
        "prompt",
    ] {
        assert!(!safe.contains(prohibited));
    }
}

#[test]
fn child_exit_is_retained_across_later_status_polls() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let root = directory
        .path()
        .canonicalize()
        .unwrap_or_else(|error| unreachable!("root: {error}"));
    let operations = ManagedAgentHostOperationsV1::open_with(
        &root.join("managed"),
        FixedStartSource::new(root),
        ExitingProcessLauncher,
    )
    .unwrap_or_else(|error| unreachable!("host operations: {error:?}"));
    operations
        .start(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("start host: {error:?}"));

    let first = operations
        .status(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("first status: {error:?}"));
    assert_eq!(
        first.failure.as_ref().map(|value| value.code.as_str()),
        Some("host_exited")
    );
    assert!(
        first
            .failure
            .as_ref()
            .is_some_and(|value| value.message.contains("17"))
    );

    let later = operations
        .status(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("later status: {error:?}"));
    assert_eq!(
        later.failure.as_ref().map(|value| value.code.as_str()),
        Some("host_exited")
    );
}

#[test]
fn silent_live_process_becomes_stale_without_fabricating_a_health_observation() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let root = directory
        .path()
        .canonicalize()
        .unwrap_or_else(|error| unreachable!("root: {error}"));
    let source = FixedStartSource {
        root,
        capacity: 1,
        stale_after: Duration::from_millis(100),
    };
    let operations = ManagedAgentHostOperationsV1::open_with(
        &directory.path().join("managed"),
        source,
        SilentProcessLauncher,
    )
    .unwrap_or_else(|error| unreachable!("host operations: {error:?}"));

    let running = operations
        .start(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("start host: {error:?}"));
    assert!(running.ready);
    std::thread::sleep(Duration::from_millis(150));
    let stale = operations
        .status(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("stale status: {error:?}"));

    assert_eq!(
        stale.freshness,
        worldstream_studio_supervisor::managed_agent_host::ManagedAgentHostFreshnessV1::Stale
    );
    assert!(!stale.ready);
}

#[derive(Clone, Copy)]
struct SilentProcessLauncher;

impl ManagedAgentHostProcessLauncherV1 for SilentProcessLauncher {
    fn launch_bridged(
        &self,
        _plan: &ManagedAgentHostLaunchPlanV1,
        _credential: &zeroize::Zeroizing<Vec<u8>>,
    ) -> Result<Box<dyn ManagedAgentHostProcessV1>, ManagedAgentHostErrorV1> {
        Ok(Box::new(SilentProcess))
    }
}

struct SilentProcess;

impl ManagedAgentHostProcessV1 for SilentProcess {
    fn try_wait(&mut self) -> Result<Option<i32>, ManagedAgentHostErrorV1> {
        Ok(None)
    }

    fn stop(&mut self) -> Result<(), ManagedAgentHostErrorV1> {
        Ok(())
    }
}

fn test_now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

#[test]
fn one_exact_host_capacity_is_shared_by_assignments_using_that_host() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let root = directory
        .path()
        .canonicalize()
        .unwrap_or_else(|error| unreachable!("root: {error}"));
    let operations = ManagedAgentHostOperationsV1::open_with(
        &directory.path().join("managed"),
        FixedStartSource::new(root),
        RecordingProcessLauncher::default(),
    )
    .unwrap_or_else(|error| unreachable!("host operations: {error:?}"));

    let first = operations
        .start(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("first host: {error:?}"));
    assert_eq!(first.capacity, 1);
    assert_eq!(first.active_invocations, 1);
    assert_eq!(
        operations.start(SECOND_ASSIGNMENT),
        Err(ManagedAgentHostErrorV1::AtCapacity)
    );
}

#[cfg(unix)]
#[test]
fn live_child_reconciles_starting_checkpoint_after_running_publication_failure() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let root = directory
        .path()
        .canonicalize()
        .unwrap_or_else(|error| unreachable!("root: {error}"));
    let operation_root = root.join("managed");
    let operations_directory = operation_root.join("operations");
    let operations = ManagedAgentHostOperationsV1::open_with(
        &operation_root,
        FixedStartSource::new(root),
        MakeOperationDirectoryReadOnlyLauncher {
            operations: operations_directory.clone(),
        },
    )
    .unwrap_or_else(|error| unreachable!("host operations: {error:?}"));

    assert_eq!(
        operations.start(ASSIGNMENT),
        Err(ManagedAgentHostErrorV1::Unavailable)
    );
    std::fs::set_permissions(
        &operations_directory,
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap_or_else(|error| unreachable!("restore operation directory: {error}"));

    let recovered = operations
        .status(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("reconcile live process: {error:?}"));
    assert_eq!(
        recovered.state,
        worldstream_studio_supervisor::managed_agent_host::ManagedAgentHostStateV1::Running
    );
    assert!(recovered.ready);
}

#[cfg(unix)]
#[test]
fn production_bridge_reaps_the_peer_when_either_fixed_child_exits() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let helper = directory.path().join("worldstream-assignment-mcp");
    let host = directory.path().join("reference-agent-host");
    let host_pid = directory.path().join("host.pid");
    std::fs::write(&helper, "#!/bin/sh\n/bin/sleep 1\nexit 17\n")
        .unwrap_or_else(|error| unreachable!("helper fixture: {error}"));
    std::fs::write(
        &host,
        format!(
            "#!/bin/sh\necho $$ > '{}'; exec sleep 30\n",
            host_pid.display()
        ),
    )
    .unwrap_or_else(|error| unreachable!("host fixture: {error}"));
    for path in [&helper, &host] {
        let mut permissions = std::fs::metadata(path)
            .unwrap_or_else(|error| unreachable!("fixture metadata: {error}"))
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(path, permissions)
            .unwrap_or_else(|error| unreachable!("fixture permissions: {error}"));
    }
    let plan = ManagedAgentHostLaunchPlanV1::new(
        &helper,
        directory.path(),
        SECRET_REFERENCE,
        &host,
        "openai-compatible",
        provider_address(),
        "test-model",
    )
    .unwrap_or_else(|error| unreachable!("launch plan: {error:?}"));
    let mut process = OsManagedAgentHostProcessLauncherV1
        .launch_bridged(&plan, &zeroize::Zeroizing::new(b"provider-secret".to_vec()))
        .unwrap_or_else(|error| unreachable!("launch bridge: {error:?}"));
    let deadline = Instant::now() + Duration::from_secs(10);
    let code = loop {
        if let Some(code) = process
            .try_wait()
            .unwrap_or_else(|error| unreachable!("poll bridge: {error:?}"))
        {
            break code;
        }
        assert!(Instant::now() < deadline, "helper exit was not observed");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(code, 17);
    let pid = std::fs::read_to_string(host_pid)
        .unwrap_or_else(|error| unreachable!("host peer pid was not published: {error}"));
    let status = std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap_or_else(|error| unreachable!("inspect peer process: {error}"));
    assert!(!status.success(), "host peer remained alive");
}

#[cfg(unix)]
#[test]
fn dropping_a_live_bridge_reaps_both_children() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let helper = directory.path().join("worldstream-assignment-mcp");
    let host = directory.path().join("reference-agent-host");
    let helper_pid = directory.path().join("helper.pid");
    let host_pid = directory.path().join("host.pid");
    for (path, pid) in [(&helper, &helper_pid), (&host, &host_pid)] {
        std::fs::write(
            path,
            format!("#!/bin/sh\necho $$ > '{}'; exec sleep 30\n", pid.display()),
        )
        .unwrap_or_else(|error| unreachable!("child fixture: {error}"));
        let mut permissions = std::fs::metadata(path)
            .unwrap_or_else(|error| unreachable!("fixture metadata: {error}"))
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(path, permissions)
            .unwrap_or_else(|error| unreachable!("fixture permissions: {error}"));
    }
    let plan = ManagedAgentHostLaunchPlanV1::new(
        &helper,
        directory.path(),
        SECRET_REFERENCE,
        &host,
        "openai-compatible",
        provider_address(),
        "test-model",
    )
    .unwrap_or_else(|error| unreachable!("launch plan: {error:?}"));
    let process = OsManagedAgentHostProcessLauncherV1
        .launch_bridged(&plan, &zeroize::Zeroizing::new(b"provider-secret".to_vec()))
        .unwrap_or_else(|error| unreachable!("launch bridge: {error:?}"));
    for _ in 0..100 {
        if helper_pid.exists() && host_pid.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(process);
    for pid_path in [helper_pid, host_pid] {
        if let Ok(pid) = std::fs::read_to_string(pid_path) {
            let status = std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .status()
                .unwrap_or_else(|error| unreachable!("inspect child process: {error}"));
            assert!(!status.success(), "bridged child remained alive");
        }
    }
}

#[cfg(unix)]
#[test]
#[allow(clippy::too_many_lines)]
fn separate_reference_host_process_completes_one_real_stdio_mcp_turn_via_loopback_provider() {
    use std::{
        io::{Read as _, Write as _},
        net::TcpListener,
        os::unix::fs::PermissionsExt as _,
    };

    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let helper = directory.path().join("worldstream-assignment-mcp");
    let calls = directory.path().join("calls.jsonl");
    let script = format!(
        r#"#!/usr/bin/python3
import json, sys
calls = open({calls:?}, "a", buffering=1)
completed = False
terminal_submission = False
activation_polls = 0
for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    calls.write(json.dumps({{"method":method}}) + "\n")
    if method == "notifications/initialized":
        continue
    if method == "initialize":
        result = {{"protocolVersion":"2025-06-18"}}
    else:
        params = request["params"]
        name = params["name"]
        arguments = params["arguments"]
        calls.write(json.dumps({{"name":name,"arguments":arguments}}) + "\n")
        if name == "worldstream.prepare_managed_turn":
            activation_polls += 1
            if activation_polls == 1 or completed:
                result = {{"content":[],"structuredContent":{{"code":"assignment_activation_none_available","retryable":True,"next_action":"wait_then_request_next_activation"}},"isError":True}}
                print(json.dumps({{"jsonrpc":"2.0","id":request["id"],"result":result}}), flush=True)
                continue
            if terminal_submission:
                completed = True
                value = {{"schema":"worldstream/managed-turn-preparation/v1","state":"reconciled"}}
                result = {{"content":[],"structuredContent":value,"isError":False}}
                print(json.dumps({{"jsonrpc":"2.0","id":request["id"],"result":result}}), flush=True)
                continue
            value = {{"schema":"worldstream/managed-turn-preparation/v1","instruction":"Select exactly one listed offer and return its offer_id and payload.","observation":{{"observations":[{{"frame_seq":9,"observation":{{"turn":3}}}}]}},"offers":{{"precondition":{{"room_seq":7,"head_hash":"blake3:" + "b"*64}},"offers":[{{"offer_id":"7:0:digest","action_type":"increment","payload_schema":{{"schema":{{"type":"object"}}}}}}]}}}}
        elif name == "worldstream.submit_managed_turn_action":
            if terminal_submission:
                raise RuntimeError("unexpected duplicate managed submission")
            terminal_submission = True
            result = {{"content":[],"structuredContent":{{"code":"assignment_activation_lease_expired","retryable":True,"next_action":"request_next_activation"}},"isError":True}}
            print(json.dumps({{"jsonrpc":"2.0","id":request["id"],"result":result}}), flush=True)
            continue
        else:
            raise RuntimeError("unexpected tool")
        result = {{"content":[],"structuredContent":value,"isError":False}}
    print(json.dumps({{"jsonrpc":"2.0","id":request["id"],"result":result}}), flush=True)
"#,
        calls = calls.display()
    );
    std::fs::write(&helper, script).unwrap_or_else(|error| unreachable!("helper fixture: {error}"));
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| unreachable!("helper permissions: {error}"));

    let provider = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| unreachable!("provider listener: {error}"));
    let provider_address = provider
        .local_addr()
        .unwrap_or_else(|error| unreachable!("provider address: {error}"));
    let provider_thread = std::thread::spawn(move || {
        let (mut stream, _) = provider
            .accept()
            .unwrap_or_else(|error| unreachable!("provider accept: {error}"));
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap_or_else(|error| unreachable!("provider timeout: {error}"));
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream
                .read(&mut buffer)
                .unwrap_or_else(|error| unreachable!("provider request: {error}"));
            request.extend_from_slice(&buffer[..read]);
            let complete = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .and_then(|boundary| {
                    let headers = std::str::from_utf8(&request[..boundary]).ok()?;
                    let length = headers.lines().find_map(|line| {
                        line.strip_prefix("Content-Length: ")?.parse::<usize>().ok()
                    })?;
                    Some(request.len() >= boundary + 4 + length)
                })
                .unwrap_or(false);
            if read == 0 || complete {
                break;
            }
        }
        let request_text = String::from_utf8_lossy(&request);
        assert!(request_text.contains("Authorization: Bearer provider-secret"));
        assert!(request_text.contains("7:0:digest"));
        let body = r#"{"choices":[{"message":{"content":"{\"offer_id\":\"7:0:digest\",\"payload\":{\"amount\":1}}"}}]}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap_or_else(|error| unreachable!("provider response: {error}"));
    });

    let host = std::path::PathBuf::from(
        std::env::var("CARGO_BIN_EXE_worldstream-managed-agent-host")
            .unwrap_or_else(|error| unreachable!("managed host binary: {error}")),
    );
    let plan = ManagedAgentHostLaunchPlanV1::new_managed_reference(
        &helper,
        directory.path(),
        SECRET_REFERENCE,
        &host,
        ManagedReferenceProviderV1::OpenAiCompatible,
        provider_address,
        "test-model",
    )
    .unwrap_or_else(|error| unreachable!("launch plan: {error:?}"));
    let mut process = OsManagedAgentHostProcessLauncherV1
        .launch_bridged(&plan, &zeroize::Zeroizing::new(b"provider-secret".to_vec()))
        .unwrap_or_else(|error| unreachable!("launch host: {error:?}"));
    let deadline = Instant::now() + Duration::from_mins(1);
    loop {
        let managed_turn_calls = std::fs::read_to_string(&calls).map_or(0, |calls| {
            calls
                .lines()
                .filter(|line| line.contains("worldstream.prepare_managed_turn"))
                .count()
        });
        if managed_turn_calls >= 4 {
            break;
        }
        assert_eq!(
            process
                .try_wait()
                .unwrap_or_else(|error| unreachable!("poll host: {error:?}")),
            None
        );
        assert!(
            Instant::now() < deadline,
            "host did not recover the terminal post-model lease; calls={}",
            std::fs::read_to_string(&calls).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    process
        .stop()
        .unwrap_or_else(|error| unreachable!("stop host: {error:?}"));
    provider_thread
        .join()
        .unwrap_or_else(|_| unreachable!("provider thread panicked"));

    let retained_calls =
        std::fs::read_to_string(calls).unwrap_or_else(|error| unreachable!("read calls: {error}"));
    for tool in [
        "worldstream.prepare_managed_turn",
        "worldstream.submit_managed_turn_action",
    ] {
        assert!(retained_calls.contains(tool), "missing tool {tool}");
    }
    for prohibited in [SECRET_REFERENCE, "provider-secret", "state-dir", "bearer"] {
        assert!(!retained_calls.contains(prohibited));
    }
    let calls = retained_calls
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|value| {
            value
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .zip(value.get("arguments").cloned())
        })
        .collect::<Vec<_>>();
    assert!(calls.iter().all(|(name, _)| {
        !matches!(
            name.as_str(),
            "worldstream.next_activation"
                | "worldstream.observe"
                | "worldstream.acknowledge"
                | "worldstream.list_current_action_offers"
                | "worldstream.submit_action"
                | "worldstream.complete_activation"
        )
    }));
    let completion = &calls
        .iter()
        .find(|(name, _)| name == "worldstream.submit_managed_turn_action")
        .unwrap_or_else(|| unreachable!("managed turn submission"))
        .1;
    assert_eq!(
        completion
            .get("offer_id")
            .and_then(serde_json::Value::as_str),
        Some("7:0:digest")
    );
    assert_eq!(
        calls
            .iter()
            .filter(|(name, _)| name == "worldstream.submit_managed_turn_action")
            .count(),
        1,
        "terminal recovery must reuse the retained Action without another model-selected submission"
    );
}
