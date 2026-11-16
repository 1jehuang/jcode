// Model price matching shared by scripts/sync-model-prices.mjs (manual runs)
// and the worker's nightly cron (automatic runs).
//
// Why the cron: prices used to be synced only by hand, so every new model
// (claude-opus-5-5 being the big one) landed with no price row and the spend
// dashboards silently priced it at $0. Spend appeared to fall from ~$30K/day
// to ~$2K/day while real usage was flat. Running the same matcher nightly
// keeps newly observed labels priced without anyone remembering to do it.
//
// Source catalog: the free models.dev API, the same one the CLI uses in
// crates/jcode-base/src/model_pricing.rs.

export const CATALOG_URL = "https://models.dev/api.json";

// Providers whose reported input token count already includes cache reads.
// OpenAI-compatible APIs report `prompt_tokens_details.cached_tokens` as a
// subset of `prompt_tokens`, so pricing input and cache-read separately would
// bill the cached portion twice. Anthropic's Messages API reports
// `input_tokens` and `cache_read_input_tokens` as disjoint buckets.
// Keep in sync with jcode-compaction-core's estimate_compaction_tokens note.
export const CACHE_INCLUSIVE_PROVIDERS = new Set([
  "openai",
  "openrouter",
  "copilot",
  "gemini",
  "google",
  "xai",
  "groq",
  "cerebras",
  "deepseek",
  "moonshot",
  "zai",
  "minimax",
  "mistral",
  "ollama",
  "openai-compatible",
]);

// jcode's own internal route labels, which are real priced traffic but are not
// models.dev ids. Mapped explicitly so they do not land in the unpriced bucket.
const LABEL_ALIASES = new Map([
  // The auto code-review pass runs on the Codex model family.
  ["codex-auto-review", "gpt-5.3-codex"],
  ["codex-auto-review-spark", "gpt-5.3-codex-spark"],
]);

// Only explicit free routes and local/mock model labels are known to cost $0.
// Unknown/router aliases and subscription/cloud routes are NOT free models:
// their underlying inference still has list-price value. Keep them unpriced
// when resolution fails so missing attribution is visible in coverage.
const FREE_LABEL_PATTERNS = [
  /^mock$/i,
  /^free$/i,
  /^openrouter\/free$/i,
  /:free$/i,
  /-free$/i,
  /^ollama\//i,
  /^qwen[\d.]*(-coder)?:\d+b$/i,
  /^qwen\d[\d.]*:\d+b$/i,
];

// Labels that can only be a model running on the user's own hardware: weight
// file formats, quantization tags, Ollama size tags, filesystem paths. There
// is no list price for local inference, so these count as free instead of
// sitting in the unpriced bucket beside hosted traffic. A bare `:latest` tag
// or an `uncensored` name is not proof: gateways use both for hosted routes.
const LOCAL_LABEL_PATTERNS = [
  /gguf|(^|[-_.\s/])(mlx|omlx|nvfp4|fp8|imatrix)([-_.\s:]|$)/i,
  /(^|[-_.\s:])(ud-)?i?q\d(_[a-z0-9]+)+/i,
  /(^|[-_.\s])iq\d[-_]?(xxs|xs|s|m)\b/i,
  /(^|[-_.\s])\d+-?bit\b/i,
  /:e?\d+(\.\d+)?b(-[\w.-]+)?$/i,
  /^(hf\.co|local|lmstudio|unsloth)\//i,
  /^(\/|~\/|[a-z]:\\)/i,
];

const EFFORT = "xhigh|x-high|max-effort|max|high|medium|low|none|minimal";

// Normalize a telemetry model label toward models.dev model ids. Users reach
// the same model through many gateways that prefix or rename it. Returns
// candidates in priority order, most literal first.
export function normalizeLabel(label) {
  let s = String(label ?? "").trim();
  const alias = LABEL_ALIASES.get(s.toLowerCase());
  if (alias) return [alias];
  // Strip jcode's own suffixes/markers.
  s = s.replace(/\[1m\]$/i, "").replace(/\[web\]$/i, "");
  // Parenthesized reasoning effort from routers: `cc/claude-opus-5-5(high)`.
  s = s.replace(new RegExp(`\\((${EFFORT})\\)$`, "i"), "");
  // Strip a trailing `@Provider` disambiguator (`...-4-8@Anthropic`), but not
  // a leading `@cf/...` namespace, which used to erase the whole label.
  s = s.replace(/(.)@[^@/]+$/, "$1");
  // Ollama cloud tags (`deepseek-v4.1-flash:cloud`, `...:1017-cloud`) name the
  // hosting route, not a different model. The base model keeps its list price.
  s = s.replace(/:(?:[\w.]+-)?cloud$/i, "");
  // Reasoning-effort suffixes (`-xhigh`, `-high`) never change price.
  s = s.replace(/-(xhigh|x-high|max-effort)$/i, "");
  const candidates = new Set([s]);
  // jcode route prefix `provider:model` (`anthropic-oauth:opus-5-5`,
  // `zai-api:glm-5.3`). Only the first colon, and only when the left side
  // looks like a route name rather than part of a model id.
  const colon = s.match(/^([a-z][a-z0-9_-]*):(.+)$/i);
  // `:latest` / `:0` style tags leave a right side with no model name in it.
  if (colon && /[a-z]{2}.*\d|\d.*[a-z]{2}/i.test(colon[2])) candidates.add(colon[2]);
  // Gateway/route prefixes: `cc/`, `oc/`, `cx/`, `kr/`, `ag/`, `nvidia/z-ai/`.
  for (const c of [...candidates]) {
    const parts = c.split("/");
    if (parts.length > 1) {
      candidates.add(parts[parts.length - 1]);
      candidates.add(parts.slice(1).join("/"));
    }
  }
  // Effort suffixes can also sit on the gateway-stripped form.
  for (const c of [...candidates]) {
    candidates.add(c.replace(new RegExp(`-(${EFFORT})$`, "i"), ""));
  }
  for (const c of [...candidates]) {
    // Anthropic dated ids: claude-opus-4-5-20251101 -> claude-opus-4-5.
    let undated = c.replace(/-\d{8}$/, "");
    // Bare Anthropic family names: `opus-5-5` -> `claude-opus-5-5`.
    if (/^(opus|sonnet|haiku)-\d/i.test(undated)) undated = `claude-${undated}`;
    candidates.add(undated);
    // Dotted vs dashed Anthropic spellings: claude-sonnet-4.6 <-> 4-6.
    if (/^claude-/i.test(undated)) {
      candidates.add(undated.replace(/(\d)\.(\d)/g, "$1-$2"));
      candidates.add(undated.replace(/(\d)-(\d)(?!\d)/g, "$1.$2"));
    }
    // Thinking/highspeed/tiered variants price the same as the base model.
    // `-fast` and `-preview` are priced at the base rate too: a premium fast
    // tier is understated, but that is closer than counting it as $0.
    candidates.add(undated.replace(/-(thinking|highspeed|tiered|high|medium|low|fast|preview)$/i, ""));
  }
  return [...candidates].filter(Boolean);
}

// Spelling-independent token form of a model name, so the same release matches
// however a gateway writes it: `deepseek-4-1-flash`, `DeepSeek V4.1 Flash` and
// `deepseek-v4.1-flash` all become [deepseek, 41, flash], and `qwen38-27b`
// meets `qwen3.8-27b` at [qwen, 38, 27b]. Version digits are glued, so
// `gpt-5-5` is [gpt, 55] and never contains `gpt-5` ([gpt, 5]).
export function canonTokens(name) {
  const raw = String(name ?? "").toLowerCase()
    .replace(/[^a-z0-9]+/g, " ")
    .replace(/([a-z])(\d)/g, "$1 $2")
    .split(" ").filter(Boolean);
  const out = [];
  for (let i = 0; i < raw.length; i++) {
    const t = raw[i];
    if (t === "v" && /^\d/.test(raw[i + 1] ?? "")) continue;
    const prev = out[out.length - 1];
    if (/^\d$/.test(t) && prev && /^\d{1,2}$/.test(prev)) out[out.length - 1] = prev + t;
    else out.push(t);
  }
  return out;
}

// Catalog ids by token form. Only names specific enough to identify a release:
// a real word plus a version, never a bare word that an alias could contain.
function buildTokenIndex(index) {
  const byKey = new Map();
  for (const [modelId, entries] of index) {
    if (isZeroCost(entries[0].cost) || /free$/i.test(modelId)) continue;
    const base = modelId.slice(modelId.lastIndexOf("/") + 1).split(":")[0];
    const tokens = canonTokens(base);
    if (tokens.length < 2 || !tokens.some((t) => /^\d/.test(t)) || !tokens.some((t) => /^[a-z]{3,}$/.test(t))) continue;
    const key = tokens.join("-");
    const prev = byKey.get(key);
    const best = { modelId, ...entries[0] };
    if (!prev || compareEntries(best, prev.entry) < 0) byKey.set(key, { tokens, entry: best });
  }
  const byFirst = new Map();
  for (const item of byKey.values()) {
    if (!byFirst.has(item.tokens[0])) byFirst.set(item.tokens[0], []);
    byFirst.get(item.tokens[0]).push(item);
  }
  // Longest first, so the most specific release wins.
  for (const list of byFirst.values()) list.sort((a, b) => b.tokens.length - a.tokens.length);
  return { byKey, byFirst };
}

function tokenEstimate(candidates, tokenIndex) {
  let best = null;
  for (const candidate of candidates) {
    let tokens = canonTokens(candidate);
    // Gateways drop the vendor word: `antigravity-opus5.5` is claude-opus-5-5.
    const tier = tokens.findIndex((t) => /^(opus|sonnet|haiku|fable)$/.test(t));
    if (tier >= 0 && !tokens.includes("claude")) tokens = [...tokens.slice(0, tier), "claude", ...tokens.slice(tier)];
    // A release still in preview is often labelled without the suffix.
    const preview = tokenIndex.byKey.get([...tokens, "preview"].join("-"));
    for (let at = 0; at < tokens.length; at++) {
      for (const item of tokenIndex.byFirst.get(tokens[at]) ?? []) {
        if (best && item.tokens.length <= best.tokens.length) break;
        if (item.tokens.every((t, i) => tokens[at + i] === t)) best = item;
      }
    }
    if (!best && preview) best = preview;
  }
  return best?.entry ?? null;
}

export function isFreeLabel(label) {
  // Ollama cloud inference is not established as free by its route prefix.
  if (/:cloud$/i.test(label)) return false;
  return FREE_LABEL_PATTERNS.some((re) => re.test(label));
}

// Checked only after an exact catalog match fails, so a hosted model whose id
// happens to carry a quantization tag keeps its real price.
export function isLocalLabel(label) {
  if (/[:-]cloud$/i.test(label)) return false;
  return LOCAL_LABEL_PATTERNS.some((re) => re.test(label));
}

// Router placeholders say nothing about the model behind them. Some gateways
// publish a catalog id with the same name (`auto`), which must not price them.
const ROUTER_LABEL = /^(auto|default|unknown|router|pool\d*)$/i;

export function buildPriceIndex(catalog) {
  // model id (lowercased) -> [{provider, cost}], provider-preference sorted.
  const index = new Map();
  for (const [providerId, provider] of Object.entries(catalog ?? {})) {
    for (const [modelId, model] of Object.entries(provider?.models ?? {})) {
      const cost = model?.cost;
      if (!cost || typeof cost.input !== "number") continue;
      const key = modelId.toLowerCase();
      if (!index.has(key)) index.set(key, []);
      index.get(key).push({ provider: providerId, cost });
    }
  }
  for (const entries of index.values()) entries.sort(compareEntries);
  return index;
}

const PROVIDER_PREFERENCE = [
  "anthropic", "openai", "google", "xai", "deepseek", "zai", "zhipuai",
  "moonshotai", "minimax", "openrouter",
];

function isZeroCost(cost) {
  return !(cost.input > 0) && !(cost.output > 0);
}

function compareEntries(a, b) {
  // Coding-plan and token-plan providers list every model at $0 because the
  // plan is billed separately. That is not the model's list price, so a real
  // rate always wins. Explicitly free labels are handled by isFreeLabel.
  const az = isZeroCost(a.cost);
  const bz = isZeroCost(b.cost);
  if (az !== bz) return az ? 1 : -1;
  const ai = PROVIDER_PREFERENCE.indexOf(a.provider);
  const bi = PROVIDER_PREFERENCE.indexOf(b.provider);
  // Known first-party providers first, then the cheapest-listed as a proxy
  // for "most likely the real rate" rather than an arbitrary map order.
  if (ai !== bi) return (ai < 0 ? 999 : ai) - (bi < 0 ? 999 : bi);
  return a.cost.input - b.cost.input;
}

// Catalog ids that exist only with a vendor prefix (`meituan/longcat-2.5`),
// keyed by their last segment, so a bare `LongCat-2.5` label still resolves.
// Versioned names only: a bare word like `high` must never match by suffix.
function buildBasenameIndex(index) {
  const basenames = new Map();
  for (const [modelId, entries] of index) {
    const slash = modelId.lastIndexOf("/");
    if (slash < 0) continue;
    const base = modelId.slice(slash + 1);
    if (!/\d/.test(base) || !/[a-z]{2}/.test(base) || index.has(base)) continue;
    if (!basenames.has(base)) basenames.set(base, []);
    basenames.get(base).push(...entries);
  }
  for (const entries of basenames.values()) entries.sort(compareEntries);
  return basenames;
}

// First candidate with a price. A first-party listing beats a reseller's
// discounted one, even when the reseller uses the more literal spelling.
function pickEntry(candidates, source) {
  let fallback = null;
  let zero = null;
  for (const candidate of candidates) {
    const entries = source.get(candidate.toLowerCase());
    if (!entries?.length) continue;
    const hit = { candidate, ...entries[0] };
    if (isZeroCost(hit.cost)) {
      zero ??= hit;
      continue;
    }
    if (PROVIDER_PREFERENCE.includes(hit.provider)) return hit;
    fallback ??= hit;
  }
  // A $0 listing is used only when no spelling of the label has a real rate.
  return fallback ?? zero;
}

// Resolve observed {model, provider} rows to one price row per label.
export function resolvePriceRows(observed, index) {
  const families = buildFamilyIndex(index);
  const basenames = buildBasenameIndex(index);
  const embedded = buildEmbeddedIndex(index);
  const tokenIndex = buildTokenIndex(index);
  const rows = new Map();
  for (const row of observed) {
    const label = row.model;
    if (label == null || rows.has(label)) continue;
    const providerKey = String(row.provider ?? "").toLowerCase();
    const cacheInclusive = CACHE_INCLUSIVE_PROVIDERS.has(providerKey) ? 1 : 0;
    const base = { model: label, cacheInclusive };

    if (isFreeLabel(label)) {
      rows.set(label, {
        ...base, sourceModel: null, sourceProvider: null,
        input: 0, output: 0, cacheRead: 0, cacheWrite: 0, kind: "free",
      });
      continue;
    }

    const candidates = normalizeLabel(label);
    const router = ROUTER_LABEL.test(String(label).trim());
    const match = router ? null
      : pickEntry(candidates, index) ?? pickEntry(candidates, basenames);
    if (!match) {
      // A router placeholder gets no estimate either: `pool12` is not a model.
      let estimate = router ? null : familyEstimate(label, families);
      if (!estimate && !router) {
        // Local weights are checked before the looser token match, so a
        // quantized build is not priced like the hosted release it derives from.
        const local = isLocalLabel(label);
        const wrapped = embeddedEstimate(candidates, embedded)
          ?? (local ? null : tokenEstimate(candidates, tokenIndex));
        if (!wrapped && local) {
          rows.set(label, {
            ...base, sourceModel: null, sourceProvider: null,
            input: 0, output: 0, cacheRead: 0, cacheWrite: 0, kind: "free",
          });
          continue;
        }
        // Re-resolve the wrapped id so `claude-opus-5.5` inside a gateway label
        // gets Anthropic's rate, not the reseller that spells it with a dot.
        const best = wrapped && pickEntry(normalizeLabel(wrapped.modelId), index);
        estimate = best ? { modelId: best.candidate, provider: best.provider, cost: best.cost } : wrapped;
      }
      if (estimate) {
        const cost = estimate.cost;
        rows.set(label, {
          ...base,
          sourceModel: estimate.modelId,
          sourceProvider: estimate.provider,
          input: cost.input,
          output: cost.output ?? null,
          cacheRead: cost.cache_read ?? cost.input * 0.1,
          cacheWrite: cost.cache_write ?? cost.input * 1.25,
          kind: "family_estimate",
        });
        continue;
      }
      rows.set(label, {
        ...base, sourceModel: null, sourceProvider: null,
        input: null, output: null, cacheRead: null, cacheWrite: null, kind: "unpriced",
      });
      continue;
    }
    const cost = match.cost;
    rows.set(label, {
      ...base,
      sourceModel: match.candidate,
      sourceProvider: match.provider,
      input: cost.input ?? null,
      output: cost.output ?? null,
      // Anthropic-style caching: a cache read costs 0.1x input when the
      // catalog omits an explicit rate.
      cacheRead: cost.cache_read ?? (cost.input != null ? cost.input * 0.1 : null),
      cacheWrite: cost.cache_write ?? (cost.input != null ? cost.input * 1.25 : null),
      kind: "catalog",
    });
  }
  return rows;
}

// A brand-new release (say `claude-opus-6`) is used by people on day one, but
// models.dev often lists it days later. Until then, price it like the newest
// first-party model in the same family (`claude-opus-*`) and tag the row
// `family_estimate`, so it is counted instead of silently reading $0. The
// nightly sync replaces it with the exact catalog price once one exists.
const FAMILY_PROVIDERS = new Set(["anthropic", "openai", "google", "xai", "deepseek", "zai", "moonshotai", "minimax"]);

function familyKey(id) {
  const tokens = String(id).toLowerCase().split(/[-_.]/).filter((t) => t && !/^\d+[a-z]?$/.test(t) && !/^\d{8}$/.test(t));
  // Need a vendor and a tier (`claude` + `opus`), not just a vendor.
  return tokens.length >= 2 ? tokens.join("-") : null;
}

function versionOf(id) {
  return (String(id).match(/\d+/g) ?? []).filter((n) => n.length < 8).map(Number);
}

function compareVersions(a, b) {
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const d = (a[i] ?? 0) - (b[i] ?? 0);
    if (d) return d;
  }
  return 0;
}

function buildFamilyIndex(index) {
  const families = new Map();
  for (const [modelId, entries] of index) {
    if (!/\d/.test(modelId)) continue;
    const first = entries.find((e) => FAMILY_PROVIDERS.has(e.provider));
    if (!first) continue;
    const key = familyKey(modelId);
    if (!key) continue;
    const prev = families.get(key);
    if (!prev || compareVersions(versionOf(modelId), versionOf(prev.modelId)) > 0) {
      families.set(key, { modelId, ...first });
    }
  }
  return families;
}

function familyEstimate(label, families) {
  for (const candidate of normalizeLabel(label)) {
    // Only versioned names: opaque aliases like `coder` or `high` stay unpriced.
    if (!/\d/.test(candidate)) continue;
    const fam = families.get(familyKey(candidate));
    // Only for releases at least as new as the newest priced member. Older
    // models (gpt-3.5 vs a gpt-4 family head) would otherwise borrow a price
    // that can be wrong by 10x.
    if (fam && compareVersions(versionOf(candidate), versionOf(fam.modelId)) >= 0) return fam;
  }
  return null;
}

// Gateway operators wrap a real model id in their own naming:
// `halogen-qwen3.8-flash-next`, `maas-glm-5.3-zhipu-codeagent`,
// `aixdev-gpt-6.1-sol`. When a whole versioned catalog id sits inside the label
// on token boundaries, price the label at that model's rate and tag it
// `family_estimate`. Opaque aliases (`high`, `coder`) contain no such id.
function buildEmbeddedIndex(index) {
  const ids = [];
  for (const [modelId, entries] of index) {
    if (modelId.length < 7 || /[/:@\s]/.test(modelId)) continue;
    if (!/\d/.test(modelId) || !/[a-z]{3}/.test(modelId)) continue;
    if (isZeroCost(entries[0].cost)) continue;
    ids.push({ modelId, ...entries[0] });
  }
  // Longest first, so `gpt-6.1-sol` wins over `gpt-6.1`.
  return ids.sort((a, b) => b.modelId.length - a.modelId.length);
}

function embeddedEstimate(candidates, embedded) {
  for (const candidate of candidates) {
    const label = candidate.toLowerCase();
    for (const entry of embedded) {
      if (entry.modelId.length >= label.length) continue;
      const at = label.indexOf(entry.modelId);
      if (at < 0) continue;
      const before = label[at - 1];
      const after = label.slice(at + entry.modelId.length);
      if (before && /[a-z0-9.]/.test(before)) continue;
      if (after && /^[a-z0-9]/.test(after)) continue;
      // `gpt-5-5` contains `gpt-5` but is a different release, not a wrapper.
      if (/^[-.]\d+(?![a-z0-9])/.test(after)) continue;
      return entry;
    }
  }
  return null;
}

// Preserve known catalog prices and estimates across transient catalog misses.
// A prior 'free' classification is not a known catalog price: allow the resolver
// to correct it to 'unpriced' when a routing/subscription alias was mislabeled.
export const PRICE_KIND_RANK_SQL = (col) =>
  `(CASE ${col} WHEN 'unpriced' THEN 0 WHEN 'family_estimate' THEN 1 ELSE 2 END)`;
// Router placeholders name no model, so an old price row for one is always
// wrong (the catalog lists a $0 "auto" that used to match) and must not be
// preserved. Mirrors ROUTER_LABEL.
export const ROUTER_LABEL_SQL = (col) =>
  `(lower(${col}) IN ('auto', 'default', 'unknown', 'router', 'pool') OR (lower(${col}) GLOB 'pool[0-9]*' AND lower(${col}) NOT GLOB 'pool*[^0-9]*'))`;
export const PRICE_UPSERT_GUARD_SQL = `${PRICE_KIND_RANK_SQL("excluded.price_kind")} >= ${PRICE_KIND_RANK_SQL("model_prices.price_kind")}
   OR (model_prices.price_kind = 'free' AND excluded.price_kind = 'unpriced')
   OR (excluded.price_kind = 'unpriced' AND ${ROUTER_LABEL_SQL("model_prices.model")})`;
export const UPSERT_PRICE_SQL = `INSERT INTO model_prices (model, source_model, source_provider, input_usd_per_mtok, output_usd_per_mtok, cache_read_usd_per_mtok, cache_write_usd_per_mtok, input_includes_cache_read, price_kind, updated_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
ON CONFLICT(model) DO UPDATE SET source_model=excluded.source_model, source_provider=excluded.source_provider, input_usd_per_mtok=excluded.input_usd_per_mtok, output_usd_per_mtok=excluded.output_usd_per_mtok, cache_read_usd_per_mtok=excluded.cache_read_usd_per_mtok, cache_write_usd_per_mtok=excluded.cache_write_usd_per_mtok, input_includes_cache_read=excluded.input_includes_cache_read, price_kind=excluded.price_kind, updated_at=datetime('now')
WHERE ${PRICE_UPSERT_GUARD_SQL}`;

export function upsertParams(r) {
  return [
    r.model, r.sourceModel, r.sourceProvider, r.input, r.output,
    r.cacheRead, r.cacheWrite, r.cacheInclusive, r.kind,
  ];
}

const OBSERVED_LABELS_SQL = `SELECT model_end AS model, provider_end AS provider, COUNT(*) AS sessions,
  SUM(input_tokens + output_tokens + cache_read_input_tokens + cache_creation_input_tokens) AS tokens
FROM events
WHERE event = 'session_end' AND created_at >= datetime('now', ?) AND model_end IS NOT NULL
GROUP BY model_end, provider_end
ORDER BY tokens DESC`;

// Per-response usage (usage_report) also covers compaction and sidecar calls,
// whose models may never appear as a session_end model.
export function usageLabelsSql(canonical = false) {
  return `SELECT model, provider, SUM(responses) AS sessions, SUM(total_tokens) AS tokens
FROM ${canonical ? "daily_model_usage_v2" : "daily_model_usage"}
WHERE usage_date >= date('now', ?) OR updated_at >= datetime('now', ?)
GROUP BY model, provider
ORDER BY tokens DESC`;
}

export function observedLabelsSql() {
  return OBSERVED_LABELS_SQL;
}

// Nightly cron entry: price every label seen in the last `days` days.
export async function syncModelPrices(env, { days = 3, fetchImpl = fetch, batchSize = 50 } = {}) {
  const response = await fetchImpl(CATALOG_URL);
  if (!response.ok) throw new Error(`models.dev returned ${response.status}`);
  const index = buildPriceIndex(await response.json());
  // An empty or malformed catalog would mark everything unpriced. The upsert
  // guard already protects existing prices, but skip the writes entirely.
  if (index.size < 100) throw new Error(`models.dev catalog too small (${index.size} ids)`);
  const observed = (await env.DB.prepare(OBSERVED_LABELS_SQL).bind(`-${days} days`).all()).results ?? [];
  for (const canonical of [false, true]) {
    try {
      observed.push(...((await env.DB.prepare(usageLabelsSql(canonical))
        .bind(`-${days} days`, `-${days} days`).all()).results ?? []));
    } catch (err) {
      // Support rolling migrations without excluding the other table's labels.
      // Operational failures must remain visible instead of reporting success.
      if (!/no such table/i.test(String(err?.message || err))) throw err;
      console.warn(`${canonical ? "daily_model_usage_v2" : "daily_model_usage"} labels unavailable`, err?.message || err);
    }
  }
  const rows = [...resolvePriceRows(observed, index).values()];
  const stmt = env.DB.prepare(UPSERT_PRICE_SQL);
  for (let i = 0; i < rows.length; i += batchSize) {
    await env.DB.batch(rows.slice(i, i + batchSize).map((r) => stmt.bind(...upsertParams(r))));
  }
  return {
    labels: rows.length,
    priced: rows.filter((r) => r.kind !== "unpriced").length,
  };
}
