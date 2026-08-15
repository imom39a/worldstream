//! Checked Counter normal/build dependency closure.
//!
//! The Counter revision lock must not absorb unrelated workspace packages,
//! workspace-wide feature unification, or dev-only test dependencies. This
//! module derives one canonical artifact from Cargo's resolved graph plus
//! `Cargo.lock`, then verifies the checked-in copy.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

const ARTIFACT_RELATIVE_PATH: &str = "crates/worldstream-core/src/counter-dependency-closure.json";
const ROOT_PACKAGE: &str = "worldstream-core";
const SCHEMA: &str = "worldstream/counter-dependency-closure/v1";

type EnabledFeatures = BTreeMap<(String, String), BTreeSet<String>>;
type LockEntries<'a> = BTreeMap<(String, String, Option<String>), &'a toml::Value>;

pub fn generate(repository_root: &Path) -> Result<()> {
    let bytes = expected_bytes(repository_root)?;
    let path = repository_root.join(ARTIFACT_RELATIVE_PATH);
    fs::write(&path, bytes).with_context(|| format!("cannot write {}", path.display()))
}

pub fn verify(repository_root: &Path) -> Result<()> {
    let expected = expected_bytes(repository_root)?;
    let path = repository_root.join(ARTIFACT_RELATIVE_PATH);
    let checked_in = fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
    verify_bytes(&expected, &checked_in, &path)
}

pub(crate) fn verify_bytes(expected: &[u8], checked_in: &[u8], path: &Path) -> Result<()> {
    if expected != checked_in {
        bail!(
            "{} differs from the resolved Counter normal/build dependency closure; run `cargo run --locked -p xtask -- counter-deps generate`",
            path.display()
        );
    }
    Ok(())
}

fn expected_bytes(repository_root: &Path) -> Result<Vec<u8>> {
    let repository_root = fs::canonicalize(repository_root)
        .with_context(|| format!("cannot canonicalize {}", repository_root.display()))?;
    let metadata = cargo_metadata(&repository_root)?;
    let lock = cargo_lock(&repository_root)?;
    let features = cargo_tree_features(&repository_root)?;
    let value = closure_value(&repository_root, &metadata, &lock, &features)?;
    let mut bytes = serde_json::to_vec_pretty(&value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn cargo_metadata(repository_root: &Path) -> Result<Value> {
    let output = Command::new("cargo")
        .current_dir(repository_root)
        .args(["metadata", "--format-version=1", "--locked"])
        .output()
        .context("cannot invoke cargo metadata for Counter dependency closure")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed while resolving Counter dependency closure: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("cargo metadata returned invalid JSON")
}

fn cargo_lock(repository_root: &Path) -> Result<toml::Value> {
    let path = repository_root.join("Cargo.lock");
    let source =
        fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    toml::from_str(&source).with_context(|| format!("invalid {}", path.display()))
}

fn cargo_tree_features(repository_root: &Path) -> Result<EnabledFeatures> {
    let output = Command::new("cargo")
        .current_dir(repository_root)
        .args([
            "tree",
            "--locked",
            "-p",
            ROOT_PACKAGE,
            "--target",
            "all",
            "-e",
            "normal,build,features",
            "--format",
            "{p}|{f}",
        ])
        .output()
        .context("cannot invoke cargo tree for Counter dependency features")?;
    if !output.status.success() {
        bail!(
            "cargo tree failed while resolving Counter dependency features: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let mut features = EnabledFeatures::new();
    for raw_line in String::from_utf8(output.stdout)
        .context("cargo tree emitted non-UTF-8 feature output")?
        .lines()
    {
        if raw_line.contains("[build-dependencies]") {
            continue;
        }
        let line =
            raw_line.trim_start_matches(|character: char| !character.is_ascii_alphanumeric());
        if line.is_empty() || line == "(*)" || line.contains(" feature ") {
            continue;
        }
        let (package, enabled) = line
            .split_once('|')
            .with_context(|| format!("cargo tree package line lacks feature separator: {line}"))?;
        let (name, version) = package
            .split_once(" v")
            .with_context(|| format!("cargo tree package line lacks name/version: {line}"))?;
        let version = version
            .split_whitespace()
            .next()
            .context("cargo tree package version is empty")?;
        let enabled = enabled.strip_suffix(" (*)").unwrap_or(enabled);
        let entry = features
            .entry((name.to_owned(), version.to_owned()))
            .or_default();
        entry.extend(
            enabled
                .split(',')
                .filter(|feature| !feature.is_empty())
                .map(str::to_owned),
        );
    }
    if features.is_empty() {
        bail!("cargo tree yielded no Counter package feature records");
    }
    Ok(features)
}

fn closure_value(
    repository_root: &Path,
    metadata: &Value,
    lock: &toml::Value,
    enabled_features: &EnabledFeatures,
) -> Result<Value> {
    let packages = metadata["packages"]
        .as_array()
        .context("cargo metadata has no packages array")?;
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .context("cargo metadata has no resolved node array")?;

    let root = packages
        .iter()
        .find(|package| {
            package["name"] == ROOT_PACKAGE
                && package["manifest_path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with("/crates/worldstream-core/Cargo.toml"))
        })
        .context("cargo metadata has no worldstream-core package")?;
    let root_id = string(root, "id")?.to_owned();

    let package_by_id: BTreeMap<_, _> = packages
        .iter()
        .map(|package| Ok((string(package, "id")?.to_owned(), package)))
        .collect::<Result<_>>()?;
    let node_by_id: BTreeMap<_, _> = nodes
        .iter()
        .map(|node| Ok((string(node, "id")?.to_owned(), node)))
        .collect::<Result<_>>()?;
    let lock_by_package = lock_entries(lock)?;

    let mut reachable = BTreeSet::new();
    let mut queue = VecDeque::from([root_id]);
    while let Some(id) = queue.pop_front() {
        if !reachable.insert(id.clone()) {
            continue;
        }
        let node = node_by_id
            .get(&id)
            .copied()
            .with_context(|| format!("resolved graph has no node for {id}"))?;
        for dependency in normal_or_build_dependencies(node)? {
            queue.push_back(dependency);
        }
    }

    let mut records = Vec::new();
    for id in reachable {
        let package = package_by_id
            .get(&id)
            .copied()
            .with_context(|| format!("metadata has no package for {id}"))?;
        let node = node_by_id
            .get(&id)
            .copied()
            .with_context(|| format!("resolved graph has no node for {id}"))?;
        let name = string(package, "name")?;
        let version = string(package, "version")?;
        let enabled_features = enabled_features
            .get(&(name.to_owned(), version.to_owned()))
            .with_context(|| format!("cargo tree has no feature record for {name} {version}"))?;
        let source = package_source(repository_root, package)?;
        let checksum = lock_by_package
            .get(&(
                name.to_owned(),
                version.to_owned(),
                package["source"].as_str().map(str::to_owned),
            ))
            .and_then(|entry| entry.get("checksum"))
            .and_then(toml::Value::as_str);
        // Do not use `cargo metadata` node features: they are workspace-wide
        // unified. `cargo tree -p worldstream-core` scopes this to Counter's
        // normal/build root graph, so behavior-affecting features are bound
        // without absorbing unrelated workspace feature requests.
        let edges = normal_or_build_edges(repository_root, node, &package_by_id)?;
        records.push(json!({
            "identity": package_identity(repository_root, package)?,
            "name": name,
            "version": version,
            "source": source,
            "checksum": checksum,
            "enabled_features": enabled_features,
            "normal_or_build_edges": edges,
        }));
    }
    records
        .sort_unstable_by(|left, right| left["identity"].as_str().cmp(&right["identity"].as_str()));

    Ok(json!({
        "schema": SCHEMA,
        "root": package_identity(repository_root, root)?,
        "packages": records,
    }))
}

fn normal_or_build_dependencies(node: &Value) -> Result<Vec<String>> {
    Ok(node["deps"]
        .as_array()
        .context("resolved node has no deps")?
        .iter()
        .filter_map(|dependency| {
            let kinds = dependency["dep_kinds"].as_array()?;
            kinds
                .iter()
                .any(|kind| kind["kind"].is_null() || kind["kind"].as_str() == Some("build"))
                .then(|| dependency["pkg"].as_str().map(str::to_owned))
                .flatten()
        })
        .collect::<Vec<_>>())
}

fn normal_or_build_edges(
    repository_root: &Path,
    node: &Value,
    package_by_id: &BTreeMap<String, &Value>,
) -> Result<Vec<Value>> {
    let mut edges = Vec::new();
    for dependency in node["deps"]
        .as_array()
        .context("resolved node has no deps")?
    {
        let mut kinds = dependency["dep_kinds"]
            .as_array()
            .context("resolved dependency has no dep_kinds")?
            .iter()
            .filter_map(|kind| match kind["kind"].as_str() {
                None => Some("normal"),
                Some("build") => Some("build"),
                Some(_) => None,
            })
            .collect::<Vec<_>>();
        kinds.sort_unstable();
        kinds.dedup();
        if kinds.is_empty() {
            continue;
        }
        let package_id = dependency["pkg"]
            .as_str()
            .context("resolved dependency has no package ID")?;
        let package = package_by_id
            .get(package_id)
            .copied()
            .with_context(|| format!("metadata has no package for {package_id}"))?;
        edges.push(json!({
            // Cargo metadata emits the effective dependency name here: this is
            // the rename when the manifest uses `package = "..."`.
            "dependency_name_or_rename": string(dependency, "name")?,
            "kinds": kinds,
            "package": package_identity(repository_root, package)?,
        }));
    }
    edges.sort_unstable_by(|left, right| {
        left["dependency_name_or_rename"]
            .as_str()
            .cmp(&right["dependency_name_or_rename"].as_str())
            .then_with(|| left["package"].as_str().cmp(&right["package"].as_str()))
    });
    Ok(edges)
}

fn lock_entries(lock: &toml::Value) -> Result<LockEntries<'_>> {
    lock["package"]
        .as_array()
        .context("Cargo.lock has no package array")?
        .iter()
        .map(|entry| {
            let name = entry["name"]
                .as_str()
                .context("lock package has no name")?
                .to_owned();
            let version = entry["version"]
                .as_str()
                .context("lock package has no version")?
                .to_owned();
            let source = entry
                .get("source")
                .and_then(toml::Value::as_str)
                .map(str::to_owned);
            Ok(((name, version, source), entry))
        })
        .collect()
}

fn package_identity(repository_root: &Path, package: &Value) -> Result<String> {
    Ok(format!(
        "{}@{}#{}",
        string(package, "name")?,
        string(package, "version")?,
        package_source(repository_root, package)?
    ))
}

fn package_source(repository_root: &Path, package: &Value) -> Result<String> {
    if let Some(source) = package["source"].as_str() {
        return Ok(source.to_owned());
    }
    let manifest = PathBuf::from(string(package, "manifest_path")?);
    let directory = manifest
        .parent()
        .context("package manifest has no parent")?;
    let relative = directory
        .strip_prefix(repository_root)
        .with_context(|| format!("local package {} escapes workspace", directory.display()))?;
    Ok(format!("path:{}", relative.to_string_lossy()))
}

fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value[field]
        .as_str()
        .with_context(|| format!("metadata object has no string {field}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_closure_excludes_the_unrelated_sqlite_adapter() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let expected =
            expected_bytes(&root).unwrap_or_else(|error| unreachable!("closure: {error}"));
        let value: Value = serde_json::from_slice(&expected)
            .unwrap_or_else(|error| unreachable!("closure JSON: {error}"));
        assert!(value["packages"]
            .as_array()
            .unwrap_or_else(|| unreachable!("packages"))
            .iter()
            .all(|package| package["name"] != "rusqlite" && package["name"] != "libsqlite3-sys"));
    }

    #[test]
    fn closure_verification_fails_closed_for_real_byte_drift() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let expected =
            expected_bytes(&root).unwrap_or_else(|error| unreachable!("closure: {error}"));
        let mut drifted = expected.clone();
        let index = drifted
            .iter()
            .position(u8::is_ascii_alphanumeric)
            .unwrap_or_else(|| unreachable!("canonical closure has text"));
        drifted[index] = b'x';
        let error = verify_bytes(
            &expected,
            &drifted,
            Path::new("counter-dependency-closure.json"),
        );
        assert!(error.is_err());
    }

    #[test]
    fn core_scoped_feature_drift_changes_counter_closure() {
        let root = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))
            .unwrap_or_else(|error| unreachable!("workspace root: {error}"));
        let metadata =
            cargo_metadata(&root).unwrap_or_else(|error| unreachable!("metadata: {error}"));
        let lock = cargo_lock(&root).unwrap_or_else(|error| unreachable!("lock: {error}"));
        let features = cargo_tree_features(&root)
            .unwrap_or_else(|error| unreachable!("Counter features: {error}"));
        let expected = closure_value(&root, &metadata, &lock, &features)
            .unwrap_or_else(|error| unreachable!("closure: {error}"));

        let mut with_core_feature = features.clone();
        with_core_feature
            .entry((ROOT_PACKAGE.to_owned(), "0.1.0".to_owned()))
            .or_default()
            .insert("behavior-affecting-core-feature".to_owned());
        let actual = closure_value(&root, &metadata, &lock, &with_core_feature)
            .unwrap_or_else(|error| unreachable!("feature-bound closure: {error}"));
        assert_ne!(actual, expected);
    }

    #[test]
    fn unrelated_workspace_and_sqlite_features_do_not_change_counter_closure() {
        let root = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))
            .unwrap_or_else(|error| unreachable!("workspace root: {error}"));
        let metadata =
            cargo_metadata(&root).unwrap_or_else(|error| unreachable!("metadata: {error}"));
        let lock = cargo_lock(&root).unwrap_or_else(|error| unreachable!("lock: {error}"));
        let features = cargo_tree_features(&root)
            .unwrap_or_else(|error| unreachable!("Counter features: {error}"));
        let expected = closure_value(&root, &metadata, &lock, &features)
            .unwrap_or_else(|error| unreachable!("closure: {error}"));

        let mut workspace_unified_metadata = metadata.clone();
        let shared = workspace_unified_metadata["resolve"]["nodes"]
            .as_array_mut()
            .unwrap_or_else(|| unreachable!("resolved nodes"))
            .iter_mut()
            .find(|node| node["id"].as_str().is_some_and(|id| id.contains("#serde@")))
            .unwrap_or_else(|| unreachable!("shared serde node"));
        shared["features"]
            .as_array_mut()
            .unwrap_or_else(|| unreachable!("serde features"))
            .push(Value::String("unrelated-workspace-feature".to_owned()));
        let from_workspace_unification =
            closure_value(&root, &workspace_unified_metadata, &lock, &features)
                .unwrap_or_else(|error| unreachable!("feature-independent closure: {error}"));
        assert_eq!(from_workspace_unification, expected);

        let mut with_unrelated_sqlite_feature = features.clone();
        with_unrelated_sqlite_feature
            .entry(("rusqlite".to_owned(), "0.40.1".to_owned()))
            .or_default()
            .insert("bundled".to_owned());
        let from_sqlite_feature =
            closure_value(&root, &metadata, &lock, &with_unrelated_sqlite_feature)
                .unwrap_or_else(|error| unreachable!("sqlite-independent closure: {error}"));
        assert_eq!(from_sqlite_feature, expected);
    }
}
