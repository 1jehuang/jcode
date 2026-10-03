//! SWE-Pruner slim-composition session measurement (exec/pruner-slim).
//!
//! Same wired path, sessions, and token accounting as
//! `pruner_session_ab.rs` (scripted multi-turn sessions, REAL tool use),
//! but the second arm runs `JCODE_PRUNER_MODE=slim` (byte-min composition:
//! the send view is the shorter of the pruned display copy and the full
//! substitution) instead of `on`.
//!
//! The `on`-mode assertions of the A/B file do not transfer: a slim view
//! that sends the pruned copy alone carries NO `[jcode-retention:` marker,
//! so this harness joins cleared ids on the OFF run's grammar-matched
//! substitution views and gates per view KIND: fallback (substitution-
//! carrying) slim views must equal the OFF substitution EXACTLY after
//! collapsing the two per-arm random path segments (temp-dir name +
//! session id) to same-length tokens; pruned-alone views must stay within
//! a documented sid-lottery tolerance of the OFF rendering (raw chars).
//! Session totals are compared RAW (exactly what the provider is charged
//! for, same accounting as `pruner_session_ab.rs`).
//!
//! Assertions per session x window: stored offload bytes identical across
//! modes (task fixed), dispatch + DONE success in both arms, session
//! tokens within lottery tolerance of OFF, every stored error line of
//! every pruned-alone view present verbatim, cue+ref intact on fallback
//! views, pruner markers present on pruned-alone views, `todo` bypass
//! verbatim, and exact inertness with clearing disabled.

use std::collections::{HashMap, HashSet};
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
        "session-slim-script"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn tokens_for_text(text: &str) -> usize {
    (text.chars().count() + 3) / 4
}

/// Provider-bound tokens of one captured request: every message, every
/// content block that reaches the wire.
fn request_tokens(messages: &[Message]) -> usize {
    let mut chars = 0usize;
    for msg in messages {
        for block in &msg.content {
            match block {
                ContentBlock::Text { text, .. } => chars += text.chars().count(),
                ContentBlock::ToolResult { content, .. } => {
                    chars += content.chars().count()
                }
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
    let needle =
        |w: &[u8]| bytes.windows(w.len()).any(|win| win.eq_ignore_ascii_case(w));
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
/// requirements exclude them.
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

/// Outcome of one session run (one mode, one window).
struct SessionRun {
    /// Total provider-bound message tokens over ALL requests (raw bytes —
    /// the headline metric, same accounting as `pruner_session_ab.rs`).
    total_tokens: usize,
    /// Per-request tokens, in order.
    per_request: Vec<usize>,
    /// Complete stored bytes recovered from offload files as
    /// (stem, bytes) pairs (the task-fixed source of truth).
    offloaded: Vec<(String, String)>,
    /// LAST ToolResult view per tool_use_id across ALL requests (clearing
    /// is monotonic — once a message clears it stays cleared — so
    /// last-wins is the most-cleared state per id).
    last_view_by_id: HashMap<String, String>,
    /// Full (never-cleared) ToolResult views by tool_use_id.
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
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
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

    // Last ToolResult view per id (most-cleared state) + never-cleared set.
    let mut last_view_by_id: HashMap<String, String> = HashMap::new();
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
                    last_view_by_id.insert(tool_use_id.clone(), content.clone());
                    if !content.contains("[jcode-retention:") {
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
        offloaded: read_offloaded_results(home),
        last_view_by_id,
        full_by_id,
        answer,
    }
}

/// Normalize the TWO per-arm random path segments out of a send-view
/// string: the temp-dir name (`.tmpXXXXXX`, from `TempDir::new()`) and the
/// random session-id offload subdirectory (`session_<word>_<ms>_<16hex>` —
/// timestamp and rand are fixed-width, the memorable word spans 2-9
/// chars). Both collapse to SAME-LENGTH fixed tokens, so normalized char
/// counts stay directly comparable to raw counts, and
/// substitution-carrying views over identical stored bytes tie EXACTLY
/// (same chunks → same cues → same text) — the fallback gate relies on
/// this.
///
/// The surrounding home prefix is deliberately NOT normalized: both arms
/// use `TempDir::new()` in one process, so it is identical text in both
/// arms, and its bytes are genuine provider-bound bytes (as are the
/// sid/tmpdir bytes — the model needs the real path to re-read).
fn normalize_view(view: &str, run_home: &std::path::Path) -> String {
    let mut v = view.to_string();
    // Collapse the arm's temp-dir name to a same-length fixed token
    // (`<TMPDIRXX>` is 10 chars, exactly `.tmpXXXXXX`). Occurrence counts
    // are identical for identical substitutions, so fallback views
    // equalize exactly while char counts stay comparable.
    if let Some(tmp_name) = run_home.file_name().and_then(|n| n.to_str()) {
        v = v.replace(tmp_name, "<TMPDIRXX>");
    }
    // Collapse `sessions/offloaded/<random-sid>/` to a same-length-ish
    // fixed segment (operates on the tmpdir-collapsed string above).
    let marker = "sessions/offloaded/";
    let mut out = String::with_capacity(v.len());
    let mut rest = v.as_str();
    while let Some(pos) = rest.find(marker) {
        out.push_str(&rest[..pos + marker.len()]);
        rest = &rest[pos + marker.len()..];
        match rest.find('/') {
            Some(end) => {
                out.push_str("<SID>/");
                rest = &rest[end + 1..];
            }
            None => {
                out.push_str(rest);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
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
/// Identical plans to `pruner_session_ab.rs`.
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
        let temp = Box::leak(Box::new(
            tempfile::TempDir::new().expect("temp dir"),
        ));
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
            Some(w) => jcode_app_core::env::set_var(
                "JCODE_COMPACTION_CLEAR_TOOL_RESULTS_OLDER_THAN",
                w,
            ),
            None => jcode_app_core::env::remove_var("JCODE_COMPACTION_CLEAR_TOOL_RESULTS_OLDER_THAN"),
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
        if let Some(stamps) = self.stamps.take() {
            jcode_app_core::env::set_var("JCODE_MESSAGE_TIMESTAMPS", stamps);
        } else {
            jcode_app_core::env::remove_var("JCODE_MESSAGE_TIMESTAMPS");
        }
        jcode_app_core::config::Config::invalidate_cache();
    }
}

#[tokio::test]
async fn pruner_session_slim_tokens_per_completion() {
    let _guard = jcode_app_core::storage::lock_test_env();
    let root = repo_root();
    let plans = session_plans(&root);

    // Row: (session, window, off_tokens, slim_tokens, stored_equal,
    //       success_off, success_slim)
    let mut rows: Vec<String> = Vec::new();
    // Per-cleared-result ledger (keep=1): normalized slim vs off view chars.
    let mut ledger: Vec<String> = Vec::new();
    let mut err_kept_slim = 0usize;
    let mut err_total_slim = 0usize;
    let mut cue_ok = 0usize;
    let mut cue_total = 0usize;
    let mut pruned_alone = 0usize;
    let mut strictly_smaller_ids: HashSet<String> = HashSet::new();
    // Sid-lottery tolerance (chars): the OFF and SLIM arms get different
    // random session ids (`session_<word>_<ms>_<16hex>`, words span 2-9
    // chars), each embedded ~2x per offload chunk + once in expand. Bounds
    // the cross-arm rendering wobble of IDENTICAL substitutions with room
    // to spare (~120 worst case) while still catching any structural
    // blowup (the on-wire added thousands of chars per result).
    const SID_LOTTERY_TOLERANCE: usize = 256;

    for window in [Some("1"), Some("10"), None] {
        let window_label = window.unwrap_or("none");
        for (session, plan, prompts) in &plans {
            let prompt_refs: Vec<&str> = prompts.iter().map(String::as_str).collect();
            // OFF first, then SLIM — back-to-back so fodder files cannot
            // change between the paired runs.
            let env_off = EnvGuard::set(window, None);
            let off = run_session(session, plan.clone(), &prompt_refs, &env_off.run_home).await;
            let off_home = env_off.run_home.clone();
            drop(env_off);
            let env_slim = EnvGuard::set(window, Some("slim"));
            let slim =
                run_session(session, plan.clone(), &prompt_refs, &env_slim.run_home).await;
            let slim_home = env_slim.run_home.clone();
            drop(env_slim);

            // Task-fixed check (source of truth): complete stored bytes
            // recovered from the offload files, byte-identical across modes.
            let stored_equal = off.offloaded == slim.offloaded;
            // Dispatch check: each tool turn = 2 provider requests (call +
            // post-execution re-request), final answer = 1.
            let n_calls = plan
                .iter()
                .filter(|t| matches!(t, TurnPlan::Call { .. }))
                .count();
            let expect_reqs = 2 * n_calls + 1;
            let dispatch_ok = off.per_request.len() == expect_reqs
                && slim.per_request.len() == expect_reqs;
            // Success: full dispatch, no turn errored (run_session panics
            // on turn error), final answer carries the scripted DONE marker.
            let success_off = dispatch_ok && off.answer.contains("DONE");
            let success_slim = dispatch_ok && slim.answer.contains("DONE");

            // Headline session totals are RAW provider-bound tokens (same
            // accounting as `pruner_session_ab.rs` — exactly what the
            // provider is charged for, sid-lottery noise included).
            let ratio = slim.total_tokens as f64 / off.total_tokens.max(1) as f64;

            // Per-cleared-result join: OFF cleared ids (grammar-matched
            // substitution views) are the ground truth of WHAT cleared;
            // the slim view for the same id must be byte-shorter-or-equal
            // after normalization (byte-min composition: min(P, S) <= S).
            let off_cleared: HashMap<String, String> = off
                .last_view_by_id
                .iter()
                .filter(|(_, view)| is_cleared_view(view))
                .map(|(id, view)| (id.clone(), view.clone()))
                .collect();
            let mut stored_map: HashMap<&str, &str> = HashMap::new();
            for (stem, bytes) in &slim.offloaded {
                stored_map.insert(stem.as_str(), bytes.as_str());
            }
            let mut cleared_ids: Vec<String> = off_cleared.keys().cloned().collect();
            cleared_ids.sort();
            for stem in &cleared_ids {
                let off_view = &off_cleared[stem];
                let slim_view = slim.last_view_by_id.get(stem).unwrap_or_else(|| {
                    panic!("{session} keep={window_label}: id {stem} cleared OFF but missing in SLIM run")
                });
                let off_n = normalize_view(off_view, &off_home);
                let slim_n = normalize_view(slim_view, &slim_home);
                // Three-way gate on the view KIND (determined by whether
                // the slim view carries the substitution):
                // - fallback (substitution-carrying): identical stored
                //   bytes chunk identically and cues derive from content,
                //   so after sid normalization the views tie EXACTLY
                //   (deterministic, zero tolerance);
                // - pruned-alone: the wire guarantees slim_raw <= the
                //   SLIM arm's own substitution rendering; the OFF arm's
                //   rendering differs only by the sid lottery, bounded by
                //   SID_LOTTERY_TOLERANCE.
                let slim_raw = slim_view.chars().count();
                let off_raw = off_view.chars().count();
                let is_fallback = slim_n.contains("[jcode-retention:offloaded");
                if is_fallback {
                    assert_eq!(
                        slim_n, off_n,
                        "{session} keep={window_label} {stem}: fallback slim view must equal off substitution (sid-normalized)",
                    );
                } else {
                    assert!(
                        slim_raw <= off_raw + SID_LOTTERY_TOLERANCE,
                        "{session} keep={window_label} {stem}: slim view ({slim_raw} chars) exceeds off view ({off_raw} chars) beyond sid-lottery tolerance",
                    );
                    if slim_raw < off_raw {
                        strictly_smaller_ids.insert(format!("{session} keep={window_label} {stem}"));
                    }
                }
                if window == Some("1") {
                    let stored_len =
                        stored_map.get(stem.as_str()).map(|s| s.len()).unwrap_or(0);
                    // Raw view chars (provider-billed) + kind flag.
                    ledger.push(format!(
                        "{session} {stem} stored={stored_len} slim_view={slim_raw} off_view(subst_only)={off_raw} pruned_alone={}",
                        !is_fallback,
                    ));
                }
                // Integrity on the slim view, joined to COMPLETE stored
                // bytes. Two view kinds, two invariants:
                // - pruned-alone (no substitution): the scorer guarantees
                //   error lines verbatim, so every stored error line must
                //   be present in the view;
                // - fallback (substitution-carrying): byte-identical to
                //   OFF modulo paths, so error lines live on disk + in
                //   session history exactly as they do with the pruner
                //   off — recovery via the intact cue+ref (asserted
                //   below), never verbatim in the view.
                if !is_fallback {
                    if let Some(stored) = stored_map.get(stem.as_str()) {
                        for line in stored.lines().filter(|l| is_error_line(l)) {
                            err_total_slim += 1;
                            if slim_view.contains(line) {
                                err_kept_slim += 1;
                            } else {
                                eprintln!("  ERROR-LINE DROPPED in {session} {stem}: {line:?}");
                            }
                        }
                    }
                }
                // Cue accounting: substitution-carrying (fallback) slim
                // views must keep cue+ref intact; pruned-alone views must
                // carry pruner markers (proof the scorer acted, not a
                // bypass or a no-op).
                if is_fallback {
                    cue_total += 1;
                    if slim_n.contains("read file_path=") && slim_n.contains("expand:") {
                        cue_ok += 1;
                    }
                } else {
                    pruned_alone += 1;
                    assert!(
                        slim_n.contains("(filtered "),
                        "{session} keep={window_label} {stem}: pruned-alone view without filter markers"
                    );
                }
            }

            rows.push(format!(
                "{session} keep={window_label} off={} ({} reqs) slim={} ({} reqs) ratio={ratio:.3} stored_equal={stored_equal} success_off={success_off} success_slim={success_slim}",
                off.total_tokens,
                off.per_request.len(),
                slim.total_tokens,
                slim.per_request.len(),
            ));
            eprintln!(
                "session-slim {session} keep={window_label}: off={} slim={} ratio={ratio:.3} stored_equal={stored_equal} ok={success_off}/{success_slim}",
                off.total_tokens, slim.total_tokens,
            );
            assert!(
                stored_equal,
                "{session} keep={window_label}: stored results diverged between modes"
            );
            assert!(
                success_off && success_slim,
                "{session} keep={window_label}: task success off={success_off} slim={success_slim}"
            );
            // Session totals are compared RAW (sid-normalized totals would
            // understate substitution-carrying views, which is most of
            // them). The sid lottery contributes at most ~120 chars per
            // cleared result; results that never clear contribute zero
            // (no offload paths in their views). Tolerance scales with the
            // cleared-id count so the gate stays tight on small sessions
            // while never flaking on the lottery.
            let session_tolerance = cleared_ids.len() * SID_LOTTERY_TOLERANCE;
            assert!(
                slim.total_tokens <= off.total_tokens + session_tolerance / 4,
                "{session} keep={window_label}: slim session tokens ({}) exceed off ({}) beyond sid-lottery tolerance",
                slim.total_tokens,
                off.total_tokens,
            );
            if window.is_none() {
                // Clearing disabled: the pruner can never act, so the runs
                // must be byte-identical in provider-bound tokens (raw —
                // no clearing means no offload paths in any view, so even
                // the random temp-home noise cannot appear; if it does,
                // the equality below pinpoints a leak).
                assert_eq!(
                    off.total_tokens, slim.total_tokens,
                    "{session} keep=none: slim changed bytes with clearing disabled"
                );
                assert!(
                    off_cleared.is_empty(),
                    "{session} keep=none: cleared views with clearing disabled"
                );
            }
            // Bypass check (s3 carries a real `todo` call: `classify_tool`
            // bypasses it, so the slim-mode send view must carry its output
            // verbatim with no pruner markers). The todo output is short
            // (under the 200-char wire floor), so it is never offloaded and
            // never cleared — it rides full in every request of both runs.
            if session == "s3-mixed-bypass" {
                let todo_id = format!("{session}-t2");
                assert!(
                    !off_cleared.contains_key(&todo_id),
                    "s3 keep={window_label}: todo result cleared in OFF run"
                );
                assert!(
                    !is_cleared_view(
                        slim.last_view_by_id.get(&todo_id).map(String::as_str).unwrap_or("")
                    ),
                    "s3 keep={window_label}: todo result pruned/cleared in SLIM run (bypass broken)"
                );
                // Verbatim in both modes, byte-identical, marker-free.
                let off_full = off.full_by_id.get(&todo_id).unwrap_or_else(|| {
                    panic!("s3 keep={window_label}: no full todo view in OFF run")
                });
                let slim_full = slim.full_by_id.get(&todo_id).unwrap_or_else(|| {
                    panic!("s3 keep={window_label}: no full todo view in SLIM run")
                });
                assert_eq!(
                    off_full, slim_full,
                    "s3 keep={window_label}: todo bytes differ between modes"
                );
                assert!(
                    !slim_full.contains("(filtered "),
                    "s3 keep={window_label}: pruner markers on bypass tool output"
                );
            }
        }
    }

    eprintln!("pruner session SLIM vs OFF (raw provider-bound message tokens per completion):");
    for row in &rows {
        eprintln!("  {row}");
    }
    eprintln!("cleared-view ledger (keep=1, raw view chars vs paired off substitution-only):");
    for line in &ledger {
        eprintln!("  {line}");
    }
    eprintln!(
        "slim cleared-view integrity: cue+ref {cue_ok}/{cue_total} on fallback views, pruned-alone {pruned_alone}, error-lines-kept {err_kept_slim}/{err_total_slim}, strictly-smaller ids {}",
        strictly_smaller_ids.len()
    );
    for id in &strictly_smaller_ids {
        eprintln!("  strictly smaller: {id}");
    }

    // Rollout-relevant assertions (session level, not per-item):
    // every fallback slim view kept cue+ref, every stored error line of
    // every PRUNED-ALONE slim view survived verbatim (fallback views carry
    // the recovery ref instead, exactly as OFF does), and at least one
    // result went pruned-alone (the wire actually exercises the new
    // composition — a run where slim fell back to substitution everywhere
    // would prove nothing beyond byte-equality with OFF).
    assert_eq!(cue_ok, cue_total, "cue+ref intact in all fallback slim views");
    assert_eq!(
        err_kept_slim, err_total_slim,
        "all stored error lines kept in slim views"
    );
    assert!(
        pruned_alone > 0,
        "slim must send at least one pruned-alone view (else the composition is untested)"
    );
    assert!(
        !strictly_smaller_ids.is_empty(),
        "slim must beat off on at least one cleared result"
    );
    // Paired stored-result equality already asserted per session above.
    let _ = HashMap::<String, String>::new();
}
