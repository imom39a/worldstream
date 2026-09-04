use serde_json::{Map, Value, json};

use super::{ContractError, ResultProjectorDocument, SummaryField};

pub(super) fn interpret(
    document: &ResultProjectorDocument,
    projection: &Value,
) -> Result<Value, ContractError> {
    let object = projection.as_object().ok_or(ContractError::InvalidShape)?;
    let terminal = object
        .get(&document.program.terminal.field)
        .and_then(Value::as_str)
        .ok_or(ContractError::InvalidShape)?;
    if terminal != document.program.terminal.equals {
        return Ok(json!({"status":"not_terminal"}));
    }
    let outcome = object
        .get(&document.program.outcome_field)
        .ok_or(ContractError::InvalidShape)?;
    if outcome.is_null() {
        return Ok(json!({"status":"terminal_without_outcome"}));
    }
    let outcome = outcome.as_object().ok_or(ContractError::InvalidShape)?;
    let mut summary = Map::new();
    summary.insert(
        "schema".to_owned(),
        Value::String(document.output.schema.clone()),
    );
    for field in &document.program.summary_fields {
        let source = outcome
            .get(field.source())
            .ok_or(ContractError::InvalidShape)?;
        let value = match field {
            SummaryField::Enum { values, .. } => {
                let value = source.as_str().ok_or(ContractError::InvalidShape)?;
                if !values.iter().any(|allowed| allowed == value) {
                    return Err(ContractError::InvalidShape);
                }
                Value::String(value.to_owned())
            }
            SummaryField::NullableIdentifier { .. } if source.is_null() => Value::Null,
            SummaryField::NullableIdentifier { maximum_bytes, .. } => {
                let value = source.as_str().ok_or(ContractError::InvalidShape)?;
                validate_public_reference(value, *maximum_bytes)?;
                Value::String(value.to_owned())
            }
            SummaryField::Integer {
                minimum, maximum, ..
            } => {
                let value = source.as_u64().ok_or(ContractError::InvalidShape)?;
                if value < *minimum || value > *maximum {
                    return Err(ContractError::InvalidShape);
                }
                Value::from(value)
            }
        };
        summary.insert(field.output().to_owned(), value);
    }
    Ok(json!({"status":"summary","summary":summary}))
}

fn validate_public_reference(value: &str, maximum_bytes: usize) -> Result<(), ContractError> {
    if value.is_empty()
        || value.len() > maximum_bytes
        || !value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        Err(ContractError::InvalidShape)
    } else {
        Ok(())
    }
}
