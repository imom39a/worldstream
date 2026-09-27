use std::{cmp::Ordering, fmt::Write as _};

use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IndependentCanonicalError {
    #[error("input is not UTF-8")]
    Utf8,
    #[error("input is not JSON: {0}")]
    Json(String),
    #[error("floating-point values are outside the A202 canonical subset")]
    Float,
    #[error("number is outside the supported integer range")]
    IntegerRange,
    #[error("canonical formatting failed")]
    Formatting,
    #[error("input bytes differ from independent canonical serialization")]
    NotCanonical,
}

pub fn parse_and_check_exact(bytes: &[u8]) -> Result<Value, IndependentCanonicalError> {
    let text = std::str::from_utf8(bytes).map_err(|_| IndependentCanonicalError::Utf8)?;
    let value: Value = serde_json::from_str(text)
        .map_err(|error| IndependentCanonicalError::Json(error.to_string()))?;
    if serialize(&value)? != bytes {
        return Err(IndependentCanonicalError::NotCanonical);
    }
    Ok(value)
}

pub fn serialize(value: &Value) -> Result<Vec<u8>, IndependentCanonicalError> {
    let mut destination = String::new();
    append(value, &mut destination)?;
    Ok(destination.into_bytes())
}

pub fn content_hash(value: &Value) -> Result<String, IndependentCanonicalError> {
    let source = value
        .as_object()
        .ok_or_else(|| IndependentCanonicalError::Json("root is not an object".to_owned()))?;
    let filtered: Map<String, Value> = source
        .iter()
        .filter(|(name, _)| {
            !matches!(
                name.as_str(),
                "content_hash" | "signatures" | "kernel_annotations"
            )
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    Ok(lower_hex(&Sha256::digest(serialize(&Value::Object(
        filtered,
    ))?)))
}

pub fn signature_message(
    value: &Value,
    key_id: &str,
    algorithm: &str,
    purpose: &str,
    signed_at: &str,
) -> Result<Vec<u8>, IndependentCanonicalError> {
    let source = value
        .as_object()
        .ok_or_else(|| IndependentCanonicalError::Json("root is not an object".to_owned()))?;
    let filtered: Map<String, Value> = source
        .iter()
        .filter(|(name, _)| {
            !matches!(
                name.as_str(),
                "content_hash" | "signatures" | "kernel_annotations"
            )
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let mut covered = serialize(&Value::Object(filtered))?;
    covered.push(b'.');
    covered.extend(serialize(&serde_json::json!({
        "algorithm": algorithm,
        "key_id": key_id,
        "purpose": purpose,
        "signed_at": signed_at,
    }))?);
    Ok(covered)
}

fn append(value: &Value, destination: &mut String) -> Result<(), IndependentCanonicalError> {
    match value {
        Value::Null => destination.push_str("null"),
        Value::Bool(true) => destination.push_str("true"),
        Value::Bool(false) => destination.push_str("false"),
        Value::Number(number) => {
            if let Some(signed) = number.as_i64() {
                write!(destination, "{signed}")
                    .map_err(|_| IndependentCanonicalError::Formatting)?;
            } else if let Some(unsigned) = number.as_u64() {
                write!(destination, "{unsigned}")
                    .map_err(|_| IndependentCanonicalError::Formatting)?;
            } else if number.as_f64().is_some() {
                return Err(IndependentCanonicalError::Float);
            } else {
                return Err(IndependentCanonicalError::IntegerRange);
            }
        }
        Value::String(text) => append_string(text, destination)?,
        Value::Array(items) => {
            destination.push('[');
            for (position, item) in items.iter().enumerate() {
                if position > 0 {
                    destination.push(',');
                }
                append(item, destination)?;
            }
            destination.push(']');
        }
        Value::Object(members) => {
            destination.push('{');
            let mut ordered: Vec<_> = members.iter().collect();
            ordered.sort_by(|(left, _), (right, _)| compare_utf16(left, right));
            for (position, (name, item)) in ordered.into_iter().enumerate() {
                if position > 0 {
                    destination.push(',');
                }
                append_string(name, destination)?;
                destination.push(':');
                append(item, destination)?;
            }
            destination.push('}');
        }
    }
    Ok(())
}

fn append_string(text: &str, destination: &mut String) -> Result<(), IndependentCanonicalError> {
    destination.push('"');
    for scalar in text.chars() {
        match scalar {
            '"' => destination.push_str("\\\""),
            '\\' => destination.push_str("\\\\"),
            '\u{8}' => destination.push_str("\\b"),
            '\u{c}' => destination.push_str("\\f"),
            '\n' => destination.push_str("\\n"),
            '\r' => destination.push_str("\\r"),
            '\t' => destination.push_str("\\t"),
            character if u32::from(character) < 0x20 => {
                write!(destination, "\\u{:04x}", u32::from(character))
                    .map_err(|_| IndependentCanonicalError::Formatting)?;
            }
            character => destination.push(character),
        }
    }
    destination.push('"');
    Ok(())
}

fn compare_utf16(left: &str, right: &str) -> Ordering {
    let left_units: Vec<_> = left.encode_utf16().collect();
    let right_units: Vec<_> = right.encode_utf16().collect();
    left_units.cmp(&right_units)
}

fn lower_hex(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                char::from(ALPHABET[usize::from(byte >> 4)]),
                char::from(ALPHABET[usize::from(byte & 15)]),
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{IndependentCanonicalError, parse_and_check_exact, serialize};

    #[test]
    fn independent_serializer_observes_utf16_member_order() -> Result<(), IndependentCanonicalError>
    {
        let encoded = serialize(&json!({"\u{e000}": 2, "\u{1f600}": 1, "z": "\u{000f}"}))?;
        assert_eq!(
            std::str::from_utf8(&encoded).map_err(|_| IndependentCanonicalError::Utf8)?,
            "{\"z\":\"\\u000f\",\"😀\":1,\"\":2}"
        );
        parse_and_check_exact(&encoded)?;
        Ok(())
    }
}
