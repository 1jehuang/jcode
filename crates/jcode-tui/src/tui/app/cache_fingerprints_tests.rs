use super::*;

struct Unserializable;

impl serde::Serialize for Unserializable {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("injected serialization failure"))
    }
}

#[test]
fn cache_serialization_failure_keeps_legacy_non_panicking_fallbacks() {
    assert_eq!(stable_hash_json(&Unserializable), stable_hash_str(""));
    assert_eq!(stable_json_len(&Unserializable), 0);
}

#[test]
fn cache_serialization_uses_encoded_json_length_and_hash() {
    let value = serde_json::json!({"key": [1, 2, 3]});
    let encoded = serde_json::to_string(&value).unwrap();
    assert_eq!(stable_hash_json(&value), stable_hash_str(&encoded));
    assert_eq!(stable_json_len(&value), encoded.len());
}
