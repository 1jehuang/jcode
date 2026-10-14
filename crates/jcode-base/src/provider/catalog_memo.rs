use super::*;

/// Memoized route catalog with the inputs that decide its freshness: build
/// time (short TTL), the auth generation at build time (bumped by
/// `AuthStatus::invalidate_cache()` on login/logout/credential edits), and the
/// catalog generation (bumped by prefetch/refresh completions).
#[derive(Clone)]
pub(super) struct RoutesMemoEntry {
    pub(super) built_at: std::time::Instant,
    pub(super) scope_key: String,
    pub(super) auth_generation: u64,
    pub(super) catalog_generation: u64,
    pub(super) routes: Vec<ModelRoute>,
    /// `listable_model_names_from_routes(&routes)`, cached because the
    /// non-chat-model heuristic string-scans every route name and callers
    /// (catalog snapshots) ask for names and routes together.
    pub(super) listable_models: Vec<String>,
}

/// Process-wide route-catalog memo shared across `MultiProvider` instances.
///
/// The shared server forks one `MultiProvider` per client connection, so a
/// per-instance memo cannot deduplicate the builds triggered by a burst of
/// simultaneous client spawns: every fresh fork still built its own catalog.
/// Catalog content is derived almost entirely from process-global state
/// (credential files, disk caches, config), so identical forks can share one
/// build. Instance-specific inputs (active provider/model/profile) are folded
/// into the memo key; anything not captured is bounded by the short TTL and
/// the auth/catalog generations.
static GLOBAL_ROUTES_MEMO: LazyLock<Mutex<HashMap<String, RoutesMemoEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Single-flight guard for catalog builds. During a client connect burst every
/// connection calls `model_routes()` at nearly the same instant; without this
/// they all miss the still-empty memo and build the same catalog in parallel
/// (a thundering herd that pegs every core). Holding this lock across the
/// build makes followers block (sleep, not spin) until the leader publishes
/// its result, which they then serve from the shared memo.
static GLOBAL_ROUTES_BUILD_LOCK: Mutex<()> = Mutex::new(());

/// Bumped whenever provider catalogs change out-of-band (prefetch completion,
/// forced catalog refresh, auth changes). Invalidates every shared memo entry.
static CATALOG_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(super) fn catalog_generation() -> u64 {
    CATALOG_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

/// Invalidate every memoized route catalog in the process.
///
/// Called when provider catalogs change out-of-band (background catalog
/// refresh completion, auth changes). This is what keeps the route memo TTL a
/// backstop rather than the mechanism: a completed refresh is reflected on the
/// next render instead of waiting for the memo to expire.
pub fn bump_catalog_generation() {
    CATALOG_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub(super) fn next_catalog_scope() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl MultiProvider {
    /// Drop this instance's route-catalog memo. Use for changes that are
    /// captured by [`Self::routes_memo_key`] (model/provider/profile switches):
    /// the shared memo stays valid because those instances key differently.
    pub(super) fn invalidate_routes_memo(&self) {
        if let Ok(mut memo) = self.routes_memo.lock() {
            *memo = None;
        }
    }

    /// Drop every memoized catalog in the process. Use for changes that alter
    /// catalog *content* beyond the memo key: credential changes and catalog
    /// prefetch/refresh completions. Deliberately not called from set_model /
    /// set_active_provider, which run once per shared-server fork during
    /// connect bursts and would otherwise defeat the shared memo.
    pub(super) fn invalidate_routes_memo_globally(&self) {
        self.invalidate_routes_memo();
        CATALOG_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Key identifying the instance-specific state that feeds the route
    /// catalog. Two `MultiProvider` instances with equal keys (given equal
    /// auth/catalog generations) produce equivalent catalogs, so shared-server
    /// forks can reuse one build. The current model matters because the active
    /// OpenRouter model gets priority endpoint-refresh scheduling and detail
    /// annotations in the catalog; the configured-provider bitmap matters
    /// because each configured runtime contributes its own route family.
    pub(super) fn routes_memo_key(&self) -> String {
        let active = self.active_provider();
        let credential_mode = self.credential_mode();
        let profile = self
            .active_openai_compatible_profile
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .unwrap_or_default();
        let mut compat_profiles: Vec<String> = self
            .openai_compatible_profiles
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .keys()
            .cloned()
            .collect();
        compat_profiles.sort();
        let configured = [
            ("an", self.anthropic_provider().is_some()),
            ("oa", self.openai_provider().is_some()),
            ("co", self.copilot_provider().is_some()),
            ("ag", self.antigravity_provider().is_some()),
            ("ge", self.gemini_provider().is_some()),
            ("cu", self.cursor_provider().is_some()),
            ("be", self.bedrock_provider().is_some()),
            ("or", self.openrouter_provider().is_some()),
        ]
        .iter()
        .filter(|(_, present)| *present)
        .map(|(tag, _)| *tag)
        .collect::<Vec<_>>()
        .join(",");
        static CATALOG_CLOCK: LazyLock<std::time::Instant> = LazyLock::new(std::time::Instant::now);
        format!(
            "{:?}",
            (
                (
                    self.catalog_scope,
                    CATALOG_CLOCK.elapsed().as_secs() / 60,
                    crate::storage::jcode_dir()
                        .ok()
                        .map(|path| path.into_os_string().into_encoded_bytes()),
                    std::env::var_os("JCODE_HOME").map(|value| value.into_encoded_bytes()),
                    crate::config::config_reload_generation(),
                ),
                Self::provider_key(active),
                self.model(),
                format!("{credential_mode:?}"),
                profile,
                configured,
                compat_profiles,
                (
                    self.anthropic_provider().map(|runtime| {
                        (
                            format!("{:?}", runtime.credential_mode()),
                            runtime.oauth_account_label(),
                        )
                    }),
                    self.openai_provider().map(|runtime| {
                        (
                            format!("{:?}", runtime.credential_mode()),
                            runtime.oauth_account_label(),
                        )
                    }),
                ),
                self.explicit_provider_pin_for_current_model(),
            )
        )
    }

    /// Return a fresh memoized catalog entry (routes + listable model names),
    /// building it at most once per TTL window per catalog-relevant state.
    ///
    /// Freshness is keyed on a short TTL plus the auth and catalog
    /// generations. Lookup order: this instance's memo, the process-wide
    /// shared memo (so shared-server forks reuse one build), then a
    /// single-flight build that followers wait on instead of duplicating.
    pub(super) fn fresh_routes_memo_entry(&self) -> RoutesMemoEntry {
        // Backstop only: invalidation is event-driven via auth/catalog
        // generations, so a completed refresh shows up on the next render.
        const ROUTES_MEMO_TTL: std::time::Duration = std::time::Duration::from_secs(60);

        // Rendering no longer schedules catalog I/O; the sweeper does.
        catalog_scheduler::ensure_started();

        for _attempt in 0..3 {
            let shared_key = self.routes_memo_key();
            let auth_generation = pricing::auth_pricing_generation();
            let catalog_gen = catalog_generation();
            let fresh = |entry: &RoutesMemoEntry| {
                entry.scope_key == shared_key
                    && entry.auth_generation == auth_generation
                    && entry.catalog_generation == catalog_gen
                    && entry.built_at.elapsed() < ROUTES_MEMO_TTL
            };

            // Fast path: this instance already built (or copied) a fresh catalog.
            if let Ok(memo) = self.routes_memo.lock()
                && let Some(entry) = memo.as_ref()
                && fresh(entry)
            {
                return entry.clone();
            }

            // Shared path: another instance with the same catalog-relevant state
            // (typically a fresh fork on the shared server) built one already.
            let try_shared = || -> Option<RoutesMemoEntry> {
                let shared = GLOBAL_ROUTES_MEMO.lock().ok()?;
                let entry = shared.get(&shared_key)?;
                if !fresh(entry) {
                    return None;
                }
                let entry = entry.clone();
                if let Ok(mut memo) = self.routes_memo.lock() {
                    *memo = Some(entry.clone());
                }
                Some(entry)
            };
            if let Some(entry) = try_shared() {
                return entry;
            }

            // Single-flight: serialize builds so a connect burst produces one
            // build and N-1 memo hits instead of N parallel builds.
            let _build_guard = GLOBAL_ROUTES_BUILD_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if self.routes_memo_key() != shared_key
                || pricing::auth_pricing_generation() != auth_generation
                || catalog_generation() != catalog_gen
            {
                continue;
            }
            // Re-check after acquiring the lock: the leader that held it may have
            // just published exactly the entry this instance needs.
            if let Some(entry) = try_shared() {
                return entry;
            }

            let routes = catalog_routes::multiprovider_model_routes(self);
            if self.routes_memo_key() != shared_key
                || pricing::auth_pricing_generation() != auth_generation
                || catalog_generation() != catalog_gen
            {
                continue;
            }
            let entry = RoutesMemoEntry {
                built_at: std::time::Instant::now(),
                scope_key: shared_key.clone(),
                auth_generation,
                catalog_generation: catalog_gen,
                listable_models: listable_model_names_from_routes(&routes),
                routes,
            };
            if let Ok(mut memo) = self.routes_memo.lock() {
                *memo = Some(entry.clone());
            }
            if let Ok(mut shared) = GLOBAL_ROUTES_MEMO.lock() {
                // Tiny keyspace (active provider + model + profile); prune stale
                // entries opportunistically so it cannot grow unbounded.
                shared.retain(|_, existing| {
                    existing.auth_generation == auth_generation
                        && existing.catalog_generation == catalog_gen
                        && existing.built_at.elapsed() < ROUTES_MEMO_TTL
                });
                shared.insert(shared_key, entry.clone());
            }
            return entry;
        }
        // Continuous refresh churn must not starve clients or publish a stale
        // shared entry. Return a private best-effort catalog instead.
        let routes = catalog_routes::multiprovider_model_routes(self);
        RoutesMemoEntry {
            built_at: std::time::Instant::now(),
            scope_key: self.routes_memo_key(),
            auth_generation: pricing::auth_pricing_generation(),
            catalog_generation: catalog_generation(),
            listable_models: listable_model_names_from_routes(&routes),
            routes,
        }
    }
}
