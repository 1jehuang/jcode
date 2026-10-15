//! Native Safari scripting without an extension or Xcode.
//! Only dedicated single-tab windows created by this process are targetable.
//! Existing user pages are never marked or adopted. Native window identity,
//! layout, URL and a nonenumerable marker on our own page guard every action.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::OnceLock, time::Duration};

pub const BACKEND: &str = "safari_applescript";
pub const SETUP_HELP: &str = "Explicitly enable Safari Develop > Allow JavaScript from Apple Events (or Settings > Developer > Automation). Use setup to request normal macOS Automation consent, or authorize Jcode in System Settings > Privacy & Security > Automation. Jcode never changes Safari settings. Open a normal webpage before checking status. No Xcode or extension is required.";
const MARKER: &str = "__jcode_owned_safari_session";
const READY_SENTINEL: &str = "jcode-safari-ready";
fn readiness_probe_response(value: &str) -> bool {
    value == READY_SENTINEL
}

#[derive(Clone)]
struct Session {
    owner: String,
    url: String,
    token: String,
}
fn sessions() -> &'static tokio::sync::Mutex<HashMap<i64, Session>> {
    static SESSIONS: OnceLock<tokio::sync::Mutex<HashMap<i64, Session>>> = OnceLock::new();
    SESSIONS.get_or_init(Default::default)
}

pub fn capabilities() -> Value {
    json!({
        "supported":["listTabs","newSession","setActiveTab","getActiveTab","navigate","getContent","getInteractables","evaluate","click","type","fillForm","waitFor","scroll","closeTab"],
        "unsupported":["screenshot","listFrames","uploadFile"],
        "tab_identity":"Jcode-session-owned dedicated single-tab window ID plus owned-page token; layout or external URL/document drift invalidates handle; handles expire with this process",
        "existing_user_tabs":"metadata-only, cannot be adopted or selected",
        "events":"synthetic DOM events, not trusted native input",
        "frames":"top frame only", "extension_required":false,
        "selection":"validates dedicated session without bringing Safari to foreground",
        "snapshot":"DOM text and selector annotations, not an accessibility tree",
        "drift_recovery":"create newSession with the desired URL; never adopt a changed document, including same-URL reloads",
        "identity_limit":"Safari exposes no durable native tab ID; concurrent user tab replacement during explicitly requested navigation cannot be perfectly distinguished"
    })
}

fn quote(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\r', "\\r")
            .replace('\n', "\\n")
    )
}

async fn run(language: Option<&str>, source: &str, timeout_ms: u64) -> Result<String> {
    if !cfg!(target_os = "macos") {
        bail!("Safari AppleScript is available only on macOS");
    }
    let mut cmd = tokio::process::Command::new("/usr/bin/osascript");
    if let Some(language) = language {
        cmd.args(["-l", language]);
    }
    cmd.arg("-e")
        .arg(source)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(
        Duration::from_millis(timeout_ms.clamp(100, 30_000)),
        cmd.output(),
    )
    .await
    .context("Safari scripting timed out; the subprocess was killed, no settings were changed")??;
    if !output.status.success() {
        bail!(
            "Safari scripting failed: {}. {}",
            String::from_utf8_lossy(&output.stderr).trim(),
            SETUP_HELP
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn permission_script(ask: bool) -> String {
    // Explicit void-pointer binding is required for NSAppleEventDescriptor.aeDesc
    // in JXA. The inferred CoreServices struct-pointer binding is incompatible.
    format!(
        "ObjC.import('Cocoa'); ObjC.bindFunction('AEDeterminePermissionToAutomateTarget',['int',['void *','unsigned int','unsigned int','bool']]); const d=$.NSAppleEventDescriptor.descriptorWithBundleIdentifier('com.apple.Safari'); $.AEDeterminePermissionToAutomateTarget(d.aeDesc, 0x2a2a2a2a, 0x2a2a2a2a, {ask});"
    )
}
async fn automation_permission(ask: bool) -> Result<i32> {
    run(
        Some("JavaScript"),
        &permission_script(ask),
        if ask { 15_000 } else { 1500 },
    )
    .await?
    .parse()
    .context("Invalid Safari Automation preflight result")
}
fn safari(body: &str) -> String {
    format!(
        "if application \"Safari\" is not running then error \"Safari is not running\"\ntell application \"Safari\"\n{body}\nend tell"
    )
}

pub async fn status() -> Result<Value> {
    let mut result = json!({"backend":BACKEND,"browser":"safari","connected_browser":null,"ready":false,"capabilities":capabilities(),"setup_help":SETUP_HELP,"javascript_permission":"unverified"});
    if !cfg!(target_os = "macos") {
        result["reason"] = json!("macOS required");
        return Ok(result);
    }
    match automation_permission(false).await {
        Ok(code) => result["automation_permission_code"] = json!(code),
        Err(error) => {
            result["reason"] = json!(error.to_string());
            return Ok(result);
        }
    }
    if result["automation_permission_code"] != 0 {
        result["reason"] =
            json!("Automation permission not granted; status never requests permission");
        return Ok(result);
    }
    // Constant-only probe reads/mutates no user page content and creates no tabs.
    let probe = safari(&format!(
        "if (count of windows) is 0 then return \"no_document\"\nreturn do JavaScript {} in current tab of front window",
        quote(&serde_json::to_string(READY_SENTINEL)?)
    ));
    match run(None, &probe, 1500).await {
        Ok(value) if readiness_probe_response(&value) => {
            result["ready"] = json!(true);
            result["connected_browser"] = json!("safari");
            result["javascript_permission"] = json!("verified by constant nonmutating probe");
        }
        Ok(value) => result["reason"] = json!(value),
        Err(error) => result["reason"] = json!(error.to_string()),
    }
    Ok(result)
}

pub async fn setup() -> Result<Value> {
    if !cfg!(target_os = "macos") {
        return status().await;
    }
    // Only explicit setup requests normal macOS consent. Never touch Safari's
    // JavaScript setting, TCC databases, defaults, or Accessibility controls.
    match automation_permission(true).await {
        Ok(0) => status().await,
        Ok(code) => Ok(
            json!({"backend":BACKEND,"browser":"safari","ready":false,"automation_permission_code":code,"setup_help":SETUP_HELP,"capabilities":capabilities()}),
        ),
        Err(error) => Ok(
            json!({"backend":BACKEND,"browser":"safari","ready":false,"reason":error.to_string(),"setup_help":SETUP_HELP,"capabilities":capabilities()}),
        ),
    }
}

fn valid_url(value: &str) -> Result<()> {
    let url = url::Url::parse(value).context("A valid absolute URL is required")?;
    if !matches!(url.scheme(), "https" | "http" | "about" | "file") {
        bail!("Unsupported Safari navigation scheme");
    }
    if value.chars().any(char::is_control) {
        bail!("URL contains control characters");
    }
    Ok(())
}
fn validate(action: &str, params: &Value) -> Result<()> {
    if !params.is_object() {
        bail!("Safari params must be an object");
    }
    if params.get("frameId").is_some_and(|v| v != 0) || params["allFrames"] == true {
        bail!("Safari AppleScript supports only the top frame; frame targeting is unsupported");
    }
    if !capabilities()["supported"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == action)
    {
        bail!(
            "Safari AppleScript does not support '{action}' (screenshot, frames and upload require a different provider)"
        );
    }
    if let Some(url) = params.get("url").and_then(Value::as_str) {
        valid_url(url)?;
    }
    Ok(())
}

fn target_script(id: i64, session: &Session) -> String {
    let check = format!(
        "String(window[{MARKER:?}]==={})",
        serde_json::to_string(&session.token).unwrap()
    );
    format!(
        "set targetWindow to window id {id}\nif (count of tabs of targetWindow) is not 1 then error \"Safari session layout changed; refusing stale handle. Create new_tab at the desired URL\"\nset targetTab to current tab of targetWindow\nif URL of targetTab is not {} then error \"Safari session URL changed; refusing stale handle. Create new_tab at the desired URL\"\nif (do JavaScript {} in targetTab) is not \"true\" then error \"Safari session document changed; refusing stale handle, including same-URL reloads. Create new_tab at the desired URL\"",
        quote(&session.url),
        quote(&check)
    )
}
fn marker_script(token: &str) -> String {
    format!(
        "if(!window[{MARKER:?}])Object.defineProperty(window,{MARKER:?},{{value:{token},enumerable:false}});if(window[{MARKER:?}]!=={token})throw Error('Safari ownership marker collision');'ok'",
        token = serde_json::to_string(token).unwrap()
    )
}

fn cleanup_script(id: i64, token: &str) -> String {
    let check = format!(
        "String(window[{MARKER:?}]==={})",
        serde_json::to_string(token).unwrap()
    );
    safari(&format!(
        "set targetWindow to window id {id}\nif (count of tabs of targetWindow) is not 1 then return \"preserved: layout changed\"\nset targetTab to current tab of targetWindow\nif (do JavaScript {} in targetTab) is not \"true\" then return \"preserved: ownership unverified\"\nclose targetWindow\nreturn \"closed: token ownership attested\"",
        quote(&check)
    ))
}
fn create_script(bootstrap: &str, token: &str) -> String {
    // Resolve the new window by the native ID set difference, never front-window
    // index. Concurrent creations make this ambiguous and are refused.
    safari(&format!(
        "set previousIDs to id of every window\nmake new document with properties {{URL:{}}}\nset currentIDs to id of every window\nset candidates to {{}}\nrepeat with candidateID in currentIDs\nset nativeID to contents of candidateID\nif nativeID is not in previousIDs then set end of candidates to nativeID\nend repeat\nif (count of candidates) is not 1 then error \"Could not identify dedicated new Safari window safely\"\nset targetID to item 1 of candidates\nset targetWindow to window id targetID\nif (count of tabs of targetWindow) is not 1 then error \"New Safari window is not single-tab\"\nset targetTab to current tab of targetWindow\nif URL of targetTab is not {} then error \"Dedicated Safari bootstrap URL changed; refusing adoption\"\ndo JavaScript {} in targetTab\nreturn ((id of targetWindow) as text) & linefeed & (URL of targetTab)",
        quote(bootstrap),
        quote(bootstrap),
        quote(&marker_script(token))
    ))
}
const LOAD_WAIT: &str = "set lastURL to URL of targetTab\nset stableReads to 0\nrepeat 300 times\ndelay 0.1\nset thisURL to URL of targetTab\nset readyState to do JavaScript \"document.readyState\" in targetTab\nif thisURL is lastURL and readyState is \"complete\" then\nset stableReads to stableReads + 1\nelse\nset stableReads to 0\nend if\nset lastURL to thisURL\nif stableReads is greater than or equal to 5 then exit repeat\nend repeat\nif stableReads is less than 5 then error \"Safari navigation did not settle before the bounded deadline\"";

async fn create(owner: &str, params: &Value, timeout: u64) -> Result<Value> {
    let url = params
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("about:blank");
    valid_url(url)?;
    let token = uuid::Uuid::new_v4().to_string();
    let bootstrap = format!("about:blank#jcode-safari-{token}");
    let identity = run(None, &create_script(&bootstrap, &token), timeout).await.with_context(|| format!("New Safari window ownership could not be attested. A blank bootstrap window at {bootstrap} may have been left intact; no unverified window will be closed"))?;
    let (id, initial_url) = identity
        .split_once('\n')
        .context("Safari omitted new window identity")?;
    if initial_url != bootstrap {
        bail!(
            "Safari bootstrap URL changed before initialization; refusing to adopt or close the window"
        );
    }
    let id: i64 = id.parse().context("Invalid Safari window ID")?;
    let bootstrap_session = Session {
        owner: owner.into(),
        url: bootstrap,
        token: token.clone(),
    };
    let target = target_script(id, &bootstrap_session);
    let initialize = safari(&format!(
        "{target}\nset URL of targetTab to {}\ndelay 0.1\n{LOAD_WAIT}\nif (count of tabs of targetWindow) is not 1 then error \"Dedicated Safari window layout changed during navigation\"\ndo JavaScript {} in targetTab\nreturn URL of targetTab",
        quote(url),
        quote(&marker_script(&token))
    ));
    match run(None, &initialize, timeout).await {
        Ok(actual_url) => {
            sessions().lock().await.insert(
                id,
                Session {
                    owner: owner.into(),
                    url: actual_url.clone(),
                    token,
                },
            );
            Ok(json!({"tabId":id,"windowId":id,"url":actual_url}))
        }
        Err(error) => {
            // Attest token ownership, not merely URL equality. If permission
            // or identity is unavailable, leave the window intact.
            let cleanup = run(None, &cleanup_script(id, &token), 1500).await;
            bail!(
                "Failed to initialize dedicated Safari window {id}: {error}. Cleanup result: {}",
                cleanup.unwrap_or_else(|_| {
                    "ownership/permission unavailable; window left intact to protect user tabs"
                        .into()
                })
            );
        }
    }
}

/// Execute the normalized bridge command vocabulary. Requires explicit owned
/// tabId for all page actions. No stale tab-index or front-window fallback.
pub async fn request(action: &str, params: Value) -> Result<Value> {
    request_for("__jcode_safari_standalone", action, params).await
}

fn owned_session(owned: &HashMap<i64, Session>, owner: &str, id: i64) -> Result<Session> {
    let session = owned
        .get(&id)
        .context("Unknown Safari session handle; create a new dedicated tab")?;
    if session.owner != owner {
        bail!(
            "Safari tab handle belongs to another Jcode session; create a dedicated tab for this session"
        );
    }
    Ok(session.clone())
}

fn own_tabs(owned: &HashMap<i64, Session>, owner: &str, windows: &[i64]) -> Vec<Value> {
    windows
        .iter()
        .filter_map(|id| {
            owned
                .get(id)
                .filter(|s| s.owner == owner)
                .map(|s| json!({"tabId":id,"windowId":id,"url":s.url}))
        })
        .collect()
}

/// Session-scoped entry point for shared daemons. Ownership is checked before
/// permission probing, and again under the serialized operation lock.
pub async fn request_for(owner: &str, action: &str, params: Value) -> Result<Value> {
    validate(action, &params)?;
    let creating = action == "newSession" || (action == "navigate" && params["newTab"] == true);
    if !creating && action != "listTabs" {
        let id = params["tabId"]
            .as_i64()
            .context("Safari requires tab_id from new_tab; existing user tabs cannot be adopted")?;
        let owned = sessions().lock().await;
        owned_session(&owned, owner, id)?;
    }
    if automation_permission(false).await? != 0 {
        bail!(
            "Safari Automation permission is not granted. {}",
            SETUP_HELP
        );
    }
    let timeout = params
        .get("timeoutMs")
        .or_else(|| params.get("timeout"))
        .and_then(Value::as_u64)
        .unwrap_or(5000)
        .clamp(100, 30_000);
    if creating {
        return create(owner, &params, timeout).await;
    }
    let mut owned = sessions().lock().await;
    if action == "listTabs" {
        let source = safari(
            "set rows to {}\nrepeat with w in windows\nset end of rows to (id of w) as text\nend repeat\nset AppleScript's text item delimiters to \",\"\nreturn \"[\" & (rows as text) & \"]\"",
        );
        let windows: Vec<i64> = serde_json::from_str(&run(None, &source, timeout).await?)?;
        owned.retain(|id, _| windows.contains(id));
        return Ok(
            json!({"tabs":own_tabs(&owned, owner, &windows),"windows":own_tabs(&owned, owner, &windows).into_iter().map(|tab| json!({"windowId":tab["windowId"],"tabs":[tab]})).collect::<Vec<_>>(),"user_windows":windows,"note":"Only dedicated sessions owned by this Jcode session are targetable; other windows are read-only metadata"}),
        );
    }
    let id = params["tabId"]
        .as_i64()
        .context("Safari requires tab_id from new_tab; existing user tabs cannot be adopted")?;
    let session = owned_session(&owned, owner, id)?;
    if params
        .get("windowId")
        .and_then(Value::as_i64)
        .is_some_and(|w| w != id)
    {
        bail!("Safari window_id does not match session tab_id");
    }
    let target = target_script(id, &session);
    if action == "closeTab" {
        run(
            None,
            &safari(&format!("{target}\nclose targetWindow\nreturn \"closed\"")),
            timeout,
        )
        .await?;
        owned.remove(&id);
        return Ok(json!({"closed":true,"tabId":id,"windowId":id}));
    }
    if matches!(action, "getActiveTab" | "setActiveTab") {
        run(
            None,
            &safari(&format!("{target}\nreturn \"validated\"")),
            timeout,
        )
        .await?;
        return Ok(json!({"tabId":id,"windowId":id,"url":session.url,"foreground_changed":false}));
    }
    if action == "navigate" {
        let url = params["url"].as_str().context("url required")?;
        valid_url(url)?;
        let source = safari(&format!(
            "{target}\nset URL of targetTab to {}\ndelay 0.1\n{LOAD_WAIT}\nif (count of tabs of targetWindow) is not 1 then error \"Safari session layout changed during navigation; refusing ownership renewal\"\ndo JavaScript {} in targetTab\nreturn URL of targetTab",
            quote(url),
            quote(&marker_script(&session.token))
        ));
        let actual_url = run(None, &source, timeout).await?;
        owned.insert(
            id,
            Session {
                owner: session.owner,
                url: actual_url.clone(),
                token: session.token,
            },
        );
        return Ok(json!({"navigated":true,"tabId":id,"windowId":id,"url":actual_url}));
    }
    let js = action_script(action, &params)?;
    let poll = if action == "waitFor" {
        format!(
            "repeat 300 times\nif response is \"{{\\\"found\\\":true}}\" then exit repeat\ndelay 0.1\nset response to do JavaScript {} in targetTab\nend repeat\nif response is not \"{{\\\"found\\\":true}}\" then error \"Safari wait condition was not satisfied\"",
            quote(&js)
        )
    } else {
        String::new()
    };
    let navigation = if matches!(action, "click" | "type" | "evaluate") {
        // Only an explicitly requested potentially navigating action can renew
        // our marker. Ordinary reads/waits never adopt an external navigation.
        format!(
            "delay 0.1\n{LOAD_WAIT}\nif (count of tabs of targetWindow) is not 1 then error \"Safari session layout changed\"\nif URL of targetTab is not {} then do JavaScript {} in targetTab",
            quote(&session.url),
            quote(&marker_script(&session.token))
        )
    } else {
        String::new()
    };
    let identity_check = format!(
        "String(window[{MARKER:?}]==={})",
        serde_json::to_string(&session.token)?
    );
    let source = safari(&format!(
        "{target}\nset response to do JavaScript {} in targetTab\n{poll}\n{navigation}\nif (count of tabs of targetWindow) is not 1 then error \"Safari session layout changed during action; create new_tab to recover\"\nif (do JavaScript {} in targetTab) is not \"true\" then error \"Safari session document changed during action, including same-URL reload; create new_tab to recover\"\nreturn response & linefeed & (URL of targetTab)",
        quote(&js),
        quote(&identity_check)
    ));
    let value = run(None, &source, timeout).await?;
    let (response, actual_url) = value
        .rsplit_once('\n')
        .context("Safari omitted action URL receipt")?;
    let result = serde_json::from_str(response).context("Safari returned a non-JSON result")?;
    owned.insert(
        id,
        Session {
            owner: session.owner,
            url: actual_url.into(),
            token: session.token,
        },
    );
    Ok(result)
}

fn action_script(action: &str, params: &Value) -> Result<String> {
    let mut p = params.clone();
    p["action"] = json!(action);
    Ok(format!(
        "JSON.stringify((()=>{{const p={};const e=p.selector?document.querySelector(p.selector):document.activeElement;const required=()=>{{if(!e)throw Error('Target not found');return e}};const selector=(t)=>{{if(t.id)return '#'+CSS.escape(t.id);const a=[];while(t&&t.nodeType===1){{let n=1;let s=t;while((s=s.previousElementSibling))if(s.tagName===t.tagName)n++;a.unshift(t.tagName.toLowerCase()+':nth-of-type('+n+')');t=t.parentElement;}}return a.join(' > ');}};const items=()=>Array.from(document.querySelectorAll('a,button,input,select,textarea,[role=button]')).map(t=>({{selector:selector(t),tag:t.tagName,text:(t.type==='password'||/password|one-time-code/i.test(t.autocomplete||''))?'[redacted]':(t.innerText||t.value||'')}}));let result;switch(p.action){{\n{}\n}}return result;}})())",
        serde_json::to_string(&p)?,
        ACTIONS
    ))
}
const ACTIONS: &str = r#"
case 'getContent': {let content=p.format==='html'?document.documentElement.outerHTML:p.format==='title'?document.title:(document.body?.innerText||'');if(p.format==='annotated')content+='\n\n'+items().map(t=>'['+t.selector+'] '+t.tag+' '+t.text).join('\n');result={content,url:location.href,title:document.title};break;}
case 'getInteractables': result={interactables:items()};break;
case 'evaluate': result={result:(new Function(p.script))()};break;
case 'click': {let t=p.selector?e:null;if(p.text&&!p.selector)t=Array.from(document.querySelectorAll('a,button,[role=button]')).find(x=>x.innerText===p.text);if(!t&&p.x!=null&&p.y!=null)t=document.elementFromPoint(p.x,p.y);if(!t)throw Error('Click target not found');t.click();result={clicked:true};break;}
case 'type': {const t=required();t.focus();const setter=Object.getOwnPropertyDescriptor(t instanceof HTMLTextAreaElement?HTMLTextAreaElement.prototype:HTMLInputElement.prototype,'value')?.set;if(!setter)throw Error('Type requires an input or textarea');setter.call(t,(p.clear?'':t.value)+p.text);t.dispatchEvent(new Event('input',{bubbles:true}));t.dispatchEvent(new Event('change',{bubbles:true}));if(p.submit)t.form?.requestSubmit();result={typed:true};break;}
case 'fillForm': for(const f of p.fields){const t=document.querySelector(f.selector);if(!t)throw Error('Field not found: '+f.selector);if(f.checked!=null){if(!(t instanceof HTMLInputElement)||!['checkbox','radio'].includes(t.type))throw Error('checked requires checkbox or radio');t.checked=f.checked;}else {const proto=t instanceof HTMLSelectElement?HTMLSelectElement.prototype:t instanceof HTMLTextAreaElement?HTMLTextAreaElement.prototype:HTMLInputElement.prototype;const setter=Object.getOwnPropertyDescriptor(proto,'value')?.set;if(!setter)throw Error('Unsupported field');setter.call(t,f.value);if(t instanceof HTMLSelectElement&&t.value!==f.value)throw Error('Option value not found');}t.dispatchEvent(new Event('input',{bubbles:true}));t.dispatchEvent(new Event('change',{bubbles:true}));}result={filled:true};break;
case 'waitFor': {const found=(!p.selector||!!e)&&(!(p.contains||p.text)||(document.body?.innerText||'').includes(p.contains||p.text));result={found};break;}
case 'scroll': if(p.selector)required().scrollIntoView();else if(p.scrollTo)window.scrollTo(p.scrollTo.x||0,p.scrollTo.y||0);else if(p.position==='top')window.scrollTo(0,0);else if(p.position==='bottom')window.scrollTo(0,document.body.scrollHeight);else window.scrollBy(p.x||0,p.y||0);result={scrolled:true,x:scrollX,y:scrollY};break;
"#;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_uses_exact_string_not_numeric_rendering() {
        assert!(readiness_probe_response("jcode-safari-ready"));
        for value in ["1", "1.0", "no_document", "", "jcode-safari-ready-fake"] {
            assert!(!readiness_probe_response(value));
        }
    }
    #[tokio::test]
    async fn foreign_session_handles_are_rejected_before_permission_or_browser_access() {
        let id = -987654321;
        sessions().lock().await.insert(
            id,
            Session {
                owner: "owner-regression".into(),
                url: "https://example.com/".into(),
                token: "test-only".into(),
            },
        );
        for action in [
            "evaluate",
            "click",
            "navigate",
            "closeTab",
            "setActiveTab",
            "getContent",
        ] {
            let error = request_for(
                "intruder-regression",
                action,
                json!({"tabId":id,"url":"https://example.com/"}),
            )
            .await
            .unwrap_err();
            assert!(
                error.to_string().contains("another Jcode session"),
                "{action}: {error}"
            );
        }
        {
            let owned = sessions().lock().await;
            assert!(owned_session(&owned, "owner-regression", id).is_ok());
        }
        sessions().lock().await.remove(&id);
    }

    #[test]
    fn tab_lists_expose_only_callers_owned_handles() {
        let owned = HashMap::from([
            (
                1,
                Session {
                    owner: "one".into(),
                    url: "https://one.example/".into(),
                    token: "one".into(),
                },
            ),
            (
                2,
                Session {
                    owner: "two".into(),
                    url: "https://two.example/".into(),
                    token: "two".into(),
                },
            ),
        ]);
        let listed = own_tabs(&owned, "one", &[1, 2, 3]);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["tabId"], 1);
        assert!(own_tabs(&owned, "unknown", &[1, 2, 3]).is_empty());
    }
    #[test]
    fn quotes_script_literals() {
        assert_eq!(quote("a\"\\\nb"), "\"a\\\"\\\\\\nb\"");
    }
    #[test]
    fn navigation_rejects_script_and_control_inputs() {
        assert!(valid_url("javascript:alert(1)").is_err());
        assert!(valid_url("https://example.com/\n").is_err());
        assert!(valid_url("https://example.com/\"hi").is_ok());
    }
    #[tokio::test]
    async fn unsupported_actions_are_explicit() {
        assert!(
            request("screenshot", json!({}))
                .await
                .unwrap_err()
                .to_string()
                .contains("does not support")
        );
    }
    #[tokio::test]
    async fn frame_targeting_is_not_ignored() {
        assert!(
            request("click", json!({"frameId":1}))
                .await
                .unwrap_err()
                .to_string()
                .contains("top frame")
        );
    }
    #[test]
    fn status_never_requests_consent_but_setup_can() {
        assert!(permission_script(false).ends_with("false);"));
        assert!(permission_script(true).ends_with("true);"));
    }
    #[test]
    fn owned_targets_never_fall_back_to_tab_indices() {
        let source = target_script(
            123,
            &Session {
                owner: "test-owner".into(),
                url: "https://example.com/".into(),
                token: "owned-token".into(),
            },
        );
        assert!(source.contains("window id 123"));
        assert!(source.contains("layout changed"));
        assert!(source.contains("document changed"));
        assert!(source.contains("URL changed"));
        assert!(!source.contains("front window"));
        assert!(!source.contains("tab 123"));
    }
    #[test]
    fn creation_uses_new_window_set_difference() {
        let source = create_script("about:blank#test-nonce", "test-token");
        assert!(source.contains("previousIDs"));
        assert!(source.contains("count of candidates"));
        assert!(source.contains("set currentIDs to id of every window"));
        assert!(source.contains("set nativeID to contents of candidateID"));
        assert!(source.contains("set end of candidates to nativeID"));
        assert!(source.contains("set targetWindow to window id targetID"));
        assert!(!source.contains("repeat with w in windows"));
        assert!(!source.contains("set end of candidates to w"));
        assert!(!source.contains("front window"));
        let url_guard = source.find("if URL of targetTab is not").unwrap();
        let stamp = source.find("Object.defineProperty").unwrap();
        assert!(url_guard < stamp);
        assert!(source.contains("about:blank#test-nonce"));
    }
    #[test]
    fn failed_creation_cleanup_requires_token_not_url_equality() {
        let source = cleanup_script(123, "expected-token");
        assert!(source.contains("layout changed"));
        assert!(source.contains("ownership unverified"));
        assert!(source.find("do JavaScript").unwrap() < source.find("close targetWindow").unwrap());
        assert!(source.contains("expected-token"));
        assert!(!source.contains("URL of"));
    }
    #[test]
    fn dynamic_scripts_are_json_encoded() {
        let source = action_script("evaluate", &json!({"script":"return '\";\\n';"})).unwrap();
        assert!(source.contains("\\\""));
        assert!(source.contains("\\\\n"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn subprocess_timeout_is_bounded() {
        let started = std::time::Instant::now();
        let error = run(None, "delay 5", 100).await.unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn generated_javascript_parses_without_a_browser() {
        let js = action_script("evaluate", &json!({"script":"return 1;"})).unwrap();
        let source = format!(
            "new Function({}); 'ok'",
            serde_json::to_string(&js).unwrap()
        );
        assert_eq!(run(Some("JavaScript"), &source, 1500).await.unwrap(), "ok");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn control_annotations_redact_password_and_otp_values() {
        let fixture = "var CSS={escape:function(x){return x;}};var document={activeElement:null,querySelectorAll:function(){return [{id:'password',tagName:'INPUT',type:'password',autocomplete:'',value:'password-sentinel'},{id:'otp',tagName:'INPUT',type:'text',autocomplete:'one-time-code',value:'otp-sentinel'},{id:'name',tagName:'INPUT',type:'text',autocomplete:'off',value:'public-name'}];}};";
        let source = format!(
            "{fixture}{}",
            action_script("getInteractables", &json!({})).unwrap()
        );
        let output = run(Some("JavaScript"), &source, 1500).await.unwrap();
        assert!(!output.contains("password-sentinel"));
        assert!(!output.contains("otp-sentinel"));
        let output: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["interactables"][0]["text"], "[redacted]");
        assert_eq!(output["interactables"][1]["text"], "[redacted]");
        assert_eq!(output["interactables"][2]["text"], "public-name");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn generated_applescript_compiles_without_execution() {
        let dir = tempfile::tempdir().unwrap();
        let target = target_script(
            123,
            &Session {
                owner: "test-owner".into(),
                url: "https://example.com/\"quoted".into(),
                token: "token".into(),
            },
        );
        let samples = [
            create_script("about:blank#test-nonce", "test-token"),
            safari(&format!("{target}\n{LOAD_WAIT}\nreturn \"ok\"")),
            cleanup_script(123, "test-token"),
        ];
        for (index, source) in samples.iter().enumerate() {
            let path = dir.path().join(format!("{index}.applescript"));
            tokio::fs::write(&path, source).await.unwrap();
            let mut command = tokio::process::Command::new("/usr/bin/osacompile");
            command
                .arg("-o")
                .arg(dir.path().join(format!("{index}.scpt")))
                .arg(path)
                .kill_on_drop(true);
            let output = tokio::time::timeout(Duration::from_secs(3), command.output())
                .await
                .unwrap()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
