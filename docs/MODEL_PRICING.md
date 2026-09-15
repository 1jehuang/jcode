# Model pricing and costs

jcode prices an API call from the layers below, highest priority first:

1. a **hand-written `[pricing.providers]` rule** in `~/.jcode/config.toml` (this document),
2. the curated static tables shipping with jcode,
3. provider-specific caches (OpenRouter endpoints),
4. the [models.dev](https://models.dev) catalog,
5. a generic fallback estimate.

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
| `default_tariff` | Tariff used when no schedule window matches. Without it, `cost` applies as written. |
| `effective_from` / `effective_until` | RFC 3339 instants bounding the rule's validity, e.g. `2026-12-31T23:59:59Z`. |
| `on_rule_expiry` | What an out-of-validity rule does: `"fallback"` (default) lets the next layer price the call and marks the rule expired where the cost is shown, `"no_price"` refuses to price it at all. |

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

### Per-call pinning

The rate card and its tariff are resolved once, at the instant of a call's first
usage snapshot, and pinned to that call. A call that starts off-peak and finishes
during peak bills entirely at the rate it started with, and a config edit
mid-call does not re-price a call in flight.

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

## Checking that it took effect

* **`/pricing`** answers "why is it this price?" for the current model: which
  layer priced it, the tariff in force right now, the currency, the state of the
  `[pricing]` section, and the rate table. Use it instead of guessing from the
  cost on screen.
* Saving the file is enough: a running session reports `Config reloaded from disk`.
* `/usage` lists the spend per currency and marks a rule that expired or was
  rejected, so a rule that stopped applying is visible instead of silently
  changing the price.
* Write a rule you can verify by hand, then run one small request and compare the
  cost shown against the arithmetic above.
