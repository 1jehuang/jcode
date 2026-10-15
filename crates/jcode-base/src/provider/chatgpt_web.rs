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
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatGptWebReasoningOption {
    pub value: String,
    pub label: String,
    pub available: bool,
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
        });
    }
    Ok(snapshot)
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

/// Remove non-fatal dynamic loader warnings from bridge stderr. A bridge built
/// against a newer glibc prints a `weak version` line on every call, which
/// otherwise hides the actual browser error behind a misleading libc message.
pub fn bridge_error_detail(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !(line.contains(": weak version `")
                    && line.contains("' not found (required by "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn now_secs() -> u64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_secs(),
        Err(error) => {
            crate::logging::warn(&format!(
                "ChatGPT catalog clock is before the Unix epoch, using zero timestamp: {error}"
            ));
            0
        }
    }
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

#[path = "chatgpt_web_cache.rs"]
mod cache;
use cache::{best_effort_cache_path, load_cache, read_cached_snapshot, store_cache};

/// Models from the last successful picker observation. Their availability
/// decides which routes can be selected. Every browser turn still proves the
/// exact model and Power choice in the live page before sending.
pub fn cached_models() -> Vec<ChatGptWebModel> {
    snapshot_models(&load_cache().snapshot)
}

/// Test helper for sibling modules: record one available picker model as if a
/// browser session had been observed, so web routes can be exercised without
/// a live bridge.
#[cfg(test)]
pub(crate) fn seed_observed_model_for_tests(label: &str) {
    let snapshot = parse_discovery(serde_json::json!({
        "models": [{
            "label": label,
            "available": true,
            "detail": "",
            "reasoning_options": []
        }]
    }))
    .expect("valid test discovery");
    store_cache(snapshot);
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
        .filter(|option| reasoning_value(&option.label) == Some(option.value.as_str()))
        .cloned()
        .map(|mut option| {
            option.available &= available;
            option
        })
        .collect()
}

/// Refresh through an existing installed bridge only. The spawned task owns
/// the session lock and cleanup even if the caller drops its prefetch future.
pub async fn refresh_models() -> Result<Vec<ChatGptWebModel>> {
    let result = tokio::spawn(async {
        let _session = lock_session().await;
        record_refresh(async {
            let initial = InstalledBridge { browser: None };
            let ping = initial.command("ping", serde_json::json!({})).await?;
            anyhow::ensure!(
                ping.get("pong").is_some(),
                "Installed browser bridge is not connected"
            );
            let browser = ping
                .get("browser")
                .and_then(serde_json::Value::as_str)
                .filter(|b| !b.is_empty())
                .context("Connected bridge did not identify its browser")?;
            let bridge = InstalledBridge {
                browser: Some(browser.to_string()),
            };
            discover_with_bridge(&bridge).await
        })
        .await
    })
    .await
    .context("ChatGPT catalog discovery task failed")?;
    if let Err(error) = &result
        && needs_user_attention(error)
        && let Err(login_error) = show_chatgpt_for_login().await
    {
        crate::logging::warn(&format!(
            "Failed to show ChatGPT login after catalog refresh failure: {login_error:#}"
        ));
    }
    result
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
            let disk = best_effort_cache_path()
                .as_deref()
                .and_then(read_cached_snapshot);
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

#[async_trait::async_trait]
trait BridgeTransport: Send + Sync {
    async fn command(&self, action: &str, params: serde_json::Value) -> Result<serde_json::Value>;
}

struct InstalledBridge {
    browser: Option<String>,
}

#[async_trait::async_trait]
impl BridgeTransport for InstalledBridge {
    async fn command(&self, action: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let binary = crate::browser::browser_binary_path();
        anyhow::ensure!(
            binary.exists(),
            "ChatGPT model discovery requires an already installed browser bridge"
        );
        let mut command = tokio::process::Command::new(binary);
        command
            .arg(action)
            .arg(serde_json::to_string(&params)?)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        crate::browser::apply_bridge_browser_env(&mut command, self.browser.as_deref());
        let output = tokio::time::timeout(std::time::Duration::from_secs(120), command.output())
            .await
            .with_context(|| format!("Browser discovery {action} timed out"))??;
        anyhow::ensure!(
            output.status.success(),
            "Browser discovery {action} failed: {} {}",
            bridge_error_detail(&String::from_utf8_lossy(&output.stderr)),
            String::from_utf8_lossy(&output.stdout).trim()
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)
            .with_context(|| format!("Browser discovery {action} returned invalid JSON"))?;
        anyhow::ensure!(
            value.get("error").is_none()
                && value.get("ok") != Some(&serde_json::Value::Bool(false)),
            "Browser discovery {action} failed: {value}"
        );
        Ok(value)
    }
}

/// Marker URL of the unfocused window jcode keeps for ChatGPT web work. Forks
/// of this tab are created and activated inside that window, so turns never
/// switch or raise a tab in the user's own windows.
pub const BACKGROUND_ANCHOR_URL: &str = "about:blank#jcode-chatgpt-web";

async fn background_anchor_with_bridge(bridge: &impl BridgeTransport) -> Result<u64> {
    use serde_json::{Value, json};
    let listing = bridge.command("listTabs", json!({})).await?;
    let existing = listing
        .get("windows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|window| window.get("tabs").and_then(Value::as_array))
        .flatten()
        .find(|tab| tab.get("url").and_then(Value::as_str) == Some(BACKGROUND_ANCHOR_URL))
        .and_then(|tab| tab.get("tabId").and_then(Value::as_u64));
    if let Some(tab_id) = existing {
        return Ok(tab_id);
    }
    let created = bridge
        .command(
            "newSession",
            json!({"url": BACKGROUND_ANCHOR_URL, "window": true, "focus": false, "wait": false}),
        )
        .await
        .context("Failed to open jcode's background ChatGPT window")?;
    // newSession makes its tab the bridge's default target; hand that back.
    if let Some(previous) = listing.get("activeTabId").and_then(Value::as_u64)
        && let Err(error) = bridge
            .command("setActiveTab", json!({"tabId": previous}))
            .await
    {
        crate::logging::warn(&format!(
            "Failed to restore browser bridge target after opening ChatGPT background window: {error:#}"
        ));
    }
    created
        .get("tabId")
        .and_then(Value::as_u64)
        .context("Browser bridge did not return the background ChatGPT tab")
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

/// Tab that ChatGPT web turns fork from, in jcode's own unfocused window.
pub async fn background_anchor_tab(browser: &str) -> Result<u64> {
    background_anchor_with_bridge(&InstalledBridge {
        browser: Some(browser.to_string()),
    })
    .await
}

/// True when ChatGPT is waiting on the user (signed out, or an access or
/// onboarding prompt), the only time jcode shows the browser.
pub fn needs_user_attention(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}");
    text.contains("ChatGPT is signed out")
        || text.contains("waiting for an onboarding or access choice")
}

static LOGIN_SHOWN_AT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// Bring chatgpt.com to the front so the user can log in. At most once every
/// five minutes, so retries do not keep opening tabs.
pub async fn show_chatgpt_for_login() -> Result<()> {
    {
        let mut shown = LOGIN_SHOWN_AT.lock().unwrap_or_else(|p| p.into_inner());
        if shown.is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(300)) {
            return Ok(());
        }
        *shown = Some(std::time::Instant::now());
    }
    let bridge = InstalledBridge { browser: None };
    let opened = bridge
        .command(
            "newSession",
            serde_json::json!({"url": "https://chatgpt.com/", "focus": true}),
        )
        .await?;
    if let Some(tab_id) = opened.get("tabId").and_then(serde_json::Value::as_u64) {
        bridge
            .command(
                "setActiveTab",
                serde_json::json!({"tabId": tab_id, "focus": true}),
            )
            .await?;
    }
    Ok(())
}

const UI_EXECUTION_READINESS_SCRIPT: &str =
    "return {hostname:location.hostname, ready:document.readyState === 'complete'};";

/// Prove read-only execution readiness in an owned ChatGPT tab.
pub async fn wait_for_ui_execution(tab_id: u64, browser: &str) -> Result<()> {
    wait_for_ui_execution_with_bridge(
        &InstalledBridge {
            browser: Some(browser.to_string()),
        },
        tab_id,
    )
    .await
}

async fn wait_for_ui_execution_with_bridge(
    bridge: &impl BridgeTransport,
    tab_id: u64,
) -> Result<()> {
    use std::time::Duration;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut last_failure = String::from("document not complete");
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        anyhow::ensure!(
            !remaining.is_zero(),
            "ChatGPT read-only execution readiness timed out: {last_failure}"
        );
        // Never abandon an attempt early. A dropped evaluate keeps running in
        // the browser, and starting another on the same tab overlaps debugger
        // sessions: the first detach kills both with "Detached while
        // handling command." Only the overall deadline may cut one short.
        match tokio::time::timeout(
            remaining,
            bridge.command(
                "evaluate",
                serde_json::json!({"tabId":tab_id,"script":UI_EXECUTION_READINESS_SCRIPT}),
            ),
        )
        .await
        {
            Ok(Ok(value)) => {
                let proof = value
                    .get("result")
                    .context("Malformed ChatGPT execution readiness proof: missing result")?;
                anyhow::ensure!(
                    proof.get("hostname").and_then(serde_json::Value::as_str)
                        == Some("chatgpt.com"),
                    "Rejected ChatGPT execution readiness origin proof: {proof}"
                );
                let ready = proof
                    .get("ready")
                    .and_then(serde_json::Value::as_bool)
                    .context(
                        "Malformed ChatGPT execution readiness proof: ready must be boolean",
                    )?;
                if ready {
                    return Ok(());
                }
                last_failure = "document not complete".to_string();
            }
            Ok(Err(error)) => last_failure = format!("{error:#}"),
            Err(_) => last_failure = "read-only evaluate attempt timed out".to_string(),
        }
        tokio::time::sleep_until(
            (tokio::time::Instant::now() + Duration::from_millis(500)).min(deadline),
        )
        .await;
    }
}

async fn discover_with_bridge(bridge: &impl BridgeTransport) -> Result<Snapshot> {
    use serde_json::json;
    let source_id = background_anchor_with_bridge(bridge).await?;
    let fork_name = format!("jcode-chatgpt-catalog-{}", uuid::Uuid::new_v4());
    let mut owned_tab = None;
    let result = async {
        let fork = bridge.command("fork", json!({"tabId":source_id,"paths":[{"name":fork_name}]})).await?;
        let tab_id = fork.get("forks").and_then(serde_json::Value::as_array)
            .and_then(|forks|forks.first()).and_then(|fork|fork.get("tabId"))
            .and_then(serde_json::Value::as_u64).context("Browser bridge did not return owned fork tab ID")?;
        owned_tab = Some(tab_id);
        bridge.command("navigate", json!({"tabId":tab_id,"url":"https://chatgpt.com/?temporary-chat=true","wait":true})).await?;
        bridge.command("waitFor", json!({
            "tabId":tab_id,
            "selector":"[contenteditable=true][data-composer-markdown], [contenteditable=true][aria-label='Chat with ChatGPT']",
            "timeout":20_000
        })).await.context("ChatGPT composer did not load in the owned temporary fork")?;
        wait_for_ui_execution_with_bridge(bridge, tab_id).await?;
        let script = format!("const request={};\n{}", json!({"op":"discover"}), UI_SCRIPT);
        let evaluated = bridge.command("evaluate", json!({"tabId":tab_id,"script":script})).await?;
        let mut discovered = evaluated.get("result").cloned().context("Browser discovery has no result")?;
        // Validate labels and shape before using any page-derived probe input.
        parse_discovery(discovered.clone())?;
        let models = discovered.get_mut("models").and_then(serde_json::Value::as_array_mut)
            .context("Browser discovery has no models array")?;
        for model in models {
            if model.get("available").and_then(serde_json::Value::as_bool) != Some(true) { continue; }
            let label = model.get("label").and_then(serde_json::Value::as_str).context("Browser model has no label")?;
            // Each evaluate must fit the installed bridge's own 30-second cap.
            // JS restores the original model/effort after each individual probe.
            let script = format!("const request={};\n{}", json!({"op":"probe","label":label}), UI_SCRIPT);
            let probed = bridge.command("evaluate", json!({"tabId":tab_id,"script":script})).await?;
            let options = probed.get("result").and_then(|result|result.get("reasoning_options"))
                .cloned().context("Browser reasoning probe has no choices")?;
            model["reasoning_options"] = options;
        }
        parse_discovery(discovered)
    }.await;
    // The fork command may create a resource even if its reply is malformed.
    let cleanup = bridge.command("killFork", json!({"fork":fork_name})).await;
    match (result, cleanup) {
        (Ok(snapshot), Ok(_)) => Ok(snapshot),
        (Err(error), Ok(_)) => Err(error),
        (result, Err(cleanup_error)) => {
            let blank = if let Some(tab_id) = owned_tab {
                bridge
                    .command(
                        "navigate",
                        json!({"tabId":tab_id,"url":"about:blank","wait":true}),
                    )
                    .await
            } else {
                Err(anyhow::anyhow!(
                    "Owned fork ID unknown, cannot blank fallback"
                ))
            };
            let original = match result {
                Err(error) => format!("Discovery also failed: {error:#}. "),
                Ok(_) => String::new(),
            };
            anyhow::bail!(
                "{original}Failed to close owned ChatGPT discovery fork: {cleanup_error:#}. Blank fallback: {}",
                match blank {
                    Ok(_) => "cleared owned tab".to_string(),
                    Err(e) => format!("failed: {e:#}"),
                }
            );
        }
    }
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
    #[ignore = "requires signed-in installed browser bridge, changes only an owned empty temporary fork"]
    async fn chatgpt_web_live_catalog_discovers_models_and_reasoning_without_messages() {
        let models = refresh_models().await.unwrap();
        for expected in ["gpt-6[web]", "gpt-5.6-sol[web]", "gpt-5.5[web]"] {
            let model = models
                .iter()
                .find(|model| model.model == expected)
                .expect("observed picker model");
            assert!(model.available, "{model:?}");
            assert!(model.expected_upstream_models.is_empty());
            let options = cached_reasoning_options(expected);
            assert_eq!(
                options
                    .iter()
                    .map(|option| option.value.as_str())
                    .collect::<Vec<_>>(),
                ["none", "medium", "high", "xhigh", "max"],
                "{expected}: {options:?}"
            );
            assert!(
                options.iter().all(|option| option.available),
                "{expected}: {options:?}"
            );
            eprintln!("{model:?}\nreasoning={options:?}");
        }
    }

    struct FakeBridge {
        actions: std::sync::Mutex<Vec<String>>,
        fail: &'static str,
    }

    #[async_trait::async_trait]
    impl BridgeTransport for FakeBridge {
        async fn command(
            &self,
            action: &str,
            params: serde_json::Value,
        ) -> Result<serde_json::Value> {
            self.actions.lock().unwrap().push(action.to_string());
            if action == self.fail {
                anyhow::bail!("injected {action} failure");
            }
            Ok(match action {
                "listTabs" => json!({"activeTabId":3,"windows":[{"windowId":1,"focused":true,
                    "tabs":[{"tabId":3,"url":"https://example.com/","active":true}]}]}),
                "newSession" => {
                    assert_eq!(params["url"], BACKGROUND_ANCHOR_URL);
                    assert_eq!(params["window"], true);
                    assert_eq!(params["focus"], false);
                    json!({"tabId":7})
                }
                "fork" => {
                    assert_eq!(params["tabId"], 7, "turns fork jcode's background tab");
                    json!({"forks":[{"tabId":8}]})
                }
                "waitFor" => {
                    assert_eq!(params["timeout"], 20_000);
                    assert!(
                        params["selector"]
                            .as_str()
                            .unwrap()
                            .contains("data-composer-markdown")
                    );
                    json!({"ok":true})
                }
                "setActiveTab" => {
                    // Only restores the bridge's default tab, never raises a window.
                    assert_eq!(params["tabId"], 3);
                    assert!(params.get("focus").is_none());
                    json!({"tabId":3})
                }
                "navigate" => {
                    assert!(
                        params["url"] == "https://chatgpt.com/?temporary-chat=true"
                            || params["url"] == "about:blank"
                    );
                    json!({"ok":true})
                }
                "evaluate" if params["script"] == UI_EXECUTION_READINESS_SCRIPT => {
                    let attempts = self
                        .actions
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|a| a.as_str() == "evaluate")
                        .count();
                    match self.fail {
                        "readiness_once" if attempts == 1 => anyhow::bail!("transient CSP blocked"),
                        "readiness_pending" => std::future::pending::<serde_json::Value>().await,
                        "readiness_slow" if attempts == 1 => {
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                            json!({"result":{"hostname":"chatgpt.com","ready":true}})
                        }
                        "readiness_incomplete" if attempts == 1 => {
                            json!({"result":{"hostname":"chatgpt.com","ready":false}})
                        }
                        "readiness_missing" => json!({"ok":true}),
                        "readiness_permanent" => anyhow::bail!("CSP blocked"),
                        "readiness_origin" => {
                            json!({"result":{"hostname":"example.com","ready":true}})
                        }
                        "readiness_malformed" => {
                            json!({"result":{"hostname":"chatgpt.com","ready":"true"}})
                        }
                        _ => json!({"result":{"hostname":"chatgpt.com","ready":true}}),
                    }
                }
                "evaluate" if self.fail.starts_with("probe") => {
                    let script = params["script"].as_str().unwrap();
                    if script.contains("\"op\":\"probe\"") {
                        assert!(script.contains("\"label\":\"GPT-6\""));
                        if self.fail == "probe" {
                            anyhow::bail!("probe failed");
                        }
                        json!({"result":{"reasoning_options":[{"value":"none","label":"Instant","available":true}]}})
                    } else {
                        json!({"result":{"models":[{"label":"GPT-6","available":true,"detail":"","reasoning_options":[]}]}})
                    }
                }
                "evaluate" => json!({"result":{"models":[]}}),
                "killFork" => json!({"ok":true}),
                _ => panic!("unexpected discovery action {action}"),
            })
        }
    }

    #[tokio::test]
    async fn chatgpt_web_prefetch_without_native_provider_discovers_catalog() {
        let bridge = FakeBridge {
            actions: Default::default(),
            fail: "probe_success",
        };
        let mut observed = None;
        super::super::prefetch_openai_or_web_catalog(None, async {
            observed = Some(discover_with_bridge(&bridge).await?);
            Ok(())
        })
        .await
        .unwrap();
        let snapshot = observed.expect("browser-only catalog refresh must run without native auth");
        assert_eq!(snapshot.models[0].model, "gpt-6[web]");
        assert!(snapshot.models[0].available);
        assert_eq!(bridge.actions.lock().unwrap().last().unwrap(), "killFork");
    }

    struct ListingBridge(serde_json::Value, std::sync::Mutex<Vec<String>>);

    #[async_trait::async_trait]
    impl BridgeTransport for ListingBridge {
        async fn command(&self, action: &str, _: serde_json::Value) -> Result<serde_json::Value> {
            self.1.lock().unwrap().push(action.to_string());
            match action {
                "listTabs" => Ok(self.0.clone()),
                _ => anyhow::bail!("unexpected {action}"),
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

    #[tokio::test]
    async fn chatgpt_web_reuses_its_background_window_instead_of_opening_another() {
        let bridge = ListingBridge(
            json!({"activeTabId":3,"windows":[
                {"windowId":1,"tabs":[{"tabId":3,"url":"https://example.com/"}]},
                {"windowId":2,"tabs":[{"tabId":9,"url":BACKGROUND_ANCHOR_URL}]}]}),
            Default::default(),
        );
        assert_eq!(background_anchor_with_bridge(&bridge).await.unwrap(), 9);
        assert_eq!(*bridge.1.lock().unwrap(), ["listTabs"]);
    }

    #[tokio::test(start_paused = true)]
    async fn chatgpt_web_discovery_always_cleans_owned_fork() {
        for fail in [
            "",
            "navigate",
            "waitFor",
            "listTabs",
            "newSession",
            "evaluate",
            "killFork",
        ] {
            let bridge = FakeBridge {
                actions: Default::default(),
                fail,
            };
            let result = discover_with_bridge(&bridge).await;
            let actions = bridge.actions.lock().unwrap();
            if matches!(fail, "listTabs" | "newSession") {
                // No background tab, so no fork was created and none can leak.
                assert!(!actions.iter().any(|a| a == "fork"), "{fail}: {actions:?}");
            } else {
                assert!(
                    actions.iter().any(|a| a == "killFork"),
                    "{fail}: {actions:?}"
                );
            }
            assert_eq!(result.is_ok(), fail.is_empty());
            if fail.is_empty() {
                assert!(
                    actions.iter().position(|a| a == "newSession").unwrap()
                        < actions.iter().position(|a| a == "fork").unwrap()
                );
            }
            if fail == "killFork" {
                assert_eq!(actions.last().unwrap(), "navigate");
            }
        }
    }

    #[tokio::test]
    async fn chatgpt_web_discovery_probes_each_model_and_fails_closed_on_probe_error() {
        for fail in ["probe", "probe_success"] {
            let bridge = FakeBridge {
                actions: Default::default(),
                fail,
            };
            let result = discover_with_bridge(&bridge).await;
            let actions = bridge.actions.lock().unwrap();
            assert_eq!(
                actions.iter().filter(|a| a.as_str() == "evaluate").count(),
                3
            );
            assert_eq!(actions.last().unwrap(), "killFork");
            if fail == "probe" {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap().reasoning["gpt-6[web]"][0].value, "none");
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn chatgpt_web_readiness_recovers_before_discovery() {
        let bridge = FakeBridge {
            actions: Default::default(),
            fail: "readiness_once",
        };
        discover_with_bridge(&bridge).await.unwrap();
        let actions = bridge.actions.lock().unwrap();
        assert_eq!(
            actions.iter().filter(|a| a.as_str() == "evaluate").count(),
            3
        );
        assert_eq!(actions.last().unwrap(), "killFork");
    }

    #[tokio::test(start_paused = true)]
    async fn chatgpt_web_readiness_waits_for_complete_document() {
        let bridge = FakeBridge {
            actions: Default::default(),
            fail: "readiness_incomplete",
        };
        let started = tokio::time::Instant::now();
        wait_for_ui_execution_with_bridge(&bridge, 8).await.unwrap();
        assert_eq!(started.elapsed(), std::time::Duration::from_millis(500));
        assert_eq!(*bridge.actions.lock().unwrap(), ["evaluate", "evaluate"]);
    }

    #[tokio::test(start_paused = true)]
    async fn chatgpt_web_readiness_never_overlaps_a_slow_evaluate() {
        // A dropped evaluate keeps running in the browser. Starting another one
        // on the same tab overlaps debugger sessions, and the first detach
        // kills both with "Detached while handling command."
        let bridge = FakeBridge {
            actions: Default::default(),
            fail: "readiness_slow",
        };
        let started = tokio::time::Instant::now();
        wait_for_ui_execution_with_bridge(&bridge, 8).await.unwrap();
        assert_eq!(started.elapsed(), std::time::Duration::from_secs(5));
        assert_eq!(*bridge.actions.lock().unwrap(), ["evaluate"]);
    }

    #[tokio::test(start_paused = true)]
    async fn chatgpt_web_readiness_fails_closed_and_cleans_fork() {
        for fail in [
            "readiness_permanent",
            "readiness_pending",
            "readiness_origin",
            "readiness_malformed",
            "readiness_missing",
        ] {
            let bridge = FakeBridge {
                actions: Default::default(),
                fail,
            };
            let started = tokio::time::Instant::now();
            assert!(discover_with_bridge(&bridge).await.is_err(), "{fail}");
            let elapsed = started.elapsed();
            let actions = bridge.actions.lock().unwrap();
            assert_eq!(actions.last().unwrap(), "killFork");
            if matches!(fail, "readiness_permanent" | "readiness_pending") {
                assert_eq!(elapsed, std::time::Duration::from_secs(10));
                assert_eq!(
                    actions.iter().filter(|a| a.as_str() == "evaluate").count(),
                    if fail == "readiness_pending" { 1 } else { 20 }
                );
            } else {
                assert_eq!(
                    actions.iter().filter(|a| a.as_str() == "evaluate").count(),
                    1
                );
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
    fn chatgpt_web_bridge_errors_drop_dynamic_loader_noise() {
        let stderr = "/home/u/.jcode/browser/browser: /lib/x86_64-linux-gnu/libc.so.6: weak version `GLIBC_2.39' not found (required by /home/u/.jcode/browser/browser)\nError: Evaluate error: Detached while handling command.\n";
        assert_eq!(
            bridge_error_detail(stderr),
            "Error: Evaluate error: Detached while handling command."
        );
        assert_eq!(bridge_error_detail("Error: plain\n"), "Error: plain");
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
