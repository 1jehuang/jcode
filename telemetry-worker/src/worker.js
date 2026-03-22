let cachedEventColumns = null;

export default {
  async fetch(request, env) {
    if (request.method === "OPTIONS") {
      return new Response(null, {
        headers: corsHeaders(),
      });
    }

    if (request.method !== "POST") {
      return jsonResponse({ error: "Method not allowed" }, 405);
    }

    const url = new URL(request.url);
    if (url.pathname !== "/v1/event") {
      return jsonResponse({ error: "Not found" }, 404);
    }

    let body;
    try {
      body = await request.json();
    } catch {
      return jsonResponse({ error: "Invalid JSON" }, 400);
    }

    if (!body.id || !body.event || !body.version || !body.os || !body.arch) {
      return jsonResponse({ error: "Missing required fields" }, 400);
    }

    if (![
      "install",
      "session_start",
      "session_end",
      "session_crash",
    ].includes(body.event)) {
      return jsonResponse({ error: "Unknown event type" }, 400);
    }

    try {
      await insertEvent(env, body);

      return jsonResponse({ ok: true });
    } catch (err) {
      return jsonResponse({ error: "Internal error" }, 500);
    }
  },
};

async function insertEvent(env, body) {
  const columns = await getEventColumns(env);

  if (body.event === "install") {
    return insertDynamic(env, [
      ["telemetry_id", body.id],
      ["event", body.event],
      ["version", body.version],
      ["os", body.os],
      ["arch", body.arch],
    ]);
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
    ];
    if (columns.has("resumed_session")) {
      values.push(["resumed_session", boolToInt(body.resumed_session)]);
    }
    return insertDynamic(env, values);
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
      ["turns", body.turns || 0],
      ["had_user_prompt", boolToInt(body.had_user_prompt)],
      ["had_assistant_response", boolToInt(body.had_assistant_response)],
      ["assistant_responses", body.assistant_responses || 0],
      ["tool_calls", body.tool_calls || 0],
      ["tool_failures", body.tool_failures || 0],
      ["transport_https", body.transport_https || 0],
      ["transport_persistent_ws_fresh", body.transport_persistent_ws_fresh || 0],
      ["transport_persistent_ws_reuse", body.transport_persistent_ws_reuse || 0],
      ["transport_cli_subprocess", body.transport_cli_subprocess || 0],
      ["transport_native_http2", body.transport_native_http2 || 0],
      ["transport_other", body.transport_other || 0],
      ["resumed_session", boolToInt(body.resumed_session)],
      ["end_reason", body.end_reason || null],
      ["error_provider_timeout", errors.provider_timeout || 0],
      ["error_auth_failed", errors.auth_failed || 0],
      ["error_tool_error", errors.tool_error || 0],
      ["error_mcp_error", errors.mcp_error || 0],
      ["error_rate_limited", errors.rate_limited || 0],
    ].filter(([name]) => columns.has(name));
    return insertDynamic(env, values);
  }
}

async function getEventColumns(env) {
  if (cachedEventColumns) {
    return cachedEventColumns;
  }
  const result = await env.DB.prepare("PRAGMA table_info(events)").all();
  cachedEventColumns = new Set((result.results || []).map((row) => row.name));
  return cachedEventColumns;
}

async function insertDynamic(env, entries) {
  const columns = entries.map(([name]) => name);
  const placeholders = columns.map(() => "?").join(", ");
  const sql = `INSERT INTO events (${columns.join(", ")}) VALUES (${placeholders})`;
  const values = entries.map(([, value]) => value);
  await env.DB.prepare(sql).bind(...values).run();
}

function boolToInt(value) {
  return value ? 1 : 0;
}

function jsonResponse(data, status = 200) {
  return new Response(JSON.stringify(data), {
    status,
    headers: {
      "Content-Type": "application/json",
      ...corsHeaders(),
    },
  });
}

function corsHeaders() {
  return {
    "Access-Control-Allow-Origin": "*",
    "Access-Control-Allow-Methods": "POST, OPTIONS",
    "Access-Control-Allow-Headers": "Content-Type",
  };
}
