pub(super) fn redact_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => {
            *s = crate::message::redact_secrets(s);
        }
        serde_json::Value::Array(values) => {
            for entry in values {
                redact_json_value(entry);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, entry) in map.iter_mut() {
                if is_sensitive_json_key(key) {
                    *entry = serde_json::Value::String("[REDACTED_SECRET]".to_string());
                } else {
                    redact_json_value(entry);
                }
            }
        }
        _ => {}
    }
}

pub(super) fn is_sensitive_json_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    normalized.contains("apikey")
        || normalized.ends_with("token")
        || normalized.ends_with("secret")
        || normalized.contains("password")
        || matches!(
            normalized.as_str(),
            "authorization" | "cookie" | "setcookie" | "privatekey" | "clientsecret"
        )
}
