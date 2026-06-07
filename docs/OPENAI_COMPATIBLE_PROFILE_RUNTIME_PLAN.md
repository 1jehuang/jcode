# OpenAI-Compatible Profile Runtime Migration Plan

## Problem

`OpenRouterProvider` currently represents two distinct concepts:

1. Standard OpenRouter, with OpenRouter-specific routing, provider pinning, endpoint metadata, and an `openrouter` catalog namespace.
2. Direct OpenAI-compatible providers such as NVIDIA NIM, Groq, Cerebras, Chutes, and custom endpoints, which reuse the same HTTP transport but have distinct credentials, API bases, catalogs, and model IDs.

Because `MultiProvider` stores only one `openrouter` runtime slot, switching from standard OpenRouter to a direct profile replaces the active runtime/catalog view. This caused issue #274: after switching from `openrouter/owl-alpha` to NVIDIA NIM, `/model` no longer exposed standard OpenRouter and could mis-associate OpenRouter models with NVIDIA.

## Target architecture

Separate transport, profile identity, and route aggregation.

```rust
struct OpenAiCompatibleClient {
    api_base: String,
    api_key_env: String,
    env_file: String,
    auth_header: AuthHeaderConfig,
}

struct OpenAiCompatibleProfileRuntime {
    profile_id: String,          // "openrouter", "nvidia-nim", "groq", ...
    display_name: String,        // "OpenRouter", "NVIDIA NIM", ...
    cache_namespace: String,     // usually profile_id
    default_model: Option<String>,
    provider_routing: bool,      // true for standard OpenRouter features
    client: OpenAiCompatibleClient,
}
```

`MultiProvider` should eventually move from:

```rust
openrouter: RwLock<Option<Arc<openrouter::OpenRouterProvider>>>,
```

to something like:

```rust
openai_compatible: RwLock<BTreeMap<String, Arc<OpenAiCompatibleProfileRuntime>>>,
active_openai_compatible_profile: RwLock<Option<String>>,
```

Standard OpenRouter becomes one profile in this map, not the container for every compatible provider.

## Route aggregation rule

`/model` should aggregate routes from every configured profile:

```rust
for profile in configured_openai_compatible_profiles() {
    routes.extend(profile.model_routes());
}
```

Switching active runtime to NVIDIA NIM should only update active selection:

```rust
active_openai_compatible_profile = Some("nvidia-nim".into());
```

It should not remove or relabel `openai_compatible["openrouter"]`.

## Compatibility requirements

Keep existing user-facing forms working:

- `openrouter:<model>` targets standard OpenRouter.
- `nvidia-nim:<model>` targets NVIDIA NIM.
- `openai-compatible:<model>` targets the configured custom endpoint.
- `--provider openrouter` remains standard OpenRouter.
- `--provider openai-compatible` remains the generic/custom profile.
- Existing `OpenRouterProvider` type can remain as a compatibility wrapper while internals move.

## Incremental migration slices

1. **Route aggregation slice, completed in `b1272ae`**
   - Standard OpenRouter cached routes are scoped to the `openrouter` namespace.
   - Direct profiles can be active without hiding standard OpenRouter from `/model`.
   - Regression: OpenRouter `owl-alpha` -> NVIDIA NIM -> `/model` keeps OpenRouter route and does not relabel it as NVIDIA.

2. **Profile runtime struct**
   - Introduce `OpenAiCompatibleProfileRuntime` around current OpenRouter provider settings.
   - Keep `OpenRouterProvider` as a type alias/wrapper initially.

3. **Runtime registry**
   - Add a map of configured compatible profiles to `MultiProvider`.
   - Populate it from configured/saved credentials at startup and auth-change time.

4. **Active profile selection**
   - Replace implicit environment mutation as the only active-profile state with explicit profile IDs.
   - Use env application only as a compatibility/bootstrap layer.

5. **Picker and server snapshots**
   - Emit profile-scoped routes and available-model snapshots.
   - Include profile ID/api method in debug output so mislabeling is testable.

6. **Rename cleanup**
   - Rename generic internals from OpenRouter to OpenAI-compatible where accurate.
   - Keep public commands and config stable.

## Validation matrix

For each configured profile pair, verify:

- Active profile A, inactive profile B: `/model` shows both A and B routes.
- Selecting a B route switches to B and keeps A visible.
- Models with slash IDs are not automatically treated as standard OpenRouter unless the route/profile says so.
- OpenRouter provider-pinning remains available only for the standard OpenRouter profile.
- Direct-profile static and live catalogs remain namespace-scoped.

Key regression scenarios:

- `openrouter/owl-alpha` -> `nvidia-nim:nvidia/llama-...` -> OpenRouter still selectable.
- Cerebras active with Groq configured -> no relabeling of Cerebras models as Groq.
- Chutes active with stale legacy OpenRouter cache -> no stale OpenRouter models under Chutes.
