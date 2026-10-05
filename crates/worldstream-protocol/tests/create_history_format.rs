use serde_json::{Value, json};
use worldstream_protocol::{
    CanonicalHistoryFormatV1, CreateRoomRequest, CreateRoomRequestWithFormat,
};

fn legacy() -> Value {
    json!({"pack":{"id":"counter","version":"1","digest":"blake3:fixture"},"configuration":{},"members":[],"idempotency_key":"same-creation"})
}

#[test]
fn additive_creation_preserves_legacy_carriage_and_normalizes_v1() -> Result<(), serde_json::Error>
{
    let old: CreateRoomRequest = serde_json::from_value(legacy())?;
    let omitted: CreateRoomRequestWithFormat = serde_json::from_value(legacy())?;
    let mut explicit = legacy();
    explicit["canonical_history_format"] = json!("worldstream/transition/v1");
    let explicit: CreateRoomRequestWithFormat = serde_json::from_value(explicit)?;
    assert_eq!(omitted, explicit);
    assert_eq!(
        omitted.canonical_history_format,
        CanonicalHistoryFormatV1::V1
    );
    assert_eq!(serde_json::to_vec(&old)?, serde_json::to_vec(&explicit)?);
    assert_eq!(explicit.request.idempotency_key, "same-creation");
    let mut compact = legacy();
    compact["canonical_history_format"] = json!("worldstream/transition/v2");
    let selected: CreateRoomRequestWithFormat = serde_json::from_value(compact.clone())?;
    assert_eq!(
        selected.canonical_history_format,
        CanonicalHistoryFormatV1::V2
    );
    assert_eq!(serde_json::to_value(&selected)?, compact);
    assert!(serde_json::from_value::<CreateRoomRequest>(compact).is_err());
    Ok(())
}

#[test]
fn creation_selector_is_closed_and_never_nullable() {
    for selector in [
        Value::Null,
        json!("v2"),
        json!("worldstream/transition/v3"),
        json!(0),
        json!(true),
        json!([]),
        json!({}),
    ] {
        let mut request = legacy();
        request["canonical_history_format"] = selector;
        assert!(serde_json::from_value::<CreateRoomRequestWithFormat>(request).is_err());
    }
    let mut request = legacy();
    request["extra"] = json!(true);
    assert!(serde_json::from_value::<CreateRoomRequestWithFormat>(request).is_err());
    let raw = r#"{"pack":{"id":"a","version":"1","digest":"x"},"configuration":{},"members":[],"idempotency_key":"x","canonical_history_format":"worldstream/transition/v1","canonical_history_format":"worldstream/transition/v2"}"#;
    assert!(serde_json::from_str::<CreateRoomRequestWithFormat>(raw).is_err());
}
