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

/// Reads the picker's models, then probes each available model's Power
/// choices. Never sends a message. The UI script restores the original model
/// and Power after every probe.
async fn discover_on(page: &impl DiscoveryPage) -> Result<Snapshot> {
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
