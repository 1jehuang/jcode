let cachedEventColumns = null;
let cachedSessionDetailColumns = null;
let cachedTurnDetailColumns = null;
let cachedWebDetailColumns = null;
let cachedInstallDetailColumns = null;
let cachedDiscoveryDetailColumns = null;
let cachedTodoSessionDetailColumns = null;

// Website beacon events (anonymous visitor_id minted in localStorage). Their
// web-only fields live in the web_details table (see migration 0016): the
// events table sits one column shy of D1's 100-column cap, so wide event
// shapes go in detail tables per the session_details / turn_details pattern.
const WEB_EVENTS = ["web_pageview", "web_cta_click", "web_vital", "web_error"];
const INSTALL_FUNNEL_EVENTS = ["install_funnel"];

// Token subscription plan lifecycle events, plus account_linked, the
// analytics<->account join anchor (telemetry_id + account_id).
const SUBSCRIPTION_EVENTS = [
  "subscription_login",
  "subscription_activated",
  "subscription_budget_exhausted",
  "subscription_router_error",
  "account_linked",
];

const CLI_EVENTS = [
  "install",
  "upgrade",
  "auth_success",
  "onboarding_step",
  "feedback",
  "session_start",
  "turn_end",
  "session_end",
  "session_crash",
  "discovery",
  "todo_session",
];

const KNOWN_EVENTS = [
  ...CLI_EVENTS,
  ...WEB_EVENTS,
  ...INSTALL_FUNNEL_EVENTS,
  ...SUBSCRIPTION_EVENTS,
];

const MAX_TRANSCRIPT_BYTES = 8 * 1024 * 1024;
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

// Origins the website beacon posts from. The default CORS policy stays the
// permissive ALLOWED_ORIGIN var ("*", telemetry is anonymous and unauthed);
// allowlisted origins are echoed back explicitly so the policy keeps working
// if ALLOWED_ORIGIN is ever narrowed.
const WEB_ALLOWED_ORIGINS = new Set([
  "https://jcode.sh",
  "https://www.jcode.sh",
  "https://solosystems.dev",
  "https://www.solosystems.dev",
  "https://solosystems.pages.dev",
]);

// ---------------------------------------------------------------------------
// Self-defense against the D1 size cap.
//
// D1 hard-caps each database at 10 GB on the Workers Paid plan (500 MB on the
// free plan). When the old free-plan cap was hit, every insert failed and
// telemetry silently stopped being recorded (June 2026; ~3 days of events were
// lost). SQLite files never shrink on DELETE - the
// nightly prune frees pages *inside* the file and the day's inserts recycle
// them - so the steady state is "file at high-water mark, internal free-page
// pool cycling". Two triggers defend the pool:
//
// 1. Size growth: every D1 result carries `meta.size_after`. If the file
//    grows past the soft limit (just above the high-water mark), the free
//    pool is exhausted and real growth has resumed; run an emergency prune.
// 2. Full-error: if an insert fails with a full/limit error, prune
//    immediately. This bounds a June-style outage to minutes instead of days.
//
// Emergency prunes use halved retention windows and are rate-limited per
// isolate.
// ---------------------------------------------------------------------------
// Keep the database below the paid plan's first 5 GB of included account-wide
// storage, leaving 500 MB for the account's other D1 databases and growth while
// an emergency prune catches up. This is a budget guardrail, not D1's 10 GB
// per-database hard cap.
const D1_SOFT_LIMIT_BYTES = 4_500_000_000;
const EMERGENCY_PRUNE_COOLDOWN_MS = 10 * 60 * 1000;
// Best-effort per-isolate state (resets on isolate recycle, which is fine:
// the next request re-observes the size from its own insert result).
let lastObservedDbSizeBytes = 0;
let lastEmergencyPruneAtMs = 0;

// ---------------------------------------------------------------------------
// Workers Analytics Engine firehose.
//
// Every event is written to the FIREHOSE dataset before the D1 insert. AE is
// a time-series store with no database size cap (~90-day retention, adaptive
// sampling on reads), so it is the primary store for high-volume raw analysis
// (turn_end / session_start / onboarding_step), while D1 remains the durable
// relational store for identity anchors, lifecycle rows, and the
// daily_active_users rollup. Because the firehose write happens first,
// telemetry keeps recording even if D1 hits its size cap.
//
// AE columns are positional (blob1..blob20, double1..double20, index1). This
// schema defines the mapping; treat it as append-only (never reorder or
// repurpose a position, or historical queries silently read the wrong field).
// ---------------------------------------------------------------------------
const FIREHOSE_SCHEMA = {
  // blob1..blob20 (strings)
  blobs: [
    "event",
    "version",
    "os",
    "arch",
    "build_channel",
    "event_id",
    "session_id",
    "step",
    "auth_provider",
    "auth_method",
    "auth_failure_reason",
    "provider_start",
    "provider_end",
    "model_start",
    "model_end",
    "agent_role",
    "session_stop_reason",
    "end_reason",
    "turn_end_reason",
    "from_version",
  ],
  // double1..double20 (numbers)
  doubles: [
    "is_ci",
    "is_git_checkout",
    "ran_from_cargo",
    "turn_index",
    "duration_secs",
    "input_tokens",
    "output_tokens",
    "total_tokens",
    "tool_calls",
    "executed_tool_calls",
    "tool_failures",
    "file_write_calls",
    "tests_run",
    "tests_passed",
    "error_auth_failed",
    "error_rate_limited",
    "error_provider_timeout",
    "turn_success",
    "turns",
    "milestone_elapsed_ms",
  ],
  // index1 (sampling key): telemetry_id, so adaptive sampling stays accurate
  // per user rather than per event shape.
  indexes: ["telemetry_id"],
};

// ---------------------------------------------------------------------------
// Web/subscription firehose (`jcode_web_firehose` dataset).
//
// FIREHOSE_SCHEMA above is append-only AND full: Analytics Engine caps a data
// point at 20 blobs + 20 doubles, and both arrays are at capacity. The new
// web/subscription fields therefore live in a second dataset with its own
// positional schema instead of repurposing existing positions (which would
// silently corrupt historical queries). Same append-only contract applies
// here: never reorder or repurpose a position.
// ---------------------------------------------------------------------------
const FIREHOSE_WEB_SCHEMA = {
  // blob1..blob20 (strings); full. Append-only: metric_name, rating, and
  // error_kind were added in migration 0018 without moving existing fields.
  blobs: [
    "event",
    "version",
    "os",
    "arch",
    "build_channel",
    "event_id",
    "session_id",
    "path",
    "referrer",
    "visitor_id",
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "cta",
    "account_id",
    "tier",
    "model",
    "metric_name",
    "rating",
    "error_kind",
  ],
  // double1..double20 (numbers); metric_value was appended as double2.
  doubles: ["is_ci", "metric_value"],
  // index1 (sampling key): visitor_id for web events, telemetry_id otherwise.
  indexes: ["visitor_id_or_telemetry_id"],
};

// Sponsored discovery gets a dedicated dataset. The main CLI firehose is at
// Analytics Engine's 20 blob / 20 double limit, and discovery dimensions need
// to remain independently queryable for reliability and funnel analysis.
const FIREHOSE_DISCOVERY_SCHEMA = {
  blobs: [
    "event", "version", "os", "arch", "build_channel", "event_id",
    "session_id", "request_id", "phase", "category", "selected_tool",
    "outcome", "failure_reason",
  ],
  doubles: [
    "is_ci", "is_git_checkout", "ran_from_cargo", "http_status",
    "latency_ms", "response_bytes", "result_count", "query_present",
    "reason_present", "custom_endpoint", "benchmark_run",
  ],
};

// Install conversion events need their opaque join key in the outage-resistant
// firehose. The general and web datasets are both at their positional limits,
// so this dedicated dataset preserves the complete 90-day funnel when D1 is
// unavailable or temporarily full.
const FIREHOSE_INSTALL_SCHEMA = {
  blobs: [
    "event", "event_id", "conversion_id", "stage", "outcome", "source",
    "placement", "install_method", "failure_stage", "visitor_id", "session_id",
    "pageview_id", "utm_source", "utm_medium", "utm_campaign", "path",
    "version", "os", "arch",
  ],
  doubles: [],
};

// Coarse geography (`jcode_geo_firehose` dataset). The main and web datasets
// are both at Analytics Engine's 20-blob limit, so the country dimension gets
// its own dataset instead of repurposing a position. Country only: no IP,
// city, region, coordinates, or timezone is read from request.cf.
const FIREHOSE_GEO_SCHEMA = {
  blobs: ["event", "country", "version", "os", "arch", "build_channel"],
  doubles: ["is_ci"],
  // index1: telemetry_id, so adaptive sampling stays per user.
  indexes: ["telemetry_id"],
};

// Cloudflare sets request.cf.country to "XX" for unknown clients and "T1" for
// Tor exit nodes. Normalize to a 2-letter uppercase code or null.
const NON_COUNTRY_CODES = new Set(["XX", "T1"]);

function normalizeCountry(value) {
  const code = String(value || "").trim().toUpperCase();
  if (!/^[A-Z]{2}$/.test(code) || NON_COUNTRY_CODES.has(code)) {
    return null;
  }
  return code;
}

function writeGeoFirehose(env, body) {
  const sink = env.FIREHOSE_GEO;
  if (!body.country || !sink || typeof sink.writeDataPoint !== "function") {
    return false;
  }
  try {
    sink.writeDataPoint({
      indexes: [String(body.id || "").slice(0, 96)],
      blobs: FIREHOSE_GEO_SCHEMA.blobs.map((name) => {
        const value = body[name];
        return value == null ? "" : String(value).slice(0, 200);
      }),
      doubles: [boolToInt(body.is_ci)],
    });
    return true;
  } catch (err) {
    console.warn("geo firehose write failed", err?.message || err);
    return false;
  }
}

function writeFirehose(env, body) {
  // Geo is dimensioned separately from every event family, so it is written
  // before the per-family dispatch below (which returns early).
  writeGeoFirehose(env, body);
  if (body.event === "discovery") {
    return writeDiscoveryFirehose(env, body);
  }
  const installFirehoseOk = body.conversion_id
    ? writeInstallFirehose(env, body)
    : false;
  if (WEB_EVENTS.includes(body.event) || SUBSCRIPTION_EVENTS.includes(body.event)) {
    return writeWebFirehose(env, body) || installFirehoseOk;
  }
  if (INSTALL_FUNNEL_EVENTS.includes(body.event)) {
    return installFirehoseOk;
  }
  if (!env.FIREHOSE || typeof env.FIREHOSE.writeDataPoint !== "function") {
    return installFirehoseOk;
  }
  const errors = body.errors || {};
  const boolFields = new Set([
    "is_ci",
    "is_git_checkout",
    "ran_from_cargo",
    "turn_success",
  ]);
  const errorFields = {
    error_auth_failed: "auth_failed",
    error_rate_limited: "rate_limited",
    error_provider_timeout: "provider_timeout",
  };
  try {
    env.FIREHOSE.writeDataPoint({
      indexes: [String(body.id || "").slice(0, 96)],
      blobs: FIREHOSE_SCHEMA.blobs.map((name) => {
        const value = body[name];
        // Cap each blob defensively: AE limits total blob bytes per point.
        return value == null ? "" : String(value).slice(0, 200);
      }),
      doubles: FIREHOSE_SCHEMA.doubles.map((name) => {
        if (boolFields.has(name)) {
          return boolToInt(body[name]);
        }
        if (name in errorFields) {
          const value = errors[errorFields[name]] ?? body[name];
          return Number(value) || 0;
        }
        return Number(body[name]) || 0;
      }),
    });
    return true;
  } catch (err) {
    console.warn("firehose write failed", err?.message || err);
    return false;
  }
}

function writeInstallFirehose(env, body) {
  const sink = env.FIREHOSE_INSTALL;
  if (!sink || typeof sink.writeDataPoint !== "function") {
    return false;
  }
  try {
    sink.writeDataPoint({
      indexes: [String(body.conversion_id || "").slice(0, 96)],
      blobs: FIREHOSE_INSTALL_SCHEMA.blobs.map((name) => {
        const value = body[name];
        return value == null ? "" : String(value).slice(0, 200);
      }),
      doubles: [],
    });
    return true;
  } catch (err) {
    console.warn("install firehose write failed", err?.message || err);
    return false;
  }
}

function writeDiscoveryFirehose(env, body) {
  const sink = env.FIREHOSE_DISCOVERY;
  if (!sink || typeof sink.writeDataPoint !== "function") {
    return false;
  }
  const boolFields = new Set([
    "is_ci", "is_git_checkout", "ran_from_cargo", "query_present",
    "reason_present", "custom_endpoint", "benchmark_run",
  ]);
  try {
    sink.writeDataPoint({
      indexes: [String(body.id || "").slice(0, 96)],
      blobs: FIREHOSE_DISCOVERY_SCHEMA.blobs.map((name) => {
        const value = body[name];
        return value == null ? "" : String(value).slice(0, 200);
      }),
      doubles: FIREHOSE_DISCOVERY_SCHEMA.doubles.map((name) => (
        boolFields.has(name) ? boolToInt(body[name]) : Number(body[name]) || 0
      )),
    });
    return true;
  } catch (err) {
    console.warn("discovery firehose write failed", err?.message || err);
    return false;
  }
}

function writeWebFirehose(env, body) {
  const sink = env.FIREHOSE_WEB;
  if (!sink || typeof sink.writeDataPoint !== "function") {
    return false;
  }
  try {
    sink.writeDataPoint({
      indexes: [String(body.visitor_id || body.id || "").slice(0, 96)],
      blobs: FIREHOSE_WEB_SCHEMA.blobs.map((name) => {
        const value = body[name];
        return value == null ? "" : String(value).slice(0, 200);
      }),
      doubles: FIREHOSE_WEB_SCHEMA.doubles.map((name) => {
        if (name === "is_ci") {
          return boolToInt(body.is_ci);
        }
        return Number(body[name]) || 0;
      }),
    });
    return true;
  } catch (err) {
    console.warn("web firehose write failed", err?.message || err);
    return false;
  }
}

export default {
  async fetch(request, env, ctx) {
    const cors = corsHeaders(request, env);
    if (request.method === "OPTIONS") {
      return new Response(null, {
        headers: cors,
      });
    }

    const url = new URL(request.url);

    // Monitoring endpoint: database size vs the soft limit, so cap pressure
    // is observable before inserts start failing.
    if (request.method === "GET" && url.pathname === "/v1/health") {
      try {
        const probe = await env.DB.prepare("SELECT 1").run();
        observeDbSize(probe);
      } catch (err) {
        return jsonResponse(
          { ok: false, error: "d1 probe failed", detail: String(err?.message || err) },
          500,
          cors,
        );
      }
      return jsonResponse({
        ok: true,
        db_size_bytes: lastObservedDbSizeBytes,
        db_soft_limit_bytes: D1_SOFT_LIMIT_BYTES,
        over_soft_limit: lastObservedDbSizeBytes >= D1_SOFT_LIMIT_BYTES,
        last_emergency_prune_at_ms: lastEmergencyPruneAtMs || null,
      }, 200, cors);
    }

    if (request.method !== "POST") {
      return jsonResponse({ error: "Method not allowed" }, 405, cors);
    }

    if (url.pathname === "/v1/transcript") {
      return ingestTranscript(request, env, cors);
    }

    if (url.pathname !== "/v1/event") {
      return jsonResponse({ error: "Not found" }, 404, cors);
    }

    let body;
    try {
      body = await request.json();
    } catch {
      return jsonResponse({ error: "Invalid JSON" }, 400, cors);
    }

    // Web beacon events are normalized before the generic required-field
    // check: the browser has no version/os/arch, so sensible defaults are
    // filled in, and the anonymous visitor_id doubles as the telemetry id.
    if (typeof body.event === "string" && WEB_EVENTS.includes(body.event)) {
      const problem = normalizeWebEvent(body);
      if (problem) {
        return jsonResponse({ error: problem }, 400, cors);
      }
    }

    if (body.event === "install_funnel") {
      const problem = normalizeInstallFunnelEvent(body);
      if (problem) {
        return jsonResponse({ error: problem }, 400, cors);
      }
    }

    if (body.event === "install" && body.install_conversion_id != null) {
      const problem = normalizeInstallConversion(body);
      if (problem) {
        return jsonResponse({ error: problem }, 400, cors);
      }
    }

    if (!body.id || !body.event || !body.version || !body.os || !body.arch) {
      return jsonResponse({ error: "Missing required fields" }, 400, cors);
    }

    if (!KNOWN_EVENTS.includes(body.event)) {
      return jsonResponse({ error: "Unknown event type" }, 400, cors);
    }

    // Coarse geography, resolved at the edge rather than collected by the
    // client. Clients cannot spoof or set this: any inbound `country` field is
    // overwritten. Country only, so this stays consistent with TELEMETRY.md
    // (no IP, city, region, coordinates, or timezone is read or stored).
    body.country = normalizeCountry(request.cf?.country);

    if (SUBSCRIPTION_EVENTS.includes(body.event)) {
      const problem = normalizeSubscriptionEvent(body);
      if (problem) {
        return jsonResponse({ error: problem }, 400, cors);
      }
    }

    if (body.event === "discovery") {
      const problem = normalizeDiscoveryEvent(body);
      if (problem) {
        return jsonResponse({ error: problem }, 400, cors);
      }
    }

    if (body.event === "todo_session") {
      const problem = normalizeTodoSessionEvent(body);
      if (problem) {
        return jsonResponse({ error: problem }, 400, cors);
      }
    }

    // Firehose first: even if D1 is at its size cap, the raw event is
    // recorded in Analytics Engine and the day is reconstructable.
    const firehoseOk = writeFirehose(env, body);

    let durableOk = true;
    try {
      await insertEvent(env, body);
    } catch (err) {
      durableOk = false;
      console.error(
        `d1 insert failed for ${body.event} (db_size=${lastObservedDbSizeBytes})`,
        err?.message || err,
      );
      // A full/limit failure means the internal free-page pool is exhausted
      // (June 2026 failure mode). Prune NOW so telemetry recovers within
      // minutes instead of staying dead until someone notices.
      if (isDbFullError(err) && ctx && typeof ctx.waitUntil === "function") {
        const now = Date.now();
        if (now - lastEmergencyPruneAtMs >= EMERGENCY_PRUNE_COOLDOWN_MS) {
          lastEmergencyPruneAtMs = now;
          ctx.waitUntil(emergencyPrune(env));
        }
      }
    }

    maybeScheduleEmergencyPrune(env, ctx);

    if (!durableOk && !firehoseOk) {
      return jsonResponse({ error: "Internal error" }, 500, cors);
    }
    return jsonResponse({ ok: true, durable: durableOk, firehose: firehoseOk }, 200, cors);
  },

  // Nightly retention pruning bounds durable raw-history growth and keeps the
  // database inside its paid-plan storage budget. Aggregate signal is preserved
  // in the daily_active_users rollup and in long-retention lifecycle events.
  async scheduled(event, env, ctx) {
    ctx.waitUntil(
      (async () => {
        await pruneOldEvents(env);
        // If the normal prune did not free enough headroom, escalate with the
        // emergency (halved) retention windows instead of waiting for inserts
        // to start failing mid-day.
        try {
          const probe = await env.DB.prepare("SELECT 1").run();
          observeDbSize(probe);
        } catch {
          // ignore: size stays at last observation
        }
        if (lastObservedDbSizeBytes >= D1_SOFT_LIMIT_BYTES) {
          await emergencyPrune(env);
        }
      })(),
    );
  },
};

async function ingestTranscript(request, env, cors) {
  if (!env.TRANSCRIPTS || typeof env.TRANSCRIPTS.put !== "function") {
    return jsonResponse({ error: "Transcript storage unavailable" }, 503, cors);
  }
  const declaredSize = Number(request.headers.get("content-length") || 0);
  if (declaredSize > MAX_TRANSCRIPT_BYTES) {
    return jsonResponse({ error: "Transcript payload too large" }, 413, cors);
  }

  let body;
  try {
    body = await request.json();
  } catch {
    return jsonResponse({ error: "Invalid JSON" }, 400, cors);
  }
  const problem = validateTranscript(body);
  if (problem) {
    return jsonResponse({ error: problem }, 400, cors);
  }

  // Defense in depth: clients redact before upload, but the public endpoint
  // must not trust callers to have done so. Preserve ordinary code and prose
  // while removing high-confidence credentials and sensitive object fields.
  redactSecretsInValue(body.messages);

  const encoded = JSON.stringify(body);
  const byteLength = new TextEncoder().encode(encoded).byteLength;
  if (byteLength > MAX_TRANSCRIPT_BYTES) {
    return jsonResponse({ error: "Transcript payload too large" }, 413, cors);
  }

  const month = new Date().toISOString().slice(0, 7);
  const objectKey = `transcripts/${month}/${body.upload_id}.json`;
  try {
    await env.TRANSCRIPTS.put(objectKey, encoded, {
      httpMetadata: { contentType: "application/json" },
      customMetadata: {
        consent_version: String(body.consent_version),
        telemetry_id: body.id,
      },
    });
    await env.DB.prepare(`
      INSERT INTO transcript_uploads (
        upload_id, telemetry_id, object_key, consent_version, schema_version,
        version, provider, model, end_reason, message_count, byte_count
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    `).bind(
      body.upload_id,
      body.id,
      objectKey,
      body.consent_version,
      body.schema_version,
      body.version,
      body.provider || null,
      body.model || null,
      body.end_reason,
      body.message_count,
      byteLength,
    ).run();
  } catch (err) {
    try {
      await env.TRANSCRIPTS.delete(objectKey);
    } catch {
      // Best effort rollback. R2 lifecycle retention remains the safety net.
    }
    console.error("transcript upload failed", err?.message || err);
    return jsonResponse({ error: "Internal error" }, 500, cors);
  }
  return jsonResponse({ ok: true, upload_id: body.upload_id }, 200, cors);
}

function validateTranscript(body) {
  if (!body || body.event !== "transcript") return "Invalid transcript event";
  if (!UUID_RE.test(body.id || "") || !UUID_RE.test(body.upload_id || "")) {
    return "Invalid transcript identifier";
  }
  if (body.consent_version !== 1) return "Unsupported consent version";
  if (!Number.isInteger(body.schema_version) || body.schema_version < 1) {
    return "Invalid schema version";
  }
  if (typeof body.version !== "string" || !body.version) return "Missing version";
  if (!Array.isArray(body.messages) || body.messages.length === 0) {
    return "Transcript messages must be a non-empty array";
  }
  if (!Number.isInteger(body.message_count) || body.message_count !== body.messages.length) {
    return "Transcript message count mismatch";
  }
  if (!["normal_exit", "user_exit", "error", "unknown", "superseded"].includes(body.end_reason)) {
    return "Invalid transcript end reason";
  }
  return null;
}

const SENSITIVE_KEY_RE = /^(?:authorization|cookie|setcookie|privatekey|clientsecret)$/;

function isSensitiveKey(key) {
  const normalized = String(key).toLowerCase().replace(/[^a-z0-9]/g, "");
  return normalized.includes("apikey")
    || normalized.endsWith("token")
    || normalized.endsWith("secret")
    || normalized.includes("password")
    || SENSITIVE_KEY_RE.test(normalized);
}

function redactSecretText(text) {
  return text
    .replace(/sk-ant-(?:oat|ort)01-[A-Za-z0-9_-]{20,}/g, "[REDACTED_SECRET]")
    .replace(/sk-or-v1-[A-Za-z0-9_-]{20,}/g, "[REDACTED_SECRET]")
    .replace(/ghp_[A-Za-z0-9]{20,}/g, "[REDACTED_SECRET]")
    .replace(/github_pat_[A-Za-z0-9_]{20,}/g, "[REDACTED_SECRET]")
    .replace(/ya29\.[A-Za-z0-9._-]{20,}/g, "[REDACTED_SECRET]")
    .replace(/AIza[0-9A-Za-z_-]{20,}/g, "[REDACTED_SECRET]")
    .replace(/xox[baprs]-[A-Za-z0-9-]{10,}/g, "[REDACTED_SECRET]")
    .replace(/AKIA[0-9A-Z]{16}/g, "[REDACTED_SECRET]")
    .replace(/Bearer\s+[A-Za-z0-9._~+/=-]{20,}/gi, "Bearer [REDACTED_SECRET]")
    .replace(/eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}/g, "[REDACTED_SECRET]")
    .replace(/-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----[\s\S]*?-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/g, "[REDACTED_SECRET]")
    .replace(/^\s*([A-Z][A-Z0-9_]*(?:API_KEY|TOKEN|SECRET|PASSWORD|COOKIE)\s*=\s*)[^\r\n]+/gim, "$1[REDACTED_SECRET]")
    .replace(/^\s*(AUTHORIZATION\s*[:=]\s*)[^\r\n]+/gim, "$1[REDACTED_SECRET]");
}

function redactSecretsInValue(value) {
  if (Array.isArray(value)) {
    for (let index = 0; index < value.length; index += 1) {
      if (typeof value[index] === "string") value[index] = redactSecretText(value[index]);
      else redactSecretsInValue(value[index]);
    }
    return;
  }
  if (!value || typeof value !== "object") return;
  for (const [key, entry] of Object.entries(value)) {
    if (isSensitiveKey(key)) {
      value[key] = "[REDACTED_SECRET]";
    } else if (typeof entry === "string") {
      value[key] = redactSecretText(entry);
    } else {
      redactSecretsInValue(entry);
    }
  }
}

function observeDbSize(result) {
  const size = result?.meta?.size_after;
  if (typeof size === "number" && size > 0) {
    lastObservedDbSizeBytes = size;
  }
  return lastObservedDbSizeBytes;
}

function isDbFullError(err) {
  const message = String(err?.message || err || "").toLowerCase();
  // D1 surfaces the cap as SQLITE_FULL ("database or disk is full") or an
  // explicit size-limit message. Keep this narrow: a false positive triggers
  // an unnecessary (rate-limited) prune, but matching e.g. "LIMIT" in SQL
  // syntax errors would prune on every malformed query.
  return (
    message.includes("sqlite_full")
    || message.includes("disk is full")
    || message.includes("database is full")
    || message.includes("exceeds the maximum size")
    || message.includes("maximum database size")
  );
}

function maybeScheduleEmergencyPrune(env, ctx) {
  if (lastObservedDbSizeBytes < D1_SOFT_LIMIT_BYTES) {
    return;
  }
  const now = Date.now();
  if (now - lastEmergencyPruneAtMs < EMERGENCY_PRUNE_COOLDOWN_MS) {
    return;
  }
  lastEmergencyPruneAtMs = now;
  if (ctx && typeof ctx.waitUntil === "function") {
    ctx.waitUntil(emergencyPrune(env));
  }
}

async function emergencyPrune(env) {
  console.error(
    `EMERGENCY PRUNE: db size ${lastObservedDbSizeBytes} bytes >= soft limit ${D1_SOFT_LIMIT_BYTES}; pruning with halved retention windows`,
  );
  await pruneOldEvents(env, { retentionScale: 0.5, maxBatches: 24 });
}

// Retention windows, in days, per event type. Children (turn_details /
// session_details) are deleted before their parent events rows to satisfy the
// FOREIGN KEY (event_id) constraints.
//
// Rationale:
// - turn_end / session_start / onboarding_step are the high-volume rows that
//   filled the database; their aggregate signal is captured in the
//   daily_active_users rollup at insert time.
// - session_end / session_crash power the headline "total users" and crash
//   metrics; keep them for 12 months per the documented retention policy.
// - install and feedback rows are tiny and act as identity/product anchors;
//   they are never pruned here.
// - web_pageview is the high-volume website row; keep a 90-day raw tail in D1
//   (matching firehose retention) and prune beyond it. web_cta_click is the
//   low-volume conversion anchor; keep 12 months. Web vitals need only a
//   30-day performance window; classified web errors remain useful for 90 days.
// - subscription_activated and account_linked are identity/revenue anchors and
//   are never pruned (like install / feedback).
const RETENTION_DAYS = {
  turn_end: 30,
  session_start: 30,
  onboarding_step: 30,
  upgrade: 60,
  auth_success: 180,
  session_end: 365,
  session_crash: 365,
  web_pageview: 90,
  web_cta_click: 365,
  web_vital: 30,
  web_error: 90,
  install_funnel: 90,
  subscription_login: 180,
  subscription_router_error: 90,
  subscription_budget_exhausted: 365,
  todo_session: 365,
};

const PRUNE_BATCH_LIMIT = 10000;
const PRUNE_MAX_BATCHES_PER_RUN = 12;

async function pruneOldEvents(env, options = {}) {
  const retentionScale = options.retentionScale ?? 1;
  const maxBatches = options.maxBatches ?? PRUNE_MAX_BATCHES_PER_RUN;
  const linkageDays = Math.max(1, Math.round(90 * retentionScale));
  const linkageCutoff = `-${linkageDays} days`;
  // The conversion key is deliberately short-lived. Keep the aggregate CTA and
  // install anchors, but sever the browser <-> CLI join after 90 days.
  for (const table of ["web_details", "install_details"]) {
    try {
      await env.DB.prepare(
        `UPDATE ${table} SET conversion_id = NULL WHERE conversion_id IS NOT NULL
         AND event_id IN (
           SELECT event_id FROM events
           WHERE created_at < datetime('now', ?) AND event_id IS NOT NULL
         )`
      ).bind(linkageCutoff).run();
    } catch (err) {
      console.warn(`${table} attribution redaction failed`, err?.message || err);
    }
  }
  let batchesUsed = 0;
  for (const [eventType, days] of Object.entries(RETENTION_DAYS)) {
    const scaledDays = Math.max(1, Math.round(days * retentionScale));
    const cutoff = `-${scaledDays} days`;
    while (batchesUsed < maxBatches) {
      batchesUsed += 1;
      // Delete web_details children first (own try/catch: databases that
      // predate migration 0016 have no web_details table, and that must not
      // abort pruning of the event rows themselves).
      if (WEB_EVENTS.includes(eventType)) {
        try {
          await env.DB.prepare(
            `DELETE FROM web_details WHERE event_id IN (
               SELECT event_id FROM events
               WHERE event = ? AND created_at < datetime('now', ?) AND event_id IS NOT NULL
               LIMIT ?)`
          ).bind(eventType, cutoff, PRUNE_BATCH_LIMIT).run();
        } catch (err) {
          console.warn(`web_details prune failed for ${eventType}`, err?.message || err);
        }
      }
      if (INSTALL_FUNNEL_EVENTS.includes(eventType)) {
        try {
          await env.DB.prepare(
            `DELETE FROM install_details WHERE event_id IN (
               SELECT event_id FROM events
               WHERE event = ? AND created_at < datetime('now', ?) AND event_id IS NOT NULL
               LIMIT ?)`
          ).bind(eventType, cutoff, PRUNE_BATCH_LIMIT).run();
        } catch (err) {
          console.warn(`install_details prune failed for ${eventType}`, err?.message || err);
        }
      }
      if (eventType === "todo_session") {
        try {
          await env.DB.prepare(
            `DELETE FROM todo_session_details WHERE event_id IN (
               SELECT event_id FROM events
               WHERE event = ? AND created_at < datetime('now', ?) AND event_id IS NOT NULL
               LIMIT ?)`
          ).bind(eventType, cutoff, PRUNE_BATCH_LIMIT).run();
        } catch (err) {
          console.warn("todo_session_details prune failed", err?.message || err);
        }
      }
      try {
        // Delete detail children first so the events FK never blocks the prune.
        await env.DB.prepare(
          `DELETE FROM turn_details WHERE event_id IN (
             SELECT event_id FROM events
             WHERE event = ? AND created_at < datetime('now', ?) AND event_id IS NOT NULL
             LIMIT ?)`
        ).bind(eventType, cutoff, PRUNE_BATCH_LIMIT).run();
        await env.DB.prepare(
          `DELETE FROM session_details WHERE event_id IN (
             SELECT event_id FROM events
             WHERE event = ? AND created_at < datetime('now', ?) AND event_id IS NOT NULL
             LIMIT ?)`
        ).bind(eventType, cutoff, PRUNE_BATCH_LIMIT).run();
        const result = await env.DB.prepare(
          `DELETE FROM events WHERE id IN (
             SELECT id FROM events
             WHERE event = ? AND created_at < datetime('now', ?)
             LIMIT ?)`
        ).bind(eventType, cutoff, PRUNE_BATCH_LIMIT).run();
        observeDbSize(result);
        const changes = result?.meta?.changes ?? result?.changes ?? 0;
        if (changes < PRUNE_BATCH_LIMIT) {
          break;
        }
      } catch (err) {
        console.warn(`retention prune failed for ${eventType}`, err?.message || err);
        break;
      }
    }
  }
}

async function insertEvent(env, body) {
  const columns = await getEventColumns(env);
  const sessionDetailColumns = await getSessionDetailColumns(env);
  const turnDetailColumns = await getTurnDetailColumns(env);
  const installDetailColumns = await getInstallDetailColumns(env);
  const common = commonEventEntries(body, columns);

  if (body.event === "todo_session") {
    const values = [
      ["telemetry_id", body.id],
      ["event", body.event],
      ["version", body.version],
      ["os", body.os],
      ["arch", body.arch],
      ...common,
    ].filter(([name]) => columns.has(name));
    const inserted = await insertEventRow(env, body, values);
    if (inserted) {
      await insertTodoSessionDetails(env, body, await getTodoSessionDetailColumns(env));
    }
    return;
  }

  if (body.event === "discovery") {
    const values = [
      ["telemetry_id", body.id],
      ["event", body.event],
      ["version", body.version],
      ["os", body.os],
      ["arch", body.arch],
      ...common,
    ].filter(([name]) => columns.has(name));
    const inserted = await insertEventRow(env, body, values);
    if (inserted) {
      await insertDiscoveryDetails(env, body, await getDiscoveryDetailColumns(env));
    }
    return;
  }

  if (WEB_EVENTS.includes(body.event)) {
    const values = [
      ["telemetry_id", body.id],
      ["event", body.event],
      ["version", body.version],
      ["os", body.os],
      ["arch", body.arch],
      ...common,
    ].filter(([name]) => columns.has(name));
    const inserted = await insertEventRow(env, body, values);
    if (inserted) {
      await insertWebDetails(env, body, await getWebDetailColumns(env));
    }
    return;
  }

  if (SUBSCRIPTION_EVENTS.includes(body.event)) {
    return insertEventRow(env, body, [
      ["telemetry_id", body.id],
      ["event", body.event],
      ["version", body.version],
      ["os", body.os],
      ["arch", body.arch],
      ["account_id", body.account_id || null],
      ["tier", body.tier || null],
      // Subscription events reuse the generic model_start column for the
      // routed model (new event types; no historical rows are re-read).
      ["model_start", body.model || null],
      ...common,
    ].filter(([name]) => columns.has(name)));
  }

  if (body.event === "install_funnel") {
    const values = [
      ["telemetry_id", body.id],
      ["event", body.event],
      ["version", body.version],
      ["os", body.os],
      ["arch", body.arch],
      ...common,
    ].filter(([name]) => columns.has(name));
    const inserted = await insertEventRow(env, body, values);
    if (inserted) {
      await insertInstallDetails(env, body, installDetailColumns);
    }
    return;
  }

  if (body.event === "install") {
    const inserted = await insertEventRow(env, body, [
      ["telemetry_id", body.id],
      ["event", body.event],
      ["version", body.version],
      ["os", body.os],
      ["arch", body.arch],
      ...common,
    ].filter(([name]) => columns.has(name)));
    if (inserted && body.conversion_id) {
      await insertInstallDetails(env, {
        ...body,
        stage: "first_run",
        outcome: "success",
        source: "cli",
      }, installDetailColumns);
    }
    return;
  }

  if (body.event === "upgrade") {
    return insertEventRow(env, body, [
      ["telemetry_id", body.id],
      ["event", body.event],
      ["version", body.version],
      ["os", body.os],
      ["arch", body.arch],
      ["from_version", body.from_version || null],
      ...common,
    ].filter(([name]) => columns.has(name)));
  }

  if (body.event === "auth_success") {
    return insertEventRow(env, body, [
      ["telemetry_id", body.id],
      ["event", body.event],
      ["version", body.version],
      ["os", body.os],
      ["arch", body.arch],
      ["auth_provider", body.auth_provider || null],
      ["auth_method", body.auth_method || null],
      ...common,
    ].filter(([name]) => columns.has(name)));
  }

  if (body.event === "onboarding_step") {
    return insertEventRow(env, body, [
      ["telemetry_id", body.id],
      ["event", body.event],
      ["version", body.version],
      ["os", body.os],
      ["arch", body.arch],
      ["step", body.step || null],
      ["auth_provider", body.auth_provider || null],
      ["auth_method", body.auth_method || null],
      ["auth_failure_reason", body.auth_failure_reason || null],
      ["milestone_elapsed_ms", body.milestone_elapsed_ms || null],
      ...common,
    ].filter(([name]) => columns.has(name)));
  }

  if (body.event === "feedback") {
    return insertEventRow(env, body, [
      ["telemetry_id", body.id],
      ["event", body.event],
      ["version", body.version],
      ["os", body.os],
      ["arch", body.arch],
      ["feedback_rating", body.feedback_rating || null],
      ["feedback_reason", body.feedback_reason || null],
      ["feedback_text", body.feedback_text || null],
      ...common,
    ].filter(([name]) => columns.has(name)));
  }

  if (body.event === "session_start") {
    const values = [
      ["telemetry_id", body.id],
      ["event", body.event],
      ["version", body.version],
      ["os", body.os],
      ["arch", body.arch],
      ["provider_start", body.provider_start || null],
      ["model_start", body.model_start || null],
      ["session_start_hour_utc", body.session_start_hour_utc ?? null],
      ["session_start_weekday_utc", body.session_start_weekday_utc ?? null],
      ["previous_session_gap_secs", body.previous_session_gap_secs ?? null],
      ["sessions_started_24h", body.sessions_started_24h || 0],
      ["sessions_started_7d", body.sessions_started_7d || 0],
      ["active_sessions_at_start", body.active_sessions_at_start || 0],
      ["other_active_sessions_at_start", body.other_active_sessions_at_start || 0],
      ...common,
    ];
    if (columns.has("resumed_session")) {
      values.push(["resumed_session", boolToInt(body.resumed_session)]);
    }
    return insertEventRow(env, body, values.filter(([name]) => columns.has(name)));
  }

  if (body.event === "turn_end") {
    const values = [
      ["telemetry_id", body.id],
      ["event", body.event],
      ["version", body.version],
      ["os", body.os],
      ["arch", body.arch],
      ["turn_index", body.turn_index ?? null],
      ["turn_started_ms", body.turn_started_ms ?? null],
      ["turn_active_duration_ms", body.turn_active_duration_ms ?? null],
      ["idle_before_turn_ms", body.idle_before_turn_ms ?? null],
      ["idle_after_turn_ms", body.idle_after_turn_ms ?? null],
      ["input_tokens", body.input_tokens || 0],
      ["output_tokens", body.output_tokens || 0],
      ["cache_read_input_tokens", body.cache_read_input_tokens || 0],
      ["cache_creation_input_tokens", body.cache_creation_input_tokens || 0],
      ["total_tokens", body.total_tokens || 0],
      ["turn_success", boolToInt(body.turn_success)],
      ["turn_abandoned", boolToInt(body.turn_abandoned)],
      ["turn_end_reason", body.turn_end_reason || null],
      ...common,
    ].filter(([name]) => columns.has(name));
    const inserted = await insertEventRow(env, body, values);
    if (inserted) {
      await insertTurnDetails(env, body, turnDetailColumns);
    }
    return;
  }

  if (["session_end", "session_crash"].includes(body.event)) {
    const errors = body.errors || {};
    const values = [
      ["telemetry_id", body.id],
      ["event", body.event],
      ["version", body.version],
      ["os", body.os],
      ["arch", body.arch],
      ["provider_start", body.provider_start || null],
      ["provider_end", body.provider_end || null],
      ["model_start", body.model_start || null],
      ["model_end", body.model_end || null],
      ["provider_switches", body.provider_switches || 0],
      ["model_switches", body.model_switches || 0],
      ["duration_mins", body.duration_mins || 0],
      ["duration_secs", body.duration_secs || 0],
      ["turns", body.turns || 0],
      ["had_user_prompt", boolToInt(body.had_user_prompt)],
      ["had_assistant_response", boolToInt(body.had_assistant_response)],
      ["assistant_responses", body.assistant_responses || 0],
      ["first_assistant_response_ms", body.first_assistant_response_ms || null],
      ["first_tool_call_ms", body.first_tool_call_ms || null],
      ["first_tool_success_ms", body.first_tool_success_ms || null],
      ["tool_calls", body.tool_calls || 0],
      ["tool_failures", body.tool_failures || 0],
      ["executed_tool_calls", body.executed_tool_calls || 0],
      ["executed_tool_successes", body.executed_tool_successes || 0],
      ["executed_tool_failures", body.executed_tool_failures || 0],
      ["tool_latency_total_ms", body.tool_latency_total_ms || 0],
      ["tool_latency_max_ms", body.tool_latency_max_ms || 0],
      ["file_write_calls", body.file_write_calls || 0],
      ["tests_run", body.tests_run || 0],
      ["tests_passed", body.tests_passed || 0],
      ["input_tokens", body.input_tokens || 0],
      ["output_tokens", body.output_tokens || 0],
      ["cache_read_input_tokens", body.cache_read_input_tokens || 0],
      ["cache_creation_input_tokens", body.cache_creation_input_tokens || 0],
      ["total_tokens", body.total_tokens || 0],
      ["feature_memory_used", boolToInt(body.feature_memory_used)],
      ["feature_swarm_used", boolToInt(body.feature_swarm_used)],
      ["feature_web_used", boolToInt(body.feature_web_used)],
      ["feature_email_used", boolToInt(body.feature_email_used)],
      ["feature_mcp_used", boolToInt(body.feature_mcp_used)],
      ["feature_side_panel_used", boolToInt(body.feature_side_panel_used)],
      ["feature_goal_used", boolToInt(body.feature_goal_used)],
      ["feature_selfdev_used", boolToInt(body.feature_selfdev_used)],
      ["feature_background_used", boolToInt(body.feature_background_used)],
      ["feature_subagent_used", boolToInt(body.feature_subagent_used)],
      ["unique_mcp_servers", body.unique_mcp_servers || 0],
      ["session_success", boolToInt(body.session_success)],
      ["abandoned_before_response", boolToInt(body.abandoned_before_response)],
      ["session_stop_reason", body.session_stop_reason || null],
      ["agent_role", body.agent_role || null],
      ["parent_session_id", body.parent_session_id || null],
      ["agent_active_ms_total", body.agent_active_ms_total || 0],
      ["agent_model_ms_total", body.agent_model_ms_total || 0],
      ["agent_tool_ms_total", body.agent_tool_ms_total || 0],
      ["session_idle_ms_total", body.session_idle_ms_total || 0],
      ["agent_blocked_ms_total", body.agent_blocked_ms_total || 0],
      ["time_to_first_agent_action_ms", body.time_to_first_agent_action_ms ?? null],
      ["time_to_first_useful_action_ms", body.time_to_first_useful_action_ms ?? null],
      ["spawned_agent_count", body.spawned_agent_count || 0],
      ["background_task_count", body.background_task_count || 0],
      ["background_task_completed_count", body.background_task_completed_count || 0],
      ["subagent_task_count", body.subagent_task_count || 0],
      ["subagent_success_count", body.subagent_success_count || 0],
      ["swarm_task_count", body.swarm_task_count || 0],
      ["swarm_success_count", body.swarm_success_count || 0],
      ["user_cancelled_count", body.user_cancelled_count || 0],
      ["transport_https", body.transport_https || 0],
      ["transport_persistent_ws_fresh", body.transport_persistent_ws_fresh || 0],
      ["transport_persistent_ws_reuse", body.transport_persistent_ws_reuse || 0],
      ["transport_cli_subprocess", body.transport_cli_subprocess || 0],
      ["transport_native_http2", body.transport_native_http2 || 0],
      ["transport_other", body.transport_other || 0],
      ["session_start_hour_utc", body.session_start_hour_utc ?? null],
      ["session_start_weekday_utc", body.session_start_weekday_utc ?? null],
      ["session_end_hour_utc", body.session_end_hour_utc ?? null],
      ["session_end_weekday_utc", body.session_end_weekday_utc ?? null],
      ["previous_session_gap_secs", body.previous_session_gap_secs ?? null],
      ["sessions_started_24h", body.sessions_started_24h || 0],
      ["sessions_started_7d", body.sessions_started_7d || 0],
      ["active_sessions_at_start", body.active_sessions_at_start || 0],
      ["other_active_sessions_at_start", body.other_active_sessions_at_start || 0],
      ["max_concurrent_sessions", body.max_concurrent_sessions || 0],
      ["multi_sessioned", boolToInt(body.multi_sessioned)],
      ["resumed_session", boolToInt(body.resumed_session)],
      ["end_reason", body.end_reason || null],
      ["error_provider_timeout", errors.provider_timeout || 0],
      ["error_auth_failed", errors.auth_failed || 0],
      ["error_tool_error", errors.tool_error || 0],
      ["error_mcp_error", errors.mcp_error || 0],
      ["error_rate_limited", errors.rate_limited || 0],
      ...common,
    ].filter(([name]) => columns.has(name));
    const inserted = await insertEventRow(env, body, values);
    if (inserted) {
      await insertSessionDetails(env, body, sessionDetailColumns);
    }
    return;
  }
}

async function insertEventRow(env, body, entries) {
  const result = await insertDynamic(env, "events", entries);
  const inserted = wasInserted(result);
  if (inserted) {
    await recordDailyActivity(env, body);
  }
  return inserted;
}

function wasInserted(result) {
  return (result?.meta?.changes ?? result?.changes ?? 0) > 0;
}

async function insertTurnDetails(env, body, columns) {
  if (!columns || columns.size === 0 || !body.event_id || !columns.has("event_id")) {
    return;
  }
  const values = [
    ["event_id", body.event_id],
    ["turn_index", body.turn_index ?? null],
    ["turn_started_ms", body.turn_started_ms ?? null],
    ["turn_active_duration_ms", body.turn_active_duration_ms ?? null],
    ["idle_before_turn_ms", body.idle_before_turn_ms ?? null],
    ["idle_after_turn_ms", body.idle_after_turn_ms ?? null],
    ["turn_success", boolToInt(body.turn_success)],
    ["turn_abandoned", boolToInt(body.turn_abandoned)],
    ["turn_end_reason", body.turn_end_reason || null],
    ["input_tokens", body.input_tokens || 0],
    ["output_tokens", body.output_tokens || 0],
    ["total_tokens", body.total_tokens || 0],
    ["assistant_responses", body.assistant_responses || 0],
    ["first_assistant_response_ms", body.first_assistant_response_ms ?? null],
    ["first_tool_call_ms", body.first_tool_call_ms ?? null],
    ["first_tool_success_ms", body.first_tool_success_ms ?? null],
    ["first_file_edit_ms", body.first_file_edit_ms ?? null],
    ["first_test_pass_ms", body.first_test_pass_ms ?? null],
    ["tool_calls", body.tool_calls || 0],
    ["tool_failures", body.tool_failures || 0],
    ["executed_tool_calls", body.executed_tool_calls || 0],
    ["executed_tool_successes", body.executed_tool_successes || 0],
    ["executed_tool_failures", body.executed_tool_failures || 0],
    ["tool_latency_total_ms", body.tool_latency_total_ms || 0],
    ["tool_latency_max_ms", body.tool_latency_max_ms || 0],
    ["file_write_calls", body.file_write_calls || 0],
    ["tests_run", body.tests_run || 0],
    ["tests_passed", body.tests_passed || 0],
    ["feature_memory_used", boolToInt(body.feature_memory_used)],
    ["feature_swarm_used", boolToInt(body.feature_swarm_used)],
    ["feature_web_used", boolToInt(body.feature_web_used)],
    ["feature_email_used", boolToInt(body.feature_email_used)],
    ["feature_mcp_used", boolToInt(body.feature_mcp_used)],
    ["feature_side_panel_used", boolToInt(body.feature_side_panel_used)],
    ["feature_goal_used", boolToInt(body.feature_goal_used)],
    ["feature_selfdev_used", boolToInt(body.feature_selfdev_used)],
    ["feature_background_used", boolToInt(body.feature_background_used)],
    ["feature_subagent_used", boolToInt(body.feature_subagent_used)],
    ["unique_mcp_servers", body.unique_mcp_servers || 0],
    ["tool_cat_read_search", body.tool_cat_read_search || 0],
    ["tool_cat_write", body.tool_cat_write || 0],
    ["tool_cat_shell", body.tool_cat_shell || 0],
    ["tool_cat_web", body.tool_cat_web || 0],
    ["tool_cat_memory", body.tool_cat_memory || 0],
    ["tool_cat_subagent", body.tool_cat_subagent || 0],
    ["tool_cat_swarm", body.tool_cat_swarm || 0],
    ["tool_cat_email", body.tool_cat_email || 0],
    ["tool_cat_side_panel", body.tool_cat_side_panel || 0],
    ["tool_cat_goal", body.tool_cat_goal || 0],
    ["tool_cat_mcp", body.tool_cat_mcp || 0],
    ["tool_cat_other", body.tool_cat_other || 0],
    ["tool_cat_todo", body.tool_cat_todo || 0],
    ["feature_todo_used", boolToInt(body.feature_todo_used)],
    ["todo_gate_ownership_count", body.todo_gate_ownership_count || 0],
    ["todo_gate_hill_count", body.todo_gate_hill_count || 0],
    ["todo_gate_completion_count", body.todo_gate_completion_count || 0],
    ["todo_gate_spike_count", body.todo_gate_spike_count || 0],
    ["workflow_chat_only", boolToInt(body.workflow_chat_only)],
    ["workflow_coding_used", boolToInt(body.workflow_coding_used)],
    ["workflow_research_used", boolToInt(body.workflow_research_used)],
    ["workflow_tests_used", boolToInt(body.workflow_tests_used)],
    ["workflow_background_used", boolToInt(body.workflow_background_used)],
    ["workflow_subagent_used", boolToInt(body.workflow_subagent_used)],
    ["workflow_swarm_used", boolToInt(body.workflow_swarm_used)],
  ].filter(([name]) => columns.has(name));
  if (values.length > 1) {
    await insertDynamic(env, 'turn_details', values);
  }
}

async function recordDailyActivity(env, body) {
  // Country rollup covers every event family, including the ones that never
  // reach the DAU table (install, upgrade, web_pageview, ...).
  await recordCountryDaily(env, body);
  if (!["session_start", "turn_end", "session_end", "session_crash"].includes(body.event)) {
    return;
  }

  const activityDate = new Date().toISOString().slice(0, 10);
  const meaningful = isMeaningfulLifecycleEvent(body) ? 1 : 0;
  const release = body.build_channel === "release" ? 1 : 0;
  const meaningfulRelease = meaningful && release ? 1 : 0;
  const isCi = boolToInt(body.is_ci);
  const sessionStartCount = body.event === "session_start" ? 1 : 0;
  const turnEndCount = body.event === "turn_end" ? 1 : 0;
  const sessionEndCount = body.event === "session_end" ? 1 : 0;
  const sessionCrashCount = body.event === "session_crash" ? 1 : 0;

  try {
    await env.DB.prepare(`
      INSERT INTO daily_active_users (
        activity_date,
        telemetry_id,
        raw_active,
        meaningful_active,
        release_active,
        meaningful_release_active,
        session_start_count,
        turn_end_count,
        session_end_count,
        session_crash_count,
        ci_active,
        last_is_ci,
        last_build_channel,
        last_country
      ) VALUES (?, ?, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      ON CONFLICT(activity_date, telemetry_id) DO UPDATE SET
        last_seen_at = datetime('now'),
        raw_active = 1,
        meaningful_active = MAX(meaningful_active, excluded.meaningful_active),
        release_active = MAX(release_active, excluded.release_active),
        meaningful_release_active = MAX(meaningful_release_active, excluded.meaningful_release_active),
        session_start_count = session_start_count + excluded.session_start_count,
        turn_end_count = turn_end_count + excluded.turn_end_count,
        session_end_count = session_end_count + excluded.session_end_count,
        session_crash_count = session_crash_count + excluded.session_crash_count,
        ci_active = MAX(ci_active, excluded.ci_active),
        last_is_ci = excluded.last_is_ci,
        last_build_channel = COALESCE(excluded.last_build_channel, daily_active_users.last_build_channel),
        last_country = COALESCE(excluded.last_country, daily_active_users.last_country)
    `).bind(
      activityDate,
      body.id,
      meaningful,
      release,
      meaningfulRelease,
      sessionStartCount,
      turnEndCount,
      sessionEndCount,
      sessionCrashCount,
      isCi,
      isCi,
      body.build_channel || null,
      body.country || null,
    ).run();
  } catch (err) {
    // Older databases may not have the rollup migration yet. Do not reject the
    // canonical event insert, because raw events remain the source of truth.
    console.warn("daily activity rollup failed", err?.message || err);
  }
}

// Durable per-day country x event counts. Aggregate only (no telemetry_id), so
// it survives retention pruning and cannot be used to profile an individual.
async function recordCountryDaily(env, body) {
  if (!body.country) {
    return;
  }
  const activityDate = new Date().toISOString().slice(0, 10);
  try {
    await env.DB.prepare(`
      INSERT INTO country_daily (activity_date, country, event, is_ci, event_count)
      VALUES (?, ?, ?, ?, 1)
      ON CONFLICT(activity_date, country, event, is_ci) DO UPDATE SET
        event_count = event_count + 1,
        last_seen_at = datetime('now')
    `).bind(activityDate, body.country, body.event, boolToInt(body.is_ci)).run();
  } catch (err) {
    // Databases predating migration 0022 have no country_daily table; never
    // fail the canonical event insert over the geo rollup.
    console.warn("country rollup failed", err?.message || err);
  }
}

function isMeaningfulLifecycleEvent(body) {
  const errors = body.errors || {};
  if (["session_end", "session_crash"].includes(body.event)) {
    return (
      (body.turns || 0) > 0
      || boolToInt(body.had_user_prompt) > 0
      || boolToInt(body.had_assistant_response) > 0
      || (body.assistant_responses || 0) > 0
      || (body.tool_calls || 0) > 0
      || (body.executed_tool_calls || 0) > 0
      || (body.duration_secs || 0) > 0
      || (errors.provider_timeout || 0) > 0
      || (errors.auth_failed || 0) > 0
      || (errors.tool_error || 0) > 0
      || (errors.mcp_error || 0) > 0
      || (errors.rate_limited || 0) > 0
      || (body.provider_switches || 0) > 0
      || (body.model_switches || 0) > 0
    );
  }
  // A turn_end event only fires after a real user turn completes (a prompt was
  // submitted and the agent did work), so it is strong evidence of meaningful
  // activity even when the session_end/session_crash event is lost (process
  // killed, machine shutdown, network drop on the final flush, or a session
  // still open at UTC midnight). Counting it here avoids undercounting the
  // headline meaningful DAU for those users.
  if (body.event === "turn_end") {
    return (
      (body.assistant_responses || 0) > 0
      || (body.tool_calls || 0) > 0
      || (body.executed_tool_calls || 0) > 0
      || (body.file_write_calls || 0) > 0
      || (body.tests_run || 0) > 0
      || boolToInt(body.turn_success) > 0
    );
  }
  return false;
}

async function insertSessionDetails(env, body, columns) {
  if (!columns || columns.size === 0 || !body.event_id || !columns.has("event_id")) {
    return;
  }
  const values = [
    ["event_id", body.event_id],
    ["session_start_hour_utc", body.session_start_hour_utc ?? null],
    ["session_start_weekday_utc", body.session_start_weekday_utc ?? null],
    ["session_end_hour_utc", body.session_end_hour_utc ?? null],
    ["session_end_weekday_utc", body.session_end_weekday_utc ?? null],
    ["previous_session_gap_secs", body.previous_session_gap_secs ?? null],
    ["sessions_started_24h", body.sessions_started_24h || 0],
    ["sessions_started_7d", body.sessions_started_7d || 0],
    ["active_sessions_at_start", body.active_sessions_at_start || 0],
    ["other_active_sessions_at_start", body.other_active_sessions_at_start || 0],
    ["max_concurrent_sessions", body.max_concurrent_sessions || 0],
    ["multi_sessioned", boolToInt(body.multi_sessioned)],
    ["first_file_edit_ms", body.first_file_edit_ms || null],
    ["first_test_pass_ms", body.first_test_pass_ms || null],
    ["tool_cat_read_search", body.tool_cat_read_search || 0],
    ["tool_cat_write", body.tool_cat_write || 0],
    ["tool_cat_shell", body.tool_cat_shell || 0],
    ["tool_cat_web", body.tool_cat_web || 0],
    ["tool_cat_memory", body.tool_cat_memory || 0],
    ["tool_cat_subagent", body.tool_cat_subagent || 0],
    ["tool_cat_swarm", body.tool_cat_swarm || 0],
    ["tool_cat_email", body.tool_cat_email || 0],
    ["tool_cat_side_panel", body.tool_cat_side_panel || 0],
    ["tool_cat_goal", body.tool_cat_goal || 0],
    ["tool_cat_mcp", body.tool_cat_mcp || 0],
    ["tool_cat_other", body.tool_cat_other || 0],
    ["tool_cat_todo", body.tool_cat_todo || 0],
    ["feature_todo_used", boolToInt(body.feature_todo_used)],
    ["todo_gate_ownership_count", body.todo_gate_ownership_count || 0],
    ["todo_gate_hill_count", body.todo_gate_hill_count || 0],
    ["todo_gate_completion_count", body.todo_gate_completion_count || 0],
    ["todo_gate_spike_count", body.todo_gate_spike_count || 0],
    ["command_login_used", boolToInt(body.command_login_used)],
    ["command_model_used", boolToInt(body.command_model_used)],
    ["command_usage_used", boolToInt(body.command_usage_used)],
    ["command_resume_used", boolToInt(body.command_resume_used)],
    ["command_memory_used", boolToInt(body.command_memory_used)],
    ["command_swarm_used", boolToInt(body.command_swarm_used)],
    ["command_goal_used", boolToInt(body.command_goal_used)],
    ["command_selfdev_used", boolToInt(body.command_selfdev_used)],
    ["command_feedback_used", boolToInt(body.command_feedback_used)],
    ["command_other_used", boolToInt(body.command_other_used)],
    ["workflow_chat_only", boolToInt(body.workflow_chat_only)],
    ["workflow_coding_used", boolToInt(body.workflow_coding_used)],
    ["workflow_research_used", boolToInt(body.workflow_research_used)],
    ["workflow_tests_used", boolToInt(body.workflow_tests_used)],
    ["workflow_background_used", boolToInt(body.workflow_background_used)],
    ["workflow_subagent_used", boolToInt(body.workflow_subagent_used)],
    ["workflow_swarm_used", boolToInt(body.workflow_swarm_used)],
    ["project_repo_present", boolToInt(body.project_repo_present)],
    ["project_lang_rust", boolToInt(body.project_lang_rust)],
    ["project_lang_js_ts", boolToInt(body.project_lang_js_ts)],
    ["project_lang_python", boolToInt(body.project_lang_python)],
    ["project_lang_go", boolToInt(body.project_lang_go)],
    ["project_lang_markdown", boolToInt(body.project_lang_markdown)],
    ["project_lang_mixed", boolToInt(body.project_lang_mixed)],
    ["days_since_install", body.days_since_install || null],
    ["active_days_7d", body.active_days_7d || 0],
    ["active_days_30d", body.active_days_30d || 0],
  ].filter(([name]) => columns.has(name));
  if (values.length > 1) {
    await insertDynamic(env, 'session_details', values);
  }
}

function commonEventEntries(body, columns) {
  const values = [];
  if (columns.has("event_id")) {
    values.push(["event_id", body.event_id || null]);
  }
  if (columns.has("session_id")) {
    values.push(["session_id", body.session_id || null]);
  }
  if (columns.has("schema_version")) {
    values.push(["schema_version", body.schema_version || 1]);
  }
  if (columns.has("build_channel")) {
    values.push(["build_channel", body.build_channel || null]);
  }
  if (columns.has("is_git_checkout")) {
    values.push(["is_git_checkout", boolToInt(body.is_git_checkout)]);
  }
  if (columns.has("is_ci")) {
    values.push(["is_ci", boolToInt(body.is_ci)]);
  }
  if (columns.has("ran_from_cargo")) {
    values.push(["ran_from_cargo", boolToInt(body.ran_from_cargo)]);
  }
  return values;
}

async function getEventColumns(env) {
  if (cachedEventColumns) {
    return cachedEventColumns;
  }
  const result = await env.DB.prepare("PRAGMA table_info(events)").all();
  cachedEventColumns = new Set((result.results || []).map((row) => row.name));
  return cachedEventColumns;
}

async function getSessionDetailColumns(env) {
  if (cachedSessionDetailColumns) {
    return cachedSessionDetailColumns;
  }
  try {
    const result = await env.DB.prepare("PRAGMA table_info(session_details)").all();
    cachedSessionDetailColumns = new Set((result.results || []).map((row) => row.name));
  } catch {
    cachedSessionDetailColumns = new Set();
  }
  return cachedSessionDetailColumns;
}

async function getTurnDetailColumns(env) {
  if (cachedTurnDetailColumns) {
    return cachedTurnDetailColumns;
  }
  try {
    const result = await env.DB.prepare("PRAGMA table_info(turn_details)").all();
    cachedTurnDetailColumns = new Set((result.results || []).map((row) => row.name));
  } catch {
    cachedTurnDetailColumns = new Set();
  }
  return cachedTurnDetailColumns;
}

async function getWebDetailColumns(env) {
  if (cachedWebDetailColumns) {
    return cachedWebDetailColumns;
  }
  try {
    const result = await env.DB.prepare("PRAGMA table_info(web_details)").all();
    cachedWebDetailColumns = new Set((result.results || []).map((row) => row.name));
  } catch {
    cachedWebDetailColumns = new Set();
  }
  return cachedWebDetailColumns;
}

async function getInstallDetailColumns(env) {
  if (cachedInstallDetailColumns) {
    return cachedInstallDetailColumns;
  }
  try {
    const result = await env.DB.prepare("PRAGMA table_info(install_details)").all();
    cachedInstallDetailColumns = new Set((result.results || []).map((row) => row.name));
  } catch {
    cachedInstallDetailColumns = new Set();
  }
  return cachedInstallDetailColumns;
}

async function getDiscoveryDetailColumns(env) {
  if (cachedDiscoveryDetailColumns) {
    return cachedDiscoveryDetailColumns;
  }
  try {
    const result = await env.DB.prepare("PRAGMA table_info(discovery_details)").all();
    cachedDiscoveryDetailColumns = new Set((result.results || []).map((row) => row.name));
  } catch {
    cachedDiscoveryDetailColumns = new Set();
  }
  return cachedDiscoveryDetailColumns;
}

async function getTodoSessionDetailColumns(env) {
  if (cachedTodoSessionDetailColumns) {
    return cachedTodoSessionDetailColumns;
  }
  try {
    const result = await env.DB.prepare("PRAGMA table_info(todo_session_details)").all();
    cachedTodoSessionDetailColumns = new Set((result.results || []).map((row) => row.name));
  } catch {
    cachedTodoSessionDetailColumns = new Set();
  }
  return cachedTodoSessionDetailColumns;
}

async function insertDiscoveryDetails(env, body, columns) {
  if (!columns || columns.size === 0 || !body.event_id || !columns.has("event_id")) {
    return;
  }
  const values = [
    ["event_id", body.event_id],
    ["request_id", body.request_id],
    ["phase", body.phase],
    ["category", body.category || null],
    ["selected_tool", body.selected_tool || null],
    ["outcome", body.outcome],
    ["failure_reason", body.failure_reason || null],
    ["http_status", body.http_status ?? null],
    ["latency_ms", body.latency_ms || 0],
    ["response_bytes", body.response_bytes ?? null],
    ["result_count", body.result_count ?? null],
    ["query_present", boolToInt(body.query_present)],
    ["reason_present", boolToInt(body.reason_present)],
    ["custom_endpoint", boolToInt(body.custom_endpoint)],
    ["benchmark_run", boolToInt(body.benchmark_run)],
  ].filter(([name]) => columns.has(name));
  if (values.length > 1) {
    await insertDynamic(env, "discovery_details", values);
  }
}

async function insertTodoSessionDetails(env, body, columns) {
  if (!columns || columns.size === 0 || !body.event_id || !columns.has("event_id")) {
    return;
  }
  const values = [
    ["event_id", body.event_id],
    ["correlation_id", body.correlation_id],
    ["session_end_reason", body.session_end_reason],
    ["todos_created", body.todos_created || 0],
    ["todos_completed", body.todos_completed || 0],
    ["todos_abandoned", body.todos_abandoned || 0],
    ["todo_updates", body.todo_updates || 0],
    ["groups_completed", body.groups_completed || 0],
    ["groups_total", body.groups_total || 0],
    ["max_todo_list_size", body.max_todo_list_size || 0],
    ["confidence_min", body.confidence_min ?? null],
    ["confidence_mean", body.confidence_mean ?? null],
    ["confidence_count", body.confidence_count || 0],
    ["completion_confidence_min", body.completion_confidence_min ?? null],
    ["completion_confidence_mean", body.completion_confidence_mean ?? null],
    ["completion_confidence_count", body.completion_confidence_count || 0],
    ["understands_user_intent_min", body.understands_user_intent_min ?? null],
    ["understands_user_intent_mean", body.understands_user_intent_mean ?? null],
    ["understands_user_intent_count", body.understands_user_intent_count || 0],
    ["closed_feedback_loop_min", body.closed_feedback_loop_min ?? null],
    ["closed_feedback_loop_mean", body.closed_feedback_loop_mean ?? null],
    ["closed_feedback_loop_count", body.closed_feedback_loop_count || 0],
    ["end_to_end_ownership_min", body.end_to_end_ownership_min ?? null],
    ["end_to_end_ownership_mean", body.end_to_end_ownership_mean ?? null],
    ["end_to_end_ownership_count", body.end_to_end_ownership_count || 0],
  ].filter(([name]) => columns.has(name));
  if (values.length > 1) {
    await insertDynamic(env, "todo_session_details", values);
  }
}

async function insertWebDetails(env, body, columns) {
  if (!columns || columns.size === 0 || !body.event_id || !columns.has("event_id")) {
    return;
  }
  const values = [
    ["event_id", body.event_id],
    ["path", body.path || null],
    ["referrer", body.referrer || null],
    ["visitor_id", body.visitor_id || null],
    ["utm_source", body.utm_source || null],
    ["utm_medium", body.utm_medium || null],
    ["utm_campaign", body.utm_campaign || null],
    ["cta", body.cta || null],
    ["metric_name", body.metric_name || null],
    ["metric_value", body.metric_value ?? null],
    ["rating", body.rating || null],
    ["error_kind", body.error_kind || null],
    ["pageview_id", body.pageview_id || null],
    ["conversion_id", body.conversion_id || null],
    ["placement", body.placement || null],
    ["install_method", body.install_method || null],
  ].filter(([name]) => columns.has(name));
  if (values.length > 1) {
    await insertDynamic(env, "web_details", values);
  }
}

async function insertInstallDetails(env, body, columns) {
  if (!columns || columns.size === 0 || !body.event_id || !columns.has("event_id")) {
    return;
  }
  const values = [
    ["event_id", body.event_id],
    ["conversion_id", body.conversion_id],
    ["stage", body.stage],
    ["outcome", body.outcome],
    ["source", body.source],
    ["placement", body.placement || null],
    ["install_method", body.install_method || null],
    ["failure_stage", body.failure_stage || null],
  ].filter(([name]) => columns.has(name));
  if (values.length > 1) {
    await insertDynamic(env, "install_details", values);
  }
}

// Normalize a website beacon event in place. Browsers do not send
// version/os/arch and mint an anonymous visitor_id in localStorage, so the
// visitor_id doubles as the telemetry id and free-text fields are
// length-capped (same defensive posture as the firehose blob caps).
// Returns an error string when the event is invalid, otherwise null.
function normalizeWebEvent(body) {
  if (typeof body.visitor_id !== "string" || body.visitor_id.length === 0) {
    return "Missing visitor_id";
  }
  if (typeof body.path !== "string" || body.path.length === 0) {
    return "Missing path";
  }
  if (body.event === "web_cta_click" && (typeof body.cta !== "string" || body.cta.length === 0)) {
    return "Missing cta";
  }
  if (body.event === "web_vital") {
    const metricNames = new Set(["CLS", "FCP", "INP", "LCP", "TTFB"]);
    const ratings = new Set(["good", "needs-improvement", "poor"]);
    if (!metricNames.has(body.metric_name)) {
      return "Invalid metric_name";
    }
    if (typeof body.metric_value !== "number" || !Number.isFinite(body.metric_value) || body.metric_value < 0) {
      return "Invalid metric_value";
    }
    if (!ratings.has(body.rating)) {
      return "Invalid rating";
    }
    const cap = body.metric_name === "CLS" ? 10 : 300_000;
    body.metric_value = Math.min(body.metric_value, cap);
  }
  if (body.event === "web_error") {
    const errorKinds = new Set(["script", "promise", "resource"]);
    if (!errorKinds.has(body.error_kind)) {
      return "Invalid error_kind";
    }
    // Keep only route-level context for errors. In particular, do not retain
    // referrer or campaign values that may contain full URLs or query strings.
    for (const field of [
      "referrer", "utm_source", "utm_medium", "utm_campaign", "cta",
      "session_id", "pageview_id", "conversion_id", "placement", "install_method",
    ]) {
      delete body[field];
    }
  }
  if (body.conversion_id != null && !CONVERSION_ID_PATTERN.test(body.conversion_id)) {
    return "Invalid conversion_id";
  }
  if (body.install_method != null && !new Set([
    "shell", "download_macos", "download_linux", "download_windows",
  ]).has(body.install_method)) {
    return "Invalid install_method";
  }
  if (body.placement != null && !new Set(["hero", "sticky"]).has(body.placement)) {
    return "Invalid install placement";
  }
  // Error payloads are classification-only. Never retain messages, stacks,
  // filenames, or full URLs even if a caller includes them.
  for (const field of ["message", "error_message", "stack", "url", "filename", "source_url"]) {
    delete body[field];
  }
  body.visitor_id = body.visitor_id.slice(0, 96);
  if (body.session_id != null) body.session_id = String(body.session_id).slice(0, 96);
  body.id = body.id || body.visitor_id;
  // The beacon does not send an event_id, but web_details rows join on it.
  // Mint one server-side so path/referrer/utm/cta are actually persisted.
  body.event_id = body.event_id || crypto.randomUUID();
  body.version = body.version || "web";
  body.os = body.os || "web";
  body.arch = body.arch || "web";
  for (const field of [
    "path", "referrer", "utm_source", "utm_medium", "utm_campaign", "cta",
    "pageview_id", "conversion_id", "placement", "install_method",
  ]) {
    if (body[field] != null) {
      body[field] = String(body[field]).slice(0, 200);
    }
  }
  return null;
}

const CONVERSION_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

function normalizeInstallConversion(body) {
  if (!CONVERSION_ID_PATTERN.test(body.install_conversion_id)) {
    return "Invalid install_conversion_id";
  }
  body.conversion_id = body.install_conversion_id.toLowerCase();
  return null;
}

function normalizeInstallFunnelEvent(body) {
  const stages = new Set(["command_copy", "script_request", "installer_start", "installer_finish"]);
  const outcomes = new Set(["success", "failure"]);
  const sources = new Set(["browser", "install_endpoint", "installer"]);
  const methods = new Set(["shell", "download_macos", "download_linux", "download_windows"]);
  const placements = new Set(["hero", "sticky"]);

  if (!CONVERSION_ID_PATTERN.test(body.conversion_id)) {
    return "Invalid conversion_id";
  }
  if (!stages.has(body.stage)) {
    return "Invalid install stage";
  }
  if (!outcomes.has(body.outcome)) {
    return "Invalid install outcome";
  }
  if (!sources.has(body.source)) {
    return "Invalid install source";
  }
  if (body.install_method != null && !methods.has(body.install_method)) {
    return "Invalid install_method";
  }
  if (body.placement != null && !placements.has(body.placement)) {
    return "Invalid install placement";
  }
  if (body.failure_stage != null && !/^[a-z0-9_]{0,64}$/i.test(body.failure_stage)) {
    return "Invalid failure_stage";
  }
  if (body.outcome !== "failure") {
    body.failure_stage = null;
  }
  body.conversion_id = body.conversion_id.toLowerCase();
  body.id = body.id || body.conversion_id;
  body.event_id = body.event_id || crypto.randomUUID();
  for (const field of ["stage", "outcome", "source", "placement", "install_method", "failure_stage"]) {
    if (body[field] != null) body[field] = String(body[field]).slice(0, 100);
  }
  return null;
}

// Normalize a token-subscription event in place. account_id is required for
// all of them; account_linked is the analytics<->account join event and also
// requires the telemetry id. Returns an error string or null.
function normalizeSubscriptionEvent(body) {
  if (typeof body.account_id !== "string" || body.account_id.length === 0) {
    return "Missing account_id";
  }
  body.account_id = body.account_id.slice(0, 96);
  for (const field of ["tier", "model"]) {
    if (body[field] != null) {
      body[field] = String(body[field]).slice(0, 200);
    }
  }
  return null;
}

function normalizeDiscoveryEvent(body) {
  const phases = new Set(["browse", "select", "suggest", "unknown"]);
  const outcomes = new Set(["success", "failure"]);
  const failures = new Set([
    "disabled", "invalid_input", "invalid_category", "timeout",
    "connect_error", "transport_error", "http_error", "body_error",
    "response_too_large", "invalid_json", "invalid_response",
  ]);
  if (typeof body.request_id !== "string" || body.request_id.length === 0) {
    return "Missing request_id";
  }
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(body.request_id)) {
    return "Invalid request_id";
  }
  if (!phases.has(body.phase)) {
    return "Invalid discovery phase";
  }
  if (!outcomes.has(body.outcome)) {
    return "Invalid discovery outcome";
  }
  if (body.outcome === "failure" && !failures.has(body.failure_reason)) {
    return "Invalid discovery failure_reason";
  }
  if (body.outcome === "success") {
    body.failure_reason = null;
  }
  body.request_id = body.request_id.slice(0, 96);
  for (const field of ["category", "selected_tool"]) {
    if (body[field] != null) {
      body[field] = String(body[field]).slice(0, 100);
      if (!/^[a-z0-9][a-z0-9 ._+\/-]{0,99}$/i.test(body[field])) {
        return `Invalid discovery ${field}`;
      }
    }
  }
  body.http_status = body.http_status == null ? null : Math.max(100, Math.min(599, Number(body.http_status) || 0));
  body.latency_ms = Math.max(0, Math.min(300_000, Number(body.latency_ms) || 0));
  body.response_bytes = body.response_bytes == null ? null : Math.max(0, Math.min(1_048_576, Number(body.response_bytes) || 0));
  body.result_count = body.result_count == null ? null : Math.max(0, Math.min(10_000, Number(body.result_count) || 0));
  body.benchmark_run = body.benchmark_run === true;
  return null;
}

function normalizeTodoSessionEvent(body) {
  const uuidV4 = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
  const endReasons = new Set([
    "normal_exit", "panic", "signal", "disconnect", "reload", "superseded", "unknown",
  ]);
  if (typeof body.correlation_id !== "string" || !uuidV4.test(body.correlation_id)) {
    return "Invalid todo correlation_id";
  }
  // This event must never carry the persistent telemetry ID. Its required `id`
  // envelope field is the same ephemeral UUID used for the within-session join.
  if (body.id !== body.correlation_id) {
    return "Todo id must equal correlation_id";
  }
  if (!endReasons.has(body.session_end_reason)) {
    return "Invalid todo session_end_reason";
  }

  for (const field of [
    "todos_created", "todos_completed", "todos_abandoned", "todo_updates",
    "groups_completed", "groups_total", "max_todo_list_size", "confidence_count",
    "completion_confidence_count", "understands_user_intent_count",
    "closed_feedback_loop_count", "end_to_end_ownership_count",
  ]) {
    body[field] = Math.max(0, Math.min(1_000_000, Math.trunc(Number(body[field]) || 0)));
  }
  for (const field of [
    "confidence_min", "confidence_mean", "completion_confidence_min",
    "completion_confidence_mean", "understands_user_intent_min",
    "understands_user_intent_mean", "closed_feedback_loop_min",
    "closed_feedback_loop_mean", "end_to_end_ownership_min", "end_to_end_ownership_mean",
  ]) {
    body[field] = body[field] == null
      ? null
      : Math.max(0, Math.min(100, Number(body[field]) || 0));
  }
  return null;
}

async function insertDynamic(env, table, entries) {
  const columns = entries.map(([name]) => name);
  const placeholders = columns.map(() => "?").join(", ");
  const sql = `INSERT OR IGNORE INTO ${table} (${columns.join(", ")}) VALUES (${placeholders})`;
  const values = entries.map(([, value]) => value);
  const result = await env.DB.prepare(sql).bind(...values).run();
  observeDbSize(result);
  return result;
}

function boolToInt(value) {
  return value ? 1 : 0;
}

function jsonResponse(data, status = 200, cors = null) {
  return new Response(JSON.stringify(data), {
    status,
    headers: {
      "Content-Type": "application/json",
      ...(cors || corsHeaders()),
    },
  });
}

function corsHeaders(request = null, env = null) {
  // Default policy: ALLOWED_ORIGIN var (currently "*"; telemetry is anonymous
  // and unauthenticated). Website beacon origins are additionally echoed back
  // explicitly, so browser preflights keep passing even if ALLOWED_ORIGIN is
  // ever narrowed away from "*".
  let allowOrigin = env?.ALLOWED_ORIGIN || "*";
  const origin = request?.headers?.get?.("Origin");
  const headers = {
    "Access-Control-Allow-Methods": "POST, OPTIONS",
    "Access-Control-Allow-Headers": "Content-Type",
  };
  if (origin && WEB_ALLOWED_ORIGINS.has(origin)) {
    allowOrigin = origin;
    headers["Vary"] = "Origin";
  }
  headers["Access-Control-Allow-Origin"] = allowOrigin;
  return headers;
}
