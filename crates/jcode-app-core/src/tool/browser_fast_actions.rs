//! Trusted DOM observations and conservative automatically enumerated browser actions.
use super::*;

// Static trusted code. Never interpolate page text, goal, selectors, or model output.
// Values are deliberately NOT read, including arbitrary text inputs and passwords.
pub(super) const OBSERVE_SCRIPT: &str = r#"return (() => {
 const clip=(s,n=180)=>String(s||'').replace(/\s+/g,' ').trim().slice(0,n);
 const unique=s=>{try{return document.querySelectorAll(s).length===1;}catch{return false;}};
 const selector=e=>{
  const parts=[];
  while(e&&e.nodeType===1){
   if(e.id){const id='#'+CSS.escape(e.id);if(unique(id)){parts.unshift(id);return parts.join(' > ');}}
   let i=1;for(let s=e.previousElementSibling;s;s=s.previousElementSibling)if(s.localName===e.localName)i++;
   parts.unshift(CSS.escape(e.localName)+':nth-of-type('+i+')');
   const path=parts.join(' > ');if(unique(path))return path;
   e=e.parentElement;
  }
  return parts.join(' > ');
 };
 const intersects=r=>r.width>0&&r.height>0&&r.bottom>0&&r.right>0&&r.top<innerHeight&&r.left<innerWidth;
 const rectVisible=(r,e)=>{
  if(!intersects(r))return false;
  for(let p=e.parentElement;p;p=p.parentElement){const s=getComputedStyle(p),b=p.getBoundingClientRect();if(/auto|scroll|hidden|clip/.test(s.overflowY)&&(r.bottom<=b.top||r.top>=b.bottom))return false;if(/auto|scroll|hidden|clip/.test(s.overflowX)&&(r.right<=b.left||r.left>=b.right))return false;}
  return true;
 };
 const visible=e=>{
  if(e.closest('[hidden],[inert],noscript,script,style,template'))return false;
  for(let p=e;p;p=p.parentElement){const s=getComputedStyle(p);if(s.visibility==='hidden'||s.visibility==='collapse'||s.display==='none'||s.opacity==='0')return false;}
  return Array.from(e.getClientRects()).some(r=>rectVisible(r,e));
 };
 const textVisible=node=>{const range=document.createRange();range.selectNodeContents(node);return Array.from(range.getClientRects()).some(r=>rectVisible(r,node));};
 const excluded='input,textarea,select,script,style,noscript,template,[contenteditable]';
 const visibleText=(root,limit)=>{
  const walker=document.createTreeWalker(root,NodeFilter.SHOW_TEXT);let text='',node,visited=0;
  while(text.length<limit&&++visited<=8000&&(node=walker.nextNode())){const p=node.parentElement;if(p&&!p.closest(excluded)&&visible(p)&&textVisible(node)){const chunk=clip(node.textContent,400);if(chunk)text+=(text?' ':'')+chunk;}}
  return text.slice(0,limit);
 };
 const identity=globalThis.__jcodeFastBrowserIdentity||(globalThis.__jcodeFastBrowserIdentity={nodes:new WeakMap(),next:1});
 // Stable for this document/realm, distinct after navigation even at the same URL.
 // Upgrade an identity object left by an earlier observer without resetting node IDs.
 if(!identity.document_id)identity.document_id=typeof globalThis.crypto?.randomUUID==='function'?globalThis.crypto.randomUUID():Array.from(globalThis.crypto.getRandomValues(new Uint32Array(4)),n=>n.toString(16).padStart(8,'0')).join('');
 const controls='a,button,input,textarea,select,[role="button"],[role="link"],[role="searchbox"],[contenteditable="true"],iframe';
 const main=document.querySelector('main,[role="main"]');
 const pool=Array.from(new Set([...(main?main.querySelectorAll(controls):[]),...document.querySelectorAll(controls)]));
 const priority=e=>(e.closest('main,[role="main"]')?4:0)+(e.matches('input,textarea,select,button,[role="button"],[role="searchbox"]')?2:0)-(e.closest('header,footer,nav,[role="navigation"],[role="banner"]')?2:0);
 pool.sort((a,b)=>priority(b)-priority(a));
 const elements=[];let sensitive=false,scanned=0;
 for(const e of pool){
  if(++scanned>4096)break;if(!visible(e))continue;
  const type=clip(e.getAttribute('type')||'',40).toLowerCase(),role=clip(e.getAttribute('role'),40);
  const aria=clip(e.getAttribute('aria-label')),name=clip(e.getAttribute('name')),placeholder=clip(e.getAttribute('placeholder'));
  const autocomplete=clip(e.getAttribute('autocomplete'),80);
  const text=e.matches(excluded)?'':visibleText(e,180);
  if(type==='password'||/one-time-code/i.test(autocomplete)||/captcha|\botp\b|verification code|security code|reset password/i.test(aria+' '+name+' '+text+' '+e.getAttribute('src')))sensitive=true;
  if(elements.length>=64)continue;
  const css=selector(e);if(css.length>1000||!unique(css))continue;
  if(!identity.nodes.has(e))identity.nodes.set(e,identity.next++);
  const searchHint=type==='search'||role==='searchbox'||/^(search|search query|search for.*|query|q)$/i.test(aria||placeholder||name);
  const form=e.form;
  const baseTarget=document.querySelector('base[target]')?.getAttribute('target')||'';
  const formTarget=form?(form.getAttribute('target')||baseTarget):baseTarget;
  const sameTarget=t=>t===''||t.toLowerCase()==='_self';
  const submitTargetsSafe=!form||Array.from(form.elements).every(b=>sameTarget(b.getAttribute('formtarget')||formTarget)&&!b.hasAttribute('formaction')&&(!b.hasAttribute('formmethod')||b.getAttribute('formmethod').toLowerCase()==='get'));
  const unsafeForm=form&&/send|buy|purchase|pay|delete|remove|confirm|order|checkout|subscribe|transfer|donate|publish|post|invite|accept|agree|approve|authorize|sign|reset|password|cart|basket|save|cancel|mail|message/i.test([form.action,...Array.from(form.querySelectorAll('button,input[type="submit"]')).map(b=>(b.getAttribute('aria-label')||'')+' '+visibleText(b,180))].join(' '));
  // Never infer search submission for POST forms or mixed editable forms.
  const search=e.localName==='input'&&['','text','search'].includes(type)&&searchHint&&!unsafeForm&&sameTarget(formTarget)&&submitTargetsSafe&&(!form||(form.method.toLowerCase()==='get'&&Array.from(form.elements).every(f=>f===e||f.disabled||['hidden','submit','button','select-one'].includes(f.type))));
  elements.push({identity:identity.nodes.get(e),selector:css,tag:e.localName,type,role,text,aria,name,placeholder,autocomplete,search,form_action:form?clip(form.action,500):'',href:e.localName==='a'?clip(e.href,1000):'',target:clip(e.getAttribute('target')||baseTarget,40),disabled:!!e.disabled||e.getAttribute('aria-disabled')==='true',form:!!form,options:e.localName==='select'?Array.from(e.options).filter(o=>o.value.length<=200).slice(0,16).map(o=>({text:clip(o.text),value:o.value,disabled:o.disabled})):[]});
 }
 const body=visibleText(main||document.body||document.documentElement,6000);
 if(/captcha|one.time (password|code)|verification code|reset (your )?password/i.test(body))sensitive=true;
 const root=document.scrollingElement||document.documentElement;
 const scrollState=e=>({can_up:e.scrollTop>0,can_down:e.scrollTop+e.clientHeight<e.scrollHeight-1,can_left:e.scrollLeft>0,can_right:e.scrollLeft+e.clientWidth<e.scrollWidth-1});
 const scroll_containers=[];scanned=0;
 for(const e of document.querySelectorAll('main,section,div,aside,ul,ol,article,[role="region"],[role="listbox"],[role="dialog"]')){
  if(++scanned>4096||scroll_containers.length>=8)break;
  if(e===root||!visible(e)||e.clientHeight<60||e.clientWidth<60)continue;
  const style=getComputedStyle(e),state=scrollState(e);
  const vertical=/(auto|scroll)/.test(style.overflowY),horizontal=/(auto|scroll)/.test(style.overflowX);
  state.can_up&&=vertical;state.can_down&&=vertical;state.can_left&&=horizontal;state.can_right&&=horizontal;
  if(!Object.values(state).some(Boolean))continue;
  const css=selector(e);if(css.length>1000||!unique(css))continue;
  if(!identity.nodes.has(e))identity.nodes.set(e,identity.next++);
  scroll_containers.push({identity:identity.nodes.get(e),selector:css,x:e.scrollLeft,y:e.scrollTop,label:clip(e.getAttribute('aria-label')||e.getAttribute('role')||e.localName,100),...state});
 }
 const result={document_id:identity.document_id,ready_state:document.readyState,scroll:{x:scrollX,y:scrollY,...scrollState(root)},url:clip(location.href,1500),title:clip(document.title,300),text:body,sensitive,elements,scroll_containers};
 // Enforce a UTF-8 JSON byte budget, including escaping and non-ASCII text.
 const bytes=()=>new TextEncoder().encode(JSON.stringify(result)).length;
 while(bytes()>30000&&elements.length)elements.pop();
 while(bytes()>30000&&scroll_containers.length)scroll_containers.pop();
 while(bytes()>30000&&result.text.length)result.text=result.text.slice(0,-256);
 return result;
})()"#;

// A conservative convenience filter, not a sandbox for arbitrary website handlers.
// Sensitive workflows should be completed by the parent with exact authorized actions.
pub(super) fn risky(text: &str) -> bool {
    let text = text.to_lowercase();
    [
        "send",
        "cart",
        "basket",
        "wishlist",
        "favorite",
        "favourite",
        "mail",
        "message",
        "reply",
        "comment",
        "buy",
        "purchase",
        "pay",
        "delete",
        "remove",
        "submit",
        "confirm",
        "order",
        "checkout",
        "subscribe",
        "unsubscribe",
        "transfer",
        "donate",
        "publish",
        "post",
        "invite",
        "accept",
        "agree",
        "approve",
        "authorize",
        "sign",
        "log out",
        "logout",
        "reset",
        "password",
        "otp",
        "captcha",
        "verification",
        "security code",
        "credit",
        "card",
        "bank",
        "billing",
        "cc-",
        "cvv",
        "cvc",
        "iban",
        "routing",
        "social security",
        "ssn",
        "transaction-",
        "save",
        "cancel",
        "disable",
        "enable",
        "install",
        "download",
        "execute",
        "run",
    ]
    .iter()
    .any(|word| text.contains(word))
}

pub(super) fn candidates(input: &BrowserInput, observation: &Value) -> Result<Vec<Candidate>> {
    let mut result = Vec::new();
    for (exact_index, exact) in input.candidates.iter().enumerate() {
        anyhow::ensure!(
            exact.label.len() <= 500 && exact.input.to_string().len() <= 16000,
            "Exact candidate too large"
        );
        result.push(Candidate {
            exact_index: Some(exact_index),
            label: exact.label.clone(),
            input: scoped(serde_json::from_value(exact.input.clone())?, input)?,
        });
    }
    let mut add = |label: String, action: Value| -> Result<()> {
        if result.len() < MAX_OPTIONS - 4 {
            result.push(Candidate {
                exact_index: None,
                label,
                input: scoped(serde_json::from_value(action)?, input)?,
            });
        }
        Ok(())
    };
    if let Some(elements) = observation["elements"].as_array() {
        for element in elements.iter().take(64) {
            let Some(selector) = element["selector"].as_str() else {
                continue;
            };
            let tag = element["tag"].as_str().unwrap_or("");
            let kind = element["type"].as_str().unwrap_or("");
            let description = format!(
                "{} {} {} {} {} {} {}",
                element["text"].as_str().unwrap_or(""),
                element["aria"].as_str().unwrap_or(""),
                element["name"].as_str().unwrap_or(""),
                element["href"].as_str().unwrap_or(""),
                element["autocomplete"].as_str().unwrap_or(""),
                element["placeholder"].as_str().unwrap_or(""),
                element["form_action"].as_str().unwrap_or("")
            );
            let label = ["aria", "text", "placeholder", "name"]
                .iter()
                .filter_map(|key| element[*key].as_str())
                .find(|text| !text.trim().is_empty())
                .unwrap_or(tag)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let label: String = label.chars().take(160).collect();
            if risky(&description) || element["disabled"] == true {
                continue;
            }
            if tag == "a"
                && matches!(element["target"].as_str().unwrap_or(""), "" | "_self")
                && element["href"]
                    .as_str()
                    .is_some_and(|href| href.starts_with("https://") || href.starts_with("http://"))
            {
                add(
                    format!("Click link {label}"),
                    json!({"action":"click","selector":selector,"url":element["href"]}),
                )?;
            }
            let button_text = label.to_lowercase();
            if (tag == "button" || element["role"] == "button")
                && element["form"] == false
                && kind != "submit"
                && matches!(
                    button_text.as_str(),
                    "next"
                        | "previous"
                        | "back"
                        | "menu"
                        | "show more"
                        | "expand"
                        | "next page"
                        | "previous page"
                        | "load more"
                        | "show less"
                        | "collapse"
                        | "close"
                        | "close menu"
                        | "open menu"
                        | "toggle menu"
                        | "navigation"
                        | "view details"
                        | "learn more"
                        | "filters"
                        | "filter"
                        | "sort"
                        | "sort by"
                        | "grid view"
                        | "list view"
                )
            {
                add(
                    format!("Click {label}"),
                    json!({"action":"click","selector":selector}),
                )?;
            }
            if matches!(tag, "input" | "textarea")
                && matches!(kind, "" | "text" | "search" | "email" | "url")
            {
                for (index, text) in input.text_values.iter().enumerate() {
                    add(
                        format!(
                            "Fill {label} with supplied text {index} WITHOUT submitting or searching"
                        ),
                        json!({"action":"type","selector":selector,"text":text,"clear":true,"submit":false}),
                    )?;
                    if tag == "input"
                        && element["search"] == true
                        && matches!(kind, "" | "text" | "search")
                    {
                        add(
                            format!(
                                "Search for supplied text {index} in {label}: type AND submit the search"
                            ),
                            json!({"action":"type","selector":selector,"text":text,"clear":true,"submit":true}),
                        )?;
                    }
                }
            }
            if tag == "select"
                && let Some(options) = element["options"].as_array()
            {
                for option in options.iter().take(16) {
                    if option["disabled"] == true
                        || risky(option["text"].as_str().unwrap_or(""))
                        || risky(option["value"].as_str().unwrap_or(""))
                    {
                        continue;
                    }
                    if let Some(value) = option["value"].as_str() {
                        add(
                            format!(
                                "Select {} in {label}",
                                option["text"].as_str().unwrap_or(value)
                            ),
                            json!({"action":"select","selector":selector,"text":value}),
                        )?;
                    }
                }
            }
        }
    }
    if let Some(containers) = observation["scroll_containers"].as_array() {
        for container in containers.iter().take(8) {
            let Some(selector) = container["selector"].as_str() else {
                continue;
            };
            let label = container["label"].as_str().unwrap_or("container");
            for (direction, axis, delta) in [
                ("down", "y", 600),
                ("up", "y", -600),
                ("right", "x", 600),
                ("left", "x", -600),
            ] {
                if container[format!("can_{direction}")] == true {
                    let mut action = json!({"action":"scroll", "selector":selector});
                    action[axis] = json!(delta);
                    add(format!("Scroll {direction} in {label}"), action)?;
                }
            }
        }
    }
    if observation["scroll"]["can_down"].as_bool().unwrap_or(true) {
        add("Scroll down".into(), json!({"action":"scroll","y":600}))?;
    }
    if observation["scroll"]["can_up"].as_bool().unwrap_or(true) {
        add("Scroll up".into(), json!({"action":"scroll","y":-600}))?;
    }
    if observation["scroll"]["can_right"] == true {
        add("Scroll right".into(), json!({"action":"scroll","x":600}))?;
    }
    if observation["scroll"]["can_left"] == true {
        add("Scroll left".into(), json!({"action":"scroll","x":-600}))?;
    }
    if observation["ready_state"] == "loading"
        || observation["elements"].as_array().is_none_or(Vec::is_empty)
    {
        add(
            "Wait for page content".into(),
            json!({"action":"wait","selector":"body","timeout_ms":1000}),
        )?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> BrowserInput {
        BrowserInput {
            action: "handoff".into(),
            tab_id: Some(7),
            text_values: vec!["caller supplied query".into()],
            ..Default::default()
        }
    }

    #[test]
    fn search_submission_requires_search_observation_and_caller_text() {
        let observation = json!({"elements":[
            {"selector":"#search", "tag":"input", "type":"search", "search":true, "aria":"Search"},
            {"selector":"#ordinary", "tag":"input", "type":"text", "search":false},
            {"selector":"#sensitive", "tag":"input", "type":"search", "search":true, "form_action":"https://example.com/checkout"}
        ]});
        let options = candidates(&input(), &observation).unwrap();
        let typed: Vec<_> = options
            .iter()
            .filter(|c| c.input.action == "type")
            .collect();
        assert_eq!(typed.len(), 3);
        assert_eq!(typed[0].input.submit, Some(false));
        assert_eq!(typed[1].input.submit, Some(true));
        assert_eq!(typed[1].input.selector.as_deref(), Some("#search"));
        assert_eq!(
            typed[1].input.text.as_deref(),
            Some("caller supplied query")
        );
        assert_eq!(typed[2].input.submit, Some(false));
        let mut no_text = input();
        no_text.text_values.clear();
        assert!(
            candidates(&no_text, &observation)
                .unwrap()
                .iter()
                .all(|c| c.input.action != "type")
        );
    }

    #[test]
    fn nested_scroll_actions_are_scoped_and_directional() {
        let observation = json!({"elements":[], "scroll":{"can_up":false,"can_down":false}, "scroll_containers":[
            {"selector":"#results", "label":"results", "can_down":true,"can_left":true,"can_up":false}
        ]});
        let options = candidates(&input(), &observation).unwrap();
        let scrolls: Vec<_> = options
            .iter()
            .filter(|c| c.input.action == "scroll")
            .collect();
        assert_eq!(scrolls.len(), 2);
        for candidate in scrolls {
            assert_eq!(candidate.input.selector.as_deref(), Some("#results"));
            assert_eq!(candidate.input.tab_id, Some(7));
            assert_eq!(candidate.input.frame_id, Some(0));
        }
    }

    #[test]
    fn navigation_buttons_do_not_enable_side_effects() {
        let elements: Vec<_> = ["Next page", "Load more", "Close menu", "Add to cart", "Add to basket", "Send email", "Publish", "Delete", "Pay", "Submit", "Confirm", "Save"]
            .iter().enumerate().map(|(i, label)| json!({"selector":format!("#b{i}"),"tag":"button","type":"button","form":false,"aria":label})).collect();
        let options = candidates(&input(), &json!({"elements":elements})).unwrap();
        let clicks: Vec<_> = options
            .iter()
            .filter(|c| c.input.action == "click")
            .collect();
        assert_eq!(clicks.len(), 3);
        assert!(risky("Add to cart"));
        assert!(risky("Add to basket"));
        assert!(risky("Reply to email"));
        assert!(!risky("Next page"));
    }

    #[test]
    fn observation_has_stable_per_document_nonce() {
        assert!(OBSERVE_SCRIPT.contains("if(!identity.document_id)identity.document_id="));
        assert!(OBSERVE_SCRIPT.contains("globalThis.crypto.randomUUID()"));
        assert!(OBSERVE_SCRIPT.contains("globalThis.crypto.getRandomValues(new Uint32Array(4))"));
        assert!(OBSERVE_SCRIPT.contains("document_id:identity.document_id"));
    }

    #[test]
    fn links_allow_self_but_not_other_browsing_contexts() {
        let elements: Vec<_> = ["", "_self", "_blank", "_parent", "_top", "named"]
            .iter().enumerate().map(|(i,target)| json!({"selector":format!("#link{i}"),"tag":"a","target":target,"href":"https://example.com/page","text":"Read more"})).collect();
        let options = candidates(&input(), &json!({"elements":elements})).unwrap();
        let clicks: Vec<_> = options
            .iter()
            .filter(|c| c.input.action == "click")
            .collect();
        assert_eq!(clicks.len(), 2);
        assert_eq!(clicks[1].input.selector.as_deref(), Some("#link1"));
    }

    #[test]
    fn form_buttons_remain_excluded_and_root_scroll_defaults_remain() {
        let options = candidates(&input(), &json!({"elements":[
            {"selector":"#next", "tag":"button", "type":"button", "form":true,"text":"Next page"}
        ]})).unwrap();
        assert!(options.iter().all(|c| c.input.action != "click"));
        assert_eq!(
            options
                .iter()
                .filter(|c| c.input.action == "scroll")
                .count(),
            2
        );
    }
}
