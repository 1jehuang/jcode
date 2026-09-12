//! Runtime-owned model usage. Historical picker selections are read separately.
//!
//! Agent turns with persisted responses are recorded prospectively in SQLite. A turn can have
//! many tool continuations but contributes at most once to each serving route.
//! A dedicated delta event refreshes client caches without taking an Agent lock
//! or rebuilding the full provider catalog after every response.
use anyhow::Result;
use jcode_provider_core::{ModelRoute, ModelRouteApiMethod, Provider, ResolvedCredential};
use jcode_usage_types::ModelUsage;
use rusqlite::{Connection, params};
use serde::Deserialize;
use std::{collections::HashMap, path::Path, time::Duration};

type RouteKey = (String, String, String);

fn key(model: &str, provider: &str, api_method: &str) -> RouteKey {
    let provider = jcode_provider_core::normalize_model_route_provider_label(provider);
    let provider = match provider.as_str() {
        "claude" => "anthropic",
        "google" => "gemini",
        "awsbedrock" => "bedrock",
        "githubcopilot" | "copilotcode" => "copilot",
        other => other,
    };
    (
        model.trim().to_string(),
        provider.to_string(),
        jcode_provider_core::RuntimeKey::from_api_method(
            &ModelRouteApiMethod::parse(api_method),
            provider,
        )
        .stable_id(),
    )
}

fn open(path: &Path, now: u64) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(2))?;
    db.execute_batch("PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS tracking (id INTEGER PRIMARY KEY CHECK(id=1), started INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS turns (
          turn_id TEXT NOT NULL, model TEXT NOT NULL, provider TEXT NOT NULL,
          api_method TEXT NOT NULL, last_used INTEGER NOT NULL,
          PRIMARY KEY(turn_id, model, provider, api_method));
        CREATE INDEX IF NOT EXISTS turns_route ON turns(model, provider, api_method);")?;
    db.execute(
        "INSERT OR IGNORE INTO tracking(id, started) VALUES(1, ?1)",
        [now],
    )?;
    Ok(db)
}

fn path() -> Result<std::path::PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("model-usage-v1.sqlite3"))
}

fn now() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn record_at(
    db: &Connection,
    turn_id: &str,
    route: &ModelRoute,
    timestamp: u64,
) -> Result<ModelUsage> {
    let (model, provider, method) = key(&route.model, &route.provider, &route.api_method);
    db.execute(
        "INSERT INTO turns(turn_id, model, provider, api_method, last_used)
        VALUES(?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(turn_id, model, provider, api_method)
        DO UPDATE SET last_used = MAX(last_used, excluded.last_used)",
        params![turn_id, model, provider, method, timestamp],
    )?;
    let started = db.query_row("SELECT started FROM tracking WHERE id=1", [], |row| {
        row.get(0)
    })?;
    Ok(db.query_row("SELECT COUNT(*), MAX(last_used) FROM turns WHERE model=?1 AND provider=?2 AND api_method=?3",
        params![model, provider, method], |row| Ok(ModelUsage {
            count: row.get(0)?, last_used_unix_secs: row.get(1)?,
            tracking_started_unix_secs: Some(started), ..Default::default()
        }))?)
}

#[derive(Default, Deserialize)]
struct LegacyStore {
    version: u8,
    selections: HashMap<String, LegacyEntry>,
}
#[derive(Deserialize)]
struct LegacyEntry {
    count: u64,
    last_selected_unix_secs: u64,
}

fn legacy_selections(path: &Path) -> HashMap<RouteKey, ModelUsage> {
    let Ok(store) = crate::storage::read_json::<LegacyStore>(path) else {
        return HashMap::new();
    };
    if store.version != 1 {
        return HashMap::new();
    }
    let mut result: HashMap<RouteKey, ModelUsage> = HashMap::new();
    for (identity, entry) in store.selections {
        let parts: Vec<_> = identity.split('\u{1f}').collect();
        if parts.len() != 4 || entry.count == 0 {
            continue;
        }
        let usage = result.entry(key(parts[0], parts[1], parts[2])).or_default();
        usage.selection_count = usage.selection_count.saturating_add(entry.count);
        usage.last_selected_unix_secs = usage
            .last_selected_unix_secs
            .max(Some(entry.last_selected_unix_secs));
    }
    result
}

fn legacy() -> HashMap<RouteKey, ModelUsage> {
    crate::storage::app_config_dir()
        .ok()
        .map(|dir| legacy_selections(&dir.join("model_picker_usage.json")))
        .unwrap_or_default()
}

/// Add local usage to provider-owned catalog rows. Failure leaves usage unknown,
/// never fabricated zero. No transcript loading or historical inference occurs.
pub fn enrich_routes(routes: &mut [ModelRoute]) {
    let mut usage = legacy();
    let mut started: Option<u64> = None;
    let read = || -> Result<(u64, Vec<(RouteKey, u64, Option<u64>)>)> {
        let db = Connection::open_with_flags(path()?, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        db.busy_timeout(Duration::from_secs(2))?;
        let started = db.query_row("SELECT started FROM tracking WHERE id=1", [], |row| {
            row.get(0)
        })?;
        let mut query = db.prepare("SELECT model, provider, api_method, COUNT(*), MAX(last_used) FROM turns GROUP BY model, provider, api_method")?;
        let rows = query
            .query_map([], |row| {
                Ok((
                    (row.get(0)?, row.get(1)?, row.get(2)?),
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((started, rows))
    };
    if let Ok((start, rows)) = read() {
        started = Some(start);
        for (key, count, last_used) in rows {
            let entry = usage.entry(key).or_default();
            entry.count = count;
            entry.last_used_unix_secs = last_used;
        }
    }
    for route in routes {
        let mut entry = usage
            .get(&key(&route.model, &route.provider, &route.api_method))
            .cloned()
            .unwrap_or_default();
        entry.tracking_started_unix_secs = started;
        route.usage = (started.is_some() || entry.selection_count > 0).then_some(entry);
    }
}

/// Resolve a catalog row using live serving identity, including credential mode
/// and explicit OpenRouter pins. Ambiguous identities remain unrecorded rather
/// than crediting an unrelated route.
pub fn serving_route(provider: &dyn Provider, selected_method: Option<&str>) -> Option<ModelRoute> {
    let model = provider.model();
    let display = provider.display_name();
    let provider_name = jcode_provider_core::normalize_model_route_provider_label(provider.name());
    let expected_method = match (
        provider_name.as_str(),
        provider.active_resolved_credential(),
    ) {
        ("claude" | "anthropic", Some(ResolvedCredential::ApiKey)) => Some("anthropic-api-key"),
        ("claude" | "anthropic", Some(ResolvedCredential::Oauth)) => Some("claude-oauth"),
        ("openai", Some(ResolvedCredential::ApiKey)) => Some("openai-api-key"),
        ("openai", Some(ResolvedCredential::Oauth)) => Some("openai-oauth"),
        ("copilot", _) => Some("copilot"),
        ("cursor", _) => Some("cursor"),
        ("bedrock", _) => Some("bedrock"),
        ("gemini", _) => Some("code-assist-oauth"),
        ("antigravity", _) => Some("antigravity-https"),
        _ => None,
    };
    let pin = provider.explicit_provider_pin_for_current_model();
    let mut candidates: Vec<_> = provider
        .model_routes()
        .into_iter()
        .filter(|route| {
            let same_model = route.model == model
                || (route.api_method_kind().is_openrouter()
                    && crate::provider::openrouter_catalog_model_id(&model).as_deref()
                        == Some(route.model.as_str()));
            let same_provider = if let Some(pin) = pin.as_ref() {
                route.api_method_kind().is_openrouter() && route.provider.eq_ignore_ascii_case(pin)
            } else {
                jcode_provider_core::model_route_provider_labels_match(&route.provider, &display)
            };
            same_model
                && same_provider
                && expected_method.is_none_or(|method| {
                    route.api_method_kind() == ModelRouteApiMethod::parse(method)
                })
        })
        .collect();
    if candidates.len() > 1
        && let Some(method) = selected_method
    {
        candidates.retain(|route| route.api_method_kind() == ModelRouteApiMethod::parse(method));
    }
    (candidates.len() == 1).then(|| candidates.remove(0))
}

/// Record one persisted response within an agent turn. Repeated continuations
/// update recency but do not increment count. All interfaces use this same hook.
pub fn record_turn(turn_id: &str, route: &ModelRoute) -> Result<ModelUsage> {
    let timestamp = now();
    let mut usage = record_at(&open(&path()?, timestamp)?, turn_id, route, timestamp)?;
    if let Some(selection) = legacy().get(&key(&route.model, &route.provider, &route.api_method)) {
        usage.selection_count = selection.selection_count;
        usage.last_selected_unix_secs = selection.last_selected_unix_secs;
    }
    Ok(usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(method: &str) -> ModelRoute {
        ModelRoute {
            model: "test-model".into(),
            provider: "OpenAI".into(),
            api_method: method.into(),
            available: true,
            detail: String::new(),
            cheapness: None,
            usage: None,
        }
    }

    #[test]
    fn continuations_count_once_per_turn_and_route_and_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite3");
        let db = open(&path, 10).unwrap();
        let oauth = route("openai-oauth");
        assert_eq!(record_at(&db, "turn-1", &oauth, 20).unwrap().count, 1);
        let continued = record_at(&db, "turn-1", &oauth, 30).unwrap();
        assert_eq!(continued.count, 1);
        assert_eq!(continued.last_used_unix_secs, Some(30));
        assert_eq!(
            record_at(&db, "turn-1", &oauth, 25)
                .unwrap()
                .last_used_unix_secs,
            Some(30)
        );
        assert_eq!(record_at(&db, "turn-2", &oauth, 40).unwrap().count, 2);
        assert_eq!(
            record_at(&db, "turn-1", &route("openai-api"), 40)
                .unwrap()
                .count,
            1
        );
        drop(db);
        let db = open(&path, 50).unwrap();
        let resumed = record_at(&db, "turn-2", &oauth, 60).unwrap();
        assert_eq!(resumed.count, 2);
        assert_eq!(resumed.tracking_started_unix_secs, Some(10));
        assert_eq!(resumed.selection_count, 0);
    }

    #[test]
    fn concurrent_connections_do_not_lose_turns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite3");
        drop(open(&path, 1).unwrap());
        let workers: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let db = open(&path, 1).unwrap();
                    record_at(&db, &format!("turn-{i}"), &route("openai-oauth"), 2 + i).unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        let result = record_at(
            &open(&path, 10).unwrap(),
            "turn-0",
            &route("openai-oauth"),
            10,
        )
        .unwrap();
        assert_eq!(result.count, 8);
    }

    #[test]
    fn legacy_selections_are_separate_and_efforts_aggregate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model_picker_usage.json");
        let contents = serde_json::json!({"version":1,"selections":{
            "test-model\u{1f}OpenAI\u{1f}openai-api\u{1f}high": {"count":3,"last_selected_unix_secs":40},
            "test-model\u{1f}OpenAI\u{1f}openai-api-key\u{1f}low": {"count":2,"last_selected_unix_secs":20},
            "bad-key": {"count":100,"last_selected_unix_secs":99}
        }});
        std::fs::write(&path, contents.to_string()).unwrap();
        let entries = legacy_selections(&path);
        assert_eq!(entries.len(), 1);
        let usage = &entries[&key("test-model", "OpenAI", "openai-api-key")];
        assert_eq!(usage.selection_count, 5);
        assert_eq!(usage.last_selected_unix_secs, Some(40));
        assert_eq!(usage.count, 0);
        assert_eq!(usage.last_used_unix_secs, None);
        assert_eq!(usage.tracking_started_unix_secs, None);
        std::fs::write(&path, "corrupt").unwrap();
        assert!(legacy_selections(&path).is_empty());
        std::fs::write(&path, r#"{"version":2,"selections":{}}"#).unwrap();
        assert!(legacy_selections(&path).is_empty());
    }

    #[test]
    fn aliases_normalize_without_collapsing_distinct_routes() {
        assert_eq!(
            key("m", "Claude", "claude-api"),
            key("m", "Anthropic", "anthropic-api-key")
        );
        assert_ne!(
            key("m", "OpenAI", "openai-api"),
            key("m", "OpenAI", "openai-oauth")
        );
        assert_ne!(
            key("m", "NIM", "openai-compatible:nim"),
            key("m", "NIM", "openai-compatible:other")
        );
    }
}
