use std::{net::TcpListener, time::Duration};

use worldstream_runtime::CliOverrides;
use worldstream_studio_supervisor::{
    local_initialization::{InitializationRequest, initialize_local},
    managed_lifecycle::{RuntimeControl, RuntimeObservation},
    process_ownership::{ProcessOwnership, ProcessRole},
    process_runtime::{ManagedRuntimeSpec, ProcessRuntimeControl},
};

// Cleanup uses the same authenticated public interface, never a retained PID.
// On unexpected unresponsive ownership, retain its authority for safe recovery.
struct OwnedInstallation {
    temporary: Option<tempfile::TempDir>,
    spec: ManagedRuntimeSpec,
}

impl Drop for OwnedInstallation {
    fn drop(&mut self) {
        if let Ok(control) = ProcessRuntimeControl::open(self.spec.clone()) {
            let _ = control.stop();
        }
        let released = ProcessOwnership::open(&self.spec.state).is_ok_and(|ownership| {
            if ownership.is_leased(ProcessRole::Runtime) != Ok(false) {
                return false;
            }
            if let Ok(Some(snapshot)) = ownership.snapshot(ProcessRole::Runtime) {
                let _ = ownership.cancel_abandoned(ProcessRole::Runtime, &snapshot.generation);
            }
            ownership.is_leased(ProcessRole::Runtime) == Ok(false)
        });
        if !released && let Some(temporary) = self.temporary.take() {
            eprintln!(
                "Owned Runtime fixture retained for authenticated cleanup: {}",
                temporary.keep().display()
            );
        }
    }
}

#[test]
fn runtime_survives_adapter_reopen_and_remains_controllable_after_config_deletion()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let reserved_port = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = reserved_port.local_addr()?;
    let installation = initialize_local(&InitializationRequest {
        config: None,
        overrides: CliOverrides {
            bind: Some(endpoint),
            ..CliOverrides::default()
        },
        state_dir: temporary.path().join(".worldstream/studio"),
        working_directory: temporary.path().to_path_buf(),
        environment: Vec::new(),
        preview: false,
    })?;
    let spec = ManagedRuntimeSpec {
        executable: env!("CARGO_BIN_EXE_worldstreamd").into(),
        config: installation.config_path.clone(),
        state: installation.state_dir.clone(),
        working_directory: temporary.path().to_path_buf(),
        endpoint,
        timeout: Duration::from_secs(180),
    };
    let _cleanup = OwnedInstallation {
        temporary: Some(temporary),
        spec: spec.clone(),
    };
    drop(reserved_port);
    let control = ProcessRuntimeControl::open(spec.clone())?;
    assert_eq!(control.observe(), RuntimeObservation::Stopped);
    control.start()?;
    assert_eq!(control.observe(), RuntimeObservation::Ready);
    drop(control);
    std::fs::remove_file(&installation.config_path)?;
    let recovered = ProcessRuntimeControl::open(spec)?;
    assert_eq!(recovered.observe(), RuntimeObservation::Ready);
    recovered.stop()?;
    assert_eq!(recovered.observe(), RuntimeObservation::Stopped);
    assert!(!ProcessOwnership::open(&installation.state_dir)?.is_leased(ProcessRole::Runtime)?);
    Ok(())
}

#[test]
fn explicit_start_fences_a_free_abandoned_launch_before_replacement()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = initialized_fixture()?;
    let ownership = ProcessOwnership::open(&fixture.spec.state)?;
    // Simulate Controller loss after durable reservation, before facts/spawn.
    let abandoned = ownership.reserve(ProcessRole::Runtime)?;
    let control = ProcessRuntimeControl::open(fixture.spec.clone())?;
    assert_eq!(control.observe(), RuntimeObservation::Unmanaged);
    control.start()?;
    assert_eq!(control.observe(), RuntimeObservation::Ready);
    let current = ownership
        .snapshot(ProcessRole::Runtime)?
        .ok_or("missing replacement")?;
    assert_ne!(current.generation, abandoned.generation());
    control.stop()?;
    assert!(
        ownership
            .claim(ProcessRole::Runtime, abandoned.generation())
            .is_err()
    );
    assert!(!ownership.is_leased(ProcessRole::Runtime)?);
    Ok(())
}

#[cfg(unix)]
#[test]
fn missing_permanent_lock_rejects_stop_without_shutting_down_the_runtime()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = initialized_fixture()?;
    let control = ProcessRuntimeControl::open(fixture.spec.clone())?;
    control.start()?;
    assert_eq!(control.observe(), RuntimeObservation::Ready);
    let lock = fixture.spec.state.join("managed-runtime.lock");
    let aside = fixture.spec.state.join("owned-test-runtime-lock.aside");
    // Unix permits moving this fixture's held lock. Windows deliberately denies
    // delete sharing. Preserve the same inode so cleanup retains real ownership.
    std::fs::rename(&lock, &aside)?;
    let result = control.stop();
    std::fs::rename(&aside, &lock)?;

    // Give any incorrectly admitted asynchronous shutdown time to become
    // observable; successful rejection must preserve readiness throughout.
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    let mut stayed_ready = true;
    while std::time::Instant::now() < deadline {
        if control.observe() != RuntimeObservation::Ready {
            stayed_ready = false;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(result.is_err(), "incomplete ownership authorized stop");
    assert!(stayed_ready, "rejected stop still shut down the Runtime");
    control.stop()?;
    assert!(!ProcessOwnership::open(&fixture.spec.state)?.is_leased(ProcessRole::Runtime)?);
    Ok(())
}

fn initialized_fixture() -> Result<OwnedInstallation, Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let port = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = port.local_addr()?;
    let installation = initialize_local(&InitializationRequest {
        config: None,
        overrides: CliOverrides {
            bind: Some(endpoint),
            ..CliOverrides::default()
        },
        state_dir: temporary.path().join(".worldstream/studio"),
        working_directory: temporary.path().to_path_buf(),
        environment: Vec::new(),
        preview: false,
    })?;
    let spec = ManagedRuntimeSpec {
        executable: env!("CARGO_BIN_EXE_worldstreamd").into(),
        config: installation.config_path,
        state: installation.state_dir,
        working_directory: temporary.path().to_path_buf(),
        endpoint,
        timeout: Duration::from_secs(180),
    };
    drop(port);
    Ok(OwnedInstallation {
        temporary: Some(temporary),
        spec,
    })
}
