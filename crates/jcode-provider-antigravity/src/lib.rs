use chrono::{DateTime, Utc};
use jcode_provider_gemini::CodeAssistGenerateResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Known-good model id used when the backend default is unknown. The literal
/// alias `"default"` is rejected by `generateContent` with HTTP 404, so we must
/// always resolve it to a real model id before issuing a request.
pub const DEFAULT_FALLBACK_MODEL: &str = "gemini-3-flash";
pub const AVAILABLE_MODELS: &[&str] = &[
    "claude-opus-4-6-thinking",
    "claude-sonnet-4-6",
    "gemini-3.1-pro-high",
    "gemini-3.1-pro-low",
    "gemini-3-flash",
    "gemini-3-flash-agent",
    "gemini-3.5-flash-low",
    "gpt-oss-120b-medium",
];
pub const FETCH_MODELS_API_URL: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels";
pub const GENERATE_CONTENT_API_URL: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:generateContent";
const VERSION_ENV: &str = "JCODE_ANTIGRAVITY_VERSION";
pub const ANTIGRAVITY_VERSION: &str = "1.18.3";
pub const X_GOOG_API_CLIENT: &str = "google-cloud-sdk vscode_cloudshelleditor/0.1";
const CATALOG_REFRESH_TTL_HOURS: i64 = 6;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PersistedCatalog {
    pub models: Vec<CatalogModel>,
    pub fetched_at_rfc3339: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model_id: Option<String>,
}

/// Result of parsing the backend `fetchAvailableModels` response: the ordered
/// catalog plus the backend-advertised default agent model id. The alias
/// `"default"` is not a real model id, so the resolved backend default is what
/// inference must actually send.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CatalogSnapshot {
    pub models: Vec<CatalogModel>,
    pub default_model_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct CatalogModel {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default)]
    pub recommended: bool,
    #[serde(default)]
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_fraction_milli: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchAvailableModelsResponse {
    #[serde(default)]
    models: HashMap<String, FetchAvailableModelEntry>,
    #[serde(default)]
    default_agent_model_id: Option<String>,
    #[serde(default)]
    command_model_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchAvailableModelEntry {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    model_name: Option<String>,
    #[serde(default)]
    quota_info: Option<FetchAvailableQuotaInfo>,
    #[serde(default)]
    pub recommended: bool,
    #[serde(default)]
    pub tag_title: Option<String>,
    #[serde(default)]
    pub model_provider: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchAvailableQuotaInfo {
    #[serde(default)]
    remaining_fraction: Option<f64>,
    #[serde(default)]
    pub reset_time: Option<String>,
}

pub fn metadata_platform() -> &'static str {
    // The Cloud Code backend currently rejects OS-specific string enum values
    // such as MACOS, WINDOWS, and LINUX for ClientMetadata.Platform. Use the
    // string value that is accepted across platforms instead of varying by OS.
    "PLATFORM_UNSPECIFIED"
}

pub fn antigravity_version() -> String {
    std::env::var(VERSION_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| ANTIGRAVITY_VERSION.to_string())
}

pub fn antigravity_user_agent() -> String {
    if cfg!(target_os = "windows") {
        format!("antigravity/{} windows/amd64", antigravity_version())
    } else if cfg!(target_arch = "aarch64") {
        format!("antigravity/{} darwin/arm64", antigravity_version())
    } else {
        format!("antigravity/{} darwin/amd64", antigravity_version())
    }
}

pub fn client_metadata_header() -> String {
    format!(
        "{{\"ideType\":\"ANTIGRAVITY\",\"platform\":\"{}\",\"pluginType\":\"GEMINI\"}}",
        metadata_platform()
    )
}

fn remaining_fraction_to_milli(value: Option<f64>) -> Option<u16> {
    let value = value?;
    if !value.is_finite() {
        return None;
    }
    let clamped = value.clamp(0.0, 1.0);
    Some((clamped * 1000.0).round() as u16)
}

pub fn merge_antigravity_model_ids(models: impl IntoIterator<Item = String>) -> Vec<String> {
    let models: Vec<String> = models
        .into_iter()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect();

    let mut seen = HashSet::new();
    let mut preferred = Vec::new();

    for known in AVAILABLE_MODELS {
        if models.iter().any(|model| model == known) && seen.insert((*known).to_string()) {
            preferred.push((*known).to_string());
        }
    }

    let mut extras: Vec<String> = models
        .into_iter()
        .filter(|model| seen.insert(model.clone()))
        .collect();
    extras.sort();
    preferred.extend(extras);
    preferred
}

pub fn is_known_model(model: &str) -> bool {
    let normalized = model.trim();
    !normalized.is_empty() && AVAILABLE_MODELS.contains(&normalized)
}

pub fn parse_fetch_available_models_response(
    response: &FetchAvailableModelsResponse,
) -> CatalogSnapshot {
    let default_model_id = response
        .default_agent_model_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);

    let mut preferred_ids = Vec::new();
    if let Some(default_agent_model_id) = response.default_agent_model_id.as_deref() {
        preferred_ids.push(default_agent_model_id.trim().to_string());
    }
    preferred_ids.extend(
        response
            .command_model_ids
            .iter()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty()),
    );
    preferred_ids.extend(response.models.keys().map(|id| id.trim().to_string()));

    let ordered_ids = merge_antigravity_model_ids(preferred_ids);
    let mut by_id: HashMap<String, CatalogModel> = HashMap::new();

    for (model_id, entry) in &response.models {
        let id = model_id.trim();
        if id.is_empty() {
            continue;
        }
        let available = entry
            .quota_info
            .as_ref()
            .and_then(|quota| quota.remaining_fraction)
            .map(|remaining| remaining > 0.0)
            .unwrap_or(true);
        by_id.insert(
            id.to_string(),
            CatalogModel {
                id: id.to_string(),
                display_name: entry
                    .display_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                reset_time: entry
                    .quota_info
                    .as_ref()
                    .and_then(|quota| quota.reset_time.as_deref())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                tag_title: entry
                    .tag_title
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                model_provider: entry
                    .model_provider
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                max_tokens: entry.max_tokens,
                max_output_tokens: entry.max_output_tokens,
                recommended: entry.recommended,
                available,
                remaining_fraction_milli: remaining_fraction_to_milli(
                    entry
                        .quota_info
                        .as_ref()
                        .and_then(|quota| quota.remaining_fraction),
                ),
            },
        );

        if let Some(alias) = entry.model_name.as_deref().map(str::trim)
            && !alias.is_empty()
            && alias != id
        {
            by_id
                .entry(alias.to_string())
                .or_insert_with(|| CatalogModel {
                    id: alias.to_string(),
                    display_name: entry
                        .display_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string),
                    reset_time: entry
                        .quota_info
                        .as_ref()
                        .and_then(|quota| quota.reset_time.as_deref())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string),
                    tag_title: entry
                        .tag_title
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string),
                    model_provider: entry
                        .model_provider
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string),
                    max_tokens: entry.max_tokens,
                    max_output_tokens: entry.max_output_tokens,
                    recommended: entry.recommended,
                    available,
                    remaining_fraction_milli: remaining_fraction_to_milli(
                        entry
                            .quota_info
                            .as_ref()
                            .and_then(|quota| quota.remaining_fraction),
                    ),
                });
        }
    }

    let mut models: Vec<CatalogModel> = ordered_ids
        .into_iter()
        .map(|id| {
            by_id.remove(&id).unwrap_or(CatalogModel {
                id,
                display_name: None,
                reset_time: None,
                tag_title: None,
                model_provider: None,
                max_tokens: None,
                max_output_tokens: None,
                recommended: false,
                available: true,
                remaining_fraction_milli: None,
            })
        })
        .collect();
    models.sort_by_key(|model| !model.available);
    CatalogSnapshot {
        models,
        default_model_id,
    }
}

pub fn catalog_model_detail(model: &CatalogModel) -> String {
    let mut parts = Vec::new();
    if let Some(display_name) = model.display_name.as_deref()
        && display_name != model.id
    {
        parts.push(display_name.to_string());
    }
    if model.recommended {
        parts.push("recommended".to_string());
    }
    if let Some(tag_title) = model.tag_title.as_deref() {
        parts.push(tag_title.to_string());
    }
    if let Some(model_provider) = model.model_provider.as_deref() {
        parts.push(model_provider.to_ascii_lowercase());
    }
    if let Some(remaining) = model.remaining_fraction_milli {
        let percent = remaining as f64 / 10.0;
        parts.push(format!("quota {:.1}%", percent));
    }
    if let Some(reset_time) = model.reset_time.as_deref() {
        parts.push(format!("resets {}", reset_time));
    }
    parts.join(" · ")
}

pub fn catalog_is_stale(fetched_at_rfc3339: &str) -> bool {
    let Ok(fetched_at) = DateTime::parse_from_rfc3339(fetched_at_rfc3339) else {
        return true;
    };
    Utc::now()
        .signed_duration_since(fetched_at.with_timezone(&Utc))
        .num_hours()
        >= CATALOG_REFRESH_TTL_HOURS
}

/// Whether a resolved Antigravity model id targets an Anthropic Claude model.
pub fn model_is_claude(model: &str) -> bool {
    model.trim().to_ascii_lowercase().contains("claude")
}

/// Whether a `generateContent` response is an abnormal turn that produced no
/// usable output (no text, no function call). This is the shape Gemini-3
/// "thinking" models intermittently return when they emit Python-style
/// pseudo-code instead of a clean functionCall: `finish_reason ==
/// MALFORMED_FUNCTION_CALL` (or another non-terminal reason) with empty content.
/// Such a turn is worth one transparent retry before surfacing an error.
///
/// Normal terminal reasons (`STOP`, `MAX_TOKENS`, unspecified) are never treated
/// as retryable here, even with empty content, so a legitimately empty answer is
/// not retried in a loop.
pub fn is_retryable_empty_turn(response: &CodeAssistGenerateResponse) -> bool {
    let Some(candidate) = response
        .response
        .as_ref()
        .and_then(|r| r.candidates.as_ref())
        .and_then(|c| c.first())
    else {
        // No candidate at all is handled separately (hard error), not retried here.
        return false;
    };
    let produced_output = candidate
        .content
        .as_ref()
        .map(|content| {
            content.parts.iter().any(|part| {
                part.function_call.is_some()
                    || part.text.as_deref().is_some_and(|text| !text.is_empty())
            })
        })
        .unwrap_or(false);
    if produced_output {
        return false;
    }
    candidate
        .finish_reason
        .as_deref()
        .map(|reason| {
            !matches!(
                reason.to_ascii_uppercase().as_str(),
                "STOP" | "MAX_TOKENS" | "FINISH_REASON_UNSPECIFIED" | ""
            )
        })
        .unwrap_or(false)
}

/// Remap model ids that the Antigravity catalog advertises but the
/// `generateContent`/`streamGenerateContent` backend cannot actually service,
/// onto an equivalent id that works.
///
/// `gemini-3.1-pro-high` is advertised as `available` and is a *recognized* id
/// (a typo'd id returns HTTP 404, but this one returns HTTP 400), yet every
/// request for it is rejected with a detail-less HTTP 400 "Request contains an
/// invalid argument" on both the unary and streaming endpoints, across all
/// client versions, with or without tools, and regardless of `generationConfig`
/// / `thinkingConfig`. The sibling `gemini-3.1-pro-low` accepts byte-identical
/// requests and succeeds, and `gemini-pro-agent` advertises the *same* display
/// name ("Gemini 3.1 Pro (High)"), provider, and token limits while accepting
/// the same requests, so it is the working route to the High Pro model. Map the
/// broken id onto it so users who pick "Gemini 3.1 Pro (High)" get a working
/// model instead of a hard 400.
pub fn remap_unsupported_model(model: &str) -> &str {
    match model {
        "gemini-3.1-pro-high" => "gemini-pro-agent",
        other => other,
    }
}

/// Whether a resolved Antigravity model id targets a Gemini model.
///
/// Gemini is the backend's native path and accepts every JSON Schema construct
/// jcode emits, so no schema rewriting is needed for these models.
pub fn model_is_gemini(model: &str) -> bool {
    model.trim().to_ascii_lowercase().starts_with("gemini")
}

/// Normalize a tool-parameter JSON schema for the Antigravity backend path that
/// the resolved model uses.
///
/// The Antigravity Cloud Code backend multiplexes several upstreams behind one
/// `generateContent` endpoint, and each upstream validates tool schemas
/// differently. jcode's emitted schemas are valid JSON Schema draft 2020-12
/// (verified against the metaschema), but two upstreams reject specific
/// constructs after their own re-translation:
///
/// - **Claude** (Gemini->Anthropic translation): rejects combiners
///   (`anyOf`/`oneOf`/`allOf`) with HTTP 400 "must match JSON Schema draft
///   2020-12". We collapse each combiner to its first branch.
/// - **gpt-oss / other OpenAI-compatible bridges**: round-trip numeric schema
///   bounds through a protobuf `int64`, which proto3 JSON re-encodes as a
///   string, then reject it ("'10' is not of type 'integer'"). We drop
///   `minItems`/`maxItems`/`minLength`/`maxLength`/`minProperties`/
///   `maxProperties` for these models. These are advisory bounds the model does
///   not need to satisfy a call, so dropping them is safe.
///
/// Gemini (the native path) is returned unchanged.
pub fn antigravity_compatible_schema(schema: &Value, model: &str) -> Value {
    if model_is_gemini(model) {
        return schema.clone();
    }
    if model_is_claude(model) {
        return flatten_schema_combiners(schema);
    }
    // Non-Gemini, non-Claude models (e.g. gpt-oss) reach an OpenAI-compatible
    // bridge that mangles numeric bounds; also flatten combiners defensively
    // since those bridges share Anthropic's strictness about them.
    strip_numeric_schema_bounds(&flatten_schema_combiners(schema))
}

/// Numeric JSON Schema bounds an OpenAI-compatible Antigravity bridge corrupts
/// when round-tripping through a protobuf `int64` field.
const NUMERIC_SCHEMA_BOUND_KEYS: &[&str] = &[
    "minItems",
    "maxItems",
    "minLength",
    "maxLength",
    "minProperties",
    "maxProperties",
];

/// Recursively drop [`NUMERIC_SCHEMA_BOUND_KEYS`] from a schema. See
/// `antigravity_compatible_schema` for why this is needed.
pub fn strip_numeric_schema_bounds(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                if NUMERIC_SCHEMA_BOUND_KEYS.contains(&key.as_str()) {
                    continue;
                }
                out.insert(key.clone(), strip_numeric_schema_bounds(value));
            }
            Value::Object(out)
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(strip_numeric_schema_bounds).collect())
        }
        _ => schema.clone(),
    }
}

/// Collapse JSON Schema combiners (`anyOf`/`oneOf`/`allOf`) to their first
/// branch throughout a tool-parameter schema.
///
/// The Antigravity Cloud Code backend forwards Claude tool calls through a
/// Gemini->Anthropic schema translation that rejects these combiners with
/// HTTP 400 ("input_schema: JSON schema is invalid. It must match JSON Schema
/// draft 2020-12"). Collapsing to the first branch preserves a usable, valid
/// schema (e.g. `anyOf: [string, array<string>]` becomes `string`) so the tool
/// call is accepted; the agent simply uses the primary branch's shape.
pub fn flatten_schema_combiners(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            for combiner in ["anyOf", "oneOf", "allOf"] {
                if let Some(Value::Array(branches)) = map.get(combiner)
                    && let Some(first) = branches.first()
                {
                    // Merge sibling keys (e.g. `description`) onto the chosen
                    // branch so we don't lose prompt-visible metadata.
                    let mut flattened = match flatten_schema_combiners(first) {
                        Value::Object(branch_map) => branch_map,
                        other => return other,
                    };
                    for (key, value) in map {
                        if key == combiner {
                            continue;
                        }
                        flattened
                            .entry(key.clone())
                            .or_insert_with(|| flatten_schema_combiners(value));
                    }
                    return Value::Object(flattened);
                }
            }
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                out.insert(key.clone(), flatten_schema_combiners(value));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(flatten_schema_combiners).collect()),
        _ => schema.clone(),
    }
}
