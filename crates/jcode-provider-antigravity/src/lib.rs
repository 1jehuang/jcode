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
/// Default Cloud Code base endpoint used for Antigravity requests.
///
/// The official Antigravity IDE talks to `daily-cloudcode-pa.googleapis.com`,
/// not `cloudcode-pa.googleapis.com`. Google Front End routes by `Host`
/// header, and for consumer (`@gmail.com`) accounts allocated to the
/// `aicode-consumers` project, the non-`daily` host rejects otherwise-valid
/// requests with `HTTP 429 RESOURCE_EXHAUSTED`, even though the identical
/// token succeeds against `daily-cloudcode-pa.googleapis.com`. See
/// <https://github.com/1jehuang/jcode/issues/1329>.
pub const DEFAULT_ENDPOINT: &str = "https://daily-cloudcode-pa.googleapis.com";
/// Environment variable that overrides [`DEFAULT_ENDPOINT`], for accounts or
/// environments that need a different Cloud Code host.
pub const ENDPOINT_ENV: &str = "JCODE_ANTIGRAVITY_ENDPOINT";
const VERSION_ENV: &str = "JCODE_ANTIGRAVITY_VERSION";
pub const ANTIGRAVITY_VERSION: &str = "1.18.3";
pub const X_GOOG_API_CLIENT: &str = "google-cloud-sdk vscode_cloudshelleditor/0.1";
const CATALOG_REFRESH_TTL_HOURS: i64 = 6;

/// Resolve the Cloud Code base endpoint, honoring [`ENDPOINT_ENV`] and
/// otherwise defaulting to [`DEFAULT_ENDPOINT`].
///
/// A configured override that is not a well-formed `http(s)://host` base URL
/// (no query string, no fragment, no trailing RPC path) falls back to
/// [`DEFAULT_ENDPOINT`] rather than silently producing broken RPC URLs: a
/// query-bearing base would send the `:fetchAvailableModels` /
/// `:generateContent` suffix as query text instead of path, and a scheme-less
/// host would fail request construction with an opaque `reqwest` builder
/// error. See [`is_valid_endpoint_base`].
pub fn antigravity_endpoint() -> String {
    std::env::var(ENDPOINT_ENV)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .filter(|value| is_valid_endpoint_base(value))
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string())
}

/// Whether `value` is a plausible `http(s)://host` Cloud Code base URL: an
/// absolute HTTP(S) URL with no query string or fragment. This is a
/// deliberately narrow, dependency-free check (not full RFC 3986 parsing)
/// meant only to catch the override mistakes that would otherwise silently
/// break RPC URL construction in [`fetch_models_api_url`] and
/// [`generate_content_api_url`].
fn is_valid_endpoint_base(value: &str) -> bool {
    let Some(rest) = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
    else {
        return false;
    };
    !rest.is_empty() && !value.contains(['?', '#'])
}

/// Full URL for the `fetchAvailableModels` RPC against the resolved endpoint.
pub fn fetch_models_api_url() -> String {
    format!("{}/v1internal:fetchAvailableModels", antigravity_endpoint())
}

/// Full URL for the `generateContent` RPC against the resolved endpoint.
pub fn generate_content_api_url() -> String {
    format!("{}/v1internal:generateContent", antigravity_endpoint())
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PersistedCatalog {
    pub models: Vec<CatalogModel>,
    pub fetched_at_rfc3339: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model_id: Option<String>,
    /// Cloud Code base endpoint (see [`antigravity_endpoint`]) the catalog was
    /// fetched from. `None` means the cache predates this field (written by an
    /// older jcode version) and its endpoint is unknown.
    ///
    /// A model catalog is endpoint-specific: model availability, ids, and
    /// quotas can differ between Cloud Code hosts (e.g. `cloudcode-pa` vs
    /// `daily-cloudcode-pa`). A cache written against one endpoint must not be
    /// trusted after the resolved endpoint changes (default flip, or a new
    /// `JCODE_ANTIGRAVITY_ENDPOINT` value), or a user could be offered a model
    /// id that the new endpoint doesn't actually serve. See
    /// [`catalog_matches_current_endpoint`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

/// Whether a persisted catalog was fetched from the endpoint currently
/// resolved by [`antigravity_endpoint`]. A cache with no recorded endpoint
/// (written before this field existed) is treated as matching, so existing
/// caches are not invalidated by an in-place upgrade; but once repersisted, a
/// stamped cache will be checked against future endpoint changes. Callers
/// should discard the cache and force a fresh fetch when this returns `false`.
pub fn catalog_matches_current_endpoint(catalog: &PersistedCatalog) -> bool {
    match catalog.endpoint.as_deref() {
        Some(endpoint) => endpoint == antigravity_endpoint(),
        None => true,
    }
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

/// Detect the recovery marker used when unsigned historical tool calls have to
/// be downgraded to text. Gemini can imitate that marker as a new assistant
/// response, which looks like a tool call but is not safe to execute.
pub fn is_pseudo_tool_call_turn(response: &CodeAssistGenerateResponse) -> bool {
    response
        .response
        .as_ref()
        .and_then(|response| response.candidates.as_ref())
        .and_then(|candidates| candidates.first())
        .and_then(|candidate| candidate.content.as_ref())
        .is_some_and(|content| {
            content.parts.iter().any(|part| {
                let Some(text) = part.text.as_deref() else {
                    return false;
                };
                let Some(call) = text.trim_start().strip_prefix("[previous tool call]") else {
                    return false;
                };
                let call = call.trim_start();
                let name_len = call
                    .chars()
                    .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
                    .map(char::len_utf8)
                    .sum::<usize>();
                name_len > 0 && call[name_len..].trim_start().starts_with('(')
            })
        })
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
pub fn model_is_gemini(model: &str) -> bool {
    model.trim().to_ascii_lowercase().starts_with("gemini")
}

/// The schema dialect the Antigravity backend will validate a request against,
/// which depends on which upstream the resolved model is routed to.
///
/// The Cloud Code backend multiplexes several upstreams behind one
/// `generateContent` endpoint, and each validates tool schemas differently:
///
/// - **Gemini** (native): an OpenAPI 3.0 subset. Rejects draft keywords such as
///   `propertyNames` (#754) and `required` naming an undeclared property (#655).
/// - **Claude** (Gemini->Anthropic translation): rejects combiners
///   (`anyOf`/`oneOf`/`allOf`) at any depth with HTTP 400 "must match JSON
///   Schema draft 2020-12".
/// - **gpt-oss / other OpenAI-compatible bridges**: round-trip numeric bounds
///   through a protobuf `int64`, which proto3 JSON re-encodes as a string, then
///   reject the string ("'10' is not of type 'integer'").
pub fn antigravity_dialect(model: &str) -> &'static jcode_schema_dialect::DialectSpec {
    if model_is_gemini(model) {
        &jcode_schema_dialect::registry::GEMINI
    } else if model_is_claude(model) {
        &jcode_schema_dialect::registry::ANTIGRAVITY_CLAUDE
    } else {
        &jcode_schema_dialect::registry::ANTIGRAVITY_BRIDGE
    }
}

/// Normalize a tool-parameter JSON schema for the Antigravity backend path the
/// resolved model uses. See [`antigravity_dialect`] for the per-upstream rules
/// and `jcode-schema-dialect` for why the subsets are allow-lists.
pub fn antigravity_compatible_schema(schema: &Value, model: &str) -> Value {
    jcode_schema_dialect::normalize(schema, antigravity_dialect(model))
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

#[cfg(test)]
mod endpoint_tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize env-var mutation across tests in this module: `std::env` is
    // process-global, and cargo runs tests in this crate on multiple threads.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn antigravity_endpoint_defaults_to_daily_host() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var(ENDPOINT_ENV);
        }
        assert_eq!(antigravity_endpoint(), DEFAULT_ENDPOINT);
        assert_eq!(
            fetch_models_api_url(),
            format!("{DEFAULT_ENDPOINT}/v1internal:fetchAvailableModels")
        );
        assert_eq!(
            generate_content_api_url(),
            format!("{DEFAULT_ENDPOINT}/v1internal:generateContent")
        );
    }

    #[test]
    fn antigravity_endpoint_honors_env_override_and_trims_trailing_slash() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var(ENDPOINT_ENV, " https://example.googleapis.com/ ");
        }
        assert_eq!(antigravity_endpoint(), "https://example.googleapis.com");
        assert_eq!(
            fetch_models_api_url(),
            "https://example.googleapis.com/v1internal:fetchAvailableModels"
        );
        unsafe {
            std::env::remove_var(ENDPOINT_ENV);
        }
    }

    #[test]
    fn antigravity_endpoint_ignores_blank_env_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var(ENDPOINT_ENV, "   ");
        }
        assert_eq!(antigravity_endpoint(), DEFAULT_ENDPOINT);
        unsafe {
            std::env::remove_var(ENDPOINT_ENV);
        }
    }

    #[test]
    fn antigravity_endpoint_falls_back_on_scheme_less_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var(ENDPOINT_ENV, "example.googleapis.com");
        }
        assert_eq!(antigravity_endpoint(), DEFAULT_ENDPOINT);
        unsafe {
            std::env::remove_var(ENDPOINT_ENV);
        }
    }

    #[test]
    fn antigravity_endpoint_falls_back_on_override_with_query_string() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var(
                ENDPOINT_ENV,
                "https://example.googleapis.com?not=a-base-url",
            );
        }
        assert_eq!(antigravity_endpoint(), DEFAULT_ENDPOINT);
        unsafe {
            std::env::remove_var(ENDPOINT_ENV);
        }
    }

    #[test]
    fn antigravity_endpoint_falls_back_on_override_with_fragment() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var(ENDPOINT_ENV, "https://example.googleapis.com#frag");
        }
        assert_eq!(antigravity_endpoint(), DEFAULT_ENDPOINT);
        unsafe {
            std::env::remove_var(ENDPOINT_ENV);
        }
    }

    #[test]
    fn antigravity_endpoint_accepts_valid_http_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var(ENDPOINT_ENV, "http://localhost:8080");
        }
        assert_eq!(antigravity_endpoint(), "http://localhost:8080");
        unsafe {
            std::env::remove_var(ENDPOINT_ENV);
        }
    }

    fn sample_catalog(endpoint: Option<&str>) -> PersistedCatalog {
        PersistedCatalog {
            models: vec![CatalogModel {
                id: "gemini-3-flash".to_string(),
                display_name: None,
                reset_time: None,
                tag_title: None,
                model_provider: None,
                max_tokens: None,
                max_output_tokens: None,
                recommended: false,
                available: true,
                remaining_fraction_milli: None,
            }],
            fetched_at_rfc3339: "2026-01-01T00:00:00Z".to_string(),
            default_model_id: None,
            endpoint: endpoint.map(str::to_string),
        }
    }

    #[test]
    fn catalog_with_no_recorded_endpoint_matches_any_current_endpoint() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var(ENDPOINT_ENV);
        }
        assert!(catalog_matches_current_endpoint(&sample_catalog(None)));
    }

    #[test]
    fn catalog_stamped_with_current_endpoint_matches() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var(ENDPOINT_ENV);
        }
        assert!(catalog_matches_current_endpoint(&sample_catalog(Some(
            DEFAULT_ENDPOINT
        ))));
    }

    #[test]
    fn catalog_stamped_with_stale_endpoint_does_not_match_after_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var(ENDPOINT_ENV, "https://example.googleapis.com");
        }
        // Cached from the old default before the override was set.
        assert!(!catalog_matches_current_endpoint(&sample_catalog(Some(
            DEFAULT_ENDPOINT
        ))));
        unsafe {
            std::env::remove_var(ENDPOINT_ENV);
        }
    }
}
