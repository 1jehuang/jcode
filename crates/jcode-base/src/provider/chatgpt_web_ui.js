// Shared DOM workflow for account discovery and verified temporary-chat turns.
// The caller prefixes a JSON-serialized `request`. No page text is executable.
return (async () => {
  const hidden = '[aria-hidden="true"],[inert],[data-active="false"],[hidden]';
  const visible = e => !!e && e.getClientRects().length > 0 && !e.closest(hidden);
  const all = (selector, root = document) => Array.from(root.querySelectorAll(selector));
  const one = selector => all(selector).find(visible) || null;
  // Every evaluation is bounded. jcode's DevTools command timeout is 60
  // seconds, so stop early enough to run preference restoration and return
  // an error first. A cold prepare is ~12s, so leave headroom for slow loads.
  const started = Date.now();
  let restoring = false;
  function checkBudget() {
    if (Date.now() - started >= (restoring ? 55_000 : 40_000)) throw Error('ChatGPT UI evaluation exceeded its time budget');
  }
  // A hidden tab (occluded or background browser window) clamps timers to
  // ~1s steps, which overruns the budget. MessageChannel tasks are not throttled.
  const sleep = ms => document.hidden ? new Promise(resolve => {
    const end = Date.now() + ms;
    const channel = new MessageChannel();
    channel.port1.onmessage = () => {
      if (Date.now() < end) return channel.port2.postMessage(0);
      channel.port1.close();
      resolve();
    };
    channel.port2.postMessage(0);
  }) : new Promise(resolve => setTimeout(resolve, ms));
  const delay = async () => { checkBudget(); await sleep(300); checkBudget(); };
  async function until(check, description, attempts = 30) {
    for (let attempt=0; attempt<attempts; attempt++) {
      checkBudget();
      if (check()) return;
      await delay();
    }
    throw Error(`ChatGPT did not finish ${description}`);
  }
  const effortNames = { instant: 'none', medium: 'medium', high: 'high', 'extra high': 'xhigh', pro: 'max' };
  const trigger = () => one('button[aria-label="Select ChatGPT model"]');
  // The live legacy data-selected-reasoning-effort attribute aliases Extra
  // High to "max" and Pro to "medium". It is not proof of the Power choice.
  // Corroborate the labeled slider with the independently rendered closed
  // trigger label instead, keeping all five observed Power levels distinct.
  const closedTriggerEffort = model => {
    let label = (trigger()?.innerText || '').trim().toLowerCase();
    // Older checked GPT rows render a version badge, e.g. "5.6 Medium".
    // Remove only the exact version of the independently checked model.
    const version = model?.match(/^GPT-(\d+(?:\.\d+)*)(?: |$)/)?.[1];
    if (version && label.startsWith(`${version} `)) label = label.slice(version.length + 1);
    return effortNames[label] || '';
  };
  const editorSelector = '[contenteditable="true"][data-composer-markdown], [contenteditable="true"][aria-label="Chat with ChatGPT"]';
  const submitSelector = 'button[type="submit"][aria-label="Send"], #composer-submit-button';
  const rowLabel = e => (e.innerText || '').split('\n').map(s => s.trim()).find(Boolean) || '';
  const rows = () => all('[role="menuitemradio"]').filter(visible);
  const unavailable = e => e.disabled || e.getAttribute('aria-disabled') === 'true' || e.hasAttribute('data-disabled') ||
    (e.getAttribute('aria-describedby') || '').split(/\s+/).some(id => /locked|opens access|upgrade/i.test(document.getElementById(id)?.innerText || ''));
  const temporary = () => !!one('button[aria-label="Turn off temporary chat"]');

  function checkOrigin() {
    if (location.hostname !== 'chatgpt.com') throw Error('ChatGPT bridge refused a non-ChatGPT page');
  }
  const isTemporaryNotice = dialog => /temporary chat/i.test(dialog.innerText || '') && /(won[’']t appear|not appear).*(history|conversation)/i.test(dialog.innerText || '');
  function checkAccess(allowTemporaryNotice = false) {
    if (all('button,a').filter(visible).some(e => /^(log in|sign up)$/i.test((e.innerText || '').trim()))) {
      throw Error('ChatGPT is signed out. Log in in the connected browser');
    }
    for (const dialog of all('[role="dialog"]').filter(visible)) {
      // A read-only chat side panel is not onboarding or an access decision.
      if (dialog.getAttribute('aria-label') === 'Chat' || (allowTemporaryNotice && isTemporaryNotice(dialog))) continue;
      throw Error('ChatGPT is waiting for an onboarding or access choice. The bridge will not accept it');
    }
  }
  async function acknowledgeTemporaryNotice() {
    for (const dialog of all('[role="dialog"]').filter(visible)) {
      if (isTemporaryNotice(dialog)) {
        const button = all('button', dialog).find(e => visible(e) && (e.innerText || '').trim() === 'Continue');
        if (button) { button.click(); await delay(); }
      }
    }
    checkAccess();
  }
  function requireTemporary(empty = false) {
    checkOrigin(); checkAccess();
    if (!temporary()) throw Error('ChatGPT did not enter Temporary Chat. Refusing to write persistent history');
    if (empty && one('[data-user-message-bubble], [data-message-author-role], [data-markdown-text-style="assistant-message"]')) {
      throw Error('ChatGPT bridge requires an empty owned Temporary Chat');
    }
  }
  async function closeMenu() {
    const b = trigger();
    if (b?.getAttribute('aria-expanded') === 'true') {
      const menu = one('[role="menu"]');
      if (!menu) throw Error('ChatGPT open picker has no visible menu');
      menu.focus();
      menu.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', code: 'Escape', bubbles: true }));
      await until(() => trigger()?.getAttribute('aria-expanded') !== 'true' && !one('[role="menu"]'), 'closing the model picker');
      await delay(); // Radix restores trigger focus after removing the menu.
    }
  }
  async function openPicker(advanced = false) {
    checkAccess();
    await closeMenu();
    const b = trigger();
    if (!b || unavailable(b)) throw Error('ChatGPT model picker is unavailable');
    // A freshly loaded ChatGPT page intermittently ignores the first trigger
    // click. Re-click only while the visible trigger is verifiably still
    // closed, so a retry can never toggle an open picker shut.
    const menuOpen = () => !!one('[role="menu"]');
    for (let click = 1; ; click++) {
      const current = trigger();
      if (!current || unavailable(current)) throw Error('ChatGPT model picker is unavailable');
      if (current.getAttribute('aria-expanded') !== 'true' && !menuOpen()) current.click();
      try { await until(menuOpen, 'opening the model picker', 8); break; }
      catch (error) { if (click >= 3 || current.getAttribute('aria-expanded') === 'true') throw error; }
    }
    await delay(); checkAccess();
    if (advanced && rows().length === 0) {
      const toggle = one('[data-model-picker-view-toggle="true"]');
      if (!toggle) throw Error('ChatGPT model list cannot be opened');
      toggle.click(); await until(() => rows().length > 0, 'opening the advanced model list'); checkAccess();
    }
    if (!advanced) await until(() => !!one('[data-reasoning-slider="true"]')?.querySelector('[role="slider"]'), 'opening the thinking effort control');
  }
  async function observation() {
    try {
      await openPicker(true);
      const checked = rows().filter(e => e.getAttribute('aria-checked') === 'true');
      if (checked.length !== 1 || unavailable(checked[0])) throw Error('ChatGPT selected model is not uniquely checked and available');
      const model = rowLabel(checked[0]);
      await closeMenu();
      await openPicker(false);
      const power = powerState();
      await closeMenu();
      if (!power.value || power.value !== closedTriggerEffort(model)) throw Error(`ChatGPT thinking effort slider and trigger label disagree: slider '${power.label}' at ${power.index}, trigger '${(trigger()?.innerText || '').trim()}' for '${model}'`);
      return { model, effort: power.value, temporary: temporary() };
    } finally { await closeMenu(); }
  }
  async function selectModel(label) {
    try {
      await openPicker(true);
      const candidates = rows().filter(e => rowLabel(e) === label);
      if (candidates.length !== 1 || unavailable(candidates[0])) throw Error(`ChatGPT model '${label}' is unavailable or locked`);
      if (candidates[0].getAttribute('aria-checked') !== 'true') { candidates[0].click(); await delay(); }
    } finally { await closeMenu(); }
    const selected = await observation();
    if (selected.model !== label) throw Error(`ChatGPT model selection mismatch: '${selected.model}' instead of '${label}'`);
    return selected;
  }
  function powerState() {
    const control = one('[data-reasoning-slider="true"]');
    const slider = control?.querySelector('[role="slider"]');
    if (!control || !slider || unavailable(control)) throw Error('ChatGPT thinking effort control is unavailable');
    const min = Number(slider.getAttribute('aria-valuemin'));
    const max = Number(slider.getAttribute('aria-valuemax'));
    const index = Number(slider.getAttribute('aria-valuenow'));
    if (![min,max,index].every(Number.isInteger) || min < 0 || max < min || max-min > 10 || index < min || index > max) {
      throw Error('ChatGPT thinking effort slider has an unsupported range');
    }
    const description = (control.getAttribute('aria-describedby') || '').split(/\s+/)
      .map(id => document.getElementById(id)?.innerText || '').find(text => /,\s*\d+\s+of\s+\d+/.test(text)) || '';
    const label = description.replace(/,\s*\d+\s+of\s+\d+\.?\s*$/, '').trim();
    const ordinal = description.match(/,\s*(\d+)\s+of\s+(\d+)/);
    if (!ordinal || Number(ordinal[1]) !== index-min+1 || Number(ordinal[2]) !== max-min+1) throw Error('ChatGPT thinking effort label and slider index disagree');
    return { control, min, max, index, label, value: effortNames[label.toLowerCase()] || '' };
  }
  async function movePower(target) {
    for (let step = 0; step < 12; step++) {
      checkAccess();
      const current = powerState();
      if (current.index === target) return current;
      const key = target > current.index ? 'ArrowRight' : 'ArrowLeft';
      current.control.focus();
      current.control.dispatchEvent(new KeyboardEvent('keydown', { key, code: key, bubbles: true }));
      await delay(); checkAccess();
      if (powerState().index !== current.index + (key === 'ArrowRight' ? 1 : -1)) throw Error('ChatGPT thinking effort did not change as requested');
    }
    throw Error('ChatGPT thinking effort selection exceeded its bound');
  }
  async function efforts(target = null, model = null) {
    if (target !== null && !Object.values(effortNames).includes(target)) throw Error(`Unsupported ChatGPT web thinking effort '${target}'`);
    await openPicker(false);
    const start = powerState();
    const choices = [];
    let selected = start.index;
    try {
      for (let index = start.min; index <= start.max; index++) {
        const current = await movePower(index);
        if (current.value) choices.push({ value: current.value, label: current.label, available: true, index });
      }
      if (target !== null) {
        const choice = choices.find(e => e.value === target);
        if (!choice) throw Error(`ChatGPT web thinking effort '${target}' is not supported by the selected model`);
        selected = choice.index;
      }
      await movePower(selected);
    } catch (error) {
      try { await movePower(start.index); } catch (restoreError) { throw Error(`${error.message}. Thinking effort restoration also failed: ${restoreError.message}`); }
      throw error;
    } finally { await closeMenu(); }
    if (target !== null && closedTriggerEffort(model) !== target) throw Error('ChatGPT did not commit the exact thinking effort');
    return choices.map(({ index, ...choice }) => choice);
  }
  async function restore(original) {
    const wasRestoring = restoring;
    restoring = true;
    try {
      if (!original?.model || !original?.effort) throw Error('ChatGPT preference restoration lacks verified original state');
      const selected = await selectModel(original.model);
      if (selected.effort !== original.effort) await efforts(original.effort, original.model);
      const current = await observation();
      if (current.model !== original.model || current.effort !== original.effort) throw Error('ChatGPT original preferences were not restored');
      return current;
    } finally { restoring = wasRestoring; }
  }
  function promptState() {
    const editor = one(editorSelector);
    const submit = one(submitSelector);
    // Literal pastes keep the exact text in marked spans. ChatGPT sends
    // those verbatim; read them directly, because innerText adds paragraph
    // breaks and autolink chrome.
    const literal = editor ? all('[data-prompt-literal-paste]', editor) : [];
    const literalText = node => {
      let text = '';
      for (const child of node.childNodes || []) {
        if (child.nodeType === 3) text += child.data;
        else if (child.nodeName === 'BR') text += '\n';
        else text += literalText(child);
      }
      return text;
    };
    const text = literal.length ? literal.map(literalText).join('')
      : editor ? (editor.children.length ? Array.from(editor.children).map(e => e.textContent || '').join('\n') : editor.innerText) : '';
    let hash = 2166136261;
    for (let index=0; index<text.length; index++) hash = Math.imul(hash ^ text.charCodeAt(index), 16777619);
    const attachments = all('button[aria-label^="Remove"]').filter(visible).length;
    return { length: text.length, hash: hash >>> 0, submitDisabled: !submit || unavailable(submit), literal: literal.length > 0, attachments };
  }
  // Typed text goes through ChatGPT's rich editor, which autolinks URLs and
  // then re-serializes the whole prompt as escaped Markdown (\#, \`, \\").
  // Paste the prompt as literal text instead. ChatGPT turns one large paste
  // into a file attachment, so paste in chunks below that threshold.
  async function insertLiteral(text) {
    if (typeof text !== 'string' || !text) throw Error('Refusing to insert an empty ChatGPT prompt');
    const editor = one(editorSelector);
    if (!editor) throw Error('ChatGPT composer is not available');
    editor.focus();
    const selection = getSelection();
    selection.removeAllRanges();
    const range = document.createRange();
    range.selectNodeContents(editor);
    selection.addRange(range);
    document.execCommand('delete', false, null);
    // The editor drops a paste dispatched in the same task as focus or
    // clearing. Let it settle first.
    await delay();
    const chunk = 6000;
    for (let start = 0; start < text.length;) {
      checkBudget();
      let end = Math.min(text.length, start + chunk);
      // Never split a UTF-16 surrogate pair across two pastes.
      if (end < text.length && /[\uD800-\uDBFF]/.test(text[end - 1])) end--;
      const data = new DataTransfer();
      data.setData('text/plain', text.slice(start, end));
      editor.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
      start = end;
    }
    await delay();
    const state = promptState();
    if (state.attachments) throw Error('ChatGPT turned the prompt into a file attachment instead of literal text');
    if (!state.literal) throw Error('ChatGPT did not accept the prompt as literal pasted text');
    return state;
  }
  function responseState() {
    const root = one('main') || document;
    const current = all('[data-markdown-text-style="assistant-message"]', root).filter(visible).at(-1);
    const legacy = all('[data-message-author-role="assistant"]', root).filter(visible).at(-1);
    const message = current || legacy;
    const markdown = current || legacy?.querySelector('.markdown') || legacy;
    const turn = message?.closest('[data-talvt-turn-state], section[data-turn="assistant"]');
    const busy = all('button', root).filter(visible).some(isStopButton);
    // data-talvt-turn-state reads "complete" while a reasoning model is still
    // thinking, so a turn is finished only once its answer action bar renders.
    const answered = !!turn?.querySelector('[data-testid="copy-turn-action-button"], button[aria-label="Copy"], button[aria-label="Rate response"], button[aria-label="Regenerate response"], button[aria-label*="Good response"], button[aria-label*="Bad response"]');
    const text = markdown?.innerText || '';
    return { text, busy, terminal: !busy && answered,
      alert: all('[role="alert"]', root).filter(visible).map(e => (e.innerText || '').trim()).filter(Boolean).at(-1) || '',
      snippet: text ? '' : ((turn || root).innerText || '').trim().slice(-300),
      model: message?.getAttribute('data-message-model-slug') || '' };
  }
  // The composer's Send button turns into a plain "Stop" button while a
  // response generates. Older markup used longer labels or a test id.
  function isStopButton(e) {
    return /^stop( (generating|streaming|response))?$/i.test((e.getAttribute('aria-label') || '').trim()) || e.getAttribute('data-testid') === 'stop-button';
  }
  // ChatGPT's built-in shell, which runs on OpenAI's remote machine. jcode
  // runs the same command locally instead (see chatgpt_web/sandbox.rs).
  function isSandboxExec(m) { return m?.role === 'assistant' && m.recipient === 'container.exec' && m.type === 'code'; }
  // Visible text written before index `end`, then the sandbox call itself.
  function sandboxCallOf(messages, end, code) {
    const before = messages.slice(0, end).filter(m => m.role === 'assistant' && m.recipient === 'all' && m.type === 'text' && m.text.trim()).map(m => m.text);
    return { tool: messages[end].recipient, code, before };
  }
  // The rendered DOM is lossy: markdown hides backslashes, and ChatGPT can
  // render wrapped markup such as <escape>...</escape> as an empty message.
  // Read the owned conversation's raw assistant text from the same-origin
  // backend instead. The session token never leaves the page.
  async function transcript(key) {
    // The stream names its conversation. A dropped stream can leave the tab
    // without /c/<id> in its URL, so fall back to the id the stream gave.
    const streamed = globalThis.__jcodeChatGptWebStream?.turns.filter(t => t.key === key).at(-1)?.conversationId;
    const id = location.pathname.match(/^\/c\/([0-9a-f-]{36})$/i)?.[1] || (/^[0-9a-f-]{36}$/i.test(streamed || '') ? streamed : null);
    if (!id) return { available: false, reason: 'the owned tab has no conversation id yet' };
    // jcode reads the unmasked transcript; the page sees the placeholder.
    const fetch = globalThis.__jcodeChatGptWebStream?.original || globalThis.fetch;
    const session = await (await fetch('/api/auth/session', { credentials: 'same-origin' })).json();
    if (!session?.accessToken) return { available: false, reason: 'no ChatGPT session token' };
    const response = await fetch(`/backend-api/conversation/${id}`, { credentials: 'same-origin', headers: { authorization: `Bearer ${session.accessToken}` } });
    if (!response.ok) return { available: false, reason: `conversation request failed with HTTP ${response.status}` };
    const data = await response.json();
    const chain = [];
    for (let node = data.current_node, guard = 0; node && data.mapping?.[node] && guard < 10_000; node = data.mapping[node].parent, guard++) chain.push(data.mapping[node].message);
    chain.reverse();
    const turn = chain.slice(chain.findLastIndex(m => m?.author?.role === 'user') + 1).filter(Boolean);
    const assistant = turn.filter(m => m.author?.role === 'assistant');
    const texts = assistant.filter(m => (m.recipient ?? 'all') === 'all' && m.content?.content_type === 'text')
      .map(m => (m.content.parts || []).filter(part => typeof part === 'string').join('')).filter(text => text.trim());
    const last = turn.at(-1);
    const flat = turn.map(m => ({ role: m.author?.role, recipient: m.recipient ?? 'all', type: m.content?.content_type, text: (m.content?.parts || []).filter(part => typeof part === 'string').join('') }));
    const callIndex = flat.findIndex(isSandboxExec);
    return { available: true, temporary: data.is_temporary_chat === true, texts,
      sandboxCall: callIndex < 0 ? null : sandboxCallOf(flat, callIndex, String(turn[callIndex].content.text ?? '')),
      complete: last?.author?.role === 'assistant' && last.end_turn === true && last.status === 'finished_successfully',
      builtin: [...new Set(assistant.map(m => m.recipient).filter(r => r && r !== 'all'))] };
  }
  // Live view of the owned turn's own network stream. The page still sends
  // the request itself (with its own sentinel checks); jcode only reads a tee
  // of the response. Decodes the v1 delta encoding into per-message state.
  function armStream(key) {
    if (typeof key !== 'string' || !key.startsWith('jcode-')) throw Error('ChatGPT stream capture lacks its owned turn key');
    const net = (globalThis.__jcodeChatGptWebStream ??= { key: null, original: null, turns: [], pending: null });
    if (!net.original) {
      net.original = globalThis.fetch;
      globalThis.fetch = async function (input, init) {
        const url = typeof input === 'string' ? input : input?.url || String(input);
        const method = (init?.method || input?.method || 'GET').toUpperCase();
        const isTurn = method === 'POST' && /\/backend-api\/f\/conversation(\?|$)/.test(url);
        const isConversationRead = method === 'GET' && /\/backend-api\/conversation\/[0-9a-f-]{36}(\?|$)/i.test(url);
        let substituted = null;
        let mask = null;
        if (isTurn && net.pending) {
          // Swap jcode's exact prompt into the page's own send, so the page
          // still mints its sentinel headers. Fail closed: an unexpected body
          // shape aborts the send rather than submitting the placeholder.
          const pending = net.pending;
          net.pending = null;
          try {
            if (typeof init?.body !== 'string') throw Error('unexpected ChatGPT send body type');
            const body = JSON.parse(init.body);
            const messages = Array.isArray(body.messages) ? body.messages : [];
            const user = messages.length === 1 ? messages[0] : null;
            if (!user || user.author?.role !== 'user' || user.content?.content_type !== 'text' || !Array.isArray(user.content.parts) || user.content.parts.length !== 1) {
              throw Error('unexpected ChatGPT send body shape');
            }
            if (typeof user.content.parts[0] !== 'string' || user.content.parts[0].trim() !== pending.placeholder.trim()) throw Error('ChatGPT send did not carry the owned placeholder');
            user.content.parts = [pending.text];
            // Pick the exact model and thinking level in the request itself.
            // The page's picker shows only part of the account's catalog.
            if (pending.route) {
              body.model = pending.route.slug;
              if (pending.route.effort) body.thinking_effort = pending.route.effort;
              else delete body.thinking_effort;
            }
            init = { ...init, body: JSON.stringify(body) };
            let hash = 2166136261;
            for (let index = 0; index < pending.text.length; index++) hash = Math.imul(hash ^ pending.text.charCodeAt(index), 16777619);
            substituted = { key: pending.key, length: pending.text.length, hash: hash >>> 0, model: body.model || '', thinkingEffort: body.thinking_effort || '' };
            // The page must never render jcode's prompt: its Markdown renderer
            // can stall for minutes on one long line, which freezes the tab.
            mask = { text: pending.text, placeholder: pending.placeholder };
            net.mask = mask;
          } catch (error) {
            // Nothing was sent. Record why, so jcode can fall back safely.
            net.sendError = { key: pending.key, message: String(error?.message || error) };
            throw error;
          }
        }
        const response = await net.original.call(this ?? globalThis, input, init);
        if (isConversationRead && net.mask && response.ok) return maskedResponse(response, await response.text(), net.mask);
        if (!isTurn || !response.body || !net.key) return response;
        const turn = { key: net.key, status: response.status, substituted, buffer: '', events: 0, messages: {}, order: [], lastPath: null, current: null, done: false, error: null, started: Date.now(), firstToken: null };
        net.turns.push(turn);
        const [mine, theirs] = response.body.tee();
        (async () => {
          const reader = mine.getReader();
          const decoder = new TextDecoder();
          for (;;) {
            const { value, done } = await reader.read();
            if (done) break;
            turn.buffer += decoder.decode(value, { stream: true });
            let end;
            while ((end = turn.buffer.indexOf('\n\n')) >= 0) {
              const block = turn.buffer.slice(0, end);
              turn.buffer = turn.buffer.slice(end + 2);
              for (const line of block.split('\n')) if (line.startsWith('data: ')) applyStreamData(turn, line.slice(6));
            }
          }
          turn.done = true;
          if (turn.status !== 200 && !turn.error) turn.error = `HTTP ${turn.status}: ${turn.buffer.slice(0, 300)}`;
        })().catch(error => {
          // The page's copy or the connection dropped (navigation, network
          // change, page abort). ChatGPT keeps generating server-side, so
          // this is not a rejection: the transcript holds the answer.
          if (!turn.done && !turn.error) { turn.interrupted = String(error?.message || error); turn.done = true; }
        });
        const pageBody = mask ? theirs.pipeThrough(maskStream(mask)) : theirs;
        return new Response(pageBody, { status: response.status, statusText: response.statusText, headers: response.headers });
      };
      net.wrapper = globalThis.fetch;
    }
    net.key = key;
    return { armed: true };
  }
  // Replaces every string equal to jcode's prompt with its placeholder.
  function maskValue(value, mask, depth = 0) {
    if (typeof value === 'string') return value === mask.text ? { value: mask.placeholder, changed: true } : { value, changed: false };
    if (!value || typeof value !== 'object' || depth > 64) return { value, changed: false };
    let changed = false;
    for (const key of Object.keys(value)) {
      const result = maskValue(value[key], mask, depth + 1);
      if (result.changed) { value[key] = result.value; changed = true; }
    }
    return { value, changed };
  }
  // A JSON text that may contain the prompt is at least as long as it.
  function maskJsonText(text, mask) {
    if (text.length < mask.text.length) return text;
    try {
      const result = maskValue(JSON.parse(text), mask);
      return result.changed ? JSON.stringify(result.value) : text;
    } catch { return text; }
  }
  function maskSseBlock(block, mask) {
    if (block.length < mask.text.length) return block;
    return block.split('\n').map(line => line.startsWith('data: ') ? `data: ${maskJsonText(line.slice(6), mask)}` : line).join('\n');
  }
  // The page's copy of the turn stream, with the echoed prompt masked.
  function maskStream(mask) {
    const decoder = new TextDecoder();
    const encoder = new TextEncoder();
    let buffer = '';
    return new TransformStream({
      transform(chunk, controller) {
        buffer += decoder.decode(chunk, { stream: true });
        let end, out = '';
        while ((end = buffer.indexOf('\n\n')) >= 0) {
          out += maskSseBlock(buffer.slice(0, end), mask) + '\n\n';
          buffer = buffer.slice(end + 2);
        }
        if (out) controller.enqueue(encoder.encode(out));
      },
      flush(controller) {
        buffer += decoder.decode();
        if (buffer) controller.enqueue(encoder.encode(maskSseBlock(buffer, mask)));
      },
    });
  }
  function maskedResponse(response, text, mask) {
    const headers = new Headers(response.headers);
    headers.delete('content-length');
    headers.delete('content-encoding');
    return new Response(maskJsonText(text, mask), { status: response.status, statusText: response.statusText, headers });
  }
  function armSend(key, text, placeholder, route) {
    if (typeof text !== 'string' || !text) throw Error('Refusing to arm an empty ChatGPT prompt');
    if (typeof placeholder !== 'string' || !placeholder.trim()) throw Error('ChatGPT send substitution lacks its placeholder');
    if (route != null && (typeof route.slug !== 'string' || !/^[a-z0-9][a-z0-9.-]{0,63}$/.test(route.slug)
      || (route.effort != null && !/^[a-z]{1,16}$/.test(route.effort)))) throw Error('ChatGPT send route is invalid');
    armStream(key);
    const net = globalThis.__jcodeChatGptWebStream;
    net.sendError = null;
    net.pending = { key, text, placeholder, route: route ?? null };
    return { armed: true, length: text.length };
  }
  function applyStreamData(turn, data) {
    if (data === '[DONE]') { turn.done = true; return; }
    let event;
    try { event = JSON.parse(data); } catch { return; }
    if (!event || typeof event !== 'object') return;
    turn.events++;
    // The server reports a failed turn as {message:null, error:"..."}.
    if (typeof event.error === 'string' && event.error && !event.type) { turn.error = event.error; return; }
    if (typeof event.conversation_id === 'string' && !turn.conversationId) turn.conversationId = event.conversation_id;
    if (event.type) {
      if (event.type === 'message_stream_complete') turn.done = true;
      if (event.error) turn.error = String(event.error);
      return;
    }
    const apply = (op, path, value) => {
      const message = turn.current;
      if (path === '' || (value && typeof value === 'object' && value.message && !path)) {
        const m = value?.message;
        if (!m?.id) return;
        if (!turn.messages[m.id]) turn.order.push(m.id);
        turn.messages[m.id] = { role: m.author?.role, recipient: m.recipient ?? 'all', channel: m.channel ?? null, type: m.content?.content_type,
          text: (m.content?.parts || []).filter(p => typeof p === 'string').join(''), status: m.status, endTurn: m.end_turn,
          code: m.content?.content_type === 'code' ? String(m.content.text ?? '') : null };
        // The server reports which model and thinking level actually ran.
        const meta = m.metadata || {};
        if (m.author?.role !== 'user') {
          const served = meta.resolved_model_slug || meta.model_slug;
          if (typeof served === 'string' && served) turn.served = served;
          if (typeof meta.thinking_effort === 'string' && meta.thinking_effort) turn.servedEffort = meta.thinking_effort;
        }
        turn.current = m.id;
        if (value.error) turn.error = String(value.error);
        return;
      }
      const entry = message && turn.messages[message];
      if (!entry || !path) return;
      turn.lastPath = path;
      if (/^\/message\/content\/parts\/0$/.test(path)) {
        entry.text = op === 'replace' ? String(value ?? '') : entry.text + String(value ?? '');
        if (entry.role === 'assistant' && entry.text && !turn.firstToken) turn.firstToken = Date.now();
      } else if (path === '/message/content/text' && entry.code != null) {
        entry.code = op === 'replace' ? String(value ?? '') : entry.code + String(value ?? '');
      } else if (path === '/message/status' && op === 'replace') entry.status = value;
      else if (path === '/message/end_turn' && op === 'replace') entry.endTurn = value;
    };
    const run = item => {
      if (!item || typeof item !== 'object') return;
      const op = item.o || 'append';
      if (op === 'patch' && Array.isArray(item.v)) { item.v.forEach(run); return; }
      apply(op, item.p ?? (typeof item.v === 'string' ? turn.lastPath : ''), item.v);
    };
    if (Array.isArray(event.v)) event.v.forEach(run);
    else if (event.o === 'patch' && Array.isArray(event.v)) event.v.forEach(run);
    else if (typeof event.v === 'string' && !event.p) apply('append', turn.lastPath, event.v);
    else run(event);
  }
  function streamState(key) {
    const net = globalThis.__jcodeChatGptWebStream;
    const turn = net?.turns.filter(t => t.key === key).at(-1);
    if (!turn) {
      // No response yet. `pending` still set means the page never sent the
      // turn POST (the click did not reach it); cleared means the POST is in
      // flight and ChatGPT has not answered yet.
      const alert = all('[role="alert"], [data-testid*="error" i]').filter(visible).map(e => (e.innerText || '').trim()).filter(Boolean).join(' | ').slice(0, 300);
      const send = one(submitSelector);
      return { started: false, sendError: net?.sendError?.key === key ? net.sendError.message : null, pending: net?.pending?.key === key,
        alert: alert || null, sendEnabled: !!send && !send.disabled, title: String(document.title || '').slice(0, 80) };
    }
    const assistant = turn.order.map(id => turn.messages[id]).filter(m => m.role === 'assistant');
    const texts = assistant.filter(m => m.recipient === 'all' && m.type === 'text' && m.text.trim()).map(m => m.text);
    const last = assistant.at(-1);
    // A sandbox call is final once its message is complete or the sandbox's
    // output (any later message) has arrived.
    const ordered = turn.order.map(id => turn.messages[id]);
    const callIndex = ordered.findIndex(isSandboxExec);
    const call = ordered[callIndex];
    const sandboxCall = call && (call.status === 'finished_successfully' || callIndex < ordered.length - 1) ? sandboxCallOf(ordered, callIndex, call.code ?? '') : null;
    return { started: true, status: turn.status, sandboxCall, done: turn.done, error: turn.error, interrupted: turn.interrupted || null, events: turn.events, texts, substituted: turn.substituted,
      served: turn.served || null, servedEffort: turn.servedEffort || null,
      complete: turn.done && !!last && last.endTurn === true && last.status === 'finished_successfully',
      builtin: [...new Set(assistant.map(m => m.recipient).filter(r => r && r !== 'all'))],
      firstTokenMs: turn.firstToken ? turn.firstToken - turn.started : null };
  }
  // The account's full model catalog, from the same endpoint the picker uses.
  // Reading it sends no message. Only non-secret catalog fields are returned.
  async function catalog() {
    const session = await (await globalThis.fetch('/api/auth/session', { credentials: 'same-origin' })).json();
    if (!session?.accessToken) throw Error('ChatGPT is signed out. Log in in the connected browser');
    const response = await globalThis.fetch('/backend-api/models?iim=false&include_icons=false&is_gizmo=false', { credentials: 'same-origin', headers: { authorization: `Bearer ${session.accessToken}` } });
    if (!response.ok) throw Error(`ChatGPT model catalog request failed with HTTP ${response.status}`);
    const data = await response.json();
    if (!Array.isArray(data?.models)) throw Error('ChatGPT model catalog has no models');
    const number = v => Number.isFinite(Number(v)) && Number(v) > 0 ? Math.floor(Number(v)) : null;
    return {
      defaultSlug: typeof data.default_model_slug === 'string' ? data.default_model_slug : null,
      versions: (data.versions || []).filter(v => v && v.enabled !== false).map(v => ({
        id: String(v.id ?? ''), title: String(v.display_text_for_intelligence || v.display_text || ''),
        presets: (v.intelligence_presets || []).filter(p => p?.preset_type === 'available').map(p => ({
          title: String(p.title || ''), slug: String(p.model_slug || ''), effort: p.thinking_effort ? String(p.thinking_effort) : null, lane: String(p.lane || '') })),
      })),
      models: data.models.map(m => ({
        slug: String(m.slug || ''), title: String(m.title || ''), reasoningType: String(m.reasoning_type || ''),
        contextTokens: number(m.max_tokens), workMode: m.is_work_mode_model === true,
        efforts: (m.thinking_efforts || []).map(t => ({ value: String(t.thinking_effort || ''), label: String(t.short_label || t.full_label || ''), description: String(t.description || '') })),
        tools: Array.isArray(m.enabled_tools) ? m.enabled_tools.map(String) : [],
        imageInput: !!m.product_features?.attachments?.image_mime_types?.length,
      })),
    };
  }

  checkOrigin();
  if (request.op === 'cancel') {
    if (typeof request.cancelKey !== 'string' || !request.cancelKey.startsWith('jcode-')) throw Error('ChatGPT cancellation lacks its owned turn key');
    (globalThis.__jcodeChatGptWebCancelled ??= {})[request.cancelKey] = true;
    await until(() => globalThis.__jcodeChatGptWebSubmissions?.[request.cancelKey]?.active !== true, 'quiescing the cancelled submission');
    return { cancelled: true, quiescent: true };
  }
  if (request.op === 'response') return responseState();
  if (request.op === 'transcript') return await transcript(request.key);
  if (request.op === 'arm_stream') return armStream(request.key);
  if (request.op === 'arm_send') return armSend(request.key, request.text, request.placeholder, request.route);
  if (request.op === 'catalog') return await catalog();
  if (request.op === 'stream') return streamState(request.key);
  if (request.op === 'stop') {
    const root = one('main');
    if (!root) throw Error('ChatGPT owned conversation is unavailable for safe Stop');
    const stop = all('button', root).filter(visible).find(isStopButton);
    if (stop) stop.click();
    await until(() => !responseState().busy, 'stopping the owned response');
    return { stopped: !!stop, busy: false };
  }
  if (request.op === 'prompt') return promptState();
  if (request.op === 'insert') { requireTemporary(true); return await insertLiteral(request.text); }
  if (request.op === 'restore') return await restore(request.original);
  if (request.op === 'verify') {
    if (request.requireTemporary !== false) requireTemporary();
    const current = await observation();
    if (current.model !== request.label || (request.effort && current.effort !== request.effort)) throw Error('ChatGPT exact model or thinking effort changed before verification');
    return current;
  }
  if (request.op === 'ready') {
    // Network-route turns choose the model in the request body, so the
    // picker is left alone. Only require an empty owned Temporary Chat.
    await until(() => temporary() || all('[role="dialog"]').filter(visible).some(e => /temporary chat/i.test(e.innerText || '')), 'loading Temporary Chat');
    await acknowledgeTemporaryNotice();
    await until(() => temporary() && !!one(editorSelector), 'loading the temporary composer');
    requireTemporary(true);
    return { ready: true, temporary: true };
  }
  if (request.op === 'click_send') {
    // Re-click Send for a turn whose first click never reached the page. Only
    // while the hook still holds this turn's prompt: once the page sends, the
    // hook clears `pending`, so a second click can never send a duplicate or
    // the bare placeholder.
    const net = globalThis.__jcodeChatGptWebStream;
    if (typeof request.armedKey !== 'string' || net?.pending?.key !== request.armedKey || globalThis.fetch !== net.wrapper) return { clicked: false, reason: 'not armed' };
    if (net.turns.some(t => t.key === request.armedKey)) return { clicked: false, reason: 'already sent' };
    const send = one(submitSelector);
    if (!send || send.disabled) return { clicked: false, reason: 'send unavailable' };
    setTimeout(() => send.click(), 0);
    return { clicked: true };
  }
  if (request.op === 'submit') {
    if (typeof request.cancelKey !== 'string' || !request.cancelKey.startsWith('jcode-')) throw Error('ChatGPT submission lacks its owned cancellation key');
    const checkCancelled = () => {
      if (globalThis.__jcodeChatGptWebCancelled?.[request.cancelKey]) throw Error('ChatGPT submission was cancelled before Send');
    };
    const pending = (globalThis.__jcodeChatGptWebSubmissions ??= {})[request.cancelKey] = { active: true };
    try {
      checkCancelled();
      requireTemporary(true);
      // Network-route turns pick the model in the request body; the picker
      // state is irrelevant and is not compared.
      const current = request.route ? { model: null, effort: null, temporary: true } : await observation();
      checkCancelled();
      if (!request.route && (current.model !== request.label || current.effort !== request.effort)) throw Error('ChatGPT exact model or effort changed before submission');
      if (request.route && request.armedKey == null) throw Error('ChatGPT route submission requires the network hook');
      requireTemporary(true);
      const prompt = promptState();
      if (prompt.submitDisabled || prompt.length !== request.length || prompt.hash !== request.hash) throw Error('ChatGPT prompt fingerprint or submit state changed before submission');
      if (request.armedKey != null) {
        // The composer only holds a placeholder. Never send it unless the
        // network hook that swaps in the real prompt is still installed.
        const net = globalThis.__jcodeChatGptWebStream;
        if (net?.pending?.key !== request.armedKey || !net.wrapper || globalThis.fetch !== net.wrapper) throw Error('ChatGPT network hook is not armed for this turn');
      }
      checkBudget(); checkCancelled();
      const send = one(submitSelector);
      if (request.armedKey != null) {
        // Clicking Send makes the page navigate to /c/<id>, which can collect
        // this evaluation's promise before DevTools gets the result. Click
        // on the next task instead and let `stream` observe the send.
        setTimeout(() => send.click(), 0);
        return { submitted: true, deferred: true, ...current };
      }
      send.click();
      return { submitted: true, ...current };
    } finally { pending.active = false; }
  }
  if (request.op === 'prepare' || request.op === 'discover' || request.op === 'probe') {
    await until(() => { checkAccess(true); return temporary() || all('[role="dialog"]').filter(visible).some(isTemporaryNotice); }, 'loading Temporary Chat');
    await acknowledgeTemporaryNotice();
    await until(() => { checkAccess(); return temporary() && !!one(editorSelector); }, 'loading the temporary composer');
    await until(() => { checkAccess(); return !!trigger() && !unavailable(trigger()); }, 'loading the available model picker');
    requireTemporary(true);
    const original = await observation();
    if (request.op === 'prepare') {
      try {
        const selected = await selectModel(request.label);
        const target = request.effort || selected.effort;
        if (!target) throw Error('ChatGPT current thinking effort is unknown');
        if (selected.effort !== target) await efforts(target, request.label);
        const current = await observation();
        requireTemporary(true);
        if (current.model !== request.label || current.effort !== target) throw Error('ChatGPT exact selection verification failed');
        return { ...current, original };
      } catch (error) {
        try { await restore(original); } catch (restoreError) { throw Error(`${error.message}. Model restoration also failed: ${restoreError.message}`); }
        throw error;
      }
    }
    try {
      if (request.op === 'probe') {
        await selectModel(request.label);
        return { reasoning_options: await efforts() };
      }
      await openPicker(true);
      const models = rows().map(e => ({ label: rowLabel(e), available: !unavailable(e), detail: (e.innerText || '').split('\n').slice(1).join(' ').trim(), reasoning_options: [] }));
      await closeMenu();
      if (!models.length || models.length > 32) throw Error('ChatGPT model discovery returned an unsupported model count');
      return { models };
    } finally { await restore(original); await closeMenu(); }
  }
  throw Error('Unsupported ChatGPT bridge UI operation');
})();
