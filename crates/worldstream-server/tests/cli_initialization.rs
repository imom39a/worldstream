//! Protected initialization through the shipped CLI process boundary.

use std::{
    env, fs,
    path::Path,
    process::{Command, Output, Stdio},
};

fn control(directory: &Path, arguments: &[&str]) -> std::io::Result<Output> {
    control_with_environment(directory, arguments, &[])
}

fn control_with_environment(
    directory: &Path,
    arguments: &[&str],
    environment: &[(&str, &str)],
) -> std::io::Result<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_worldstreamctl"));
    command
        .current_dir(directory)
        .args(arguments)
        .stdin(Stdio::null());
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("WORLDSTREAM") {
            command.env_remove(key);
        }
    }
    command.envs(environment.iter().copied()).output()
}

#[test]
fn initialization_preview_is_nonmutating_and_honors_the_delivery_boundary()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let output = control(directory.path(), &["init", "--preview", "--json"])?;
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(fs::read_dir(directory.path())?.count(), 0);
    if cfg!(feature = "cli-operator-preview") {
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(report["status"], "complete");
        assert_eq!(report["initialization"]["mode"], "preview");
        assert!(
            report["initialization"]["config_path"]
                .as_str()
                .is_some_and(|path| Path::new(path)
                    .ends_with(Path::new(".worldstream").join("worldstream.toml")))
        );
    } else {
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(report["code"], "not_implemented");
    }
    Ok(())
}

#[cfg(feature = "cli-operator-preview")]
#[test]
fn fresh_initialization_returns_a_reusable_config_and_repeat_preserves_it()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let output = control(directory.path(), &["init", "--json"])?;
    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let initialization = &report["initialization"];
    assert_eq!(initialization["mode"], "initialized");
    assert_eq!(initialization["config_created"], true);
    assert_eq!(initialization["control_created"], true);
    assert_eq!(initialization["services_started"], false);
    let config_path = initialization["config_path"]
        .as_str()
        .ok_or("missing config path")?;
    assert_eq!(
        initialization["next_config_args"],
        serde_json::json!(["--config", config_path])
    );
    let original = fs::read(config_path)?;
    let validation = control(
        directory.path(),
        &["--config", config_path, "config", "validate"],
    )?;
    assert_eq!(validation.status.code(), Some(0));
    let repeat = control(
        directory.path(),
        &["--config", config_path, "init", "--json"],
    )?;
    assert_eq!(repeat.status.code(), Some(0));
    let repeated: serde_json::Value = serde_json::from_slice(&repeat.stdout)?;
    assert_eq!(repeated["initialization"]["config_created"], false);
    assert_eq!(repeated["initialization"]["control_created"], false);
    assert!(
        original == fs::read(config_path)?,
        "repeat must preserve configuration bytes"
    );
    Ok(())
}

#[cfg(feature = "cli-operator-preview")]
#[test]
fn explicit_control_rotation_uses_only_local_access_and_never_prints_credentials()
-> Result<(), Box<dyn std::error::Error>> {
    use worldstream_studio_supervisor::control_access::ControlAccess;

    let directory = tempfile::tempdir()?;
    let state = directory.path().join("controller");
    let access = ControlAccess::initialize(&state)?;
    let before = access.authorization_header()?;
    let output = control(
        directory.path(),
        &[
            "--config",
            "does-not-exist.toml",
            "server",
            "rotate-control-credential",
            "--state-dir",
            state.to_str().ok_or("state path")?,
            "--json",
        ],
    )?;
    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["status"], "complete");
    let after = access.authorization_header()?;
    assert!(
        before.as_bytes() != after.as_bytes(),
        "rotation must replace only control access"
    );
    for header in [&before, &after] {
        let token = header
            .to_str()?
            .strip_prefix("Bearer ")
            .ok_or("control header")?;
        assert!(!String::from_utf8_lossy(&output.stdout).contains(token));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(token));
    }
    Ok(())
}

#[cfg(feature = "cli-operator-preview")]
#[test]
fn preview_config_selection_preserves_explicit_over_environment_precedence()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    for (arguments, filename) in [
        (vec!["init", "--preview", "--json"], "environment.toml"),
        (
            vec!["--config", "explicit.toml", "init", "--preview", "--json"],
            "explicit.toml",
        ),
    ] {
        let output = control_with_environment(
            directory.path(),
            &arguments,
            &[("WORLDSTREAM_CONFIG", "environment.toml")],
        )?;
        assert_eq!(output.status.code(), Some(0));
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        assert!(
            report["initialization"]["config_path"]
                .as_str()
                .is_some_and(|path| Path::new(path)
                    .file_name()
                    .is_some_and(|name| name == filename))
        );
        assert_eq!(fs::read_dir(directory.path())?.count(), 0);
    }
    Ok(())
}

#[cfg(feature = "cli-operator-preview")]
#[test]
fn rejected_initialization_never_echoes_configuration_values()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let canary = "DO_NOT_ECHO_PROVIDER_SECRET";
    fs::write(
        directory.path().join("invalid.toml"),
        format!("config_version = 1\n[server]\nbind = \"{canary}\"\n"),
    )?;
    for arguments in [
        vec!["--config", "invalid.toml", "init", "--preview", "--json"],
        vec!["--config", "invalid.toml", "init", "--json"],
    ] {
        let output = control(directory.path(), &arguments)?;
        assert_eq!(output.status.code(), Some(1));
        assert!(!String::from_utf8_lossy(&output.stdout).contains(canary));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(canary));
        assert_eq!(fs::read_dir(directory.path())?.count(), 1);
    }
    Ok(())
}

#[cfg(feature = "cli-operator-preview")]
#[test]
fn rotation_does_not_initialize_missing_control_state() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let output = control(
        directory.path(),
        &["server", "rotate-control-credential", "--json"],
    )?;
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fs::read_dir(directory.path())?.count(), 0);
    Ok(())
}
