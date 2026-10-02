// Version 2 measures live runtime Agent incarnations, not the historical
// process-global lifecycle singleton. Never coerce, clip, or infer missing data.
export const CONCURRENCY_EVENTS = [
  "session_concurrency", "session_start", "session_end", "session_crash",
];
export const CONCURRENCY_COUNTS = [
  "active_sessions_at_start", "other_active_sessions_at_start",
  "max_concurrent_sessions", "root_sessions_at_start", "child_sessions_at_start",
  "max_concurrent_root_sessions", "max_concurrent_child_sessions",
];
export const CONCURRENCY_FIELDS = [
  "concurrency_tracking_version", "concurrency_tracking_scope",
  "concurrency_tracking_available", "concurrency_session_id", "phase", "agent_role",
  ...CONCURRENCY_COUNTS, "multi_sessioned", "is_ci",
];
const has = (body, key) => Object.hasOwn(body, key);
const integer = (value) => Number.isSafeInteger(value) && value >= 0;
const presentString = (value) => typeof value === "string" && value.trim().length > 0;

export function classifyConcurrency(body) {
  const result = (quality, reason) => ({ quality, reason });
  if (!has(body, "concurrency_tracking_version")) {
    return result("missing_version", "concurrency_tracking_version");
  }
  const version = body.concurrency_tracking_version;
  if (!integer(version)) return result("invalid", "invalid_tracking_version");
  if (version < 2) return result("legacy_version", "pre_v2");
  if (version !== 2) return result("unsupported_version", "unknown_tracking_version");
  // A lifecycle event cannot become a runtime-Agent observation just by carrying v2.
  if (body.event !== "session_concurrency") {
    return result("legacy_scope", "process_global_lifecycle");
  }
  for (const key of ["concurrency_tracking_scope", "concurrency_tracking_available"]) {
    if (!has(body, key)) return result("missing_fields", key);
  }
  if (body.concurrency_tracking_scope !== "runtime_agent_sessions") {
    return result("invalid", "invalid_tracking_scope");
  }
  if (typeof body.concurrency_tracking_available !== "boolean") {
    return result("invalid", "invalid_tracking_available");
  }
  if (!body.concurrency_tracking_available) return result("unavailable", "tracker_unavailable");
  for (const key of ["phase", "agent_role", "concurrency_session_id", "session_id", "is_ci",
    ...CONCURRENCY_COUNTS, "multi_sessioned"]) {
    if (!has(body, key)) return result("missing_fields", key);
  }
  if (!["start", "end"].includes(body.phase)) return result("invalid", "invalid_phase");
  if (!["root", "child"].includes(body.agent_role)) return result("invalid", "invalid_agent_role");
  if (!presentString(body.concurrency_session_id) || !presentString(body.session_id)) {
    return result("invalid", "invalid_session_identity");
  }
  if (typeof body.is_ci !== "boolean") return result("invalid", "invalid_is_ci");
  for (const key of CONCURRENCY_COUNTS) {
    if (!integer(body[key])) return result("invalid", `invalid_integer:${key}`);
  }
  if (typeof body.multi_sessioned !== "boolean") return result("invalid", "invalid_multi_sessioned");
  const active = body.active_sessions_at_start;
  const peak = body.max_concurrent_sessions;
  const root = body.root_sessions_at_start;
  const child = body.child_sessions_at_start;
  const rootPeak = body.max_concurrent_root_sessions;
  const childPeak = body.max_concurrent_child_sessions;
  if (active < 1 || peak < active) return result("invalid", "peak_below_start_or_empty_start");
  if (body.other_active_sessions_at_start !== active - 1) return result("invalid", "other_disagrees");
  if (body.multi_sessioned !== (peak > 1)) return result("invalid", "multi_disagrees");
  // Subtraction avoids an unsafe sum near Number.MAX_SAFE_INTEGER.
  if (root > active || child !== active - root) return result("invalid", "roles_disagree_with_start");
  if ((body.agent_role === "root" && root < 1) || (body.agent_role === "child" && child < 1)) {
    return result("invalid", "own_role_missing_at_start");
  }
  if (rootPeak < root || childPeak < child || rootPeak > peak || childPeak > peak
    || rootPeak < peak - childPeak) return result("invalid", "invalid_role_peaks");
  if (body.phase === "start" && (peak !== active || rootPeak !== root || childPeak !== child)) {
    return result("invalid", "start_peaks_disagree");
  }
  return result("trusted", "validated_v2");
}

export function rawConcurrency(body) {
  return JSON.stringify(Object.fromEntries(CONCURRENCY_FIELDS
    .filter((key) => has(body, key)).map((key) => [key, body[key]])));
}

// Compatibility columns remain raw/untrusted. JSON containers cannot be bound
// to D1, so retain them as JSON text instead of dropping the whole valid event.
// Missing stays NULL, unlike the historical || 0 default. Exact parsed values
// (including booleans, nulls, strings and missing keys) also live in raw_json.
export function rawConcurrencyScalar(value) {
  if (value == null) return null;
  if (typeof value === "boolean") return Number(value);
  if (typeof value === "object") return JSON.stringify(value);
  return value;
}

export function concurrencyEntries(body) {
  const { quality, reason } = classifyConcurrency(body);
  return [
    ["event_id", body.event_id],
    ["concurrency_tracking_version", integer(body.concurrency_tracking_version) ? body.concurrency_tracking_version : null],
    ["concurrency_tracking_scope", typeof body.concurrency_tracking_scope === "string" ? body.concurrency_tracking_scope : null],
    ["concurrency_tracking_available", typeof body.concurrency_tracking_available === "boolean" ? Number(body.concurrency_tracking_available) : null],
    ["concurrency_session_id", typeof body.concurrency_session_id === "string" ? body.concurrency_session_id : null],
    ["phase", typeof body.phase === "string" ? body.phase : null],
    ["agent_role", typeof body.agent_role === "string" ? body.agent_role : null],
    ["runtime_is_ci", typeof body.is_ci === "boolean" ? Number(body.is_ci) : null],
    ...CONCURRENCY_COUNTS.map((key) => [key, integer(body[key]) ? body[key] : null]),
    ["multi_sessioned", typeof body.multi_sessioned === "boolean" ? Number(body.multi_sessioned) : null],
    ["quality", quality], ["quality_reason", reason], ["raw_json", rawConcurrency(body)],
  ];
}
