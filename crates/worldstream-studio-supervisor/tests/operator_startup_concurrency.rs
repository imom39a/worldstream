//! A live launcher retains its pending generation while its child is delayed.
#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::{PermissionsExt as _, symlink},
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};
use worldstream_runtime::{CliOverrides, ConfigLoader};
use worldstream_studio_supervisor::{
    local_initialization::{InitializationRequest, initialize_local},
    operator_connection::{ControllerExecutables, OperatorConnection},
    process_ownership::{ProcessOwnership, ProcessRole},
};

struct ReleaseOnDrop(std::path::PathBuf);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        let _ = fs::write(&self.0, b"");
    }
}

#[test]
fn a_concurrent_start_does_not_fence_another_live_launchers_delayed_child()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let directory = temporary.path();
    let state = directory.join(".worldstream/studio");
    let config = directory.join(".worldstream/worldstream.toml");
    let runtime = std::net::TcpListener::bind("127.0.0.1:0")?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let endpoint = listener.local_addr()?;
    initialize_local(&InitializationRequest {
        config: Some(config.clone()),
        overrides: CliOverrides {
            bind: Some(runtime.local_addr()?),
            ..CliOverrides::default()
        },
        state_dir: state.clone(),
        working_directory: directory.to_owned(),
        environment: Vec::new(),
        preview: false,
    })?;
    drop(runtime);
    drop(listener);
    // The substitution is an OS launch delay only; the eventual process is the
    // actual Controller, using its normal claim, listener, proof and shutdown.
    let wrapper = directory.join("delayed-controller.sh");
    symlink(
        env!("CARGO_BIN_EXE_worldstream-studio-supervisor"),
        directory.join("real-controller"),
    )?;
    fs::write(&wrapper, b"#!/bin/sh\nfixture_dir=\"${0%/*}\"\n: > \"$fixture_dir/child-entered\"\nwhile [ ! -f \"$fixture_dir/release-child\" ]; do /bin/sleep 0.02; done\nexec \"$fixture_dir/real-controller\" \"$@\" 2>> \"$fixture_dir/controller-errors\"\n")?;
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700))?;
    let executables = ControllerExecutables {
        controller: wrapper,
        // This test never requests Runtime start. It still uses a real fixed
        // executable path for the configured but inactive Runtime.
        runtime: env!("CARGO_BIN_EXE_worldstream-studio-supervisor").into(),
        assignment_mcp: env!("CARGO_BIN_EXE_worldstream-assignment-mcp").into(),
    };
    let connection = OperatorConnection::open(&state, endpoint, Duration::from_secs(180))?;
    let loader = ConfigLoader::with_environment(Some(config.clone()), CliOverrides::default(), []);
    let ownership = ProcessOwnership::open(&state)?;
    let second_entered = Arc::new(Barrier::new(2));
    let result = thread::scope(|scope| {
        let release = ReleaseOnDrop(directory.join("release-child"));
        let first =
            scope.spawn(|| connection.ensure_started(&loader, &config, directory, &executables));
        let deadline = Instant::now() + Duration::from_secs(180);
        while !directory.join("child-entered").exists() {
            if Instant::now() >= deadline {
                return Err("owned delayed child did not start");
            }
            thread::sleep(Duration::from_millis(25));
        }
        let before = ownership
            .snapshot(ProcessRole::Controller)
            .map_err(|_| "initial ownership unavailable")?
            .ok_or("missing initial generation")?
            .generation;
        let second = scope.spawn(|| {
            second_entered.wait();
            connection.ensure_started(&loader, &config, directory, &executables)
        });
        second_entered.wait();
        let deadline = Instant::now() + Duration::from_secs(2);
        let after = loop {
            let observed = ownership
                .snapshot(ProcessRole::Controller)
                .map_err(|_| "concurrent ownership unavailable")?
                .ok_or("missing pending generation")?
                .generation;
            if observed != before || Instant::now() >= deadline {
                break observed;
            }
            thread::sleep(Duration::from_millis(10));
        };
        drop(release);
        let first = first.join().map_err(|_| "first launcher panicked")?;
        let second = second.join().map_err(|_| "second launcher panicked")?;
        Ok((before, after, first, second))
    });
    let stopped = connection.stop_controller();
    let released = ownership.is_leased(ProcessRole::Controller) == Ok(false);
    if !released || stopped.is_err() {
        eprintln!(
            "Owned delayed-controller fixture retained: {}",
            temporary.keep().display()
        );
    }
    let (before, after, first, second) = result?;
    assert_eq!(
        before, after,
        "another live launcher replaced the pending generation"
    );
    assert!(first.is_ok() && second.is_ok());
    assert!(
        released && stopped.is_ok(),
        "owned Controller cleanup must complete"
    );
    assert!(ownership.snapshot(ProcessRole::Runtime)?.is_none());
    Ok(())
}
