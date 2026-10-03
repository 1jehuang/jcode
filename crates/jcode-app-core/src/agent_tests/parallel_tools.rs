//! Concurrent execution of adjacent concurrency-safe tool calls.
//!
//! These drive a real `Agent` turn with a scripted provider and instrumented
//! tools, so they exercise the same loop code production uses: overlap of safe
//! calls, the unsafe-call barrier, result ordering, and the config toggle.

use super::*;
use serde_json::{Value, json};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Restore scheduler and hook state before releasing the shared environment lock.
struct ParallelTestSandbox {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _sandbox: crate::auth::test_sandbox::AuthTestSandbox,
}

impl ParallelTestSandbox {
    fn new(enabled: Option<bool>) -> Self {
        let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
        let saved = [
            "JCODE_PARALLEL_TOOLS",
            "JCODE_HOOK_PRE_TOOL",
            "JCODE_HOOK_PRE_TOOL_TRANSFORM",
            "JCODE_HOOKS_DISABLED",
        ]
        .into_iter()
        .map(|key| {
            let previous = std::env::var_os(key);
            crate::env::remove_var(key);
            (key, previous)
        })
        .collect();
        if let Some(enabled) = enabled {
            crate::env::set_var("JCODE_PARALLEL_TOOLS", if enabled { "1" } else { "0" });
        }
        crate::config::invalidate_config_cache();
        Self {
            saved,
            _sandbox: sandbox,
        }
    }
}

impl Drop for ParallelTestSandbox {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            if let Some(value) = value {
                crate::env::set_var(key, value);
            } else {
                crate::env::remove_var(key);
            }
        }
        crate::config::invalidate_config_cache();
    }
}

mod rollout {
    use super::*;

    #[tokio::test]
    async fn invalid_and_sdk_completed_calls_break_prefetch_runs() {
        let _sandbox = ParallelTestSandbox::new(Some(true));
        let mut h = harness(vec![], Duration::ZERO).await;
        for invalid in [false, true] {
            let mut calls: Vec<ToolCall> = (0..3)
                .map(|index| ToolCall {
                    id: format!("candidate-{index}"),
                    name: "safe_sleep".into(),
                    input: json!({"label":index}),
                    ..Default::default()
                })
                .collect();
            let mut sdk_results = HashMap::new();
            if invalid {
                calls[1].name.clear();
                assert!(calls[1].validation_error().is_some());
            } else {
                sdk_results.insert(calls[1].id.clone(), ("already completed".into(), false));
            }
            let mut prefetch = h.agent.tool_prefetch().await;
            h.agent
                .maybe_prefetch_safe_run(
                    &mut prefetch,
                    &calls,
                    0,
                    "excluded",
                    &sdk_results,
                    crate::agent::parallel_tools::PrefetchRegistry::Shared,
                )
                .await;
            for index in 0..calls.len() {
                assert!(!prefetch.contains(index));
            }
            assert!(h.timeline.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn policy_denied_calls_break_prefetch_runs() {
        let _sandbox = ParallelTestSandbox::new(Some(true));
        let mut h = harness(vec![], Duration::ZERO).await;
        let calls: Vec<ToolCall> = ["safe_sleep", "unsafe_sleep", "safe_sleep"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| ToolCall {
                id: format!("policy-{index}"),
                name: name.into(),
                input: json!({"label":name}),
                ..Default::default()
            })
            .collect();
        // The middle tool is safe at the registry but denied by Agent policy.
        h.agent
            .registry
            .register(
                "unsafe_sleep".into(),
                Arc::new(SleepTool {
                    name: "unsafe_sleep",
                    safe: true,
                    sleep: Duration::ZERO,
                    timeline: h.timeline.clone(),
                    active: Arc::new(AtomicUsize::new(0)),
                    max_active: h.max_active.clone(),
                    gate: h.gate.clone(),
                }),
            )
            .await;
        h.agent.disabled_tools.insert("unsafe_sleep".into());
        let mut prefetch = h.agent.tool_prefetch().await;
        h.agent
            .maybe_prefetch_safe_run(
                &mut prefetch,
                &calls,
                0,
                "policy",
                &HashMap::new(),
                crate::agent::parallel_tools::PrefetchRegistry::Shared,
            )
            .await;
        for index in 0..calls.len() {
            assert!(!prefetch.contains(index));
        }
        assert!(h.timeline.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn sdk_custom_override_is_not_concurrency_safe() {
        let _sandbox = ParallelTestSandbox::new(Some(true));
        let h = harness(vec![], Duration::ZERO).await;
        struct RemoveOverlay(String);
        impl Drop for RemoveOverlay {
            fn drop(&mut self) {
                crate::tool::sdk::remove_session(&self.0);
            }
        }
        let _cleanup = RemoveOverlay(h.agent.session.id.clone());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        crate::tool::sdk::configure(
            &h.agent.session.id,
            "parallel-policy-test",
            crate::protocol::SessionToolConfig {
                custom: vec![crate::protocol::SessionToolDefinition {
                    name: "safe_sleep".into(),
                    description: "SDK override".into(),
                    parameters: json!({"type":"object"}),
                }],
                ..Default::default()
            },
            tx,
        )
        .unwrap();
        assert!(
            !h.agent
                .registry
                .is_concurrency_safe(&h.agent.session.id, "safe_sleep", &json!({}))
                .await
        );
    }

    async fn assert_batches(streaming: bool, parallel: bool) {
        let ids: Vec<String> = (0..23).map(|index| format!("call-{index}")).collect();
        let calls = ids
            .iter()
            .map(|id| (id.as_str(), "safe_sleep", json!({"label":id, "gated":true})))
            .collect();
        let mut h = harness(calls, Duration::ZERO).await;
        let timeline = h.timeline.clone();
        let gate = h.gate.clone();
        let batch_size = if parallel { 10 } else { 1 };
        tokio::join!(run_mode(&mut h.agent, streaming), async {
            for batch in ids.chunks(batch_size) {
                for id in batch {
                    wait_for_event(&timeline, "start", id).await;
                }
                // Every observed start is gated until this batch is released.
                let events = timeline.lock().unwrap().clone();
                let active = events.iter().filter(|(e, _)| e == "start").count()
                    - events.iter().filter(|(e, _)| e == "end").count();
                assert_eq!(active, batch.len(), "{events:?}");
                gate.add_permits(batch.len());
                for id in batch {
                    wait_for_event(&timeline, "end", id).await;
                }
            }
        });
        assert_eq!(h.max_active.load(Ordering::SeqCst), batch_size);
        assert_eq!(
            recorded_results(&h.agent),
            ids.iter()
                .map(|id| { (id.clone(), format!("result:{id}")) })
                .collect::<Vec<_>>()
        );
        let events = h.timeline.lock().unwrap();
        assert_eq!(
            events.len(),
            ids.len() * 2,
            "each call executes exactly once"
        );
        for boundary in (batch_size..ids.len()).step_by(batch_size) {
            for prior in &ids[boundary - batch_size..boundary] {
                assert!(
                    position(&events, "end", prior) < position(&events, "start", &ids[boundary])
                );
            }
        }
    }

    #[tokio::test]
    async fn streaming_parallel_cap_ten_with_twenty_three_calls() {
        let _sandbox = ParallelTestSandbox::new(Some(true));
        assert_batches(true, true).await;
    }

    #[tokio::test]
    async fn blocking_parallel_cap_ten_with_twenty_three_calls() {
        let _sandbox = ParallelTestSandbox::new(Some(true));
        assert_batches(false, true).await;
    }

    #[tokio::test]
    async fn streaming_parallel_defaults_off() {
        let _sandbox = ParallelTestSandbox::new(None);
        assert!(!crate::config::config().tools.parallel);
        assert_batches(true, false).await;
    }

    #[tokio::test]
    async fn blocking_parallel_defaults_off() {
        let _sandbox = ParallelTestSandbox::new(None);
        assert!(!crate::config::config().tools.parallel);
        assert_batches(false, false).await;
    }

    #[tokio::test]
    async fn built_in_parallel_eligibility_is_exactly_four_read_tools() {
        let _sandbox = ParallelTestSandbox::new(Some(true));
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let registry = Registry::new(provider).await;
        let expected = ["jcode_docs", "ls", "read", "webfetch"];
        let mut eligible = Vec::new();
        for definition in registry.definitions(None).await {
            let safe = registry
                .is_concurrency_safe(
                    "matrix",
                    &definition.name,
                    &json!({"file_path": "src/lib.rs"}),
                )
                .await;
            assert_eq!(
                safe,
                expected.contains(&definition.name.as_str()),
                "{}",
                definition.name
            );
            if safe {
                eligible.push(definition.name);
            }
        }
        eligible.sort();
        assert_eq!(eligible, expected);
        for name in ["bash", "agentgrep", "websearch", "unknown", "batch"] {
            assert!(
                !registry
                    .is_concurrency_safe("matrix", name, &json!({"command":"pwd"}))
                    .await,
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn image_pdf_and_malformed_reads_stay_sequential() {
        let _sandbox = ParallelTestSandbox::new(Some(true));
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let registry = Registry::new(provider).await;
        for path in [
            "picture.png",
            "photo.JPG",
            "animation.gif",
            "image.webp",
            "image.bmp",
            "favicon.ico",
            "photo.jpeg",
            "report.PDF",
        ] {
            assert!(
                !registry
                    .is_concurrency_safe("matrix", "read", &json!({"file_path": path}))
                    .await,
                "media read must remain sequential: {path}"
            );
        }
        for input in [
            json!({}),
            json!({"file_path": null}),
            json!({"file_path": 12}),
        ] {
            assert!(!registry.is_concurrency_safe("matrix", "read", &input).await);
        }
        for path in ["src/main.rs", "README.md", "Cargo.toml", "Makefile"] {
            assert!(
                registry
                    .is_concurrency_safe("matrix", "read", &json!({"file_path": path}))
                    .await
            );
        }
    }

    #[tokio::test]
    async fn pre_tool_transform_excludes_parallel_until_hooks_disabled() {
        let sandbox = ParallelTestSandbox::new(Some(true));
        std::fs::write(
            sandbox._sandbox.root().join("config.toml"),
            "[hooks]\npre_tool_transform = ['not-executed-transform']\n",
        )
        .unwrap();
        crate::config::invalidate_config_cache();
        let h = harness(vec![], Duration::ZERO).await;
        assert!(
            !h.agent
                .registry
                .is_concurrency_safe(&h.agent.session.id, "safe_sleep", &json!({}))
                .await
        );
        crate::env::set_var("JCODE_HOOKS_DISABLED", "1");
        assert!(
            h.agent
                .registry
                .is_concurrency_safe(&h.agent.session.id, "safe_sleep", &json!({}))
                .await
        );
    }
}

/// Provider that emits a fixed list of tool calls on the first request and a
/// plain answer afterwards.
#[derive(Clone)]
struct ScriptedProvider {
    calls: Arc<Vec<(String, String, Value)>>,
    served: Arc<AtomicUsize>,
}

impl ScriptedProvider {
    fn new(calls: Vec<(&str, &str, Value)>) -> Self {
        Self {
            calls: Arc::new(
                calls
                    .into_iter()
                    .map(|(id, name, input)| (id.to_string(), name.to_string(), input))
                    .collect(),
            ),
            served: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        let first = self.served.fetch_add(1, Ordering::SeqCst) == 0;
        let mut events = Vec::new();
        if first {
            for (id, name, input) in self.calls.iter() {
                events.push(StreamEvent::ToolUseStart {
                    id: id.clone(),
                    name: name.clone(),
                });
                events.push(StreamEvent::ToolInputDeltaFor {
                    id: id.clone(),
                    delta: input.to_string(),
                });
                events.push(StreamEvent::ToolUseEndFor { id: id.clone() });
            }
            events.push(StreamEvent::MessageEnd {
                stop_reason: Some("tool_use".into()),
            });
        } else {
            events.push(StreamEvent::TextDelta("done".into()));
            events.push(StreamEvent::MessageEnd {
                stop_reason: Some("end_turn".into()),
            });
        }
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
    fn name(&self) -> &str {
        "parallel-tools-test"
    }
    fn supports_compaction(&self) -> bool {
        false
    }
    fn context_window(&self) -> usize {
        100_000
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

/// Shared timeline of (event, tool label) entries.
type Timeline = Arc<Mutex<Vec<(String, String)>>>;

/// A tool that sleeps and records when it starts and ends. Safety is fixed
/// per instance so tests can mix safe and unsafe calls.
struct SleepTool {
    name: &'static str,
    safe: bool,
    sleep: Duration,
    timeline: Timeline,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    gate: Arc<tokio::sync::Semaphore>,
}

#[async_trait]
impl crate::tool::Tool for SleepTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "sleep test tool"
    }
    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {"label": {"type": "string"}}})
    }
    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        self.safe
    }
    async fn execute(&self, input: Value, _: crate::tool::ToolContext) -> Result<ToolOutput> {
        let label = input["label"].as_str().unwrap_or("?").to_string();
        let now_active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(now_active, Ordering::SeqCst);
        self.timeline
            .lock()
            .unwrap()
            .push(("start".into(), label.clone()));
        struct ActiveCall {
            active: Arc<AtomicUsize>,
            timeline: Timeline,
            label: String,
            completed: bool,
        }
        impl Drop for ActiveCall {
            fn drop(&mut self) {
                self.active.fetch_sub(1, Ordering::SeqCst);
                if !self.completed {
                    self.timeline
                        .lock()
                        .unwrap()
                        .push(("cancel".into(), self.label.clone()));
                }
            }
        }
        let mut active = ActiveCall {
            active: self.active.clone(),
            timeline: self.timeline.clone(),
            label: label.clone(),
            completed: false,
        };
        if input["gated"].as_bool().unwrap_or(false) {
            self.gate.acquire().await.unwrap().forget();
        }
        let sleep = input["sleep_ms"]
            .as_u64()
            .map(Duration::from_millis)
            .unwrap_or(self.sleep);
        tokio::time::sleep(sleep).await;
        self.timeline
            .lock()
            .unwrap()
            .push(("end".into(), label.clone()));
        active.completed = true;
        if input["fail"].as_bool().unwrap_or(false) {
            anyhow::bail!("failure:{label}");
        }
        let output = input["output_chars"]
            .as_u64()
            .map(|chars| "x".repeat(chars as usize))
            .unwrap_or_else(|| format!("result:{label}"));
        Ok(ToolOutput::new(output))
    }
}

struct Harness {
    agent: Agent,
    timeline: Timeline,
    max_active: Arc<AtomicUsize>,
    gate: Arc<tokio::sync::Semaphore>,
}

async fn harness(calls: Vec<(&str, &str, Value)>, sleep: Duration) -> Harness {
    let provider = Arc::new(ScriptedProvider::new(calls));
    let timeline: Timeline = Arc::new(Mutex::new(Vec::new()));
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let registry = Registry::empty();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    for (name, safe) in [("safe_sleep", true), ("unsafe_sleep", false)] {
        registry
            .register(
                name.into(),
                Arc::new(SleepTool {
                    name,
                    safe,
                    sleep,
                    timeline: timeline.clone(),
                    active: active.clone(),
                    max_active: max_active.clone(),
                    gate: gate.clone(),
                }),
            )
            .await;
    }
    let mut agent = Agent::new(provider, registry);
    agent.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "run the tools".into(),
            cache_control: None,
        }],
    );
    Harness {
        agent,
        timeline,
        max_active,
        gate,
    }
}

fn call(id: &'static str, tool: &'static str, label: &str) -> (&'static str, &'static str, Value) {
    (id, tool, json!({"label": label, "intent": "test"}))
}

/// Tool results in the order they were appended to the session.
fn recorded_results(agent: &Agent) -> Vec<(String, String)> {
    agent
        .session
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => Some((tool_use_id.clone(), content.clone())),
            _ => None,
        })
        .collect()
}

fn position(timeline: &[(String, String)], event: &str, label: &str) -> usize {
    timeline
        .iter()
        .position(|(e, l)| e == event && l == label)
        .unwrap_or_else(|| panic!("missing {event} for {label} in {timeline:?}"))
}

async fn run_streaming(agent: &mut Agent) {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::time::timeout(Duration::from_secs(30), agent.run_turn_streaming_mpsc(tx))
        .await
        .expect("turn finishes")
        .expect("turn succeeds");
}

#[tokio::test]
async fn safe_calls_run_concurrently_and_keep_model_order() {
    let _sandbox = ParallelTestSandbox::new(Some(true));
    let sleep = Duration::from_millis(400);
    let mut h = harness(
        vec![
            call("c1", "safe_sleep", "a"),
            call("c2", "safe_sleep", "b"),
            call("c3", "safe_sleep", "c"),
            call("c4", "safe_sleep", "d"),
        ],
        sleep,
    )
    .await;

    let started = Instant::now();
    run_streaming(&mut h.agent).await;
    let elapsed = started.elapsed();

    assert_eq!(
        h.max_active.load(Ordering::SeqCst),
        4,
        "all four safe calls should overlap"
    );
    // Sequential would be ~1.6s. Allow generous slack for slow CI.
    assert!(
        elapsed < sleep * 3,
        "four 400ms safe calls took {elapsed:?}, expected roughly one sleep"
    );
    assert_eq!(
        recorded_results(&h.agent),
        vec![
            ("c1".into(), "result:a".into()),
            ("c2".into(), "result:b".into()),
            ("c3".into(), "result:c".into()),
            ("c4".into(), "result:d".into()),
        ],
        "results must be recorded in the model's order"
    );
}

#[tokio::test]
async fn unsafe_call_is_a_barrier_between_safe_runs() {
    let _sandbox = ParallelTestSandbox::new(Some(true));
    let mut h = harness(
        vec![
            call("c1", "safe_sleep", "r1"),
            call("c2", "safe_sleep", "r2"),
            call("c3", "unsafe_sleep", "w"),
            call("c4", "safe_sleep", "r3"),
            call("c5", "safe_sleep", "r4"),
        ],
        Duration::from_millis(150),
    )
    .await;

    run_streaming(&mut h.agent).await;
    let timeline = h.timeline.lock().unwrap().clone();

    // The write starts only after both earlier reads finished...
    let write_start = position(&timeline, "start", "w");
    assert!(
        position(&timeline, "end", "r1") < write_start,
        "{timeline:?}"
    );
    assert!(
        position(&timeline, "end", "r2") < write_start,
        "{timeline:?}"
    );
    // ...and the later reads start only after the write finished.
    let write_end = position(&timeline, "end", "w");
    assert!(
        write_end < position(&timeline, "start", "r3"),
        "{timeline:?}"
    );
    assert!(
        write_end < position(&timeline, "start", "r4"),
        "{timeline:?}"
    );
    // Each side of the barrier still ran in parallel.
    assert_eq!(h.max_active.load(Ordering::SeqCst), 2);
    let ids: Vec<String> = recorded_results(&h.agent)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, ["c1", "c2", "c3", "c4", "c5"]);
}

#[tokio::test]
async fn blocking_run_turn_loop_also_runs_safe_calls_concurrently() {
    let _sandbox = ParallelTestSandbox::new(Some(true));
    let sleep = Duration::from_millis(400);
    let mut h = harness(
        vec![
            call("c1", "safe_sleep", "a"),
            call("c2", "safe_sleep", "b"),
            call("c3", "safe_sleep", "c"),
        ],
        sleep,
    )
    .await;

    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(30), h.agent.run_turn(false))
        .await
        .expect("turn finishes")
        .expect("turn succeeds");
    let elapsed = started.elapsed();

    assert_eq!(h.max_active.load(Ordering::SeqCst), 3);
    assert!(elapsed < sleep * 2, "took {elapsed:?}");
    let ids: Vec<String> = recorded_results(&h.agent)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, ["c1", "c2", "c3"]);
}

#[tokio::test]
async fn disabling_parallel_tools_restores_sequential_execution() {
    let _sandbox = ParallelTestSandbox::new(Some(false));
    assert!(!crate::config::config().tools.parallel);

    let mut h = harness(
        vec![
            call("c1", "safe_sleep", "a"),
            call("c2", "safe_sleep", "b"),
            call("c3", "safe_sleep", "c"),
        ],
        Duration::from_millis(100),
    )
    .await;
    run_streaming(&mut h.agent).await;

    assert_eq!(
        h.max_active.load(Ordering::SeqCst),
        1,
        "with the toggle off, no two calls may overlap"
    );
    let ids: Vec<String> = recorded_results(&h.agent)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, ["c1", "c2", "c3"]);
}

#[tokio::test]
async fn lone_safe_call_and_unsafe_only_responses_stay_sequential() {
    let _sandbox = ParallelTestSandbox::new(Some(true));
    let mut h = harness(
        vec![
            call("c1", "unsafe_sleep", "w1"),
            call("c2", "safe_sleep", "r1"),
            call("c3", "unsafe_sleep", "w2"),
        ],
        Duration::from_millis(80),
    )
    .await;
    run_streaming(&mut h.agent).await;

    assert_eq!(h.max_active.load(Ordering::SeqCst), 1);
    let timeline = h.timeline.lock().unwrap().clone();
    let labels: Vec<&str> = timeline.iter().map(|(_, label)| label.as_str()).collect();
    assert_eq!(labels, ["w1", "w1", "r1", "r1", "w2", "w2"]);
}

async fn wait_for_event(timeline: &Timeline, event: &str, label: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if timeline
                .lock()
                .unwrap()
                .iter()
                .any(|(e, l)| e == event && l == label)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("missing {event} for {label}"));
}

async fn run_mode(agent: &mut Agent, streaming: bool) {
    if streaming {
        run_streaming(agent).await;
    } else {
        tokio::time::timeout(Duration::from_secs(30), agent.run_turn(false))
            .await
            .expect("turn finishes")
            .expect("turn succeeds");
    }
}

async fn assert_completion_durations(streaming: bool) {
    let mut h = harness(
        vec![
            ("slow", "safe_sleep", json!({"label":"slow", "gated":true})),
            call("fast", "safe_sleep", "fast"),
        ],
        Duration::ZERO,
    )
    .await;
    let timeline = h.timeline.clone();
    let gate = h.gate.clone();
    tokio::join!(run_mode(&mut h.agent, streaming), async move {
        wait_for_event(&timeline, "end", "fast").await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        gate.add_permits(1);
    });
    let durations: Vec<_> = h
        .agent
        .session
        .messages
        .iter()
        .filter_map(|message| message.tool_duration_ms)
        .collect();
    assert_eq!(durations.len(), 2);
    assert!(
        durations[0] >= durations[1] + 100,
        "durations: {durations:?}"
    );
    assert_eq!(
        recorded_results(&h.agent),
        vec![
            ("slow".into(), "result:slow".into()),
            ("fast".into(), "result:fast".into()),
        ]
    );
}

#[tokio::test]
async fn blocking_parallel_duration_excludes_ordered_wait() {
    let _sandbox = ParallelTestSandbox::new(Some(true));
    assert_completion_durations(false).await;
}

#[tokio::test]
async fn streaming_parallel_duration_excludes_ordered_wait() {
    let _sandbox = ParallelTestSandbox::new(Some(true));
    assert_completion_durations(true).await;
}

async fn assert_turn_cancellation(streaming: bool) {
    let mut h = harness(
        vec![
            ("a", "safe_sleep", json!({"label":"a", "gated":true})),
            ("b", "safe_sleep", json!({"label":"b", "gated":true})),
        ],
        Duration::ZERO,
    )
    .await;
    let timeline = h.timeline.clone();
    {
        let turn = run_mode(&mut h.agent, streaming);
        tokio::pin!(turn);
        tokio::select! {
            _ = &mut turn => panic!("gated turn must not finish"),
            _ = async {
                wait_for_event(&timeline, "start", "a").await;
                wait_for_event(&timeline, "start", "b").await;
            } => {}
        }
        // Dropping the turn here must cancel both the task removed from the
        // prefetch map and the task still owned by that map.
    }
    wait_for_event(&timeline, "cancel", "a").await;
    wait_for_event(&timeline, "cancel", "b").await;
    assert!(recorded_results(&h.agent).is_empty());
}

#[tokio::test]
async fn cancelling_blocking_turn_aborts_active_and_pending_prefetch() {
    let _sandbox = ParallelTestSandbox::new(Some(true));
    assert_turn_cancellation(false).await;
}

#[tokio::test]
async fn cancelling_streaming_turn_aborts_active_and_pending_prefetch() {
    let _sandbox = ParallelTestSandbox::new(Some(true));
    assert_turn_cancellation(true).await;
}

#[tokio::test]
async fn urgent_interrupt_preserves_completed_prefetch_errors_and_cancels_running_work() {
    let _sandbox = ParallelTestSandbox::new(Some(true));
    let mut h = harness(
        vec![
            ("a", "safe_sleep", json!({"label":"a", "gated":true})),
            ("b", "safe_sleep", json!({"label":"b", "fail":true})),
            ("c", "safe_sleep", json!({"label":"c", "sleep_ms":60000})),
            call("d", "unsafe_sleep", "d"),
        ],
        Duration::ZERO,
    )
    .await;
    let queue = h.agent.soft_interrupt_queue();
    let timeline = h.timeline.clone();
    let gate = h.gate.clone();
    tokio::join!(run_streaming(&mut h.agent), async move {
        wait_for_event(&timeline, "end", "b").await;
        wait_for_event(&timeline, "start", "c").await;
        queue.lock().unwrap().push(SoftInterruptMessage {
            content: "stop remaining work".into(),
            images: vec![],
            urgent: true,
            source: SoftInterruptSource::User,
        });
        gate.add_permits(1);
    });
    let results = recorded_results(&h.agent);
    assert_eq!(results.len(), 4);
    assert_eq!(results[0].1, "result:a");
    assert!(results[1].1.contains("failure:b"), "{results:?}");
    assert!(results[2].1.contains("Skipped"));
    assert!(results[3].1.contains("Skipped"));
    wait_for_event(&h.timeline, "cancel", "c").await;
    assert!(
        !h.timeline
            .lock()
            .unwrap()
            .iter()
            .any(|(_, label)| label == "d")
    );
}

#[tokio::test]
async fn reload_preserves_completed_prefetch_and_cancels_running_work() {
    let _sandbox = ParallelTestSandbox::new(Some(true));
    let mut h = harness(
        vec![
            ("a", "safe_sleep", json!({"label":"a", "gated":true})),
            call("b", "safe_sleep", "b"),
            ("c", "safe_sleep", json!({"label":"c", "sleep_ms":60000})),
        ],
        Duration::ZERO,
    )
    .await;
    let shutdown = h.agent.graceful_shutdown_signal();
    let timeline = h.timeline.clone();
    tokio::join!(run_streaming(&mut h.agent), async move {
        wait_for_event(&timeline, "end", "b").await;
        wait_for_event(&timeline, "start", "c").await;
        shutdown.fire();
    });
    let results = recorded_results(&h.agent);
    assert_eq!(results.len(), 3);
    assert!(results[0].1.contains("reload"));
    assert_eq!(results[1].1, "result:b");
    assert!(results[2].1.contains("Skipped"));
    wait_for_event(&h.timeline, "cancel", "a").await;
    wait_for_event(&h.timeline, "cancel", "c").await;
}

async fn assert_aggregate_budget(streaming: bool) {
    let mut h = harness(
        vec![
            (
                "a",
                "safe_sleep",
                json!({"label":"a", "gated":true, "output_chars":40000}),
            ),
            (
                "b",
                "safe_sleep",
                json!({"label":"b", "output_chars":40000}),
            ),
        ],
        Duration::ZERO,
    )
    .await;
    {
        let compaction = h.agent.registry.compaction();
        let mut manager = compaction.write().await;
        manager.set_budget(100_000);
        manager.update_observed_input_tokens(75_000);
    }
    let timeline = h.timeline.clone();
    let gate = h.gate.clone();
    tokio::join!(run_mode(&mut h.agent, streaming), async move {
        wait_for_event(&timeline, "end", "b").await;
        gate.add_permits(1);
    });
    let results = recorded_results(&h.agent);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].1, "x".repeat(40000));
    assert!(
        results[1].1.contains("accept_large_output"),
        "second output not withheld: {}",
        results[1].1.len()
    );
    assert!(results[1].1.len() < 4000);
}

#[tokio::test]
async fn blocking_parallel_outputs_share_current_context_budget() {
    let _sandbox = ParallelTestSandbox::new(Some(true));
    assert_aggregate_budget(false).await;
}

#[tokio::test]
async fn streaming_parallel_outputs_share_current_context_budget() {
    let _sandbox = ParallelTestSandbox::new(Some(true));
    assert_aggregate_budget(true).await;
}

#[tokio::test]
async fn configured_pre_tool_gate_disables_automatic_concurrency() {
    let _sandbox = ParallelTestSandbox::new(Some(true));
    struct Restore {
        hook: Option<std::ffi::OsString>,
        disabled: Option<std::ffi::OsString>,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            for (key, value) in [
                ("JCODE_HOOK_PRE_TOOL", &self.hook),
                ("JCODE_HOOKS_DISABLED", &self.disabled),
            ] {
                if let Some(value) = value {
                    crate::env::set_var(key, value);
                } else {
                    crate::env::remove_var(key);
                }
            }
            crate::config::invalidate_config_cache();
        }
    }
    let _restore = Restore {
        hook: std::env::var_os("JCODE_HOOK_PRE_TOOL"),
        disabled: std::env::var_os("JCODE_HOOKS_DISABLED"),
    };
    let h = harness(vec![], Duration::ZERO).await;
    crate::env::set_var("JCODE_HOOK_PRE_TOOL", "stateful-policy-command");
    crate::env::remove_var("JCODE_HOOKS_DISABLED");
    crate::config::invalidate_config_cache();
    assert!(
        !h.agent
            .registry
            .is_concurrency_safe(&h.agent.session.id, "safe_sleep", &json!({}))
            .await
    );
    crate::env::set_var("JCODE_HOOKS_DISABLED", "1");
    assert!(
        h.agent
            .registry
            .is_concurrency_safe(&h.agent.session.id, "safe_sleep", &json!({}))
            .await
    );
}

#[tokio::test]
async fn explicit_background_transfer_survives_streaming_turn_cancellation() {
    let _sandbox = ParallelTestSandbox::new(Some(true));
    // The process-wide manager may have been initialized in a prior sandbox.
    let output_path = crate::background::global().output_path_for("scheduler-test");
    std::fs::create_dir_all(output_path.parent().unwrap()).unwrap();
    let mut h = harness(
        vec![
            ("a", "safe_sleep", json!({"label":"a", "gated":true})),
            ("b", "safe_sleep", json!({"label":"b", "sleep_ms":60000})),
        ],
        Duration::ZERO,
    )
    .await;
    let timeline = h.timeline.clone();
    let background = h.agent.background_tool_signal();
    let session_id = h.agent.session.id.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    {
        let turn = h.agent.run_turn_streaming_mpsc(tx);
        tokio::pin!(turn);
        tokio::select! {
            result = &mut turn => panic!("turn unexpectedly finished: {result:?}"),
            _ = async {
                wait_for_event(&timeline, "start", "a").await;
                wait_for_event(&timeline, "start", "b").await;
                background.fire();
                tokio::time::timeout(Duration::from_secs(5), async {
                    while let Some(event) = rx.recv().await {
                        if matches!(event, crate::protocol::ServerEvent::ToolDone { ref id, ref output, .. }
                            if id == "a" && output.contains("moved to background")) { return; }
                    }
                    panic!("missing background ToolDone");
                }).await.expect("background registration finishes");
            } => {}
        }
    }
    wait_for_event(&h.timeline, "cancel", "b").await;
    assert!(
        !h.timeline
            .lock()
            .unwrap()
            .iter()
            .any(|(e, l)| e == "cancel" && l == "a")
    );
    h.gate.add_permits(1);
    wait_for_event(&h.timeline, "end", "a").await;
    let task_id = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let tasks = crate::background::global().list().await;
            if let Some(task) = tasks.iter().find(|task| task.session_id == session_id)
                && !crate::background::global().is_live_task(&task.task_id)
                && crate::background::global()
                    .output(&task.task_id)
                    .await
                    .as_deref()
                    == Some("result:a")
            {
                return task.task_id.clone();
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("background output persisted");
    let _ = std::fs::remove_file(crate::background::global().output_path_for(&task_id));
    let _ = std::fs::remove_file(crate::background::global().status_path_for(&task_id));
}
