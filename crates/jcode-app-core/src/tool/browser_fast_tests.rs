use super::*;
use std::collections::VecDeque;
use std::sync::Mutex;

struct MockBrowser {
    observations: Mutex<VecDeque<Value>>,
    calls: Mutex<Vec<String>>,
    ready: bool,
    fail_action: bool,
}
impl MockBrowser {
    fn new(observations: Vec<Value>) -> Self {
        Self {
            observations: Mutex::new(observations.into()),
            calls: Mutex::new(Vec::new()),
            ready: true,
            fail_action: false,
        }
    }
}
#[async_trait]
impl BrowserProvider for MockBrowser {
    fn id(&self) -> &'static str {
        "mock"
    }
    fn supported_browsers(&self) -> &'static [&'static str] {
        &["auto"]
    }
    async fn status(&self, _: &ToolContext) -> Result<ToolOutput> {
        self.calls.lock().unwrap().push("status".into());
        Ok(ToolOutput::new("").with_metadata(json!({"ready":self.ready})))
    }
    async fn setup(&self) -> Result<ToolOutput> {
        panic!("must not setup")
    }
    async fn ensure_ready(&self) -> Result<Option<String>> {
        panic!("must not implicitly launch/setup")
    }
    async fn execute(
        &self,
        action: &str,
        input: &BrowserInput,
        _: &ToolContext,
    ) -> Result<ToolOutput> {
        assert_eq!(input.tab_id, Some(7));
        assert_eq!(input.frame_id, Some(0));
        assert_eq!(input.all_frames, Some(false));
        self.calls.lock().unwrap().push(action.into());
        if action == "eval" && input.script.as_deref() == Some(OBSERVE_SCRIPT) {
            let mut values = self.observations.lock().unwrap();
            let value = if values.len() > 1 {
                values.pop_front().unwrap()
            } else {
                values.front().unwrap().clone()
            };
            return Ok(ToolOutput::new("").with_metadata(json!({"result":value})));
        }
        if self.fail_action {
            anyhow::bail!("mock failure");
        }
        Ok(ToolOutput::new("").with_metadata(json!({"ok":true})))
    }
}
struct MockTransport {
    decisions: Mutex<VecDeque<Decision>>,
    observed: Mutex<Vec<Value>>,
}
impl MockTransport {
    fn new(choices: &[(&str, f64)]) -> Self {
        Self {
            decisions: Mutex::new(
                choices
                    .iter()
                    .map(|(choice, confidence)| Decision {
                        choice: (*choice).into(),
                        confidence: *confidence,
                        reason: "mock decision".into(),
                    })
                    .collect(),
            ),
            observed: Mutex::new(Vec::new()),
        }
    }
}
#[async_trait]
impl DecisionTransport for MockTransport {
    fn model(&self) -> &str {
        "mock/jev"
    }
    async fn decide(&self, request: &DecisionRequest) -> Result<Decision> {
        assert!(request.options.len() <= MAX_OPTIONS);
        self.observed
            .lock()
            .unwrap()
            .push(request.observation.clone());
        self.decisions
            .lock()
            .unwrap()
            .pop_front()
            .context("Unexpected decision call")
    }
}
fn ctx() -> ToolContext {
    ToolContext {
        session_id: "browser-test".into(),
        message_id: "m".into(),
        tool_call_id: "t".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: super::super::super::ToolExecutionMode::Direct,
    }
}
fn input() -> BrowserInput {
    BrowserInput {
        action: "handoff".into(),
        goal: Some("Inspect the page".into()),
        tab_id: Some(7),
        ..Default::default()
    }
}
fn page(text: &str) -> Value {
    json!({"url":"https://example.test/","title":"Test","text":text,"sensitive":false,"elements":[]})
}
async fn result(browser: &MockBrowser, transport: &MockTransport, input: &BrowserInput) -> Value {
    run(browser, transport, input, &ctx())
        .await
        .unwrap()
        .metadata
        .unwrap()
}

#[tokio::test]
async fn observes_before_done_and_after_each_action() {
    let browser = MockBrowser::new(vec![page("before"), page("before"), page("after")]);
    let transport = MockTransport::new(&[("a0", 0.99), ("done", 0.95)]);
    let result = result(&browser, &transport, &input()).await;
    assert_eq!(result["status"], "done");
    assert_eq!(result["model"], "mock/jev");
    assert_eq!(result["final_observation"]["text"], "after");
    assert_eq!(result["action_trace"][0]["status"], "executed");
    assert_eq!(
        *browser.calls.lock().unwrap(),
        vec!["status", "eval", "eval", "scroll", "eval", "eval", "eval"]
    );
    assert_eq!(transport.observed.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn invalid_or_uncertain_decisions_never_execute() {
    for (choice, probability, reason) in [
        ("a0", 0.79, "Low confidence"),
        ("a0", f64::NAN, "Invalid decision"),
        ("a0", 1.01, "Invalid decision"),
        ("a999", 0.99, "unknown action"),
        ("hand_back", 0.99, "mock decision"),
    ] {
        let browser = MockBrowser::new(vec![page("before")]);
        let transport = MockTransport::new(&[(choice, probability)]);
        let result = result(&browser, &transport, &input()).await;
        assert_eq!(result["status"], "hand_back");
        assert!(
            result["reason"].as_str().unwrap().contains(reason),
            "{result}"
        );
        assert_eq!(*browser.calls.lock().unwrap(), vec!["status", "eval"]);
    }
}
#[tokio::test]
async fn budget_always_observes_last_action() {
    let browser = MockBrowser::new(vec![page("before"), page("before"), page("after")]);
    let transport = MockTransport::new(&[("a0", 0.99)]);
    let mut input = input();
    input.max_steps = Some(1);
    let result = result(&browser, &transport, &input).await;
    assert_eq!(result["status"], "hand_back");
    assert!(result["reason"].as_str().unwrap().contains("budget"));
    assert_eq!(result["final_observation"]["text"], "after");
}
#[tokio::test]
async fn stall_returns_control() {
    let browser = MockBrowser::new(vec![page("same")]);
    let transport = MockTransport::new(&[("a0", 0.99), ("a0", 0.99), ("a0", 0.99)]);
    let result = result(&browser, &transport, &input()).await;
    assert!(result["reason"].as_str().unwrap().contains("stalled"));
    assert_eq!(result["action_trace"].as_array().unwrap().len(), 3);
}
#[tokio::test]
async fn sensitive_pages_never_reach_transport() {
    let mut page = page("captcha");
    page["sensitive"] = json!(true);
    let browser = MockBrowser::new(vec![page]);
    let transport = MockTransport::new(&[]);
    let result = result(&browser, &transport, &input()).await;
    assert_eq!(result["status"], "hand_back");
    assert!(
        result["reason"]
            .as_str()
            .unwrap()
            .contains("Never reset passwords")
    );
    assert!(transport.observed.lock().unwrap().is_empty());
}
#[tokio::test]
async fn validates_inputs_before_browser_calls() {
    for value in [
        json!({"action":"handoff","goal":"x"}),
        json!({"action":"handoff","tab_id":7}),
        json!({"action":"handoff","tab_id":7,"goal":"x","max_steps":31}),
        json!({"action":"handoff","tab_id":7,"goal":"x","confidence_threshold":-0.1}),
    ] {
        let browser = MockBrowser::new(vec![]);
        let result = result(
            &browser,
            &MockTransport::new(&[]),
            &serde_json::from_value(value).unwrap(),
        )
        .await;
        assert_eq!(result["status"], "hand_back");
        assert!(browser.calls.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn browser_errors_and_not_ready_hand_back() {
    let mut browser = MockBrowser::new(vec![page("before")]);
    browser.fail_action = true;
    let value = result(&browser, &MockTransport::new(&[("a0", 0.99)]), &input()).await;
    assert_eq!(value["status"], "hand_back");
    assert!(value["reason"].as_str().unwrap().contains("mock failure"));
    browser.ready = false;
    let value = result(&browser, &MockTransport::new(&[]), &input()).await;
    assert!(value["reason"].as_str().unwrap().contains("not ready"));
}
#[test]
fn rejects_scope_escapes_and_recursive_actions() {
    for value in [
        json!({"action":"handoff"}),
        json!({"action":"setup"}),
        json!({"action":"new_tab"}),
        json!({"action":"click","selector":"a","tab_id":8}),
        json!({"action":"click","selector":"a","frame_id":3}),
        json!({"action":"click","selector":"a","window_id":8}),
        json!({"action":"click","selector":"a","all_frames":true}),
        json!({"action":"open","url":"https://example.test","new_tab":true}),
        json!({"action":"provider_command","provider_action":"newSession"}),
        json!({"action":"provider_command","provider_action":"click","params":{"tabId":8}}),
        json!({"action":"provider_command","provider_action":"click","params":{"target":{"tabId":8}}}),
        json!({"action":"provider_command","provider_action":"click","params":{"allFrames":true}}),
    ] {
        assert!(
            scoped(serde_json::from_value(value.clone()).unwrap(), &input()).is_err(),
            "{value}"
        );
    }
}
#[test]
fn raw_commands_get_authoritative_scope() {
    let scoped=scoped(serde_json::from_value(json!({"action":"provider_command","provider_action":"evaluate","params":{"script":"document.title"}})).unwrap(),&input()).unwrap();
    let (_, params, _) = bridge_request("provider_command", &scoped).unwrap();
    assert_eq!(params["tabId"], 7);
    assert_eq!(params["frameId"], 0);
    assert_eq!(params["allFrames"], false);
    assert_eq!(params["script"], "document.title");
}
#[test]
fn exact_payloads_support_all_tab_local_capabilities() {
    for value in [
        json!({"action":"eval","script":"document.title"}),
        json!({"action":"upload","selector":"input","path":"/fixture.txt"}),
        json!({"action":"fill_form","fields":[{"selector":"#x","value":"trusted"}]}),
        json!({"action":"select","selector":"#s","text":"v"}),
        json!({"action":"press","key":"Enter","selector":"#search"}),
        json!({"action":"click","selector":"#authorized-send"}),
    ] {
        assert!(
            scoped(serde_json::from_value(value.clone()).unwrap(), &input()).is_ok(),
            "{value}"
        );
    }
}
#[test]
fn auto_candidates_filter_risky_controls_and_use_only_supplied_text() {
    let mut input = input();
    input.text_values = vec!["trusted query".into()];
    let page = json!({"elements":[
        {"tag":"a","selector":"#safe","text":"Documentation","href":"https://example.test/docs"},
        {"tag":"a","selector":"#bad","text":"Buy now","href":"https://example.test/buy"},
        {"tag":"button","selector":"#send","text":"Send"},
        {"tag":"input","selector":"#pw","type":"password"},
        {"tag":"input","selector":"#q","type":"search"},
        {"tag":"select","selector":"#s","options":[{"text":"English","value":"en"}]}
    ]});
    let options = candidates(&input, &page).unwrap();
    assert!(
        options
            .iter()
            .any(|c| c.input.selector.as_deref() == Some("#safe"))
    );
    assert!(
        !options
            .iter()
            .any(|c| matches!(c.input.selector.as_deref(), Some("#bad" | "#send" | "#pw")))
    );
    let typing = options.iter().find(|c| c.input.action == "type").unwrap();
    assert_eq!(typing.input.text.as_deref(), Some("trusted query"));
    assert_eq!(typing.input.submit, Some(false));
    let select = options.iter().find(|c| c.input.action == "select").unwrap();
    assert_eq!(select.input.text.as_deref(), Some("en"));
}
#[test]
fn candidates_are_capped_below_model_limit() {
    let mut input = input();
    input.text_values = vec!["text".into(); 16];
    let page = json!({"elements":vec![json!({"tag":"input","selector":"#x","type":"text"});64]});
    assert!(candidates(&input, &page).unwrap().len() + 4 <= MAX_OPTIONS);
}
#[tokio::test]
async fn oversized_observation_is_rejected_not_truncated_json() {
    let browser = MockBrowser::new(vec![page(&"x".repeat(MAX_OBSERVATION))]);
    let value = result(&browser, &MockTransport::new(&[]), &input()).await;
    assert_eq!(value["status"], "hand_back");
    assert!(value["reason"].as_str().unwrap().contains("size limit"));
    assert!(value["final_observation"].is_null());
}
#[tokio::test]
async fn timeout_and_cancellation_drop_pending_work() {
    let error = bounded(
        &ctx(),
        Duration::from_millis(1),
        std::future::pending::<Result<()>>(),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("timed out"));
    let mut ctx = ctx();
    let signal = crate::agent::InterruptSignal::new();
    signal.fire();
    ctx.graceful_shutdown_signal = Some(signal);
    let error = bounded(
        &ctx,
        Duration::from_secs(30),
        std::future::pending::<Result<()>>(),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
}
#[test]
fn observation_script_does_not_read_form_values() {
    assert!(!OBSERVE_SCRIPT.contains("e.value"));
    assert!(OBSERVE_SCRIPT.contains("e.matches('input,textarea,[contenteditable]')||e.querySelector('input,textarea,[contenteditable]')?''"));
    assert!(OBSERVE_SCRIPT.contains("document.querySelectorAll(css).length!==1"));
    assert!(OBSERVE_SCRIPT.contains("o.value.length<=200"));
}

#[tokio::test]
async fn stale_dom_never_executes_selected_action() {
    let browser = MockBrowser::new(vec![page("before"), page("changed while deciding")]);
    let value = result(&browser, &MockTransport::new(&[("a0", 0.99)]), &input()).await;
    assert_eq!(value["status"], "hand_back");
    assert!(value["reason"].as_str().unwrap().contains("DOM changed"));
    assert_eq!(
        *browser.calls.lock().unwrap(),
        vec!["status", "eval", "eval"]
    );
}

#[tokio::test]
async fn exact_actions_execute_once_and_results_stay_with_parent() {
    let browser = MockBrowser::new(vec![page("before")]);
    let transport = MockTransport::new(&[("a0", 0.99), ("done", 0.99)]);
    let mut input = input();
    input.candidates.push(ExactCandidate {
        label: "Authorized click".into(),
        input: json!({"action":"click","selector":"#authorized"}),
    });
    let value = result(&browser, &transport, &input).await;
    assert_eq!(value["status"], "done");
    assert_eq!(value["action_trace"][0]["result"]["metadata"]["ok"], true);
    let requests = transport.observed.lock().unwrap();
    assert_eq!(requests[1]["action_history"][0]["action"], "click");
    assert!(requests[1]["action_history"][0].get("result").is_none());
}

#[test]
fn whole_tab_actions_cannot_escape_explicit_subframe() {
    let mut parent = input();
    parent.frame_id = Some(9);
    for value in [
        json!({"action":"open","url":"https://example.test"}),
        json!({"action":"screenshot"}),
        json!({"action":"list_frames"}),
        json!({"action":"provider_command","provider_action":"navigate","params":{"url":"https://example.test"}}),
    ] {
        assert!(
            scoped(serde_json::from_value(value).unwrap(), &parent)
                .unwrap_err()
                .to_string()
                .contains("Whole-tab")
        );
    }
}

#[tokio::test]
async fn mismatched_window_is_rejected_before_observing_page() {
    let browser = MockBrowser::new(vec![]);
    let mut input = input();
    input.window_id = Some(9);
    let value = result(&browser, &MockTransport::new(&[]), &input).await;
    assert_eq!(value["status"], "hand_back");
    assert!(
        value["reason"]
            .as_str()
            .unwrap()
            .contains("requested window")
    );
    assert_eq!(*browser.calls.lock().unwrap(), vec!["status", "list_tabs"]);
}

#[test]
fn bridge_evaluate_function_body_returns_observation() {
    // Actual bridge uses `new Function(params.script)()`, not eval(expression).
    assert!(OBSERVE_SCRIPT.starts_with("return (() => {"));
    assert!(OBSERVE_SCRIPT.ends_with("})()"));
}

#[test]
fn nullable_handoff_options_do_not_break_direct_actions() {
    let input: BrowserInput =
        serde_json::from_value(json!({"action":"status","candidates":null,"text_values":null}))
            .unwrap();
    assert!(input.candidates.is_empty());
    assert!(input.text_values.is_empty());
}

#[tokio::test]
async fn credentials_in_page_never_reach_transport_or_parent() {
    for secret in [
        "sk-or-v1-fake-secret-test-only-123456",
        "Bearer fake-credential-12345",
        "https://example.test/callback?code=test-only",
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.test",
    ] {
        let browser = MockBrowser::new(vec![page(secret)]);
        let transport = MockTransport::new(&[]);
        let value = result(&browser, &transport, &input()).await;
        assert_eq!(value["status"], "hand_back");
        assert!(!value.to_string().contains(secret));
        assert!(value["final_observation"]["sensitive"] == true);
        assert!(transport.observed.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn done_checks_latest_dom_and_returns_changed_observation() {
    let browser = MockBrowser::new(vec![page("completed"), page("actually failed")]);
    let value = result(&browser, &MockTransport::new(&[("done", 0.99)]), &input()).await;
    assert_eq!(value["status"], "hand_back");
    assert_eq!(value["final_observation"]["text"], "actually failed");
}

#[tokio::test]
async fn same_origin_navigation_requires_renewed_exact_actions() {
    let mut after = page("new page");
    after["url"] = json!("https://example.test/other");
    let browser = MockBrowser::new(vec![page("before"), page("before"), after]);
    let mut input = input();
    input.candidates.push(ExactCandidate {
        label: "Navigate".into(),
        input: json!({"action":"open","url":"https://example.test/other"}),
    });
    input.candidates.push(ExactCandidate {
        label: "Pending action bound to original page".into(),
        input: json!({"action":"click","selector":"#send"}),
    });
    let value = result(&browser, &MockTransport::new(&[("a0", 0.99)]), &input).await;
    assert_eq!(value["status"], "hand_back");
    assert!(value["reason"].as_str().unwrap().contains("URL changed"));
}

#[test]
fn partial_form_results_are_failures() {
    for value in [
        json!({"filled":true,"success":0,"total":1,"results":[{"selector":"#x","ok":false,"error":"missing"}]}),
        json!({"success":1,"total":2}),
        json!({"results":[{"ok":false}]}),
        json!({"error":"failed"}),
    ] {
        assert!(action_failed(&value), "{value}");
    }
    assert!(!action_failed(
        &json!({"success":2,"total":2,"results":[{"ok":true},{"ok":true}]})
    ));
}

#[test]
fn handoff_clicks_do_not_dispatch_twice() {
    let click = scoped(
        serde_json::from_value(json!({"action":"click","selector":"#next"})).unwrap(),
        &input(),
    )
    .unwrap();
    let (_, params, _) = bridge_request("click", &click).unwrap();
    assert_eq!(params["dispatchEvents"], false);
    let raw=scoped(serde_json::from_value(json!({"action":"provider_command","provider_action":"click","params":{"selector":"#next"}})).unwrap(),&input()).unwrap();
    let (_, params, _) = bridge_request("provider_command", &raw).unwrap();
    assert_eq!(params["dispatchEvents"], false);
}

#[tokio::test]
async fn requests_main_agent_script_or_text_without_executing() {
    for (choice, help) in [("script_needed", "script"), ("text_needed", "text")] {
        let browser = MockBrowser::new(vec![page("needs help")]);
        let value = result(&browser, &MockTransport::new(&[(choice, 0.99)]), &input()).await;
        assert_eq!(value["status"], "hand_back");
        assert_eq!(value["requested_help"], help);
        assert_eq!(*browser.calls.lock().unwrap(), vec!["status", "eval"]);
    }
}

#[tokio::test]
async fn exact_candidate_is_not_reoffered_after_execution() {
    let browser = MockBrowser::new(vec![page("same")]);
    let mut input = input();
    input.max_steps = Some(2);
    input.candidates.push(ExactCandidate {
        label: "Authorized one-shot".into(),
        input: json!({"action":"click","selector":"#send"}),
    });
    let value = result(
        &browser,
        &MockTransport::new(&[("a0", 0.99), ("a0", 0.99)]),
        &input,
    )
    .await;
    assert_eq!(value["action_trace"][0]["action"], "click");
    assert_eq!(value["action_trace"][1]["action"], "scroll");
    assert_eq!(
        browser
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|a| *a == "click")
            .count(),
        1
    );
}

struct PendingDecision;
#[async_trait]
impl DecisionTransport for PendingDecision {
    fn model(&self) -> &str {
        "pending"
    }
    async fn decide(&self, _: &DecisionRequest) -> Result<Decision> {
        std::future::pending().await
    }
}

#[tokio::test]
async fn decision_timeout_is_structured_handback() {
    let browser = MockBrowser::new(vec![page("same")]);
    let mut input = input();
    input.timeout_ms = Some(1);
    let output = run(&browser, &PendingDecision, &input, &ctx())
        .await
        .unwrap();
    let value = output.metadata.unwrap();
    assert_eq!(value["status"], "hand_back");
    assert!(value["reason"].as_str().unwrap().contains("timed out"));
    assert_eq!(*browser.calls.lock().unwrap(), vec!["status", "eval"]);
}

#[tokio::test]
async fn cancelled_handoff_never_starts_browser_work() {
    let browser = MockBrowser::new(vec![]);
    let mut ctx = ctx();
    let signal = crate::agent::InterruptSignal::new();
    signal.fire();
    ctx.graceful_shutdown_signal = Some(signal);
    let value = run(&browser, &PendingDecision, &input(), &ctx)
        .await
        .unwrap()
        .metadata
        .unwrap();
    assert_eq!(value["status"], "hand_back");
    assert!(value["reason"].as_str().unwrap().contains("cancelled"));
    assert!(browser.calls.lock().unwrap().is_empty());
}

#[test]
fn compact_candidates_omit_unavailable_scroll_and_wait() {
    let page = json!({"ready_state":"complete","scroll":{"can_up":false,"can_down":false},"elements":[{"tag":"a","selector":"#docs","text":"  Documentation  ","aria":"Documentation","href":"https://example.test/docs"}]});
    let choices = candidates(&input(), &page).unwrap();
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].label, "Click link Documentation");
    let mut scrolling = page.clone();
    scrolling["scroll"]["can_down"] = json!(true);
    let choices = candidates(&input(), &scrolling).unwrap();
    assert_eq!(choices.len(), 2);
    assert_eq!(choices[1].label, "Scroll down");
    scrolling["ready_state"] = json!("loading");
    assert!(
        candidates(&input(), &scrolling)
            .unwrap()
            .iter()
            .any(|c| c.input.action == "wait")
    );
    scrolling["ready_state"] = json!("complete");
    scrolling["elements"] = json!([]);
    assert!(
        candidates(&input(), &scrolling)
            .unwrap()
            .iter()
            .any(|c| c.input.action == "wait")
    );
}

#[test]
fn observer_filters_viewport_and_tracks_scroll_availability() {
    assert!(OBSERVE_SCRIPT.contains("r.bottom>0&&r.right>0&&r.top<innerHeight&&r.left<innerWidth"));
    assert!(OBSERVE_SCRIPT.contains("range.getClientRects()"));
    assert!(OBSERVE_SCRIPT.contains("while(p)"));
    assert!(OBSERVE_SCRIPT.contains("can_down:scrollY+innerHeight<root.scrollHeight-1"));
    assert!(OBSERVE_SCRIPT.contains("ready_state:document.readyState"));
}

#[tokio::test]
async fn delayed_navigation_settles_before_second_model_decision() {
    let mut loading = page("Loading destination");
    loading["ready_state"] = json!("loading");
    let mut after = page("Destination ready");
    after["ready_state"] = json!("complete");
    let mut before = page("before");
    before["ready_state"] = json!("complete");
    before["elements"] = json!([{"tag":"a","text":"Documentation","selector":"#docs","href":"https://example.test/docs"}]);
    after["url"] = json!("https://example.test/docs");
    let browser = MockBrowser::new(vec![
        before.clone(),
        before.clone(),
        before.clone(),
        before.clone(),
        loading,
        after.clone(),
        after.clone(),
    ]);
    let transport = MockTransport::new(&[("a0", 0.99), ("done", 0.99)]);
    let value = result(&browser, &transport, &input()).await;
    assert_eq!(value["status"], "done");
    let seen = transport.observed.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1]["page"]["text"], "Destination ready");
}

#[tokio::test]
async fn settling_loading_page_times_out_without_model_retry() {
    let mut loading = page("Loading");
    loading["ready_state"] = json!("loading");
    let browser = MockBrowser::new(vec![loading]);
    let observe = scoped(
        BrowserInput {
            action: "eval".into(),
            script: Some(OBSERVE_SCRIPT.into()),
            ..Default::default()
        },
        &input(),
    )
    .unwrap();
    let error = settle_after_action(
        &browser,
        &observe,
        &ctx(),
        Duration::from_secs(1),
        tokio::time::Instant::now() + Duration::from_millis(180),
        &page("before"),
        false,
    )
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains("timed out") || error.to_string().contains("did not settle")
    );
}

#[test]
fn cumulative_action_results_are_bounded_without_side_effect_retry_advice() {
    let mut bytes = 0;
    let mut results = Vec::new();
    for _ in 0..30 {
        results.push(retain_result(
            json!({"output":"x".repeat(10_000),"metadata":{"ok":true}}),
            &mut bytes,
        ));
    }
    assert!(bytes <= 32_000);
    assert_eq!(
        results
            .iter()
            .filter(|value| value.get("output").is_some())
            .count(),
        3
    );
    let encoded = serde_json::to_string(&results).unwrap();
    assert!(encoded.len() < 40_000);
    assert!(!encoded.contains("Repeat the direct action"));
    let guidance = results[3]["omitted"].as_str().unwrap();
    assert!(guidance.contains("read-only"));
    assert!(guidance.contains("Do not repeat side effects"));
}
