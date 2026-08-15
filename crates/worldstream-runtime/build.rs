use std::{env, error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let repository_root = crate_dir.join("../..");
    let toml_path = repository_root.join("compatibility.toml");
    let json_path = repository_root.join("compatibility.json");

    println!("cargo:rerun-if-changed={}", toml_path.display());
    println!("cargo:rerun-if-changed={}", json_path.display());

    let authored = fs::read_to_string(&toml_path)?;
    let parsed_toml: toml::Value = toml::from_str(&authored)?;
    let canonical = canonical_json_bytes(serde_json::to_value(parsed_toml)?)?;
    let checked_in = fs::read(&json_path)?;

    if canonical != checked_in {
        return Err(format!(
            "{} drifted from {}; run `cargo xtask compat generate`",
            json_path.display(),
            toml_path.display()
        )
        .into());
    }

    Ok(())
}

fn canonical_json_bytes(value: serde_json::Value) -> Result<Vec<u8>, serde_json::Error> {
    let mut bytes = serde_json::to_vec_pretty(&sort_json(value))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn sort_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(sort_json).collect())
        }
        serde_json::Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sort_json(value)))
                    .collect(),
            )
        }
        scalar => scalar,
    }
}
