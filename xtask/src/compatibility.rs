use std::{fs, path::Path};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use worldstream_core::{PackDigestV1, builtin_counter_registry};

const EXPECTED_MANIFEST_SCHEMA: &str = "worldstream/storage-compatibility-manifest/v1";

pub fn generate(repository_root: &Path) -> Result<()> {
    let canonical = canonical_from_toml(&repository_root.join("compatibility.toml"))?;
    let target = repository_root.join("compatibility.json");
    fs::write(&target, canonical)
        .with_context(|| format!("cannot write canonical mirror {}", target.display()))?;
    Ok(())
}

pub fn verify(repository_root: &Path) -> Result<()> {
    let toml_path = repository_root.join("compatibility.toml");
    let json_path = repository_root.join("compatibility.json");
    let canonical = canonical_from_toml(&toml_path)?;
    let checked_in = fs::read(&json_path)
        .with_context(|| format!("cannot read canonical mirror {}", json_path.display()))?;
    if canonical != checked_in {
        bail!(
            "{} is not the deterministic sorted mirror of {}; run `cargo xtask compat generate`",
            json_path.display(),
            toml_path.display()
        );
    }

    let manifest: Value = serde_json::from_slice(&checked_in)
        .with_context(|| format!("invalid JSON in {}", json_path.display()))?;
    verify_required_identity(&manifest)?;
    verify_workspace_toolchain(repository_root, &manifest)?;
    verify_embedded_counter_registry(&manifest)?;
    Ok(())
}

fn verify_embedded_counter_registry(manifest: &Value) -> Result<()> {
    let rows = manifest["pack_executors"]
        .as_array()
        .context("manifest has no pack_executors array")?;
    let counter_rows: Vec<_> = rows
        .iter()
        .filter(|row| row["pack_id"] == "worldstream.counter")
        .collect();
    if counter_rows.len() != 2 {
        bail!("manifest must contain exactly Counter v1 and v2 executor rows");
    }

    let registry = builtin_counter_registry().context("embedded Counter registry is invalid")?;
    for expected_version in ["1.0.0", "2.0.0"] {
        let row = counter_rows
            .iter()
            .copied()
            .find(|row| row["explanatory_version"] == expected_version)
            .with_context(|| format!("manifest is missing Counter {expected_version}"))?;
        let digest_text = row["revision_digest"]
            .as_str()
            .context("Counter manifest row has no revision_digest")?;
        let digest: PackDigestV1 = digest_text
            .parse()
            .context("Counter manifest row has a malformed revision_digest")?;
        let retained = registry.load_retained(&digest).with_context(|| {
            format!("manifest Counter {expected_version} is not the embedded retained executor")
        })?;
        let descriptor = retained.descriptor();
        let revision_lock = retained.revision_lock();
        let revision_digest = descriptor.revision_digest.to_string();

        let exact = [
            ("pack_id", descriptor.pack_id.as_str()),
            (
                "explanatory_version",
                descriptor.explanatory_version.as_str(),
            ),
            ("host_contract_id", descriptor.host_contract.as_str()),
            ("revision_lock_id", revision_lock.revision_lock_id.as_str()),
            ("revision_digest", revision_digest.as_str()),
        ];
        for (field, embedded) in exact {
            if row[field].as_str() != Some(embedded) {
                bail!("manifest Counter {expected_version} {field} differs from embedded executor");
            }
        }
        let digests = [
            (
                "descriptor_digest",
                revision_lock.descriptor_digest.to_string(),
            ),
            (
                "executor_artifact_digest",
                retained.executor_artifact_digest().to_string(),
            ),
            (
                "schema_bundle_digest",
                revision_lock.schema_bundle_digest.to_string(),
            ),
            (
                "codec_bundle_digest",
                revision_lock.codec_bundle_digest.to_string(),
            ),
            (
                "golden_corpus_digest",
                retained.golden_corpus_digest().to_string(),
            ),
        ];
        for (field, embedded) in digests {
            if row[field].as_str() != Some(embedded.as_str()) {
                bail!("manifest Counter {expected_version} {field} differs from embedded executor");
            }
        }
        if row["revision_digest_algorithm"] != "blake3"
            || row["status"] != "resolved"
            || row["runnable_for_retained_rooms"] != true
            || row["selectable_for_new_rooms"] != false
            || row["required_for_release"] != true
        {
            bail!("manifest Counter {expected_version} status contract is invalid");
        }
    }
    Ok(())
}

fn canonical_from_toml(path: &Path) -> Result<Vec<u8>> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("cannot read authored manifest {}", path.display()))?;
    let authored: toml::Value = toml::from_str(&source)
        .with_context(|| format!("invalid authored TOML manifest {}", path.display()))?;
    let value = sort_json(serde_json::to_value(authored)?);
    let mut output = serde_json::to_vec_pretty(&value)?;
    output.push(b'\n');
    Ok(output)
}

fn sort_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(sort_json).collect()),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sort_json(value)))
                    .collect(),
            )
        }
        scalar => scalar,
    }
}

fn verify_required_identity(manifest: &Value) -> Result<()> {
    if manifest["schema"] != EXPECTED_MANIFEST_SCHEMA {
        bail!("unsupported compatibility schema");
    }
    if manifest["manifest_revision"] != 1 {
        bail!("unsupported compatibility manifest revision");
    }
    if manifest["canonical_mirror"] != "compatibility.json"
        || manifest["reviewed_source"] != "compatibility.toml"
    {
        bail!("manifest source/mirror identity is inconsistent");
    }
    if manifest["contracts"]["product"] != manifest["release_candidate"] {
        bail!("release candidate and product contract differ");
    }
    Ok(())
}

fn verify_workspace_toolchain(repository_root: &Path, manifest: &Value) -> Result<()> {
    let toolchain_path = repository_root.join("rust-toolchain.toml");
    let toolchain_source = fs::read_to_string(&toolchain_path)
        .with_context(|| format!("cannot read {}", toolchain_path.display()))?;
    let toolchain: toml::Value = toml::from_str(&toolchain_source)?;
    let pinned_toolchain = toolchain
        .get("toolchain")
        .and_then(|value| value.get("channel"))
        .and_then(toml::Value::as_str)
        .context("rust-toolchain.toml has no toolchain.channel")?;
    if manifest["toolchains"]["rust"] != pinned_toolchain {
        bail!("manifest Rust toolchain differs from rust-toolchain.toml");
    }

    let workspace_path = repository_root.join("Cargo.toml");
    let workspace_source = fs::read_to_string(&workspace_path)
        .with_context(|| format!("cannot read {}", workspace_path.display()))?;
    let workspace: toml::Value = toml::from_str(&workspace_source)?;
    let edition = workspace
        .get("workspace")
        .and_then(|value| value.get("package"))
        .and_then(|value| value.get("edition"))
        .and_then(toml::Value::as_str)
        .context("Cargo.toml has no workspace.package.edition")?;
    let rust_version = workspace
        .get("workspace")
        .and_then(|value| value.get("package"))
        .and_then(|value| value.get("rust-version"))
        .and_then(toml::Value::as_str)
        .context("Cargo.toml has no workspace.package.rust-version")?;
    if manifest["toolchains"]["rust_edition"] != edition
        || manifest["toolchains"]["rust"] != rust_version
    {
        bail!("manifest Rust toolchain differs from workspace package pins");
    }

    verify_node_workspace(repository_root, manifest)?;
    verify_python_workspace(repository_root, manifest)?;
    Ok(())
}

fn verify_node_workspace(repository_root: &Path, manifest: &Value) -> Result<()> {
    let node = manifest["toolchains"]["node"]
        .as_str()
        .context("manifest has no string toolchains.node pin")?;
    for relative in [".node-version", ".nvmrc"] {
        let path = repository_root.join(relative);
        let pin =
            fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
        if pin.trim() != node {
            bail!("manifest Node pin differs from {relative}");
        }
    }

    let product = manifest["contracts"]["product"]
        .as_str()
        .context("manifest has no string contracts.product")?;
    for relative in ["package.json", "web/console/package.json"] {
        let path = repository_root.join(relative);
        let source = fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
        let package: Value = serde_json::from_slice(&source)
            .with_context(|| format!("invalid JSON in {}", path.display()))?;
        if package["engines"]["node"] != node {
            bail!("manifest Node pin differs from {relative} engines.node");
        }
        if package["version"] != product {
            bail!("manifest product version differs from {relative}");
        }
    }
    Ok(())
}

fn verify_python_workspace(repository_root: &Path, manifest: &Value) -> Result<()> {
    let toolchains = &manifest["toolchains"];
    let minimum = toolchains["python_sdk_min"]
        .as_str()
        .context("manifest has no string toolchains.python_sdk_min")?;
    let maximum = toolchains["python_sdk_max"]
        .as_str()
        .context("manifest has no string toolchains.python_sdk_max")?;
    let quickstart = toolchains["python_quickstart"]
        .as_str()
        .context("manifest has no string toolchains.python_quickstart")?;
    if !quickstart.starts_with(&format!("{maximum}.")) {
        bail!("manifest Python quickstart is outside the declared SDK maximum line");
    }

    let version_path = repository_root.join(".python-version");
    let version = fs::read_to_string(&version_path)
        .with_context(|| format!("cannot read {}", version_path.display()))?;
    if version.trim() != quickstart {
        bail!("manifest Python quickstart differs from .python-version");
    }

    let maximum_parts: Vec<_> = maximum.split('.').collect();
    if maximum_parts.len() != 2 {
        bail!("manifest Python SDK maximum must be a major.minor line");
    }
    let maximum_minor: u32 = maximum_parts[1]
        .parse()
        .context("manifest Python SDK maximum minor is not numeric")?;
    let expected_range = format!(">={minimum},<{}.{}", maximum_parts[0], maximum_minor + 1);

    let pyproject_path = repository_root.join("sdk/python/pyproject.toml");
    let pyproject_source = fs::read_to_string(&pyproject_path)
        .with_context(|| format!("cannot read {}", pyproject_path.display()))?;
    let pyproject: toml::Value = toml::from_str(&pyproject_source)?;
    let project = pyproject
        .get("project")
        .and_then(toml::Value::as_table)
        .context("sdk pyproject has no project table")?;
    let requires_python = project
        .get("requires-python")
        .and_then(toml::Value::as_str)
        .context("sdk pyproject has no project.requires-python")?;
    if requires_python.replace(' ', "") != expected_range {
        bail!("manifest Python SDK range differs from sdk/python/pyproject.toml");
    }
    let product = manifest["contracts"]["product"]
        .as_str()
        .context("manifest has no string contracts.product")?;
    if project.get("version").and_then(toml::Value::as_str) != Some(product) {
        bail!("manifest product version differs from sdk/python/pyproject.toml");
    }
    let expected_ruff = format!("py{}", minimum.replace('.', ""));
    let ruff_target = pyproject
        .get("tool")
        .and_then(|value| value.get("ruff"))
        .and_then(|value| value.get("target-version"))
        .and_then(toml::Value::as_str)
        .context("sdk pyproject has no tool.ruff.target-version")?;
    if ruff_target != expected_ruff {
        bail!("manifest Python SDK minimum differs from Ruff target version");
    }

    let lock_path = repository_root.join("sdk/python/uv.lock");
    let lock_source = fs::read_to_string(&lock_path)
        .with_context(|| format!("cannot read {}", lock_path.display()))?;
    let lock: toml::Value = toml::from_str(&lock_source)?;
    let lock_range = lock
        .get("requires-python")
        .and_then(toml::Value::as_str)
        .context("uv.lock has no requires-python")?;
    if lock_range.replace(' ', "") != expected_range {
        bail!("manifest Python SDK range differs from sdk/python/uv.lock");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{canonical_from_toml, sort_json, verify_node_workspace, verify_python_workspace};

    #[test]
    fn canonicalization_sorts_nested_keys_and_keeps_array_order() {
        let input = serde_json::json!({"z": {"b": 2, "a": 1}, "a": [2, 1]});
        let output = serde_json::to_string(&sort_json(input))
            .unwrap_or_else(|error| unreachable!("serialize JSON: {error}"));
        assert_eq!(output, r#"{"a":[2,1],"z":{"a":1,"b":2}}"#);
    }

    #[test]
    fn canonical_toml_output_has_sorted_keys_and_one_newline() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        let path = directory.path().join("manifest.toml");
        fs::write(&path, "z = 2\na = 1\n")
            .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        let output = canonical_from_toml(&path)
            .unwrap_or_else(|error| unreachable!("canonicalize TOML: {error}"));
        assert_eq!(output, b"{\n  \"a\": 1,\n  \"z\": 2\n}\n");
    }

    #[test]
    fn node_workspace_pins_fail_closed_on_drift() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        fs::create_dir_all(directory.path().join("web/console"))
            .unwrap_or_else(|error| unreachable!("create fixture: {error}"));
        fs::write(directory.path().join(".node-version"), "24.18.1\n")
            .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        fs::write(directory.path().join(".nvmrc"), "24.18.1\n")
            .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        let package = br#"{"version":"0.1.0","engines":{"node":"24.18.1"}}"#;
        fs::write(directory.path().join("package.json"), package)
            .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        fs::write(directory.path().join("web/console/package.json"), package)
            .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        let manifest = serde_json::json!({
            "contracts": {"product": "0.1.0"},
            "toolchains": {"node": "24.18.1"}
        });

        assert!(verify_node_workspace(directory.path(), &manifest).is_ok());
        fs::write(directory.path().join(".nvmrc"), "24.18.2\n")
            .unwrap_or_else(|error| unreachable!("write drift: {error}"));
        assert!(verify_node_workspace(directory.path(), &manifest).is_err());
    }

    #[test]
    fn python_workspace_pins_fail_closed_on_drift() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp dir: {error}"));
        fs::create_dir_all(directory.path().join("sdk/python"))
            .unwrap_or_else(|error| unreachable!("create fixture: {error}"));
        fs::write(directory.path().join(".python-version"), "3.14.7\n")
            .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        fs::write(
            directory.path().join("sdk/python/pyproject.toml"),
            "[project]\nversion = \"0.1.0\"\nrequires-python = \">=3.11,<3.15\"\n\n[tool.ruff]\ntarget-version = \"py311\"\n",
        )
        .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        fs::write(
            directory.path().join("sdk/python/uv.lock"),
            "version = 1\nrequires-python = \">=3.11, <3.15\"\n",
        )
        .unwrap_or_else(|error| unreachable!("write fixture: {error}"));
        let manifest = serde_json::json!({
            "contracts": {"product": "0.1.0"},
            "toolchains": {
                "python_sdk_min": "3.11",
                "python_sdk_max": "3.14",
                "python_quickstart": "3.14.7"
            }
        });

        assert!(verify_python_workspace(directory.path(), &manifest).is_ok());
        fs::write(directory.path().join(".python-version"), "3.14.8\n")
            .unwrap_or_else(|error| unreachable!("write drift: {error}"));
        assert!(verify_python_workspace(directory.path(), &manifest).is_err());
    }
}
