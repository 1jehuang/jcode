//! Repeat-call identity without cloning the input tree.
use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::Value;
use std::hash::{Hash, Hasher};

struct IdentityInput<'a>(&'a Value);

impl Serialize for IdentityInput<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Object(object) => {
                // Preserve the Value map's iteration order and only filter the
                // top level. Nested nulls and metadata remain identity-bearing.
                let mut map = serializer.serialize_map(None)?;
                for (key, value) in object {
                    if key != "intent"
                        && key != jcode_tool_core::ACCEPT_LARGE_OUTPUT_KEY
                        && !value.is_null()
                    {
                        map.serialize_entry(key, value)?;
                    }
                }
                map.end()
            }
            value => value.serialize(serializer),
        }
    }
}

/// `intent` is a freely reworded display label, not part of call identity.
pub(super) fn withheld_call_key(tool: &str, input: &Value) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tool.hash(&mut hasher);
    // Keep the exact legacy String Hash framing, not incremental byte hashing.
    // Value's Display serializer starts with an empty buffer. Match that
    // allocation policy, rather than to_string's eager 128-byte reservation.
    let serialized = if input.is_object() {
        let mut bytes = Vec::new();
        serde_json::to_writer(&mut bytes, &IdentityInput(input))
            .expect("serializing a JSON Value cannot fail");
        String::from_utf8(bytes).expect("JSON serialization is UTF-8")
    } else {
        input.to_string()
    };
    serialized.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
#[path = "withheld_identity_tests.rs"]
mod tests;
