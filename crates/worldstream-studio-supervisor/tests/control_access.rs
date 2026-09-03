#![allow(
    clippy::manual_assert_eq,
    reason = "Failed secret-byte checks must not print either credential operand."
)]

use axum::http::{HeaderMap, header::AUTHORIZATION};
use std::{fs, io::Write as _};
use worldstream_studio_supervisor::control_access::{ControlAccess, ControlAccessError};
use worldstream_studio_supervisor::secrets::{FileSecretVaultV1, SecretKindV1};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn explicit_initialization_retains_one_protected_control_credential() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let state = temporary.path().join("state");
    let access = ControlAccess::initialize(&state)?;
    let original = access.authorization_header()?;
    assert!(original.is_sensitive());
    assert!(!format!("{access:?}").contains(original.to_str()?));
    let repeated = ControlAccess::initialize(&state)?;
    assert_eq!(repeated.authorization_header()?, original);
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, original);
    assert!(ControlAccess::open(&state)?.authenticate(&headers)?);
    assert!(!access.authenticate(&HeaderMap::new())?);
    Ok(())
}

#[test]
fn rotation_invalidates_prior_control_without_changing_other_authority() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let state = temporary.path().join("state");
    let access = ControlAccess::initialize(&state)?;
    let original = access.authorization_header()?;
    let observer = ControlAccess::open(&state)?;
    let other = state.join("runtime-authority.secret");
    let material = b"separate-authority-canary-remains";
    let mut file = worldstream_runtime::create_owner_only_file(&other)?;
    file.write_all(material)?;
    file.sync_all()?;
    drop(file);
    let vault = FileSecretVaultV1::open(&state.join("secrets"))?;
    let mut retained = Vec::new();
    for (kind, marker) in [
        (SecretKindV1::HostAuthority, 41),
        (SecretKindV1::MembershipAuthority, 42),
        (SecretKindV1::RunnerAuthority, 43),
        (SecretKindV1::ModelProvider, 44),
    ] {
        let bytes = [marker; 32];
        retained.push((kind, vault.store(kind, &bytes)?, bytes));
    }

    access.rotate()?;
    let current = observer.authorization_header()?;
    assert_ne!(current, original);
    assert!(current.is_sensitive());
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, original);
    assert!(!observer.authenticate(&headers)?);
    headers.insert(AUTHORIZATION, current);
    assert!(access.authenticate(&headers)?);
    assert!(fs::read(other)? == material);
    for (kind, reference, bytes) in retained {
        assert!(vault.resolve(kind, &reference)?.as_bytes() == bytes);
    }
    Ok(())
}

#[test]
fn retained_access_fails_closed_without_repairing_or_creating_state() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let state = temporary.path().join("state");
    assert_eq!(
        ControlAccess::open(&state).err(),
        Some(ControlAccessError::NotInitialized)
    );
    assert_eq!(fs::read_dir(temporary.path())?.count(), 0);

    worldstream_runtime::prepare_data_directory(&state)?;
    assert_eq!(
        ControlAccess::open(&state).err(),
        Some(ControlAccessError::NotInitialized)
    );
    assert_eq!(fs::read_dir(&state)?.count(), 0);

    let access = ControlAccess::initialize(&state)?;
    let record = state.join("control-access.v1");
    let retained_files = fs::read_dir(&state)?.count();
    let corrupt = b"damaged-control-record-canary";
    fs::write(&record, corrupt)?;
    assert_eq!(
        ControlAccess::open(&state).err(),
        Some(ControlAccessError::Invalid)
    );
    assert_eq!(
        ControlAccess::initialize(&state).err(),
        Some(ControlAccessError::Invalid)
    );
    assert_eq!(access.rotate(), Err(ControlAccessError::Invalid));
    assert!(fs::read(&record)? == corrupt);
    assert_eq!(fs::read_dir(&state)?.count(), retained_files);
    Ok(())
}

#[test]
fn concurrent_initialization_and_rotation_retain_one_valid_record() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let state = temporary.path().join("state");
    let barrier = std::sync::Barrier::new(8);
    let headers = std::thread::scope(|scope| -> Result<Vec<_>, Box<dyn std::error::Error>> {
        let workers = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    ControlAccess::initialize(&state)?.authorization_header()
                })
            })
            .collect::<Vec<_>>();
        let mut headers = Vec::new();
        for worker in workers {
            match worker
                .join()
                .map_err(|_| "initialization worker panicked")?
            {
                Ok(header) => headers.push(header),
                Err(ControlAccessError::Unavailable) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(headers)
    })?;
    let first = headers.first().ok_or("no initialization result")?;
    assert!(headers.iter().all(|header| header == first));
    let outcomes = std::thread::scope(|scope| -> Result<Vec<_>, Box<dyn std::error::Error>> {
        let workers = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    ControlAccess::open(&state)?.rotate()
                })
            })
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .map(|worker| worker.join().map_err(|_| "rotation worker panicked".into()))
            .collect()
    })?;
    assert!(outcomes.iter().any(Result::is_ok));
    assert!(
        outcomes
            .iter()
            .all(|outcome| { matches!(outcome, Ok(()) | Err(ControlAccessError::Unavailable)) })
    );
    let access = ControlAccess::open(&state)?;
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, first.clone());
    assert!(!access.authenticate(&headers)?);
    headers.insert(AUTHORIZATION, access.authorization_header()?);
    assert!(access.authenticate(&headers)?);
    Ok(())
}

#[test]
fn killed_rotation_process_leaves_usable_control_and_releases_its_lock() -> TestResult {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    let temporary = tempfile::tempdir()?;
    let state = temporary.path().join("state");
    let access = ControlAccess::initialize(&state)?;
    let original = access.authorization_header()?;
    let mut worker = RotationWorker(
        Command::new(std::env::current_exe()?)
            .args(["--exact", "rotation_crash_worker", "--ignored"])
            .env("WORLDSTREAM_TEST_CONTROL_STATE", &state)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while access.authorization_header()? == original {
        if worker.0.try_wait()?.is_some() || Instant::now() >= deadline {
            return Err("rotation worker did not make progress before the deadline".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // Kill only the child owned by this fixture, during its rotation loop.
    worker.0.kill()?;
    worker.0.wait()?;
    let recovered = ControlAccess::open(&state)?;
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, original);
    assert!(!recovered.authenticate(&headers)?);
    headers.insert(AUTHORIZATION, recovered.authorization_header()?);
    assert!(recovered.authenticate(&headers)?);
    // A process death must not leave a permanent lock or require a credential reset.
    recovered.rotate()?;
    assert!(!recovered.authenticate(&headers)?);
    Ok(())
}

#[test]
fn initialization_refuses_inflight_control_publication() -> TestResult {
    let temporary = tempfile::tempdir()?;
    let state = temporary.path().join("state");
    let access = ControlAccess::initialize(&state)?;
    access.rotate()?;
    let previous = access.authorization_header()?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(state.join("control-access.lock"))?;
    lock.try_lock()?;

    assert_eq!(
        ControlAccess::initialize(&state).err(),
        Some(ControlAccessError::Unavailable)
    );
    assert_eq!(access.authorization_header()?, previous);
    drop(lock);
    assert_eq!(
        ControlAccess::initialize(&state)?.authorization_header()?,
        previous
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn unsafe_or_redirected_access_is_rejected_without_permission_repair() -> TestResult {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    let temporary = tempfile::tempdir()?;
    let state = temporary.path().join("state");
    let access = ControlAccess::initialize(&state)?;
    let record = state.join("control-access.v1");
    let original = fs::read(&record)?;
    fs::set_permissions(&record, fs::Permissions::from_mode(0o644))?;
    assert_eq!(
        ControlAccess::open(&state).err(),
        Some(ControlAccessError::Invalid)
    );
    assert_eq!(
        ControlAccess::initialize(&state).err(),
        Some(ControlAccessError::Invalid)
    );
    assert_eq!(access.rotate(), Err(ControlAccessError::Invalid));
    assert_eq!(fs::metadata(&record)?.permissions().mode() & 0o777, 0o644);
    assert!(fs::read(&record)? == original);

    // Restore only this fixture's permissions, then replace its path with an alias.
    fs::set_permissions(&record, fs::Permissions::from_mode(0o600))?;
    let retained = state.join("retained-control");
    fs::rename(&record, &retained)?;
    symlink(&retained, &record)?;
    assert_eq!(
        ControlAccess::open(&state).err(),
        Some(ControlAccessError::Invalid)
    );
    assert_eq!(
        ControlAccess::initialize(&state).err(),
        Some(ControlAccessError::Invalid)
    );
    assert_eq!(access.rotate(), Err(ControlAccessError::Invalid));
    assert!(fs::symlink_metadata(&record)?.file_type().is_symlink());
    assert!(fs::read(&retained)? == original);

    let state_alias = temporary.path().join("state-alias");
    symlink(&state, &state_alias)?;
    assert_eq!(
        ControlAccess::open(&state_alias).err(),
        Some(ControlAccessError::Invalid)
    );
    assert!(fs::symlink_metadata(&state_alias)?.file_type().is_symlink());
    Ok(())
}

struct RotationWorker(std::process::Child);

impl Drop for RotationWorker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "owned subprocess helper for the control rotation crash test"]
fn rotation_crash_worker() -> TestResult {
    let state = std::env::var_os("WORLDSTREAM_TEST_CONTROL_STATE")
        .ok_or("missing disposable rotation state")?;
    let access = ControlAccess::open(std::path::Path::new(&state))?;
    for _ in 0..100_000 {
        access.rotate()?;
    }
    Ok(())
}
