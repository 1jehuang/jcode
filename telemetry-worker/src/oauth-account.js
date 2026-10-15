// Private model-provider OAuth account telemetry. Never forward to analytics or logs.
const PROVIDERS = new Set(["anthropic", "openai", "gemini", "copilot", "antigravity"]);
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const HASH = /^[0-9a-f]{64}$/;
const MAX_BYTES = 1024;

function reply(body, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json", "Cache-Control": "no-store", "X-Content-Type-Options": "nosniff" },
  });
}

export async function oauthAccount(request, env, check) {
  // No browser CORS access: a UUID is a correlation identifier, not authentication.
  if (request.headers.has("Origin")) return reply({ error: "Forbidden" }, 403);
  if (request.method !== "POST") return reply({ error: "Method not allowed" }, 405);
  if (!/^application\/json(?:\s*;|$)/i.test(request.headers.get("Content-Type") || "")) {
    return reply({ error: "Expected JSON" }, 415);
  }
  let body;
  try {
    const reader = request.body?.getReader();
    if (!reader) return reply({ error: "Invalid JSON" }, 400);
    const chunks = [];
    let size = 0;
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > MAX_BYTES) {
        await reader.cancel();
        return reply({ error: "Body too large" }, 413);
      }
      chunks.push(value);
    }
    const bytes = new Uint8Array(size);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
    body = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch { return reply({ error: "Invalid JSON" }, 400); }
  const allowed = new Set(check ? ["id", "provider", "email_hash"] : ["id", "provider", "email", "email_hash"]);
  if (!body || typeof body !== "object" || Array.isArray(body)
      || Object.keys(body).some(key => !allowed.has(key))
      || typeof body.id !== "string" || !UUID.test(body.id)
      || !PROVIDERS.has(body.provider)
      || (check && (typeof body.email_hash !== "string" || !HASH.test(body.email_hash)))) {
    return reply({ error: "Invalid account payload" }, 400);
  }
  body.id = body.id.toLowerCase();
  let email;
  let hash = body.email_hash;
  if (!check) {
    // ASCII mailbox form only, bounded and without display names/control characters.
    if (typeof body.email !== "string" || body.email.length > 254) return reply({ error: "Invalid email" }, 400);
    email = body.email.trim().toLowerCase();
    if (!/^[a-z0-9.!#$%&'*+\/=?^_`{|}~-]+@[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?\.[a-z]{2,63}$/.test(email)
        || email.split("@")[0].length > 64 || email.includes("..")
        || email.startsWith(".") || email.includes(".@")
        || email.split("@")[1].split(".").some(label => !/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(label))) {
      return reply({ error: "Invalid email" }, 400);
    }
    const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(email));
    const computed = Array.from(new Uint8Array(digest), byte => byte.toString(16).padStart(2, "0")).join("");
    if (hash !== undefined && (typeof hash !== "string" || !HASH.test(hash) || hash !== computed)) {
      return reply({ error: "Invalid email hash" }, 400);
    }
    hash = computed;
  }
  try {
    if (check) {
      const row = await env.DB.prepare(`SELECT
        EXISTS(SELECT 1 FROM events WHERE telemetry_id = ? UNION ALL
          SELECT 1 FROM daily_active_users WHERE telemetry_id = ? UNION ALL
          SELECT 1 FROM oauth_accounts WHERE telemetry_id = ?) AS installation_known,
        EXISTS(SELECT 1 FROM oauth_accounts WHERE telemetry_id = ? AND provider = ? AND email_hash = ?) AS email_tracked`)
        .bind(body.id, body.id, body.id, body.id, body.provider, hash).first();
      const known = Boolean(row?.installation_known);
      // Membership is always installation-scoped, never a global email lookup.
      return reply({ installation_known: known, email_tracked: known && Boolean(row?.email_tracked) });
    }
    // Per-installation deduplication is atomic even if the client preflight raced.
    // New installations may upload too: known-install status gates the notice only.
    await env.DB.prepare(`INSERT INTO oauth_accounts (telemetry_id, provider, email, email_hash)
      VALUES (?, ?, ?, ?)
      ON CONFLICT(telemetry_id, provider, email_hash) DO NOTHING`)
      .bind(body.id, body.provider, email, hash).run();
    // Do not expose insertion/dedup status as an account-membership oracle.
    return reply({ ok: true });
  } catch {
    // Never echo database errors, payloads, email addresses, or private digests.
    return reply({ error: "Account storage unavailable" }, 503);
  }
}
