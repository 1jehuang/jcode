//! Shared, single-flight processing for daemon-owned model catalog broadcasts.
use super::available_models_dedup::available_models_dedup_key;
use crate::agent::Agent;
use crate::protocol::ServerEvent;
use crate::provider::Provider;
use jcode_provider_core::ModelCatalogSnapshot;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(60);
const MAX_ENTRIES: usize = 128;
const MAX_LIVE_BYTES: usize = 64 * 1024;

pub(super) struct Catalog {
    pub event: ServerEvent,
    pub dedup_key: Arc<str>,
    pub encoded_len: usize,
    slim: Option<ServerEvent>,
}

impl Catalog {
    fn new(snapshot: ModelCatalogSnapshot) -> Self {
        let event = ServerEvent::AvailableModelsUpdated {
            provider_name: snapshot.provider_name,
            provider_model: snapshot.provider_model,
            available_models: snapshot.available_models,
            available_model_routes: snapshot.model_routes,
        };
        let dedup_key = Arc::from(available_models_dedup_key(&event));
        let encoded_len = crate::protocol::encode_event(&event).len();
        let slim = if encoded_len > MAX_LIVE_BYTES {
            let slim = match &event {
                ServerEvent::AvailableModelsUpdated {
                    provider_name,
                    provider_model,
                    available_models,
                    ..
                } => ServerEvent::AvailableModelsUpdated {
                    provider_name: provider_name.clone(),
                    provider_model: provider_model.clone(),
                    available_models: available_models.clone(),
                    available_model_routes: Vec::new(),
                },
                _ => event.clone(),
            };
            (crate::protocol::encode_event(&slim).len() <= MAX_LIVE_BYTES).then_some(slim)
        } else {
            None
        };
        Self {
            event,
            dedup_key,
            encoded_len,
            slim,
        }
    }

    pub fn live_event(&self) -> Option<&ServerEvent> {
        if self.encoded_len <= MAX_LIVE_BYTES {
            Some(&self.event)
        } else {
            self.slim.as_ref()
        }
    }

    pub fn snapshot(&self) -> ModelCatalogSnapshot {
        match &self.event {
            ServerEvent::AvailableModelsUpdated {
                provider_name,
                provider_model,
                available_models,
                available_model_routes,
            } => ModelCatalogSnapshot::new(
                provider_name.clone(),
                provider_model.clone(),
                available_models.clone(),
                available_model_routes.clone(),
            ),
            _ => ModelCatalogSnapshot::default(),
        }
    }
}

struct Entry {
    built_at: Instant,
    catalog: Arc<Catalog>,
}
#[derive(Default)]
struct Cache {
    entries: HashMap<String, Entry>,
    hits: u64,
    builds: u64,
}

impl Cache {
    fn get(
        &mut self,
        key: Option<String>,
        build: impl FnOnce() -> ModelCatalogSnapshot,
    ) -> Arc<Catalog> {
        if let Some(key) = key.as_ref()
            && let Some(entry) = self.entries.get(key)
            && entry.built_at.elapsed() < TTL
        {
            self.hits += 1;
            return Arc::clone(&entry.catalog);
        }
        self.builds += 1;
        let catalog = Arc::new(Catalog::new(build()));
        if let Some(key) = key {
            self.entries
                .retain(|_, entry| entry.built_at.elapsed() < TTL);
            if self.entries.len() >= MAX_ENTRIES
                && let Some(oldest) = self
                    .entries
                    .iter()
                    .min_by_key(|(_, e)| e.built_at)
                    .map(|(k, _)| k.clone())
            {
                self.entries.remove(&oldest);
            }
            self.entries.insert(
                key,
                Entry {
                    built_at: Instant::now(),
                    catalog: Arc::clone(&catalog),
                },
            );
        }
        catalog
    }
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache::default()));

fn key(provider: &dyn Provider) -> Option<String> {
    let identity = provider.catalog_sharing_identity()?;
    Some(format!(
        "{identity:?}/{:?}",
        crate::model_usage::catalog_usage_revision()
    ))
}

pub(super) fn for_agent(agent: &Agent) -> Arc<Catalog> {
    for_provider(agent.provider_handle().as_ref())
}

fn provider_catalog_snapshot(provider: &dyn Provider) -> ModelCatalogSnapshot {
    let model = provider.model();
    let model = provider
        .explicit_provider_pin_for_current_model()
        .map(|pin| format!("{model}@{pin}"))
        .unwrap_or(model);
    let mut snapshot = ModelCatalogSnapshot::new(
        Some(provider.display_name()),
        Some(model),
        provider.available_models_display(),
        provider.model_routes(),
    );
    crate::model_usage::enrich_routes(&mut snapshot.model_routes);
    snapshot
}

fn for_provider(provider: &dyn Provider) -> Arc<Catalog> {
    // Keep the mutex across the build: concurrent clients wait for one leader
    // instead of each enriching/encoding the same catalog on a miss.
    for _ in 0..3 {
        let before = key(provider);
        let catalog = CACHE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(before.clone(), || provider_catalog_snapshot(provider));
        // Provider/catalog refreshes and usage writes can race the build. Never
        // return a memo whose declared inputs changed while it was being built.
        if before == key(provider) {
            return catalog;
        }
    }
    Arc::new(Catalog::new(provider_catalog_snapshot(provider)))
}

// A cloned handle is not a frozen provider: selection mutates it in place.
// Include model and display name even for providers without a sharing identity.
#[derive(PartialEq, Eq)]
struct BuildToken {
    identity: Option<String>,
    name: String,
    model: String,
    pin: Option<String>,
    usage: crate::model_usage::CatalogUsageRevision,
}

impl BuildToken {
    fn new(provider: &dyn Provider) -> Self {
        Self {
            identity: provider.catalog_sharing_identity(),
            name: provider.display_name(),
            model: provider.model(),
            pin: provider.explicit_provider_pin_for_current_model(),
            usage: crate::model_usage::catalog_usage_revision(),
        }
    }
}

/// Build without holding the agent or blocking the async worker. Capture reply
/// metadata only after revalidating under the agent lock, so a model switch
/// cannot combine old catalog fields with a new serving credential/tier. A busy
/// agent preserves the caller's existing fallback or skip behavior. Changed
/// inputs retry, with a bound so continuous refreshes cannot starve a request.
/// A failed blocking task is an error, not an authoritative empty catalog.
pub(super) async fn try_for_agent<T>(
    agent: &Arc<tokio::sync::Mutex<Agent>>,
    capture: impl FnOnce(&Agent) -> T,
) -> anyhow::Result<Option<(Arc<Catalog>, T)>> {
    for _ in 0..3 {
        let (provider, generation, token) = {
            let Ok(guard) = agent.try_lock() else {
                return Ok(None);
            };
            let provider = guard.provider_handle();
            let token = BuildToken::new(provider.as_ref());
            (provider, guard.provider_model_selection_generation(), token)
        };
        let build_provider = Arc::clone(&provider);
        let catalog =
            tokio::task::spawn_blocking(move || for_provider(build_provider.as_ref())).await?;
        let Ok(guard) = agent.try_lock() else {
            return Ok(None);
        };
        let current = guard.provider_handle();
        if !Arc::ptr_eq(&provider, &current)
            || guard.provider_model_selection_generation() != generation
            || BuildToken::new(current.as_ref()) != token
        {
            continue;
        }
        return Ok(Some((catalog, capture(&guard))));
    }
    anyhow::bail!("Model catalog changed repeatedly while building; retry the request")
}

pub(super) fn stats() -> serde_json::Value {
    let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    serde_json::json!({"entries": cache.entries.len(), "hits": cache.hits, "builds": cache.builds})
}

pub(super) fn invalidate() {
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entries
        .clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(model: &str) -> ModelCatalogSnapshot {
        ModelCatalogSnapshot::new(
            Some("test".into()),
            Some(model.into()),
            vec![model.into()],
            vec![],
        )
    }
    #[test]
    fn equivalent_clients_share_processing_and_identity_changes_rebuild() {
        let mut cache = Cache::default();
        let first = cache.get(Some("auth1/catalog1/usage1/model1".into()), || {
            snapshot("model1")
        });
        for _ in 0..100 {
            let next = cache.get(Some("auth1/catalog1/usage1/model1".into()), || {
                panic!("rebuilt equivalent catalog")
            });
            assert!(Arc::ptr_eq(&first, &next));
            assert!(Arc::ptr_eq(&first.dedup_key, &next.dedup_key));
        }
        for key in ["auth2", "catalog2", "usage2", "model2"] {
            let changed = cache.get(Some(key.into()), || snapshot(key));
            assert!(!Arc::ptr_eq(&first, &changed));
        }
        assert_eq!((cache.builds, cache.hits), (5, 100));
    }
    #[test]
    fn simultaneous_clients_single_flight_the_catalog_build() {
        let cache = Arc::new(Mutex::new(Cache::default()));
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let cache = Arc::clone(&cache);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    cache
                        .lock()
                        .unwrap()
                        .get(Some("shared".into()), || snapshot("model"))
                })
            })
            .collect();
        let catalogs: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        for catalog in &catalogs {
            assert!(Arc::ptr_eq(&catalogs[0], catalog));
        }
        let cache = cache.lock().unwrap();
        assert_eq!((cache.builds, cache.hits), (1, 15));
    }

    #[test]
    fn providers_without_explicit_identity_never_share() {
        let mut cache = Cache::default();
        let first = cache.get(None, || snapshot("one"));
        let next = cache.get(None, || snapshot("two"));
        assert!(!Arc::ptr_eq(&first, &next));
        assert!(cache.entries.is_empty());
    }
    #[test]
    fn expired_and_invalidated_entries_rebuild_and_cache_is_bounded() {
        let mut cache = Cache::default();
        cache.get(Some("one".into()), || snapshot("one"));
        cache.entries.get_mut("one").unwrap().built_at = Instant::now() - TTL;
        cache.get(Some("one".into()), || snapshot("new"));
        assert_eq!(cache.builds, 2);
        cache.entries.clear();
        cache.get(Some("one".into()), || snapshot("invalidated"));
        assert_eq!(cache.builds, 3);
        for i in 0..MAX_ENTRIES + 5 {
            cache.get(Some(i.to_string()), || snapshot("model"));
        }
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
    }
    #[test]
    fn oversized_catalog_preserves_names_in_live_payload() {
        let mut full = snapshot("one");
        full.model_routes = vec![crate::provider::ModelRoute {
            model: "one".into(),
            provider: "test".into(),
            api_method: "test".into(),
            available: true,
            detail: "x".repeat(MAX_LIVE_BYTES),
            usage: None,
            cheapness: None,
        }];
        let catalog = Catalog::new(full);
        assert!(catalog.encoded_len > MAX_LIVE_BYTES);
        let Some(ServerEvent::AvailableModelsUpdated {
            available_models,
            available_model_routes,
            ..
        }) = catalog.live_event()
        else {
            panic!("no slim payload")
        };
        assert_eq!(available_models, &["one"]);
        assert!(available_model_routes.is_empty());
        assert_eq!(catalog.snapshot().model_routes.len(), 1);
    }
}

#[cfg(test)]
#[path = "available_models_cache_tests.rs"]
mod offload_tests;
