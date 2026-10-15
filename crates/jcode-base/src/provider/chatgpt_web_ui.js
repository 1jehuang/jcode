// Shared DOM workflow for account discovery and verified temporary-chat turns.
// The caller prefixes a JSON-serialized `request`. No page text is executable.
return (async () => {
  const hidden = '[aria-hidden="true"],[inert],[data-active="false"],[hidden]';
  const visible = e => !!e && e.getClientRects().length > 0 && !e.closest(hidden);
  const all = (selector, root = document) => Array.from(root.querySelectorAll(selector));
  const one = selector => all(selector).find(visible) || null;
  // The bridge kills evaluation at 30 seconds. Stop work early enough to run
  // preference restoration and return an error before that outer deadline.
  const started = Date.now();
  let restoring = false;
  function checkBudget() {
    if (Date.now() - started >= (restoring ? 27_000 : 18_000)) throw Error('ChatGPT UI evaluation exceeded its time budget');
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
    const text = editor ? (editor.children.length ? Array.from(editor.children).map(e => e.textContent || '').join('\n') : editor.innerText) : '';
    let hash = 2166136261;
    for (let index=0; index<text.length; index++) hash = Math.imul(hash ^ text.charCodeAt(index), 16777619);
    return { length: text.length, hash: hash >>> 0, submitDisabled: !submit || unavailable(submit) };
  }
  // The current page labels the button just "Stop"; older pages said "Stop generating".
  const isStopButton = e => /^stop( generating| streaming| response)?$/i.test((e.getAttribute('aria-label') || '').trim()) || e.getAttribute('data-testid') === 'stop-button';
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

  checkOrigin();
  if (request.op === 'cancel') {
    if (typeof request.cancelKey !== 'string' || !request.cancelKey.startsWith('jcode-')) throw Error('ChatGPT cancellation lacks its owned turn key');
    (globalThis.__jcodeChatGptWebCancelled ??= {})[request.cancelKey] = true;
    await until(() => globalThis.__jcodeChatGptWebSubmissions?.[request.cancelKey]?.active !== true, 'quiescing the cancelled submission');
    return { cancelled: true, quiescent: true };
  }
  if (request.op === 'response') return responseState();
  if (request.op === 'stop') {
    const root = one('main');
    if (!root) throw Error('ChatGPT owned conversation is unavailable for safe Stop');
    const stop = all('button', root).filter(visible).find(isStopButton);
    if (stop) stop.click();
    await until(() => !responseState().busy, 'stopping the owned response');
    return { stopped: !!stop, busy: false };
  }
  if (request.op === 'prompt') return promptState();
  if (request.op === 'restore') return await restore(request.original);
  if (request.op === 'verify') {
    if (request.requireTemporary !== false) requireTemporary();
    const current = await observation();
    if (current.model !== request.label || (request.effort && current.effort !== request.effort)) throw Error('ChatGPT exact model or thinking effort changed before verification');
    return current;
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
      const current = await observation();
      checkCancelled();
      if (current.model !== request.label || current.effort !== request.effort) throw Error('ChatGPT exact model or effort changed before submission');
      requireTemporary(true);
      const prompt = promptState();
      if (prompt.submitDisabled || prompt.length !== request.length || prompt.hash !== request.hash) throw Error('ChatGPT prompt fingerprint or submit state changed before submission');
      checkBudget(); checkCancelled();
      one(submitSelector).click();
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
