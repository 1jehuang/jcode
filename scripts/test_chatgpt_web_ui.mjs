import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const path = new URL('../crates/jcode-base/src/provider/chatgpt_web_ui.js', import.meta.url);

class Element {
  constructor(tag, attrs = {}, text = '', children = []) {
    this.tagName = tag.toUpperCase();
    this.attrs = { ...attrs };
    this.text = text;
    this.children = children;
    this.disabled = false;
    this.hidden = false;
    for (const child of children) child.parentElement = this;
  }
  get innerText() { return this.text || this.children.map(c => c.innerText).join('\n'); }
  get textContent() { return this.innerText; }
  get id() { return this.attrs.id || ''; }
  get dataset() { return Object.fromEntries(Object.entries(this.attrs).filter(([k]) => k.startsWith('data-')).map(([k,v]) => [k.slice(5).replace(/-([a-z])/g, (_,c) => c.toUpperCase()), v])); }
  getAttribute(k) { return this.attrs[k] ?? null; }
  setAttribute(k,v) { this.attrs[k] = String(v); }
  hasAttribute(k) { return k in this.attrs; }
  matches(selector) {
    return selector.split(',').some(part => {
      part = part.trim();
      if (part.startsWith('#')) return this.id === part.slice(1);
      const tag = part.match(/^[a-z]+/i)?.[0];
      if (tag && this.tagName !== tag.toUpperCase()) return false;
      const attrs = [...part.matchAll(/\[([^=\]^]+)(\^?=)?(?:["']?([^\]"']*)["']?)?\]/g)];
      return attrs.length + (tag ? 1 : 0) > 0 && attrs.every(([,k,op,v]) => !op ? this.hasAttribute(k) : op === '^=' ? (this.getAttribute(k) ?? '').startsWith(v) : this.getAttribute(k) === v);
    });
  }
  closest(sel) { for (let e=this; e; e=e.parentElement) if (e.matches(sel)) return e; return null; }
  querySelectorAll(sel) { return this.children.flatMap(c => [...(c.matches(sel) ? [c] : []), ...c.querySelectorAll(sel)]); }
  querySelector(sel) { return this.querySelectorAll(sel)[0] || null; }
  getClientRects() { return this.hidden ? [] : [{}]; }
  getBoundingClientRect() { return { width: this.hidden ? 0 : 100, height: this.hidden ? 0 : 30 }; }
  focus() {}
  click() { this.onClick?.(); }
  dispatchEvent(e) { this.onKey?.(e); return true; }
}

function fixture({ temporary=true, hiddenDuplicate=false, locked=false, wrongSelection=false, clickCannotClose=false, ignoreTriggerClicks=0, signedOut=false, accessDialog=false, temporaryNotice=false } = {}) {
  const labels = ['GPT-6', 'GPT-5.6 Sol', 'GPT-5.5'];
  let selected = 'GPT-6';
  const powers = {
    'GPT-6': { labels:['Instant','Medium','High','Extra High','Pro'], min:0, index:1 },
    'GPT-5.6 Sol': { labels:['Medium','High','Extra High','Pro'], min:2, index:5 },
    'GPT-5.5': { labels:['High','Extra High'], min:5, index:5 },
  };
  const current = () => powers[selected];
  // The legacy attribute aliases Extra High/Pro, independent of true Power labels.
  const aliases = {Instant:'none',Medium:'medium',High:'high','Extra High':'max',Pro:'medium'};
  const trigger = new Element('button', { 'aria-label':'Select ChatGPT model', 'data-selected-reasoning-effort':'medium' }, 'Medium');
  const renderTrigger = () => `${selected === 'GPT-6' ? '' : selected === 'GPT-5.6 Sol' ? '5.6 ' : '5.5 '}${current().labels[current().index-current().min]}`;
  const desc = new Element('span', { id:'power-status', role:'status' }, 'Medium, 2 of 5.');
  const thumb = new Element('span', { role:'slider', 'aria-valuemin':'0', 'aria-valuemax':'4', 'aria-valuenow':'1', 'aria-hidden':'true' });
  const power = new Element('div', { 'data-reasoning-slider':'true', 'aria-describedby':'power-status', role:'menuitem' }, '', [thumb]);
  const renderPower = () => {
    const {labels,min,index} = current();
    thumb.setAttribute('aria-valuemin',min);
    thumb.setAttribute('aria-valuemax',min+labels.length-1);
    thumb.setAttribute('aria-valuenow',index);
    trigger.setAttribute('data-selected-reasoning-effort',aliases[labels[index-min]]);
    trigger.text = renderTrigger();
    desc.text = `${labels[index-min]}, ${index-min+1} of ${labels.length}.`;
  };
  power.onKey = e => { const state=current(); state.index=Math.max(state.min,Math.min(state.min+state.labels.length-1,state.index+(e.key === 'ArrowRight' ? 1 : -1))); renderPower(); };
  const rows = labels.map(label => new Element('div', { role:'menuitemradio', 'aria-checked': String(label === selected), ...(locked && label === 'GPT-5.6 Sol' ? {'aria-disabled':'true', 'aria-describedby':'locked'} : {}) }, label));
  for (const row of rows) row.onClick = () => { if (!wrongSelection) { selected=row.text; for(const other of rows) other.setAttribute('aria-checked', String(other===row)); renderPower(); } close(); };
  const simple = new Element('div', { 'data-active':'true', 'aria-hidden':'false' }, '', []);
  const advanced = new Element('div', { 'data-active':'false', 'aria-hidden':'true', inert:'' }, '', rows);
  const toggle = new Element('div', { 'data-model-picker-view-toggle':'true' }, 'Select model');
  simple.children=[toggle,desc,power]; for(const e of simple.children)e.parentElement=simple;
  const menu = new Element('div', {role:'menu'}, '', [simple,advanced]); menu.hidden=true;
  const update = active => { for(const [e,on] of [[simple,active==='simple'],[advanced,active==='advanced']]) { e.setAttribute('aria-hidden', String(!on)); e.setAttribute('data-active',String(on)); if(on)delete e.attrs.inert;else e.attrs.inert=''; } };
  const close = () => { menu.hidden=true; trigger.setAttribute('aria-expanded','false'); simple.setAttribute('data-active','false');advanced.setAttribute('data-active','false'); };
  menu.onKey=e=>{ if(e.key==='Escape')close(); };
  let triggerClicks = 0;
  trigger.onClick=() => { triggerClicks++; if(triggerClicks<=ignoreTriggerClicks) return; if(menu.hidden){ menu.hidden=false;trigger.setAttribute('aria-expanded','true');update('simple'); } else if(!clickCannotClose)close(); };
  toggle.onClick=()=>update('advanced');
  const editor = new Element('div', {contenteditable:'true',role:'textbox','data-composer-markdown':''}, 'diagnostic');
  const submit = new Element('button', {type:'submit','aria-label':'Send'});
  let submits = 0; submit.onClick=()=>submits++;
  const temporaryButton = new Element('button',{'aria-label':'Turn off temporary chat'});
  temporaryButton.hidden = !temporary;
  const main = new Element('main', {}, '', [trigger,editor,submit,...(temporary||temporaryNotice?[temporaryButton]:[])]);
  const login = new Element('button',{},'Log in'); login.hidden=!signedOut;
  const access = new Element('div',{role:'dialog'},'Choose your workspace'); access.hidden=!accessDialog;
  const proceed = new Element('button',{},'Continue');
  const notice = new Element('div',{role:'dialog'},"Temporary Chat won't appear in conversation history",[proceed]); notice.hidden=!temporaryNotice;
  let noticeAccepts=0; proceed.onClick=()=>{noticeAccepts++;notice.hidden=true;temporaryButton.hidden=false;};
  const extra = hiddenDuplicate ? [new Element('div', {'aria-hidden':'true'}, '', [new Element('div',{role:'menuitemradio','aria-checked':'true'},'GPT-999')])] : [];
  const body = new Element('body', {}, '', [main,menu,new Element('span',{id:'locked'},'Locked, opens access options'),login,access,notice, ...extra]);
  const document = { body, querySelectorAll: sel => body.querySelectorAll(sel), querySelector: sel=>body.querySelector(sel), getElementById:id=>body.querySelector(`#${id}`), addEventListener() {},
    createRange: () => ({ selectNodeContents() {} }), execCommand: () => { editor.text=''; editor.children=[]; return true; } };
  return { document, rows, power, thumb, trigger, main, menu, editor, submit, close, login, access, powers, selected:()=>selected, submits:()=>submits, noticeAccepts:()=>noticeAccepts, triggerClicks:()=>triggerClicks };
}

async function run(request, f, options = {}) {
  const source = readFileSync(path,'utf8');
  let now = 0;
  const globals = options.globals || {};
  const location = { hostname:'chatgpt.com', pathname: options.pathname || '/' };
  return await new Function('request','document','location','KeyboardEvent','setTimeout','globalThis','Date','getSelection','DataTransfer','ClipboardEvent','TextDecoder','Response',source)(request,f.document,location,class {constructor(type,options){this.type=type;Object.assign(this,options);}}, (cb, ms) => {now += options.elapsedPerDelay ?? ms; options.onDelay?.(globals); cb();return 0;}, globals, {now:()=>now},
    () => ({ removeAllRanges() {}, addRange() {} }),
    class { constructor() { this.data = {}; } setData(type, value) { this.data[type] = value; } getData(type) { return this.data[type] ?? ''; } },
    class { constructor(type, options) { this.type = type; Object.assign(this, options); } },
    TextDecoder, Response);
}

function conversation(turn, { temporary = true } = {}) {
  const mapping = {};
  let parent = null;
  turn.forEach((message, index) => { const id = `n${index}`; mapping[id] = { parent, message }; parent = id; });
  return { is_temporary_chat: temporary, current_node: parent, mapping };
}
const said = (role, text, extra = {}) => ({ author: { role }, recipient: 'all', status: 'finished_successfully', end_turn: null, content: { content_type: 'text', parts: [text] }, ...extra });
function backend(data) {
  const calls = [];
  const fetch = async (url, init) => {
    calls.push({ url, init });
    if (url === '/api/auth/session') return { ok: true, status: 200, json: async () => ({ accessToken: 'token' }) };
    return { ok: true, status: 200, json: async () => data };
  };
  return { fetch, calls };
}

for (const op of ['prepare','discover','probe']) {
  for (const [state,error] of [['signedOut',/ChatGPT is signed out/],['accessDialog',/waiting for an onboarding or access choice/]]) {
    test(`${op} detects ${state} before waiting for Temporary Chat`,async()=>{
      const f=fixture({temporary:false,[state]:true}); let waits=0;
      await assert.rejects(run({op,label:'GPT-6',effort:'medium'},f,{onDelay:()=>waits++}),error);
      assert.equal(waits,0);assert.equal(f.triggerClicks(),0);assert.equal(f.submits(),0);
    });
  }
  test(`${op} still acknowledges only the temporary-chat notice`,async()=>{
    const f=fixture({temporary:false,temporaryNotice:true});
    await run({op,label:'GPT-6',effort:'medium'},f);
    assert.equal(f.noticeAccepts(),1);assert.equal(f.submits(),0);
  });
}
test('an access choice appearing during startup wait is detected immediately',async()=>{
  const f=fixture({temporary:false}); let waits=0;
  await assert.rejects(run({op:'discover'},f,{onDelay:()=>{waits++;f.access.hidden=false;}}),/waiting for an onboarding or access choice/);
  assert.equal(waits,1);assert.equal(f.submits(),0);
});
test('different model ranges restore the original model and exact Power after a switch',async()=>{
  const f=fixture();
  const prepared=await run({op:'prepare',label:'GPT-5.5',effort:'xhigh'},f);
  assert.equal(prepared.model,'GPT-5.5');assert.equal(prepared.effort,'xhigh');
  assert.equal(f.thumb.getAttribute('aria-valuemin'),'5');assert.equal(f.thumb.getAttribute('aria-valuemax'),'6');assert.equal(f.thumb.getAttribute('aria-valuenow'),'6');
  const restored=await run({op:'restore',original:prepared.original},f);
  assert.equal(restored.model,'GPT-6');assert.equal(restored.effort,'medium');
  assert.equal(f.thumb.getAttribute('aria-valuemin'),'0');assert.equal(f.thumb.getAttribute('aria-valuemax'),'4');assert.equal(f.thumb.getAttribute('aria-valuenow'),'1');
  assert.equal(f.submits(),0);
});
test('a Power offered by another model is rejected and original preferences restored',async()=>{
  const f=fixture();
  await assert.rejects(run({op:'prepare',label:'GPT-5.5',effort:'none'},f),/not supported/);
  assert.equal(f.selected(),'GPT-6');assert.equal(f.powers['GPT-6'].index,1);assert.equal(f.powers['GPT-5.5'].index,5);assert.equal(f.submits(),0);
});

test('cancellation during asynchronous submission verification cannot click Send',async()=>{
  const f=fixture();
  const prompt=await run({op:'prompt'},f);
  const globals={};
  const request={op:'submit',label:'GPT-6',effort:'medium',length:prompt.length,hash:prompt.hash,cancelKey:'jcode-test-cancel'};
  await assert.rejects(run(request,f,{globals,onDelay:g=>{g.__jcodeChatGptWebCancelled ??= {};g.__jcodeChatGptWebCancelled[request.cancelKey]=true;}}),/cancel/i);
  assert.equal(f.submits(),0);
});
test('cancellation acknowledges idle only after the pending submission finishes',async()=>{
  const f=fixture();const key='jcode-test-pending';const globals={__jcodeChatGptWebSubmissions:{[key]:{active:true}}};
  let waits=0;const result=await run({op:'cancel',cancelKey:key},f,{globals,onDelay:g=>{waits++;g.__jcodeChatGptWebSubmissions[key].active=false;}});
  assert.equal(result.quiescent,true);assert.equal(waits,1);assert.equal(globals.__jcodeChatGptWebCancelled[key],true);assert.equal(f.submits(),0);
});
test('authorized exact-model submission clicks Send once and rejects altered prompts',async()=>{
  const f=fixture();const prompt=await run({op:'prompt'},f);
  const request={op:'submit',label:'GPT-6',effort:'medium',length:prompt.length,hash:prompt.hash,cancelKey:'jcode-test-authorized'};
  const result=await run(request,f);assert.equal(result.submitted,true);assert.equal(f.submits(),1);
  const changed=fixture();changed.editor.text='changed after verification';
  await assert.rejects(run(request,changed),/fingerprint/i);assert.equal(changed.submits(),0);
});
test('evaluation deadline fails closed before delayed discovery can advertise models',async()=>{
  const f=fixture();
  await assert.rejects(run({op:'discover'},f,{elapsedPerDelay:10_000}),/deadline|time.*budget/i);
  assert.equal(f.submits(),0);
});

test('discovers actual visible models and all labeled efforts, restores preferences, and never submits', async()=>{
  const f=fixture({hiddenDuplicate:true,clickCannotClose:true});
  const result=await run({op:'discover'},f);
  assert.deepEqual(result.models.map(m=>m.label),['GPT-6','GPT-5.6 Sol','GPT-5.5']);
  const expected = {'GPT-6':['none','medium','high','xhigh','max'],'GPT-5.6 Sol':['medium','high','xhigh','max'],'GPT-5.5':['high','xhigh']};
  const positions = Object.fromEntries(Object.entries(f.powers).map(([model,power])=>[model,power.index]));
  for(const m of result.models) {
    const probe=await run({op:'probe',label:m.label},f);
    assert.deepEqual(probe.reasoning_options.map(e=>e.value),expected[m.label]);
    assert.deepEqual(Object.fromEntries(Object.entries(f.powers).map(([model,power])=>[model,power.index])),positions);
    assert.equal(f.selected(),'GPT-6');
  }
  assert.equal(f.selected(),'GPT-6'); assert.equal(f.trigger.getAttribute('data-selected-reasoning-effort'),'medium'); assert.equal(f.submits(),0); assert.equal(f.menu.hidden,true);
});
test('requires exact checked model and effort, then closes the picker',async()=>{
  const f=fixture(); const result=await run({op:'prepare',label:'GPT-5.6 Sol',effort:'xhigh'},f);
  assert.equal(result.model,'GPT-5.6 Sol');assert.equal(result.effort,'xhigh');assert.equal(result.temporary,true);assert.equal(f.submits(),0);assert.equal(f.menu.hidden,true);
});
test('a fresh page that ignores the first picker click is retried while verifiably closed',async()=>{
  const f=fixture({ignoreTriggerClicks:1}); const result=await run({op:'prepare',label:'GPT-5.6 Sol',effort:'medium'},f);
  assert.equal(result.model,'GPT-5.6 Sol');assert.equal(result.effort,'medium');assert.equal(f.submits(),0);assert.equal(f.menu.hidden,true);
});
test('a picker that never opens fails closed after a bounded number of clicks',async()=>{
  const f=fixture({ignoreTriggerClicks:1000});
  await assert.rejects(run({op:'verify',label:'GPT-6',effort:'medium'},f),/opening the model picker/);
  assert.equal(f.triggerClicks(),3);assert.equal(f.submits(),0);
});
test('persistent history is rejected before selecting or submitting',async()=>{
  const f=fixture({temporary:false}); await assert.rejects(run({op:'prepare',label:'GPT-5.6 Sol',effort:'medium'},f),/Temporary Chat/i);assert.equal(f.selected(),'GPT-6');assert.equal(f.submits(),0);
});
test('locked model never opens its access flow',async()=>{
  const f=fixture({locked:true});await assert.rejects(run({op:'prepare',label:'GPT-5.6 Sol',effort:'medium'},f),/unavailable|locked/i);assert.equal(f.selected(),'GPT-6');assert.equal(f.submits(),0);
});
test('a clicked model that did not become checked fails closed',async()=>{
  const f=fixture({wrongSelection:true});await assert.rejects(run({op:'prepare',label:'GPT-5.6 Sol',effort:'medium'},f),/select|checked|mismatch/i);assert.equal(f.submits(),0);
});
test('unknown effort never guesses a numeric slider position',async()=>{
  const f=fixture();await assert.rejects(run({op:'prepare',label:'GPT-6',effort:'low'},f),/effort|support/i);assert.equal(f.submits(),0);
});
test('stale closed trigger label cannot override independently observed slider state',async()=>{
  const f=fixture();f.power.onKey({key:'ArrowLeft'});f.trigger.setAttribute('data-selected-reasoning-effort','medium');f.trigger.text='Medium';
  await assert.rejects(run({op:'prepare',label:'GPT-6',effort:'medium'},f),/effort.*(disagree|mismatch)/i);assert.equal(f.submits(),0);
});
test('all five observed Power labels remain distinct despite the legacy attribute aliases',async()=>{
  for(const effort of ['none','medium','high','xhigh','max']) {
    const f=fixture();
    const result=await run({op:'prepare',label:'GPT-6',effort},f);
    assert.equal(result.effort,effort);
    const verified=await run({op:'verify',label:'GPT-6',effort},f);
    assert.equal(verified.effort,effort);
    const restored=await run({op:'restore',original:result.original},f);
    assert.equal(restored.effort,'medium');
    assert.equal(f.submits(),0);
  }
});
test('a closed trigger version badge must agree with the independently checked model',async()=>{
  const f=fixture();f.trigger.text='5.5 Medium';
  await assert.rejects(run({op:'verify',label:'GPT-6',effort:'medium'},f),/effort.*disagree/i);
  assert.equal(f.submits(),0);
});
test('new composer fingerprint includes UTF-16 and verifies Send button',async()=>{
  const f=fixture();f.editor.text='🙂漢';const result=await run({op:'prompt'},f);assert.equal(result.length,3);assert.equal(result.submitDisabled,false);
});
test('stop cleanup waits for the owned generation to become idle without submitting',async()=>{
  const f=fixture();const stop=new Element('button',{'aria-label':'Stop generating'});
  let stops=0;stop.onClick=()=>{stops++;stop.hidden=true;};f.main.children.push(stop);stop.parentElement=f.main;
  const result=await run({op:'stop'},f);assert.equal(result.stopped,true);assert.equal(result.busy,false);assert.equal(stops,1);assert.equal(f.submits(),0);
});
test('a thinking turn already marked complete is busy, not finished, until its answer bar renders',async()=>{
  const f=fixture(); const assistant=new Element('div',{'data-markdown-text-style':'assistant-message'},''); const turn=new Element('div',{'data-talvt-turn-state':'complete'},'',[assistant]); const stop=new Element('button',{'aria-label':'Stop'});
  f.main.children.push(turn,stop); turn.parentElement=f.main; stop.parentElement=f.main;
  let result=await run({op:'response'},f); assert.equal(result.busy,true); assert.equal(result.terminal,false);
  stop.hidden=true; result=await run({op:'response'},f); assert.equal(result.busy,false); assert.equal(result.terminal,false);
});
test('stop cleanup clicks the current plain Stop button',async()=>{
  const f=fixture();const stop=new Element('button',{'aria-label':'Stop'});
  let stops=0;stop.onClick=()=>{stops++;stop.hidden=true;};f.main.children.push(stop);stop.parentElement=f.main;
  const result=await run({op:'stop'},f);assert.equal(result.stopped,true);assert.equal(stops,1);
});
test('updated response markup returns complete assistant text, not user prompt',async()=>{
  const f=fixture(); const assistant=new Element('div',{'data-markdown-text-style':'assistant-message'},'JCODE_BRIDGE_OK'); const turn=new Element('div',{'data-talvt-turn-state':'complete'},'', [assistant,new Element('button',{'aria-label':'Copy'})]);f.main.children.push(turn);turn.parentElement=f.main;
  const result=await run({op:'response'},f);assert.equal(result.text,'JCODE_BRIDGE_OK');assert.equal(result.terminal,true);assert.equal(result.busy,false);assert.equal(result.model,'');
});
test('the composer Stop button marks generation busy and blocks completion',async()=>{
  const f=fixture(); const assistant=new Element('div',{'data-markdown-text-style':'assistant-message'},'partial'); const turn=new Element('div',{'data-talvt-turn-state':'complete'},'', [assistant]);
  const stop=new Element('button',{'aria-label':'Stop'});f.main.children.push(turn,stop);turn.parentElement=f.main;stop.parentElement=f.main;
  const result=await run({op:'response'},f);assert.equal(result.busy,true);assert.equal(result.terminal,false);
  const stopping=new Element('button',{'aria-label':'Stop dictation'});stop.hidden=true;f.main.children.push(stopping);stopping.parentElement=f.main;
  assert.equal((await run({op:'response'},f)).busy,false,'only the exact Stop label means generation');
});
test('prompts are pasted as literal text in chunks and fingerprinted from the literal spans',async()=>{
  const f=fixture(); f.editor.text=''; const pastes=[];
  f.editor.dispatchEvent=e=>{ if(e.type!=='paste')return true; const text=e.clipboardData.getData('text/plain'); pastes.push(text);
    const span=new Element('span',{'data-prompt-literal-paste':''},''); span.childNodes=[{nodeType:3,data:text}]; f.editor.children.push(span); span.parentElement=f.editor; return true; };
  const prompt='# Head\nSee https://example.com/x and \\"q\\" `t`\n'.repeat(400)+'🙂';
  const result=await run({op:'insert',text:prompt},f);
  assert.ok(pastes.length>1,'large prompts are pasted in several chunks');
  assert.ok(pastes.every(p=>p.length<=6000));assert.equal(pastes.join(''),prompt);
  assert.equal(result.literal,true);assert.equal(result.attachments,0);
  const expected=await run({op:'prompt'},f);assert.equal(expected.length,prompt.length);assert.equal(f.submits(),0);
});
test('a paste that ChatGPT turns into a file attachment fails closed',async()=>{
  const f=fixture(); f.editor.text='';
  f.editor.dispatchEvent=e=>{ if(e.type==='paste'){ const remove=new Element('button',{'aria-label':'Remove pasted text attachment'}); f.main.children.push(remove); remove.parentElement=f.main; } return true; };
  await assert.rejects(run({op:'insert',text:'x'.repeat(100)},f),/attachment/);assert.equal(f.submits(),0);
});
test('transcript returns raw assistant text of the last turn, including wrapper markup the renderer hides',async()=>{
  const f=fixture();
  const envelope='<escape><jcode_tool_call>{"name":"bash","input":{"command":"printf \'a\\\\nb\'"}}</jcode_tool_call></escape>';
  const data=conversation([said('user','old'),said('assistant','old answer',{end_turn:true}),said('user','prompt'),
    { author:{role:'assistant'}, recipient:'container.exec', status:'finished_successfully', content:{content_type:'code',text:'ls'} },
    { author:{role:'tool',name:'container.exec'}, recipient:'all', status:'finished_successfully', content:{content_type:'execution_output',text:'x'} },
    said('assistant','Checking.'), { author:{role:'assistant'}, recipient:'all', status:'finished_successfully', content:{content_type:'thoughts',thoughts:[]} },
    said('assistant',envelope,{end_turn:true})]);
  const api=backend(data);
  const result=await run({op:'transcript'},f,{pathname:'/c/6ad08967-29f4-83eb-8b81-c5ccaa17b323',globals:{fetch:api.fetch}});
  assert.deepEqual(result.texts,['Checking.',envelope]);assert.equal(result.complete,true);assert.equal(result.temporary,true);
  assert.deepEqual(result.builtin,['container.exec']);
  assert.equal(api.calls[1].url,'/backend-api/conversation/6ad08967-29f4-83eb-8b81-c5ccaa17b323');
  assert.equal(api.calls[1].init.headers.authorization,'Bearer token');assert.equal(f.submits(),0);
});
test('transcript reports an unfinished turn and an unknown conversation without guessing',async()=>{
  const f=fixture();
  const pending=backend(conversation([said('user','prompt'),said('assistant','partial',{status:'in_progress'})]));
  const result=await run({op:'transcript'},f,{pathname:'/c/6ad08967-29f4-83eb-8b81-c5ccaa17b323',globals:{fetch:pending.fetch}});
  assert.equal(result.complete,false);
  const none=await run({op:'transcript'},f,{pathname:'/',globals:{fetch:pending.fetch}});
  assert.equal(none.available,false);
});

// Network path: the page sends its own request (with its own sentinel
// headers); jcode swaps the prompt in and reads a tee of the SSE stream.
// The fixture is a scrubbed live capture of one temporary-chat turn.
const streamFixture = readFileSync(new URL('./fixtures/chatgpt_web_stream_v1.sse', import.meta.url), 'utf8');
function sseResponse(text, { chunk = 97, status = 200 } = {}) {
  const bytes = new TextEncoder().encode(text);
  return new Response(new ReadableStream({ start(c) { for (let i = 0; i < bytes.length; i += chunk) c.enqueue(bytes.slice(i, i + chunk)); c.close(); } }), { status });
}
const sendBody = text => JSON.stringify({ action: 'next', model: 'gpt-6', messages: [{ id: 'm1', author: { role: 'user' }, content: { content_type: 'text', parts: [text] } }] });
async function settle() { for (let i = 0; i < 5; i++) await new Promise(r => setImmediate(r)); }
const sseOf = events => events.map(e => `event: delta\ndata: ${JSON.stringify(e)}\n\n`).join('') + 'data: [DONE]\n\n';

test('arm_send swaps the exact prompt into the page send and decodes the v1 stream',async()=>{
  const f=fixture(); const sent=[];
  const globals={ fetch: async (url, init) => { sent.push({ url, init }); return sseResponse(streamFixture); } };
  const prompt=String.raw`# exact \"prompt\" \n with ` + '\u{1F642} and https://example.com';
  await run({op:'arm_send',key:'jcode-t1',text:prompt,placeholder:'jcode turn jcode-t1'},f,{globals});
  const response=await globals.fetch('https://chatgpt.com/backend-api/f/conversation',{method:'POST',headers:{a:'b'},body:sendBody('jcode turn jcode-t1\n')});
  await response.text(); await settle();
  assert.equal(sent.length,1); assert.deepEqual(sent[0].init.headers,{a:'b'},'page headers pass through untouched');
  assert.equal(JSON.parse(sent[0].init.body).messages[0].content.parts[0],prompt);
  const state=await run({op:'stream',key:'jcode-t1'},f,{globals});
  assert.equal(state.complete,true); assert.equal(state.error,null); assert.deepEqual(state.builtin,[]);
  assert.equal(state.substituted.length,prompt.length);
  assert.equal(state.texts.length,1);
  assert.ok(state.texts[0].startsWith('Rivers are among the most beautiful'));
  assert.ok(state.texts[0].endsWith('future generations.'));
  assert.equal(f.submits(),0);
});
test('arm_send refuses to send when the page body is not the owned placeholder',async()=>{
  const f=fixture(); let sends=0;
  const globals={ fetch: async () => { sends++; return sseResponse(streamFixture); } };
  await run({op:'arm_send',key:'jcode-t2',text:'real prompt',placeholder:'jcode turn jcode-t2'},f,{globals});
  await assert.rejects(globals.fetch('https://chatgpt.com/backend-api/f/conversation',{method:'POST',body:sendBody('something the user typed')}),/placeholder/);
  assert.equal(sends,0,'nothing reaches the network');
  const state=await run({op:'stream',key:'jcode-t2'},f,{globals});
  assert.equal(state.started,false); assert.match(state.sendError,/placeholder/);
});
test('arm_send leaves unrelated requests alone and substitutes only once',async()=>{
  const f=fixture(); const sent=[];
  const globals={ fetch: async (url, init) => { sent.push({ url, init }); return sseResponse(streamFixture); } };
  await run({op:'arm_send',key:'jcode-t3',text:'PROMPT',placeholder:'jcode turn jcode-t3'},f,{globals});
  await globals.fetch('https://chatgpt.com/backend-api/f/conversation/prepare',{method:'POST',body:'{"x":1}'});
  assert.equal(sent[0].init.body,'{"x":1}');
  await (await globals.fetch('https://chatgpt.com/backend-api/f/conversation',{method:'POST',body:sendBody('jcode turn jcode-t3')})).text();
  await (await globals.fetch('https://chatgpt.com/backend-api/f/conversation',{method:'POST',body:sendBody('jcode turn jcode-t3')})).text();
  assert.equal(JSON.parse(sent[1].init.body).messages[0].content.parts[0],'PROMPT');
  assert.equal(JSON.parse(sent[2].init.body).messages[0].content.parts[0],'jcode turn jcode-t3','a second send is never rewritten');
});
test('a placeholder is only sent while the network hook is armed, and the click is deferred',async()=>{
  const f=fixture(); const placeholder='jcode turn jcode-t6'; f.editor.text=placeholder;
  const prompt=await run({op:'prompt'},f);
  const base={op:'submit',label:'GPT-6',effort:'medium',length:prompt.length,hash:prompt.hash,armedKey:'jcode-t6'};
  await assert.rejects(run({...base,cancelKey:'jcode-t6-a'},f,{globals:{}}),/network hook is not armed/);
  assert.equal(f.submits(),0,'unarmed placeholder never sent');
  const globals={ fetch: async () => sseResponse(streamFixture) };
  await run({op:'arm_send',key:'jcode-t6',text:'REAL',placeholder},f,{globals});
  const timers=[];
  const source=readFileSync(path,'utf8');
  const result=await new Function('request','document','location','KeyboardEvent','setTimeout','globalThis','Date','getSelection','DataTransfer','ClipboardEvent','TextDecoder','Response',source)(
    {...base,cancelKey:'jcode-t6-b'},f.document,{hostname:'chatgpt.com',pathname:'/'},class{constructor(t,o){this.type=t;Object.assign(this,o);}},
    (cb,ms)=>{ if(ms===0){timers.push(cb);return 0;} cb(); return 0; },globals,{now:()=>0},()=>({}),class{},class{},TextDecoder,Response);
  assert.equal(result.deferred,true); assert.equal(f.submits(),0,'click waits for the next task');
  timers.forEach(cb=>cb()); assert.equal(f.submits(),1);
});
test('a dropped stream is reported as interrupted, not as a server rejection',async()=>{
  // "BodyStreamBuffer was aborted": the connection or the page's copy dropped
  // mid-answer. ChatGPT keeps generating, so jcode must read the transcript.
  const f=fixture(); const placeholder='jcode turn jcode-x1';
  const partial='event: delta\ndata: '+JSON.stringify({v:{message:{id:'a',author:{role:'assistant'},content:{content_type:'text',parts:['1 2 3']},status:'in_progress',recipient:'all',end_turn:null,metadata:{}}}})+'\n\n';
  const bytes=new TextEncoder().encode(partial);
  const globals={ fetch: async () => new Response(new ReadableStream({ start(c){ c.enqueue(bytes); c.error(new DOMException('BodyStreamBuffer was aborted','AbortError')); } }), { status: 200 }) };
  await run({op:'arm_send',key:'jcode-x1',text:'P',placeholder},f,{globals});
  await (await globals.fetch('https://chatgpt.com/backend-api/f/conversation',{method:'POST',body:sendBody(placeholder)})).text().catch(()=>{}); await settle();
  const state=await run({op:'stream',key:'jcode-x1'},f,{globals});
  assert.equal(state.error,null,'a dropped connection is not a rejection');
  assert.equal(state.done,true); assert.equal(state.complete,false);
  assert.match(state.interrupted,/aborted/);
});
test('a dropped stream before the page navigates still finds its conversation',async()=>{
  // Observed live: the stream dropped before the tab reached /c/<id>, so the
  // transcript had no id and jcode waited out the whole response timeout.
  const f=fixture(); const placeholder='jcode turn jcode-x2'; const id='6ad1cdfc-aadc-83ed-ab06-8d7c4c11d49d';
  const partial='data: '+JSON.stringify({type:'resume_conversation_token',conversation_id:id})+'\n\n';
  const bytes=new TextEncoder().encode(partial);
  const api=backend(conversation([said('user','P'),said('assistant','Recovered.',{end_turn:true})]));
  // The id arrives first; the connection drops a moment later.
  const globals={ fetch: async (url,init) => /f\/conversation$/.test(url) ? new Response(new ReadableStream({ start(c){ c.enqueue(bytes); setTimeout(()=>c.error(new TypeError('network error')),5); } }), { status: 200 }) : api.fetch(url,init) };
  await run({op:'arm_send',key:'jcode-x2',text:'P',placeholder},f,{globals});
  await (await globals.fetch('https://chatgpt.com/backend-api/f/conversation',{method:'POST',body:sendBody(placeholder)})).text().catch(()=>{}); await new Promise(r=>setTimeout(r,20));
  assert.match((await run({op:'stream',key:'jcode-x2'},f,{globals})).interrupted,/network error/);
  assert.equal((await run({op:'transcript'},f,{globals})).available,false,'no id without the turn key');
  const t=await run({op:'transcript',key:'jcode-x2'},f,{globals});
  assert.equal(t.available,true); assert.deepEqual(t.texts,['Recovered.']); assert.equal(t.complete,true);
  assert.equal(api.calls.at(-1).url,`/backend-api/conversation/${id}`);
});
test('click_send re-clicks only while this turn is armed and unsent',async()=>{
  const f=fixture(); const placeholder='jcode turn jcode-c1'; f.editor.text=placeholder;
  const globals={ fetch: async () => sseResponse(streamFixture) };
  assert.equal((await run({op:'click_send',armedKey:'jcode-c1'},f,{globals})).clicked,false,'never clicks when the hook is not armed');
  await run({op:'arm_send',key:'jcode-c1',text:'REAL',placeholder},f,{globals});
  const timers=[]; const source=readFileSync(path,'utf8');
  const clickOnce=()=>new Function('request','document','location','KeyboardEvent','setTimeout','globalThis','Date','getSelection','DataTransfer','ClipboardEvent','TextDecoder','Response',source)(
    {op:'click_send',armedKey:'jcode-c1'},f.document,{hostname:'chatgpt.com',pathname:'/'},class{constructor(t,o){this.type=t;Object.assign(this,o);}},
    (cb,ms)=>{ if(ms===0){timers.push(cb);return 0;} cb(); return 0; },globals,{now:()=>0},()=>({}),class{},class{},TextDecoder,Response);
  assert.equal((await clickOnce()).clicked,true); timers.splice(0).forEach(cb=>cb());
  assert.equal(f.submits(),1);
  // Once the page sends, the hook clears `pending`: no second click.
  await (await globals.fetch('https://chatgpt.com/backend-api/f/conversation',{method:'POST',body:sendBody(placeholder)})).text(); await settle();
  assert.equal((await clickOnce()).clicked,false); timers.splice(0).forEach(cb=>cb());
  assert.equal(f.submits(),1,'a sent turn is never sent again');
  assert.equal((await run({op:'click_send',armedKey:'jcode-other'},f,{globals})).clicked,false,'another turn key never clicks');
});
test('stream reports the first sandbox shell call with the text written before it',async()=>{
  // Observed live: ChatGPT runs `container.exec` in its own remote machine.
  // jcode runs the same script locally, so it needs the exact call text.
  const f=fixture(); const placeholder='jcode turn jcode-s1';
  const msg=(id,role,content,extra={})=>({v:{message:{id,author:{role,name:extra.name??null},content,status:extra.status??'finished_successfully',recipient:extra.recipient??'all',end_turn:extra.end_turn??null,metadata:{}}}});
  const events=[msg('n','assistant',{content_type:'text',parts:['Checking the disk.']}),
    msg('c','assistant',{content_type:'code',language:'unknown',text:'bash -lc df -h'},{recipient:'container.exec',status:'in_progress'})];
  const body=events.map(e=>'data: '+JSON.stringify(e)+'\n\n').join('');
  const more=['data: '+JSON.stringify({p:'/message/content/text',o:'append',v:' | head -5'})+'\n\n',
    'data: '+JSON.stringify(msg('o','tool',{content_type:'execution_output',text:'remote fs'},{name:'container.exec'}))+'\n\n'];
  let push; const globals={ fetch: async () => new Response(new ReadableStream({ start(c){ push=c; c.enqueue(new TextEncoder().encode(body)); } }), { status: 200 }) };
  await run({op:'arm_send',key:'jcode-s1',text:'P',placeholder},f,{globals});
  const reading=(await globals.fetch('https://chatgpt.com/backend-api/f/conversation',{method:'POST',body:sendBody(placeholder)})).text(); await settle();
  assert.equal((await run({op:'stream',key:'jcode-s1'},f,{globals})).sandboxCall,null,'an unfinished call is not acted on');
  push.enqueue(new TextEncoder().encode(more[0])); await settle();
  assert.equal((await run({op:'stream',key:'jcode-s1'},f,{globals})).sandboxCall,null);
  push.enqueue(new TextEncoder().encode(more[1])); await settle();
  const state=await run({op:'stream',key:'jcode-s1'},f,{globals});
  assert.deepEqual(state.sandboxCall,{tool:'container.exec',code:'bash -lc df -h | head -5',before:['Checking the disk.']});
  push.close(); await reading;
});
test('transcript reports the sandbox shell call, and python runs are not shell calls',async()=>{
  const f=fixture();
  const data=conversation([said('user','prompt'),said('assistant','Looking.'),
    { author:{role:'assistant'}, recipient:'python', status:'finished_successfully', content:{content_type:'code',text:'print(1)'} },
    { author:{role:'assistant'}, recipient:'container.exec', status:'finished_successfully', content:{content_type:'code',text:'sh -c ls'} },
    { author:{role:'tool',name:'container.exec'}, recipient:'all', status:'finished_successfully', content:{content_type:'execution_output',text:'x'} },
    said('assistant','Done.',{end_turn:true})]);
  const result=await run({op:'transcript'},f,{pathname:'/c/6ad08967-29f4-83eb-8b81-c5ccaa17b323',globals:{fetch:backend(data).fetch}});
  assert.deepEqual(result.sandboxCall,{tool:'container.exec',code:'sh -c ls',before:['Looking.']});
  const plain=conversation([said('user','prompt'),{ author:{role:'assistant'}, recipient:'python', status:'finished_successfully', content:{content_type:'code',text:'print(1)'} },said('assistant','1',{end_turn:true})]);
  assert.equal((await run({op:'transcript'},f,{pathname:'/c/6ad08967-29f4-83eb-8b81-c5ccaa17b323',globals:{fetch:backend(plain).fetch}})).sandboxCall,null);
});
test('stream reports HTTP rejections such as unusual-activity blocks',async()=>{
  const f=fixture();
  const globals={ fetch: async () => sseResponse('{"detail":"Unusual activity has been detected"}',{status:403}) };
  await run({op:'arm_send',key:'jcode-t4',text:'P',placeholder:'jcode turn jcode-t4'},f,{globals});
  await (await globals.fetch('https://chatgpt.com/backend-api/f/conversation',{method:'POST',body:sendBody('jcode turn jcode-t4')})).text(); await settle();
  const state=await run({op:'stream',key:'jcode-t4'},f,{globals});
  assert.equal(state.complete,false); assert.match(state.error,/HTTP 403.*Unusual activity/);
});
test('stream decoding handles sandbox tool messages, raw wrapper markup and patch shorthand',async()=>{
  const f=fixture();
  const envelope='<escape><jcode_tool_call>{"name":"bash","input":{"command":"printf a"}}</jcode_tool_call></escape>';
  const text=sseOf([
    {p:'',o:'add',v:{message:{id:'u',author:{role:'user'},content:{content_type:'text',parts:['q']},status:'finished_successfully',recipient:'all'}}},
    {v:{message:{id:'a1',author:{role:'assistant'},content:{content_type:'code',text:'ls'},status:'finished_successfully',recipient:'container.exec'}}},
    {v:{message:{id:'a2',author:{role:'assistant'},content:{content_type:'text',parts:[envelope.slice(0,25)]},status:'in_progress',recipient:'all',end_turn:null}}},
    {p:'/message/content/parts/0',o:'append',v:envelope.slice(25,60)},
    {v:envelope.slice(60)},
    {o:'patch',v:[{p:'/message/status',o:'replace',v:'finished_successfully'},{p:'/message/end_turn',o:'replace',v:true}]},
    {type:'message_stream_complete'}]);
  const globals={ fetch: async () => sseResponse(text,{chunk:13}) };
  await run({op:'arm_stream',key:'jcode-t5'},f,{globals});
  await (await globals.fetch('https://chatgpt.com/backend-api/f/conversation',{method:'POST',body:sendBody('x')})).text(); await settle();
  const state=await run({op:'stream',key:'jcode-t5'},f,{globals});
  assert.equal(state.complete,true); assert.deepEqual(state.builtin,['container.exec']);
  assert.deepEqual(state.texts,[envelope]);
});

// The page must never receive jcode's prompt back: its Markdown renderer
// froze the tab for minutes on a 200K-character single-line prompt.
const echoTurn = (prompt, answer) => sseOf([
  {p:'',o:'add',v:{message:{id:'u',author:{role:'user'},content:{content_type:'text',parts:[prompt]},status:'finished_successfully',recipient:'all',metadata:{}}}},
  {v:{message:{id:'a',author:{role:'assistant'},content:{content_type:'text',parts:[answer]},status:'in_progress',recipient:'all',end_turn:null,metadata:{resolved_model_slug:'gpt-6-thinking',thinking_effort:'extended'}}}},
  {o:'patch',v:[{p:'/message/status',o:'replace',v:'finished_successfully'},{p:'/message/end_turn',o:'replace',v:true}]},
  {type:'message_stream_complete'}]);
test('the page copy of the turn stream carries the placeholder, never the prompt',async()=>{
  const f=fixture(); const placeholder='jcode turn jcode-m1';
  const prompt='[{"content":[{"text":"'+'x'.repeat(5000)+'"}]}]';
  const globals={ fetch: async () => sseResponse(echoTurn(prompt,'ANSWER'),{chunk:61}) };
  await run({op:'arm_send',key:'jcode-m1',text:prompt,placeholder},f,{globals});
  const page=await (await globals.fetch('https://chatgpt.com/backend-api/f/conversation',{method:'POST',body:sendBody(placeholder)})).text(); await settle();
  assert.ok(!page.includes('x'.repeat(100)),'prompt never reaches the page renderer');
  assert.ok(page.includes(JSON.stringify(placeholder)),'page sees its own placeholder');
  assert.ok(page.includes('ANSWER'),'answer passes through');
  assert.ok(page.endsWith('data: [DONE]\n\n'),'stream framing is preserved');
  const state=await run({op:'stream',key:'jcode-m1'},f,{globals});
  assert.equal(state.complete,true); assert.deepEqual(state.texts,['ANSWER']);
  assert.equal(state.substituted.length,prompt.length,'jcode still verifies the exact prompt was sent');
});
test('conversation reads by the page are masked, while jcode reads the real transcript',async()=>{
  const f=fixture(); const placeholder='jcode turn jcode-m2'; const prompt='P'.repeat(3000);
  const data=conversation([said('user',prompt),said('assistant','DONE',{end_turn:true})]);
  const calls=[];
  const globals={ fetch: async (url, init) => {
    calls.push(url);
    if (/f\/conversation$/.test(url)) return sseResponse(echoTurn(prompt,'DONE'));
    if (url === '/api/auth/session') return new Response(JSON.stringify({accessToken:'token'}));
    return new Response(JSON.stringify(data),{headers:{'content-type':'application/json','content-length':'999'}});
  } };
  await run({op:'arm_send',key:'jcode-m2',text:prompt,placeholder},f,{globals});
  await (await globals.fetch('https://chatgpt.com/backend-api/f/conversation',{method:'POST',body:sendBody(placeholder)})).text(); await settle();
  const pageRead=await (await globals.fetch('https://chatgpt.com/backend-api/conversation/6ad08967-29f4-83eb-8b81-c5ccaa17b323')).json();
  assert.equal(pageRead.mapping.n0.message.content.parts[0],placeholder,'page history shows the placeholder');
  assert.equal(pageRead.mapping.n1.message.content.parts[0],'DONE');
  const t=await run({op:'transcript'},f,{pathname:'/c/6ad08967-29f4-83eb-8b81-c5ccaa17b323',globals});
  assert.deepEqual(t.texts,['DONE']); assert.equal(t.complete,true);
});
test('a route sets the exact model and thinking level, and the stream reports what served it',async()=>{
  const f=fixture(); const sent=[]; const placeholder='jcode turn jcode-r1';
  const globals={ fetch: async (url, init) => { sent.push(init); return sseResponse(echoTurn('Q','OK')); } };
  await run({op:'arm_send',key:'jcode-r1',text:'Q',placeholder,route:{slug:'gpt-6-thinking',effort:'extended'}},f,{globals});
  await (await globals.fetch('https://chatgpt.com/backend-api/f/conversation',{method:'POST',body:JSON.stringify({...JSON.parse(sendBody(placeholder)),thinking_effort:'standard'})})).text(); await settle();
  const body=JSON.parse(sent[0].body);
  assert.equal(body.model,'gpt-6-thinking'); assert.equal(body.thinking_effort,'extended');
  const state=await run({op:'stream',key:'jcode-r1'},f,{globals});
  assert.equal(state.served,'gpt-6-thinking'); assert.equal(state.servedEffort,'extended');
  assert.deepEqual(state.substituted.model,'gpt-6-thinking');
});
test('a route without a thinking level removes the page default, and unsafe routes are refused',async()=>{
  const f=fixture(); const sent=[]; const placeholder='jcode turn jcode-r2';
  const globals={ fetch: async (url, init) => { sent.push(init); return sseResponse(echoTurn('Q','OK')); } };
  await run({op:'arm_send',key:'jcode-r2',text:'Q',placeholder,route:{slug:'gpt-6-instant',effort:null}},f,{globals});
  await (await globals.fetch('https://chatgpt.com/backend-api/f/conversation',{method:'POST',body:JSON.stringify({...JSON.parse(sendBody(placeholder)),thinking_effort:'standard'})})).text();
  const body=JSON.parse(sent[0].body);
  assert.equal(body.model,'gpt-6-instant'); assert.equal('thinking_effort' in body,false);
  await assert.rejects(run({op:'arm_send',key:'jcode-r3',text:'Q',placeholder,route:{slug:'GPT 6; drop',effort:null}},f,{globals}),/route is invalid/);
  await assert.rejects(run({op:'arm_send',key:'jcode-r4',text:'Q',placeholder,route:{slug:'gpt-6',effort:'max!'}},f,{globals}),/route is invalid/);
});
test('catalog returns only non-secret fields of the account model list',async()=>{
  const f=fixture();
  const models={ default_model_slug:'gpt-6', versions:[{id:'latest',display_text:'GPT-6',enabled:true,intelligence_presets:[{preset_type:'available',title:'Instant',model_slug:'gpt-6-instant',lane:'instant'},{preset_type:'unavailable',model_slug:'gpt-6-pro',lane:'pro'}]},{id:'old',enabled:false,intelligence_presets:[]}],
    models:[{slug:'gpt-6-thinking',title:'GPT-6 Thinking',reasoning_type:'reasoning',max_tokens:262144,is_work_mode_model:false,secret_field:'s3cret',thinking_efforts:[{thinking_effort:'min',short_label:'Light'}],enabled_tools:['search','canvas'],product_features:{attachments:{image_mime_types:['image/png']}}}] };
  const globals={ fetch: async url => url === '/api/auth/session' ? new Response(JSON.stringify({accessToken:'token'})) : new Response(JSON.stringify(models)) };
  const c=await run({op:'catalog'},f,{globals});
  assert.equal(c.defaultSlug,'gpt-6'); assert.equal(c.versions.length,1,'disabled families are dropped');
  assert.deepEqual(c.versions[0].presets.map(p=>p.slug),['gpt-6-instant'],'unavailable presets are dropped');
  assert.deepEqual(c.models[0],{slug:'gpt-6-thinking',title:'GPT-6 Thinking',reasoningType:'reasoning',contextTokens:262144,workMode:false,efforts:[{value:'min',label:'Light',description:''}],tools:['search','canvas'],imageInput:true});
  assert.ok(!JSON.stringify(c).includes('s3cret'));
});
test('ready only requires an owned Temporary Chat and never opens the picker',async()=>{
  const f=fixture();
  assert.deepEqual(await run({op:'ready'},f),{ready:true,temporary:true});
  assert.equal(f.triggerClicks(),0); assert.equal(f.submits(),0);
  await assert.rejects(run({op:'ready'},fixture({temporary:false}),{elapsedPerDelay:1000}),/Temporary Chat|temporary/);
});
test('a route submit skips the picker but still requires the armed hook',async()=>{
  const f=fixture(); const placeholder='jcode turn jcode-r5'; f.editor.text=placeholder;
  const prompt=await run({op:'prompt'},f);
  const base={op:'submit',route:{slug:'gpt-6-instant',effort:null},length:prompt.length,hash:prompt.hash,armedKey:'jcode-r5'};
  await assert.rejects(run({...base,cancelKey:'jcode-r5-a'},f,{globals:{}}),/network hook is not armed/);
  const globals={ fetch: async () => sseResponse(streamFixture) };
  await run({op:'arm_send',key:'jcode-r5',text:'REAL',placeholder},f,{globals});
  await run({...base,cancelKey:'jcode-r5-b'},f,{globals});
  assert.equal(f.triggerClicks(),0,'picker untouched'); assert.equal(f.submits(),1);
});
