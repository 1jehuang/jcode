use super::*;

pub(super) fn bridge_request(
    action: &str,
    input: &BrowserInput,
) -> Result<(String, Value, String)> {
    let mut bridge_action = match action {
        "list_tabs" => "listTabs",
        "new_tab" => "newSession",
        "select_tab" => "setActiveTab",
        "get_active_tab" => "getActiveTab",
        "list_frames" => "listFrames",
        "open" => "navigate",
        "snapshot" => "getContent",
        "get_content" => "getContent",
        "interactables" => "getInteractables",
        "click" => "click",
        "type" => "type",
        "fill_form" => "fillForm",
        "select" => "fillForm",
        "wait" => "waitFor",
        "screenshot" => "screenshot",
        "eval" => "evaluate",
        "scroll" => "scroll",
        "upload" => "uploadFile",
        "press" => "evaluate",
        "provider_command" => input.provider_action.as_deref().ok_or_else(|| {
            anyhow::anyhow!("provider_action is required when action='provider_command'")
        })?,
        other => anyhow::bail!("Unsupported browser action: {}", other),
    }
    .to_string();

    let mut params = Map::new();
    apply_common_targeting(&mut params, input);

    match action {
        "new_tab" => {
            if let Some(url) = &input.url {
                params.insert("url".into(), json!(url));
            }
            if let Some(timeout_ms) = input.timeout_ms {
                params.insert("timeoutMs".into(), json!(timeout_ms));
            }
        }
        "select_tab" => {
            let tab_id = input
                .tab_id
                .ok_or_else(|| anyhow::anyhow!("tab_id is required for select_tab"))?;
            params.insert("tabId".into(), json!(tab_id));
            if let Some(focus) = input.focus {
                params.insert("focus".into(), json!(focus));
            }
        }
        "open" => {
            let url = input
                .url
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("url is required for open"))?;
            params.insert("url".into(), json!(url));
            params.insert("wait".into(), json!(input.wait.unwrap_or(true)));
            params.insert(
                "newTab".into(),
                json!(input.new_tab.unwrap_or(input.tab_id.is_none())),
            );
            if let Some(timeout_ms) = input.timeout_ms {
                params.insert("timeoutMs".into(), json!(timeout_ms));
            }
        }
        "snapshot" => {
            params.insert("format".into(), json!("annotated"));
        }
        "get_content" => {
            params.insert(
                "format".into(),
                json!(input.format.as_deref().unwrap_or("text")),
            );
        }
        "interactables" => {}
        "click" => {
            // Handoff must never double-activate a selected action. Preserve existing
            // direct-click event sequences for callers that rely on mouse down/up.
            if input.handoff_single_click {
                params.insert("dispatchEvents".into(), json!(false));
            }
            if input.selector.is_none()
                && input.text.is_none()
                && input.x.is_none()
                && input.y.is_none()
            {
                anyhow::bail!("click requires selector, text, or x/y coordinates");
            }
            if let Some(x) = input.x {
                params.insert("x".into(), json!(x));
            }
            if let Some(y) = input.y {
                params.insert("y".into(), json!(y));
            }
        }
        "type" => {
            let text = input
                .text
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("text is required for type"))?;
            params.insert("text".into(), json!(text));
            if let Some(clear) = input.clear {
                params.insert("clear".into(), json!(clear));
            }
            if let Some(submit) = input.submit {
                params.insert("submit".into(), json!(submit));
            }
        }
        "fill_form" => {
            let fields = input
                .fields
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("fields are required for fill_form"))?;
            let mapped: Vec<Value> = fields
                .iter()
                .map(|field| {
                    let mut obj = Map::new();
                    obj.insert("selector".into(), json!(field.selector));
                    if let Some(value) = &field.value {
                        obj.insert("value".into(), json!(value));
                    }
                    if let Some(checked) = field.checked {
                        obj.insert("checked".into(), json!(checked));
                    }
                    Value::Object(obj)
                })
                .collect();
            params.insert("fields".into(), Value::Array(mapped));
        }
        "select" => {
            let selector = input
                .selector
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("selector is required for select"))?;
            let value = input.text.as_deref().ok_or_else(|| {
                anyhow::anyhow!("text is required for select and is used as the option value")
            })?;
            params.insert(
                "fields".into(),
                json!([{ "selector": selector, "value": value }]),
            );
        }
        "wait" => {
            if input.selector.is_none() && input.text.is_none() && input.contains.is_none() {
                anyhow::bail!("wait requires selector, text, or contains");
            }
            if let Some(timeout_ms) = input.timeout_ms {
                params.insert("timeout".into(), json!(timeout_ms));
            }
            if let Some(contains) = &input.contains {
                params.insert("contains".into(), json!(contains));
            }
        }
        "screenshot" => {}
        "eval" => {
            let script = input
                .script
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("script is required for eval"))?;
            params.insert("script".into(), json!(script));
            if let Some(page_world) = input.page_world {
                params.insert("pageWorld".into(), json!(page_world));
            }
        }
        "scroll" => {
            // The bridge's selector scroll means scrollIntoView, not scrolling
            // the selected container. Implement explicit container deltas here.
            if let Some(selector) = &input.selector
                && (input.x.is_some() || input.y.is_some())
                && input.scroll_to.is_none()
                && input.position.is_none()
            {
                let selector = serde_json::to_string(selector)?;
                let x = input.x.unwrap_or(0.0);
                let y = input.y.unwrap_or(0.0);
                let behavior =
                    serde_json::to_string(input.behavior.as_deref().unwrap_or("instant"))?;
                params.insert("script".into(), json!(format!(
                    "const element=document.querySelector({selector}); if(!element) throw new Error('Scroll container not found'); element.scrollBy({{left:{x},top:{y},behavior:{behavior}}}); return {{scrolled:true,x:element.scrollLeft,y:element.scrollTop}};"
                )));
                bridge_action = "evaluate".into();
                return Ok((
                    bridge_action,
                    Value::Object(params),
                    "browser scroll".into(),
                ));
            }
            if let Some(x) = input.x {
                params.insert("x".into(), json!(x));
            }
            if let Some(y) = input.y {
                params.insert("y".into(), json!(y));
            }
            if let Some(position) = &input.position {
                params.insert("position".into(), json!(position));
            }
            if let Some(behavior) = &input.behavior {
                params.insert("behavior".into(), json!(behavior));
            }
            if let Some(scroll_to) = &input.scroll_to {
                let mut target = Map::new();
                if let Some(x) = scroll_to.x {
                    target.insert("x".into(), json!(x));
                }
                if let Some(y) = scroll_to.y {
                    target.insert("y".into(), json!(y));
                }
                params.insert("scrollTo".into(), Value::Object(target));
            }
            if !params.contains_key("x")
                && !params.contains_key("y")
                && !params.contains_key("selector")
                && !params.contains_key("position")
                && !params.contains_key("scrollTo")
            {
                anyhow::bail!("scroll requires x/y, selector, position, or scroll_to");
            }
        }
        "upload" => {
            let path = input
                .path
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("path is required for upload"))?;
            // Two readers, two keys. Through a session daemon the native host
            // reads `filePath`. Without one (Windows, or when the daemon could not
            // start) the bridge CLI encodes the file itself and requires `path`,
            // failing with "uploadFile requires 'path' param" otherwise (#1743).
            // Each reader ignores the other's key, so send both.
            params.insert("filePath".into(), json!(path));
            params.insert("path".into(), json!(path));
            if let Some(file_name) = std::path::Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
            {
                params.insert("fileName".into(), json!(file_name));
            }
        }
        "press" => {
            let script = build_press_script(input.key.as_deref(), input.selector.as_deref())?;
            params.insert("script".into(), json!(script));
            params.insert("pageWorld".into(), json!(true));
        }
        "provider_command" => {
            if let Some(raw) = &input.params {
                return Ok((bridge_action, raw.clone(), format!("browser {}", action)));
            }
        }
        _ => {}
    }

    Ok((
        bridge_action,
        Value::Object(params),
        format!("browser {}", action),
    ))
}

fn apply_common_targeting(params: &mut Map<String, Value>, input: &BrowserInput) {
    if let Some(tab_id) = input.tab_id {
        params.insert("tabId".into(), json!(tab_id));
    }
    if let Some(window_id) = input.window_id {
        params.insert("windowId".into(), json!(window_id));
    }
    if let Some(frame_id) = input.frame_id {
        params.insert("frameId".into(), json!(frame_id));
    }
    if let Some(all_frames) = input.all_frames {
        params.insert("allFrames".into(), json!(all_frames));
    }
    if let Some(selector) = &input.selector {
        params.insert("selector".into(), json!(selector));
    }
    if let Some(text) = &input.text {
        params.insert("text".into(), json!(text));
    }
}

pub(super) fn build_press_script(key: Option<&str>, selector: Option<&str>) -> Result<String> {
    let key = key.ok_or_else(|| anyhow::anyhow!("key is required for press"))?;
    let selector_literal = selector.map(serde_json::to_string).transpose()?;
    let selector_expr = selector_literal
        .map(|s| format!("document.querySelector({})", s))
        .unwrap_or_else(|| "null".to_string());
    let key_literal = serde_json::to_string(key)?;
    Ok(format!(
        r#"return (() => {{
  const target = {selector_expr} || document.activeElement || document.body;
  if (!target) throw new Error('No target available for key press');
  if (typeof target.focus === 'function') target.focus();
  const key = {key_literal};
  const eventInit = {{ key, bubbles: true, cancelable: true }};
  target.dispatchEvent(new KeyboardEvent('keydown', eventInit));
  target.dispatchEvent(new KeyboardEvent('keypress', eventInit));
  if (key === 'Enter' && target.form && typeof target.form.submit === 'function') {{
    target.form.submit();
  }}
  target.dispatchEvent(new KeyboardEvent('keyup', eventInit));
  return {{ pressed: true, key, tag: target.tagName || null }};
}})();"#
    ))
}
