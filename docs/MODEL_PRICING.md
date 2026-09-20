# Model pricing and costs

jcode can price API calls from **your** price list instead of guessing. This is
the layer for *any* vendor you talk to: the cases it exists for are the ones
where models.dev has no entry for your model, or lists a price that differs from
what you actually pay. A file (or card) is grouped per vendor and keyed by model
id. The model is one idea:

> **`config.toml` names your own vendor rules.** Each `[pricing.providers.
> <vendor>]` entry either writes rate cards inline or points at a local JSON
> price file. A rule is matched by **model id**, so it prices that model no
> matter which route a call actually uses — unless the rule scopes itself with
> an optional `route = [...]` filter.

So the answer to "why is it this price?" has **two layers**:

1. **Your rules.** A `[pricing.providers.<vendor>]` card written inline in
   `config.toml` outranks a rule inside that vendor's `file`, and both outrank
   everything jcode derives itself. Rules are matched by model id, not by route
   key: a rule for `acme-small` prices that model whether it is reached directly
   through your own vendor or through a router/aggregator, which is what you
   want when models.dev has no entry or has a USD price where your vendor bills
   CNY. Add an optional `route = [...]` filter when one model id is billed at
   different prices per route.
2. **jcode's own chain**, used for anything your rules do not price: the
   curated static tables shipping with jcode, then provider-specific caches
   (OpenRouter endpoints), then the [models.dev](https://models.dev) catalog,
   then a generic fallback estimate.

An empty `[pricing]` section leaves the cost path exactly as it was before the
section existed.

## Quick start: point jcode at your price file

Two lines in `~/.jcode/config.toml`:

```toml
[pricing.providers.acme]
file = "prices.json"
currency = "CNY"
```

A bare name like `prices.json` resolves under `~/.jcode/cache/`, so that is
where this file lives. Any other value is a path (`~` expanded), which is what
you want for a file you keep elsewhere, e.g. `file = "~/pricing/prices.json"` or
`file = "/srv/pricing/prices.json"`. The file is a plain JSON document with a
`models` map at the top level, and **no outer vendor key** (the vendor is
already the `[pricing.providers.acme]` key):

```json
{
  "models": {
    "acme-large": {
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
```

`currency = "CNY"` says those numbers are CNY per million tokens; without it
they are USD, like models.dev. Editing `prices.json` takes effect at the next
lookup, because a local file is re-read when it changes — there is nothing to
wait for and no command to run. Any model the file does not mention keeps
falling through to jcode's own chain.

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

### `[pricing.providers.<vendor>]`

A vendor is **your own label**, not a route identity. It groups a set of rules,
inline or in a file, and the notes below apply to both.

```toml
[pricing.providers.acme]
file = "prices.json"       # optional: a local JSON file with the rules
currency = "CNY"           # currency of this vendor's numbers; default USD

# optional: write a rule directly, without a file
[pricing.providers.acme.models."acme-small".cost]
input = 1.0
output = 4.0
```

| Key | Meaning |
| --- | --- |
| `file` | Optional local price file for this vendor. A bare name (no path separator) resolves under `~/.jcode/cache/`; any other value is a path (absolute, or relative with a separator, `~` expanded). A bare name may not be one of jcode's own cache files. |
| `currency` | Currency every number under this vendor is denominated in, inline or in the file. Defaults to `USD`. |
| `models.<model>` | An inline rule for one model, in the shape of [A model rule](#a-model-rule) below. Outranks the same model in `file`. |

**Rules are matched by model id, not by route**, unless they opt into a
`route = [...]` filter. `[pricing.providers.acme.models."acme-small"]`
and an `acme-small` entry in that vendor's file both price a call for
`acme-small`, whichever route reports it. This is deliberate: binding every
rule to a route would silently fall back to models.dev's USD numbers for the
exact case the feature exists to fix (a model from your own vendor run through a
router/aggregator).

One model id can still cost different amounts per route. Real example: your
vendor bills `acme-small` at $0.15/$0.60, while the same model id through a
router such as OpenRouter costs $0.04844/$0.09688. Scope a rule to the route(s)
it is true for:

```toml
[pricing.providers.openrouter.models.acme-small]
route = ["openrouter"]
cost = { input = 0.04844, output = 0.09688 }
```

`route` is a list of **billing identities** — the same spelling as a vendor key
or activity source key: `"openrouter"`, `"acme"`, `"acme:api-key"`,
`"openai-compatible:acme"`. A compatible profile may also be named by its
short form, so `route = ["acme"]` matches both an `acme` call and an
`openai-compatible:acme` one. Entries are trimmed, order is kept, and
duplicates are allowed. An empty entry is rejected with its exact field path.

**An empty `route` applies to every route**, which keeps a rule without the key
exactly as it behaved before the key existed. A non-empty `route` applies only
to the routes it names: on any other route the rule is **skipped**, and the next
layer prices the call. It never produces a price, and never a "rule expired"
marker — a route the rule does not name was never its claim. So the failure mode
is a fall-through to the next layer, never a misprice.

```toml
# The same model at two per-route prices, each rule owning its own route.
[pricing.providers.acme.models.acme-small]
route = ["acme", "openai-compatible:acme"]
cost = { input = 0.15, output = 0.60 }

[pricing.providers.openrouter.models.acme-small]
route = ["openrouter"]
cost = { input = 0.04844, output = 0.09688 }
```

(The second rule can live in that vendor's `file` instead, and can use the same
`route` key there.)

Vendors are consulted in lexicographic order when two of them name the same
model, and an inline card always outranks a file. Within one vendor, the first
**applicable** rule that names the model decides.

### A vendor price file

A vendor file is a local JSON document. jcode **never fetches** it. Its only
accepted shape is a top-level `models` map:

```json
{
  "models": {
    "<model-id>": { "cost": { ... }, "tariffs": { ... }, "schedule": [ ... ] }
  }
}
```

There is no outer provider key. The old shape `{"acme": {"models": { ... }}}`
is rejected with a message naming the key to remove: the vendor is already the
`[pricing.providers.<vendor>]` config key, so repeating it in the file would be
ambiguous.

Each `models.<id>` value is exactly the rule shape an inline card uses, so a
file rule can carry `route`, `cost`, `tariffs`, `schedule`, `context_tiers`,
`default_tariff`, `effective_from`/`effective_until`, and `on_rule_expiry`. See
[A model rule](#a-model-rule) below.

**Out of effect falls through, and is labelled.** A file rule whose validity
window does not cover the call's instant is skipped, and the next layer prices
the call. Because the file is your own configuration, that price is marked with
the vendor name: the cost line shows `(rule expired (pricing.providers
\`acme\` file))`, and `/pricing` prints an `out of effect:` line for the same
reason. Without this the figure would silently switch from your file to
models.dev's number.

**Failure degrades, it never fabricates.** A file that is missing, unreadable,
not valid JSON, oversized, or out of effect simply does not price the call, and
the next layer does. A missing file is left alone for a short failure backoff
rather than re-attempted on every lookup. The size ceiling is 32 MiB, checked
both before and during the read so an oversized file is never fully buffered.

An invalid file is not fatal to jcode: the file contributes no rules for that
lookup and a line naming the problem goes to the log. An invalid `[pricing]`
field is different — the section is rejected as a whole, `/pricing` and the cost
display say `invalid [pricing]: pricing.providers.acme.file`, and every rule
in the section is ignored until you fix it.

**Currency follows the price here too.** A file under a vendor that states
`currency = "CNY"` prices in CNY and never inherits models.dev's USD numbers; a
vendor that states nothing is USD, like models.dev. A rule that cannot price a
call on its own is not relabelled with another layer's currency — the call goes
to the next layer instead.

### Inline rules: the escape hatch

A file is the intended way to price a model, but an inline card is still worth
knowing, because it is the only way to express field-level partial override at
the *card* layer, and because it outranks the file when both name a model.

```toml
[pricing.providers.acme]
currency = "CNY"

[pricing.providers.acme.models."acme-small".cost]
input = 1.0
```

An inline rule may state just `input` and keep the next layer's `output` for the
same currency, which is what a single rate change usually looks like. A card
written in a currency other than the next layer's never inherits that layer's
numbers (currency follows the price).

**Rates must be finite and non-negative.** `nan`, `inf`, or a negative number in
any rate field is rejected at load with the field path, for an inline card and
for a file rule alike: one `NaN` would otherwise poison the session total (it
renders as `NaN` and never recovers) and a negative rate would read as free.

The full card example — a vendor billed in CNY with peak/off-peak hours:

```toml
[pricing]
fx_base = "USD"

[pricing.fx_rates]
CNY = 7.20

[pricing.providers.acme]
currency = "CNY"

[pricing.providers.acme.models.acme-small.cost]
input = 1.0
output = 4.0
cache_read = 0.02

[pricing.providers.acme.models.acme-small.tariffs.peak]
multiplier = 2.0

[[pricing.providers.acme.models.acme-small.schedule]]
tariff = "peak"
utc_offset_minutes = 0
weekdays = ["Mon", "Tue", "Wed", "Thu", "Fri"]
windows = [["01:00", "04:00"], ["06:00", "10:00"]]
```

With this in place a call for that model costs ¥1/¥4 per million input/output tokens
(¥2/¥8 during peak), the widget and `/usage` show the amounts in CNY, and the
model picker orders this route by its converted cost next to USD-priced models.
Any model not named here keeps falling through to the layers below.

`on_rule_expiry = "no_price"` is available for an inline card: an expired rule
refuses to price the call rather than falling through to a worse estimate. A
vendor file rule has no refusal semantics — out of effect always means the next
layer prices the call, and the marker names the vendor.

### A model rule

The shape below is what `models.<model>` in a card means, and the same keys are
what a `models.<id>` entry in a vendor file may carry.

```toml
[pricing.providers.acme.models."acme-mini"]
route = ["acme", "openai-compatible:acme"]   # optional; omit to match every route
cost = { input = 4.5, output = 13.5, cache_read = 0.15 }
default_tariff = "off_peak"
effective_until = "2026-12-31T23:59:59Z"
on_rule_expiry = "fallback"
```

| Key | Meaning |
| --- | --- |
| `route` | Optional list of billing identities this rule is restricted to. Omit it (the default) and the rule matches every route, exactly as before the key existed. With a list, the rule applies only to the routes it names; on any other route it is skipped and the next layer prices the call (a fall-through, never a misprice and never a "rule expired" marker). |
| `cost` | Rates per **million tokens**, in the vendor's `currency`. All four components (`input`, `output`, `cache_read`, `cache_write`) are optional; an unpriced component means "this rule cannot price that part of the call" and the next layer of the same currency fills it. |
| `tariffs.<name>` | A named rate card: `multiplier = 2.0` multiplies `cost`, or write explicit `input`/`output`/`cache_read`/`cache_write` rates. |
| `schedule` | When a tariff applies. An array of tables, please: `[[pricing...schedule]]`. |
| `context_tiers` | Long-context rates. An array of tables: `[[pricing...context_tiers]]`, described below. |
| `default_tariff` | Tariff used when no schedule window matches. Without it, `cost` applies as written. |
| `effective_from` / `effective_until` | RFC 3339 instants bounding the rule's validity, e.g. `2026-12-31T23:59:59Z`. |
| `on_rule_expiry` | What an out-of-validity **inline card** does: `"fallback"` (default) lets the next layer price the call and marks the rule expired where the cost is shown, `"no_price"` refuses to price it at all (nothing is billed, and the cost line says `(rule cannot price this call)` so the resulting zero is not mistaken for a free call). A vendor file rule ignores it: out of effect always falls through. |

### Schedule rules

```toml
[[pricing.providers.acme.models.acme-small.schedule]]
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
[[pricing.providers.acme.models."acme-large".context_tiers]]
min_input_tokens = 200_000
multiplier = 2.0

[[pricing.providers.acme.models."acme-large".context_tiers]]
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
  no multi-line inline tables. (This is why the card syntax is verbose; a JSON
  vendor file has no such rule.)
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
  layer priced it (naming the vendor and file when a file did), the tariff in
  force right now, the currency, the state of the `[pricing]` section, and the
  rate table. Use it instead of guessing from the cost on screen.
* Saving your price file is enough: a local file is re-read when it changes, and
  saving `config.toml` makes a running session report `Config reloaded from disk`.
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
