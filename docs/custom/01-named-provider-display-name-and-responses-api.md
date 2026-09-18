# 01: Named provider `display_name` and `api = "openai-responses"`

Patch: `[01] feat(providers): honor display_name on named providers/models, add api = "openai-responses" wire mode`

## Why

Custom OpenAI-compatible endpoints declared under `[providers.<name>]` were
second-class citizens: the UI showed internal ids instead of the labels the
user configured, and there was no way to pick a different wire protocol.

- The provider label in the header/info widget always rendered the generic
  name (`openrouter`) or a lowercased version of it — a gateway like
  `viola` could not present itself as "Viola Router".
- Model ids were always run through the generic prettifier, so `swe-2`
  showed as "Swe 2" instead of a configured "SWE 2".
- The `api` field existed in the config schema but was dead code, so an
  endpoint that only speaks the OpenAI Responses API could not be used.

## What the user can do

```toml
[providers.viola]
type = "openai-compatible"
display_name = "Viola Router"
base_url = "https://example.com/v1"
api = "openai-responses"        # or "responses"; unset = chat/completions
api_key_env = "VIOLA_API_KEY"

[[providers.viola.models]]
id = "swe-2"
display_name = "SWE 2"
context_window = 262000
```

- `display_name` on the profile becomes the provider label everywhere it is
  shown (header, info widget, session history, model picker), preserving the
  exact configured casing. Falls back to the profile key when unset.
- `display_name` on a model entry overrides id prettification for that model
  in the header, `/model` picker, and anywhere a friendly name is rendered.
- `api = "openai-responses"` switches the profile to the Responses wire
  protocol (`POST {base_url}/responses`) with full streaming, tool calls,
  reasoning replay, and the same retry/rollback behavior as the default
  chat/completions path.

## Acceptance

- Header, floating info widget, and `/model` picker show the configured
  labels verbatim ("Viola Router", "SWE 2").
- With `api = "openai-responses"` set, requests hit `{base_url}/responses`
  and stream correctly, including tool-call round trips and multi-turn
  reasoning replay.

## Remote (SSH) propagation

Remote clients cannot see the server's `[providers.*]` config, so the
server-resolved metadata now travels on the wire:

- `ServerEvent::History`, `ModelChanged`, and `AvailableModelsUpdated` carry
  `model_display_name`, `model_context_window`, and `available_efforts`.
  `available_efforts` is `Option<Vec<String>>`: `Some(vec)` is authoritative
  (empty = provider has no effort support), `None` means the server predates
  the field and clients fall back to local inference.
- `ModelRoute` carries `display_name` and `context_window`, filled by
  `model_usage::enrich_routes` from the `openai-compatible:<profile>` encoded
  in `api_method`.
- `ModelCatalogSnapshot` carries the same three fields so both the live event
  path and the persisted remote catalog cache round-trip them.
- Client side: `remote_model_display_name`, `remote_model_context_window`,
  `remote_available_efforts` are tracked on `App`; picker labels consult a
  process-global registry (`provider_catalog::remote_model_display_name`)
  rebuilt from each accepted snapshot's routes; `update_context_limit_for_model`
  prefers the wire window before local resolution; effort cycling/listing
  prefers the wire ladder before `inferred_reasoning_efforts`.
- The registry is also seeded with the *current* model's resolved label
  (`set_remote_model_display_name`) whenever a History/ModelChanged snapshot
  carries `provider_model` + `model_display_name`. Providers with
  `model_catalog = false` send no routes, so without this seeding picker rows
  and overlays never learned the configured label and prettified the raw id
  ("swe-2" → "Swe 2" instead of "SWE 2").
- `pretty_model_display_name` consults the remote label table before the
  env/config lookups, so the floating info widget, right-side fact stack, and
  overscroll status line resolve wire labels the same way the picker does.

## Upstream status

Candidate for upstream PR — general feature, not local config. Until merged,
maintained as patch `[01]`.
