#![cfg_attr(test, allow(clippy::await_holding_lock))]
use super::*;

#[test]
fn missing_managed_session_allows_only_metadata_creation_or_concrete_scope() {
    for action in ["newSession", "listTabs"] {
        assert!(require_safe_bridge_session_fallback(action, &json!({})).is_ok());
    }
    for action in [
        "evaluate",
        "click",
        "navigate",
        "getContent",
        "uploadFile",
        "getInteractables",
        "fillForm",
        "waitFor",
        "scroll",
        "screenshot",
        "listFrames",
        "getActiveTab",
        "setActiveTab",
    ] {
        assert!(
            require_safe_bridge_session_fallback(action, &json!({"tabId":123})).is_ok(),
            "{action}"
        );
        assert!(
            require_safe_bridge_session_fallback(action, &json!({"windowId":456})).is_ok(),
            "{action}"
        );
        let error = require_safe_bridge_session_fallback(action, &json!({})).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Refusing unscoped browser action"),
            "{action}: {error}"
        );
    }
}

#[test]
fn missing_managed_session_rejects_current_sentinels_and_malformed_scope() {
    for value in [
        json!(-2),
        json!(-1),
        json!(0),
        Value::Null,
        json!("123"),
        json!(1.5),
        json!(true),
        json!(i64::MAX),
    ] {
        for key in ["tabId", "windowId"] {
            let params = json!({(key):value.clone()});
            assert!(
                require_safe_bridge_session_fallback("getContent", &params).is_err(),
                "{params}"
            );
            assert!(
                require_safe_bridge_session_fallback("click", &params).is_err(),
                "{params}"
            );
        }
    }
    // Snake-case raw params are not the bridge's actual target fields.
    assert!(require_safe_bridge_session_fallback("evaluate", &json!({"tab_id":123})).is_err());
    assert!(require_safe_bridge_session_fallback("navigate", &json!({"newTab":true})).is_err());
    assert!(
        require_safe_bridge_session_fallback("click", &json!({"tabId":-1,"windowId":123})).is_err()
    );
    assert!(
        require_safe_bridge_session_fallback("getContent", &json!({"tabId":123,"windowId":-2}))
            .is_err()
    );
}

#[test]
fn missing_managed_session_rejects_composite_unknown_and_fork_routed_commands() {
    let batch = json!({"tabId":123,"commands":[{"action":"click","params":{"selector":"#x"}}]});
    for action in [
        "batch",
        "executeBatch",
        "parallel",
        "fork",
        "scout",
        "unknownRawCommand",
        "closeTab",
    ] {
        assert!(
            require_safe_bridge_session_fallback(action, &batch).is_err(),
            "{action}"
        );
    }
    for fork in [json!("other"), json!("unknown"), Value::Null] {
        for action in ["click", "evaluate", "getContent", "listTabs", "newSession"] {
            assert!(
                require_safe_bridge_session_fallback(action, &json!({"tabId":123,"fork":fork}))
                    .is_err(),
                "{action}"
            );
        }
    }
}

#[test]
fn macos_bridge_page_commands_require_tab_scope() {
    for action in [
        "navigate",
        "getContent",
        "getInteractables",
        "evaluate",
        "click",
        "type",
        "fillForm",
        "waitFor",
        "scroll",
        "listFrames",
        "setActiveTab",
    ] {
        for id in [1, i32::MAX] {
            assert!(require_safe_macos_bridge_command(action, &json!({"tabId":id})).is_ok());
        }
        for params in [json!({}), json!({"windowId":123}), json!({"tab_id":123})] {
            assert!(
                require_safe_macos_bridge_command(action, &params).is_err(),
                "{action}: {params}"
            );
        }
    }
}

#[test]
fn macos_bridge_rejects_raw_upload_even_with_tab_scope() {
    let input: BrowserInput = serde_json::from_value(json!({
        "action":"provider_command", "provider_action":"uploadFile",
        "params":{"tabId":123,"windowId":456,"path":"/nonexistent-audit-upload","selector":"input"}
    }))
    .unwrap();
    let (action, params, _) = bridge_request(&input.action, &input).unwrap();
    assert!(require_safe_macos_bridge_command(&action, &params).is_err());
}

#[test]
fn macos_bridge_denies_capture_even_with_explicit_tab() {
    for action in ["screenshot", "uploadFile"] {
        assert!(macos_bridge_params_json(action, json!({"tabId":123})).is_err());
    }
}

#[test]
fn macos_bridge_serializes_creation_without_implicit_content() {
    for params in [
        Value::Null,
        json!({}),
        json!({"returnContent":true}),
        json!({"returnContent":false}),
    ] {
        let outgoing: Value =
            serde_json::from_str(&macos_bridge_params_json("newSession", params).unwrap()).unwrap();
        assert_eq!(outgoing["returnContent"], false);
    }
    let params = json!({"tabId":123,"format":"text"});
    let outgoing: Value =
        serde_json::from_str(&macos_bridge_params_json("getContent", params.clone()).unwrap())
            .unwrap();
    assert_eq!(outgoing, params);
}

#[test]
fn macos_bridge_serialization_rejects_malformed_and_nested_creators() {
    for action in ["newSession", "newWindow"] {
        for params in [
            json!([]),
            json!(""),
            json!(false),
            json!(1),
            json!({"commands":[]}),
            json!({"params":{"action":"getContent"}}),
            json!({"fork":null}),
            json!({"url":{"command":"getContent"}}),
            json!({"returnContent":[]}),
            json!({"batch":[]}),
        ] {
            assert!(macos_bridge_params_json(action, params).is_err());
        }
    }
}

#[test]
fn macos_bridge_metadata_rejects_nested_and_unknown_options() {
    for action in ["listTabs", "getActiveTab"] {
        for params in [
            json!({"commands":[]}),
            json!({"params":{"tabId":123}}),
            json!({"retarget":123}),
            json!({"fork":null}),
            json!({"url":"about:blank"}),
        ] {
            assert!(macos_bridge_params_json(action, params).is_err());
        }
        for params in [Value::Null, json!({}), json!({"tabId":123,"windowId":456})] {
            assert!(macos_bridge_params_json(action, params).is_ok());
        }
    }
}

#[test]
fn macos_bridge_rejects_malformed_ids() {
    for value in [
        json!(-2),
        json!(-1),
        json!(0),
        Value::Null,
        json!("123"),
        json!(1.5),
        json!(true),
        json!([]),
        json!({}),
        json!(i64::MAX),
    ] {
        for action in ["getContent", "click"] {
            assert!(require_safe_macos_bridge_command(action, &json!({"tabId":value})).is_err());
            assert!(
                require_safe_macos_bridge_command(action, &json!({"tabId":123,"windowId":value}))
                    .is_err()
            );
        }
    }
}

#[test]
fn macos_bridge_allows_only_known_creation_and_metadata_without_tab_scope() {
    for action in ["newSession", "newWindow", "listTabs", "getActiveTab"] {
        assert!(require_safe_macos_bridge_command(action, &json!({})).is_ok());
    }
}

#[test]
fn macos_bridge_rejects_composites_unknown_commands_and_fork_even_with_tab_scope() {
    for action in [
        "batch",
        "executeBatch",
        "parallel",
        "fork",
        "scout",
        "unknownRawCommand",
        "closeTab",
    ] {
        assert!(
            require_safe_macos_bridge_command(action, &json!({"tabId":123,"commands":[]})).is_err()
        );
    }
    for action in [
        "getContent",
        "click",
        "newSession",
        "newWindow",
        "listTabs",
        "getActiveTab",
    ] {
        for fork in [json!("other"), json!(false), Value::Null] {
            assert!(
                require_safe_macos_bridge_command(action, &json!({"tabId":123,"fork":fork}))
                    .is_err()
            );
        }
        for params in [Value::Null, json!([]), json!("invalid")] {
            assert!(require_safe_macos_bridge_command(action, &params).is_err());
        }
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_bridge_public_tool_denies_before_dispatch_despite_live_session() {
    use std::os::unix::net::UnixListener;
    let _guard = jcode_base::storage::lock_test_env();
    struct RestoreEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Drop for RestoreEnv {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                match value {
                    Some(value) => jcode_base::env::set_var(key, value),
                    None => jcode_base::env::remove_var(key),
                }
            }
        }
    }
    let _restore = RestoreEnv(
        ["XDG_RUNTIME_DIR", "BROWSER_SESSION"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect(),
    );
    let temp = tempfile::Builder::new()
        .prefix("jcode-tab-guard-")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = temp
        .path()
        .join("browser-session-guard-firefox-target-v4.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    std::fs::write(
        temp.path()
            .join("browser-session-guard-firefox-target-v4.pid"),
        std::process::id().to_string(),
    )
    .unwrap();
    jcode_base::env::set_var("XDG_RUNTIME_DIR", temp.path());
    jcode_base::env::set_var("BROWSER_SESSION", "guard-firefox-target-v4");
    for request in [
        json!({"action":"get_content"}),
        json!({"action":"get_content","window_id":123}),
        json!({"action":"provider_command","provider_action":"getContent","params":{"tabId":0}}),
        json!({"action":"provider_command","provider_action":"batch","params":{"tabId":123}}),
        json!({"action":"provider_command","provider_action":"getContent","params":[]}),
        json!({"action":"provider_command","provider_action":"listTabs","params":{"commands":[]}}),
        json!({"action":"provider_command","provider_action":"getActiveTab","params":{"params":{"tabId":123}}}),
        json!({"action":"provider_command","provider_action":"getContent","params":{"tabId":123,"fork":null}}),
        json!({"action":"handoff","window_id":123}),
        json!({"action":"handoff","tab_id":123,"window_id":-1}),
        json!({"action":"handoff","tab_id":123,"window_id":i64::MAX}),
    ] {
        let mut request = request;
        request["browser"] = json!("firefox");
        let ctx = ToolContext {
            session_id: "guard".into(),
            message_id: "m".into(),
            tool_call_id: "t".into(),
            working_dir: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: super::super::ToolExecutionMode::Direct,
        };
        let error = BrowserTool::new().execute(request, ctx).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Refusing unscoped or unsafe macOS browser action"),
            "{error}"
        );
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

// Managed CLI sessions remain the non-macOS Unix policy.
#[cfg(all(unix, not(target_os = "macos")))]
#[path = "browser_session_tests.rs"]
mod browser_session_tests;

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
        "auto", "firefox", "chrome", "chromium", "edge", "brave", "opera", "safari",
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
fn safari_uses_native_backend_and_exposes_limits() {
    assert_eq!(
        resolve_provider(Some("safari")).unwrap().id(),
        "safari_applescript"
    );
    assert_eq!(
        resolve_provider(Some("opera")).unwrap().id(),
        "firefox_agent_bridge"
    );
    let capabilities = crate::browser_safari::capabilities();
    assert_eq!(capabilities["extension_required"], false);
    assert!(
        capabilities["unsupported"]
            .as_array()
            .unwrap()
            .contains(&json!("screenshot"))
    );
    assert!(
        BrowserTool::new().parameters_schema()["properties"]["browser"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("opera"))
    );
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
    // Sharing an extension build does not make browser identities interchangeable.
    let edge = jcode_base::browser::BrowserStatus {
        connected_browser: Some("edge".into()),
        ..status
    };
    assert!(ready_in_requested_browser(&edge, BrowserKind::Chrome, true).is_err());
    let brave = jcode_base::browser::BrowserStatus {
        connected_browser: Some("brave".into()),
        ..edge
    };
    assert!(ready_in_requested_browser(&brave, BrowserKind::Chrome, true).is_err());
    assert!(ready_in_requested_browser(&brave, BrowserKind::Brave, true).is_ok());
    let incompatible_wrong_browser = jcode_base::browser::BrowserStatus {
        compatible: false,
        ready: false,
        ..brave
    };
    let message =
        ready_in_requested_browser(&incompatible_wrong_browser, BrowserKind::Chrome, true)
            .unwrap_err()
            .to_string();
    assert!(
        message.contains("connected to Brave, not Google Chrome"),
        "{message}"
    );
    assert!(!message.contains("missing required actions"));
    assert!(!message.contains("Only one browser"));
    let unidentified = jcode_base::browser::BrowserStatus {
        connected_browser: None,
        ..incompatible_wrong_browser
    };
    assert!(
        ready_in_requested_browser(&unidentified, BrowserKind::Chrome, true)
            .unwrap_err()
            .to_string()
            .contains("unverified browser")
    );
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

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "requires approved Chrome or Firefox native bridge; creates two fresh loopback fixture tabs, leaves them open"]
async fn live_macos_bridge_explicit_tab_ignores_selected_decoy() -> Result<()> {
    use anyhow::{Context, ensure};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let browser = std::env::var("JCODE_BROWSER_LIVE_BROWSER")?;
    ensure!(
        matches!(browser.as_str(), "chrome" | "firefox"),
        "Only Chrome and Firefox are approved"
    );
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_nanos()
        .to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}/", listener.local_addr()?);
    let receipt = nonce.clone();
    let server = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut request = [0u8; 4096];
            if !matches!(tokio::time::timeout(Duration::from_secs(2), stream.read(&mut request)).await, Ok(Ok(n)) if n > 0)
            {
                continue;
            }
            let path = String::from_utf8_lossy(&request)
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .to_string();
            let body = format!(
                "<!doctype html><title>{receipt}</title><p>{receipt}{path}</p><input id='owned' autocomplete='off'>"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    struct StopServer(tokio::task::JoinHandle<()>);
    impl Drop for StopServer {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _server = StopServer(server);
    let ctx = ToolContext {
        session_id: format!("owned-decoy-{nonce}"),
        message_id: "m".into(),
        tool_call_id: "t".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: super::super::ToolExecutionMode::Direct,
    };
    async fn call(browser: &str, ctx: &ToolContext, mut input: Value) -> Result<Value> {
        input["browser"] = json!(browser);
        let output = tokio::time::timeout(
            Duration::from_secs(30),
            BrowserTool::new().execute(input, ctx.clone()),
        )
        .await??;
        let metadata = output.metadata.context("missing native metadata")?;
        Ok(metadata.get("result").unwrap_or(&metadata).clone())
    }
    let mut ids = Vec::new();
    for role in ["first", "decoy"] {
        let url = format!("{base}{nonce}/{role}");
        let created = call(&browser, &ctx, json!({"action":"provider_command","provider_action":"newSession","params":{"url":url,"returnContent":true}})).await?;
        ensure!(
            created.get("content").is_none()
                && created.get("text").is_none()
                && created.get("html").is_none(),
            "creator returned implicit page content: {created}"
        );
        let id = created["tabId"]
            .as_i64()
            .context("creator omitted concrete tabId")?;
        ensure!(id > 0 && id <= i32::MAX as i64, "invalid created ID");
        ids.push(id);
        eprintln!("OWNED_DECOY_FIXTURE browser={browser} tabId={id} url={url} nonce={nonce}");
        call(
            &browser,
            &ctx,
            json!({"action":"wait","tab_id":id,"selector":"#owned"}),
        )
        .await?;
        let check = call(&browser, &ctx, json!({"action":"eval","tab_id":id,"script":"return JSON.stringify({url:location.href,title:document.title,value:document.querySelector('#owned')?.value})"})).await?;
        ensure!(
            check.to_string().contains(&url) && check.to_string().contains(&nonce),
            "fresh fixture identity mismatch: {check}"
        );
    }
    ensure!(ids[0] != ids[1], "creator reused a tab");
    call(
        &browser,
        &ctx,
        json!({"action":"select_tab","tab_id":ids[1],"focus":true}),
    )
    .await?;
    let decoy_before = call(&browser, &ctx, json!({"action":"eval","tab_id":ids[1],"script":"return JSON.stringify({url:location.href,title:document.title,value:document.querySelector('#owned').value,changed:document.body.dataset.changed||''})"})).await?;
    let read = call(
        &browser,
        &ctx,
        json!({"action":"get_content","tab_id":ids[0]}),
    )
    .await?;
    ensure!(
        read.to_string().contains(&format!("{nonce}/{nonce}/first")),
        "explicit read missed first-tab receipt: {read}"
    );
    // Re-select the exact owned decoy before each target operation.
    call(
        &browser,
        &ctx,
        json!({"action":"select_tab","tab_id":ids[1],"focus":true}),
    )
    .await?;
    call(
        &browser,
        &ctx,
        json!({"action":"type","tab_id":ids[0],"selector":"#owned","text":nonce,"clear":true}),
    )
    .await?;
    call(
        &browser,
        &ctx,
        json!({"action":"select_tab","tab_id":ids[1],"focus":true}),
    )
    .await?;
    let script = format!(
        "if(location.href!=={})throw Error('wrong fixture');document.body.dataset.changed={};return JSON.stringify({{value:document.querySelector('#owned').value,changed:document.body.dataset.changed}})",
        serde_json::to_string(&format!("{base}{nonce}/first"))?,
        serde_json::to_string(&nonce)?
    );
    let changed = call(
        &browser,
        &ctx,
        json!({"action":"eval","tab_id":ids[0],"script":script}),
    )
    .await?;
    ensure!(
        changed.to_string().matches(&nonce).count() >= 2,
        "first tab was not changed: {changed}"
    );
    let decoy_after = call(&browser, &ctx, json!({"action":"eval","tab_id":ids[1],"script":"return JSON.stringify({url:location.href,title:document.title,value:document.querySelector('#owned').value,changed:document.body.dataset.changed||''})"})).await?;
    ensure!(
        decoy_before == decoy_after,
        "decoy changed: {decoy_before} -> {decoy_after}"
    );
    eprintln!(
        "Owned {browser} fixture tabs retained: first={} decoy={} nonce={nonce}",
        ids[0], ids[1]
    );
    Ok(())
}
