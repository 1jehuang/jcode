//! ChatGPT browser model catalog. Discovery never sends a message.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatGptWebModel {
    pub model: String,
    pub label: String,
    pub available: bool,
    pub detail: String,
    pub expected_upstream_models: Vec<String>,
    /// ChatGPT's own model slug, sent in the request body. Present for routes
    /// discovered from the account catalog; picker-only routes have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// Context window reported by ChatGPT for this model, in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<usize>,
    /// ChatGPT's built-in tools for this model (search, canvas, ...). Shown
    /// for information; jcode's web transport tells the model not to use them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub image_input: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatGptWebReasoningOption {
    pub value: String,
    pub label: String,
    pub available: bool,
    /// ChatGPT's own `thinking_effort` value for this choice, sent in the
    /// request body. Absent for picker-only choices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    /// Context window of the model this choice runs on. Instant and thinking
    /// variants of one family differ (e.g. 52K vs 262K).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<usize>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    observed_at: u64,
    models: Vec<ChatGptWebModel>,
    reasoning: BTreeMap<String, Vec<ChatGptWebReasoningOption>>,
    /// Most recent failed refresh. It annotates, but never erases, the last
    /// successful observation: every browser turn re-verifies the exact model
    /// and Power choice in the page before sending anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_refresh_error: Option<String>,
}

static SESSION_LOCK: LazyLock<Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));

/// Serialize discovery and whole browser turns within this process. Refresh
/// already takes this non-reentrant lock, so refresh before taking a turn guard.
pub async fn lock_session() -> tokio::sync::OwnedMutexGuard<()> {
    SESSION_LOCK.clone().lock_owned().await
}

/// Shared browser DOM logic, also used by the OpenAI browser runtime.
pub const UI_SCRIPT: &str = include_str!("chatgpt_web_ui.js");

/// Explicit response-slug whitelist. Labels are not API dataset slugs.
/// No response dataset slugs have been observed for this UI yet.
pub fn accepted_upstream_models_for_label(_label: &str) -> Vec<String> {
    Vec::new()
}

#[derive(Deserialize)]
struct Discovery {
    models: Vec<ObservedModel>,
}
#[derive(Deserialize)]
struct ObservedModel {
    label: String,
    available: bool,
    detail: String,
    reasoning_options: Vec<ChatGptWebReasoningOption>,
}

fn reasoning_value(label: &str) -> Option<&'static str> {
    match label {
        "Instant" => Some("none"),
        "Medium" => Some("medium"),
        "High" => Some("high"),
        "Extra High" => Some("xhigh"),
        "Pro" => Some("max"),
        _ => None,
    }
}

fn parse_discovery(value: serde_json::Value) -> Result<Snapshot> {
    let discovery: Discovery = serde_json::from_value(value)?;
    let mut snapshot = Snapshot {
        version: 1,
        observed_at: now_secs(),
        ..Default::default()
    };
    let mut seen = std::collections::BTreeSet::new();
    for observed in discovery.models {
        let model = jcode_provider_core::normalize_chatgpt_web_model_label(&observed.label)
            .ok_or_else(|| anyhow::anyhow!("Unsafe ChatGPT picker label: {:?}", observed.label))?;
        anyhow::ensure!(
            seen.insert(model.clone()),
            "Duplicate ChatGPT model ID: {model}"
        );
        anyhow::ensure!(
            observed.detail.len() <= 4096,
            "ChatGPT picker detail too long"
        );
        let mut efforts = std::collections::BTreeSet::new();
        let mut options = observed.reasoning_options;
        for option in &mut options {
            anyhow::ensure!(
                reasoning_value(&option.label) == Some(option.value.as_str()),
                "Unmapped ChatGPT reasoning label/value: {:?}/{:?}",
                option.label,
                option.value
            );
            anyhow::ensure!(
                efforts.insert(option.value.clone()),
                "Duplicate reasoning choice for {model}"
            );
            option.available &= observed.available;
        }
        snapshot.reasoning.insert(model.clone(), options);
        snapshot.models.push(ChatGptWebModel {
            model,
            expected_upstream_models: accepted_upstream_models_for_label(&observed.label),
            label: observed.label,
            available: observed.available,
            detail: observed.detail,
            slug: None,
            context_tokens: None,
            capabilities: Vec::new(),
            image_input: false,
        });
    }
    Ok(snapshot)
}

/// Converts the account catalog from `/backend-api/models` into routes. Each
/// picker family (ChatGPT's "versions") becomes one route whose effort ladder
/// spans its presets: Instant (the instant model), every thinking level the
/// thinking model accepts, and Pro. Standalone models with their own context
/// (the minis) become their own routes. Work-mode `-wm` models are skipped:
/// a live probe showed ChatGPT silently serves them as the default model.
fn parse_catalog(value: serde_json::Value) -> Result<Snapshot> {
    #[derive(Deserialize)]
    struct Catalog {
        versions: Vec<Version>,
        models: Vec<CatalogModel>,
    }
    #[derive(Deserialize)]
    struct Version {
        title: String,
        presets: Vec<Preset>,
    }
    #[derive(Deserialize)]
    struct Preset {
        slug: String,
        effort: Option<String>,
        lane: String,
    }
    #[derive(Deserialize, Clone)]
    struct CatalogModel {
        slug: String,
        title: String,
        #[serde(rename = "reasoningType")]
        reasoning_type: String,
        #[serde(rename = "contextTokens")]
        context_tokens: Option<usize>,
        #[serde(rename = "workMode")]
        work_mode: bool,
        efforts: Vec<CatalogEffort>,
        tools: Vec<String>,
        #[serde(rename = "imageInput")]
        image_input: bool,
    }
    #[derive(Deserialize, Clone)]
    struct CatalogEffort {
        value: String,
    }

    let catalog: Catalog = serde_json::from_value(value)?;
    let by_slug: BTreeMap<&str, &CatalogModel> = catalog
        .models
        .iter()
        .map(|m| (m.slug.as_str(), m))
        .collect();
    let safe_slug = |slug: &str| {
        !slug.is_empty()
            && slug.len() <= 64
            && slug
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
    };
    let capabilities = |model: &CatalogModel| {
        model
            .tools
            .iter()
            .filter_map(|tool| match tool.as_str() {
                "search" => Some("web search"),
                "canvas" => Some("canvas"),
                "image_gen_tool_enabled" => Some("image generation"),
                "tools" | "tools2" => Some("code execution"),
                _ => None,
            })
            .map(str::to_string)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    };

    let mut snapshot = Snapshot {
        version: 1,
        observed_at: now_secs(),
        ..Default::default()
    };
    let mut used_slugs = std::collections::BTreeSet::new();
    let push = |snapshot: &mut Snapshot,
                label: String,
                model: &CatalogModel,
                options: Vec<ChatGptWebReasoningOption>,
                detail: String|
     -> Result<()> {
        let Some(route) = jcode_provider_core::normalize_chatgpt_web_model_label(&label) else {
            anyhow::bail!("Unsafe ChatGPT model title: {label:?}");
        };
        if snapshot.models.iter().any(|m| m.model == route) {
            return Ok(());
        }
        let detail = model_detail(&detail, &options);
        snapshot.reasoning.insert(route.clone(), options);
        snapshot.models.push(ChatGptWebModel {
            model: route,
            expected_upstream_models: accepted_upstream_models_for_label(&label),
            label,
            available: true,
            detail,
            slug: Some(model.slug.clone()),
            context_tokens: model.context_tokens,
            capabilities: capabilities(model),
            image_input: model.image_input,
        });
        Ok(())
    };

    // Picker families: one route, the full preset ladder as efforts.
    for version in &catalog.versions {
        let mut options = Vec::new();
        let mut anchor: Option<&CatalogModel> = None;
        let mut thinking: Option<&CatalogModel> = None;
        for preset in &version.presets {
            let Some(model) = by_slug.get(preset.slug.as_str()).copied() else {
                continue;
            };
            if !safe_slug(&model.slug) || model.work_mode {
                continue;
            }
            used_slugs.insert(model.slug.clone());
            match preset.lane.as_str() {
                "instant" => options.push(
                    effort_option("none", "Instant", &model.slug, None)
                        .with_context(model.context_tokens),
                ),
                "thinking" => {
                    thinking = Some(model);
                    anchor.get_or_insert(model);
                }
                "pro" => {
                    let upstream = model.efforts.first().map(|e| e.value.clone());
                    options.push(
                        effort_option("max", "Pro", &model.slug, upstream)
                            .with_context(model.context_tokens),
                    );
                }
                _ => {}
            }
            let _ = &preset.effort;
        }
        // Every thinking level the thinking model accepts, not only the
        // levels the picker happens to show.
        if let Some(model) = thinking {
            for effort in &model.efforts {
                if let Some((value, label)) = thinking_level(&effort.value) {
                    options.push(
                        effort_option(value, label, &model.slug, Some(effort.value.clone()))
                            .with_context(model.context_tokens),
                    );
                }
            }
        }
        options.sort_by_key(|option| effort_rank(&option.value));
        options.dedup_by(|a, b| a.value == b.value);
        let anchor = anchor.or_else(|| {
            version
                .presets
                .iter()
                .find_map(|p| by_slug.get(p.slug.as_str()).copied())
        });
        let (Some(anchor), false) = (anchor, options.is_empty()) else {
            continue;
        };
        push(
            &mut snapshot,
            version.title.clone(),
            anchor,
            options,
            String::new(),
        )?;
    }

    // Standalone models the picker groups hide (minis). Skip auto routers,
    // work-mode models, and anything already covered by a family.
    for model in &catalog.models {
        if used_slugs.contains(&model.slug)
            || model.work_mode
            || model.reasoning_type == "auto"
            || model.reasoning_type == "pro"
            || !safe_slug(&model.slug)
            || TEMPORARY_CHAT_FAILURES.contains(&model.slug.as_str())
        {
            continue;
        }
        let suffix = match (
            model.slug.ends_with("-t-mini"),
            model.slug.ends_with("-mini"),
        ) {
            (true, _) => " Thinking mini",
            (false, true) => " mini",
            _ => "",
        };
        if suffix.is_empty() && !model.slug.contains("mini") {
            // Instant/thinking variants of a family are reached through its
            // effort ladder, not as separate routes.
            continue;
        }
        let label = if model.title.to_ascii_lowercase().contains("mini") {
            model.title.clone()
        } else {
            format!("{}{suffix}", model.title)
        };
        let options = if model.reasoning_type == "reasoning" {
            vec![
                effort_option("medium", "Thinking", &model.slug, None)
                    .with_context(model.context_tokens),
            ]
        } else {
            vec![
                effort_option("none", "Instant", &model.slug, None)
                    .with_context(model.context_tokens),
            ]
        };
        push(
            &mut snapshot,
            label,
            model,
            options,
            format!("ChatGPT model {}", model.slug),
        )?;
    }
    anyhow::ensure!(
        !snapshot.models.is_empty(),
        "ChatGPT catalog contained no usable models"
    );
    Ok(snapshot)
}

/// One picker line: the context window range across the route's levels.
/// ChatGPT's own tools (search, code, canvas) and image input are not
/// listed: jcode turns are text only and tell the model not to use them,
/// since they run in ChatGPT's sandbox rather than on the user's machine.
fn model_detail(prefix: &str, options: &[ChatGptWebReasoningOption]) -> String {
    let short = |tokens: usize| {
        if tokens >= 1000 {
            format!("{}K", tokens / 1000)
        } else {
            tokens.to_string()
        }
    };
    let contexts: Vec<usize> = options.iter().filter_map(|o| o.context_tokens).collect();
    let mut parts = Vec::new();
    if !prefix.is_empty() {
        parts.push(prefix.to_string());
    }
    match (contexts.iter().min(), contexts.iter().max()) {
        (Some(min), Some(max)) if min == max => parts.push(format!("{} context", short(*min))),
        (Some(min), Some(max)) => parts.push(format!("{}-{} context", short(*min), short(*max))),
        _ => {}
    }
    if options.iter().any(|o| o.label == "Pro") {
        parts.push("Pro runs in the background".to_string());
    }
    parts.join(" · ")
}

/// Catalog models the server rejects in a Temporary Chat (every jcode turn
/// is one), observed 2026-10-15: `research` answers HTTP 500, and
/// `gpt-5-3-mini` ends every turn with "Error in message stream".
const TEMPORARY_CHAT_FAILURES: &[&str] = &["research", "gpt-5-3-mini"];

/// jcode effort value and label for a ChatGPT `thinking_effort`.
fn thinking_level(upstream: &str) -> Option<(&'static str, &'static str)> {
    match upstream {
        "min" => Some(("low", "Light")),
        "standard" => Some(("medium", "Medium")),
        "extended" => Some(("high", "High")),
        "max" => Some(("xhigh", "Extra High")),
        _ => None,
    }
}

fn effort_rank(value: &str) -> usize {
    ["none", "low", "medium", "high", "xhigh", "max"]
        .iter()
        .position(|v| *v == value)
        .unwrap_or(usize::MAX)
}

/// An effort choice that maps to an exact model slug and thinking level.
/// `slug` is stored in `upstream` as `slug` or `slug:thinking_effort`.
fn effort_option(
    value: &str,
    label: &str,
    slug: &str,
    thinking_effort: Option<String>,
) -> ChatGptWebReasoningOption {
    ChatGptWebReasoningOption {
        value: value.to_string(),
        label: label.to_string(),
        available: true,
        upstream: Some(match thinking_effort {
            Some(effort) => format!("{slug}:{effort}"),
            None => slug.to_string(),
        }),
        context_tokens: None,
    }
}

impl ChatGptWebReasoningOption {
    fn with_context(mut self, tokens: Option<usize>) -> Self {
        self.context_tokens = tokens;
        self
    }
}

/// Context window for a route at the given jcode effort: the window of the
/// exact model that effort runs on, else the route's own.
pub fn context_tokens(model: &str, effort: Option<&str>) -> Option<usize> {
    let options = cached_reasoning_options(model);
    effort
        .and_then(|effort| options.iter().find(|o| o.value == effort))
        .or_else(|| options.iter().find(|o| o.value == "medium"))
        .or_else(|| (options.len() == 1).then(|| &options[0]))
        .and_then(|o| o.context_tokens)
        .or_else(|| find_model(model).and_then(|m| m.context_tokens))
}

/// The exact ChatGPT model slug and thinking level for a route and jcode
/// effort, when the route came from the account catalog.
pub fn request_route(model: &str, effort: Option<&str>) -> Option<(String, Option<String>)> {
    let options = cached_reasoning_options(model);
    let option = match effort {
        Some(effort) => options.iter().find(|o| o.available && o.value == effort),
        None => None,
    }
    .or_else(|| options.iter().find(|o| o.available && o.value == "medium"))
    .or_else(|| options.iter().find(|o| o.available))?;
    let upstream = option.upstream.as_deref()?;
    let (slug, effort) = match upstream.split_once(':') {
        Some((slug, effort)) => (slug, Some(effort.to_string())),
        None => (upstream, None),
    };
    Some((slug.to_string(), effort))
}

/// Availability is the last successful picker observation. Age, restarts and
/// failed refreshes never hide it: a transient discovery failure must not make
/// the routes unselectable, and each turn verifies selection in the live page.
fn snapshot_models(snapshot: &Snapshot) -> Vec<ChatGptWebModel> {
    if snapshot.version != 1 {
        return Vec::new();
    }
    let note = snapshot
        .last_refresh_error
        .as_deref()
        .map(|error| format!("last refresh failed: {error}"));
    snapshot
        .models
        .iter()
        .filter(|model| {
            jcode_provider_core::normalize_chatgpt_web_model_label(&model.label).as_deref()
                == Some(model.model.as_str())
        })
        .cloned()
        .map(|mut model| {
            // Never trust mappings persisted by another build or edited cache file.
            model.expected_upstream_models = accepted_upstream_models_for_label(&model.label);
            if let Some(note) = &note {
                model.detail = if model.detail.is_empty() {
                    note.clone()
                } else {
                    format!("{} · {note}", model.detail)
                };
            }
            model
        })
        .collect()
}

/// One short line for model-picker details.
fn refresh_error_summary(error: &anyhow::Error) -> String {
    let text = format!("{error:#}");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match text.char_indices().nth(200) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text,
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

const CACHE_FILE: &str = "chatgpt_web_model_catalog_cache.json";

#[derive(Default)]
struct Cache {
    path: Option<std::path::PathBuf>,
    snapshot: Snapshot,
}

static CACHE: LazyLock<std::sync::Mutex<Cache>> = LazyLock::new(Default::default);

fn cache_path() -> Result<std::path::PathBuf> {
    Ok(crate::storage::jcode_dir()?.join(CACHE_FILE))
}

fn load_cache() -> Cache {
    let path = cache_path().ok();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.path != path {
        cache.snapshot = path
            .as_ref()
            .and_then(|p| crate::storage::read_json(p).ok())
            .unwrap_or_default();
        cache.path = path.clone();
    }
    Cache {
        path,
        snapshot: cache.snapshot.clone(),
    }
}

fn store_cache(snapshot: Snapshot) {
    let path = cache_path().ok();
    if let Some(path) = &path
        && let Err(error) = crate::storage::write_json(path, &snapshot)
    {
        crate::logging::warn(&format!(
            "Failed to persist ChatGPT browser catalog: {error:#}"
        ));
    }
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Cache { path, snapshot };
}

/// Models from the last successful picker observation. Their availability
/// decides which routes can be selected. Every browser turn still proves the
/// exact model and Power choice in the live page before sending.
pub fn cached_models() -> Vec<ChatGptWebModel> {
    snapshot_models(&load_cache().snapshot)
}

/// Offline discovery input for downstream refresh integration regressions.
#[cfg(any(test, feature = "test-support"))]
pub async fn refresh_models_from_discovery_for_tests(
    discovery: serde_json::Value,
) -> Result<Vec<ChatGptWebModel>> {
    record_refresh(async { parse_discovery(discovery) }).await
}

/// Fixed web IDs jcode used before model discovery (gpt-5.6-pro[web] since the
/// first web route, the others briefly on 2026-10-14), mapped to the
/// discovered model family and its Power level so saved sessions keep working.
pub fn legacy_web_alias(model: &str) -> Option<(&'static str, &'static str)> {
    Some(match model.trim() {
        "gpt-6-pro[web]" => ("gpt-6[web]", "max"),
        "gpt-6-thinking[web]" => ("gpt-6[web]", "medium"),
        "gpt-6-instant[web]" => ("gpt-6[web]", "none"),
        "gpt-5.6-pro[web]" => ("gpt-5.6-sol[web]", "max"),
        "gpt-5.6-thinking[web]" => ("gpt-5.6-sol[web]", "medium"),
        "gpt-5.6-instant[web]" => ("gpt-5.6-sol[web]", "none"),
        _ => return None,
    })
}

pub fn find_model(model: &str) -> Option<ChatGptWebModel> {
    if !jcode_provider_core::is_chatgpt_web_model(model) {
        return None;
    }
    cached_models()
        .into_iter()
        .find(|entry| entry.model == model)
}

pub fn cached_reasoning_options(model: &str) -> Vec<ChatGptWebReasoningOption> {
    let cache = load_cache();
    let available = snapshot_models(&cache.snapshot)
        .iter()
        .any(|entry| entry.model == model && entry.available);
    cache
        .snapshot
        .reasoning
        .get(model)
        .into_iter()
        .flatten()
        .filter(|option| {
            // Catalog-derived choices carry their exact upstream route.
            option.upstream.is_some()
                || reasoning_value(&option.label) == Some(option.value.as_str())
        })
        .cloned()
        .map(|mut option| {
            option.available &= available;
            option
        })
        .collect()
}

/// Refreshes the account's ChatGPT model list in jcode's private, invisible
/// browser. `sign_in` decides whether a signed-out profile may show the
/// one-time sign-in window: background prefetches never do. The spawned task
/// owns the session lock and page cleanup even if the caller is dropped.
pub async fn refresh_models_with(
    sign_in: super::chatgpt_web_instance::SignIn,
) -> Result<Vec<ChatGptWebModel>> {
    tokio::spawn(async move {
        let _session = lock_session().await;
        record_refresh(async move {
            let (instance, page) =
                super::chatgpt_web_instance::open_signed_in_page(sign_in).await?;
            let tab = InstanceTab {
                instance: &instance,
                page: &page,
            };
            let result = discover_on(&tab).await;
            let closed = instance.close_page(&page).await;
            match (result, closed) {
                (Ok(snapshot), Ok(())) => Ok(snapshot),
                (Err(error), _) => Err(error),
                (Ok(_), Err(error)) => {
                    Err(error.context("Failed to close the owned ChatGPT discovery tab"))
                }
            }
        })
        .await
    })
    .await
    .context("ChatGPT catalog discovery task failed")?
}

/// Background refresh. Never opens a visible window.
pub async fn refresh_models() -> Result<Vec<ChatGptWebModel>> {
    refresh_models_with(super::chatgpt_web_instance::SignIn::Never).await
}

/// Store a successful observation, or annotate the previous one with the
/// failure. A running or failed refresh never removes known availability.
async fn record_refresh(
    discovery: impl std::future::Future<Output = Result<Snapshot>>,
) -> Result<Vec<ChatGptWebModel>> {
    match discovery.await {
        Ok(snapshot) => {
            let models = snapshot_models(&snapshot);
            store_cache(snapshot);
            Ok(models)
        }
        Err(error) => {
            // Another jcode process (CLI or daemon) may have stored a newer
            // observation. Annotate the newest one instead of overwriting it.
            let memory = load_cache().snapshot;
            let disk = cache_path()
                .ok()
                .and_then(|path| crate::storage::read_json::<Snapshot>(&path).ok());
            let mut previous = match disk {
                Some(disk) if disk.version == 1 && disk.observed_at > memory.observed_at => disk,
                _ => memory,
            };
            if previous.version == 1 {
                previous.last_refresh_error = Some(refresh_error_summary(&error));
                store_cache(previous);
            }
            Err(error)
        }
    }
}

/// One owned ChatGPT page that discovery drives through `chatgpt_web_ui.js`.
#[async_trait::async_trait]
trait DiscoveryPage: Send + Sync {
    async fn ui(&self, request: serde_json::Value) -> Result<serde_json::Value>;
}

struct InstanceTab<'a> {
    instance: &'a super::chatgpt_web_instance::Instance,
    page: &'a super::chatgpt_web_instance::Page,
}

#[async_trait::async_trait]
impl DiscoveryPage for InstanceTab<'_> {
    async fn ui(&self, request: serde_json::Value) -> Result<serde_json::Value> {
        super::chatgpt_web_instance::ui_request(self.instance, self.page, &request).await
    }
}

/// Reads the account's model catalog, which lists every model with its
/// thinking levels and context window. Falls back to reading the picker if
/// the catalog cannot be read. Never sends a message.
async fn discover_on(page: &impl DiscoveryPage) -> Result<Snapshot> {
    use serde_json::json;
    match page
        .ui(json!({"op":"catalog"}))
        .await
        .and_then(parse_catalog)
    {
        Ok(snapshot) => return Ok(snapshot),
        Err(error) => crate::logging::warn(&format!(
            "ChatGPT model catalog unavailable, reading the picker instead: {error:#}"
        )),
    }
    discover_from_picker(page).await
}

/// Reads the picker's models, then probes each available model's Power
/// choices. Never sends a message. The UI script restores the original model
/// and Power after every probe.
async fn discover_from_picker(page: &impl DiscoveryPage) -> Result<Snapshot> {
    use serde_json::json;
    let mut discovered = page.ui(json!({"op":"discover"})).await?;
    // Validate labels and shape before using any page-derived probe input.
    parse_discovery(discovered.clone())?;
    let models = discovered
        .get_mut("models")
        .and_then(serde_json::Value::as_array_mut)
        .context("Browser discovery has no models array")?;
    for model in models {
        if model.get("available").and_then(serde_json::Value::as_bool) != Some(true) {
            continue;
        }
        let label = model
            .get("label")
            .and_then(serde_json::Value::as_str)
            .context("Browser model has no label")?;
        let probed = page.ui(json!({"op":"probe","label":label})).await?;
        model["reasoning_options"] = probed
            .get("reasoning_options")
            .cloned()
            .context("Browser reasoning probe has no choices")?;
    }
    parse_discovery(discovered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct HomeGuard {
        previous: Option<std::ffi::OsString>,
        _temp: tempfile::TempDir,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        fn new() -> Self {
            let lock = crate::storage::lock_test_env();
            let temp = tempfile::tempdir().unwrap();
            let previous = std::env::var_os("JCODE_HOME");
            crate::env::set_var("JCODE_HOME", temp.path());
            Self {
                previous,
                _temp: temp,
                _lock: lock,
            }
        }
    }
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => crate::env::set_var("JCODE_HOME", value),
                None => crate::env::remove_var("JCODE_HOME"),
            }
        }
    }

    #[test]
    fn chatgpt_web_legacy_fixed_ids_map_to_their_family_and_power() {
        assert_eq!(
            legacy_web_alias("gpt-6-thinking[web]"),
            Some(("gpt-6[web]", "medium"))
        );
        assert_eq!(
            legacy_web_alias(crate::provider::CHATGPT_WEB_MODEL),
            Some(("gpt-5.6-sol[web]", "max"))
        );
        assert_eq!(legacy_web_alias("gpt-6[web]"), None);
        for (legacy, (family, _)) in [
            "gpt-6-pro[web]",
            "gpt-6-instant[web]",
            "gpt-5.6-thinking[web]",
            "gpt-5.6-instant[web]",
        ]
        .into_iter()
        .filter_map(|id| legacy_web_alias(id).map(|alias| (id, alias)))
        {
            assert!(jcode_provider_core::is_chatgpt_web_model(legacy));
            assert_eq!(
                jcode_provider_core::normalize_chatgpt_web_model_label(
                    family.trim_end_matches("[web]")
                )
                .as_deref(),
                Some(family)
            );
        }
    }

    #[test]
    fn chatgpt_web_cache_restart_keeps_observed_models_and_drops_slug_mappings() {
        let _home = HomeGuard::new();
        assert!(cached_models().is_empty());
        let snapshot = parse_discovery(json!({"models":[{"label":"GPT-6","available":true,"detail":"","reasoning_options":[{"value":"none","label":"Instant","available":true}]}]})).unwrap();
        store_cache(snapshot);
        assert!(find_model("gpt-6[web]").unwrap().available);
        assert!(cached_reasoning_options("gpt-6[web]")[0].available);
        assert!(super::super::known_openai_model_ids().contains(&"gpt-6[web]".to_string()));
        let routes = super::super::simplified_model_routes_for_picker("OpenAI", "", Vec::new());
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].api_method, "chatgpt-web");
        assert!(routes[0].available);
        assert_eq!(
            super::super::openrouter_catalog_model_id("gpt-6[web]"),
            None
        );
        *CACHE.lock().unwrap() = Cache::default();
        assert!(find_model("gpt-6[web]").unwrap().available);
        assert!(cached_reasoning_options("gpt-6[web]")[0].available);
        assert!(find_model("gpt-6").is_none());
        let mut cache = load_cache().snapshot;
        cache.models[0]
            .expected_upstream_models
            .push("invented".to_string());
        store_cache(cache);
        assert!(
            find_model("gpt-6[web]")
                .unwrap()
                .expected_upstream_models
                .is_empty()
        );
        store_cache(Snapshot {
            version: 1,
            ..Default::default()
        });
        assert!(cached_models().is_empty());
    }

    #[tokio::test]
    #[ignore = "requires jcode's private ChatGPT browser profile to be signed in; reads the picker in an owned Temporary Chat tab and never sends a message"]
    async fn chatgpt_web_live_catalog_discovers_models_and_reasoning_without_messages() {
        let models = refresh_models().await.unwrap();
        assert!(!models.is_empty());
        for model in models.iter().filter(|model| model.available) {
            assert!(model.expected_upstream_models.is_empty());
            let options = cached_reasoning_options(&model.model);
            assert!(!options.is_empty(), "{}: {options:?}", model.model);
            assert!(
                options.iter().all(|option| option.available),
                "{}: {options:?}",
                model.model
            );
            eprintln!("{model:?}\nreasoning={options:?}");
        }
    }

    struct FakePage {
        ops: std::sync::Mutex<Vec<String>>,
        fail_probe: bool,
    }

    #[async_trait::async_trait]
    impl DiscoveryPage for FakePage {
        async fn ui(&self, request: serde_json::Value) -> Result<serde_json::Value> {
            let op = request["op"].as_str().unwrap().to_string();
            if op == "catalog" {
                // This fake exercises the picker fallback path.
                anyhow::bail!("catalog unavailable");
            }
            self.ops.lock().unwrap().push(op.clone());
            match op.as_str() {
                "discover" => Ok(json!({"models":[
                    {"label":"GPT-6","available":true,"detail":"","reasoning_options":[]},
                    {"label":"GPT-5.6 Sol","available":false,"detail":"Locked","reasoning_options":[]}
                ]})),
                "probe" => {
                    assert_eq!(request["label"], "GPT-6", "locked models are never probed");
                    if self.fail_probe {
                        anyhow::bail!("probe failed");
                    }
                    Ok(
                        json!({"reasoning_options":[{"value":"none","label":"Instant","available":true}]}),
                    )
                }
                other => panic!("unexpected discovery op {other}"),
            }
        }
    }

    #[tokio::test]
    async fn chatgpt_web_prefetch_without_native_provider_discovers_catalog() {
        let page = FakePage {
            ops: Default::default(),
            fail_probe: false,
        };
        let mut observed = None;
        super::super::prefetch_openai_or_web_catalog(None, async {
            observed = Some(discover_on(&page).await?);
            Ok(())
        })
        .await
        .unwrap();
        let snapshot = observed.expect("browser-only catalog refresh must run without native auth");
        assert_eq!(snapshot.models[0].model, "gpt-6[web]");
        assert!(snapshot.models[0].available);
        assert!(!snapshot.models[1].available);
    }

    #[tokio::test]
    async fn chatgpt_web_discovery_probes_each_available_model_and_fails_closed_on_probe_error() {
        for fail_probe in [true, false] {
            let page = FakePage {
                ops: Default::default(),
                fail_probe,
            };
            let result = discover_on(&page).await;
            assert_eq!(*page.ops.lock().unwrap(), ["discover", "probe"]);
            if fail_probe {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap().reasoning["gpt-6[web]"][0].value, "none");
            }
        }
    }

    fn observed_gpt6() -> Snapshot {
        parse_discovery(json!({"models":[{"label":"GPT-6","available":true,"detail":"","reasoning_options":[{"value":"medium","label":"Medium","available":true}]}]})).unwrap()
    }

    #[tokio::test]
    async fn chatgpt_web_failed_or_running_refresh_keeps_last_observed_availability() {
        let _home = HomeGuard::new();
        record_refresh(async { Ok(observed_gpt6()) }).await.unwrap();
        let error = record_refresh(async {
            // A slow live discovery must not hide routes while it runs.
            assert!(find_model("gpt-6[web]").unwrap().available);
            anyhow::bail!("Evaluate error: ChatGPT did not finish opening the model picker")
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains("model picker"));
        let model = find_model("gpt-6[web]").unwrap();
        assert!(model.available, "{model:?}");
        assert!(model.detail.contains("last refresh failed"), "{model:?}");
        assert!(cached_reasoning_options("gpt-6[web]")[0].available);
        let routes = super::super::simplified_model_routes_for_picker("OpenAI", "", Vec::new());
        assert!(routes[0].available);
        record_refresh(async { Ok(observed_gpt6()) }).await.unwrap();
        assert!(
            !find_model("gpt-6[web]")
                .unwrap()
                .detail
                .contains("refresh failed")
        );
    }

    #[test]
    fn chatgpt_web_last_observed_availability_survives_age_and_restart() {
        let _home = HomeGuard::new();
        let mut snapshot = observed_gpt6();
        snapshot.observed_at = now_secs() - 24 * 60 * 60;
        store_cache(snapshot);
        *CACHE.lock().unwrap() = Cache::default();
        assert!(find_model("gpt-6[web]").unwrap().available);
        assert!(cached_reasoning_options("gpt-6[web]")[0].available);
    }

    #[test]
    fn chatgpt_web_discovery_preserves_picker_labels_and_per_model_efforts() {
        let result = parse_discovery(json!({"models":[
            {"label":"GPT-6","available":true,"detail":"", "reasoning_options":[
                {"value":"none","label":"Instant","available":true},
                {"value":"max","label":"Pro","available":true}]},
            {"label":"GPT-5.6 Sol","available":true,"detail":"", "reasoning_options":[
                {"value":"medium","label":"Medium","available":true}]},
            {"label":"GPT-5.5","available":true,"detail":"Leaving on October 14", "reasoning_options":[]}
        ]})).unwrap();
        assert_eq!(
            result
                .models
                .iter()
                .map(|m| m.model.as_str())
                .collect::<Vec<_>>(),
            ["gpt-6[web]", "gpt-5.6-sol[web]", "gpt-5.5[web]"]
        );
        assert_eq!(result.models[2].detail, "Leaving on October 14");
        assert!(
            result
                .models
                .iter()
                .all(|m| m.expected_upstream_models.is_empty())
        );
        assert_eq!(result.reasoning["gpt-6[web]"].len(), 2);
        assert_eq!(result.reasoning["gpt-5.6-sol[web]"].len(), 1);
    }

    #[test]
    fn chatgpt_web_discovery_rejects_malformed_labels_and_efforts() {
        assert!(parse_discovery(json!({"models":[{"label":"GPT-6/unsafe","available":true,"detail":"","reasoning_options":[]}]})).is_err());
        assert!(parse_discovery(json!({"models":[{"label":"GPT-6","available":true,"detail":"","reasoning_options":[{"value":"max","label":"Instant","available":true}]}]})).is_err());
        assert!(parse_discovery(json!({"models":[{"label":"GPT-6","available":true,"detail":"","reasoning_options":[]},{"label":"GPT-6","available":false,"detail":"disabled","reasoning_options":[]}]})).is_err());
    }

    #[test]
    fn chatgpt_web_snapshot_keeps_observed_availability_and_rejects_unknown_versions() {
        let mut snapshot = parse_discovery(json!({"models":[
            {"label":"GPT-6","available":true,"detail":"","reasoning_options":[{"value":"none","label":"Instant","available":true}]},
            {"label":"GPT-5.6 Sol","available":false,"detail":"Locked","reasoning_options":[]}
        ]})).unwrap();
        snapshot.observed_at = 100;
        let models = snapshot_models(&snapshot);
        assert!(models[0].available);
        // Picker-observed locks stay locked; persistence never grants access.
        assert!(!models[1].available);
        assert!(
            snapshot_models(&Snapshot {
                version: 99,
                ..snapshot.clone()
            })
            .is_empty()
        );
        snapshot.last_refresh_error = Some("bridge failed".to_string());
        let models = snapshot_models(&snapshot);
        assert!(models[0].available);
        assert_eq!(models[0].detail, "last refresh failed: bridge failed");
        assert_eq!(
            models[1].detail,
            "Locked · last refresh failed: bridge failed"
        );
        assert!(snapshot.reasoning["gpt-6[web]"][0].available);
    }

    fn catalog_fixture() -> serde_json::Value {
        let thinking = |slug: &str, title: &str, ctx: usize| {
            json!({"slug":slug,"title":title,"reasoningType":"reasoning","contextTokens":ctx,"workMode":false,
            "efforts":[{"value":"min"},{"value":"standard"},{"value":"extended"},{"value":"max"}],"tools":["tools","search","canvas"],"imageInput":true})
        };
        let plain = |slug: &str, title: &str, kind: &str, ctx: usize| json!({"slug":slug,"title":title,"reasoningType":kind,"contextTokens":ctx,"workMode":false,"efforts":[],"tools":["search"],"imageInput":true});
        json!({
            "versions":[
                {"id":"latest","title":"GPT-6","presets":[
                    {"title":"Instant","slug":"gpt-6-instant","effort":null,"lane":"instant"},
                    {"title":"Medium","slug":"gpt-6-thinking","effort":"standard","lane":"thinking"},
                    {"title":"High","slug":"gpt-6-thinking","effort":"extended","lane":"thinking"},
                    {"title":"Extra High","slug":"gpt-6-thinking","effort":"max","lane":"thinking"},
                    {"title":"Pro","slug":"gpt-6-pro","effort":null,"lane":"pro"}]},
                {"id":"5.6","title":"GPT-5.6 Sol","presets":[
                    {"title":"Instant","slug":"gpt-5-6-instant","effort":null,"lane":"instant"},
                    {"title":"Medium","slug":"gpt-5-6-thinking","effort":"standard","lane":"thinking"}]}],
            "models":[
                plain("gpt-6","GPT-6","auto",52815),
                plain("gpt-6-instant","GPT-6","none",52815),
                thinking("gpt-6-thinking","GPT-6",262144),
                {"slug":"gpt-6-pro","title":"GPT-6 Pro","reasoningType":"pro","contextTokens":410000,"workMode":false,"efforts":[{"value":"standard"}],"tools":["search"],"imageInput":true},
                plain("gpt-6-mini","GPT-6","none",137000),
                plain("gpt-6-t-mini","GPT-6","reasoning",262144),
                plain("gpt-5-6-instant","GPT-5.6 Sol","none",52815),
                thinking("gpt-5-6-thinking","GPT-5.6 Sol",262144),
                plain("gpt-5-3-mini","GPT-5.3 Mini","none",52815),
                {"slug":"gpt-6.1-sol-wm","title":"GPT-6.1 Sol","reasoningType":"reasoning","contextTokens":262144,"workMode":true,"efforts":[{"value":"min"}],"tools":[],"imageInput":true},
                plain("research","Deep Research","none",34815)
            ]
        })
    }

    #[test]
    fn catalog_routes_cover_every_served_model_and_thinking_level() {
        let snapshot = parse_catalog(catalog_fixture()).unwrap();
        let routes: Vec<_> = snapshot.models.iter().map(|m| m.model.as_str()).collect();
        // Existing route IDs stay stable; minis appear; work-mode, auto,
        // research and models that fail in Temporary Chat do not.
        assert_eq!(
            routes,
            [
                "gpt-6[web]",
                "gpt-5.6-sol[web]",
                "gpt-6-mini[web]",
                "gpt-6-thinking-mini[web]",
            ]
        );
        let gpt6: Vec<_> = snapshot.reasoning["gpt-6[web]"]
            .iter()
            .map(|o| {
                (
                    o.value.as_str(),
                    o.label.as_str(),
                    o.upstream.as_deref().unwrap(),
                    o.context_tokens.unwrap(),
                )
            })
            .collect();
        assert_eq!(
            gpt6,
            [
                ("none", "Instant", "gpt-6-instant", 52815),
                ("low", "Light", "gpt-6-thinking:min", 262144),
                ("medium", "Medium", "gpt-6-thinking:standard", 262144),
                ("high", "High", "gpt-6-thinking:extended", 262144),
                ("xhigh", "Extra High", "gpt-6-thinking:max", 262144),
                ("max", "Pro", "gpt-6-pro:standard", 410000),
            ]
        );
        let gpt6_model = &snapshot.models[0];
        assert_eq!(gpt6_model.slug.as_deref(), Some("gpt-6-thinking"));
        assert_eq!(gpt6_model.context_tokens, Some(262144));
        assert_eq!(
            gpt6_model.detail,
            "52K-410K context · Pro runs in the background"
        );
        assert_eq!(
            snapshot.models[2].detail,
            "ChatGPT model gpt-6-mini · 137K context"
        );
        assert!(gpt6_model.image_input);
        assert!(gpt6_model.capabilities.contains(&"web search".to_string()));
        // A family without Pro in its presets does not invent one.
        assert!(
            !snapshot.reasoning["gpt-5.6-sol[web]"]
                .iter()
                .any(|o| o.value == "max")
        );
        assert!(
            snapshot.reasoning["gpt-5.6-sol[web]"]
                .iter()
                .any(|o| o.value == "low" && o.upstream.as_deref() == Some("gpt-5-6-thinking:min"))
        );
    }

    #[test]
    fn catalog_request_routes_and_context_follow_the_selected_effort() {
        let _home = HomeGuard::new();
        store_cache(parse_catalog(catalog_fixture()).unwrap());
        assert_eq!(
            request_route("gpt-6[web]", Some("low")),
            Some(("gpt-6-thinking".to_string(), Some("min".to_string())))
        );
        assert_eq!(
            request_route("gpt-6[web]", Some("none")),
            Some(("gpt-6-instant".to_string(), None))
        );
        assert_eq!(
            request_route("gpt-6[web]", Some("max")),
            Some(("gpt-6-pro".to_string(), Some("standard".to_string())))
        );
        // Unknown or missing efforts fall back to Medium, never to Pro.
        assert_eq!(
            request_route("gpt-6[web]", None),
            Some(("gpt-6-thinking".to_string(), Some("standard".to_string())))
        );
        assert_eq!(
            request_route("gpt-6-mini[web]", Some("high")),
            Some(("gpt-6-mini".to_string(), None))
        );
        assert_eq!(context_tokens("gpt-6[web]", Some("none")), Some(52815));
        assert_eq!(context_tokens("gpt-6[web]", Some("high")), Some(262144));
        assert_eq!(context_tokens("gpt-6[web]", Some("max")), Some(410000));
        // The shared resolver (remote panels, compaction) uses the catalog,
        // not the GPT-6 API family's 1M fallback.
        assert_eq!(
            crate::provider::context_limit_for_model_with_provider("gpt-6[web]", Some("OpenAI")),
            Some(262144)
        );
        assert_eq!(
            crate::provider::context_limit_for_model_with_provider(
                "gpt-6-mini[web]",
                Some("OpenAI")
            ),
            Some(137000)
        );
        assert_eq!(request_route("gpt-9[web]", None), None);
        // Picker-only snapshots (old caches) have no request routes.
        store_cache(observed_gpt6());
        assert_eq!(request_route("gpt-6[web]", Some("medium")), None);
    }

    #[test]
    fn catalog_rejects_unsafe_slugs_and_empty_catalogs() {
        let mut bad = catalog_fixture();
        bad["models"][2]["slug"] = json!("gpt-6-thinking\"; drop");
        bad["versions"][0]["presets"][1]["slug"] = json!("gpt-6-thinking\"; drop");
        let snapshot = parse_catalog(bad).unwrap();
        assert!(
            snapshot.reasoning["gpt-6[web]"].iter().all(|o| !o
                .upstream
                .as_deref()
                .unwrap()
                .contains('"'))
        );
        assert!(parse_catalog(json!({"versions":[],"models":[]})).is_err());
    }

    #[tokio::test]
    async fn discovery_prefers_the_catalog_and_falls_back_to_the_picker() {
        struct CatalogPage {
            catalog_ok: bool,
            ops: std::sync::Mutex<Vec<String>>,
        }
        #[async_trait::async_trait]
        impl DiscoveryPage for CatalogPage {
            async fn ui(&self, request: serde_json::Value) -> Result<serde_json::Value> {
                let op = request["op"].as_str().unwrap().to_string();
                self.ops.lock().unwrap().push(op.clone());
                match op.as_str() {
                    "catalog" if self.catalog_ok => Ok(catalog_fixture()),
                    "catalog" => anyhow::bail!("HTTP 403"),
                    "discover" => Ok(
                        json!({"models":[{"label":"GPT-6","available":true,"detail":"","reasoning_options":[]}]}),
                    ),
                    "probe" => Ok(
                        json!({"reasoning_options":[{"value":"none","label":"Instant","available":true}]}),
                    ),
                    other => panic!("unexpected {other}"),
                }
            }
        }
        let page = CatalogPage {
            catalog_ok: true,
            ops: Default::default(),
        };
        let snapshot = discover_on(&page).await.unwrap();
        assert_eq!(
            *page.ops.lock().unwrap(),
            ["catalog"],
            "no picker clicks when the catalog works"
        );
        assert_eq!(snapshot.models.len(), 4);
        let page = CatalogPage {
            catalog_ok: false,
            ops: Default::default(),
        };
        let snapshot = discover_on(&page).await.unwrap();
        assert_eq!(*page.ops.lock().unwrap(), ["catalog", "discover", "probe"]);
        assert_eq!(snapshot.models[0].model, "gpt-6[web]");
    }

    #[tokio::test]
    async fn chatgpt_web_session_lock_serializes_runtime_and_discovery() {
        let guard = lock_session().await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), lock_session())
                .await
                .is_err()
        );
        drop(guard);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), lock_session())
                .await
                .is_ok()
        );
    }
}
