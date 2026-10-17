# suggested-replies-cascade Specification

## Purpose
Define the configuration (`[suggested_replies]`), environment hot-reload, and two-stage macro/micro branching cascade combining OpenJev SystemOne (`https://api.openjev.sh/v1/systemone`) and OpenGF (`Sidecar` with `"store": false`) to produce `SuggestedRepliesResponse` (`{ defaultAction, secondarySuggestions, branch }`).

## Requirements
### Requirement: Suggested Replies Configuration & Lenient Hot-Reload
The system SHALL provide a `[suggested_replies]` configuration section (`SuggestedRepliesConfig`) with `enabled` (bool, default `true`), `jev_provider` (string, default `"auto"`, supporting `"auto"`, `"laya"`, `"openjev"`, `"typesafe"`, `"openrouter"`, `"aimlapi"`, and `"jcode"`), `use_open_gf` (bool, default `false`), `max_secondary_suggestions` (usize, default `5`, clamped `1..=5`), `confidence_threshold` (f32, default `0.55`, clamped `0.0..=1.0`), `enable_full_auto_explorer` (bool, default `false`), and `six_lens_auto_review` (bool, default `false`).

#### Scenario: Lenient config parsing and environment fingerprint tracking
- **WHEN** `config.toml` or `JCODE_*` environment variables specify `[suggested_replies]` values (including out-of-range integers or floats)
- **THEN** `SuggestedRepliesConfig` clamps `max_secondary_suggestions` into `1..=5` and `confidence_threshold` into `0.0..=1.0` without failing `Config::load_from_file_strict()`, and `CONFIG_ENV_KEYS` tracks every environment variable override for hot-reload.

### Requirement: Two-Stage Macro/Micro Branching with OpenJev SystemOne and OpenGF
The system SHALL execute a two-stage cascade at turn completion: Iteration 1 determines the macro conversation branch (via OpenJev SystemOne `choice`/`noul` when `use_open_gf = false`, or via OpenGF `Sidecar` with `"store": false` when `use_open_gf = true`), and Iteration 2 scores candidate actions via OpenJev SystemOne (`https://api.openjev.sh/v1/systemone` with `model: "openjev"` when `OPENJEV_API_KEY` is configured) or deterministic local scoring when credentials are absent or return HTTP 402.

#### Scenario: Default Action promotion and secondary suggestions distribution
- **WHEN** candidate actions are scored and sorted by `confidence` descending
- **THEN** if the top candidate has `confidence >= confidence_threshold`, it is assigned to `defaultAction` and up to `max_secondary_suggestions` remaining candidates are placed in `secondarySuggestions`; otherwise `defaultAction` is `None` and up to `max_secondary_suggestions` candidates are placed in `secondarySuggestions`.

