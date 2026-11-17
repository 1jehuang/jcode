//! Two-tier harness suite: outer agent loop (routing, append-only policy,
//! turn-completion gate) and inner tool gates (phase/prerequisite checks for
//! native `codemode` and MCP). Real `Agent` and tool `Registry`; only the
//! provider is scripted, and it records every request it receives.

use anyhow::Result;
use async_stream::stream;
use jcode::agent::Agent;
use jcode::message::{ContentBlock, Message, StreamEvent, ToolDefinition};
use jcode::provider::{EventStream, Provider};
use jcode::tool::Registry;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const POLICY_TAG: &str = "<harness_execution_policy>";

#[derive(Clone, Default)]
struct Scripted {
    responses: Arc<Mutex<VecDeque<Vec<StreamEvent>>>>,
    requests: Arc<Mutex<Vec<Vec<Message>>>>,
}

impl Scripted {
    fn text(&self, text: &str) {
        self.responses.lock().unwrap().push_back(vec![
            StreamEvent::TextDelta(text.into()),
            StreamEvent::MessageEnd {
                stop_reason: Some("end_turn".into()),
            },
        ]);
    }
    fn tool(&self, id: &str, name: &str, input: Value) {
        self.responses.lock().unwrap().push_back(vec![
            StreamEvent::ToolUseStart {
                id: id.into(),
                name: name.into(),
            },
            StreamEvent::ToolInputDelta(input.to_string()),
            StreamEvent::ToolUseEnd,
            StreamEvent::MessageEnd {
                stop_reason: Some("tool_use".into()),
            },
        ]);
    }
    fn requests(&self) -> Vec<Vec<Message>> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Provider for Scripted {
    async fn complete(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume: Option<&str>,
    ) -> Result<EventStream> {
        self.requests.lock().unwrap().push(messages.to_vec());
        let events = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                vec![
                    StreamEvent::TextDelta("script exhausted".into()),
                    StreamEvent::MessageEnd {
                        stop_reason: Some("end_turn".into()),
                    },
                ]
            });
        Ok(Box::pin(stream! { for e in events { yield Ok(e); } }))
    }
    fn name(&self) -> &str {
        "mock"
    }
    fn model(&self) -> String {
        "mock".into()
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn text_of(m: &Message) -> String {
    m.content
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text, .. } => text.clone(),
            ContentBlock::ToolResult { content, .. } => content.clone(),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn count_policy(req: &[Message]) -> usize {
    req.iter()
        .filter(|m| text_of(m).contains(POLICY_TAG))
        .count()
}

fn tool_results(req: &[Message]) -> Vec<String> {
    req.iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect()
}

struct Env {
    _home: tempfile::TempDir,
    work: tempfile::TempDir,
}

static LOCK: Mutex<()> = Mutex::new(());

fn env() -> Result<(std::sync::MutexGuard<'static, ()>, Env)> {
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir()?;
    std::fs::write(home.path().join("no_telemetry"), b"")?;
    jcode::env::set_var("JCODE_HOME", home.path());
    jcode::env::set_var("JCODE_RUNTIME_DIR", home.path().join("rt"));
    jcode::env::set_var("JCODE_TEST_SESSION", "1");
    jcode::env::set_var("JCODE_MEMORY_ENABLED", "0");
    jcode::env::set_var("JCODE_MEMORY_SIDECAR_ENABLED", "0");
    let work = tempfile::tempdir()?;
    let dir = work.path().join(".jcode/skills/crm-audit");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: crm-audit\ndescription: Audit CRM contact data\ntriggers: [crm, contacts]\nrequired_tools: [codemode, mcp]\n---\n# Examples\n- example: look at contacts\n\n## HARD CONSTRAINTS\n- Inspect the contacts schema before any query\n- Report row counts, never guesses\n",
    )?;
    Ok((guard, Env { _home: home, work }))
}

async fn agent(env: &Env, provider: &Scripted) -> Agent {
    let p: Arc<dyn Provider> = Arc::new(provider.clone());
    let registry = Registry::new(p.clone()).await;
    let mut agent = Agent::new(p, registry);
    agent.set_working_dir(env.work.path().to_str().unwrap());
    agent
}

// ------------------------------------------------------------ outer loop

async fn check_policy_request_prefix(streaming: bool) -> Result<()> {
    let (_g, env) = env()?;
    let p = Scripted::default();
    p.tool(
        "load",
        "skill_manage",
        json!({"action": "load", "name": "crm-audit"}),
    );
    p.tool(
        "query",
        "codemode",
        json!({"code": "return 1", "phase": "discover_bridge"}),
    );
    p.text("[VERIFIED] codemode return 1 -> 1");
    let mut a = agent(&env, &p).await;
    let prompt = "Inspect the available data using the appropriate skill and tools";
    if streaming {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        a.run_once_streaming_mpsc(prompt, vec![], None, tx).await?;
    } else {
        a.run_once_capture(prompt).await?;
    }
    p.text("ok");
    if streaming {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        a.run_once_streaming_mpsc("thanks", vec![], None, tx)
            .await?;
    } else {
        a.run_once_capture("thanks").await?;
    }
    let requests = p.requests();
    assert_eq!(requests.len(), 4, "load, query, final, next user turn");
    for pair in requests.windows(2) {
        let previous = serde_json::to_value(&pair[0])?;
        let next = serde_json::to_value(&pair[1])?;
        assert!(
            next.as_array()
                .unwrap()
                .starts_with(previous.as_array().unwrap()),
            "request history must remain an exact prefix after a tool round or new turn"
        );
    }
    assert_eq!(count_policy(&requests[0]), 1);
    for request in &requests[1..] {
        assert_eq!(count_policy(request), 2, "append only the changed policy");
        let latest = request
            .iter()
            .rev()
            .find(|m| text_of(m).contains(POLICY_TAG))
            .unwrap();
        assert!(text_of(latest).starts_with("<system-reminder>\n"));
        assert!(text_of(latest).contains("crm-audit"));
    }
    jcode::harness::reset(a.session_id());
    Ok(())
}

#[tokio::test]
async fn blocking_policy_preserves_request_prefix() -> Result<()> {
    check_policy_request_prefix(false).await
}

#[tokio::test]
async fn streaming_policy_preserves_request_prefix() -> Result<()> {
    check_policy_request_prefix(true).await
}

#[tokio::test]
async fn routes_skill_injects_policy_once_and_gates_premature_stop() -> Result<()> {
    let (_g, env) = env()?;
    let p = Scripted::default();
    let evidence = "[VERIFIED] codemode return 1 -> 1";
    p.text(evidence); // A verification label alone cannot replace tool use.
    p.tool(
        "t1",
        "codemode",
        json!({"code": "return 1", "phase": "discover_bridge"}),
    );
    p.text(evidence);
    let mut a = agent(&env, &p).await;
    let out = a
        .run_once_capture("why do contacts in CRM have no address?")
        .await?;
    let sid = a.session_id().to_string();

    let reqs = p.requests();
    assert_eq!(reqs.len(), 3, "guess, tool round, final");
    for r in &reqs {
        assert_eq!(count_policy(r), 1, "exactly one policy block per request");
    }
    let policy = text_of(
        reqs[0]
            .iter()
            .rev()
            .find(|m| text_of(m).contains(POLICY_TAG))
            .unwrap(),
    );
    assert!(policy.contains("ACTION OVER MEMORY"));
    assert!(
        policy.contains("crm-audit"),
        "auto-routed before first call: {policy}"
    );
    assert!(
        policy.contains("Inspect the contacts schema"),
        "hard constraints embedded"
    );
    assert!(policy.contains("codemode"), "required tools bound");

    // Intervention reached the model; the guess never did.
    let second: String = reqs[1].iter().map(text_of).collect();
    assert!(second.contains("[HARNESS INTERVENTION]"));
    assert!(!second.contains(evidence), "premature text suppressed");
    assert_eq!(out, evidence, "inline evidence needs no external artifact");

    let t = jcode::harness::turn(&sid);
    assert_eq!(t.auto_routed_skills, vec!["crm-audit"]);
    assert_eq!(t.stop_interventions, 1);
    assert_eq!(t.tools_called, vec!["codemode"], "no artifact-writing tool");

    // An unchanged policy snapshot is reused on the next turn.
    p.text("ok");
    a.run_once_capture("thanks").await?;
    let last = p.requests().pop().unwrap();
    assert_eq!(count_policy(&last), 1, "no accumulation across turns");
    jcode::harness::reset(&sid);
    Ok(())
}

#[tokio::test]
async fn interventions_are_capped_and_small_talk_passes() -> Result<()> {
    let (_g, env) = env()?;
    let p = Scripted::default();
    for _ in 0..3 {
        p.text("guess");
    }
    let mut a = agent(&env, &p).await;
    a.run_once_capture("list the open invoices for march")
        .await?;
    let sid = a.session_id().to_string();
    assert_eq!(p.requests().len(), 3, "1 + MAX_INTERVENTIONS");
    assert_eq!(jcode::harness::turn(&sid).stop_interventions, 2);

    p.text("hello!");
    a.run_once_capture("hi").await?;
    assert_eq!(p.requests().len(), 4, "chit-chat is never intercepted");
    assert_eq!(jcode::harness::turn(&sid).stop_interventions, 0);
    jcode::harness::reset(&sid);
    Ok(())
}

// ------------------------------------------------------------ inner gates

#[tokio::test]
async fn crm_investigation_trace_is_gated_and_self_corrects() -> Result<()> {
    let (_g, env) = env()?;
    let p = Scripted::default();
    // Skill loaded explicitly before work, then re-requested: must ack, not re-inject.
    p.tool(
        "s1",
        "skill_manage",
        json!({"action": "load", "name": "crm-audit"}),
    );
    p.tool(
        "s2",
        "skill_manage",
        json!({"action": "load", "name": "crm-audit"}),
    );
    // Premature query on both paths.
    p.tool(
        "q1",
        "codemode",
        json!({"code": "return 1", "phase": "execute_query", "target": "contacts"}),
    );
    p.tool(
        "q2",
        "mcp",
        json!({"action": "call", "server": "crm", "tool": "execute_sql", "arguments": {}}),
    );
    // Self-correct: inspect, then query.
    p.tool(
        "i1",
        "codemode",
        json!({"code": "return 'schema'", "phase": "inspect_schema", "target": "contacts"}),
    );
    p.tool(
        "q3",
        "codemode",
        json!({"code": "return 0", "phase": "execute_query", "target": "contacts"}),
    );
    p.text("0 contacts missing address after verification.");
    let mut a = agent(&env, &p).await;
    a.run_once_capture("why contacts in CRM have no address?")
        .await?;
    let sid = a.session_id().to_string();

    let results = tool_results(p.requests().last().unwrap());
    assert_eq!(results.len(), 6, "{results:#?}");
    assert!(results[0].contains("## Skill: crm-audit"));
    assert!(
        results[1].contains("already active; proceed directly"),
        "{}",
        results[1]
    );
    assert!(
        results[2].contains("BLOCKED_BY_HARNESS"),
        "native codemode gated: {}",
        results[2]
    );
    assert!(
        results[3].contains("BLOCKED_BY_HARNESS"),
        "MCP gated: {}",
        results[3]
    );
    assert!(!results[4].contains("BLOCKED_BY_HARNESS"));
    assert!(
        !results[5].contains("BLOCKED_BY_HARNESS"),
        "query after inspection passes: {}",
        results[5]
    );

    let t = jcode::harness::turn(&sid);
    assert_eq!(t.gate_blocks, 2);
    assert_eq!(t.stop_interventions, 0, "tools were exercised");
    assert!(
        !t.tools_called.iter().any(|n| n == "mcp_search"),
        "no redundant registry search: {:?}",
        t.tools_called
    );
    jcode::harness::reset(&sid);
    Ok(())
}
