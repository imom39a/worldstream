use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use worldstream_core::{PackDigestV1, builtin_agent_heist_registry, builtin_counter_registry};

use crate::counter_dependency_closure;

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
    verify_embedded_storage_identity(&manifest)?;
    counter_dependency_closure::verify(repository_root)?;
    verify_embedded_counter_registry(&manifest)?;
    verify_embedded_agent_heist_registry(&manifest)?;
    Ok(())
}

fn verify_embedded_storage_identity(manifest: &Value) -> Result<()> {
    let sqlite = &manifest["storage"]["sqlite"];
    if sqlite["version"] != worldstream_sqlite::SQLITE_VERSION
        || sqlite["source_id"] != worldstream_sqlite::SQLITE_SOURCE_ID
        || sqlite["bundle_source_inventory_revision"]
            != worldstream_sqlite::RUSQLITE_BUNDLE_REVISION
        || sqlite["bundle_source_inventory_digest"]
            != worldstream_sqlite::SQLITE_BUNDLE_SOURCE_INVENTORY_DIGEST
        || sqlite["bundle_source_inventory_status"] != "resolved"
    {
        bail!("manifest bundled SQLite identity differs from the embedded adapter");
    }

    let postgres = &manifest["storage"]["postgresql"];
    let verified_patches = postgres["release_verified_patches"]
        .as_array()
        .context("manifest has no PostgreSQL release_verified_patches array")?;
    if postgres["minimum_patch"] != worldstream_postgres::POSTGRES_MINIMUM_PATCH
        || verified_patches.as_slice()
            != [Value::String(
                worldstream_postgres::POSTGRES_MINIMUM_PATCH.to_owned(),
            )]
        || postgres["release_verified_patches_status"] != "resolved"
    {
        bail!("manifest PostgreSQL patch identity differs from the embedded adapter");
    }

    let migrations = &manifest["migrations"];
    if migrations["logical_history_id"] != worldstream_sqlite::LOGICAL_HISTORY_ID
        || migrations["logical_history_id"] != worldstream_postgres::LOGICAL_HISTORY_ID
    {
        bail!("manifest logical migration history differs from the adapters");
    }
    let embedded_schema_fingerprint =
        worldstream_postgres::schema_contract_fingerprint().to_string();
    if migrations["schema_contract_fingerprint"].as_str()
        != Some(embedded_schema_fingerprint.as_str())
        || migrations["schema_contract_fingerprint_status"] != "implementation_verified"
    {
        bail!(
            "manifest schema fingerprint differs from the PostgreSQL adapter: expected {embedded_schema_fingerprint}, found {}",
            migrations["schema_contract_fingerprint"]
        );
    }

    let entries = migrations["entries"]
        .as_array()
        .context("manifest has no migrations.entries array")?;
    let mut rows = BTreeMap::new();
    for row in entries {
        let id = row["id"]
            .as_str()
            .context("manifest migration entry has no string id")?;
        if rows.insert(id, row).is_some() {
            bail!("manifest contains duplicate migration id {id}");
        }
        if row["status"] != "implementation_verified" {
            bail!("manifest migration {id} is not implementation_verified");
        }
    }

    let sqlite_history = worldstream_sqlite::migration_history();
    let postgres_history = worldstream_postgres::migration_history();
    let sqlite_by_id = sqlite_history
        .iter()
        .map(|descriptor| (descriptor.id, descriptor.checksum().to_string()))
        .collect::<BTreeMap<_, _>>();
    let postgres_by_id = postgres_history
        .iter()
        .map(|descriptor| (descriptor.id, descriptor.checksum().to_string()))
        .collect::<BTreeMap<_, _>>();
    let expected_ids = sqlite_by_id
        .keys()
        .chain(postgres_by_id.keys())
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    if rows
        .keys()
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        != expected_ids
    {
        bail!("manifest migration IDs differ from the typed adapter histories");
    }

    for (id, row) in rows {
        let sqlite_checksum = row["sqlite_checksum"]
            .as_str()
            .context("manifest migration has no string sqlite_checksum")?;
        let postgres_checksum = row["postgresql_checksum"]
            .as_str()
            .context("manifest migration has no string postgresql_checksum")?;
        match sqlite_by_id.get(id) {
            Some(expected) if sqlite_checksum == expected => {}
            Some(_) => bail!("manifest SQLite migration checksum differs for {id}"),
            None if sqlite_checksum.is_empty() => {}
            None => bail!("non-SQLite migration {id} has a SQLite checksum"),
        }
        match postgres_by_id.get(id) {
            Some(expected) if postgres_checksum == expected => {}
            Some(_) => bail!("manifest PostgreSQL migration checksum differs for {id}"),
            None if postgres_checksum.is_empty() => {}
            None => bail!("non-PostgreSQL migration {id} has a PostgreSQL checksum"),
        }
    }
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

fn verify_embedded_agent_heist_registry(manifest: &Value) -> Result<()> {
    let rows = manifest["pack_executors"]
        .as_array()
        .context("manifest has no pack_executors array")?;
    let heist_rows: Vec<_> = rows
        .iter()
        .filter(|row| row["pack_id"] == "worldstream.agent-heist")
        .collect();
    let registry =
        builtin_agent_heist_registry().context("embedded Agent Heist registry is invalid")?;
    if heist_rows.len() != 2 {
        bail!("manifest must contain exactly retained-only and active Agent Heist executor rows");
    }

    for (expected_version, expected_selectable) in [("0.0.1", false), ("0.1.0", true)] {
        let row = heist_rows
            .iter()
            .copied()
            .find(|row| row["explanatory_version"] == expected_version)
            .with_context(|| format!("manifest is missing Agent Heist {expected_version}"))?;
        let digest_text = row["revision_digest"]
            .as_str()
            .context("Agent Heist manifest row has no revision_digest")?;
        let digest: PackDigestV1 = digest_text
            .parse()
            .context("Agent Heist manifest row has a malformed revision_digest")?;
        let retained = registry.load_retained(&digest).with_context(|| {
            format!("manifest Agent Heist {expected_version} is not the embedded retained executor")
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
                bail!(
                    "manifest Agent Heist {expected_version} {field} differs from embedded executor"
                );
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
                bail!(
                    "manifest Agent Heist {expected_version} {field} differs from embedded executor"
                );
            }
        }

        if row["revision_digest_algorithm"] != "blake3"
            || row["status"] != "resolved"
            || row["runnable_for_retained_rooms"] != true
            || row["selectable_for_new_rooms"] != expected_selectable
            || row["required_for_release"] != true
        {
            bail!("manifest Agent Heist {expected_version} status contract is invalid");
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

    use super::{
        canonical_from_toml, sort_json, verify_embedded_storage_identity, verify_node_workspace,
        verify_python_workspace,
    };

    #[test]
    fn embedded_storage_identity_is_exact_and_rejects_non_applicable_checksums() {
        let mut manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../compatibility.json"))
                .unwrap_or_else(|error| unreachable!("parse compatibility fixture: {error}"));
        assert!(verify_embedded_storage_identity(&manifest).is_ok());

        let postgres_only = manifest["migrations"]["entries"]
            .as_array_mut()
            .and_then(|entries| {
                entries
                    .iter_mut()
                    .find(|row| row["id"] == "0003-kernel-conformance-v1")
            })
            .unwrap_or_else(|| unreachable!("PostgreSQL-only migration fixture"));
        postgres_only["sqlite_checksum"] = serde_json::Value::String(
            "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        );
        assert!(verify_embedded_storage_identity(&manifest).is_err());
    }

    #[test]
    fn embedded_storage_identity_rejects_stale_schema_fingerprint() {
        let mut manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../compatibility.json"))
                .unwrap_or_else(|error| unreachable!("parse compatibility fixture: {error}"));
        manifest["migrations"]["schema_contract_fingerprint"] = serde_json::Value::String(
            "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        );

        let error = verify_embedded_storage_identity(&manifest)
            .err()
            .unwrap_or_else(|| {
                unreachable!("a stale schema fingerprint must fail compatibility verification")
            });
        assert!(
            error
                .to_string()
                .contains("schema fingerprint differs from the PostgreSQL adapter")
        );
    }

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
