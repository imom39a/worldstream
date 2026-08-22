use std::{env, error::Error, fs, path::PathBuf, process::Command};

const REVISION_FILE: &str = ".worldstream-source-revision";

fn main() -> Result<(), Box<dyn Error>> {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let root = crate_dir.join("../..");
    let revision_file = root.join(REVISION_FILE);
    println!("cargo:rerun-if-env-changed=WORLDSTREAM_BUILD_REVISION");
    println!("cargo:rerun-if-changed={}", revision_file.display());
    emit_git_rerun_paths(&root);

    let explicit = env::var("WORLDSTREAM_BUILD_REVISION").ok();
    let git = git_revision(&root);
    if let (Some(expected), Some(observed)) = (&explicit, &git)
        && expected != observed
    {
        return Err(format!(
            "WORLDSTREAM_BUILD_REVISION {expected} differs from checkout HEAD {observed}"
        )
        .into());
    }
    if explicit.is_some() && git.is_some() && !git_checkout_is_clean(&root)? {
        return Err(
            "release build requires a clean Git checkout so the claimed commit binds every compiled source byte"
                .into(),
        );
    }
    let revision = explicit
        .or(git)
        .or_else(|| {
            fs::read_to_string(&revision_file)
                .ok()
                .map(|value| value.trim().to_owned())
        })
        .ok_or("WorldStream build has no exact source revision")?;
    if revision.len() != 40
        || !revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(
            "WorldStream source revision must be exactly 40 lowercase hex characters".into(),
        );
    }
    println!("cargo:rustc-env=WORLDSTREAM_BUILD_REVISION={revision}");
    Ok(())
}

fn git_revision(root: &std::path::Path) -> Option<String> {
    git_stdout(root, &["rev-parse", "HEAD"])
}

fn git_checkout_is_clean(root: &std::path::Path) -> Result<bool, Box<dyn Error>> {
    let output = Command::new("git")
        .args([
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ])
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        return Err("release build could not verify that the Git checkout is clean".into());
    }
    Ok(output.stdout.is_empty())
}

fn emit_git_rerun_paths(root: &std::path::Path) {
    let mut git_paths = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
    if let Some(symbolic_ref) = git_stdout(root, &["symbolic-ref", "-q", "HEAD"]) {
        git_paths.push(symbolic_ref);
    }
    if git_stdout(root, &["rev-parse", "--show-ref-format"]).as_deref() == Some("reftable") {
        git_paths.push("reftable".to_owned());
    }
    for git_path in git_paths {
        let Some(path) = git_stdout(root, &["rev-parse", "--git-path", &git_path]) else {
            continue;
        };
        let path = PathBuf::from(path);
        let resolved = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        println!("cargo:rerun-if-changed={}", resolved.display());
    }
}

fn git_stdout(root: &std::path::Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}
