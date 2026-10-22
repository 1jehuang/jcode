#![cfg_attr(test, allow(clippy::await_holding_lock))]
use super::*;

#[test]
fn open_defaults_to_new_tab_unless_targeted_or_explicitly_reused() {
    for (tab_id, new_tab, expected) in [
        (None, None, true),
        (Some(7), None, false),
        (None, Some(false), false),
        (Some(7), Some(true), true),
    ] {
        let input = BrowserInput {
            action: "open".into(),
            url: Some("https://example.com".into()),
            tab_id,
            new_tab,
            ..Default::default()
        };
        let (_, params, _) = bridge_request("open", &input).unwrap();
        assert_eq!(params["newTab"], expected);
    }
}

#[test]
fn fresh_handoff_replaces_stale_tab_window_and_frame() {
    let mut input = BrowserInput {
        tab_id: Some(7),
        window_id: Some(8),
        frame_id: Some(9),
        all_frames: Some(true),
        new_tab: Some(true),
        ..Default::default()
    };
    bind_created_handoff_tab(&mut input, Some(&json!({"tabId": 42, "windowId": 43}))).unwrap();
    assert_eq!(input.tab_id, Some(42));
    assert_eq!(input.window_id, Some(43));
    assert_eq!(input.frame_id, Some(0));
    assert_eq!(input.all_frames, Some(false));
    assert_eq!(input.new_tab, Some(false));
}

#[test]
fn fresh_handoff_never_falls_back_to_old_tab_on_invalid_response() {
    for result in [None, Some(json!({})), Some(json!({"tabId": "42"}))] {
        let mut input = BrowserInput {
            tab_id: Some(7),
            ..Default::default()
        };
        assert!(bind_created_handoff_tab(&mut input, result.as_ref()).is_err());
    }
}

struct FreshTabProvider;

#[async_trait]
impl BrowserProvider for FreshTabProvider {
    fn id(&self) -> &'static str {
        "test"
    }
    fn supported_browsers(&self) -> &'static [&'static str] {
        &["auto"]
    }
    async fn status(&self, _: &ToolContext) -> Result<ToolOutput> {
        unreachable!()
    }
    async fn setup(&self) -> Result<ToolOutput> {
        unreachable!()
    }
    async fn ensure_ready(&self) -> Result<Option<String>> {
        Ok(None)
    }
    async fn execute(
        &self,
        action: &str,
        input: &BrowserInput,
        _: &ToolContext,
    ) -> Result<ToolOutput> {
        assert_eq!(action, "new_tab");
        assert_eq!(input.url.as_deref(), Some("https://example.com"));
        assert_eq!(input.tab_id, None);
        assert_eq!(input.frame_id, None);
        Ok(ToolOutput::new("created").with_metadata(json!({"tabId": 42, "windowId": 43})))
    }
}

#[tokio::test]
async fn handoff_creates_and_binds_fresh_tab_in_one_call() {
    let ctx = ToolContext {
        session_id: "fresh-tab-test".into(),
        message_id: "m".into(),
        tool_call_id: "t".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        decision_request_tx: None,
        execution_mode: crate::tool::ToolExecutionMode::Direct,
    };
    for (tab_id, new_tab, expected) in [
        (Some(7), Some(true), 42),
        (None, None, 42),
        (Some(7), None, 7),
    ] {
        let mut input = BrowserInput {
            action: "handoff".into(),
            goal: Some("Read the page".into()),
            url: Some("https://example.com".into()),
            tab_id,
            new_tab,
            frame_id: Some(9),
            ..Default::default()
        };
        prepare_handoff_tab(&FreshTabProvider, &mut input, &ctx)
            .await
            .unwrap();
        assert_eq!(input.tab_id, Some(expected));
    }
    let mut invalid = BrowserInput {
        new_tab: Some(true),
        ..Default::default()
    };
    assert!(
        prepare_handoff_tab(&FreshTabProvider, &mut invalid, &ctx)
            .await
            .is_err()
    );
    assert!(
        recover_slow_navigation(
            &json!({"newTab": true, "tabId": 7, "url": "https://example.com"}),
            None,
            &ctx,
        )
        .await
        .is_none()
    );
}

#[test]
fn best_effort_bridge_diagnostics_never_persist_page_payloads() {
    let secret = "https://private.example/?token=secret-token page text and script contents";
    let bridge = anyhow::anyhow!(secret.to_string()).context("getContent failed");
    let transport = anyhow::Error::new(std::io::Error::other(secret))
        .context(format!("getActiveTab failed: {secret}"));
    let json = anyhow::Error::new(
        serde_json::from_str::<Value>("not json with secret-token").unwrap_err(),
    )
    .context(secret);
    for error in [bridge, transport, json] {
        let diagnostic = browser_error_diagnostic(&error);
        assert!(!diagnostic.contains("secret-token"), "{diagnostic}");
        assert!(!diagnostic.contains("private.example"), "{diagnostic}");
        assert!(!diagnostic.contains("page text"), "{diagnostic}");
        assert!(!diagnostic.contains("script contents"), "{diagnostic}");
        assert!(diagnostic.contains("failure") || diagnostic.contains("failed"));
    }
}

#[cfg(unix)]
#[path = "browser_session_tests.rs"]
mod browser_session_tests;

#[cfg(unix)]
#[path = "browser_user_browser_tests.rs"]
mod browser_user_browser_tests;

#[test]
fn press_script_uses_selector_when_present() {
    let script = build_press_script(Some("Enter"), Some("#email")).unwrap();
    assert!(script.contains("document.querySelector"));
    assert!(script.contains("Enter"));
}

#[test]
fn content_formatter_prefers_content_text() {
    let rendered = format_content_result(&json!({"content": "hello", "title": "x"}));
    assert_eq!(rendered, "hello");
}

#[test]
fn snapshot_maps_to_annotated_get_content() {
    let input = BrowserInput {
        action: "snapshot".into(),
        browser: None,
        provider_action: None,
        params: None,
        url: None,
        tab_id: Some(7),
        window_id: None,
        frame_id: Some(3),
        all_frames: Some(true),
        selector: None,
        text: None,
        contains: None,
        script: None,
        key: None,
        x: None,
        y: None,
        format: None,
        wait: None,
        new_tab: None,
        focus: None,
        clear: None,
        submit: None,
        page_world: None,
        position: None,
        behavior: None,
        timeout_ms: None,
        path: None,
        fields: None,
        scroll_to: None,
        ..Default::default()
    };

    let (action, params, _) = bridge_request("snapshot", &input).unwrap();
    assert_eq!(action, "getContent");
    assert_eq!(params["format"], "annotated");
    assert_eq!(params["tabId"], 7);
    assert_eq!(params["frameId"], 3);
    assert_eq!(params["allFrames"], true);
}

#[test]
fn eval_maps_script_and_page_world() {
    let input = BrowserInput {
        action: "eval".into(),
        browser: None,
        provider_action: None,
        params: None,
        url: None,
        tab_id: None,
        window_id: None,
        frame_id: None,
        all_frames: None,
        selector: None,
        text: None,
        contains: None,
        script: Some("return document.title".into()),
        key: None,
        x: None,
        y: None,
        format: None,
        wait: None,
        new_tab: None,
        focus: None,
        clear: None,
        submit: None,
        page_world: Some(true),
        position: None,
        behavior: None,
        timeout_ms: None,
        path: None,
        fields: None,
        scroll_to: None,
        ..Default::default()
    };

    let (action, params, _) = bridge_request("eval", &input).unwrap();
    assert_eq!(action, "evaluate");
    assert_eq!(params["script"], "return document.title");
    assert_eq!(params["pageWorld"], true);
}

#[test]
fn interactables_maps_to_bridge_action() {
    let input = BrowserInput {
        action: "interactables".into(),
        browser: None,
        provider_action: None,
        params: None,
        url: None,
        tab_id: Some(9),
        window_id: None,
        frame_id: None,
        all_frames: None,
        selector: Some("main".into()),
        text: None,
        contains: None,
        script: None,
        key: None,
        x: None,
        y: None,
        format: None,
        wait: None,
        new_tab: None,
        focus: None,
        clear: None,
        submit: None,
        page_world: None,
        position: None,
        behavior: None,
        timeout_ms: None,
        path: None,
        fields: None,
        scroll_to: None,
        ..Default::default()
    };

    let (action, params, _) = bridge_request("interactables", &input).unwrap();
    assert_eq!(action, "getInteractables");
    assert_eq!(params["tabId"], 9);
    assert_eq!(params["selector"], "main");
}

#[test]
fn schema_exposes_advanced_browser_fields() {
    let schema = BrowserTool::new().parameters_schema();
    let props = schema["properties"]
        .as_object()
        .expect("browser schema should have properties");

    assert!(props.contains_key("action"));
    assert!(props.contains_key("browser"));
    assert!(props.contains_key("url"));
    assert!(props.contains_key("tab_id"));
    assert!(props.contains_key("frame_id"));
    assert!(props.contains_key("selector"));
    assert!(props.contains_key("text"));
    assert!(props.contains_key("contains"));
    assert!(props.contains_key("script"));
    assert!(props.contains_key("key"));
    assert!(props.contains_key("x"));
    assert!(props.contains_key("y"));
    assert!(props.contains_key("format"));
    assert!(props.contains_key("wait"));
    assert!(props.contains_key("new_tab"));
    assert!(props.contains_key("timeout_ms"));
    assert!(props.contains_key("path"));
    assert!(props.contains_key("fields"));
    assert!(props.contains_key("provider_action"));
    assert!(props.contains_key("params"));
    assert!(props.contains_key("all_frames"));
    assert!(props.contains_key("focus"));
    assert!(props.contains_key("clear"));
    assert!(props.contains_key("submit"));
    assert!(props.contains_key("page_world"));
    assert!(props.contains_key("position"));
    assert!(props.contains_key("behavior"));
    assert!(props.contains_key("scroll_to"));
}

#[test]
fn resolve_provider_accepts_every_supported_browser() {
    for browser in [
        "auto", "firefox", "chrome", "chromium", "edge", "brave", "safari",
    ] {
        assert!(resolve_provider(Some(browser)).is_ok(), "{browser}");
    }
}

#[test]
fn resolve_provider_rejects_unknown_browser() {
    let err = resolve_provider(Some("netscape"))
        .err()
        .expect("unknown browsers must not resolve");
    assert!(err.to_string().contains("Unknown browser 'netscape'"));
}

#[test]
fn explicit_browser_request_refuses_a_different_connected_browser() {
    let status = jcode_base::browser::BrowserStatus {
        backend: "firefox_agent_bridge",
        browser: "chrome",
        detected_via: "requested explicitly",
        connected_browser: Some("firefox".into()),
        setup_complete: true,
        binary_installed: true,
        responding: true,
        compatible: true,
        missing_actions: vec![],
        ready: true,
    };
    let err = ready_in_requested_browser(&status, BrowserKind::Chrome, true)
        .expect_err("explicit chrome must not silently drive firefox");
    assert!(
        err.to_string()
            .contains("connected to Firefox, not Google Chrome")
    );
    // Auto mode drives whichever browser owns the bridge.
    assert!(ready_in_requested_browser(&status, BrowserKind::Chrome, false).is_ok());
    // Chromium-family browsers share one extension build.
    let edge = jcode_base::browser::BrowserStatus {
        connected_browser: Some("edge".into()),
        ..status
    };
    assert!(ready_in_requested_browser(&edge, BrowserKind::Chrome, true).is_ok());
}

#[test]
fn prepend_setup_message_preserves_images_and_metadata() {
    let output = ToolOutput::new("done")
        .with_title("browser screenshot")
        .with_metadata(json!({"backend": "firefox_agent_bridge"}))
        .with_labeled_image("image/png", "abc", "shot");

    let output = prepend_setup_message(output, "setup log");
    assert!(output.output.starts_with("setup log\n\ndone"));
    assert_eq!(output.images.len(), 1);
    assert_eq!(output.title.as_deref(), Some("browser screenshot"));
    assert_eq!(output.metadata.as_ref().unwrap()["setup_ran"], true);
    assert_eq!(
        output.metadata.as_ref().unwrap()["backend"],
        "firefox_agent_bridge"
    );
}

#[test]
fn description_tells_models_to_check_status_before_setup() {
    let tool = BrowserTool::new();
    let description = tool.description();
    assert!(description.contains("action='status'"));
    assert!(description.contains("setup only if not ready"));
}

#[cfg(unix)]
#[tokio::test]
async fn readiness_does_not_trust_a_stale_setup_marker() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = jcode_base::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let prev_autolaunch = std::env::var_os("JCODE_BROWSER_AUTOLAUNCH");
    let temp = tempfile::TempDir::new().expect("create temp dir");
    jcode_base::env::set_var("JCODE_HOME", temp.path());
    let prev_auto_update = std::env::var_os("JCODE_BROWSER_AUTO_UPDATE");
    // Keep the test hermetic: never launch a real Firefox or query GitHub.
    jcode_base::env::set_var("JCODE_BROWSER_AUTOLAUNCH", "0");
    jcode_base::env::set_var("JCODE_BROWSER_AUTO_UPDATE", "0");

    let browser_dir = temp.path().join("browser");
    std::fs::create_dir_all(&browser_dir).expect("create browser dir");
    let browser = browser_dir.join("browser");
    std::fs::write(&browser, "#!/bin/sh\nexit 1\n").expect("write fake browser");
    let mut perms = std::fs::metadata(&browser)
        .expect("stat fake browser")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&browser, perms).expect("chmod fake browser");
    std::fs::write(browser_dir.join("firefox-agent-bridge-host"), "host").expect("write fake host");
    std::fs::write(browser_dir.join(".setup-complete"), "complete").expect("write setup marker");

    // Pin the target so the test does not depend on this machine's browsers.
    let target = jcode_base::browser::resolve_target_browser(Some("firefox")).expect("firefox");
    let error = ensure_firefox_ready(&target, false)
        .await
        .expect_err("stale setup marker must not bypass live readiness");
    let message = error.to_string();
    assert!(message.contains("not responding"), "{message}");
    assert!(
        message.contains("Do not retry browser actions"),
        "{message}"
    );
    assert!(message.contains("capability discovery"), "{message}");

    if let Some(prev_home) = prev_home {
        jcode_base::env::set_var("JCODE_HOME", prev_home);
    } else {
        jcode_base::env::remove_var("JCODE_HOME");
    }
    if let Some(prev_autolaunch) = prev_autolaunch {
        jcode_base::env::set_var("JCODE_BROWSER_AUTOLAUNCH", prev_autolaunch);
    } else {
        jcode_base::env::remove_var("JCODE_BROWSER_AUTOLAUNCH");
    }
    match prev_auto_update {
        Some(v) => jcode_base::env::set_var("JCODE_BROWSER_AUTO_UPDATE", v),
        None => jcode_base::env::remove_var("JCODE_BROWSER_AUTO_UPDATE"),
    }
}

#[test]
fn ordinary_click_preserves_existing_bridge_dispatch() {
    let input = BrowserInput {
        action: "click".into(),
        selector: Some("#next".into()),
        ..Default::default()
    };
    let (_, params, _) = bridge_request("click", &input).unwrap();
    assert!(params.get("dispatchEvents").is_none());
}

#[test]
fn handoff_schema_defaults_to_fast_agent_and_bounds_inputs() {
    let _guard = jcode_base::storage::lock_test_env();
    let tool = BrowserTool::new();
    assert!(
        tool.description()
            .contains("Use action='handoff' by default for browser tasks")
    );
    let schema = tool.parameters_schema();
    assert!(
        schema["properties"]["action"]["description"]
            .as_str()
            .unwrap()
            .contains("Use handoff by default for browser tasks")
    );
    assert_eq!(schema["properties"]["max_steps"]["default"], 40);
    assert_eq!(schema["properties"]["max_steps"]["maximum"], 100);
    assert_eq!(schema["properties"]["confidence_threshold"]["default"], 0.8);
    for key in ["goal", "context", "candidates", "text_values"] {
        assert!(schema["properties"].get(key).is_some());
    }
}

#[tokio::test]
async fn handoff_disabled_switch_removes_schema_and_rejects_execution_before_provider_setup() {
    const KEY: &str = "JCODE_BROWSER_HANDOFF_DISABLED";
    struct RestoreEnv(Option<std::ffi::OsString>);
    impl Drop for RestoreEnv {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => jcode_base::env::set_var(KEY, value),
                None => jcode_base::env::remove_var(KEY),
            }
        }
    }

    let _guard = jcode_base::storage::lock_test_env();
    let _restore = RestoreEnv(std::env::var_os(KEY));
    let tool = BrowserTool::new();
    for value in [None, Some("0"), Some("1")] {
        match value {
            Some(value) => jcode_base::env::set_var(KEY, value),
            None => jcode_base::env::remove_var(KEY),
        }
        let disabled = value == Some("1");
        assert_eq!(browser_handoff_disabled(), disabled);
        let schema = tool.parameters_schema();
        let properties = &schema["properties"];
        let actions = properties["action"]["enum"].as_array().unwrap();
        assert_eq!(actions.contains(&json!("handoff")), !disabled);
        for action in ["status", "setup", "open", "click", "fill_form", "eval"] {
            assert!(actions.contains(&json!(action)));
        }
        for key in [
            "goal",
            "context",
            "max_steps",
            "confidence_threshold",
            "text_values",
            "candidates",
        ] {
            assert_eq!(properties.get(key).is_some(), !disabled, "{key}");
        }
        assert_eq!(
            tool.description()
                .contains("Use action='handoff' by default"),
            !disabled
        );
        assert_eq!(
            properties["action"]["description"]
                .as_str()
                .unwrap()
                .contains("Use handoff by default"),
            !disabled
        );
        let ctx = ToolContext {
            session_id: "browser-disable-test".into(),
            message_id: "m".into(),
            tool_call_id: "t".into(),
            working_dir: None,
            stdin_request_tx: None,
            decision_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: super::super::ToolExecutionMode::Direct,
        };
        // An unknown browser makes the enabled branch hermetic. In the
        // disabled branch the guard must fire before even resolving a provider.
        let err = tool
            .execute(json!({"action":"handoff", "browser":"netscape"}), ctx)
            .await
            .expect_err("request must fail without browser side effects");
        if disabled {
            assert!(err.to_string().contains("JCODE_BROWSER_HANDOFF_DISABLED=1"));
        } else {
            assert!(err.to_string().contains("Unknown browser 'netscape'"));
        }
    }
}

#[test]
fn nested_scroll_uses_container_delta_without_escaping_scope() {
    let input: BrowserInput = serde_json::from_value(json!({
        "action":"scroll","selector":"#sections","y":600,"tab_id":7,"frame_id":0,"all_frames":false
    }))
    .unwrap();
    let (action, params, _) = bridge_request("scroll", &input).unwrap();
    assert_eq!(action, "evaluate");
    assert_eq!(params["tabId"], 7);
    assert_eq!(params["frameId"], 0);
    assert_eq!(params["allFrames"], false);
    let script = params["script"].as_str().unwrap();
    assert!(script.contains("element.scrollBy"));
    assert!(script.contains("top:600"));
    assert!(script.contains("return {scrolled:true"));
}

// #1743 live acceptance (ignored by default; see the module docs).
#[cfg(unix)]
#[path = "browser_live_1743_tests.rs"]
mod browser_live_1743_tests;
