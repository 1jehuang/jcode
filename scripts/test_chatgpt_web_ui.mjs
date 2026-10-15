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
      const attrs = [...part.matchAll(/\[([^=\]]+)(?:=["']?([^\]"']*)["']?)?\]/g)];
      return attrs.length + (tag ? 1 : 0) > 0 && attrs.every(([,k,v]) => v === undefined ? this.hasAttribute(k) : this.getAttribute(k) === v);
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
  const document = { body, querySelectorAll: sel => body.querySelectorAll(sel), querySelector: sel=>body.querySelector(sel), getElementById:id=>body.querySelector(`#${id}`), addEventListener() {} };
  return { document, rows, power, thumb, trigger, main, menu, editor, submit, close, login, access, powers, selected:()=>selected, submits:()=>submits, noticeAccepts:()=>noticeAccepts, triggerClicks:()=>triggerClicks };
}

async function run(request, f, options = {}) {
  const source = readFileSync(path,'utf8');
  let now = 0;
  const globals = options.globals || {};
  return await new Function('request','document','location','KeyboardEvent','setTimeout','globalThis','Date',source)(request,f.document,{hostname:'chatgpt.com',pathname:'/'},class {constructor(type,options){this.type=type;Object.assign(this,options);}}, (cb, ms) => {now += options.elapsedPerDelay ?? ms; options.onDelay?.(globals); cb();return 0;}, globals, {now:()=>now});
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
