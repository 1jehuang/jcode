# Model pricing and costs

jcode prices an API call from the layers below, highest priority first:

1. a **hand-written `[pricing.providers]` rule** in `~/.jcode/config.toml` (this document),
2. an **extra `[[pricing.sources]]` price sheet** named in the same file: a URL or
   a local file in models.dev's own JSON shape,
3. the curated static tables shipping with jcode,
4. provider-specific caches (OpenRouter endpoints),
5. the [models.dev](https://models.dev) catalog,
6. a generic fallback estimate.

The config layer therefore outranks everything else, and an empty `[pricing]`
section leaves the cost path exactly as it was before the section existed.

## Quick start: DeepSeek billed in CNY with peak/off-peak

DeepSeek publishes CNY prices and charges double during peak hours. This is the
whole rule, and every field in it is explained below:

```toml
[pricing]
fx_base = "USD"

[pricing.fx_rates]
CNY = 7.20

[pricing.providers.deepseek]
currency = "CNY"

[pricing.providers.deepseek.models.deepseek-flash.cost]
input = 1.0
output = 4.0
cache_read = 0.02

[pricing.providers.deepseek.models.deepseek-flash.tariffs.peak]
multiplier = 2.0

[[pricing.providers.deepseek.models.deepseek-flash.schedule]]
tariff = "peak"
utc_offset_minutes = 0
weekdays = ["Mon", "Tue", "Wed", "Thu", "Fri"]
windows = [["01:00", "04:00"], ["06:00", "10:00"]]
```

With this in place a flash call costs ¥1/¥4 per million input/output tokens
(¥2/¥8 during peak), the widget and `/usage` show the amounts in CNY, and the
model picker orders this route by its converted cost next to USD-priced models.
Any model or provider not named here keeps falling through to the layers above.

## Reference

### `[pricing]`

| Key | Meaning |
| --- | --- |
| `fx_base` | Base currency for the rate table. Defaults to `USD`. |
| `fx_rates` | `1 {fx_base} = N {currency}` for every currency you use, e.g. `CNY = 7.20`. |

There is **no automatic rate fetching** in v1: you write the rates you want to be
billed and compared by. They are used in two places, and nowhere else:

* `[display] currency = "CNY"` converts every cost into that currency for display;
  `"native"` (the default) shows each provider in its own currency.
* The model picker orders routes by price. Estimates denominated in different
  currencies are compared **after conversion**. A currency with no rate is never
  guessed: those routes keep their native currency and sort below every
  comparable route, instead of being ordered as if `¥7` and `$7` were the same
  number.

### `[pricing.providers.<key>]`

The key identifies a provider. It accepts the runtime provider key
(`deepseek`), the scoped runtime key (`openai-compatible:deepseek`), and a
compatible profile's id or display name; `openai-compatible:foo` and `foo` are
the same profile. A key that matches nothing is reported once in the log, so a
typo does not silently do nothing.

**Cards match by provider *key*, not by model vendor.** Nothing here inspects
which company made the model: a card applies to the route whose activity key
matches, and the same model on a different route is a different key. A
`[pricing.providers.deepseek]` card prices the DeepSeek provider (and
`openai-compatible:deepseek`), but a DeepSeek model reached through
`openrouter` is keyed `openrouter`, so the card does not apply and that route
falls through to models.dev (in USD). If you want the card to cover both, write
a second entry under the other key (or `[[pricing.sources]]` with a `scope` that
lists both forms). `/pricing` prints the key it looked up, which is the fastest
way to see why a card did not take effect.

**Rates must be finite and non-negative.** `nan`, `inf`, or a negative number in
any rate field is rejected at load with the field path, for a hand-written card
and for a sheet alike: one `NaN` would otherwise poison the session total (it
renders as `NaN` and never recovers) and a negative rate would read as free.

| Key | Meaning |
| --- | --- |
| `currency` | ISO 4217 code for every rate written under this provider. Defaults to `USD`. |
| `models.<model>` | A rule for one model, described next. |

**Currency follows the price.** A card written in CNY never inherits USD numbers
from the layer below it, and vice versa: if a card cannot price a call (say it
lists `input` but no `output`), the call is reported as unpriced rather than
being topped up with another currency's rates.

### A model rule

```toml
[pricing.providers.deepseek.models."deepseek-v4-pro"]
cost = { input = 4.5, output = 13.5, cache_read = 0.15 }
default_tariff = "off_peak"
effective_until = "2026-12-31T23:59:59Z"
on_rule_expiry = "fallback"
```

| Key | Meaning |
| --- | --- |
| `cost` | Rates per **million tokens**, in the provider's `currency`. All four components (`input`, `output`, `cache_read`, `cache_write`) are optional; an unpriced component means "this card cannot price that part of the call". |
| `tariffs.<name>` | A named rate card: `multiplier = 2.0` multiplies `cost`, or write explicit `input`/`output`/`cache_read`/`cache_write` rates. |
| `schedule` | When a tariff applies. An array of tables, please: `[[pricing...schedule]]`. |
| `context_tiers` | Long-context rates. An array of tables: `[[pricing...context_tiers]]`, described below. |
| `default_tariff` | Tariff used when no schedule window matches. Without it, `cost` applies as written. |
| `effective_from` / `effective_until` | RFC 3339 instants bounding the rule's validity, e.g. `2026-12-31T23:59:59Z`. |
| `on_rule_expiry` | What an out-of-validity rule does: `"fallback"` (default) lets the next layer price the call and marks the rule expired where the cost is shown, `"no_price"` refuses to price it at all (nothing is billed, and the cost line says `(rule cannot price this call)` so the resulting zero is not mistaken for a free call). |

### Schedule rules

```toml
[[pricing.providers.deepseek.models.deepseek-flash.schedule]]
tariff = "peak"
utc_offset_minutes = 0
weekdays = ["Mon", "Tue", "Wed", "Thu", "Fri"]
windows = [["01:00", "04:00"], ["06:00", "10:00"]]
```

* `tariff` — the `tariffs.<name>` entry this window selects.
* `utc_offset_minutes` — a fixed offset applied to the UTC instant. v1 has no
  IANA timezone support, so write the offset you mean; `480` is UTC+8.
* `weekdays` — decided on the **offset-shifted date**. Omit it, or leave it
  empty, for every day.
* `windows` — `[start, end)` in offset-shifted time. A `start` later than its
  `end` wraps past midnight and belongs to the start day. An empty list means the
  whole day.

Rules are matched **in declaration order and the first match wins**, so put the
narrow exceptions first.

### Long-context tiers

Some models charge more once the request is large. Say it with a
`context_tiers` array:

```toml
[[pricing.providers.deepseek.models."deepseek-v4-pro".context_tiers]]
min_input_tokens = 200_000
multiplier = 2.0

[[pricing.providers.deepseek.models."deepseek-v4-pro".context_tiers]]
min_input_tokens = 500_000
input = 9.0
output = 27.0
```

* `min_input_tokens` — the tier applies when the call's reported input token
  count is **strictly greater** than this. A call reporting exactly `200000`
  input tokens is still on the base tier; `200001` crosses.
* The rest of the entry uses the same vocabulary as `tariffs.<name>`: either a
  `multiplier` on the rate card the schedule selected, or explicit
  `input`/`output`/`cache_read`/`cache_write` rates. An explicit tier overrides
  only the fields it writes and keeps the rest, exactly like a tariff.
* Tiers are matched **in declaration order, first match wins**.
* A tier is chosen from the input token count of the call's **first usage
  snapshot**, and is then pinned with the rest of the card: a call that grows
  past a threshold mid-flight keeps the rate it started with.

The same shape is what jcode builds from models.dev's native
`context_over_200k` rates, so a model priced from models.dev gets its
long-context rates too, with no configuration at all.

**A caller with no token count prices the base tier.** The cheapness ordering in
the model picker, `effective_cost`, and the `/pricing` reference figure cannot
know a call's size, so they use the base rates — the ones below the first
`min_input_tokens`. `/pricing` names the thresholds in force next to that figure
(yours, or models.dev's own `context_over_200k` when no rule of yours prices the
model), so the base rate is not mistaken for the only one.

Once your own rule claims a model, it owns the whole card, tiers included:
models.dev's `context_over_200k` rates are not merged into it, because a lower
layer's absolute tier would silently overwrite the rates you wrote.

### Extra price sheets: `[[pricing.sources]]`

A source is a second price list you point jcode at: your own mirror of
models.dev, a vendor's published rate sheet, a file you keep in a git repo.
Sources sit **below your own rules and above every catalog jcode derives
itself**, so pointing at your own sheet is never a no-op.

Both halves of the example below are executable: the config names the sheet, and
the sheet is a JSON document in models.dev's shape.

```toml
[[pricing.sources]]
id = "corp-mirror"                                     # unique; ties break on this id
url = "file:///opt/jcode/pricing.json"                 # or https://gitlab.internal/pricing.json
scope = ["deepseek", "openai-compatible:my-gateway"]   # omit = every provider
models = ["deepseek-v4-*"]                             # omit = every model under scope
refresh_secs = 86400                                   # per-source TTL, default 24h
priority = 10                                          # lower wins; default 0
```

```json
{
  "deepseek": {
    "models": {
      "deepseek-v4-pro": {
        "cost": { "input": 4.5, "output": 13.5, "cache_read": 0.15 },
        "tariffs": { "peak": { "multiplier": 2.0 } },
        "schedule": [
          {
            "tariff": "peak",
            "utc_offset_minutes": 0,
            "weekdays": ["Mon", "Tue", "Wed", "Thu", "Fri"],
            "windows": [["01:00", "04:00"]]
          }
        ]
      }
    }
  }
}
```

| Key | Meaning |
| --- | --- |
| `id` | Name of this source. It is the tie-break key between two sources with the same `priority`, so it must be unique. |
| `url` | `https://…` or a local file: `file:///path/to/pricing.json`, or a bare path. No other scheme is accepted. |
| `scope` | Which provider identities this sheet may price. Omit it, or leave it empty, to cover every provider. |
| `models` | Model globs (`*` any run, `?` one character, case-insensitive). Omit for every model under `scope`. |
| `format` | Sheet schema. Only `models_dev_v1` exists, and it is the default. |
| `refresh_secs` | How long a fetched copy may be reused before the source is refreshed. Default `86400` (24h), the same as models.dev. |
| `priority` | Lower wins between sources. Default `0`; ties are broken by `id` in lexicographic order, so the merge never depends on map order. |
| `currency` | Currency every number in the sheet is denominated in. Defaults to `USD`, the same implicit currency models.dev uses. |

**Scope uses the same identity rules as a `[pricing.providers]` key.** One
provider has several names, and `scope` accepts any of them:

* the exact activity key: `scope = ["claude:api-key"]`, `["openrouter"]`;
* the models.dev provider id: `scope = ["deepseek"]` covers both the fallback
  slug `deepseek` and the compatible-profile route `openai-compatible:deepseek`;
* a compatible profile's id, with or without its prefix: `["my-gateway"]` and
  `["openai-compatible:my-gateway"]` are the same route.

A sheet addresses providers the same way the upstream catalog does (by provider
id, by model id under `models`), and its entries understand the same extension
fields a hand-written rule does: `cost`, `tariffs`, `schedule`, `context_tiers`,
`default_tariff`, `effective_from`/`effective_until`, `on_rule_expiry`. So a
sheet can carry peak/off-peak hours or a promotion that expires, and the call is
priced at the rate in effect at the call's own instant, exactly like a rule you
wrote yourself.

**A sheet has no `fallback`/`no_price` choice: out of effect always means "the
next layer prices the call".** A `[pricing.providers]` rule may set
`on_rule_expiry = "no_price"` to refuse a price outright; a sheet may state
`effective_from`/`effective_until` but that refusal semantics is deliberately not
part of a sheet, so a sheet's out-of-effect rule always falls through to the next
source (or models.dev). A sheet that states `on_rule_expiry = "no_price"` still
falls through — the field is parsed but only the validity bounds decide anything
for a sheet.

**The fall-through is labelled where you read the price.** A sheet is your own
configuration, so a sheet rule that is out of effect at the call's instant is
surfaced exactly like an expired hand-written rule, with the sheet named:
the cost line shows `(rule expired (pricing source \`corp-mirror\`))`, and
`/pricing` prints an `out of effect:` line for the same reason. Without this the
figure would silently switch from your sheet to models.dev's number. The next
layer still prices the call at its own rate — the marker is what changes.

**Failure degrades, it never fabricates.** A sheet that is unreachable,
unparseable, stale, or out of effect simply does not price the call, and the next
layer does. Nothing is invented to fill the gap, and a call that models.dev can
price is never left unpriced:

* a `file://` sheet is read from disk; a file that is missing or is not valid
  JSON is skipped for that lookup, with a warning in the log;
* an `https://` sheet is read from a cache under `~/.jcode/cache/`. It is fetched
  in the background, so a lookup never waits on the network. Until the fetch
  succeeds the source is unused, and the last successful copy is kept rather than
  discarded;
* a sheet whose `refresh_secs` has elapsed is **not** used while its refresh is
  outstanding: a price that may be hours stale is not silently billed;
* a sheet whose rule is out of effect at the call's instant (its
  `effective_until` passed, or its `effective_from` has not arrived) is skipped,
  and the next source or models.dev prices the call — and, as above, that price
  carries a marker naming the sheet.

**Currency follows the price here too.** A sheet that states `currency = "CNY"`
prices in CNY and never inherits models.dev's USD numbers; a sheet that states
nothing is USD, like models.dev. A sheet that cannot price a call on its own is
not relabelled with another layer's currency — the call goes to the next layer
instead.

An invalid `[[pricing.sources]]` entry is reported like any other invalid
`[pricing]` field: the section is rejected as a whole, `/pricing` and the cost
display say `invalid [pricing]: pricing.sources[0].url`, and a line naming the
problem goes to the log.

**A sheet entry is all-or-nothing, unlike your own card.** A hand-written
`[pricing.providers]` rule may be partial, and the missing fields fall through to
the next layer of the same currency. A sheet entry may not: it must state both
`cost.input` and `cost.output`, and an entry that states only one is dropped
entirely (the source simply does not price that model, and the next layer does).
That restricts how much of a sheet a broken field can affect, at the cost of
having to write both directions.

**The sheet URL is recorded, redacted.** jcode persists what it fetched from in
`~/.jcode/cache/pricing_sources.json`, and writes the URL as `scheme://host/path`
with the query string and any userinfo removed, so a token in the query is never
logged or saved. The schema has no header field yet, so a mirror that needs
authentication has to put the credential in the URL — which means the credential
still lives in your `config.toml` in the clear. Prefer a private network or an
unauthenticated path today, and move the secret into a header field once one
exists.

Two things a source does **not** do: it cannot change the cache-write premium
billing applies for Anthropic models (that stays a hand-written card's
privilege), and it cannot override a field your own rule wrote.

### Per-call pinning

The rate card and its tariff are resolved once, at the instant of a call's first
usage snapshot, and pinned to that call. A call that starts off-peak and finishes
during peak bills entirely at the rate it started with, and a config edit
mid-call does not re-price a call in flight.

### Rollback: the `*_usd` mirrors

`~/.jcode/provider_activity.json` is shared with older jcode binaries, which only
know one USD figure per window. Every spend window therefore keeps a `*_usd`
mirror beside its per-currency buckets: the **naive sum** of that window's
amounts, written only so an older reader sees a total instead of zero when it
rewrites the file. It is deliberately *not* a converted total. With a single
currency in the window the mirror is exact; with mixed currencies it is not, so a
rollback re-labels the window as USD (`{CNY 30, USD 5}` is read back by the older
binary as one `USD 35` bucket). This build never treats the mirror as USD while
the buckets are present — the buckets are the source of truth — and this trade-off
stays: a reader that only understands USD cannot be handed a correct conversion it
has no rates for.

## TOML gotchas

These are worth reading once, because each one breaks more than it looks like:

* **Inline tables must be on one line.** `cost = { input = 4.5, output = 13.5 }`
  is valid; spreading it across lines with a trailing comma is not. TOML 1.0 has
  no multi-line inline tables.
* **Schedules are arrays of tables** (`[[...schedule]]`), not a `schedule = [ {…} ]`
  inline array.
* **A syntax error anywhere in the file makes jcode ignore the whole file**, not
  just the `[pricing]` part: every setting reverts to its default. Current builds
  say so in the session and re-arm the notice once the file parses again; older
  builds only wrote a line to the log.
* **A settings change now preserves the file.** Changing a setting reloads,
  patches, and saves through the existing file: your comments and any section a
  newer build wrote are kept, and only the keys jcode models are rewritten. The
  one thing a save can still drop is a setting you deliberately reset (for
  example a cleared `/colors` or a cleared default model), because "empty" and
  "not written by this build" look identical in the file.
* **Entries inside an array of tables are rewritten wholesale.** A
  `[[providers.<name>.models]]` or `[[...schedule]]` entry is a value, not a
  sub-table jcode patches key by key, so comments *inside* such an entry are
  lost on the next save (comments around it survive). This is deliberate: a
  field spelled with a serde alias (for example `context-window` for
  `context_window`) would otherwise be kept beside the canonical name the struct
  writes, and serde reports both at once as `duplicate field` and refuses the
  whole file. If a merge would ever produce a file jcode cannot parse, the save
  falls back to a plain write instead of leaving an unusable config.

## Checking that it took effect

* **`/pricing`** answers "why is it this price?" for the current model: which
  layer priced it, the tariff in force right now, the currency, the state of the
  `[pricing]` section, and the rate table. Use it instead of guessing from the
  cost on screen.
* Saving the file is enough: a running session reports `Config reloaded from disk`.
* The **session cost line** (the amount under the context meter, and the same
  figure in the info widget) carries a short note when the number on screen is
  not what your config asked for: `(rule expired)` / `(rule not in effect yet)`
  when a rule stopped applying and a lower layer priced the call, `(invalid
  [pricing]: <path>)` when the section was rejected, and `(rule cannot price
  this call)` when your own rule refused to price the call at all. `/pricing`
  explains the same cases in more detail. `/usage` lists the spend per currency;
  it does not carry these markers.
* Write a rule you can verify by hand, then run one small request and compare the
  cost shown against the arithmetic above.
