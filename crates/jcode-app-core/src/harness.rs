//! Harness execution enforcement: pre-turn skill routing, one ephemeral
//! per-request policy block, a bounded turn-completion gate, and phase /
//! prerequisite gates for multi-step data tools.
//!
//! State is per session and process-wide (like `tool::sdk`) so the agent loop
//! and `Registry::execute` (which also serves `codemode` and `batch`
//! subcalls) see the same picture for native and MCP tools alike.

use crate::resilience::{Admission, CircuitBreaker, RecoveryReason, classify_interruption};
use crate::skill::{Skill, SkillRegistry};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

pub const MAX_INTERVENTIONS: u32 = 2;
const MAX_AUTO_ROUTED: usize = 3;
const MIN_ROUTE_SCORE: usize = 2;
/// Internal resume/reload/compaction messages are not user intent.
const SYSTEM_PREFIXES: &[&str] = &[
    "[resume",
    "[reload",
    "[system",
    "## Previous Conversation Summary",
    "<system-reminder>",
];
/// Generic words never route on their own (as a tag or a skill name).
const STOP_WORDS: &[&str] = &[
    "build",
    "do",
    "background",
    "server",
    "file",
    "files",
    "code",
    "run",
    "check",
    "test",
    "tests",
    "update",
    "fix",
    "make",
    "plan",
    "release",
    "deploy",
    "install",
    "setup",
    "init",
    "talk",
    "hero",
    "design",
    "data",
    "search",
    "review",
    "debug",
    "web",
    "page",
    "image",
    "video",
    "text",
    "api",
    "tool",
    "tools",
    "skill",
    "skills",
    "log",
    "logs",
    "git",
    "app",
    "ui",
    "doc",
    "docs",
    "report",
    "audit",
    "memory",
    "understand",
];
const MAX_CONSTRAINT_LINES: usize = 8;

pub const CORE_INVARIANT: &str = "ACTION OVER MEMORY: Never answer factual, codebase, system-state, or data questions from memory. Every claim must be verified via tool calls executed in this session. Do not stop after initial discovery; complete the full verification/action workflow.";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TurnTelemetry {
    pub turn_id: u64,
    pub actionable: bool,
    pub auto_routed_skills: Vec<String>,
    pub tools_called: Vec<String>,
    pub gate_blocks: u32,
    pub stop_interventions: u32,
}

#[derive(Debug, Default)]
struct SessionState {
    active_skills: BTreeSet<String>,
    /// Skills whose full body reached the model and has not been compacted away.
    delivered: HashSet<String>,
    bound_tools: BTreeSet<String>,
    constraints: HashMap<String, Vec<String>>,
    inspected_targets: HashSet<String>,
    turn: TurnTelemetry,
    /// Per-tool run brake: repeated failures stop retry storms.
    breakers: HashMap<String, CircuitBreaker>,
}

static STATE: LazyLock<Mutex<HashMap<String, SessionState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_TURN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn with<R>(session: &str, f: impl FnOnce(&mut SessionState) -> R) -> R {
    let mut map = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(map.entry(session.to_string()).or_default())
}

pub fn enabled() -> bool {
    // Unit tests script text-only turns; opt in explicitly there.
    if cfg!(test) {
        return std::env::var("JCODE_HARNESS_ENFORCEMENT").is_ok_and(|v| v == "1");
    }
    crate::config::config().features.harness_enforcement
}

/// Drop all state for a session (tests, session teardown).
pub fn reset(session: &str) {
    STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(session);
}

// ---------------------------------------------------------------- skills

/// Extract `## HARD CONSTRAINTS` bullets (section runs to the next `## ` or EOF).
pub fn hard_constraints(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with("## ") {
            inside = t[3..].trim().eq_ignore_ascii_case("hard constraints");
            continue;
        }
        if inside && let Some(item) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            out.push(item.trim().to_string());
        }
    }
    out
}

fn contains_phrase(haystack: &str, needle: &str) -> bool {
    let needle = needle.trim().to_lowercase();
    if needle.is_empty() {
        return false;
    }
    haystack.match_indices(&needle).any(|(i, _)| {
        let before = haystack[..i].chars().next_back();
        let after = haystack[i + needle.len()..].chars().next();
        let boundary = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric() && c != '-');
        boundary(before) && boundary(after)
    })
}

/// Skills whose `triggers` (or exact name) appear in the prompt. Descriptions
/// are deliberately not matched: with hundreds of installed skills they
/// route nearly everything.
/// Multi-word phrases and a non-generic skill name weigh 2, lone words 1,
/// generic words 0, so one incidental tag never routes alone.
fn route_score(text: &str, name: &str, triggers: &[String]) -> usize {
    let generic = |t: &str| STOP_WORDS.contains(&t.trim().to_lowercase().as_str());
    let tags: usize = triggers
        .iter()
        .filter(|t| contains_phrase(text, t))
        .map(|t| {
            if t.trim().contains(' ') {
                2
            } else if generic(t) {
                0
            } else {
                1
            }
        })
        .sum();
    tags + if !generic(name) && contains_phrase(text, name) {
        2
    } else {
        0
    }
}

fn is_system_message(prompt: &str) -> bool {
    let head = prompt.trim_start();
    SYSTEM_PREFIXES.iter().any(|p| head.starts_with(p))
}

pub fn route_skills(prompt: &str, skills: &SkillRegistry) -> Vec<String> {
    if is_system_message(prompt) {
        return Vec::new();
    }
    let text = prompt.to_lowercase();
    let mut hits: Vec<(usize, String)> = skills
        .list()
        .into_iter()
        .filter_map(|s| {
            let score = route_score(&text, &s.name, &s.triggers);
            (score >= MIN_ROUTE_SCORE).then(|| (score, s.name.clone()))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.into_iter()
        .take(MAX_AUTO_ROUTED)
        .map(|(_, n)| n)
        .collect()
}

/// Mark a skill active and bind its required tools. Returns those tools.
pub fn activate(session: &str, skill: &Skill) -> Vec<String> {
    with(session, |s| {
        s.active_skills.insert(skill.name.clone());
        s.constraints
            .insert(skill.name.clone(), hard_constraints(&skill.content));
        s.bound_tools.extend(skill.required_tools.iter().cloned());
    });
    skill.required_tools.clone()
}

/// `skill_manage load` bookkeeping. Returns true when the body was already
/// delivered this session, so the caller can answer with a short ack.
pub fn note_skill_load(session: &str, skill: &Skill) -> bool {
    activate(session, skill);
    with(session, |s| !s.delivered.insert(skill.name.clone()))
}

/// Compaction may drop delivered bodies; let the next load resend them.
pub fn on_compaction(session: &str) {
    with(session, |s| s.delivered.clear());
}

pub fn active_skills(session: &str) -> Vec<String> {
    with(session, |s| s.active_skills.iter().cloned().collect())
}

// ---------------------------------------------------------------- turn

/// Chit-chat, greetings and acknowledgements do not need tools.
pub fn is_actionable(prompt: &str) -> bool {
    let t = prompt.trim().to_lowercase();
    let t = t.trim_end_matches(['!', '.', '?', ' ']);
    if t.is_empty() {
        return false;
    }
    const SMALL_TALK: &[&str] = &[
        "hi",
        "hello",
        "hey",
        "thanks",
        "thank you",
        "thx",
        "ok",
        "okay",
        "cool",
        "nice",
        "great",
        "good morning",
        "good night",
        "bye",
        "yes",
        "no",
        "sure",
        "lol",
        "got it",
        "sounds good",
    ];
    if SMALL_TALK.contains(&t) {
        return false;
    }
    t.split_whitespace().count() >= 3
}

/// Start a user turn: reset counters and auto-route skills from the prompt.
pub fn begin_turn(session: &str, prompt: &str, skills: &SkillRegistry) -> TurnTelemetry {
    let routed = route_skills(prompt, skills);
    for name in &routed {
        if let Some(skill) = skills.get(name) {
            activate(session, skill);
        }
    }
    with(session, |s| {
        s.turn = TurnTelemetry {
            turn_id: NEXT_TURN.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            actionable: is_actionable(prompt),
            auto_routed_skills: routed,
            ..Default::default()
        };
        s.turn.clone()
    })
}

pub fn turn(session: &str) -> TurnTelemetry {
    with(session, |s| s.turn.clone())
}

pub fn record_tool_call(session: &str, name: &str) {
    with(session, |s| s.turn.tools_called.push(name.to_string()));
}

/// Ask the tool's breaker before executing. `Err` means the brake is open
/// (or a probe is already in flight) and the call is coalesced, not run.
pub fn admit_tool(session: &str, name: &str, now: Instant) -> Result<Admission, String> {
    with(session, |s| {
        match s.breakers.entry(name.to_string()).or_default().admit(now) {
            Admission::Coalesced => {
                s.turn.gate_blocks += 1;
                Err(format!(
                    "Tool '{name}' circuit open after repeated failures; retry coalesced. \
                     Wait for the cooldown or fix the cause before calling it again."
                ))
            }
            admitted => Ok(admitted),
        }
    })
}

/// Report an execution outcome. Failures feed the breaker and return their
/// named recovery reason so callers can auto-resume or escalate once.
pub fn record_tool_outcome(
    session: &str,
    name: &str,
    outcome: Result<(), &str>,
    now: Instant,
) -> Option<RecoveryReason> {
    with(session, |s| {
        let breaker = s.breakers.entry(name.to_string()).or_default();
        match outcome {
            Ok(()) => {
                breaker.record_success(now);
                None
            }
            Err(message) => {
                breaker.record_failure(now);
                Some(classify_interruption(message))
            }
        }
    })
}

/// Current policy snapshot; the agent appends it to history only when it changes.
pub fn policy_block(session: &str) -> String {
    with(session, |s| {
        let mut lines = vec![
            "<harness_execution_policy>".to_string(),
            CORE_INVARIANT.to_string(),
            "Never call load_tools for tools already listed in Active Tools. Mutating tool schemas mid-session breaks prompt KV caching and triggers CAP-WASTE.".to_string(),
            "Execute standard workflow scripts (such as ship.sh and sync-skills.sh) directly. Do not read their source code or create todo checklists unless a script fails.".to_string(),
            "Surface concise [VERIFIED] command/result evidence directly in your final response.".to_string(),
            "Never post GitHub PR comments (gh pr comment) just to record verification output.".to_string(),
            "Never create external logs or artifacts just to record verification output; concise inline [VERIFIED] lines are sufficient for the Turn-Completion Gate after actual tool use.".to_string(),
        ];
        if !s.active_skills.is_empty() {
            lines.push(format!(
                "Active skills: {}",
                s.active_skills
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !s.bound_tools.is_empty() {
            lines.push(format!(
                "Bound tools (call directly, no registry search): {}",
                s.bound_tools.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        let constraints: Vec<&String> = s
            .active_skills
            .iter()
            .filter_map(|n| s.constraints.get(n))
            .flatten()
            .take(MAX_CONSTRAINT_LINES)
            .collect();
        if !constraints.is_empty() {
            lines.push("Hard constraints:".to_string());
            lines.extend(constraints.into_iter().map(|c| format!("- {c}")));
        }
        lines.push("</harness_execution_policy>".to_string());
        lines.join("\n")
    })
}

/// Turn-completion gate. Returns the correction to append when a text-only
/// stop must be intercepted; bumps the intervention counter.
/// Checks tool telemetry, not evidence storage: inline results need no external
/// artifact, and a [VERIFIED] label alone does not replace actual tool use.
pub fn stop_intervention(session: &str) -> Option<String> {
    with(session, |s| {
        let t = &mut s.turn;
        if !t.actionable || !t.tools_called.is_empty() || t.stop_interventions >= MAX_INTERVENTIONS
        {
            return None;
        }
        t.stop_interventions += 1;
        let skills = if s.active_skills.is_empty() {
            "none".to_string()
        } else {
            s.active_skills
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        };
        Some(format!(
            "[HARNESS INTERVENTION]: You attempted to finish this turn without exercising your tools or active skills ({skills}). Do not answer from memory. Invoke the appropriate tools to inspect, verify, and execute the request first."
        ))
    })
}

/// Emit the per-turn telemetry line.
pub fn log_turn(session: &str) {
    let t = turn(session);
    crate::logging::info(&format!(
        "[harness] {}",
        serde_json::json!({
            "session_id": session,
            "turn_id": t.turn_id,
            "actionable": t.actionable,
            "auto_routed_skills": t.auto_routed_skills,
            "tools_called": t.tools_called,
            "gate_blocks": t.gate_blocks,
            "stop_interventions": t.stop_interventions,
        })
    ));
}

// ---------------------------------------------------------------- tool gates

pub const PHASES: [&str; 3] = ["discover_bridge", "inspect_schema", "execute_query"];

pub fn phase_schema() -> Value {
    serde_json::json!({
        "type": "string",
        "enum": PHASES,
        "description": "discover_bridge, inspect_schema of `target`, then execute_query on it."
    })
}

fn is_inspect_name(tool: &str) -> bool {
    let t = tool.to_lowercase();
    [
        "schema",
        "describe",
        "list_tables",
        "list_columns",
        "inspect",
        "information_schema",
    ]
    .iter()
    .any(|k| t.contains(k))
}

fn is_query_name(tool: &str) -> bool {
    let t = tool.to_lowercase();
    [
        "execute_sql",
        "run_sql",
        "sql_query",
        "query",
        "execute_query",
        "select",
        "mutate",
    ]
    .iter()
    .any(|k| t.contains(k))
        && !is_inspect_name(&t)
}

/// What a call does to a data target: (phase, target).
fn classify(name: &str, input: &Value) -> Option<(String, Option<String>)> {
    let s = |k: &str| input.get(k).and_then(Value::as_str).map(str::to_string);
    let declared = s("phase");
    let target = s("target");
    match name {
        "codemode" => declared.map(|p| (p, target)),
        "mcp" | "mcp_call" | "mcp_search" => {
            let action = s("action")
                .unwrap_or_else(|| if name == "mcp_call" { "call" } else { "search" }.to_string());
            let target = target.or(s("server"));
            let phase = declared.or_else(|| match action.as_str() {
                "search" | "list" => Some("discover_bridge".into()),
                "call" => s("tool").map(|t| {
                    if is_inspect_name(&t) {
                        "inspect_schema".into()
                    } else if is_query_name(&t) {
                        "execute_query".into()
                    } else {
                        String::new()
                    }
                }),
                _ => None,
            })?;
            (!phase.is_empty()).then_some((phase, target))
        }
        n if n.starts_with("mcp__") => {
            let rest = &n[5..];
            let (server, tool) = rest.split_once("__").unwrap_or((rest, ""));
            let target = target.or(Some(server.to_string()));
            let phase = declared.or_else(|| {
                if is_inspect_name(tool) {
                    Some("inspect_schema".into())
                } else if is_query_name(tool) {
                    Some("execute_query".into())
                } else {
                    None
                }
            })?;
            Some((phase, target))
        }
        _ => None,
    }
}

/// Validate phase/payload and prerequisite state before a tool executes.
/// `Err` carries the actionable message returned to the model.
pub fn check_tool_call(session: &str, name: &str, input: &Value) -> Result<(), String> {
    let Some((phase, target)) = classify(name, input) else {
        return Ok(());
    };
    let block = |msg: String| {
        with(session, |s| s.turn.gate_blocks += 1);
        Err(msg)
    };
    if !PHASES.contains(&phase.as_str()) {
        return block(format!(
            "BLOCKED_BY_HARNESS: unknown phase '{phase}'. Use one of: {}.",
            PHASES.join(", ")
        ));
    }
    if name == "mcp" || name == "mcp_call" {
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("call");
        let calls = name == "mcp_call" || action == "call";
        if phase == "discover_bridge" && calls {
            return block("BLOCKED_BY_HARNESS: phase discover_bridge must not call a server tool; use action=search.".into());
        }
        if phase == "execute_query" && !calls {
            return block("BLOCKED_BY_HARNESS: phase execute_query requires action=call.".into());
        }
    }
    if phase != "discover_bridge" && target.is_none() {
        return block(format!(
            "BLOCKED_BY_HARNESS: phase {phase} needs a `target` (table, server, or resource)."
        ));
    }
    let target = target.unwrap_or_default();
    match phase.as_str() {
        "inspect_schema" => with(session, |s| {
            s.inspected_targets.insert(target);
        }),
        "execute_query" if !with(session, |s| s.inspected_targets.contains(&target)) => {
            return block(format!(
                "BLOCKED_BY_HARNESS: Target schema/state not yet verified for '{target}'. Run inspection first (phase=inspect_schema, target={target}) before executing queries or mutations."
            ));
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn tool_breaker_opens_coalesces_probes_and_classifies() {
        let s = "harness-breaker";
        reset(s);
        let t0 = Instant::now();
        for i in 0..11 {
            assert_eq!(admit_tool(s, "bash", t0), Ok(Admission::Allow), "call {i}");
            assert_eq!(
                record_tool_outcome(s, "bash", Err("connection reset"), t0),
                Some(RecoveryReason::Interrupted)
            );
        }
        record_tool_outcome(s, "bash", Err("connection reset"), t0);
        // 12th failure opened it: retries coalesce, other tools unaffected.
        assert!(admit_tool(s, "bash", t0).unwrap_err().contains("coalesced"));
        assert_eq!(admit_tool(s, "read", t0), Ok(Admission::Allow));
        let later = t0 + Duration::from_secs(61);
        assert_eq!(admit_tool(s, "bash", later), Ok(Admission::Probe));
        assert!(admit_tool(s, "bash", later).is_err(), "one probe at a time");
        assert_eq!(record_tool_outcome(s, "bash", Ok(()), later), None);
        assert_eq!(
            record_tool_outcome(s, "web", Err("401 Unauthorized: login required"), t0),
            Some(RecoveryReason::UserActionRequired)
        );
        reset(s);
    }

    #[test]
    fn inline_evidence_rules_are_global_and_survive_compaction() {
        for session in ["harness-policy-new", "harness-policy-active"] {
            reset(session);
            if session.ends_with("active") {
                with(session, |s| {
                    s.active_skills.insert("test-skill".into());
                    s.constraints.insert(
                        "test-skill".into(),
                        vec!["skill constraint".into(); MAX_CONSTRAINT_LINES + 1],
                    );
                });
            }
            for compacted in [false, true] {
                if compacted {
                    on_compaction(session);
                }
                let policy = policy_block(session);
                for rule in [
                    "Surface concise [VERIFIED] command/result evidence directly in your final response.",
                    "Never post GitHub PR comments (gh pr comment) just to record verification output.",
                    "Never create external logs or artifacts just to record verification output; concise inline [VERIFIED] lines are sufficient for the Turn-Completion Gate after actual tool use.",
                ] {
                    assert_eq!(policy.matches(rule).count(), 1, "{session}: {policy}");
                }
            }
            reset(session);
        }
    }

    #[test]
    fn cache_workflow_rules_are_global() {
        for session in ["harness-workflow-fresh", "harness-workflow-active"] {
            reset(session);
            let active = session.ends_with("active");
            if active {
                with(session, |s| {
                    s.active_skills.insert("test-skill".into());
                    s.constraints.insert(
                        "test-skill".into(),
                        vec!["skill constraint".into(); MAX_CONSTRAINT_LINES + 1],
                    );
                });
            }
            for compacted in [false, true] {
                if compacted {
                    on_compaction(session);
                }
                let policy = policy_block(session);
                for rule in [
                    "Never call load_tools for tools already listed in Active Tools. Mutating tool schemas mid-session breaks prompt KV caching and triggers CAP-WASTE.",
                    "Execute standard workflow scripts (such as ship.sh and sync-skills.sh) directly. Do not read their source code or create todo checklists unless a script fails.",
                ] {
                    assert_eq!(policy.matches(rule).count(), 1, "{session}: {policy}");
                }
                assert_eq!(
                    policy.matches("- skill constraint").count(),
                    if active { MAX_CONSTRAINT_LINES } else { 0 },
                );
            }
            reset(session);
        }
    }

    #[test]
    fn constraints_come_from_the_trailing_section_only() {
        let body = "# X\n- example bullet\n## HARD CONSTRAINTS\n- never guess\n* verify first\n";
        assert_eq!(hard_constraints(body), vec!["never guess", "verify first"]);
        assert!(hard_constraints("## Other\n- a").is_empty());
    }

    #[test]
    fn small_talk_is_not_actionable() {
        assert!(!is_actionable("thanks!"));
        assert!(!is_actionable("hi"));
        assert!(is_actionable("why do CRM contacts have no address?"));
    }

    #[test]
    fn generic_words_and_resume_messages_do_not_route() {
        let v = |xs: &[&str]| xs.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        let bg = v(&["liquid", "fluid", "background", "hero"]);
        let t = "restart the background server and build it";
        assert!(route_score(t, "frame-liquid-bg-hero", &bg) < MIN_ROUTE_SCORE);
        assert!(route_score(t, "build", &[]) < MIN_ROUTE_SCORE);
        assert!(route_score("liquid fluid hero", "frame-liquid-bg-hero", &bg) >= MIN_ROUTE_SCORE);
        assert!(route_score("use code mode", "code-mode", &v(&["code mode"])) >= MIN_ROUTE_SCORE);
        assert!(is_system_message("## Previous Conversation Summary\nx"));
        assert!(!is_system_message("use code mode"));
    }

    #[test]
    fn understand_requires_architectural_intent() {
        let triggers = [
            "understand codebase",
            "knowledge graph",
            "map codebase",
            "architecture graph",
            "analyze repository structure",
        ]
        .map(str::to_string);
        for prompt in [
            "help me understand this",
            "explain this diff",
            "fix css button",
        ] {
            assert!(
                route_score(prompt, "understand", &triggers) < MIN_ROUTE_SCORE,
                "{prompt}"
            );
        }
        for prompt in &triggers {
            assert!(
                route_score(prompt, "understand", &triggers) >= MIN_ROUTE_SCORE,
                "{prompt}"
            );
        }
    }

    #[test]
    fn phrase_match_needs_word_boundaries() {
        assert!(contains_phrase("use ponytail here", "ponytail"));
        assert!(!contains_phrase("ponytails", "ponytail"));
    }

    #[test]
    fn query_before_inspection_is_blocked_then_allowed() {
        let s = "harness-unit-gate";
        reset(s);
        let q = json!({"action":"call","server":"supabase","tool":"execute_sql","arguments":{}});
        let err = check_tool_call(s, "mcp", &q).unwrap_err();
        assert!(err.starts_with("BLOCKED_BY_HARNESS"), "{err}");
        check_tool_call(s, "mcp__supabase__list_tables", &json!({})).unwrap();
        check_tool_call(s, "mcp", &q).unwrap();
        assert_eq!(turn(s).gate_blocks, 1);
        reset(s);
    }

    #[test]
    fn codemode_phase_must_match_payload() {
        let s = "harness-unit-codemode";
        reset(s);
        assert!(check_tool_call(s, "codemode", &json!({"code":"x","phase":"bogus"})).is_err());
        assert!(
            check_tool_call(s, "codemode", &json!({"code":"x","phase":"inspect_schema"})).is_err()
        );
        check_tool_call(s, "codemode", &json!({"code":"x"})).unwrap();
        check_tool_call(
            s,
            "codemode",
            &json!({"code":"x","phase":"discover_bridge"}),
        )
        .unwrap();
        reset(s);
    }
}
