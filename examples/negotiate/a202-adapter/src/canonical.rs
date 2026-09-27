use std::{cmp::Ordering, fmt::Write as _};

use serde_json::{Map, Value};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CanonicalError {
    #[error("JSON input is not UTF-8")]
    Utf8,
    #[error("JSON input could not be parsed: {0}")]
    Json(String),
    #[error("A202 shared objects permit integers, not floating-point numbers")]
    FloatingPoint,
    #[error("JSON number is outside the supported integer range")]
    IntegerRange,
    #[error("canonical JSON formatting failed")]
    Formatting,
    #[error("received bytes are not their own RFC 8785 canonical form")]
    NonCanonical,
}

/// Parse exact bytes and prove they are already canonical. The returned value
/// is a convenience view only; callers retain and verify the original bytes.
pub(crate) fn parse_exact_canonical(input: &[u8]) -> Result<Value, CanonicalError> {
    let text = std::str::from_utf8(input).map_err(|_| CanonicalError::Utf8)?;
    let value: Value =
        serde_json::from_str(text).map_err(|error| CanonicalError::Json(error.to_string()))?;
    if canonical_bytes(&value)? != input {
        return Err(CanonicalError::NonCanonical);
    }
    Ok(value)
}

/// Serialize a value using the pinned A202 canonical JSON subset.
///
/// # Errors
///
/// Returns an error for floating-point or unsupported numeric values, or when
/// canonical formatting fails.
pub fn canonical_bytes(value: &Value) -> Result<Vec<u8>, CanonicalError> {
    let mut output = String::new();
    write_value(value, &mut output)?;
    Ok(output.into_bytes())
}

/// A202 content hashing and signature bytes omit exactly three top-level
/// members. Nested members with the same names remain covered.
///
/// # Errors
///
/// Returns an error when the input is not an object or contains a value outside
/// the pinned canonical JSON subset.
pub fn canonical_content_bytes(value: &Value) -> Result<Vec<u8>, CanonicalError> {
    let object = value
        .as_object()
        .ok_or_else(|| CanonicalError::Json("top-level value is not an object".to_owned()))?;
    let mut stripped = Map::new();
    for (key, item) in object {
        if !matches!(
            key.as_str(),
            "content_hash" | "signatures" | "kernel_annotations"
        ) {
            stripped.insert(key.clone(), item.clone());
        }
    }
    canonical_bytes(&Value::Object(stripped))
}

fn write_value(value: &Value, output: &mut String) -> Result<(), CanonicalError> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(boolean) => output.push_str(if *boolean { "true" } else { "false" }),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                write!(output, "{integer}").map_err(|_| CanonicalError::Formatting)?;
            } else if let Some(integer) = number.as_u64() {
                write!(output, "{integer}").map_err(|_| CanonicalError::Formatting)?;
            } else if number.as_f64().is_some() {
                return Err(CanonicalError::FloatingPoint);
            } else {
                return Err(CanonicalError::IntegerRange);
            }
        }
        Value::String(string) => write_string(string, output)?,
        Value::Array(array) => {
            output.push('[');
            for (index, item) in array.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write_value(item, output)?;
            }
            output.push(']');
        }
        Value::Object(object) => {
            output.push('{');
            let mut members: Vec<_> = object.iter().collect();
            members.sort_by(|(left, _), (right, _)| utf16_cmp(left, right));
            for (index, (key, item)) in members.into_iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write_string(key, output)?;
                output.push(':');
                write_value(item, output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

fn utf16_cmp(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

fn write_string(value: &str, output: &mut String) -> Result<(), CanonicalError> {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{0008}' => output.push_str("\\b"),
            '\u{000C}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            control if control <= '\u{001F}' => {
                write!(output, "\\u{:04x}", u32::from(control))
                    .map_err(|_| CanonicalError::Formatting)?;
            }
            ordinary => output.push(ordinary),
        }
    }
    output.push('"');
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{CanonicalError, canonical_bytes, parse_exact_canonical};

    #[test]
    fn sorts_by_utf16_and_uses_compact_lowercase_escapes() -> Result<(), CanonicalError> {
        let value = json!({"\u{1f600}": 1, "\u{e000}": 2, "line": "\n\u{000f}"});
        let encoded = canonical_bytes(&value)?;
        assert_eq!(
            String::from_utf8(encoded).map_err(|_| CanonicalError::Utf8)?,
            "{\"line\":\"\\n\\u000f\",\"😀\":1,\"\":2}"
        );
        Ok(())
    }

    #[test]
    fn refuses_noncanonical_and_float_input() {
        assert!(matches!(
            parse_exact_canonical(br#"{ "a": 1 }"#),
            Err(CanonicalError::NonCanonical)
        ));
        assert!(matches!(
            parse_exact_canonical(br#"{"a":1.5}"#),
            Err(CanonicalError::FloatingPoint)
        ));
    }
}
