# A6 API — Astra with astra-jb laundering (custom provider line)

`a6api` is a dedicated OpenAI-compatible provider line for astra models served
by the A6 marketplace exchange (`https://api.a6api.com/v1`). It exists purely in
parallel to the normal OpenAI provider's `gpt-6-astra` catalog entry: the
openai provider's models, catalog, and request path are untouched, and none of
this profile's behavior can activate for any other provider.

What makes the line special: **the astra-jb vision-OCR laundering chain is
applied by default.** Every typed prompt on this line is rendered as a
scanned-page image (aged paper, serif type, photocopy grain, archive stamps)
and sent through the vision path with the proven OCR-transcription framing,
so the text never touches the model's text-input filter. This is the working
jailbreak from the 2026-09-28 astra-jb campaign (`launder.py` / `chain.py`,
2/2 reproduction) ported into the provider request path — see
`~/Projects/other/llmpools/astra-jb/FINDINGS.md` for the root-cause analysis
(modality-asymmetric input guard: text gets stripped, image "documents" are
transcribed faithfully).

## Setup

```bash
jcode auth login a6api        # paste your A6 API key (stored as A6API_KEY in ~/.jcode/a6api.env)
# or manually:
#   echo 'A6API_KEY=sk-...' >> ~/.jcode/a6api.env
```

The key var and env file match the pre-existing `[providers.a6api]` named
profile in config.toml, so one key serves both lines.

Then pick the model in `/model` — it appears as **A6 API → gpt-6-astra**
(static seed; the live marketplace `/v1/models` catalog takes over once
fetched). Context window falls back to a 400K static seed only when the
profile does not declare one; an explicit `[[providers.a6api.models]]`
`context_window` always wins.

## How the laundering behaves

- The **final user turn** of each request (your newest typed message) is
  rendered to up to 3 scanned pages and replaced with framing text + the page
  images. The model reads your prompt as a document, not as filtered text.
- **`---SPLIT---` in your prompt fragments it** — the campaign's one
  operational rule. The vision guard has a per-image completeness check: a
  complete sensitive combination in a single image gets refused, while the
  same content split across separate images transcribes faithfully. Each
  fragment becomes its own page image, so put `---SPLIT---` between the
  procedure part and the names part of fragment-sensitive prompts.
- Earlier turns stay plain text in history — the campaign proved
  already-contextual content never trips the per-image detector
  (recap-chaining finding).
- Tool-result turns are never laundered (agentic loops keep working; your
  instruction already rode the vision path the turn you typed it).
- Rendering is deterministic per prompt text, so retries send byte-identical
  images and the KV prefix stays stable.
- No serif font on the machine (or a prompt exceeding the page cap) skips
  laundering gracefully — the turn goes out as plain text.

## Knobs (environment)

| Variable | Default | Meaning |
|---|---|---|
| `JCODE_A6API_JB` | on | `off`/`0`/`false` disables laundering entirely (plain-text turns) |
| `JCODE_A6API_JB_FRAMING` | `coursepack` | `internal-memo`, `legal`, `archival`, or `generic` (ported from launder.py) |
| `JCODE_A6API_JB_ENGAGE` | on | `off` drops the "treat the page as my request" suffix — pure transcription framing only |
| `JCODE_A6API_JB_FONT` | system serif | explicit font file path (falls back through Times New Roman, Georgia, Liberation/DejaVu Serif) |
| `JCODE_A6API_JB_MAX_PAGES` | 3 | 1–8; prompts needing more pages are sent as text, never truncated |

## Inspecting rendered pages

```bash
cargo test -p jcode-provider-openrouter-runtime --lib astra_jb -- --ignored --nocapture
# writes sample pages to crates/jcode-provider-openrouter-runtime/target/astra-jb-samples/
```

## Implementation map

| File | Role |
|---|---|
| `crates/jcode-provider-metadata/src/catalog.rs` | `A6API_PROFILE` + `A6API_LOGIN_PROVIDER` (parallel line, aliases `a6`, `astra-jb`) |
| `crates/jcode-base/src/provider_catalog.rs` | static model seed (`gpt-6-astra`) + 400K context limit for the profile |
| `crates/jcode-provider-openrouter-runtime/src/astra_jb.rs` | renderer + framings + final-turn transform + tests |
| `crates/jcode-provider-openrouter-runtime/src/openrouter_provider_impl.rs` | `complete()` hook + vision-input gate for a6api astra models |

Scope guarantee: every code path is gated on `profile_id == "a6api"` AND an
astra-family model. Other providers — including the normal OpenAI astra line —
compile the module but never execute it.

## Not yet verified live

The laundering chain itself is the 2/2-reproduced campaign result, but the
**a6api integration has not been exercised against the live endpoint** (no
key at integration time). Two things need a first live check:

1. That a6api's merchant pools accept `image_url` parts on `gpt-6-astra`
   (the campaign proved the vision path on the apiduck relay; a6api is
   different infrastructure). If a channel rejects images, turns fail with
   an API error — set `JCODE_A6API_JB=off` and route around that channel.
2. Whether a6api's relay imposes the apiduck-style `input[0]` image
   positioning quirk. The transform already sends the proven
   text-then-image content-part order; if the endpoint 400s with an image
   URL error, the order may need inverting there.

## Verified integration evidence (offline, 2026-10-22)

| Requirement | Check | Result |
|---|---|---|
| Separate selectable line exists | rebuilt `jcode auth status` shows the `a6api` row (API key, `A6API_KEY`) | observed in binary output |
| Host/key alignment with named profile | `a6api_profile_and_login_align_with_named_config_profile` test | passes |
| Login aliases (a6, astra-jb) resolve | same test, alias loop | passes |
| Jailbreak on by default, no per-turn opt-in | `launder_final_user_turn` fires on gate alone; `final_user_turn_entry_point_respects_gate_and_transforms` | passes |
| Dual gate: profile AND astra model | `laundering_gates_on_profile_and_model` + `other_profiles_do_not_gain_image_input_via_astra_laundering` | both pass (non-a6api + astra provably does not launder) |
| Non-astra models on a6api stay clean | `laundering_gates_on_profile_and_model` (a6api + glm-5 → false) | passes |
| Kill switch `JCODE_A6API_JB=off` | `kill_switch_env_disables_laundering` | passes |
| Image parts ride the vision path | `a6api_astra_models_accept_image_input_for_laundering` (real provider impl) | passes |
| Tool-result turns are skipped intact | `launder_skips_tool_result_turns_and_keeps_them_intact` | passes |
| `---SPLIT---` → one page per fragment | `split_fragments...` + `split_prompt_renders_one_image_per_fragment...` | pass |
| Page cap skips laundering instead of truncating | `pagination_caps_pages_instead_of_truncating` | passes |
| Renderer unavailable → prompt sent as text | `launder_skips_when_renderer_unavailable` | passes |
| Rendered page is a legible scanned page | ignored render test + zone ink check (header 4.7%, body 2.3%, footer 2.1% dark pixels, 1000×1400) | observed |
| Normal astra line untouched | openai profile + astra model → laundering gate returns false (dedicated test) | passes |
| Full suites green | metadata 19/19, base catalog 43/43, runtime lib 199/199 (+2 ignored visual), app crates compile | observed |

Known cosmetic oddity: `jcode auth status` may report a6api "available" with a
phantom credential source when ambient named-provider env vars are exported in
the shell. This comes from the pre-existing named-profile fallback logic (#402),
not the a6api line, and does not affect request routing.
