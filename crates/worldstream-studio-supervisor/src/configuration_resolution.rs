//! Pure resolution of declared Activity Pack configuration values.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Closed resolution failure; supplied configuration values are never displayed.
#[derive(Debug, Deserialize, Error, Serialize)]
#[error("configuration does not satisfy the declared schema")]
pub struct ConfigurationResolutionError {
    pub path: String,
    pub code: ConfigurationResolutionCode,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationResolutionCode {
    Invalid,
    Required,
    AdditionalProperties,
}

fn failure(path: &str, code: ConfigurationResolutionCode) -> ConfigurationResolutionError {
    ConfigurationResolutionError {
        path: path.to_owned(),
        code,
    }
}

fn child_path(path: &str, field: &str) -> String {
    let field = field.replace('~', "~0").replace('/', "~1");
    let child = format!("{path}/{field}");
    if child.len() <= 512 && !child.chars().any(char::is_control) {
        child
    } else {
        path.to_owned()
    }
}

/// Supplies declared fixed values and defaults without changing the caller's input.
///
/// # Errors
/// Rejects explicit fixed-value conflicts and malformed configuration shapes.
pub fn resolve_configuration(
    schema: &Value,
    supplied: &Value,
) -> Result<Value, ConfigurationResolutionError> {
    validate_schema(schema, 0)?;
    resolve(schema, supplied, 0, "/configuration")
}

fn validate_schema(schema: &Value, depth: usize) -> Result<(), ConfigurationResolutionError> {
    if depth > 32 {
        return Err(failure(
            "/configuration",
            ConfigurationResolutionCode::Invalid,
        ));
    }
    let schema = schema
        .as_object()
        .ok_or_else(|| failure("/configuration", ConfigurationResolutionCode::Invalid))?;
    for keyword in schema.keys() {
        if !matches!(
            keyword.as_str(),
            "type"
                | "const"
                | "default"
                | "enum"
                | "properties"
                | "required"
                | "additionalProperties"
                | "items"
                | "minItems"
                | "maxItems"
                | "minimum"
                | "maximum"
                | "minLength"
                | "maxLength"
                | "$schema"
                | "title"
                | "description"
        ) {
            return Err(failure(
                "/configuration",
                ConfigurationResolutionCode::Invalid,
            ));
        }
    }
    if !matches!(
        schema.get("type").and_then(Value::as_str),
        Some("object" | "array" | "string" | "integer" | "number" | "boolean" | "null")
    ) {
        return Err(failure(
            "/configuration",
            ConfigurationResolutionCode::Invalid,
        ));
    }
    if let Some(properties) = schema.get("properties") {
        for property in properties
            .as_object()
            .ok_or_else(|| failure("/configuration", ConfigurationResolutionCode::Invalid))?
            .values()
        {
            validate_schema(property, depth + 1)?;
        }
    }
    if let Some(items) = schema.get("items") {
        validate_schema(items, depth + 1)?;
    }
    if schema
        .get("additionalProperties")
        .is_some_and(|value| !value.is_boolean())
        || schema
            .get("enum")
            .is_some_and(|value| value.as_array().is_none_or(Vec::is_empty))
        || schema.get("required").is_some_and(|value| {
            value
                .as_array()
                .is_none_or(|items| items.iter().any(|item| !item.is_string()))
        })
        || ["minItems", "maxItems", "minLength", "maxLength"]
            .into_iter()
            .any(|key| {
                schema
                    .get(key)
                    .is_some_and(|value| value.as_u64().is_none())
            })
        || ["minimum", "maximum"]
            .into_iter()
            .any(|key| schema.get(key).is_some_and(|value| !value.is_number()))
    {
        return Err(failure(
            "/configuration",
            ConfigurationResolutionCode::Invalid,
        ));
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "Keep the bounded schema interpreter and its field-path propagation together until the completed module review"
)]
fn resolve(
    schema: &Value,
    supplied: &Value,
    depth: usize,
    path: &str,
) -> Result<Value, ConfigurationResolutionError> {
    if depth > 32 {
        return Err(failure(path, ConfigurationResolutionCode::Invalid));
    }
    let schema = schema
        .as_object()
        .ok_or_else(|| failure(path, ConfigurationResolutionCode::Invalid))?;
    if schema.get("const").is_some_and(|fixed| fixed != supplied) {
        return Err(failure(path, ConfigurationResolutionCode::Invalid));
    }
    if schema
        .get("enum")
        .and_then(Value::as_array)
        .is_some_and(|choices| !choices.contains(supplied))
    {
        return Err(failure(path, ConfigurationResolutionCode::Invalid));
    }
    if let Some(text) = supplied.as_str() {
        let length = u64::try_from(text.chars().count())
            .map_err(|_| failure(path, ConfigurationResolutionCode::Invalid))?;
        if schema
            .get("minLength")
            .and_then(Value::as_u64)
            .is_some_and(|minimum| length < minimum)
            || schema
                .get("maxLength")
                .and_then(Value::as_u64)
                .is_some_and(|maximum| length > maximum)
        {
            return Err(failure(path, ConfigurationResolutionCode::Invalid));
        }
    }
    if supplied.is_number()
        && (schema
            .get("minimum")
            .is_some_and(|minimum| number_less(supplied, minimum))
            || schema
                .get("maximum")
                .is_some_and(|maximum| number_less(maximum, supplied)))
    {
        return Err(failure(path, ConfigurationResolutionCode::Invalid));
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let mut resolved = supplied
                .as_object()
                .ok_or_else(|| failure(path, ConfigurationResolutionCode::Invalid))?
                .clone();
            if let Some(properties) = schema.get("properties") {
                for (name, property) in properties
                    .as_object()
                    .ok_or_else(|| failure(path, ConfigurationResolutionCode::Invalid))?
                {
                    let value = resolved
                        .get(name)
                        .or_else(|| property.get("const"))
                        .or_else(|| property.get("default"));
                    if let Some(value) = value {
                        resolved.insert(
                            name.clone(),
                            resolve(property, value, depth + 1, &child_path(path, name))?,
                        );
                    }
                }
            }
            if schema.get("additionalProperties") == Some(&Value::Bool(false))
                && resolved.keys().any(|name| {
                    schema
                        .get("properties")
                        .and_then(Value::as_object)
                        .is_none_or(|properties| !properties.contains_key(name))
                })
            {
                return Err(failure(
                    path,
                    ConfigurationResolutionCode::AdditionalProperties,
                ));
            }
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for name in required.iter().filter_map(Value::as_str) {
                    if !resolved.contains_key(name) {
                        return Err(failure(
                            &child_path(path, name),
                            ConfigurationResolutionCode::Required,
                        ));
                    }
                }
            }
            Ok(Value::Object(resolved))
        }
        Some("array") => {
            let values = supplied
                .as_array()
                .ok_or_else(|| failure(path, ConfigurationResolutionCode::Invalid))?;
            let count = u64::try_from(values.len())
                .map_err(|_| failure(path, ConfigurationResolutionCode::Invalid))?;
            if schema
                .get("minItems")
                .and_then(Value::as_u64)
                .is_some_and(|minimum| count < minimum)
                || schema
                    .get("maxItems")
                    .and_then(Value::as_u64)
                    .is_some_and(|maximum| count > maximum)
            {
                return Err(failure(path, ConfigurationResolutionCode::Invalid));
            }
            let items = schema
                .get("items")
                .ok_or_else(|| failure(path, ConfigurationResolutionCode::Invalid))?;
            values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    resolve(
                        items,
                        value,
                        depth + 1,
                        &child_path(path, &index.to_string()),
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        Some("string") if supplied.is_string() => Ok(supplied.clone()),
        Some("integer") if supplied.is_i64() || supplied.is_u64() => Ok(supplied.clone()),
        Some("number") if supplied.is_number() => Ok(supplied.clone()),
        Some("boolean") if supplied.is_boolean() => Ok(supplied.clone()),
        Some("null") if supplied.is_null() => Ok(Value::Null),
        _ => Err(failure(path, ConfigurationResolutionCode::Invalid)),
    }
}

fn number_less(first: &Value, second: &Value) -> bool {
    let integer = |value: &Value| {
        value
            .as_i64()
            .map(i128::from)
            .or_else(|| value.as_u64().map(i128::from))
    };
    match (integer(first), integer(second)) {
        (Some(first), Some(second)) => first < second,
        _ => first
            .as_f64()
            .zip(second.as_f64())
            .is_some_and(|(first, second)| first < second),
    }
}
