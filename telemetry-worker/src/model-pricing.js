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

// Model labels that are not real priced models: local/mock/routing sentinels.
// These are recorded as price_kind='free' so the dashboard can separate
// "genuinely $0" from "we failed to find a price".
const FREE_LABEL_PATTERNS = [
  /^mock$/i,
  /^unknown$/i,
  /^default$/i,
  /^auto(\/.*)?$/i,
  /^pool\d*$/i,
  /^free$/i,
  /^openrouter\/free$/i,
  /:free$/i,
  /-free$/i,
  /^ollama\//i,
  /:cloud$/i,
  /^qwen[\d.]*(-coder)?:\d+b$/i,
  /^qwen\d[\d.]*:\d+b$/i,
  /-coding-plan\//i,
  /-token-plan/i,
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
    candidates.add(undated.replace(/-(thinking|highspeed|tiered|high|medium|low)$/i, ""));
  }
  return [...candidates].filter(Boolean);
}

export function isFreeLabel(label) {
  return FREE_LABEL_PATTERNS.some((re) => re.test(label));
}

export function buildPriceIndex(catalog) {
  // model id (lowercased) -> [{provider, cost}], provider-preference sorted.
  const index = new Map();
  const preference = [
    "anthropic",
    "openai",
    "google",
    "xai",
    "deepseek",
    "zai",
    "zhipuai",
    "moonshotai",
    "minimax",
    "openrouter",
  ];
  for (const [providerId, provider] of Object.entries(catalog ?? {})) {
    for (const [modelId, model] of Object.entries(provider?.models ?? {})) {
      const cost = model?.cost;
      if (!cost || typeof cost.input !== "number") continue;
      const key = modelId.toLowerCase();
      if (!index.has(key)) index.set(key, []);
      index.get(key).push({ provider: providerId, cost });
    }
  }
  for (const entries of index.values()) {
    entries.sort((a, b) => {
      const ai = preference.indexOf(a.provider);
      const bi = preference.indexOf(b.provider);
      // Known first-party providers first, then the cheapest-listed as a proxy
      // for "most likely the real rate" rather than an arbitrary map order.
      if (ai !== bi) return (ai < 0 ? 999 : ai) - (bi < 0 ? 999 : bi);
      return a.cost.input - b.cost.input;
    });
  }
  return index;
}

// Resolve observed {model, provider} rows to one price row per label.
export function resolvePriceRows(observed, index) {
  const families = buildFamilyIndex(index);
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

    let match = null;
    for (const candidate of normalizeLabel(label)) {
      const entries = index.get(candidate.toLowerCase());
      if (entries?.length) {
        match = { candidate, ...entries[0] };
        break;
      }
    }
    if (!match) {
      const estimate = familyEstimate(label, families);
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

// Parameterized upsert. Never downgrade: a transient catalog miss must not
// erase a price we had, and a family estimate must not replace an exact price.
export const PRICE_KIND_RANK_SQL = (col) =>
  `(CASE ${col} WHEN 'unpriced' THEN 0 WHEN 'family_estimate' THEN 1 ELSE 2 END)`;
export const UPSERT_PRICE_SQL = `INSERT INTO model_prices (model, source_model, source_provider, input_usd_per_mtok, output_usd_per_mtok, cache_read_usd_per_mtok, cache_write_usd_per_mtok, input_includes_cache_read, price_kind, updated_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
ON CONFLICT(model) DO UPDATE SET source_model=excluded.source_model, source_provider=excluded.source_provider, input_usd_per_mtok=excluded.input_usd_per_mtok, output_usd_per_mtok=excluded.output_usd_per_mtok, cache_read_usd_per_mtok=excluded.cache_read_usd_per_mtok, cache_write_usd_per_mtok=excluded.cache_write_usd_per_mtok, input_includes_cache_read=excluded.input_includes_cache_read, price_kind=excluded.price_kind, updated_at=datetime('now')
WHERE ${PRICE_KIND_RANK_SQL("excluded.price_kind")} >= ${PRICE_KIND_RANK_SQL("model_prices.price_kind")}`;

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
const USAGE_LABELS_SQL = `SELECT model, provider, SUM(responses) AS sessions, SUM(total_tokens) AS tokens
FROM daily_model_usage
WHERE usage_date >= date('now', ?)
GROUP BY model, provider
ORDER BY tokens DESC`;

export function observedLabelsSql() {
  return OBSERVED_LABELS_SQL;
}

// Nightly cron entry: price every label seen in the last `days` days.
export async function syncModelPrices(env, { days = 2, fetchImpl = fetch, batchSize = 50 } = {}) {
  const response = await fetchImpl(CATALOG_URL);
  if (!response.ok) throw new Error(`models.dev returned ${response.status}`);
  const index = buildPriceIndex(await response.json());
  // An empty or malformed catalog would mark everything unpriced. The upsert
  // guard already protects existing prices, but skip the writes entirely.
  if (index.size < 100) throw new Error(`models.dev catalog too small (${index.size} ids)`);
  const observed = (await env.DB.prepare(OBSERVED_LABELS_SQL).bind(`-${days} days`).all()).results ?? [];
  try {
    observed.push(...((await env.DB.prepare(USAGE_LABELS_SQL).bind(`-${days} days`).all()).results ?? []));
  } catch (err) {
    // Table missing (migration 0027 not applied): still price session labels.
    console.warn("daily_model_usage labels unavailable", err?.message || err);
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
