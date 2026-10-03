//! SWE-Pruner session-level A/B (exec/pruner-session-ab).
//!
//! Drives the WIRED path (`Agent::run_once_capture` -> `run_turn` ->
//! `messages_for_provider` -> `apply_tool_result_clearing` ->
//! `pruner_prune_display`) through scripted MULTI-TURN sessions with REAL
//! tool use (`read` over real repo files, `agentgrep` over the real repo,
//! one real `todo` bypass call), each session run twice with only
//! `JCODE_PRUNER_MODE` flipped (off, then on).
//!
//! Metric: provider-bound message tokens per task completion = sum over ALL
//! captured `complete()` requests of `ceil(chars/4)` over message content,
//! exactly the bytes the provider would be charged for. The scripted
//! provider captures every request's messages, so substitution text, cues,
//! refs, pruned copies, AND `(filtered N lines)` markers all count —
//! overhead cannot hide.
//!
//! Determinism: no live model. The scripted provider emits a fixed turn
//! plan; tool execution is real and local. On/off runs of the same session
//! must see byte-identical STORED tool results (asserted) — the only
//! allowed difference is the send view produced by the clearing path.
//!
//! Two clearing windows: keep=1 (every follow-up request exercises the
//! clearing path; primary signal) and keep=10 (live-like window;
//! sensitivity check that the direction holds).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use jcode_app_core::agent::Agent;
use jcode_app_core::message::{ContentBlock, Message, StreamEvent, ToolDefinition};
use jcode_app_core::provider::{EventStream, Provider};
use jcode_app_core::tool::Registry;

/// One scripted turn: a real tool call, or the final answer text.
#[derive(Clone)]
enum TurnPlan {
    Call {
        name: String,
        input: serde_json::Value,
    },
    Answer(String),
}

/// Scripted provider: odd-numbered `complete` calls emit the next planned
/// turn action; even-numbered calls (post-tool-execution re-requests within
/// the same turn) end the turn with a short ack. The final Answer turn
/// emits text with end_turn, so no re-request follows.
#[derive(Clone)]
struct SessionScriptProvider {
    plan: Vec<TurnPlan>,
    session: String,
    calls: Arc<std::sync::Mutex<usize>>,
    /// Every provider-bound request, in order (the token-accounting basis).
    seen: Arc<std::sync::Mutex<Vec<Vec<Message>>>>,
}

#[async_trait]
impl Provider for SessionScriptProvider {
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
        let call_no = *n;
        drop(n);
        // Turn index for odd calls: call 1 -> turn 0, call 3 -> turn 1, ...
        let events = if call_no % 2 == 1 {
            let turn_idx = (call_no - 1) / 2;
            match self.plan.get(turn_idx) {
                Some(TurnPlan::Call { name, input }) => {
                    let call_id = format!("{}-t{turn_idx}", self.session);
                    vec![
                        StreamEvent::ToolUseStart {
                            id: call_id.clone(),
                            name: name.clone(),
                        },
                        StreamEvent::ToolInputDelta(input.to_string()),
                        StreamEvent::ToolUseEnd,
                        StreamEvent::MessageEnd {
                            stop_reason: Some("tool_use".to_string()),
                        },
                    ]
                }
                Some(TurnPlan::Answer(text)) => vec![
                    StreamEvent::TextDelta(text.clone()),
                    StreamEvent::MessageEnd {
                        stop_reason: Some("end_turn".to_string()),
                    },
                ],
                None => panic!("{}: script exhausted at call {call_no}", self.session),
            }
        } else {
            vec![
                StreamEvent::TextDelta("noted.".to_string()),
                StreamEvent::MessageEnd {
                    stop_reason: Some("end_turn".to_string()),
                },
            ]
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
    fn name(&self) -> &str {
        "session-ab-script"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn tokens_for_text(text: &str) -> usize {
    (text.chars().count() + 3) / 4
}

/// Provider-bound tokens of one captured request: every message, every
/// content block that reaches the wire (text, tool-result content incl.
/// substitutions/cues/pruned copies/markers, tool-use name+input).
fn request_tokens(messages: &[Message]) -> usize {
    let mut chars = 0usize;
    for msg in messages {
        for block in &msg.content {
            match block {
                ContentBlock::Text { text, .. } => chars += text.chars().count(),
                ContentBlock::ToolResult { content, .. } => chars += content.chars().count(),
                ContentBlock::ToolUse { name, input, .. } => {
                    chars += name.chars().count() + input.to_string().chars().count();
                }
                ContentBlock::Image { data, .. } => chars += data.len() / 4,
                _ => {}
            }
        }
    }
    tokens_for_text(&"x".repeat(chars))
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

/// Whether a send-view ToolResult is a GENUINELY cleared view (produced by
/// `apply_tool_result_clearing`), as opposed to full fodder that merely
/// MENTIONS the marker text (s1 reads agent.rs, whose source shows the
/// substitution format string; s2 greps for it).
///
/// Rule: the substitution GRAMMAR `[jcode-retention:offloaded was <digits>
/// chars, ...` occurs at a LINE START, with `expand: ` present. Fodder
/// occurrences always show the Rust placeholders (`was {was} chars`) and
/// sit mid-line inside string literals, so the digit + line-start
/// requirements exclude them. A genuinely cleared view always differs
/// from the stored bytes (substitution-only, or pruned+substitution —
/// the wire never returns the input unchanged), so no byte comparison
/// against stored/offload truth is needed (which also dodges the
/// chunker's trailing-newline normalization).
fn is_cleared_view(view: &str) -> bool {
    let needle = "[jcode-retention:offloaded was ";
    let mut start = 0;
    while let Some(pos) = view[start..].find(needle) {
        let abs = start + pos;
        let at_line_start = abs == 0 || view[..abs].ends_with('\n');
        let digits_start = abs + needle.len();
        let digits_len = view[digits_start..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .map(char::len_utf8)
            .sum::<usize>();
        if at_line_start
            && digits_len > 0
            && view[digits_start + digits_len..].starts_with(" chars,")
            && view.contains("expand: ")
        {
            return true;
        }
        start = abs + 1;
    }
    false
}

/// Byte offset of the substitution block inside a cleared view: the
/// first grammar-matching `[jcode-retention:offloaded was <digits> chars,`
/// occurrence at a line start (see [`is_cleared_view`]). Splitting on the
/// bare marker prefix would cut inside pruned heads whose fodder mentions
/// the marker text (s1 reads agent.rs source).
fn substitution_offset(view: &str) -> Option<usize> {
    let needle = "[jcode-retention:offloaded was ";
    let mut start = 0;
    while let Some(pos) = view[start..].find(needle) {
        let abs = start + pos;
        let at_line_start = abs == 0 || view[..abs].ends_with('\n');
        let digits_start = abs + needle.len();
        let digits_len = view[digits_start..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .map(char::len_utf8)
            .sum::<usize>();
        if at_line_start
            && digits_len > 0
            && view[digits_start + digits_len..].starts_with(" chars,")
        {
            return Some(abs);
        }
        start = abs + 1;
    }
    None
}

/// Genuinely cleared views per tool_use_id (grammar-filtered; see
/// [`is_cleared_view`]).
fn genuine_cleared_views(run: &SessionRun) -> HashMap<String, String> {
    run.cleared_by_id
        .iter()
        .filter(|(_, view)| is_cleared_view(view))
        .map(|(id, view)| (id.clone(), view.clone()))
        .collect()
}

/// Outcome of one session run (one mode, one window).
struct SessionRun {
    /// Total provider-bound message tokens over ALL requests.
    total_tokens: usize,
    /// Per-request tokens, in order.
    per_request: Vec<usize>,
    /// Stored (pre-clearing) tool-result bytes per tool call, in order.
    /// (Request-level set; partial under aggressive windows — see note.)
    stored_results: Vec<String>,
    /// Complete stored bytes recovered from offload files as
    /// (stem, bytes) pairs (the task-fixed source of truth).
    offloaded: Vec<(String, String)>,
    /// Cleared send views by tool_use_id: the FIRST `[jcode-retention:`-
    /// carrying ToolResult seen for each id across ALL requests (clearing
    /// is deterministic per content, so first-seen is representative; the
    /// last request alone is insufficient under wide windows where only
    /// the earliest results clear).
    cleared_by_id: HashMap<String, String>,
    /// Full (never-cleared) ToolResult views by tool_use_id: the FIRST
    /// non-marker content seen per id. Under-floor results (e.g. the s3
    /// `todo` output) live here in every request; bypass checks join on it.
    full_by_id: HashMap<String, String>,
    /// Final captured answer text.
    answer: String,
}

/// Complete stored tool-result bytes, recovered from the offload files
/// the clearing path wrote (`<home>/sessions/offloaded/*/<id>_cNN.txt`),
/// as (offload stem, bytes) pairs sorted by stem. The stem is the
/// sanitized tool_use_id (our call ids pass the sanitizer unchanged), so
/// pairs join directly to cleared views by id. The offload body is
/// `header + "--- result ---\n" + chunk`; chunks of one tool result are
/// concatenated in `_cNN` order.
fn read_offloaded_results(home: &std::path::Path) -> Vec<(String, String)> {
    let mut chunks: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    let offloaded = home.join("sessions").join("offloaded");
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|x| x == "txt") {
                out.push(path);
            }
        }
    }
    walk(&offloaded, &mut files);
    for path in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Stem looks like `<base>_cNN`. Base is the sanitized tool_use_id
        // PLUS a `_<8hex>` disambiguator the clearing path appends (our
        // call ids contain no underscores, so the last `_`-segment is
        // always the hash). Strip it to recover the join key.
        let (mut base, idx) = match name.rsplit_once("_c") {
            Some((b, rest)) => {
                let num: usize = rest.trim_end_matches(".txt").parse().unwrap_or(usize::MAX);
                (b.to_string(), num)
            }
            None => continue,
        };
        if let Some((pre, suf)) = base.rsplit_once('_') {
            if suf.len() == 8 && suf.chars().all(|c| c.is_ascii_hexdigit()) {
                base = pre.to_string();
            }
        }
        let body = match std::fs::read_to_string(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let chunk = match body.split_once("--- result ---\n") {
            Some((_, c)) => c.to_string(),
            None => continue,
        };
        chunks.entry(base).or_default().push((idx, chunk));
    }
    let mut bases: Vec<String> = chunks.keys().cloned().collect();
    bases.sort();
    bases
        .into_iter()
        .map(|base| {
            let mut v = chunks.remove(&base).unwrap_or_default();
            v.sort_by_key(|(idx, _)| *idx);
            let bytes = v.into_iter().map(|(_, c)| c).collect::<String>();
            (base, bytes)
        })
        .collect()
}

async fn run_session(
    session: &str,
    plan: Vec<TurnPlan>,
    user_prompts: &[&str],
    home: &std::path::Path,
) -> SessionRun {
    let seen: Arc<std::sync::Mutex<Vec<Vec<Message>>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let provider = SessionScriptProvider {
        plan: plan.clone(),
        session: session.to_string(),
        calls: Arc::new(std::sync::Mutex::new(0)),
        seen: seen.clone(),
    };
    let provider_arc: Arc<dyn Provider> = Arc::new(provider);
    // Full real registry: `read` / `agentgrep` / `todo` execute for real.
    let registry = Registry::new(provider_arc.clone()).await;
    let mut agent = Agent::new(provider_arc, registry);
    let mut answer = String::new();
    for prompt in user_prompts {
        answer = agent
            .run_once_capture(prompt)
            .await
            .unwrap_or_else(|e| panic!("{session}: turn failed: {e:?}"));
    }
    let seen = seen.lock().unwrap();
    let per_request: Vec<usize> = seen.iter().map(|m| request_tokens(m)).collect();
    let total_tokens = per_request.iter().sum();

    // Stored (pre-clearing) results: ToolResult blocks WITHOUT the
    // retention marker, first occurrence order across all requests.
    // NOTE: under aggressive windows (keep=1) a result can be cleared in
    // the very re-request that first carries it, so the full text may
    // NEVER appear in any request. The complete stored bytes are recovered
    // from the offload files instead (see read_offloaded_results); this
    // request-level set is kept only as a cross-check.
    let mut stored_results = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();
    for batch in seen.iter() {
        for msg in batch {
            for block in &msg.content {
                if let ContentBlock::ToolResult { content, .. } = block {
                    if !content.contains("[jcode-retention:") && seen_ids.insert(content.clone()) {
                        stored_results.push(content.clone());
                    }
                }
            }
        }
    }

    // Cleared views by tool_use_id: LAST marker-carrying ToolResult per id
    // across ALL requests. Last-wins matters: fodder can mention the marker
    // text (s1 reads agent.rs source), so the first marker view for an id
    // may be the full pre-clearing text; clearing is monotonic (once a
    // message clears it stays cleared), so the last marker view is the
    // most-cleared state — genuinely cleared if the id ever cleared, else
    // the full text (dropped by the grammar filter). Full (never-cleared)
    // views are collected in the same pass.
    let mut cleared_by_id: HashMap<String, String> = HashMap::new();
    let mut full_by_id: HashMap<String, String> = HashMap::new();
    for batch in seen.iter() {
        for msg in batch {
            for block in &msg.content {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } = block
                {
                    if content.contains("[jcode-retention:") {
                        cleared_by_id.insert(tool_use_id.clone(), content.clone());
                    } else {
                        full_by_id
                            .entry(tool_use_id.clone())
                            .or_insert_with(|| content.clone());
                    }
                }
            }
        }
    }
    SessionRun {
        total_tokens,
        per_request,
        stored_results,
        offloaded: read_offloaded_results(home),
        cleared_by_id,
        full_by_id,
        answer,
    }
}

/// Fodder root: absolute repo paths so `read`/`agentgrep` resolve with no
/// session working dir.
fn repo_root() -> String {
    // This test file lives in <root>/crates/jcode-app-core/tests/.
    let manifest = env!("CARGO_MANIFEST_DIR");
    let root = std::path::Path::new(manifest)
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root above crate dir")
        .to_string_lossy()
        .into_owned();
    root
}

fn read_call(file_path: &str) -> TurnPlan {
    TurnPlan::Call {
        name: "read".to_string(),
        input: serde_json::json!({"file_path": file_path}),
    }
}

fn grep_call(query: &str, path: &str) -> TurnPlan {
    TurnPlan::Call {
        name: "agentgrep".to_string(),
        input: serde_json::json!({"mode": "grep", "query": query, "path": path}),
    }
}

/// Four scripted sessions (task fixed; only the env knob varies).
fn session_plans(root: &str) -> Vec<(String, Vec<TurnPlan>, Vec<String>)> {
    let appcore = format!("{root}/crates/jcode-app-core/src");
    vec![
        (
            "s1-read-heavy".to_string(),
            vec![
                read_call(&format!("{appcore}/agent.rs")),
                read_call(&format!("{appcore}/agent/pruner.rs")),
                read_call(&format!("{appcore}/agent/turn_loops.rs")),
                TurnPlan::Answer(
                    "wiring: pruner_prune_display, apply_tool_result_clearing, messages_for_provider. DONE".to_string(),
                ),
            ],
            vec![
                "find the pruner wiring".to_string(),
                "read the scorer".to_string(),
                "read the turn loop".to_string(),
                "summarize".to_string(),
            ],
        ),
        (
            "s2-grep-heavy".to_string(),
            vec![
                grep_call("jcode-retention", &appcore),
                grep_call("offload_chunk_result", &appcore),
                grep_call("pruner", &appcore),
                TurnPlan::Answer(
                    "retention markers live in agent.rs clearing path. DONE".to_string(),
                ),
            ],
            vec![
                "find retention markers".to_string(),
                "find offload sites".to_string(),
                "find pruner refs".to_string(),
                "summarize".to_string(),
            ],
        ),
        (
            "s3-mixed-bypass".to_string(),
            vec![
                read_call(&format!("{appcore}/tool/agentgrep.rs")),
                grep_call("execute_linked_agentgrep", &appcore),
                TurnPlan::Call {
                    name: "todo".to_string(),
                    input: serde_json::json!({"todos": []}),
                },
                TurnPlan::Answer("mixed session complete. DONE".to_string()),
            ],
            vec![
                "read the grep tool".to_string(),
                "find its executor".to_string(),
                "check todos".to_string(),
                "summarize".to_string(),
            ],
        ),
        (
            "s4-error-lines".to_string(),
            vec![
                grep_call("panic", &appcore),
                grep_call("assert", &format!("{appcore}/agent/pruner.rs")),
                read_call(&format!("{appcore}/agent/pruner.rs")),
                TurnPlan::Answer("failure handling reviewed. DONE".to_string()),
            ],
            vec![
                "find panic sites".to_string(),
                "find asserts in scorer".to_string(),
                "read the scorer".to_string(),
                "summarize".to_string(),
            ],
        ),
    ]
}

struct EnvGuard {
    home: Option<std::ffi::OsString>,
    window: Option<std::ffi::OsString>,
    pruner: Option<std::ffi::OsString>,
    stamps: Option<std::ffi::OsString>,
    run_home: std::path::PathBuf,
}

impl EnvGuard {
    fn set(window: Option<&str>, pruner_mode: Option<&str>) -> Self {
        let temp = Box::leak(Box::new(tempfile::TempDir::new().expect("temp dir")));
        let run_home = temp.path().to_path_buf();
        let home = std::env::var_os("JCODE_HOME");
        let window_prev = std::env::var_os("JCODE_COMPACTION_CLEAR_TOOL_RESULTS_OLDER_THAN");
        let pruner_prev = std::env::var_os("JCODE_PRUNER_MODE");
        let stamps_prev = std::env::var_os("JCODE_MESSAGE_TIMESTAMPS");
        jcode_app_core::env::set_var("JCODE_HOME", temp.path());
        // Disable send-path timestamp stamping: `Message::with_timestamps`
        // would otherwise prepend `[tool timing: ...]` to SOME ToolResult
        // views (only messages carrying a timestamp get one), which makes
        // byte-equality joins ambiguous. With stamping off, captured bytes
        // are exactly what the clearing path produced.
        jcode_app_core::env::set_var("JCODE_MESSAGE_TIMESTAMPS", "false");
        match window {
            Some(w) => {
                jcode_app_core::env::set_var("JCODE_COMPACTION_CLEAR_TOOL_RESULTS_OLDER_THAN", w)
            }
            None => {
                jcode_app_core::env::remove_var("JCODE_COMPACTION_CLEAR_TOOL_RESULTS_OLDER_THAN")
            }
        }
        match pruner_mode {
            Some(v) => jcode_app_core::env::set_var("JCODE_PRUNER_MODE", v),
            None => jcode_app_core::env::remove_var("JCODE_PRUNER_MODE"),
        }
        jcode_app_core::config::Config::invalidate_cache();
        EnvGuard {
            home,
            window: window_prev,
            pruner: pruner_prev,
            stamps: stamps_prev,
            run_home,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(home) = self.home.take() {
            jcode_app_core::env::set_var("JCODE_HOME", home);
        } else {
            jcode_app_core::env::remove_var("JCODE_HOME");
        }
        if let Some(window) = self.window.take() {
            jcode_app_core::env::set_var("JCODE_COMPACTION_CLEAR_TOOL_RESULTS_OLDER_THAN", window);
        } else {
            jcode_app_core::env::remove_var("JCODE_COMPACTION_CLEAR_TOOL_RESULTS_OLDER_THAN");
        }
        if let Some(pruner) = self.pruner.take() {
            jcode_app_core::env::set_var("JCODE_PRUNER_MODE", pruner);
        } else {
            jcode_app_core::env::remove_var("JCODE_PRUNER_MODE");
        }
        if let Some(stamps) = self.stamps.take() {
            jcode_app_core::env::set_var("JCODE_MESSAGE_TIMESTAMPS", stamps);
        } else {
            jcode_app_core::env::remove_var("JCODE_MESSAGE_TIMESTAMPS");
        }
        jcode_app_core::config::Config::invalidate_cache();
    }
}

#[tokio::test]
async fn pruner_session_ab_tokens_per_completion() {
    let _guard = jcode_app_core::storage::lock_test_env();
    let root = repo_root();
    let plans = session_plans(&root);

    // Row: (session, window, off_tokens, on_tokens, off_reqs, on_reqs,
    //       stored_equal, success_off, success_on)
    let mut rows: Vec<String> = Vec::new();
    // Overhead ledger per cleared result (keep=1, on-mode):
    // stored_chars, on_view_chars, off_view_chars(substitution-only ref
    // from the paired off run is matched by session+order below).
    let mut overhead: Vec<String> = Vec::new();
    let mut err_kept_on = 0usize;
    let mut err_total_on = 0usize;
    let mut cue_ok = 0usize;
    let mut cue_total = 0usize;

    for window in [Some("1"), Some("10"), None] {
        let window_label = window.unwrap_or("none");
        for (session, plan, prompts) in &plans {
            let prompt_refs: Vec<&str> = prompts.iter().map(String::as_str).collect();
            // OFF first, then ON — back-to-back so fodder files cannot
            // change between the paired runs.
            let env_off = EnvGuard::set(window, None);
            let off = run_session(session, plan.clone(), &prompt_refs, &env_off.run_home).await;
            let off_home = env_off.run_home.clone();
            drop(env_off);
            let env_on = EnvGuard::set(window, Some("on"));
            let on = run_session(session, plan.clone(), &prompt_refs, &env_on.run_home).await;
            drop(env_on);
            let _ = off_home;

            // Task-fixed check (source of truth): complete stored bytes
            // recovered from the offload files, byte-identical across modes.
            let stored_equal = off.offloaded == on.offloaded;
            // Dispatch check: each tool turn = 2 provider requests (call +
            // post-execution re-request), final answer = 1.
            let n_calls = plan
                .iter()
                .filter(|t| matches!(t, TurnPlan::Call { .. }))
                .count();
            let expect_reqs = 2 * n_calls + 1;
            let dispatch_ok =
                off.per_request.len() == expect_reqs && on.per_request.len() == expect_reqs;
            // Success: full dispatch, no turn errored (run_session panics
            // on turn error), final answer carries the scripted DONE marker.
            let success_off = dispatch_ok && off.answer.contains("DONE");
            let success_on = dispatch_ok && on.answer.contains("DONE");

            // Genuine cleared views (marker text can also occur IN fodder;
            // see helper). Both runs clear the same ids: task fixed.
            let on_cleared = genuine_cleared_views(&on);
            let off_cleared = genuine_cleared_views(&off);
            // Cleared-view integrity on the ON run, joined BY
            // tool_use_id: each cleared view's cue+ref intact, and every
            // error line of that result's COMPLETE stored bytes (offload
            // truth) present in the pruned head. Results never cleared in
            // any request (wide window tail) are reported, not failed.
            let mut cleared_ids: Vec<String> = on_cleared.keys().cloned().collect();
            cleared_ids.sort();
            let mut stored_map: HashMap<&str, &str> = HashMap::new();
            for (stem, bytes) in &on.offloaded {
                stored_map.insert(stem.as_str(), bytes.as_str());
            }
            let mut never_cleared: Vec<String> = Vec::new();
            for (stem, stored) in &on.offloaded {
                match on_cleared.get(stem) {
                    None => never_cleared.push(stem.clone()),
                    Some(view) => {
                        cue_total += 1;
                        if view.contains("[jcode-retention:offloaded")
                            && view.contains("read file_path=")
                            && view.contains("expand:")
                        {
                            cue_ok += 1;
                        }
                        for line in stored.lines().filter(|l| is_error_line(l)) {
                            err_total_on += 1;
                            if view.contains(line) {
                                err_kept_on += 1;
                            } else {
                                eprintln!("  ERROR-LINE DROPPED in {session} {stem}: {line:?}");
                            }
                        }
                    }
                }
            }
            if !never_cleared.is_empty() {
                eprintln!("  note {session} keep={window_label}: never cleared: {never_cleared:?}");
            }

            // Overhead split (primary window only): per cleared result,
            // on-view chars vs paired off-view chars, joined by
            // tool_use_id (both runs clear the same ids: task fixed).
            if window == Some("1") {
                for stem in &cleared_ids {
                    let on_view = &on_cleared[stem];
                    let off_view = match off_cleared.get(stem) {
                        Some(v) => v.clone(),
                        None => {
                            eprintln!("  UNPAIRED cleared id {stem} in {session} (on but not off)");
                            continue;
                        }
                    };
                    let on_s: &str = on_view;
                    let off_s: &str = &off_view;
                    // Split the on-view at the grammar-anchored substitution
                    // offset (a bare-prefix split would cut inside pruned
                    // heads whose fodder mentions the marker text).
                    let (on_head, on_sub) = match substitution_offset(on_s) {
                        Some(off) => (on_s[..off].trim_end().len(), on_s[off..].len()),
                        None => (on_s.len(), 0),
                    };
                    let stored_len = stored_map.get(stem.as_str()).map(|s| s.len()).unwrap_or(0);
                    overhead.push(format!(
                        "{session} {stem} stored={stored_len} on_view={} (pruned_head={on_head} + subst={on_sub}) off_view(subst_only)={} markers_on={}",
                        on_s.len(),
                        off_s.len(),
                        on_s.contains("(filtered "),
                    ));
                }
            }

            let ratio = on.total_tokens as f64 / off.total_tokens.max(1) as f64;
            rows.push(format!(
                "{session} keep={window_label} off={} ({} reqs) on={} ({} reqs) ratio={ratio:.3} stored_equal={stored_equal} success_off={success_off} success_on={success_on}",
                off.total_tokens,
                off.per_request.len(),
                on.total_tokens,
                on.per_request.len(),
            ));
            eprintln!(
                "session-ab {session} keep={window_label}: off={} on={} ratio={ratio:.3} stored_equal={stored_equal} ok={success_off}/{success_on}",
                off.total_tokens, on.total_tokens,
            );
            assert!(
                stored_equal,
                "{session} keep={window_label}: stored results diverged between modes"
            );
            assert!(
                success_off && success_on,
                "{session} keep={window_label}: task success off={success_off} on={success_on}"
            );
            if window.is_none() {
                // Clearing disabled: the pruner can never act, so the runs
                // must be byte-identical in provider-bound tokens.
                assert_eq!(
                    off.total_tokens, on.total_tokens,
                    "{session} keep=none: pruner changed bytes with clearing disabled"
                );
                assert!(
                    on_cleared.is_empty(),
                    "{session} keep=none: cleared views with clearing disabled"
                );
            }
            // Bypass check (s3 carries a real `todo` call: `classify_tool`
            // bypasses it, so the on-mode send view must carry its output
            // verbatim with no pruner markers). The todo output is short
            // (under the 200-char wire floor), so it is never offloaded and
            // never cleared — it rides full in every request of both runs.
            if session == "s3-mixed-bypass" {
                let todo_id = format!("{session}-t2");
                // Not cleared in either mode (no marker-carrying view).
                assert!(
                    !genuine_cleared_views(&off).contains_key(&todo_id),
                    "s3 keep={window_label}: todo result cleared in OFF run"
                );
                assert!(
                    !on_cleared.contains_key(&todo_id),
                    "s3 keep={window_label}: todo result pruned/cleared in ON run (bypass broken)"
                );
                // Verbatim in both modes, byte-identical, marker-free.
                let off_full = off.full_by_id.get(&todo_id).unwrap_or_else(|| {
                    panic!("s3 keep={window_label}: no full todo view in OFF run")
                });
                let on_full = on.full_by_id.get(&todo_id).unwrap_or_else(|| {
                    panic!("s3 keep={window_label}: no full todo view in ON run")
                });
                assert_eq!(
                    off_full, on_full,
                    "s3 keep={window_label}: todo bytes differ between modes"
                );
                assert!(
                    !on_full.contains("(filtered "),
                    "s3 keep={window_label}: pruner markers on bypass tool output"
                );
            }
        }
    }

    eprintln!("pruner session A/B (provider-bound message tokens per completion):");
    for row in &rows {
        eprintln!("  {row}");
    }
    eprintln!("cleared-view overhead (keep=1, on-mode views vs paired off substitution-only):");
    for line in &overhead {
        eprintln!("  {line}");
    }
    eprintln!(
        "on-mode cleared-view integrity: cue+ref {cue_ok}/{cue_total}, error-lines-kept {err_kept_on}/{err_total_on}"
    );

    // Rollout-relevant assertions (session level, not per-item):
    // every cleared view kept cue+ref, every stored error line survived.
    assert_eq!(
        cue_ok, cue_total,
        "cue+ref intact in all on-mode cleared views"
    );
    assert_eq!(
        err_kept_on, err_total_on,
        "all stored error lines kept in on-mode cleared views"
    );
    // Paired stored-result equality already asserted per session above.
    let _ = HashMap::<String, String>::new();
}
