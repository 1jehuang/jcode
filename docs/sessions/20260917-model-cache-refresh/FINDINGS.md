# Model Cache Background Refresh Investigation

**Date:** 2026-09-17
**Status:** Root cause identified

---

## Current State (2026-09-17 ~03:05 UTC)

All disk cache files are currently **fresh** (< 6h old):

| Cache File | `cached_at` | Age (h) | Models | Status |
|---|---|---|---|---|
| `opencode-go_models.json` | 1789642860 | 0.0h | 38 | Fresh |
| `openai-compatible_models.json` | 1789642783 | 0.1h | 11376 | Fresh |
| `opencode_models.json` | 1789642702 | 0.1h | 71 | Fresh |
| `openrouter_models.json` | 1789642225 | 0.2h | 444 | Fresh |
| `nvidia-nim_models.json` | 1789642221 | 0.2h | 82 | Fresh |
| `minimax_models.json` | 1789624346 | 5.2h | 38 | Stale |

The caches have been refreshed by a recent process/session. The "142.5h old" state from the previous session is no longer observable.

---

## Architecture Overview

### Refresh Mechanism: Two Separate Paths

**Path 1: Active Provider Path** (`maybe_schedule_model_catalog_refresh`)
- Called when `available_models_display()` is invoked for the currently-active provider
- Uses `should_background_refresh_model_catalog()` which checks `cache_age_secs < MODEL_CATALOG_SOFT_REFRESH_SECS` (15 minutes)
- Per-provider in-flight guard (`ModelCatalogRefreshState`)
- Registered per-session at startup via `spawn_anthropic_catalog_refresh_if_needed()` and `spawn_openai_catalog_refresh_if_needed()`

**Path 2: Catalog Sweeper** (`catalog_scheduler::sweep_stale_profile_catalogs`)
- Background loop running every 60 seconds
- Checks ALL configured OpenAI-compatible profiles
- Uses `profile_catalog_cache_needs_refresh()` → `cached_live_models_for_openai_compatible_profile()` with 15-minute staleness threshold
- Also refreshes standard OpenRouter catalog via `maybe_schedule_standard_openrouter_catalog_refresh()`
- Uses global `ProfileCatalogRefreshTracker` with exponential backoff

### Key Constants

| Constant | Value | File:Line |
|---|---|---|
| `MODEL_CATALOG_SOFT_REFRESH_SECS` | 15 min | `jcode-provider-openrouter-runtime/src/lib.rs:71` |
| `MODEL_CATALOG_REFRESH_RETRY_SECS` | 60s | `jcode-provider-openrouter-runtime/src/lib.rs:73` |
| `STANDARD_OPENROUTER_CATALOG_TTL_SECS` | 24h | `jcode-provider-openrouter-runtime/src/lib.rs:76` |
| `OPENAI_COMPATIBLE_PROFILE_CATALOG_SOFT_REFRESH_SECS` | 15 min | `jcode-base/src/provider/mod.rs:143` |
| `CACHE_TTL_SECS` (disk cache) | 24h | `jcode-provider-openrouter/src/lib.rs:10` |
| `SWEEP_INTERVAL` | 60s | `jcode-base/src/provider/catalog_scheduler.rs:28` |
| `INITIAL_SWEEP_DELAY` | 10s | `jcode-base/src/provider/catalog_scheduler.rs:33` |

---

## Root Causes of Stale Caches

### 1. Sweeper Lazy Start (Primary Cause)

The catalog sweeper is **not started at process launch**. It's started lazily via:

```
catalog_scheduler::ensure_started() → called from fresh_routes_memo_entry() (mod.rs:537)
```

`fresh_routes_memo_entry()` is only called when the model picker is rendered or `/model` command is invoked. If no session has triggered this path, the sweeper never starts, and background refresh never fires.

**Impact:** All OpenAI-compatible profile caches remain stale until the first `/model` render.

### 2. Profile API Key Missing → Silent Refresh Abort

`maybe_schedule_openai_compatible_profile_catalog_refresh()` (lib.rs:714) checks for API key:

```rust
let auth = if let Some(key) = load_api_key_from_env_or_config(&resolved.api_key_env, &resolved.env_file) {
    ProviderAuth::AuthorizationBearer { ... }
} else if !resolved.requires_api_key {
    ProviderAuth::None { ... }
} else {
    finish_profile_catalog_refresh(&resolved.id);  // ← aborts
    return false;
};
```

If the profile requires an API key but none is configured, the refresh silently aborts. No log output.

**Impact:** Profiles with missing/invalid API keys (e.g. MiniMax without `MINIMAX_API_KEY`) never get refreshed.

### 3. Standard OpenRouter Refresh Needs OPENROUTER_API_KEY

`maybe_schedule_standard_openrouter_catalog_refresh()` (lib.rs:807) immediately returns false if `OPENROUTER_API_KEY` is not available:

```rust
let Some(api_key) = load_api_key_from_env_or_config(DEFAULT_API_KEY_NAME, DEFAULT_ENV_FILE) else {
    return false;  // ← silent no-op
};
```

**Impact:** If `OPENROUTER_API_KEY` is not configured, the standard OpenRouter catalog never refreshes.

### 4. Exponential Backoff Can Block Refreshes for Up to 1 Hour

`begin_profile_catalog_refresh()` (lib.rs:656) checks retry delay:

```rust
if let Some(last) = state.last_attempt_unix.get(profile_id)
    && now.saturating_sub(*last) < profile_catalog_retry_delay_secs(consecutive_failures)
{
    return false;  // ← blocked by backoff
}
```

`profile_catalog_retry_delay_secs()` returns: 60s, 120s, 240s, ... up to 3600s (1 hour).

If a profile's refresh fails repeatedly (e.g. API rate limit), subsequent refresh attempts are blocked for increasingly long periods.

**Impact:** After multiple failures, a profile can be blocked from refresh for up to 1 hour per attempt.

### 5. Global Profile Catalog Refresh Tracker

```rust
struct ProfileCatalogRefreshTracker {
    in_flight: HashSet<String>,
    last_attempt_unix: HashMap<String, u64>,
    consecutive_failures: HashMap<String, u32>,
}
```

The `in_flight` set prevents concurrent refreshes for the same profile ID. This is correct behavior but means:
- If a refresh task is spawned but takes a long time, subsequent sweeps skip that profile
- If the spawned task panics, `finish_profile_catalog_refresh` is never called, and the profile is permanently blocked (in-flight forever)

**Impact:** A panic in a refresh task permanently blocks that profile's refresh for the process lifetime.

### 6. MiniMax Cache Source API Base Mismatch

The `minimax_models.json` cache has `source_api_base: https://opencode.ai/zen/go/v1`, but the MiniMax profile's `api_base` is `https://api.minimax.io/v1`. The function `cached_live_models_for_openai_compatible_profile()` checks:

```rust
if source_api_base != expected_api_base {
    return None;  // ← treated as missing cache
}
```

This means the MiniMax profile **never uses the cached data** even though it's valid, because the source doesn't match. The cache was written by the OpenCode Go provider, not the MiniMax provider.

**Impact:** MiniMax profile always falls back to static models or live fetch, regardless of cache freshness.

---

## Flow Diagram: Why Caches Go Stale

```
Process starts
  ├─ MultiProvider::new_with_auth_status()
  │   ├─ spawn_anthropic_catalog_refresh_if_needed()  ← only Anthropic
  │   ├─ spawn_openai_catalog_refresh_if_needed()      ← only OpenAI
  │   └─ NO sweeper start here!
  │
  ├─ Session runs, user asks /model
  │   └─ fresh_routes_memo_entry()
  │       └─ catalog_scheduler::ensure_started()  ← NOW sweeper starts
  │           └─ spawn sweeper loop (10s delay, then 60s interval)
  │
  ├─ Sweeper runs (every 60s)
  │   ├─ For each configured profile:
  │   │   ├─ profile_catalog_cache_needs_refresh()
  │   │   │   └─ cached_live_models_for_openai_compatible_profile()
  │   │   │       ├─ Load disk cache for profile namespace
  │   │   │       ├─ Check source_api_base matches resolved profile's api_base
  │   │   │       │   └─ If mismatch → returns None → needs refresh = true
  │   │   │       └─ Check staleness (15 min threshold)
  │   │   └─ If needs refresh:
  │   │       ├─ begin_profile_catalog_refresh()
  │   │       │   ├─ Check in-flight → if true, skip
  │   │       │   ├─ Check backoff → if too recent, skip
  │   │       │   └─ Mark in_flight, record timestamp
  │   │       └─ Spawn async fetch_models_from_api()
  │   │           ├─ On success: save disk cache, publish event
  │   │           └─ On failure: increment consecutive_failures
  │   └─ maybe_schedule_standard_openrouter_catalog_refresh()
  │       ├─ Need OPENROUTER_API_KEY → if missing, skip
  │       ├─ Check cache freshness (24h TTL)
  │       └─ If stale: begin + spawn refresh
  │
  └─ After 24h without sweeper running:
      └─ ALL caches are stale (>24h), but no mechanism to refresh them
```

---

## Recommendations

### High Priority

1. **Start sweeper at process launch, not lazily**: Move `catalog_scheduler::ensure_started()` call to `MultiProvider::new_with_auth_status()` or the composition root, so it starts regardless of whether `/model` is accessed.

2. **Add logging to silent abort paths**: Every `return false` in `maybe_schedule_openai_compatible_profile_catalog_refresh()` should log a warning (e.g. "Skipping MiniMax catalog refresh: MINIMAX_API_KEY not configured").

3. **Add panic guard to refresh tasks**: Wrap spawned refresh tasks in `std::panic::catch_unwind` or use `tokio::spawn` with a task that always calls `finish_profile_catalog_refresh` in a `Drop` guard.

### Medium Priority

4. **Fix MiniMax source API base mismatch**: The cache write path (`save_disk_cache_with_source_for_namespace`) should use the resolved profile's API base, not the runtime's current API base. This would fix the MiniMax profile never using its cache.

5. **Reduce backoff ceiling**: The 1-hour maximum backoff for catalog refreshes is too aggressive. Consider capping at 10 minutes.

6. **Add cache freshness check on process resume**: When a new session starts, check all configured profile caches and schedule refreshes for any that are stale, even if the sweeper hasn't started yet.

### Low Priority

7. **Consider cache warming at login/auth**: When credentials change, immediately refresh all profiles' caches rather than waiting for the next sweep.

---

## Evidence Files

- Cache files: `~/.jcode/cache/*_models.json`
- Refresh logic: `crates/jcode-provider-openrouter-runtime/src/lib.rs`
- Disk cache layer: `crates/jcode-provider-openrouter/src/lib.rs`
- Sweeper: `crates/jcode-base/src/provider/catalog_scheduler.rs`
- Profile catalog: `crates/jcode-base/src/provider_catalog.rs`
- Composition root: `src/cli/startup.rs`
- Provider init: `crates/jcode-base/src/provider/startup.rs`
