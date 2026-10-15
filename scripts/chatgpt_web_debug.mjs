#!/usr/bin/env node
// ChatGPT web debugging harness for jcode's [web] routes.
//
// Runs against a COPY of jcode's private ChatGPT profile, never the live one:
//   cp -a ~/.jcode/chatgpt-web/profile /tmp/cgw-profile && rm -f /tmp/cgw-profile/Singleton*
//
// Commands (all use Temporary Chats):
//   capture  <prompt-file>   Network log of one turn (HTTP, SSE, websockets), secrets redacted.
//   turn     <prompt-file>   One network-path turn through chatgpt_web_ui.js; checks the model
//                            received the exact prompt and the decoded stream equals the transcript.
//   catalog                  The account's model catalog from /backend-api/models (no message sent).
//   probe    <slug[:effort]>... Sends a tiny prompt per model slug and reports which model served it.
//
// Environment:
//   CGW_PROFILE   profile copy directory (required)
//   CGW_CHROME    Chrome/Chromium executable (default: google-chrome on PATH)
//   CGW_OUT       output file prefix (default: ./cgw)
//   PLAYWRIGHT    path to a directory containing node_modules/playwright (default: cwd)
//
// Output files are written 0600. They can contain conversation text; keep them private.
import { createRequire } from 'node:module';
import { readFileSync, writeFileSync, chmodSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const here = path.dirname(fileURLToPath(import.meta.url));
const UI = readFileSync(path.join(here, '../crates/jcode-base/src/provider/chatgpt_web_ui.js'), 'utf8');
const require = createRequire(path.join(process.env.PLAYWRIGHT || process.cwd(), 'package.json'));
const { chromium } = require('playwright');

const [command, ...args] = process.argv.slice(2);
const profile = process.env.CGW_PROFILE;
if (!command || !profile) {
  console.error('usage: CGW_PROFILE=<profile copy> node scripts/chatgpt_web_debug.mjs <capture|turn|catalog|probe> [args]');
  process.exit(2);
}
const out = process.env.CGW_OUT || './cgw';
const save = (suffix, data) => {
  const file = `${out}-${suffix}`;
  writeFileSync(file, typeof data === 'string' ? data : JSON.stringify(data, null, 1));
  chmodSync(file, 0o600);
  return file;
};
const fingerprint = s => { let h = 2166136261; for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619); return { length: s.length, hash: h >>> 0 }; };
const SECRET = /authorization|cookie|token|proof|turnstile|arkose|sentinel|session|csrf|secret|password|signature|device-id|^dx$|^p$|^seed$|access|refresh|integrity|jwt|nonce/i;
const redactJson = (v, d = 0) => d > 40 || v === null || typeof v !== 'object' ? v : Array.isArray(v) ? v.map(x => redactJson(x, d + 1))
  : Object.fromEntries(Object.entries(v).map(([k, x]) => [k, SECRET.test(k) && typeof x !== 'object' ? `<redacted len=${String(x).length}>` : redactJson(x, d + 1)]));
const redactText = t => { if (!t) return t; try { return JSON.stringify(redactJson(JSON.parse(t))); } catch {}
  return t.replace(/eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}/g, '<redacted-jwt>').replace(/[A-Za-z0-9+/=_-]{120,}/g, m => `<redacted-blob len=${m.length}>`); };
const redactHeaders = h => Object.fromEntries(Object.entries(h || {}).map(([k, v]) => [k, SECRET.test(k) ? `<redacted len=${String(v).length}>` : v]));

const ctx = await chromium.launchPersistentContext(profile, {
  executablePath: process.env.CGW_CHROME || undefined, channel: process.env.CGW_CHROME ? undefined : 'chrome',
  headless: true, viewport: { width: 1280, height: 900 },
});
const open = async () => {
  const page = await ctx.newPage();
  const ui = async request => page.evaluate(`(async () => { const request=${JSON.stringify(request)};\n${UI} })()`);
  await page.goto('https://chatgpt.com/?temporary-chat=true');
  return { page, ui };
};
const waitStream = async (ui, key, limitMs = 300000) => {
  const started = Date.now();
  for (;;) {
    const s = await ui({ op: 'stream', key });
    if (s.complete || s.error || s.sendError || (s.done && s.started) || Date.now() - started > limitMs) return s;
    await new Promise(r => setTimeout(r, 250));
  }
};
const transcript = async ui => { let t; for (let i = 0; i < 20; i++) { t = await ui({ op: 'transcript' }); if (t.complete) break; await new Promise(r => setTimeout(r, 500)); } return t; };

try {
  if (command === 'catalog') {
    const { page } = await open();
    await page.waitForTimeout(3000);
    const catalog = await page.evaluate(async () => {
      const session = await (await fetch('/api/auth/session')).json();
      const r = await fetch('/backend-api/models?iim=false&include_icons=false&is_gizmo=false', { headers: { authorization: `Bearer ${session.accessToken}` } });
      const d = await r.json();
      return { default_model_slug: d.default_model_slug, versions: d.versions, categories: d.categories,
        models: d.models.map(m => ({ slug: m.slug, title: m.title, max_tokens: m.max_tokens, reasoning_type: m.reasoning_type,
          configurable_thinking_effort: m.configurable_thinking_effort, thinking_efforts: m.thinking_efforts, enabled_tools: m.enabled_tools,
          is_work_mode_model: m.is_work_mode_model, image_input: !!m.product_features?.attachments?.image_mime_types?.length })) };
    });
    for (const m of catalog.models) console.log(`${m.slug.padEnd(20)} ${m.title.padEnd(16)} ctx=${String(m.max_tokens).padEnd(7)} ${m.reasoning_type.padEnd(9)} efforts=${(m.thinking_efforts || []).map(e => e.thinking_effort).join('/') || '-'}`);
    console.log('saved', save('catalog.json', catalog));
  } else if (command === 'turn') {
    const prompt = readFileSync(args[0], 'utf8');
    const { ui } = await open();
    await ui({ op: 'prepare', label: process.env.CGW_LABEL || 'GPT-6', effort: null });
    const key = `jcode-debug-${Date.now()}`;
    const placeholder = `jcode turn ${key}`;
    await ui({ op: 'arm_send', key, text: prompt, placeholder });
    await ui({ op: 'insert', text: placeholder });
    const f = fingerprint(placeholder);
    const sel = await ui({ op: 'verify', label: process.env.CGW_LABEL || 'GPT-6', effort: '' });
    await ui({ op: 'submit', label: sel.model, effort: sel.effort, length: f.length, hash: f.hash, cancelKey: `${key}-submit`, armedKey: key });
    const s = await waitStream(ui, key);
    const t = await transcript(ui);
    const expected = fingerprint(prompt);
    const a = (s.texts || []).join('\n\n'), b = (t?.texts || []).join('\n\n');
    console.log(JSON.stringify({ complete: s.complete, error: s.error || s.sendError, builtin: s.builtin,
      promptExact: s.substituted?.length === expected.length && s.substituted?.hash === expected.hash, decoderMatchesTranscript: a === b, answer: a.slice(0, 300) }, null, 1));
  } else if (command === 'capture') {
    const prompt = readFileSync(args[0], 'utf8');
    const events = []; const t0 = Date.now(); const log = (kind, data) => events.push({ ms: Date.now() - t0, kind, ...data });
    const page = await ctx.newPage(); const cdp = await ctx.newCDPSession(page);
    await cdp.send('Network.enable', { maxPostDataSize: 2_000_000 });
    const ours = new Set(); const wanted = u => /chatgpt\.com|openai\.com/.test(u) && !/\.(js|css|woff2?|png|svg|jpe?g|webp|ico|map)(\?|$)/.test(u);
    cdp.on('Network.requestWillBeSent', e => { if (!wanted(e.request.url)) return; ours.add(e.requestId); log('request', { id: e.requestId, method: e.request.method, url: e.request.url, headers: redactHeaders(e.request.headers), body: redactText(e.request.postData)?.slice(0, 20000) }); });
    cdp.on('Network.responseReceived', e => { if (ours.has(e.requestId)) log('response', { id: e.requestId, status: e.response.status, mime: e.response.mimeType, headers: redactHeaders(e.response.headers) }); });
    cdp.on('Network.webSocketCreated', e => log('ws_created', { id: e.requestId, url: e.url.replace(/([?&](?:verify|token|ticket|key|sig)[^=&]*=)[^&]+/gi, '$1<redacted>') }));
    cdp.on('Network.webSocketFrameSent', e => log('ws_sent', { id: e.requestId, data: redactText(e.response.payloadData).slice(0, 8000) }));
    cdp.on('Network.webSocketFrameReceived', e => log('ws_recv', { id: e.requestId, data: redactText(e.response.payloadData).slice(0, 8000) }));
    const ui = async request => page.evaluate(`(async () => { const request=${JSON.stringify(request)};\n${UI} })()`);
    await page.goto('https://chatgpt.com/?temporary-chat=true');
    await ui({ op: 'prepare', label: process.env.CGW_LABEL || 'GPT-6', effort: null });
    const key = `jcode-debug-${Date.now()}`;
    await ui({ op: 'arm_send', key, text: prompt, placeholder: `jcode turn ${key}` });
    await ui({ op: 'insert', text: `jcode turn ${key}` });
    await page.evaluate(() => document.querySelector('button[type="submit"][aria-label="Send"], #composer-submit-button').click());
    await waitStream(ui, key);
    await page.waitForTimeout(5000);
    const counts = {}; for (const e of events) counts[e.kind] = (counts[e.kind] || 0) + 1;
    console.log(JSON.stringify(counts), 'saved', save('capture.json', events));
  } else if (command === 'probe') {
    for (const spec of args) {
      const [slug, effort] = spec.split(':');
      const { page, ui } = await open();
      try {
        await ui({ op: 'prepare', label: 'GPT-6', effort: null });
        const key = `jcode-probe-${Date.now()}`;
        await ui({ op: 'arm_send', key, text: 'Reply with exactly: PROBE_OK', placeholder: `jcode turn ${key}`, route: { slug, effort: effort || null } });
        await ui({ op: 'insert', text: `jcode turn ${key}` });
        await page.evaluate(() => document.querySelector('button[type="submit"][aria-label="Send"], #composer-submit-button').click());
        const s = await waitStream(ui, key, 240000);
        console.log(JSON.stringify({ slug, effort: effort || null, served: s.served, servedEffort: s.servedEffort, complete: s.complete, error: s.error || s.sendError, answer: (s.texts || []).join(' ').slice(0, 40) }));
      } finally { await page.close(); }
    }
  } else {
    console.error(`unknown command ${command}`);
    process.exitCode = 2;
  }
} catch (error) {
  console.error('ERROR', error.message.split('\n')[0]);
  process.exitCode = 1;
} finally {
  await ctx.close();
}
