use super::discover_secrets::contains_recognizable_secret;
use super::{Tool, ToolContext, ToolExecutionMode, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt;
use std::time::Duration;
use std::time::Instant;

/// Hard timeout for discovery requests. Discovery is optional by design: if
/// the endpoint is slow or unreachable the tool fails plainly and the agent
/// continues with its normal toolset. No cache, no offline fallback, no retry.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const DISCOVERY_REQUEST_ID_HEADER: &str = "x-jcode-discovery-request-id";
const DISCOVERY_CORRELATION_ID_HEADER: &str = "x-jcode-session-correlation-id";
const DISCOVERY_BENCHMARK_HEADER: &str = "x-jcode-discovery-benchmark";
const DISCOVERY_SESSION_ID_HEADER: &str = "x-jcode-discovery-session-id";
const DISCOVERY_SESSION_METADATA_HEADER: &str = "x-jcode-discovery-session-metadata";
const DISCOVERY_SELF_DEV_HEADER: &str = "x-jcode-discovery-self-dev";
const DISCOVERY_DEBUG_HEADER: &str = "x-jcode-discovery-debug";
const DISCOVERY_CANARY_HEADER: &str = "x-jcode-discovery-canary";
const DISCOVERY_EXECUTION_MODE_HEADER: &str = "x-jcode-discovery-execution-mode";
const DISCOVERY_BUILD_CHANNEL_HEADER: &str = "x-jcode-discovery-build-channel";
const DISCOVERY_GIT_CHECKOUT_HEADER: &str = "x-jcode-discovery-git-checkout";
const DISCOVERY_CI_HEADER: &str = "x-jcode-discovery-ci";
const DISCOVERY_RAN_FROM_CARGO_HEADER: &str = "x-jcode-discovery-ran-from-cargo";
const DISCOVERY_BENCHMARK_ENV: &str = "JCODE_DISCOVERY_BENCHMARK";
const DISCOVERY_QUERY_MIN_CHARS: usize = 20;
const DISCOVERY_QUERY_MAX_CHARS: usize = 500;
const DISCOVERY_REASON_MIN_CHARS: usize = 40;
const DISCOVERY_REASON_MAX_CHARS: usize = 2_000;

/// Telemetry reason for a `select` naming an entry the catalog does not carry.
/// Kept distinct from transport failures so the rate of agents committing to
/// off-catalog products is measurable rather than hidden in `http_error`.
const OFF_CATALOG_FAILURE_REASON: &str = "off_catalog_select";

/// True when a select response carries no usable tool entry (`{}`,
/// `{"tool": null}`, or an empty object), which endpoints use instead of 404.
fn listing_has_no_tool_entry(listing: &Value) -> bool {
    // A successful off-catalog selection deliberately has no `tool` object.
    // It is still a valid receipt and must reach `render_selection` rather than
    // being mistaken for an empty catalog response.
    if listing.get("listed").and_then(Value::as_bool) == Some(false)
        && listing
            .get("selected_tool")
            .and_then(Value::as_str)
            .is_some_and(|name| !name.trim().is_empty())
    {
        return false;
    }
    match listing.get("tool") {
        None | Some(Value::Null) => true,
        Some(Value::Object(entry)) => entry.is_empty(),
        Some(_) => false,
    }
}

/// Error shown when the server cannot return a valid receipt for a selection.
/// Off-catalog choices are legitimate, but they still must be recorded before
/// the agent can claim that Discovery observed the choice.
fn selection_receipt_error(category: &str, tool_name: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "Discovery could not record the selection of '{tool_name}' for '{category}' because the \
         server returned no valid selection receipt. Retry action `select` with the same product, \
         including off-catalog products. Until a receipt is returned, do not claim the choice was \
         recorded or treat '{tool_name}' as vetted, and do not invent setup instructions from \
         memory."
    )
}

fn discovery_benchmark_run() -> bool {
    std::env::var(DISCOVERY_BENCHMARK_ENV)
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
}

#[derive(Debug)]
struct DiscoveryFetchResult {
    listing: Value,
    http_status: u16,
    response_bytes: u64,
}

#[derive(Debug)]
struct DiscoveryFetchError {
    message: String,
    failure_reason: &'static str,
    http_status: Option<u16>,
    response_bytes: Option<u64>,
}

struct DiscoveryRequestContext<'a> {
    client: &'a reqwest::Client,
    endpoint: &'a str,
    request_id: &'a str,
    category: &'a str,
    query: &'a str,
    reason: &'a str,
    benchmark_run: bool,
    provenance: DiscoveryRequestProvenance,
}

#[derive(Debug, Clone)]
struct DiscoveryRequestProvenance {
    session_id: String,
    correlation_id: Option<String>,
    session_metadata_available: bool,
    is_self_dev: bool,
    is_debug: bool,
    is_canary: bool,
    execution_mode: &'static str,
    build_channel: String,
    is_git_checkout: bool,
    is_ci: bool,
    ran_from_cargo: bool,
}

impl DiscoveryRequestProvenance {
    fn from_tool_context(ctx: &ToolContext) -> Self {
        let session = crate::session::Session::load(&ctx.session_id).ok();
        let runtime = crate::telemetry::runtime_provenance();
        Self {
            session_id: ctx.session_id.clone(),
            correlation_id: crate::telemetry::current_session_correlation_id(),
            session_metadata_available: session.is_some(),
            is_self_dev: session
                .as_ref()
                .is_some_and(|session| session.is_self_dev()),
            is_debug: session.as_ref().is_some_and(|session| session.is_debug),
            is_canary: session.as_ref().is_some_and(|session| session.is_canary),
            execution_mode: match ctx.execution_mode {
                ToolExecutionMode::AgentTurn => "agent_turn",
                ToolExecutionMode::Direct => "direct",
            },
            build_channel: runtime.build_channel,
            is_git_checkout: runtime.is_git_checkout,
            is_ci: runtime.is_ci,
            ran_from_cargo: runtime.ran_from_cargo,
        }
    }

    fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let request = request
            .header(DISCOVERY_SESSION_ID_HEADER, &self.session_id)
            .header(
                DISCOVERY_SESSION_METADATA_HEADER,
                bool_header(self.session_metadata_available),
            )
            .header(DISCOVERY_SELF_DEV_HEADER, bool_header(self.is_self_dev))
            .header(DISCOVERY_DEBUG_HEADER, bool_header(self.is_debug))
            .header(DISCOVERY_CANARY_HEADER, bool_header(self.is_canary))
            .header(DISCOVERY_EXECUTION_MODE_HEADER, self.execution_mode)
            .header(DISCOVERY_BUILD_CHANNEL_HEADER, &self.build_channel)
            .header(
                DISCOVERY_GIT_CHECKOUT_HEADER,
                bool_header(self.is_git_checkout),
            )
            .header(DISCOVERY_CI_HEADER, bool_header(self.is_ci))
            .header(
                DISCOVERY_RAN_FROM_CARGO_HEADER,
                bool_header(self.ran_from_cargo),
            );
        if let Some(correlation_id) = &self.correlation_id {
            request.header(DISCOVERY_CORRELATION_ID_HEADER, correlation_id)
        } else {
            request
        }
    }
}

fn bool_header(value: bool) -> &'static str {
    if value { "1" } else { "0" }
}

impl fmt::Display for DiscoveryFetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DiscoveryFetchError {}

#[allow(clippy::too_many_arguments)]
fn record_discovery_telemetry(
    request_id: &str,
    started_at: Instant,
    endpoint: &str,
    phase: &str,
    category: Option<&str>,
    selected_tool: Option<&str>,
    outcome: &str,
    failure_reason: Option<&str>,
    http_status: Option<u16>,
    response_bytes: Option<u64>,
    result_count: Option<u32>,
    query_present: bool,
    reason_present: bool,
) {
    crate::telemetry::record_discovery_event(crate::telemetry::DiscoveryTelemetry {
        request_id,
        phase,
        category,
        selected_tool,
        outcome,
        failure_reason,
        http_status,
        latency_ms: started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        response_bytes,
        result_count,
        query_present,
        reason_present,
        benchmark_run: discovery_benchmark_run(),
        endpoint,
    });
}

/// `discover_tools`: fetch discoverable third-party tools for a category from
/// the hosted integration directory.
///
/// Disclosure contract: some integration providers may share revenue with Jcode, but
/// commercial relationships never influence recommendations. The policy is
/// disclosed in the tool schema and at <https://jcode.sh/discovery-tools>.
/// The request carries the category, a short search query, a reason string,
/// and coarse session/build provenance used to separate likely user demand from
/// self-dev and test traffic. It never includes transcript content, file paths,
/// credentials, or user identity.
pub struct DiscoverToolsTool {
    client: reqwest::Client,
}

impl DiscoverToolsTool {
    pub fn new() -> Self {
        Self {
            client: crate::provider::shared_http_client(),
        }
    }
}

#[derive(Deserialize)]
struct DiscoverToolsInput {
    #[serde(default)]
    action: Option<String>,
    category: String,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    suggestion_kind: Option<String>,
    #[serde(default)]
    product_name: Option<String>,
    #[serde(default)]
    product_url: Option<String>,
    #[serde(default)]
    gap_evidence: Option<String>,
    #[serde(default)]
    requirements: Option<Vec<String>>,
    #[serde(default)]
    prior_request_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscoveryAction {
    Search,
    Select,
    Suggest,
}

impl DiscoveryAction {
    /// Parse the requested phase. `search`/`select` are the current names;
    /// `browse`/`setup` are accepted as aliases so transcripts, benchmark
    /// baselines, and in-flight sessions recorded under the old vocabulary
    /// keep working.
    fn parse(action: Option<&str>, has_tool: bool) -> Result<Self> {
        match action.map(str::trim).filter(|value| !value.is_empty()) {
            None => Ok(if has_tool { Self::Select } else { Self::Search }),
            Some("search" | "browse") if !has_tool => Ok(Self::Search),
            Some("select" | "setup") if has_tool => Ok(Self::Select),
            Some("suggest") if !has_tool => Ok(Self::Suggest),
            Some("search" | "browse") => Err(anyhow::anyhow!(
                "integration action 'search' cannot include `tool`; use action 'select'"
            )),
            Some("select" | "setup") => Err(anyhow::anyhow!(
                "integration action 'select' requires the chosen `tool` name"
            )),
            Some("suggest") => Err(anyhow::anyhow!(
                "integration action 'suggest' cannot include `tool`; use `product_name` for a known product"
            )),
            Some(other) => Err(anyhow::anyhow!(
                "unknown integration action '{other}'. Available: search, select, suggest"
            )),
        }
    }
}

struct ValidatedSuggestion {
    kind: String,
    product_name: Option<String>,
    product_url: Option<String>,
    gap_evidence: Option<String>,
    requirements: Vec<String>,
    prior_request_id: String,
}

#[derive(Debug)]
struct DiscoveryInputError {
    message: String,
    failure_reason: &'static str,
}

fn validate_discovery_text(
    value: Option<&str>,
    field: &'static str,
    min_chars: usize,
    max_chars: usize,
) -> std::result::Result<String, DiscoveryInputError> {
    let value = value.unwrap_or_default().trim();
    if value.is_empty() {
        return Err(DiscoveryInputError {
            message: format!(
                "discovery {field} is required; write a specific summary without private data"
            ),
            failure_reason: if field == "query" {
                "missing_query"
            } else {
                "missing_reason"
            },
        });
    }

    let chars = value.chars().count();
    if chars < min_chars {
        return Err(DiscoveryInputError {
            message: format!(
                "discovery {field} is too short; provide at least {min_chars} characters of specific, non-private context"
            ),
            failure_reason: if field == "query" {
                "query_too_short"
            } else {
                "reason_too_short"
            },
        });
    }
    if chars > max_chars {
        return Err(DiscoveryInputError {
            message: format!(
                "discovery {field} is too long; summarize it in at most {max_chars} characters without private data"
            ),
            failure_reason: if field == "query" {
                "query_too_long"
            } else {
                "reason_too_long"
            },
        });
    }
    if contains_recognizable_secret(value) {
        return Err(DiscoveryInputError {
            message: format!(
                "discovery {field} appears to contain a secret or financial credential; replace it with a non-sensitive description"
            ),
            failure_reason: if field == "query" {
                "query_sensitive_data"
            } else {
                "reason_sensitive_data"
            },
        });
    }
    if !has_sufficient_detail(value, field) {
        return Err(DiscoveryInputError {
            message: format!(
                "discovery {field} is not specific enough; describe the capability and task constraints in distinct words without private data"
            ),
            failure_reason: if field == "query" {
                "query_not_specific"
            } else {
                "reason_not_specific"
            },
        });
    }
    Ok(value.to_string())
}

fn has_sufficient_detail(value: &str, field: &str) -> bool {
    let words: Vec<String> = value
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() >= 2)
        .map(str::to_ascii_lowercase)
        .collect();
    let mut unique = words.clone();
    unique.sort_unstable();
    unique.dedup();
    let (min_words, min_unique) = if field == "query" { (4, 3) } else { (7, 5) };
    words.len() >= min_words && unique.len() >= min_unique
}

#[async_trait]
impl Tool for DiscoverToolsTool {
    fn name(&self) -> &str {
        "integration_tools"
    }

    fn description(&self) -> &str {
        "Call this before using any product, service, dev tool, API, or data source you \
         don't already have a tool for. Tools in this catalog have integrated with this agent \
         to make setup and use seamless. If a built-in tool provides the capability, use that tool directly. \
         After deciding, call again with action `select` and the chosen product, including an \
         off-catalog product. Off-catalog selections are recorded \
         but return no provider information or setup instructions."
    }

    fn parameters_schema(&self) -> Value {
        let categories: Vec<&str> = crate::sponsors::DISCOVERY_CATEGORIES.to_vec();
        json!({
            "type": "object",
            "required": ["category", "query", "reason"],
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "enum": ["search", "select", "suggest"],
                    "description": "Phase. Defaults to select when `tool` is set, else search. For a listed result, select the one you commit to (it carries setup). Always select the product actually chosen, even when it is not in the catalog. Suggest a capability gap only when no product was chosen."
                },
                "category": {
                    "type": "string",
                    "enum": categories,
                    "description": "Missing capability category; infer it from the user's goal."
                },
                "query": {
                    "type": "string",
                    "minLength": DISCOVERY_QUERY_MIN_CHARS,
                    "maxLength": DISCOVERY_QUERY_MAX_CHARS,
                    "description": "Capability summary. May be shared with integration providers; write fresh text, never secrets or personal data."
                },
                "reason": {
                    "type": "string",
                    "minLength": DISCOVERY_REASON_MIN_CHARS,
                    "maxLength": DISCOVERY_REASON_MAX_CHARS,
                    "description": "Why the chosen integration fits, or why search results were unsuitable. Never include private data."
                },
                "tool": {
                    "type": "string",
                    "minLength": 2,
                    "maxLength": 100,
                    "description": "For select: public name of the product actually chosen. Catalog selections return setup; off-catalog selections are recorded without provider information."
                },
                "suggestion_kind": {
                    "type": "string",
                    "enum": ["known_product", "capability_gap"],
                    "description": "For suggest: known_product only when confident the public product exists, else capability_gap."
                },
                "product_name": {
                    "type": "string",
                    "minLength": 2,
                    "maxLength": 100,
                    "description": "Required only for a known_product suggestion. Public product, package, service, or MCP name."
                },
                "product_url": {
                    "type": "string",
                    "maxLength": 500,
                    "description": "Optional public HTTPS URL for a known_product suggestion. Never include credentials or private URLs."
                },
                "gap_evidence": {
                    "type": "string",
                    "maxLength": 500,
                    "description": "Which search results were close and why they did not fit. Maintainers only."
                },
                "requirements": {
                    "type": "array",
                    "maxItems": 8,
                    "items": { "type": "string", "minLength": 3, "maxLength": 240 },
                    "description": "Constraints the catalog addition should satisfy. Maintainers only."
                },
                "prior_request_id": {
                    "type": "string",
                    "description": "For suggest: the request ID returned by the preceding search in this category."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let started_at = Instant::now();
        let request_id = uuid::Uuid::new_v4().to_string();
        let config = crate::config::config();
        let endpoint = config.sponsors.endpoint.clone();
        let benchmark_run = discovery_benchmark_run();
        if !config.sponsors.enabled {
            record_discovery_telemetry(
                &request_id,
                started_at,
                &endpoint,
                "unknown",
                None,
                None,
                "failure",
                Some("disabled"),
                None,
                None,
                None,
                false,
                false,
            );
            return Err(anyhow::anyhow!(
                "integration discovery is disabled (set [sponsors] enabled = true in config.toml)"
            ));
        }

        let params: DiscoverToolsInput = match serde_json::from_value(input) {
            Ok(params) => params,
            Err(err) => {
                record_discovery_telemetry(
                    &request_id,
                    started_at,
                    &endpoint,
                    "unknown",
                    None,
                    None,
                    "failure",
                    Some("invalid_input"),
                    None,
                    None,
                    None,
                    false,
                    false,
                );
                return Err(err.into());
            }
        };
        let category = params.category.trim().to_ascii_lowercase();
        let query_present = params
            .query
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
        let reason_present = params
            .reason
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
        if !crate::sponsors::DISCOVERY_CATEGORIES.contains(&category.as_str()) {
            record_discovery_telemetry(
                &request_id,
                started_at,
                &endpoint,
                "unknown",
                None,
                None,
                "failure",
                Some("invalid_category"),
                None,
                None,
                None,
                query_present,
                reason_present,
            );
            return Err(anyhow::anyhow!(
                "unknown discovery category '{}'. Available: {}",
                category,
                crate::sponsors::DISCOVERY_CATEGORIES.join(", ")
            ));
        }

        let query = match validate_discovery_text(
            params.query.as_deref(),
            "query",
            DISCOVERY_QUERY_MIN_CHARS,
            DISCOVERY_QUERY_MAX_CHARS,
        ) {
            Ok(query) => query,
            Err(err) => {
                record_discovery_telemetry(
                    &request_id,
                    started_at,
                    &endpoint,
                    "unknown",
                    Some(&category),
                    None,
                    "failure",
                    Some(err.failure_reason),
                    None,
                    None,
                    None,
                    query_present,
                    reason_present,
                );
                return Err(anyhow::anyhow!(err.message));
            }
        };
        let reason = match validate_discovery_text(
            params.reason.as_deref(),
            "reason",
            DISCOVERY_REASON_MIN_CHARS,
            DISCOVERY_REASON_MAX_CHARS,
        ) {
            Ok(reason) => reason,
            Err(err) => {
                record_discovery_telemetry(
                    &request_id,
                    started_at,
                    &endpoint,
                    "unknown",
                    Some(&category),
                    None,
                    "failure",
                    Some(err.failure_reason),
                    None,
                    None,
                    None,
                    query_present,
                    reason_present,
                );
                return Err(anyhow::anyhow!(err.message));
            }
        };

        let tool_selection = normalize_selection_name(params.tool.as_deref())?;
        let action = DiscoveryAction::parse(params.action.as_deref(), tool_selection.is_some())?;
        let discovery_request = DiscoveryRequestContext {
            client: &self.client,
            endpoint: &endpoint,
            request_id: &request_id,
            category: &category,
            query: &query,
            reason: &reason,
            benchmark_run,
            provenance: DiscoveryRequestProvenance::from_tool_context(&ctx),
        };

        if action == DiscoveryAction::Suggest {
            let suggestion = validate_suggestion(&params)?;
            let fetched = match submit_suggestion(&discovery_request, &suggestion).await {
                Ok(result) => result,
                Err(err) => {
                    record_discovery_telemetry(
                        &request_id,
                        started_at,
                        &endpoint,
                        "suggest",
                        Some(&category),
                        None,
                        "failure",
                        Some(err.failure_reason),
                        err.http_status,
                        err.response_bytes,
                        None,
                        query_present,
                        reason_present,
                    );
                    return Err(err.into());
                }
            };
            let rendered =
                render_suggestion(&category, &query, &reason, &suggestion, &fetched.listing)?;
            record_discovery_telemetry(
                &request_id,
                started_at,
                &endpoint,
                "suggest",
                Some(&category),
                None,
                "success",
                None,
                Some(fetched.http_status),
                Some(fetched.response_bytes),
                Some(1),
                query_present,
                reason_present,
            );
            return Ok(ToolOutput::new(rendered)
                .with_title("catalog suggestion".to_string())
                .with_metadata(json!({
                    "catalog_suggestion": true,
                    "category": category,
                    "suggestion_kind": suggestion.kind,
                    "suggestion_status": fetched.listing.get("status").and_then(Value::as_str),
                })));
        }

        // Select phase: return one tool's full setup instructions. The
        // selection (and the agent's reason for it) is recorded server-side.
        if let Some(tool_name) = tool_selection {
            let fetched = match fetch_listing(&discovery_request, Some(&tool_name)).await {
                Ok(result) => result,
                Err(err) => {
                    // Older endpoints returned 404 for an off-catalog choice.
                    // Current endpoints return a structured receipt instead,
                    // so a 404 now means the choice was not recorded.
                    if err.http_status == Some(404) {
                        record_discovery_telemetry(
                            &request_id,
                            started_at,
                            &endpoint,
                            "select",
                            Some(&category),
                            Some(tool_name.as_str()),
                            "off_catalog_select",
                            Some(OFF_CATALOG_FAILURE_REASON),
                            err.http_status,
                            err.response_bytes,
                            Some(0),
                            query_present,
                            reason_present,
                        );
                        return Err(selection_receipt_error(&category, &tool_name));
                    }
                    record_discovery_telemetry(
                        &request_id,
                        started_at,
                        &endpoint,
                        "select",
                        Some(&category),
                        None,
                        "failure",
                        Some(err.failure_reason),
                        err.http_status,
                        err.response_bytes,
                        None,
                        query_present,
                        reason_present,
                    );
                    return Err(err.into());
                }
            };
            // Older endpoints may answer 200 with an empty entry. It is not a
            // valid receipt, so the agent must not claim the choice was recorded.
            if listing_has_no_tool_entry(&fetched.listing) {
                record_discovery_telemetry(
                    &request_id,
                    started_at,
                    &endpoint,
                    "select",
                    Some(&category),
                    Some(tool_name.as_str()),
                    "off_catalog_select",
                    Some(OFF_CATALOG_FAILURE_REASON),
                    Some(fetched.http_status),
                    Some(fetched.response_bytes),
                    Some(0),
                    query_present,
                    reason_present,
                );
                return Err(selection_receipt_error(&category, &tool_name));
            }
            let rendered = match render_selection(&category, &tool_name, &fetched.listing) {
                Ok(rendered) => rendered,
                Err(err) => {
                    record_discovery_telemetry(
                        &request_id,
                        started_at,
                        &endpoint,
                        "select",
                        Some(&category),
                        None,
                        "failure",
                        Some("invalid_response"),
                        Some(fetched.http_status),
                        Some(fetched.response_bytes),
                        None,
                        query_present,
                        reason_present,
                    );
                    return Err(err);
                }
            };
            let catalog_tool = fetched.listing.get("tool").is_some();
            if catalog_tool {
                crate::sponsors::provenance::record_discovered_setups(extract_mcp_setups_from(
                    fetched
                        .listing
                        .get("tool")
                        .map(std::slice::from_ref)
                        .unwrap_or(&[]),
                ));
            }
            let canonical_tool = fetched
                .listing
                .get("tool")
                .and_then(|tool| tool.get("name"))
                .and_then(Value::as_str)
                .or_else(|| fetched.listing.get("selected_tool").and_then(Value::as_str))
                .unwrap_or(&tool_name);
            record_discovery_telemetry(
                &request_id,
                started_at,
                &endpoint,
                "select",
                Some(&category),
                Some(canonical_tool),
                "success",
                None,
                Some(fetched.http_status),
                Some(fetched.response_bytes),
                Some(1),
                query_present,
                reason_present,
            );
            return Ok(ToolOutput::new(rendered)
                .with_title(tool_name.to_string())
                .with_metadata(json!({
                    "discovery_selection": true,
                    "sponsored_discovery": catalog_tool,
                    "catalog_tool": catalog_tool,
                    "category": category,
                    "selected_tool": tool_name,
                    "disclosure_url": crate::sponsors::DISCOVERY_PARTNERS_URL,
                })));
        }

        let fetched = match fetch_listing(&discovery_request, None).await {
            Ok(result) => result,
            Err(err) => {
                record_discovery_telemetry(
                    &request_id,
                    started_at,
                    &endpoint,
                    "browse",
                    Some(&category),
                    None,
                    "failure",
                    Some(err.failure_reason),
                    err.http_status,
                    err.response_bytes,
                    None,
                    query_present,
                    reason_present,
                );
                return Err(err.into());
            }
        };
        let rendered = match render_listing(&category, &fetched.listing, &request_id) {
            Ok(rendered) => rendered,
            Err(err) => {
                record_discovery_telemetry(
                    &request_id,
                    started_at,
                    &endpoint,
                    "browse",
                    Some(&category),
                    None,
                    "failure",
                    Some("invalid_response"),
                    Some(fetched.http_status),
                    Some(fetched.response_bytes),
                    None,
                    query_present,
                    reason_present,
                );
                return Err(err);
            }
        };
        let result_count = fetched
            .listing
            .get("tools")
            .and_then(Value::as_array)
            .map(|tools| tools.len().min(u32::MAX as usize) as u32);

        // Remember MCP setups from this listing so a later `mcp connect`
        // matching one of them is tagged with discovery provenance (and
        // metered coarsely; see jcode_base::sponsors::provenance).
        crate::sponsors::provenance::record_discovered_setups(extract_mcp_setups(&fetched.listing));
        record_discovery_telemetry(
            &request_id,
            started_at,
            &endpoint,
            "browse",
            Some(&category),
            None,
            "success",
            None,
            Some(fetched.http_status),
            Some(fetched.response_bytes),
            result_count,
            query_present,
            reason_present,
        );

        Ok(ToolOutput::new(rendered)
            .with_title(category.to_string())
            .with_metadata(json!({
                "sponsored_discovery": true,
                "category": category,
                "disclosure_url": crate::sponsors::DISCOVERY_PARTNERS_URL,
            })))
    }
}

/// Fetch a category listing (browse) or one tool's entry (select) from the
/// discovery endpoint. Sends the category, a required capability query, a
/// required reason string, and the selected tool name only. Hard fails on
/// any error: no cache, no fallback, no retry.
async fn fetch_listing(
    context: &DiscoveryRequestContext<'_>,
    tool: Option<&str>,
) -> std::result::Result<DiscoveryFetchResult, DiscoveryFetchError> {
    let endpoint = context.endpoint.trim_end_matches('/');
    let mut request = context.provenance.apply(
        context
            .client
            .get(endpoint)
            .query(&[
                ("category", context.category),
                ("q", context.query),
                ("reason", context.reason),
            ])
            .header(
                reqwest::header::USER_AGENT,
                format!("jcode/{}", env!("CARGO_PKG_VERSION")),
            )
            .header(DISCOVERY_REQUEST_ID_HEADER, context.request_id)
            .timeout(DISCOVERY_TIMEOUT),
    );
    if let Some(tool) = tool.filter(|t| !t.trim().is_empty()) {
        request = request.query(&[("tool", tool.trim())]);
    }
    if context.benchmark_run {
        request = request.header(DISCOVERY_BENCHMARK_HEADER, "1");
    }

    let response = request.send().await.map_err(|err| DiscoveryFetchError {
        message: format!("discovery unavailable: {err}"),
        failure_reason: if err.is_timeout() {
            "timeout"
        } else if err.is_connect() {
            "connect_error"
        } else {
            "transport_error"
        },
        http_status: None,
        response_bytes: None,
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(DiscoveryFetchError {
            message: format!("discovery unavailable: HTTP {status}"),
            failure_reason: "http_error",
            http_status: Some(status.as_u16()),
            response_bytes: response.content_length(),
        });
    }
    let body = response.bytes().await.map_err(|err| DiscoveryFetchError {
        message: format!("discovery unavailable: {err}"),
        failure_reason: "body_error",
        http_status: Some(status.as_u16()),
        response_bytes: None,
    })?;
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(DiscoveryFetchError {
            message: format!("discovery response too large ({} bytes)", body.len()),
            failure_reason: "response_too_large",
            http_status: Some(status.as_u16()),
            response_bytes: Some(body.len() as u64),
        });
    }
    let listing = serde_json::from_slice(&body).map_err(|err| DiscoveryFetchError {
        message: format!("discovery returned invalid JSON: {err}"),
        failure_reason: "invalid_json",
        http_status: Some(status.as_u16()),
        response_bytes: Some(body.len() as u64),
    })?;
    Ok(DiscoveryFetchResult {
        listing,
        http_status: status.as_u16(),
        response_bytes: body.len() as u64,
    })
}

async fn submit_suggestion(
    context: &DiscoveryRequestContext<'_>,
    suggestion: &ValidatedSuggestion,
) -> std::result::Result<DiscoveryFetchResult, DiscoveryFetchError> {
    let endpoint = format!("{}/suggestions", context.endpoint.trim_end_matches('/'));
    let mut request = context.provenance.apply(
        context
            .client
            .post(endpoint)
            .header(
                reqwest::header::USER_AGENT,
                format!("jcode/{}", env!("CARGO_PKG_VERSION")),
            )
            .header(DISCOVERY_REQUEST_ID_HEADER, context.request_id)
            .json(&json!({
                "category": context.category,
                "query": context.query,
                "reason": context.reason,
                "suggestion_kind": suggestion.kind,
                "product_name": suggestion.product_name,
                "product_url": suggestion.product_url,
                "gap_evidence": suggestion.gap_evidence,
                "requirements": suggestion.requirements,
                "prior_request_id": suggestion.prior_request_id,
            }))
            .timeout(DISCOVERY_TIMEOUT),
    );
    if context.benchmark_run {
        request = request.header(DISCOVERY_BENCHMARK_HEADER, "1");
    }
    let response = request.send().await.map_err(|err| DiscoveryFetchError {
        message: format!("catalog suggestion unavailable: {err}"),
        failure_reason: if err.is_timeout() {
            "timeout"
        } else if err.is_connect() {
            "connect_error"
        } else {
            "transport_error"
        },
        http_status: None,
        response_bytes: None,
    })?;
    let status = response.status();
    let duplicate = status == reqwest::StatusCode::CONFLICT;
    if !status.is_success() && !duplicate {
        return Err(DiscoveryFetchError {
            message: format!("catalog suggestion unavailable: HTTP {status}"),
            failure_reason: "http_error",
            http_status: Some(status.as_u16()),
            response_bytes: response.content_length(),
        });
    }
    let body = response.bytes().await.map_err(|err| DiscoveryFetchError {
        message: format!("catalog suggestion unavailable: {err}"),
        failure_reason: "body_error",
        http_status: Some(status.as_u16()),
        response_bytes: None,
    })?;
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(DiscoveryFetchError {
            message: format!(
                "catalog suggestion response too large ({} bytes)",
                body.len()
            ),
            failure_reason: "response_too_large",
            http_status: Some(status.as_u16()),
            response_bytes: Some(body.len() as u64),
        });
    }
    let mut listing: Value = serde_json::from_slice(&body).map_err(|err| DiscoveryFetchError {
        message: format!("catalog suggestion returned invalid JSON: {err}"),
        failure_reason: "invalid_json",
        http_status: Some(status.as_u16()),
        response_bytes: Some(body.len() as u64),
    })?;
    // Older catalog deployments returned a successful receipt without a
    // `status` field. HTTP success (or the explicitly accepted 409 duplicate)
    // already establishes the outcome, so normalize that compatible response
    // instead of surfacing a false tool error to the user.
    if let Some(object) = listing.as_object_mut()
        && !object.contains_key("status")
    {
        object.insert(
            "status".to_string(),
            Value::String(if duplicate { "duplicate" } else { "received" }.to_string()),
        );
    }
    Ok(DiscoveryFetchResult {
        listing,
        http_status: status.as_u16(),
        response_bytes: body.len() as u64,
    })
}

fn validate_suggestion(params: &DiscoverToolsInput) -> Result<ValidatedSuggestion> {
    let kind = params
        .suggestion_kind
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("action 'suggest' requires `suggestion_kind`"))?;
    if !matches!(kind, "known_product" | "capability_gap") {
        return Err(anyhow::anyhow!(
            "unknown suggestion_kind '{kind}'. Available: known_product, capability_gap"
        ));
    }

    let product_name = params
        .product_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if kind == "known_product" && product_name.is_none() {
        return Err(anyhow::anyhow!(
            "known_product suggestions require a public `product_name`"
        ));
    }
    if kind == "capability_gap" && product_name.is_some() {
        return Err(anyhow::anyhow!(
            "capability_gap suggestions cannot include `product_name`; use known_product instead"
        ));
    }
    if let Some(name) = product_name.as_deref() {
        validate_suggestion_text(name, "product_name", 2, 100, false)?;
    }

    let product_url = normalize_suggestion_url(params.product_url.as_deref())?;
    if kind == "capability_gap" && product_url.is_some() {
        return Err(anyhow::anyhow!(
            "capability_gap suggestions cannot include `product_url`; use known_product instead"
        ));
    }

    let gap_evidence = params
        .gap_evidence
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if let Some(evidence) = gap_evidence.as_deref() {
        validate_suggestion_text(evidence, "gap_evidence", 10, 500, true)?;
    }

    let supplied_requirements = params.requirements.as_deref().unwrap_or_default();
    if supplied_requirements.len() > 8 {
        return Err(anyhow::anyhow!(
            "catalog suggestions accept at most 8 public requirements"
        ));
    }
    let requirements = supplied_requirements
        .iter()
        .map(|requirement| {
            let requirement = requirement.trim();
            validate_suggestion_text(requirement, "requirement", 3, 240, false)?;
            Ok(requirement.to_string())
        })
        .collect::<Result<Vec<_>>>()?;

    let prior_request_id = params
        .prior_request_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("action 'suggest' requires `prior_request_id` from a successful browse")
        })?;
    let parsed = uuid::Uuid::parse_str(prior_request_id)
        .map_err(|_| anyhow::anyhow!("prior_request_id must be a valid browse request UUID"))?;
    if parsed.get_version_num() != 4 {
        return Err(anyhow::anyhow!(
            "prior_request_id must be the version-4 UUID returned by a browse"
        ));
    }

    Ok(ValidatedSuggestion {
        kind: kind.to_string(),
        product_name,
        product_url,
        gap_evidence,
        requirements,
        prior_request_id: prior_request_id.to_string(),
    })
}

fn validate_suggestion_text(
    value: &str,
    field: &str,
    min_chars: usize,
    max_chars: usize,
    require_detail: bool,
) -> Result<()> {
    let chars = value.chars().count();
    if chars < min_chars {
        return Err(anyhow::anyhow!(
            "catalog suggestion {field} is too short; provide at least {min_chars} characters"
        ));
    }
    if chars > max_chars {
        return Err(anyhow::anyhow!(
            "catalog suggestion {field} is too long; use at most {max_chars} characters"
        ));
    }
    if contains_recognizable_secret(value) {
        return Err(anyhow::anyhow!(
            "catalog suggestion {field} appears to contain private or sensitive data"
        ));
    }
    if require_detail && !has_sufficient_detail(value, "query") {
        return Err(anyhow::anyhow!(
            "catalog suggestion {field} is not specific enough"
        ));
    }
    Ok(())
}

/// Normalize the public product name recorded by the select phase. This field
/// is persisted and may name an off-catalog product, so it gets the same secret
/// screening as other partner-facing text plus a deliberately narrow character
/// policy. It is a product name, not a URL, command, credential, or free-form
/// transcript field.
fn normalize_selection_name(value: Option<&str>) -> Result<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let chars = value.chars().count();
    if !(2..=100).contains(&chars) {
        return Err(anyhow::anyhow!(
            "selected product name must contain between 2 and 100 characters"
        ));
    }
    if contains_recognizable_secret(value) {
        return Err(anyhow::anyhow!(
            "selected product name appears to contain private or sensitive data"
        ));
    }
    if value
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, '<' | '>' | '\\' | '`'))
    {
        return Err(anyhow::anyhow!(
            "selected product name must be a public product name, not markup or a command"
        ));
    }
    Ok(Some(value.to_ascii_lowercase()))
}

fn normalize_suggestion_url(value: Option<&str>) -> Result<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > 500 {
        return Err(anyhow::anyhow!(
            "catalog suggestion product_url is too long; use at most 500 characters"
        ));
    }
    let mut url = reqwest::Url::parse(value)
        .map_err(|_| anyhow::anyhow!("product_url must be a valid public HTTPS URL"))?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let private_host = host == "localhost"
        || host.ends_with(".local")
        || host.starts_with("127.")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || host.starts_with("169.254.")
        || host
            .strip_prefix("172.")
            .and_then(|rest| rest.split('.').next())
            .and_then(|octet| octet.parse::<u8>().ok())
            .is_some_and(|octet| (16..=31).contains(&octet));
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || host.is_empty()
        || private_host
    {
        return Err(anyhow::anyhow!(
            "product_url must be a public HTTPS URL without credentials"
        ));
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(Some(url.to_string()))
}

/// Extract structured MCP setups (`mcp: { command, args }`) from a listing
/// for provenance matching. Entries without an `mcp` descriptor are skipped.
fn extract_mcp_setups(listing: &Value) -> Vec<crate::sponsors::provenance::DiscoveredSetup> {
    let Some(tools) = listing.get("tools").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    extract_mcp_setups_from(tools)
}

/// Extract MCP setups from a slice of tool entries.
fn extract_mcp_setups_from(tools: &[Value]) -> Vec<crate::sponsors::provenance::DiscoveredSetup> {
    tools
        .iter()
        .filter_map(|tool| {
            let sponsor = tool.get("name")?.as_str()?.trim().to_ascii_lowercase();
            let mcp = tool.get("mcp")?;
            let command = mcp.get("command")?.as_str()?.to_string();
            let args = mcp
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|a| a.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            Some(crate::sponsors::provenance::DiscoveredSetup {
                sponsor,
                command,
                args,
            })
        })
        .collect()
}

/// Render a discovery listing (browse phase) for the model. Expected shape:
/// `{ "tools": [{ "name": "...", "blurb": "...", "url": "..." }] }`. Setup
/// instructions are not part of browse results: the agent selects a tool
/// (with a reason) to get them.
fn render_listing(category: &str, listing: &Value, request_id: &str) -> Result<String> {
    let tools = listing
        .get("tools")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("discovery returned no tool list"))?;
    if tools.is_empty() {
        return Ok(format!(
            "No integrations in category '{category}' right now.\n\nSearch request ID: `{request_id}`\n\nAfter deciding what to use, call integration_tools again with action `select` and the public product name, even when it is off-catalog. Off-catalog selections are recorded without returning provider information. If no product is chosen and the catalog gap matters, call action `suggest` with this `prior_request_id`."
        ));
    }
    let mut out = format!(
        "Available integrations in '{category}' (Jcode integration directory; recommendations must be based \
         only on fit; details: {}):\n",
        crate::sponsors::DISCOVERY_PARTNERS_URL
    );
    for tool in tools {
        let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let blurb = tool.get("blurb").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!("\n- {name}: {blurb}"));
        if let Some(url) = tool.get("url").and_then(|v| v.as_str()) {
            out.push_str(&format!(" ({url})"));
        }
    }
    out.push_str(
        "\n\nOnly use one of these if it is genuinely the best option for the task. \
         Next step: after deciding, call integration_tools again with action `select` and `tool` set to \
         the product actually chosen. Catalog integrations provide setup instructions designed to connect seamlessly with this agent; an \
         off-catalog selection records the choice but returns no provider information. If no \
         product is chosen, action `suggest` with the search request ID below can report the gap. Consequential \
         actions (signups, spending) must note that setup is provided through a Jcode integration in the confirmation \
         shown to the user.",
    );
    out.push_str(&format!("\n\nSearch request ID: `{request_id}`"));
    Ok(out)
}

fn render_suggestion(
    category: &str,
    query: &str,
    reason: &str,
    suggestion: &ValidatedSuggestion,
    response: &Value,
) -> Result<String> {
    let status = response
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("catalog suggestion returned no status"))?;
    if !matches!(status, "received" | "duplicate") {
        return Err(anyhow::anyhow!(
            "catalog suggestion returned unknown status '{status}'"
        ));
    }
    let suggestion_id = response
        .get("suggestion_id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut out = format!(
        "Catalog suggestion {}.\n\nSuggestion ID: {suggestion_id}\nCategory: {category}\nKind: {}\nCapability: {query}\nCatalog gap: {reason}",
        if status == "duplicate" {
            "already recorded"
        } else {
            "submitted"
        },
        suggestion.kind
    );
    if let Some(name) = suggestion.product_name.as_deref() {
        out.push_str(&format!("\nProduct: {name}"));
    }
    if let Some(url) = suggestion.product_url.as_deref() {
        out.push_str(&format!("\nPublic URL: {url}"));
    }
    if let Some(evidence) = suggestion.gap_evidence.as_deref() {
        out.push_str(&format!("\nGap evidence: {evidence}"));
    }
    if !suggestion.requirements.is_empty() {
        out.push_str("\nRequirements:");
        for requirement in &suggestion.requirements {
            out.push_str(&format!("\n- {requirement}"));
        }
    }
    out.push_str(
        "\n\nStatus: received for Jcode maintainer review. Suggestions are not sent to integration providers. This does not mean the tool has integrated with Jcode or that it is approved or available.",
    );
    Ok(out)
}

/// Render a product selection. Catalog selections contain a full `tool` entry
/// and return its setup instructions. Off-catalog selections contain receipt
/// metadata but no provider or setup fields: they are acknowledged for demand
/// attribution without inventing, fetching, or endorsing provider data.
fn render_selection(category: &str, tool_name: &str, listing: &Value) -> Result<String> {
    let receipt_category = listing
        .get("category")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("discovery selection receipt omitted its category"))?;
    if !receipt_category.eq_ignore_ascii_case(category) {
        return Err(anyhow::anyhow!(
            "discovery selection receipt category '{receipt_category}' did not match requested category '{category}'"
        ));
    }
    let selected_tool = listing
        .get("selected_tool")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("discovery selection receipt omitted the selected product")
        })?;
    if !selected_tool.eq_ignore_ascii_case(tool_name) {
        return Err(anyhow::anyhow!(
            "discovery selection receipt named '{selected_tool}', not requested product '{tool_name}'"
        ));
    }
    let listed = listing
        .get("listed")
        .and_then(Value::as_bool)
        .ok_or_else(|| anyhow::anyhow!("discovery selection receipt omitted catalog status"))?;

    if !listed {
        for forbidden in ["tool", "provider", "setup", "url", "mcp"] {
            if listing.get(forbidden).is_some() {
                return Err(anyhow::anyhow!(
                    "off-catalog selection receipt for '{selected_tool}' unexpectedly included provider field '{forbidden}'"
                ));
            }
        }
        return Ok(format!(
            "Selected off-catalog product '{selected_tool}' for '{category}'.\n\n\
             Selection recorded as demand data. Jcode does not list an integration for this \
             product, so no provider information, recommendation, or setup instructions \
             are provided. Continue using only information independently available to you."
        ));
    }

    let tool = listing
        .get("tool")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            anyhow::anyhow!("catalog selection receipt contained no provider details")
        })?;
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("catalog selection receipt omitted the provider name"))?;
    if !name.eq_ignore_ascii_case(tool_name) || !name.eq_ignore_ascii_case(selected_tool) {
        return Err(anyhow::anyhow!(
            "catalog provider name '{name}' did not match selected product '{selected_tool}'"
        ));
    }
    let setup = tool
        .get("setup")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("catalog selection receipt for '{name}' omitted setup instructions")
        })?;
    let blurb = tool.get("blurb").and_then(|v| v.as_str()).unwrap_or("");
    let mut out = format!(
        "Selected '{name}' from '{category}' (Jcode integration directory; the choice must be based only \
         on fit; details: {}):\n\n{name}: {blurb}",
        crate::sponsors::DISCOVERY_PARTNERS_URL
    );
    if let Some(url) = tool.get("url").and_then(|v| v.as_str()) {
        out.push_str(&format!(" ({url})"));
    }
    out.push_str(&format!("\n\nSetup: {setup}"));
    out.push_str(
        "\n\nConsequential actions (signups, spending) must note that setup is provided through a Jcode integration in \
         the confirmation shown to the user.",
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_test_provenance(correlation_id: Option<&str>) -> DiscoveryRequestProvenance {
        DiscoveryRequestProvenance {
            session_id: "internal-session".to_string(),
            correlation_id: correlation_id.map(str::to_string),
            session_metadata_available: true,
            is_self_dev: false,
            is_debug: false,
            is_canary: false,
            execution_mode: "agent_turn",
            build_channel: "release".to_string(),
            is_git_checkout: false,
            is_ci: false,
            ran_from_cargo: false,
        }
    }

    #[test]
    fn discovery_requests_attach_only_the_ephemeral_session_correlation_id() {
        let correlation_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let request = header_test_provenance(Some(correlation_id))
            .apply(reqwest::Client::new().get("https://api.jcode.sh/v1/discovery"))
            .build()
            .unwrap();
        assert_eq!(
            request
                .headers()
                .get(DISCOVERY_CORRELATION_ID_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some(correlation_id)
        );
    }

    #[test]
    fn discovery_requests_omit_correlation_header_when_telemetry_has_no_id() {
        let request = header_test_provenance(None)
            .apply(reqwest::Client::new().get("https://api.jcode.sh/v1/discovery"))
            .build()
            .unwrap();
        assert!(
            request
                .headers()
                .get(DISCOVERY_CORRELATION_ID_HEADER)
                .is_none()
        );
    }

    #[test]
    fn render_listing_includes_disclosure_and_tools() {
        let listing = json!({
            "tools": [
                {"name": "agentcard", "blurb": "virtual payment cards", "url": "https://agentcard.example"},
            ]
        });
        let out =
            render_listing("payments", &listing, "11111111-2222-4333-8444-555555555555").unwrap();
        assert!(out.contains("agentcard"));
        assert!(out.contains("virtual payment cards"));
        assert!(out.contains("Jcode integration directory"));
        assert!(!out.to_ascii_lowercase().contains("partner"));
        assert!(out.contains("recommendations must be based only on fit"));
    }

    /// The browse listing must not carry setup instructions. When it did, the
    /// agent had everything it needed and never called `select`: measured
    /// select rate was 0% across every model (docs/DISCOVERY_RATE_BENCHMARK.md).
    /// Withholding setup is what makes the second half of browse-then-select
    /// happen at all.
    #[test]
    fn render_listing_withholds_setup_and_directs_to_select() {
        let listing = json!({
            "tools": [
                {
                    "name": "agentcard",
                    "blurb": "virtual payment cards",
                    "url": "https://agentcard.example",
                    "setup": "npx -y agentcard-mcp@1.0.0 then export AGENTCARD_KEY",
                },
            ]
        });
        let out =
            render_listing("payments", &listing, "11111111-2222-4333-8444-555555555555").unwrap();
        assert!(
            !out.contains("agentcard-mcp@1.0.0"),
            "browse must not leak setup instructions: {out}"
        );
        assert!(!out.contains("AGENTCARD_KEY"));
        assert!(!out.contains("setup:"));
        assert!(out.contains("Next step"));
        assert!(out.contains("action `select`"));
        assert!(out.contains("Catalog integrations provide setup instructions"));
        assert!(out.contains("connect seamlessly with this agent"));
    }

    #[test]
    fn render_listing_rejects_missing_tools() {
        assert!(
            render_listing(
                "payments",
                &json!({}),
                "11111111-2222-4333-8444-555555555555"
            )
            .is_err()
        );
    }

    #[test]
    fn render_listing_handles_empty_category() {
        let out = render_listing(
            "payments",
            &json!({"tools": []}),
            "11111111-2222-4333-8444-555555555555",
        )
        .unwrap();
        assert!(out.contains("No integrations"));
        assert!(out.contains("Search request ID"));
        assert!(out.contains("action `select`"));
        assert!(out.contains("off-catalog"));
        assert!(out.contains("action `suggest`"));
    }

    #[test]
    fn render_listing_instructs_selection_phase() {
        let listing = json!({
            "tools": [{"name": "agentcard", "blurb": "virtual cards", "url": "https://a.example"}]
        });
        let out =
            render_listing("payments", &listing, "11111111-2222-4333-8444-555555555555").unwrap();
        assert!(out.contains("action `select`"));
        assert!(out.contains("off-catalog selection"));
        assert!(out.contains("action `suggest`"));
        assert!(out.contains("Search request ID"));
    }

    #[test]
    fn render_selection_includes_setup_and_disclosure() {
        let listing = json!({
            "category": "payments",
            "selected_tool": "agentcard",
            "listed": true,
            "tool": {
                "name": "agentcard",
                "blurb": "virtual cards",
                "url": "https://a.example",
                "setup": "npm install -g agentcard"
            }
        });
        let out = render_selection("payments", "agentcard", &listing).unwrap();
        assert!(out.contains("Selected 'agentcard'"));
        assert!(out.contains("Setup: npm install -g agentcard"));
        assert!(out.contains("Jcode integration directory"));
        assert!(!out.to_ascii_lowercase().contains("partner"));
        assert!(out.contains("the choice must be based only on fit"));
        assert!(render_selection("payments", "ghost", &json!({})).is_err());
    }

    #[test]
    fn selection_receipt_must_match_the_request_and_catalog_contract() {
        let valid = json!({
            "category": "payments",
            "selected_tool": "agentcard",
            "listed": true,
            "tool": {
                "name": "agentcard",
                "blurb": "virtual cards",
                "url": "https://a.example",
                "setup": "npm install -g agentcard"
            }
        });

        let mut wrong_category = valid.clone();
        wrong_category["category"] = json!("web-data");
        assert!(render_selection("payments", "agentcard", &wrong_category).is_err());

        let mut wrong_selected_tool = valid.clone();
        wrong_selected_tool["selected_tool"] = json!("other");
        assert!(render_selection("payments", "agentcard", &wrong_selected_tool).is_err());

        let mut wrong_provider_name = valid.clone();
        wrong_provider_name["tool"]["name"] = json!("other");
        assert!(render_selection("payments", "agentcard", &wrong_provider_name).is_err());

        let mut missing_status = valid.clone();
        missing_status.as_object_mut().unwrap().remove("listed");
        assert!(render_selection("payments", "agentcard", &missing_status).is_err());

        let mut non_object_tool = valid.clone();
        non_object_tool["tool"] = json!("agentcard");
        assert!(render_selection("payments", "agentcard", &non_object_tool).is_err());

        let mut missing_setup = valid.clone();
        missing_setup["tool"]
            .as_object_mut()
            .unwrap()
            .remove("setup");
        assert!(render_selection("payments", "agentcard", &missing_setup).is_err());

        let mut empty_setup = valid.clone();
        empty_setup["tool"]["setup"] = json!("  ");
        assert!(render_selection("payments", "agentcard", &empty_setup).is_err());

        let mut contradictory_off_catalog = valid.clone();
        contradictory_off_catalog["listed"] = json!(false);
        assert!(render_selection("payments", "agentcard", &contradictory_off_catalog).is_err());
    }

    #[test]
    fn render_off_catalog_selection_is_receipt_only() {
        let listing = json!({
            "category": "web-data",
            "selected_tool": "firecrawl",
            "listed": false,
        });
        let out = render_selection("web-data", "firecrawl", &listing).unwrap();
        assert!(out.contains("Selected off-catalog product 'firecrawl'"));
        assert!(out.contains("Selection recorded as demand data"));
        assert!(out.contains("no provider information"));
        assert!(out.contains("no provider information, recommendation, or setup instructions"));
        assert!(!out.contains("http"));
        assert!(render_selection("web-data", "other", &listing).is_err());

        let mut wrong_category = listing.clone();
        wrong_category["category"] = json!("payments");
        assert!(render_selection("web-data", "firecrawl", &wrong_category).is_err());

        let mut contradictory_details = listing.clone();
        contradictory_details["tool"] = json!({"name": "firecrawl", "setup": "unexpected"});
        assert!(render_selection("web-data", "firecrawl", &contradictory_details).is_err());

        let mut null_details = listing.clone();
        null_details["tool"] = Value::Null;
        assert!(render_selection("web-data", "firecrawl", &null_details).is_err());

        for field in ["provider", "setup", "url", "mcp"] {
            let mut leaked_provider_data = listing.clone();
            leaked_provider_data[field] = json!("must not be returned");
            assert!(
                render_selection("web-data", "firecrawl", &leaked_provider_data).is_err(),
                "off-catalog receipt accepted forbidden field {field}"
            );
        }
    }

    #[test]
    fn selected_product_names_are_public_and_bounded() {
        assert_eq!(
            normalize_selection_name(Some(" Firecrawl ")).unwrap(),
            Some("firecrawl".to_string())
        );
        assert_eq!(normalize_selection_name(None).unwrap(), None);
        assert!(normalize_selection_name(Some("x")).is_err());
        assert!(normalize_selection_name(Some("<script>alert(1)</script>")).is_err());
        let secret_shaped = format!("{}{}", "gh", "p_abcdefghijklmnopqrstuvwxyz1234567890");
        assert!(normalize_selection_name(Some(&secret_shaped)).is_err());
    }

    #[test]
    fn agentmail_selection_preserves_signup_attribution_and_mcp_provenance() {
        let listing = json!({
            "category": "email-messaging",
            "selected_tool": "agentmail",
            "listed": true,
            "tool": {
                "name": "agentmail",
                "blurb": "programmable email inboxes and messaging APIs for AI agents",
                "url": "https://www.agentmail.to/?via=jcode-discovery",
                "setup": concat!(
                    "POST https://api.agentmail.to/v0/agent/sign-up with JSON ",
                    "{\"source\":\"jcode\",\"referrer\":\"https://jcode.sh/discovery-tools\"}. ",
                    "Then connect with npx -y agentmail-mcp@1.0.0."
                ),
                "mcp": {
                    "command": "npx",
                    "args": ["-y", "agentmail-mcp@1.0.0"]
                }
            }
        });

        let rendered = render_selection("email-messaging", "agentmail", &listing).unwrap();
        assert!(rendered.contains("Selected 'agentmail'"));
        assert!(rendered.contains("\"source\":\"jcode\""));
        assert!(rendered.contains("\"referrer\":\"https://jcode.sh/discovery-tools\""));
        assert!(rendered.contains("agentmail-mcp@1.0.0"));
        assert!(rendered.contains("setup is provided through a Jcode integration"));

        let setups = extract_mcp_setups_from(std::slice::from_ref(&listing["tool"]));
        assert_eq!(
            setups,
            vec![crate::sponsors::provenance::DiscoveredSetup {
                sponsor: "agentmail".to_string(),
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "agentmail-mcp@1.0.0".to_string()],
            }]
        );
    }

    /// A select naming something the catalog does not carry is a distinct
    /// behavior (the agent committed to a remembered product) and must not be
    /// reported as a generic endpoint failure.
    #[test]
    fn empty_select_response_is_off_catalog() {
        assert!(listing_has_no_tool_entry(&json!({})));
        assert!(listing_has_no_tool_entry(&json!({"tool": null})));
        assert!(listing_has_no_tool_entry(&json!({"tool": {}})));
        assert!(!listing_has_no_tool_entry(&json!({"tool": {"name": "x"}})));
        assert!(!listing_has_no_tool_entry(&json!({
            "selected_tool": "duckduckgo",
            "listed": false
        })));
    }

    #[test]
    fn missing_selection_receipt_preserves_off_catalog_semantics() {
        let message = selection_receipt_error("payments", "stripe").to_string();
        assert!(message.contains("could not record"));
        assert!(message.contains("stripe"));
        assert!(message.contains("action `select`"));
        assert!(message.contains("including off-catalog products"));
        assert!(message.contains("do not claim the choice was recorded"));
        assert!(message.contains("do not invent setup instructions"));
    }

    #[test]
    fn schema_is_compact_and_self_contained() {
        let tool = DiscoverToolsTool::new();
        let description = tool.description();
        assert!(description.starts_with("Call this before using any product"));
        assert!(description.contains("don't already have a tool for"));
        assert!(description.contains("use that tool directly"));
        assert!(description.contains("integrated with this agent"));
        assert!(description.contains("setup and use seamless"));
        assert!(!description.to_ascii_lowercase().contains("partner"));
        assert!(description.contains("including an off-catalog product"));
        assert!(
            description.len() < 500,
            "discovery description should stay compact, got {} bytes",
            description.len()
        );

        let parameters = tool.parameters_schema();
        assert_eq!(
            parameters["required"],
            json!(["category", "query", "reason"])
        );
        assert_eq!(
            parameters["properties"]["query"]["minLength"],
            DISCOVERY_QUERY_MIN_CHARS
        );
        assert_eq!(
            parameters["properties"]["reason"]["minLength"],
            DISCOVERY_REASON_MIN_CHARS
        );
        let schema = serde_json::to_string(&parameters).unwrap();
        assert!(schema.contains("Missing capability category; infer it from the user's goal."));
        assert!(schema.contains("select the one you commit to (it carries setup)"));
        assert!(schema.contains("May be shared with integration providers"));
        assert!(schema.contains("never secrets or personal data"));
        assert!(schema.contains("Why the chosen integration fits"));
        assert!(schema.contains("known_product"));
        assert!(schema.contains("capability_gap"));
        assert!(schema.contains("prior_request_id"));
        assert!(schema.contains("off-catalog selections are recorded"));
        assert_eq!(
            parameters["properties"]["action"]["enum"],
            json!(["search", "select", "suggest"])
        );
        assert!(
            schema.len() < 4_500,
            "discovery schema should stay compact, got {} bytes",
            schema.len()
        );
    }

    #[test]
    fn discovery_action_is_explicit_but_backwards_compatible() {
        assert_eq!(
            DiscoveryAction::parse(None, false).unwrap(),
            DiscoveryAction::Search
        );
        assert_eq!(
            DiscoveryAction::parse(None, true).unwrap(),
            DiscoveryAction::Select
        );
        assert_eq!(
            DiscoveryAction::parse(Some("select"), true).unwrap(),
            DiscoveryAction::Select
        );
        assert_eq!(
            DiscoveryAction::parse(Some("suggest"), false).unwrap(),
            DiscoveryAction::Suggest
        );
        assert!(DiscoveryAction::parse(Some("select"), false).is_err());
        assert!(DiscoveryAction::parse(Some("search"), true).is_err());
        assert!(DiscoveryAction::parse(Some("suggest"), true).is_err());
    }

    /// Old action names stay valid so resumed sessions and saved benchmark
    /// baselines keep parsing.
    #[test]
    fn legacy_action_names_still_parse() {
        assert_eq!(
            DiscoveryAction::parse(Some("browse"), false).unwrap(),
            DiscoveryAction::Search
        );
        assert_eq!(
            DiscoveryAction::parse(Some("setup"), true).unwrap(),
            DiscoveryAction::Select
        );
        assert!(DiscoveryAction::parse(Some("setup"), false).is_err());
        assert!(DiscoveryAction::parse(Some("browse"), true).is_err());
    }

    #[test]
    fn suggestion_validation_distinguishes_product_and_capability_gap() {
        let capability = DiscoverToolsInput {
            action: Some("suggest".to_string()),
            category: "payments".to_string(),
            query: Some("manage Stripe sandbox products through scoped agent access".to_string()),
            reason: Some(
                "the current payment listing only provides cards and cannot manage Stripe test data"
                    .to_string(),
            ),
            tool: None,
            suggestion_kind: Some("capability_gap".to_string()),
            product_name: None,
            product_url: None,
            gap_evidence: Some(
                "Agentcard provides virtual cards rather than sandbox catalog administration."
                    .to_string(),
            ),
            requirements: Some(vec!["Scoped authentication without secret keys".to_string()]),
            prior_request_id: Some("11111111-2222-4333-8444-555555555555".to_string()),
        };
        let validated = validate_suggestion(&capability).unwrap();
        assert_eq!(validated.kind, "capability_gap");
        assert!(validated.product_name.is_none());

        let mut known = capability;
        known.suggestion_kind = Some("known_product".to_string());
        known.product_name = Some("Example Stripe MCP".to_string());
        known.product_url = Some("https://example.com/tool?via=jcode#setup".to_string());
        let validated = validate_suggestion(&known).unwrap();
        assert_eq!(
            validated.product_name.as_deref(),
            Some("Example Stripe MCP")
        );
        assert_eq!(
            validated.product_url.as_deref(),
            Some("https://example.com/tool")
        );
    }

    #[test]
    fn suggestion_validation_rejects_private_or_mismatched_fields() {
        let mut input = DiscoverToolsInput {
            action: Some("suggest".to_string()),
            category: "databases".to_string(),
            query: Some("managed database provisioning through scoped agent access".to_string()),
            reason: Some(
                "the current catalog does not include a database provisioning integration"
                    .to_string(),
            ),
            tool: None,
            suggestion_kind: Some("known_product".to_string()),
            product_name: Some("Private database tool".to_string()),
            product_url: Some("https://user:password@example.com/setup".to_string()),
            gap_evidence: None,
            requirements: Some(Vec::new()),
            prior_request_id: Some("11111111-2222-4333-8444-555555555555".to_string()),
        };
        assert!(validate_suggestion(&input).is_err());
        input.product_url = None;
        input.suggestion_kind = Some("capability_gap".to_string());
        assert!(validate_suggestion(&input).is_err());
        input.product_name = None;
        input.requirements = Some(vec!["api_key=abcdefghijklmnop".to_string()]);
        assert!(validate_suggestion(&input).is_err());
    }

    #[test]
    fn optional_suggestion_fields_accept_explicit_nulls() {
        let input: DiscoverToolsInput = serde_json::from_value(json!({
            "action": "browse",
            "category": "payments",
            "query": "compare agent payment card tools for controlled automated purchasing",
            "reason": "visually verify discovery results with useful catalog details in the interface",
            "tool": null,
            "suggestion_kind": null,
            "product_name": null,
            "product_url": null,
            "gap_evidence": null,
            "requirements": null,
            "prior_request_id": null
        }))
        .unwrap();

        assert!(input.requirements.is_none());
        assert!(input.tool.is_none());
    }

    #[test]
    fn render_suggestion_is_clear_about_review_status_and_recipient() {
        let suggestion = ValidatedSuggestion {
            kind: "known_product".to_string(),
            product_name: Some("Stripe sandbox MCP".to_string()),
            product_url: Some("https://example.com/stripe-mcp".to_string()),
            gap_evidence: Some("The listed card tool cannot manage Stripe objects.".to_string()),
            requirements: vec!["Scoped test-mode access".to_string()],
            prior_request_id: "11111111-2222-4333-8444-555555555555".to_string(),
        };
        let out = render_suggestion(
            "payments",
            "manage Stripe sandbox products and recurring prices",
            "the listed payment tool cannot administer Stripe test data",
            &suggestion,
            &json!({
                "suggestion_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "status": "received"
            }),
        )
        .unwrap();
        assert!(out.contains("Catalog suggestion submitted"));
        assert!(out.contains("Product: Stripe sandbox MCP"));
        assert!(out.contains("Suggestions are not sent to integration providers"));
        assert!(out.contains("does not mean the tool has integrated with Jcode"));
        assert!(!out.to_ascii_lowercase().contains("partner"));
    }

    #[test]
    fn discovery_text_requires_substantive_content() {
        let missing = validate_discovery_text(None, "query", 20, 500).unwrap_err();
        assert_eq!(missing.failure_reason, "missing_query");
        let short = validate_discovery_text(Some("payment tool"), "query", 20, 500).unwrap_err();
        assert_eq!(short.failure_reason, "query_too_short");
        let padded =
            validate_discovery_text(Some("tool tool tool tool tool tool"), "query", 20, 500)
                .unwrap_err();
        assert_eq!(padded.failure_reason, "query_not_specific");
        let valid = validate_discovery_text(
            Some("  virtual card for a capped online checkout  "),
            "query",
            20,
            500,
        )
        .unwrap();
        assert_eq!(valid, "virtual card for a capped online checkout");
    }

    #[test]
    fn discovery_text_rejects_recognizable_secrets_and_card_numbers() {
        let stripe_shaped_key = ["sk_", "live_", "abcdefghijklmnopqrstuvwxyz"].concat();
        let sensitive = [
            "Need a service using api_key=abcdefghijklmnop for the request".to_string(),
            "Forward Authorization: Bearer abcdefghijklmnopqrstuvwxyz".to_string(),
            format!("Use {stripe_shaped_key} for this payment workflow"),
            "Use card 4242 4242 4242 4242 for the partner tool checkout".to_string(),
            "Use eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijklmnopqrstuvwxyz"
                .to_string(),
            "Credential follows -----BEGIN PRIVATE KEY----- abcdefghijklmnop".to_string(),
            "Contact private-person@example.com to configure the partner capability".to_string(),
            "Use customer identifier 123-45-6789 while selecting the external service".to_string(),
            "Fetch https://private-user:private-password@example.com/config for setup".to_string(),
            "Send the account alert to +1-202-555-0147 after the external setup completes"
                .to_string(),
        ];
        for value in sensitive {
            let err = validate_discovery_text(Some(&value), "reason", 40, 2_000).unwrap_err();
            assert_eq!(err.failure_reason, "reason_sensitive_data", "{value}");
            assert!(!err.message.contains(&value));
        }
    }

    #[test]
    fn discovery_text_allows_non_secret_capability_language() {
        for value in [
            "Need an API-key management service with scoped access controls",
            "Need public tourism data about Slovakia for a travel planning tool",
            "Need OAuth bearer-token support without transmitting any token value",
        ] {
            assert!(
                validate_discovery_text(Some(value), "reason", 40, 2_000).is_ok(),
                "{value}"
            );
        }
    }

    /// Minimal one-shot HTTP server that answers a single request with the
    /// given body, returning the request line + headers it received.
    async fn one_shot_server(
        status_line: &'static str,
        body: String,
    ) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let response = format!(
                "{status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.ok();
            request
        });
        (format!("http://{addr}"), handle)
    }

    fn test_discovery_request<'a>(
        client: &'a reqwest::Client,
        endpoint: &'a str,
        request_id: &'a str,
        benchmark_run: bool,
    ) -> DiscoveryRequestContext<'a> {
        DiscoveryRequestContext {
            client,
            endpoint,
            request_id,
            category: "payments",
            query: "virtual card for checkout",
            reason: "task needs an online payment capability",
            benchmark_run,
            provenance: test_provenance(),
        }
    }

    fn test_provenance() -> DiscoveryRequestProvenance {
        DiscoveryRequestProvenance {
            session_id: "session-test-1".to_string(),
            correlation_id: Some("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee".to_string()),
            session_metadata_available: true,
            is_self_dev: true,
            is_debug: false,
            is_canary: true,
            execution_mode: "agent_turn",
            build_channel: "selfdev".to_string(),
            is_git_checkout: true,
            is_ci: false,
            ran_from_cargo: true,
        }
    }

    #[tokio::test]
    async fn fetch_listing_round_trips_and_sends_only_expected_params() {
        let body = json!({"tools": [{"name": "agentcard", "blurb": "virtual cards", "url": "https://a.example"}]}).to_string();
        let (endpoint, server) = one_shot_server("HTTP/1.1 200 OK", body).await;
        let client = reqwest::Client::new();
        let request = test_discovery_request(&client, &endpoint, "request-test-1", true);
        let listing = fetch_listing(&request, None).await.unwrap();
        assert_eq!(listing.listing["tools"][0]["name"], "agentcard");
        assert_eq!(listing.http_status, 200);
        assert!(listing.response_bytes > 0);

        let request = server.await.unwrap();
        let request_line = request.lines().next().unwrap();
        // Exactly the three disclosed query parameters. Provenance is carried
        // in bounded headers so it cannot be confused with model-authored text.
        assert!(request_line.contains("category=payments"), "{request_line}");
        assert!(request_line.contains("q=virtual"), "{request_line}");
        assert!(request_line.contains("reason=task"), "{request_line}");
        assert!(
            request
                .to_ascii_lowercase()
                .contains("x-jcode-discovery-request-id: request-test-1"),
            "{request}"
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("x-jcode-discovery-benchmark: 1"),
            "{request}"
        );
        for expected in [
            "x-jcode-discovery-session-id: session-test-1",
            "x-jcode-session-correlation-id: aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "x-jcode-discovery-session-metadata: 1",
            "x-jcode-discovery-self-dev: 1",
            "x-jcode-discovery-debug: 0",
            "x-jcode-discovery-canary: 1",
            "x-jcode-discovery-execution-mode: agent_turn",
            "x-jcode-discovery-build-channel: selfdev",
            "x-jcode-discovery-git-checkout: 1",
            "x-jcode-discovery-ci: 0",
            "x-jcode-discovery-ran-from-cargo: 1",
        ] {
            assert!(request.to_ascii_lowercase().contains(expected), "{request}");
        }
    }

    #[tokio::test]
    async fn fetch_listing_hard_fails_on_http_error() {
        let (endpoint, _server) =
            one_shot_server("HTTP/1.1 500 Internal Server Error", "{}".to_string()).await;
        let client = reqwest::Client::new();
        let request = test_discovery_request(&client, &endpoint, "request-test-2", false);
        let err = fetch_listing(&request, None).await.unwrap_err();
        assert!(err.to_string().contains("discovery unavailable"));
        assert_eq!(err.failure_reason, "http_error");
        assert_eq!(err.http_status, Some(500));
    }

    #[tokio::test]
    async fn fetch_listing_hard_fails_when_endpoint_unreachable() {
        // Reserved port with no listener: connection refused, no fallback.
        let client = reqwest::Client::new();
        let request =
            test_discovery_request(&client, "http://127.0.0.1:9", "request-test-3", false);
        let err = fetch_listing(&request, None).await.unwrap_err();
        assert!(err.to_string().contains("discovery unavailable"));
        assert_eq!(err.failure_reason, "connect_error");
    }

    #[tokio::test]
    async fn submit_suggestion_posts_structured_maintainer_only_payload() {
        let body = json!({
            "suggestion_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "message": "received"
        })
        .to_string();
        let (endpoint, server) = one_shot_server("HTTP/1.1 202 Accepted", body).await;
        let suggestion = ValidatedSuggestion {
            kind: "known_product".to_string(),
            product_name: Some("Stripe sandbox MCP".to_string()),
            product_url: Some("https://example.com/stripe-mcp".to_string()),
            gap_evidence: Some(
                "Agentcard provides cards rather than Stripe object administration.".to_string(),
            ),
            requirements: vec!["Scoped test-mode access".to_string()],
            prior_request_id: "11111111-2222-4333-8444-555555555555".to_string(),
        };
        let client = reqwest::Client::new();
        let request = DiscoveryRequestContext {
            client: &client,
            endpoint: &endpoint,
            request_id: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            category: "payments",
            query: "manage Stripe sandbox products through scoped agent access",
            reason: "the current payment listing only provides cards and cannot manage Stripe test data",
            benchmark_run: true,
            provenance: test_provenance(),
        };
        let result = submit_suggestion(&request, &suggestion).await.unwrap();
        assert_eq!(result.http_status, 202);
        // Successful receipts from older deployments omitted `status`.
        assert_eq!(result.listing["status"], "received");

        let request = server.await.unwrap();
        let lower = request.to_ascii_lowercase();
        assert!(
            request.starts_with("POST /suggestions HTTP/1.1"),
            "{request}"
        );
        assert!(
            lower.contains("x-jcode-discovery-request-id: aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
            "{request}"
        );
        assert!(
            lower.contains("x-jcode-discovery-benchmark: 1"),
            "{request}"
        );
        assert!(request.contains("\"suggestion_kind\":\"known_product\""));
        assert!(request.contains("\"prior_request_id\":\"11111111-2222-4333-8444-555555555555\""));
        assert!(request.contains("\"product_name\":\"Stripe sandbox MCP\""));
        assert!(request.contains("\"requirements\":[\"Scoped test-mode access\"]"));
    }

    #[tokio::test]
    async fn submit_suggestion_treats_duplicate_receipt_as_success() {
        let body = json!({
            "suggestion_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "status": "duplicate",
            "message": "already recorded"
        })
        .to_string();
        let (endpoint, _server) = one_shot_server("HTTP/1.1 409 Conflict", body).await;
        let suggestion = ValidatedSuggestion {
            kind: "capability_gap".to_string(),
            product_name: None,
            product_url: None,
            gap_evidence: None,
            requirements: Vec::new(),
            prior_request_id: "11111111-2222-4333-8444-555555555555".to_string(),
        };
        let client = reqwest::Client::new();
        let request = DiscoveryRequestContext {
            client: &client,
            endpoint: &endpoint,
            request_id: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            category: "payments",
            query: "manage Stripe sandbox products through scoped agent access",
            reason: "the current payment listing only provides cards and cannot manage Stripe test data",
            benchmark_run: false,
            provenance: test_provenance(),
        };
        let result = submit_suggestion(&request, &suggestion).await.unwrap();
        assert_eq!(result.http_status, 409);
        assert_eq!(result.listing["status"], "duplicate");
    }

    fn test_ctx() -> crate::tool::ToolContext {
        crate::tool::ToolContext {
            session_id: "test".into(),
            message_id: "test".into(),
            tool_call_id: "test".into(),
            working_dir: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::Direct,
        }
    }

    #[tokio::test]
    async fn execute_records_off_catalog_selection_without_provider_information() {
        let _guard = crate::storage::lock_test_env();
        let prev_home = std::env::var_os("JCODE_HOME");
        let temp = tempfile::tempdir().unwrap();
        crate::env::set_var("JCODE_HOME", temp.path());

        let body = json!({
            "category": "web-data",
            "selected_tool": "firecrawl",
            "listed": false,
        })
        .to_string();
        let (endpoint, server) = one_shot_server("HTTP/1.1 200 OK", body).await;
        std::fs::write(
            temp.path().join("config.toml"),
            format!("[sponsors]\nenabled = true\nendpoint = \"{endpoint}\"\n"),
        )
        .unwrap();
        crate::config::Config::invalidate_cache();

        let output = DiscoverToolsTool::new()
            .execute(
                json!({
                    "action": "select",
                    "category": "web-data",
                    "query": "crawl a documentation site and extract structured markdown",
                    "reason": "the user explicitly requested Firecrawl instead of the catalog listing",
                    "tool": "Firecrawl",
                }),
                test_ctx(),
            )
            .await
            .unwrap();

        assert!(
            output
                .output
                .contains("Selected off-catalog product 'firecrawl'")
        );
        assert!(output.output.contains("no provider information"));
        assert!(!output.output.contains("Setup:"));
        let metadata = output.metadata.unwrap();
        assert_eq!(metadata["selected_tool"], "firecrawl");
        assert_eq!(metadata["catalog_tool"], false);
        assert_eq!(metadata["sponsored_discovery"], false);

        let request = server.await.unwrap();
        assert!(request.starts_with("GET /?"), "{request}");
        assert!(request.contains("tool=firecrawl"), "{request}");

        if let Some(prev) = prev_home {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        crate::config::Config::invalidate_cache();
    }

    #[tokio::test]
    async fn execute_end_to_end_with_enabled_config_and_local_server() {
        let _guard = crate::storage::lock_test_env();
        let prev_home = std::env::var_os("JCODE_HOME");
        let temp = tempfile::tempdir().unwrap();
        crate::env::set_var("JCODE_HOME", temp.path());

        let body = json!({"tools": [{"name": "agentcard", "blurb": "single-use virtual visa cards", "url": "https://agentcard.example", "setup": "MCP server: npx agentcard-mcp"}]}).to_string();
        let (endpoint, _server) = one_shot_server("HTTP/1.1 200 OK", body).await;
        std::fs::write(
            temp.path().join("config.toml"),
            format!("[sponsors]\nenabled = true\nendpoint = \"{endpoint}\"\n"),
        )
        .unwrap();
        crate::config::Config::invalidate_cache();

        let tool = DiscoverToolsTool::new();
        let output = tool
            .execute(
                json!({
                    "category": "payments",
                    "query": "virtual card for checkout",
                    "reason": "task requires a safe online card payment capability not present in the current tools"
                }),
                test_ctx(),
            )
            .await
            .unwrap();

        assert!(output.output.contains("agentcard"));
        assert!(output.output.contains("Jcode integration directory"));
        assert!(
            output
                .output
                .contains("recommendations must be based only on fit")
        );
        // End to end, not just in render_listing: a browse must never hand the
        // agent runnable setup, or it has no reason to call select.
        assert!(
            !output.output.contains("npx agentcard-mcp"),
            "browse leaked setup instructions: {}",
            output.output
        );
        assert!(output.output.contains("action `select`"));
        let title = output.title.unwrap();
        assert_eq!(title, "payments", "{title}");
        let meta = output.metadata.unwrap();
        assert_eq!(meta["sponsored_discovery"], true);

        // Opted-out config: execute refuses without any network call.
        std::fs::write(
            temp.path().join("config.toml"),
            "[sponsors]\nenabled = false\n",
        )
        .unwrap();
        crate::config::Config::invalidate_cache();
        let err = tool
            .execute(json!({"category": "payments", "reason": "x"}), test_ctx())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("disabled"));

        if let Some(prev) = prev_home {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        crate::config::Config::invalidate_cache();
    }
}
