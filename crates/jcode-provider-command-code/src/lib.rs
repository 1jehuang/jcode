//! Command Code provider wire constants and DTOs (leaf crate: no jcode deps).
//! Native endpoint is POST /alpha/generate streaming NDJSON records.

use serde::{Deserialize, Serialize};

pub const PROVIDER_KEY: &str = "command-code";

pub const BASE_URL: &str = "https://api.commandcode.ai";
pub const GENERATE_URL: &str = "https://api.commandcode.ai/alpha/generate";
pub const WHOAMI_URL: &str = "https://api.commandcode.ai/alpha/whoami";
pub const MODELS_URL: &str = "https://api.commandcode.ai/provider/v1/models";
pub const CREDITS_URL_BASE: &str = "https://api.commandcode.ai/alpha/billing/credits";

/// CLI identification headers expected on every request (D-06).
pub const USER_AGENT: &str = "cli";
pub const COMMAND_CODE_VERSION_HEADER: &str = "x-command-code-version";
pub const COMMAND_CODE_VERSION: &str = "0.52.1";
pub const SESSION_ID_HEADER: &str = "x-session-id";

/// Browser OAuth loopback flow for explicit add/replace (D-03).
pub const OAUTH_SITE: &str = "https://commandcode.ai";
pub const OAUTH_LOOPBACK_HOST: &str = "127.0.0.1";
pub const OAUTH_LOOPBACK_PORT: u16 = 5959;
pub const OAUTH_CALLBACK_PATH: &str = "/callback";
pub const OAUTH_TIMEOUT_SECS: u64 = 120;

/// Conservative curated fallback catalog (CMDC-03): used when live discovery
/// fails/offline. Closest-first; the quality default is position 0.
pub const CURATED_MODELS: &[&str] = &[
    "zai-org/GLM-5.3",
    "deepseek/deepseek-v4-flash",
    "moonshotai/Kimi-K3",
];
pub const DEFAULT_MODEL: &str = "zai-org/GLM-5.3";

/// Canonical display aliases accepted in user input (case-insensitive alias
/// lookup; canonical model ids are case-sensitive on the wire, D-16).
pub const MODEL_ALIASES: &[(&str, &str)] = &[
    ("glm-5.3", "zai-org/GLM-5.3"),
    ("glm", "zai-org/GLM-5.3"),
    ("deepseek-v4-flash", "deepseek/deepseek-v4-flash"),
    ("deepseek", "deepseek/deepseek-v4-flash"),
    ("kimi-k3", "moonshotai/Kimi-K3"),
    ("kimi", "moonshotai/Kimi-K3"),
];

/// Curated reasoning-effort ladders per model (D-16). Unsupported values are
/// never exposed to the user.
pub const MODEL_EFFORTS: &[(&str, &[&str])] = &[
    ("zai-org/GLM-5.3", &["low", "high", "max", "ultra"]),
    ("deepseek/deepseek-v4-flash", &["high", "max", "ultra"]),
    ("moonshotai/Kimi-K3", &["low", "high", "max", "ultra"]),
];

/// Minimal streaming decode target for the Plan 01 smoke path. Full record
/// mapping (tool-call, finish-step, finish, reasoning) lands in Plan 03.
#[derive(Debug, Clone, PartialEq)]
pub enum GenerateRecord {
    /// Assistant text delta.
    TextDelta(String),
    /// Terminal/protocol error record.
    Error(String),
    /// Nothing usable in this record (malformed/unknown/blank).
    Ignored,
}

/// Discriminant used by /alpha/generate records.
pub const FIELD_TYPE: &str = "type";
pub const TYPE_TEXT_DELTA: &str = "text-delta";
pub const TYPE_ERROR: &str = "error";
pub const FIELD_TEXT: &str = "text";

/// Decode one NDJSON line into a minimal record for the smoke path. Strips an
/// optional data: prefix; blank/dropped lines become Ignored.
pub fn decode_record_line(line: &str) -> GenerateRecord {
    let trimmed = line.trim();
    let payload = trimmed
        .strip_prefix("data:")
        .map(str::trim)
        .unwrap_or(trimmed);
    if payload.is_empty() {
        return GenerateRecord::Ignored;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return GenerateRecord::Ignored;
    };
    let Some(obj) = value.as_object() else {
        return GenerateRecord::Ignored;
    };
    // ponytail: single-pass minimal decode; extend with plan-03 record map.
    match obj.get(FIELD_TYPE).and_then(serde_json::Value::as_str) {
        Some(TYPE_TEXT_DELTA) => {
            let text = obj
                .get(FIELD_TEXT)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if text.is_empty() {
                GenerateRecord::Ignored
            } else {
                GenerateRecord::TextDelta(text.to_string())
            }
        }
        Some(TYPE_ERROR) => {
            let message = obj
                .get("message")
                .or_else(|| obj.get(FIELD_TEXT))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown error");
            GenerateRecord::Error(message.to_string())
        }
        _ => GenerateRecord::Ignored,
    }
}

/// One Command Code credential record from the /alpha/whoami response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WhoamiIdentity {
    pub user: WhoamiUser,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WhoamiUser {
    pub id: String,
    #[serde(rename = "userName", default)]
    pub user_name: String,
    #[serde(rename = "orgId", default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
}

/// True when identity has a non-empty user.id and userName (D-02 gate).
pub fn whoami_identity_valid(identity: &WhoamiIdentity) -> bool {
    !identity.user.id.trim().is_empty() && !identity.user.user_name.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_minimal_records() {
        assert_eq!(
            decode_record_line(r#"{"type":"text-delta","text":"hi"}"#),
            GenerateRecord::TextDelta("hi".into())
        );
        assert_eq!(
            decode_record_line("data: {\"type\":\"text-delta\",\"text\":\"yo\"}"),
            GenerateRecord::TextDelta("yo".into())
        );
        assert_eq!(
            decode_record_line(r#"{"type":"error","message":"boom"}"#),
            GenerateRecord::Error("boom".into())
        );
        assert_eq!(decode_record_line(""), GenerateRecord::Ignored);
        assert_eq!(decode_record_line("not json"), GenerateRecord::Ignored);
        assert_eq!(decode_record_line("[1,2]"), GenerateRecord::Ignored);
        assert_eq!(decode_record_line("null"), GenerateRecord::Ignored);
        assert_eq!(
            decode_record_line(r#"{"type":"unknown-kind"}"#),
            GenerateRecord::Ignored
        );
    }

    #[test]
    fn whoami_gate_requires_both_fields() {
        let ok: WhoamiIdentity = serde_json::from_value(serde_json::json!({
            "user": {"id": "u1", "userName": "ada", "orgId": "o1"}
        }))
        .unwrap();
        assert!(whoami_identity_valid(&ok));
        let bad: WhoamiIdentity = serde_json::from_value(serde_json::json!({
            "user": {"id": "", "userName": "ada"}
        }))
        .unwrap();
        assert!(!whoami_identity_valid(&bad));
    }
}
