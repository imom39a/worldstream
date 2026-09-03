use worldstream_runtime::prepare_data_directory;
use worldstream_studio_supervisor::process_ownership::{
    ProcessOwnership, ProcessPhase, ProcessRole, ProcessTermination,
};

#[test]
fn one_runtime_generation_holds_the_installation_lifetime_lease()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let state = prepare_data_directory(&temporary.path().join("state"))?;
    let ownership = ProcessOwnership::open(&state)?;
    let launch = ownership.reserve(ProcessRole::Runtime)?;
    let lease = ownership.claim(ProcessRole::Runtime, launch.generation())?;
    assert!(
        ownership
            .claim(ProcessRole::Runtime, launch.generation())
            .is_err()
    );
    assert!(ownership.reserve(ProcessRole::Runtime).is_err());
    drop(lease);
    Ok(())
}

#[test]
fn cancelling_an_abandoned_launch_fences_its_delayed_child()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let state = prepare_data_directory(&temporary.path().join("state"))?;
    let ownership = ProcessOwnership::open(&state)?;
    let old = ownership.reserve(ProcessRole::Runtime)?;
    ownership.cancel_abandoned(ProcessRole::Runtime, old.generation())?;
    let replacement = ownership.reserve(ProcessRole::Runtime)?;
    assert_ne!(old.generation(), replacement.generation());
    assert!(
        ownership
            .claim(ProcessRole::Runtime, old.generation())
            .is_err()
    );
    let _lease = ownership.claim(ProcessRole::Runtime, replacement.generation())?;
    Ok(())
}

#[test]
fn finishing_runtime_preserves_controller_ownership_and_inspectable_state()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let state = prepare_data_directory(&temporary.path().join("state"))?;
    let ownership = ProcessOwnership::open(&state)?;
    assert!(ownership.snapshot(ProcessRole::Runtime)?.is_none());
    let controller = ownership.reserve(ProcessRole::Controller)?;
    let _controller_lease = ownership.claim(ProcessRole::Controller, controller.generation())?;
    let runtime = ownership.reserve(ProcessRole::Runtime)?;
    let starting = ownership
        .snapshot(ProcessRole::Runtime)?
        .ok_or("missing launch")?;
    assert_eq!(starting.phase, ProcessPhase::Starting);
    assert_eq!(starting.pid, None);
    assert_eq!(starting.generation, runtime.generation());
    let runtime_lease = ownership.claim(ProcessRole::Runtime, runtime.generation())?;
    assert!(
        ownership
            .cancel_abandoned(ProcessRole::Runtime, runtime.generation())
            .is_err()
    );
    runtime_lease.finish(ProcessTermination::Stopped)?;
    let stopped = ownership
        .snapshot(ProcessRole::Runtime)?
        .ok_or("missing stop")?;
    assert_eq!(stopped.phase, ProcessPhase::Stopped);
    assert_eq!(stopped.pid, Some(std::process::id()));
    assert!(ownership.reserve(ProcessRole::Runtime).is_ok());
    assert!(ownership.reserve(ProcessRole::Controller).is_err());
    Ok(())
}

#[test]
fn lease_inspection_is_read_only_and_distinguishes_held_from_released()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let state = prepare_data_directory(&temporary.path().join("state"))?;
    let ownership = ProcessOwnership::open(&state)?;
    assert!(!ownership.is_leased(ProcessRole::Runtime)?);
    assert_eq!(std::fs::read_dir(&state)?.count(), 0);
    let launch = ownership.reserve(ProcessRole::Runtime)?;
    assert!(!ownership.is_leased(ProcessRole::Runtime)?);
    let lease = ownership.claim(ProcessRole::Runtime, launch.generation())?;
    assert!(ownership.is_leased(ProcessRole::Runtime)?);
    drop(lease);
    assert!(!ownership.is_leased(ProcessRole::Runtime)?);
    assert_eq!(
        ownership
            .snapshot(ProcessRole::Runtime)?
            .ok_or("missing record")?
            .phase,
        ProcessPhase::Starting
    );
    assert!(ownership.reserve(ProcessRole::Runtime).is_err());
    Ok(())
}

#[test]
fn missing_record_does_not_turn_retained_ownership_into_a_fresh_installation()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let state = prepare_data_directory(&temporary.path().join("state"))?;
    let ownership = ProcessOwnership::open(&state)?;
    let _launch = ownership.reserve(ProcessRole::Runtime)?;
    // Simulate loss of a published record, leaving the permanent lifetime lock.
    std::fs::remove_file(state.join("managed-runtime.v1"))?;
    assert!(
        ownership.reserve(ProcessRole::Runtime).is_err(),
        "missing retained ownership was silently replaced"
    );
    assert!(ownership.is_leased(ProcessRole::Runtime).is_err());
    Ok(())
}

#[cfg(unix)]
#[test]
fn unsafe_locks_and_damaged_records_are_not_repaired_by_control_operations()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt as _;
    for damage in ["lock", "record"] {
        let temporary = tempfile::tempdir()?;
        let state = prepare_data_directory(&temporary.path().join("state"))?;
        let ownership = ProcessOwnership::open(&state)?;
        let launch = ownership.reserve(ProcessRole::Runtime)?;
        let lock = state.join("managed-runtime.lock");
        let record = state.join("managed-runtime.v1");
        if damage == "lock" {
            std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o644))?;
        } else {
            std::fs::write(&record, b"damaged")?;
        }
        assert!(ownership.reserve(ProcessRole::Runtime).is_err());
        assert!(ownership.is_leased(ProcessRole::Runtime).is_err());
        assert!(
            ownership
                .cancel_abandoned(ProcessRole::Runtime, launch.generation())
                .is_err()
        );
        if damage == "lock" {
            assert_eq!(std::fs::metadata(lock)?.permissions().mode() & 0o777, 0o644);
        } else {
            assert!(std::fs::read(record)? == b"damaged");
        }
    }
    Ok(())
}

#[test]
fn unreserved_claim_and_cancel_do_not_create_orphan_ownership_files()
-> Result<(), Box<dyn std::error::Error>> {
    for operation in ["claim", "cancel"] {
        let temporary = tempfile::tempdir()?;
        let state = prepare_data_directory(&temporary.path().join("state"))?;
        let ownership = ProcessOwnership::open(&state)?;
        if operation == "claim" {
            assert!(
                ownership
                    .claim(ProcessRole::Runtime, "unreserved-generation")
                    .is_err()
            );
        } else {
            assert!(
                ownership
                    .cancel_abandoned(ProcessRole::Runtime, "unreserved-generation")
                    .is_err()
            );
        }
        assert_eq!(
            std::fs::read_dir(&state)?.count(),
            0,
            "unreserved operation created ownership artifacts"
        );
    }
    Ok(())
}

#[test]
fn simultaneous_fresh_reservations_have_one_winner_without_orphaning_the_lock()
-> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..32 {
        let temporary = tempfile::tempdir()?;
        let state = prepare_data_directory(&temporary.path().join("state"))?;
        let ownership = ProcessOwnership::open(&state)?;
        let barrier = std::sync::Barrier::new(16);
        let winners = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..16 {
                let ownership = &ownership;
                let barrier = &barrier;
                handles.push(scope.spawn(move || {
                    barrier.wait();
                    ownership.reserve(ProcessRole::Runtime).is_ok()
                }));
            }
            handles.into_iter().try_fold(0_usize, |count, handle| {
                handle
                    .join()
                    .map(|won| count + usize::from(won))
                    .map_err(|_| "reservation worker panicked")
            })
        })?;
        assert_eq!(
            winners, 1,
            "concurrent reservations left no unique usable launch"
        );
    }
    Ok(())
}

#[test]
fn missing_permanent_lock_is_not_recreated_for_a_retained_generation()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let state = prepare_data_directory(&temporary.path().join("state"))?;
    let ownership = ProcessOwnership::open(&state)?;
    let launch = ownership.reserve(ProcessRole::Runtime)?;
    ownership
        .claim(ProcessRole::Runtime, launch.generation())?
        .finish(ProcessTermination::Stopped)?;
    let lock = state.join("managed-runtime.lock");
    std::fs::remove_file(&lock)?;
    assert!(
        ownership.reserve(ProcessRole::Runtime).is_err(),
        "missing permanent lock was silently replaced"
    );
    assert!(!lock.exists());
    assert!(ownership.is_leased(ProcessRole::Runtime).is_err());
    Ok(())
}
