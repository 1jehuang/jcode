//! SWE-Pruner held-out measurement at wiring time (exec/pruner-heldout).
//!
//! Drives the WIRED path (`Agent::run_once_capture` -> `run_turn` ->
//! `messages_for_provider` -> `apply_tool_result_clearing` ->
//! `pruner_prune_display`, with `JCODE_PRUNER_MODE=on`) over the HOLDOUT
//! ids only (never tune ids), and reports the same gate numbers as the
//! pilot gate plus cue/ref intactness.
//!
//! Method per item: a scripted [`Provider`] emits one tool call (`read` or
//! `bash`, matching the manifest tool family) and then, on the follow-up
//! turn, returns the pruned send view produced by the wired clearing path.
//! The fixture tool (`holdout_fixture_read` / `holdout_fixture_bash`) echoes
//! the frozen corpus bytes verbatim, so the only transform on the send view
//! is the wired `apply_tool_result_clearing`. Metrics are computed on the
//! pruned display copy exactly as the pilot gate does:
//! `tokens_for_text` = ceiling(chars/4), error lines via the same
//! `error|fail|panic|assert|traceback` rule, cue/ref via the
//! `[jcode-retention:offloaded ... read file_path=... expand: ...]`
//! substitution grammar.
//!
//! Bypass-family holdout items (`todo`, `jcode_docs`) are exercised through
//! the same wired path to confirm bypass (send view carries the
//! substitution only, no pruner markers) and counted separately — they are
//! NOT part of the mean-ratio gate (same rule as the pilot gate, which
//! skips unclassifiable items).
//!
//! Corpus: `JCODE_PRUNER_CORPUS_DIR` or the default pilot path. Fails open
//! with a skip message when the corpus is absent (never red).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use jcode_app_core::agent::Agent;
use jcode_app_core::message::{ContentBlock, Message, StreamEvent, ToolDefinition};
use jcode_app_core::provider::{EventStream, Provider};
use jcode_app_core::tool::{Registry, Tool, ToolContext, ToolOutput};

/// Fixture tool: echoes the frozen corpus bytes for one item verbatim.
struct HoldoutEchoTool {
    name: &'static str,
    payload: Arc<std::sync::Mutex<HashMap<String, String>>>,
    call_id: String,
}

#[async_trait]
impl Tool for HoldoutEchoTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "Held-out fixture: echoes frozen corpus bytes verbatim."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }
    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let map = self.payload.lock().unwrap();
        let content = map
            .get(&self.call_id)
            .cloned()
            .unwrap_or_else(|| "fixture missing".to_string());
        Ok(ToolOutput::new(content))
    }
}

/// Scripted provider: turn 1 emits the tool call for the current item, turn 2
/// ends the turn. `complete` returns a one-shot stream; the turn loop then
/// executes the tool locally and re-requests, at which point we end.
#[derive(Clone)]
struct HoldoutScriptProvider {
    tool_name: String,
    tool_input: serde_json::Value,
    call_id: String,
    calls: Arc<std::sync::Mutex<usize>>,
    /// Captured send-view messages per provider request (for metric reads).
    seen: Arc<std::sync::Mutex<Vec<Vec<Message>>>>,
}

#[async_trait]
impl Provider for HoldoutScriptProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        self.seen.lock().unwrap().push(messages.to_vec());
        let mut n = self.calls.lock().unwrap();
        *n += 1;
        let first = *n == 1;
        drop(n);
        let events = if first {
            vec![
                StreamEvent::ToolUseStart {
                    id: self.call_id.clone(),
                    name: self.tool_name.clone(),
                },
                StreamEvent::ToolInputDelta(self.tool_input.to_string()),
                StreamEvent::ToolUseEnd,
                StreamEvent::MessageEnd {
                    stop_reason: Some("tool_use".to_string()),
                },
            ]
        } else {
            vec![
                StreamEvent::TextDelta("done".to_string()),
                StreamEvent::MessageEnd {
                    stop_reason: Some("end_turn".to_string()),
                },
            ]
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
    fn name(&self) -> &str {
        "holdout-script"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn tokens_for_text(text: &str) -> usize {
    (text.chars().count() + 3) / 4
}

fn is_error_line(line: &str) -> bool {
    let bytes = line.as_bytes();
    let needle = |w: &[u8]| {
        bytes
            .windows(w.len())
            .any(|win| win.eq_ignore_ascii_case(w))
    };
    needle(b"error")
        || needle(b"fail")
        || needle(b"panic")
        || needle(b"assert")
        || needle(b"traceback")
}

#[tokio::test]
async fn pruner_heldout_wired_path_gate() {
    let _guard = jcode_app_core::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    jcode_app_core::env::set_var("JCODE_HOME", temp.path());
    let prev_window = std::env::var_os("JCODE_COMPACTION_CLEAR_TOOL_RESULTS_OLDER_THAN");
    jcode_app_core::env::set_var("JCODE_COMPACTION_CLEAR_TOOL_RESULTS_OLDER_THAN", "1");
    let prev_pruner = std::env::var_os("JCODE_PRUNER_MODE");
    jcode_app_core::env::set_var("JCODE_PRUNER_MODE", "on");
    jcode_app_core::config::Config::invalidate_cache();

    struct Restore {
        home: Option<std::ffi::OsString>,
        window: Option<std::ffi::OsString>,
        pruner: Option<std::ffi::OsString>,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(home) = self.home.take() {
                jcode_app_core::env::set_var("JCODE_HOME", home);
            } else {
                jcode_app_core::env::remove_var("JCODE_HOME");
            }
            if let Some(window) = self.window.take() {
                jcode_app_core::env::set_var(
                    "JCODE_COMPACTION_CLEAR_TOOL_RESULTS_OLDER_THAN",
                    window,
                );
            } else {
                jcode_app_core::env::remove_var("JCODE_COMPACTION_CLEAR_TOOL_RESULTS_OLDER_THAN");
            }
            if let Some(pruner) = self.pruner.take() {
                jcode_app_core::env::set_var("JCODE_PRUNER_MODE", pruner);
            } else {
                jcode_app_core::env::remove_var("JCODE_PRUNER_MODE");
            }
            jcode_app_core::config::Config::invalidate_cache();
        }
    }
    let _restore = Restore {
        home: prev_home,
        window: prev_window,
        pruner: prev_pruner,
    };

    let dir = std::env::var("JCODE_PRUNER_CORPUS_DIR")
        .unwrap_or_else(|_| "/home/sk/.jcode/scratch/pruner-pilot/items".to_string());
    let manifest_path = format!("{dir}/manifest.json");
    let manifest_text = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(_) => {
            eprintln!("SKIP pruner heldout: no corpus at {manifest_path}");
            return;
        }
    };
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest_text).expect("corpus manifest parses");
    let items = manifest.as_array().expect("manifest is an array");

    // Full ToolUse inputs recovered from the operators' session transcripts
    // (vendored: the manifest only carries a 200-char truncated `input_json`
    // hint, which is not valid JSON for 6/18 holdout items and would leave
    // the wired turn loop with no dispatchable tool call). Recovery script:
    // match manifest (session, call_id) against
    // `~/.jcode/sessions/<session>` tool_use blocks; see REPORT §(a).
    // Tune ids are NEVER read — this file holds holdout files only.
    let full_inputs: HashMap<String, serde_json::Value> =
        serde_json::from_str(include_str!("testdata/pruner_heldout_inputs.json"))
            .expect("vendored holdout inputs parse");

    let mut ratios: Vec<f64> = Vec::new();
    let mut err_ok = 0usize;
    let mut cue_ok = 0usize;
    let mut scored = 0usize;
    let mut bypass_n = 0usize;
    let mut bypass_ok = 0usize;
    let mut per_item: Vec<String> = Vec::new();
    let mut wire_floor: Vec<String> = Vec::new();

    for item in items {
        if item.get("split").and_then(|v| v.as_str()) != Some("holdout") {
            continue;
        }
        let file = item["file"].as_str().expect("file");
        let content = std::fs::read_to_string(format!("{dir}/{file}")).expect("item text");
        let tool = item["tool"].as_str().unwrap_or("unknown");
        // Full ToolUse input for the wired turn: recovered transcripts
        // (vendored above), falling back to the manifest hint when it parses.
        // This is the input the REAL historical call carried, so the scorer's
        // ToolUse-derived hint set matches production shape exactly.
        let input: serde_json::Value = full_inputs.get(file).cloned().unwrap_or_else(|| {
            serde_json::from_str(
                item.get("input_json")
                    .and_then(|v| v.as_str())
                    .unwrap_or("null"),
            )
            .unwrap_or(serde_json::Value::Null)
        });
        if !input.is_object() {
            wire_floor.push(format!(
                "{file} tool={tool} (no JSON-object input available: wire cannot dispatch, scorer not invoked)"
            ));
            continue;
        }
        // NOTE (wire artifact, see REPORT): the live `bash` tool fronts a
        // destructive-command gate, so risky historical commands without a
        // `justification` field are REFUSED at execution time even though
        // the registry holds the echo fixture. The test does not stamp a
        // synthetic justification (it would add hint tokens the historical
        // call never carried); bash refusals surface here as measured.

        // Wired-path tool name: `read` items run under the EXACT manifest
        // name (announced + registered over the same key in this item's
        // fresh registry), so scorer parity with the pilot gate is
        // byte-for-byte: same `classify_tool` family AND same tool-name hint
        // tokens. `bash` items instead run under a synthetic name containing
        // `bash` (`holdout_bash_echo` classifies TestBash identically) that
        // the REAL bash tool's destructive gate never sees: the live gate
        // intercepts the real `bash` execution path (refusal text replaces
        // the corpus bytes for risky historical commands), which would
        // measure the refusal instead of the frozen corpus. The ToolUse block
        // still carries the full historical input, so the scorer's
        // command/intent hint fields are the real ones; only the tool-NAME
        // token differs (`holdout_bash_echo` vs `bash`). REPORT §(c)
        // quantifies that hint-token delta explicitly.
        let wired_name: &'static str = if tool.contains("read") {
            "read"
        } else if tool == "bash" {
            "holdout_bash_echo"
        } else {
            // Bypass families (todo, jcode_docs, ...): run through the wired
            // path to confirm bypass, counted separately from the gate.
            let call_id = format!("holdout-bypass-{file}");
            let payload = Arc::new(std::sync::Mutex::new(HashMap::from([(
                call_id.clone(),
                content.clone(),
            )])));
            let seen: Arc<std::sync::Mutex<Vec<Vec<Message>>>> =
                Arc::new(std::sync::Mutex::new(Vec::new()));
            let provider = HoldoutScriptProvider {
                // Same unknown name as the registered bypass tool below, so
                // the turn loop dispatches to the echo fixture.
                tool_name: "holdout_fixture_write".to_string(),
                tool_input: input.clone(),
                call_id: call_id.clone(),
                calls: Arc::new(std::sync::Mutex::new(0)),
                seen: seen.clone(),
            };
            let provider_arc: Arc<dyn Provider> = Arc::new(provider);
            let registry = Registry::new(provider_arc.clone()).await;
            // Register under an UNKNOWN name so classify_tool bypasses.
            struct BypassTool {
                payload: Arc<std::sync::Mutex<HashMap<String, String>>>,
                call_id: String,
            }
            #[async_trait]
            impl Tool for BypassTool {
                fn name(&self) -> &str {
                    "holdout_fixture_write"
                }
                fn description(&self) -> &str {
                    "bypass fixture"
                }
                fn parameters_schema(&self) -> serde_json::Value {
                    serde_json::json!({"type": "object", "properties": {}})
                }
                async fn execute(
                    &self,
                    _input: serde_json::Value,
                    _ctx: ToolContext,
                ) -> anyhow::Result<ToolOutput> {
                    let map = self.payload.lock().unwrap();
                    Ok(ToolOutput::new(
                        map.get(&self.call_id).cloned().unwrap_or_default(),
                    ))
                }
            }
            registry
                .register(
                    "holdout_fixture_write".to_string(),
                    Arc::new(BypassTool {
                        payload: payload.clone(),
                        call_id: call_id.clone(),
                    }),
                )
                .await;
            let mut agent = Agent::new(provider_arc, registry);
            agent
                .run_once_capture("holdout probe")
                .await
                .expect("turn runs");
            // Second turn forces the clearing path over the stored result:
            // keep=1 clears everything before the last message.
            agent
                .run_once_capture("holdout probe followup")
                .await
                .expect("followup turn runs");
            let seen = seen.lock().unwrap();
            // The follow-up request's send view must carry the substitution
            // only (bypass: no pruner markers).
            let mut found_substitution = false;
            let mut found_marker = false;
            for batch in seen.iter() {
                for msg in batch {
                    for block in &msg.content {
                        if let ContentBlock::ToolResult { content, .. } = block {
                            if content.contains("[jcode-retention:offloaded") {
                                found_substitution = true;
                            }
                            if content.contains("(filtered ") {
                                found_marker = true;
                            }
                        }
                    }
                }
            }
            bypass_n += 1;
            if found_substitution && !found_marker {
                bypass_ok += 1;
            }
            per_item.push(format!(
                "bypass {file} tool={tool} substitution={found_substitution} marker={found_marker}"
            ));
            continue;
        };

        let call_id = format!("holdout-{file}");
        let payload = Arc::new(std::sync::Mutex::new(HashMap::from([(
            call_id.clone(),
            content.clone(),
        )])));
        let seen: Arc<std::sync::Mutex<Vec<Vec<Message>>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let provider = HoldoutScriptProvider {
            tool_name: wired_name.to_string(),
            tool_input: input.clone(),
            call_id: call_id.clone(),
            calls: Arc::new(std::sync::Mutex::new(0)),
            seen: seen.clone(),
        };
        let provider_arc: Arc<dyn Provider> = Arc::new(provider);
        let registry = Registry::new(provider_arc.clone()).await;
        registry
            .register(
                wired_name.to_string(),
                Arc::new(HoldoutEchoTool {
                    name: wired_name,
                    payload: payload.clone(),
                    call_id: call_id.clone(),
                }),
            )
            .await;
        let mut agent = Agent::new(provider_arc, registry);
        agent
            .run_once_capture("holdout probe")
            .await
            .expect("turn runs");
        agent
            .run_once_capture("holdout probe followup")
            .await
            .expect("followup turn runs");

        // Read the pruned display copy from the follow-up request's send
        // view: the ToolResult block carrying the pruner output ahead of the
        // `[jcode-retention:offloaded ...]` substitution.
        let seen = seen.lock().unwrap();
        let mut display: Option<String> = None;
        for batch in seen.iter().rev() {
            for msg in batch {
                for block in &msg.content {
                    if let ContentBlock::ToolResult { content, .. } = block {
                        if content.contains("[jcode-retention:offloaded") {
                            display = Some(content.clone());
                            break;
                        }
                    }
                }
                if display.is_some() {
                    break;
                }
            }
            if display.is_some() {
                break;
            }
        }
        let view = display.unwrap_or_else(|| panic!("{file}: wired send view found"));
        // Strip the send-path timestamp header (`Message::with_timestamps`
        // PREPENDS `[tool timing: ...] ` / `[timestamp] ` to the ToolResult
        // content inline when `features.message_timestamps` is on): it is
        // added by the provider-bound transform AFTER the pruner runs, so it
        // must not enter the pruned-copy metrics. Strip one leading `[...] `
        // token (same line, space-separated) when the original content does
        // not start with one.
        let strip_view = |text: &str| -> String {
            let t = text.trim_start();
            if t.starts_with('[') && !content.starts_with('[') {
                if let Some(end) = t.find(']') {
                    return t[end + 1..].trim_start().to_string();
                }
            }
            text.to_string()
        };
        let view = strip_view(&view);
        let pruned = view
            .split_once("[jcode-retention:")
            .map(|(head, _)| head.trim_end().to_string())
            .unwrap_or_default();

        // Empty head has two causes: (a) scorer fail-open None (changed
        // nothing — genuine ratio 1.0), or (b) the wire floor
        // (TOOL_RESULT_CLEAR_MIN_CHARS=200): the clearing path never calls
        // the scorer for results <= 200 chars, so no pruner output exists to
        // measure. Case (b) is a property of the WIRE, not the scorer, and
        // must be reported separately, not folded into the scorer's mean.
        // Wire floor check uses the STORED ToolResult length (what the
        // clearing path measures with `content.chars().count() > 200`), read
        // from the last pre-clearing request, not the disk file length.
        let mut stored_len: Option<usize> = None;
        for batch in seen.iter() {
            for msg in batch {
                for block in &msg.content {
                    if let ContentBlock::ToolResult {
                        content: stored, ..
                    } = block
                    {
                        if !stored.contains("[jcode-retention:") {
                            stored_len = Some(stored.chars().count());
                        }
                    }
                }
            }
        }
        let under_wire_floor = stored_len.is_some_and(|n| n <= 200);
        if under_wire_floor {
            wire_floor.push(format!(
                "{file} tool={tool} chars={} (under 200-char wire floor, scorer not invoked)",
                content.chars().count()
            ));
            continue;
        }
        let effective = if pruned.is_empty() { &content } else { &pruned };
        let tb = tokens_for_text(&content);
        let ta = tokens_for_text(effective);
        ratios.push(ta as f64 / tb.max(1) as f64);
        let err_kept = content
            .lines()
            .filter(|l| is_error_line(l))
            .all(|l| effective.contains(l));
        err_ok += err_kept as usize;
        let cue = view.contains("[jcode-retention:offloaded")
            && view.contains("read file_path=")
            && view.contains("expand:");
        cue_ok += cue as usize;
        scored += 1;
        per_item.push(format!(
            "{file} tool={tool} ratio={:.3} err={err_kept} cue={cue}",
            ta as f64 / tb.max(1) as f64
        ));
    }

    eprintln!("pruner heldout (wired path):");
    for line in &per_item {
        eprintln!("  {line}");
    }
    if !wire_floor.is_empty() {
        eprintln!("  under wire floor (scorer not invoked):");
        for line in &wire_floor {
            eprintln!("    {line}");
        }
    }
    let mean = ratios.iter().sum::<f64>() / scored.max(1) as f64;
    eprintln!(
        "heldout gate: scored={scored} mean_ratio={mean:.3} err={err_ok}/{scored} cue={cue_ok}/{scored} bypass={bypass_ok}/{bypass_n} wirefloor={}",
        wire_floor.len()
    );
    assert!(
        scored >= 10,
        "scored >=10 classifiable holdout items, got {scored}"
    );
    assert_eq!(err_ok, scored, "error_lines_kept 100%");
    assert_eq!(cue_ok, scored, "cue_ref_intact 100%");
    assert!(mean <= 0.85, "mean ratio {mean:.3} <= 0.85");
}
