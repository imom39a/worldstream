//! Shared secret-material rejection for retained public configuration.

use serde_json::Value;

/// Returns true only for explicitly credential-bearing keys or unmistakable
/// credential wire prefixes. Long domain identifiers, revisions, and hashes
/// are ordinary Pack data and must not be classified by entropy alone.
pub(crate) fn contains_credential_material(value: &Value) -> bool {
    match value {
        Value::Object(object) => object
            .iter()
            .any(|(key, value)| credential_key(key) || contains_credential_material(value)),
        Value::Array(values) => values.iter().any(contains_credential_material),
        Value::String(value) => has_explicit_credential_prefix(value),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn credential_key(key: &str) -> bool {
    const CREDENTIAL_KEYS: [&str; 8] = [
        "api_key",
        "apikey",
        "authorization",
        "bearer",
        "credential",
        "password",
        "secret",
        "token",
    ];

    let normalized = key.replace('-', "_").to_ascii_lowercase();
    CREDENTIAL_KEYS
        .iter()
        .any(|credential| normalized.contains(credential))
}

fn has_explicit_credential_prefix(value: &str) -> bool {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    lower.starts_with("bearer ")
        || lower.starts_with("sk-")
        || lower.starts_with("sk_")
        || lower.starts_with("ghp_")
        || lower.starts_with("github_pat_")
        || lower.starts_with("xoxb-")
        || lower.starts_with("xoxp-")
        || value.starts_with("AKIA")
        || (value.starts_with("eyJ") && value.matches('.').count() == 2)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::contains_credential_material;

    #[test]
    fn domain_identifiers_and_hashes_are_not_credentials_by_value() {
        assert!(!contains_credential_material(&json!({
            "a202_revision": "fa85aa8b49bfe7b3f7ded487c98500a600e92e41",
            "session_id": "ses_northstar_delta_worldstream_01",
            "transaction_id": "txn_calibration_worldstream_01",
            "object_hash": "f0355964099f46e7abf4c9c04f1250f4e87b19d1e3b85e3ed0f0577433963798"
        })));
    }

    #[test]
    fn explicit_credential_keys_and_wire_prefixes_are_rejected() {
        for value in [
            json!({"api_key": "ordinary-looking-value"}),
            json!({"nested": {"secret_reference": "vault-entry"}}),
            json!({"header": "Bearer NEVER_PRINT_THIS_SECRET"}),
            json!({"provider": "sk-NEVER_PRINT_THIS_SECRET"}),
            json!({"provider": "github_pat_NEVER_PRINT_THIS_SECRET"}),
            json!({"provider": "eyJheader.payload.signature"}),
        ] {
            assert!(contains_credential_material(&value));
        }
    }
}
