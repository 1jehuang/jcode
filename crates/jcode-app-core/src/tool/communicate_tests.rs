use super::{
    CommunicateInput, CommunicateTool, canonical_swarm_action, cleanup_candidate_session_ids,
    coordination_in_flight_count, default_cleanup_target_statuses, format_awaited_members,
    format_awaited_members_with_reports, format_members, format_plan_status,
    format_swarm_model_list, latest_assistant_report, resolve_optional_target_session,
    swarm_member_is_drivable_worker, swarm_member_is_in_flight,
};
use crate::message::{Message, StreamEvent, ToolDefinition};
use crate::protocol::{
    AgentInfo, AgentStatusSnapshot, AwaitedMemberStatus, HistoryMessage, NotificationType, Request,
    ServerEvent, SessionActivitySnapshot, ToolCallSummary,
};
use crate::provider::{EventStream, Provider};
use crate::server::Server;
use crate::tool::{Tool, ToolContext, ToolExecutionMode};
use crate::transport::{ReadHalf, Stream, WriteHalf};
use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[test]
fn tool_is_named_swarm() {
    assert_eq!(CommunicateTool::new().name(), "swarm");
}

#[test]
fn canonical_swarm_action_maps_common_synonyms() {
    assert_eq!(canonical_swarm_action("inbox"), "read");
    assert_eq!(canonical_swarm_action("read_messages"), "read");
    assert_eq!(canonical_swarm_action("send"), "message");
    assert_eq!(canonical_swarm_action("msg"), "message");
    assert_eq!(canonical_swarm_action("direct_message"), "message");
    assert_eq!(canonical_swarm_action("announce"), "broadcast");
    assert_eq!(canonical_swarm_action("agents"), "list");
    assert_eq!(canonical_swarm_action("plan"), "plan_status");
    assert_eq!(canonical_swarm_action("assign"), "assign_task");
    assert_eq!(canonical_swarm_action("kill"), "stop");
}

#[test]
fn canonical_swarm_action_is_case_insensitive_and_trims() {
    assert_eq!(canonical_swarm_action("  Inbox  "), "read");
    assert_eq!(canonical_swarm_action("SEND"), "message");
}

#[test]
fn canonical_swarm_action_passes_through_known_and_unknown_actions() {
    // Real actions are unchanged.
    assert_eq!(canonical_swarm_action("spawn"), "spawn");
    assert_eq!(canonical_swarm_action("dm"), "message");
    assert_eq!(canonical_swarm_action("assign_role"), "assign_role");
    // Genuinely unknown actions are returned unchanged for normal validation.
    assert_eq!(canonical_swarm_action("totally_made_up"), "totally_made_up");
}

#[test]
fn communicate_input_aliases_to_session_and_target_session() {
    // Either field name should be accepted; the execute() normalization mirrors them.
    let from_target: CommunicateInput = serde_json::from_value(
        json!({ "action": "dm", "message": "hi", "target_session": "worker-1" }),
    )
    .expect("parse target_session input");
    assert_eq!(from_target.target_session.as_deref(), Some("worker-1"));
    assert_eq!(from_target.to_session, None);

    let from_to: CommunicateInput =
        serde_json::from_value(json!({ "action": "summary", "to_session": "worker-2" }))
            .expect("parse to_session input");
    assert_eq!(from_to.to_session.as_deref(), Some("worker-2"));
    assert_eq!(from_to.target_session, None);
}

#[test]
fn format_plan_status_includes_next_ready() {
    let output = format_plan_status(&crate::protocol::PlanGraphStatus {
        swarm_id: Some("swarm-a".to_string()),
        version: 3,
        item_count: 4,
        ready_ids: vec!["task-2".to_string(), "task-3".to_string()],
        blocked_ids: vec!["task-4".to_string()],
        active_ids: vec!["task-1".to_string()],
        completed_ids: vec!["setup".to_string()],
        failed_ids: Vec::new(),
        failed_reasons: Default::default(),
        cycle_ids: Vec::new(),
        unresolved_dependency_ids: Vec::new(),
        next_ready_ids: vec!["task-2".to_string()],
        newly_ready_ids: vec!["task-3".to_string()],
    });
    let text = output.output;
    assert!(text.contains("Plan status for swarm swarm-a"));
    assert!(text.contains("Next up: task-2"));
    assert!(text.contains("Newly ready: task-3"));
    assert!(text.contains("Blocked: task-4"));
}

#[test]
fn in_flight_slot_accounting_counts_queued_workers_not_coordinator() {
    let summary = crate::protocol::PlanGraphStatus {
        swarm_id: Some("swarm-a".to_string()),
        version: 3,
        item_count: 4,
        ready_ids: vec!["queued-assigned".to_string()],
        blocked_ids: Vec::new(),
        active_ids: vec!["running-plan-task".to_string()],
        completed_ids: Vec::new(),
        failed_ids: Vec::new(),
        failed_reasons: Default::default(),
        cycle_ids: Vec::new(),
        unresolved_dependency_ids: Vec::new(),
        next_ready_ids: vec!["queued-assigned".to_string()],
        newly_ready_ids: Vec::new(),
    };
    let members = vec![
        AgentInfo {
            session_id: "coord".to_string(),
            friendly_name: None,
            files_touched: Vec::new(),
            status: Some("running".to_string()),
            detail: None,
            role: Some("coordinator".to_string()),
            is_headless: Some(false),
            report_back_to_session_id: None,
            latest_completion_report: None,
            live_attachments: None,
            status_age_secs: None,
            ..Default::default()
        },
        AgentInfo {
            session_id: "worker-queued".to_string(),
            friendly_name: None,
            files_touched: Vec::new(),
            status: Some("queued".to_string()),
            detail: None,
            role: Some("agent".to_string()),
            is_headless: Some(true),
            report_back_to_session_id: Some("coord".to_string()),
            latest_completion_report: None,
            live_attachments: None,
            status_age_secs: None,
            ..Default::default()
        },
        AgentInfo {
            session_id: "worker-ready".to_string(),
            friendly_name: None,
            files_touched: Vec::new(),
            status: Some("ready".to_string()),
            detail: None,
            role: Some("agent".to_string()),
            is_headless: Some(true),
            report_back_to_session_id: Some("coord".to_string()),
            latest_completion_report: None,
            live_attachments: None,
            status_age_secs: None,
            ..Default::default()
        },
    ];

    assert!(swarm_member_is_in_flight(&members[1]));
    assert!(!swarm_member_is_in_flight(&members[2]));
    assert_eq!(coordination_in_flight_count(&summary, &members, "coord"), 1);
}

#[test]
fn in_flight_count_excludes_foreign_queued_session() {
    // A stale, independent (non-owned, client-attached) session that merely shares
    // the swarm and happens to sit in `queued` must NOT count as in-flight for
    // run_plan: it is never auto-driven, so awaiting it would hang the run even
    // though no plan task is assigned to it. Regression for the run_plan stall.
    let summary = crate::protocol::PlanGraphStatus {
        swarm_id: Some("swarm-a".to_string()),
        version: 1,
        item_count: 1,
        ready_ids: Vec::new(),
        blocked_ids: Vec::new(),
        active_ids: Vec::new(),
        completed_ids: vec!["done-task".to_string()],
        failed_ids: Vec::new(),
        failed_reasons: Default::default(),
        cycle_ids: Vec::new(),
        unresolved_dependency_ids: Vec::new(),
        next_ready_ids: Vec::new(),
        newly_ready_ids: Vec::new(),
    };
    let members = vec![
        AgentInfo {
            session_id: "coord".to_string(),
            status: Some("running".to_string()),
            role: Some("coordinator".to_string()),
            is_headless: Some(false),
            report_back_to_session_id: None,
            ..Default::default()
        },
        AgentInfo {
            session_id: "foreign-human".to_string(),
            status: Some("queued".to_string()),
            role: Some("agent".to_string()),
            is_headless: Some(false),
            // Not owned by coord, and a live client is attached.
            report_back_to_session_id: None,
            live_attachments: Some(1),
            ..Default::default()
        },
    ];

    // It is technically "in flight" by status, but not a drivable worker, so the
    // scoped count is zero and run_plan can reach its terminal check.
    assert!(swarm_member_is_in_flight(&members[1]));
    assert!(!swarm_member_is_drivable_worker(&members[1], "coord"));
    assert_eq!(coordination_in_flight_count(&summary, &members, "coord"), 0);
}

#[test]
fn latest_assistant_report_uses_last_non_empty_assistant_message() {
    let messages = vec![
        HistoryMessage {
            response_stats: None,
            role: "assistant".to_string(),
            content: " earlier ".to_string(),
            tool_calls: None,
            tool_data: None,
        },
        HistoryMessage {
            response_stats: None,
            role: "user".to_string(),
            content: "ignored".to_string(),
            tool_calls: None,
            tool_data: None,
        },
        HistoryMessage {
            response_stats: None,
            role: "assistant".to_string(),
            content: " final report ".to_string(),
            tool_calls: None,
            tool_data: None,
        },
    ];

    assert_eq!(
        latest_assistant_report(&messages).as_deref(),
        Some("final report")
    );
}

#[test]
fn format_awaited_members_includes_recent_outputs() {
    let members = vec![AwaitedMemberStatus {
        session_id: "session_worker".to_string(),
        friendly_name: Some("worker".to_string()),
        status: "ready".to_string(),
        done: true,
        completion_report: Some("Structured report wins.".to_string()),
    }];
    let reports = HashMap::from([(
        "session_worker".to_string(),
        "Outcome: finished. Validation: tests passed.".to_string(),
    )]);

    let output = format_awaited_members_with_reports(
        true,
        "All 1 members are done: worker",
        &members,
        &reports,
    )
    .output;

    assert!(output.contains("Recent outputs:"));
    assert!(output.contains("--- worker (ready) ---"));
    assert!(output.contains("Structured report wins."));
    assert!(!output.contains("Outcome: finished"));
}

#[test]
fn resolve_optional_target_session_defaults_to_current() {
    assert_eq!(
        resolve_optional_target_session(None, "session_current"),
        "session_current"
    );
    assert_eq!(
        resolve_optional_target_session(Some("current".to_string()), "session_current"),
        "session_current"
    );
    assert_eq!(
        resolve_optional_target_session(Some("session_other".to_string()), "session_current"),
        "session_other"
    );
}

#[path = "communicate_schema_tests.rs"]
mod schema;

struct EnvGuard {
    key: &'static str,
    original: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, original }
    }

    fn remove(key: &'static str) -> Self {
        let original = std::env::var_os(key);
        crate::env::remove_var(key);
        Self { key, original }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(value) = self.original.take() {
            crate::env::set_var(self.key, value);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

struct DelayedTestProvider {
    delay: Duration,
}

#[async_trait]
impl Provider for DelayedTestProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let delay = self.delay;
        let stream = futures::stream::once(async move {
            tokio::time::sleep(delay).await;
            Ok(StreamEvent::TextDelta("ok".to_string()))
        })
        .chain(futures::stream::once(async {
            Ok(StreamEvent::MessageEnd { stop_reason: None })
        }));
        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self { delay: self.delay })
    }
}

struct RawClient {
    reader: BufReader<ReadHalf>,
    writer: WriteHalf,
    next_id: u64,
}

impl RawClient {
    async fn connect(path: &Path) -> Result<Self> {
        let stream = Stream::connect(path).await?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
        })
    }

    async fn send_request(&mut self, request: Request) -> Result<u64> {
        let id = request.id();
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(id)
    }

    async fn read_event(&mut self) -> Result<ServerEvent> {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line).await?;
        if n == 0 {
            anyhow::bail!("server disconnected")
        }
        Ok(serde_json::from_str(&line)?)
    }

    async fn read_until<F>(&mut self, timeout: Duration, mut predicate: F) -> Result<ServerEvent>
    where
        F: FnMut(&ServerEvent) -> bool,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let event = tokio::time::timeout(remaining, self.read_event()).await??;
            if predicate(&event) {
                return Ok(event);
            }
        }
    }

    async fn subscribe(&mut self, working_dir: &Path) -> Result<()> {
        let id = self.next_id;
        self.next_id += 1;
        self.send_request(Request::Subscribe {
            system_prompt: None,
            supports_pdf_panels: false,
            supports_user_response: false,
            supports_server_failover: false,
            supports_turn_identity: false,
            id,
            working_dir: Some(working_dir.display().to_string()),
            selfdev: None,
            target_session_id: None,
            client_instance_id: None,
            client_has_local_history: false,
            history_base: None,
            allow_session_takeover: false,
            crash_on_disconnect: false,
            continue_on_disconnect: false,
            terminal_env: Vec::new(),
            client_name: None,
        })
        .await?;
        self.read_until(
            Duration::from_secs(5),
            |event| matches!(event, ServerEvent::Done { id: done_id } if *done_id == id),
        )
        .await?;
        Ok(())
    }

    async fn session_id(&mut self) -> Result<String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send_request(Request::GetState { id }).await?;
        match self
            .read_until(
                Duration::from_secs(5),
                |event| matches!(event, ServerEvent::State { id: event_id, .. } if *event_id == id),
            )
            .await?
        {
            ServerEvent::State { session_id, .. } => Ok(session_id),
            other => anyhow::bail!("unexpected state response: {other:?}"),
        }
    }

    async fn send_message(&mut self, content: &str) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        self.send_request(Request::Message {
            id,
            content: content.to_string(),
            images: vec![],
            system_reminder: None,
            active_skill: None,
            no_reply: false,
        })
        .await
    }

    async fn wait_for_done(&mut self, request_id: u64) -> Result<()> {
        self.read_until(
            Duration::from_secs(10),
            |event| matches!(event, ServerEvent::Done { id } if *id == request_id),
        )
        .await?;
        Ok(())
    }

    async fn comm_list(&mut self, session_id: &str) -> Result<Vec<AgentInfo>> {
        let id = self.next_id;
        self.next_id += 1;
        self.send_request(Request::CommList {
            id,
            session_id: session_id.to_string(),
        })
        .await?;
        match self
                .read_until(Duration::from_secs(5), |event| {
                    matches!(event, ServerEvent::CommMembers { id: event_id, .. } if *event_id == id)
                })
                .await?
            {
                ServerEvent::CommMembers { members, .. } => Ok(members),
                other => anyhow::bail!("unexpected comm_list response: {other:?}"),
            }
    }

    async fn comm_status(
        &mut self,
        session_id: &str,
        target_session: &str,
    ) -> Result<AgentStatusSnapshot> {
        let id = self.next_id;
        self.next_id += 1;
        self.send_request(Request::CommStatus {
            id,
            session_id: session_id.to_string(),
            target_session: target_session.to_string(),
        })
        .await?;
        match self
                .read_until(Duration::from_secs(5), |event| {
                    matches!(event, ServerEvent::CommStatusResponse { id: event_id, .. } if *event_id == id)
                })
                .await?
            {
                ServerEvent::CommStatusResponse { snapshot, .. } => Ok(snapshot),
                other => anyhow::bail!("unexpected comm_status response: {other:?}"),
            }
    }

    /// Wait for the next `Message` notification and return its scope
    /// ("dm", "channel", or "broadcast"). Other events are skipped.
    async fn next_message_notification(&mut self, timeout: Duration) -> Result<Option<String>> {
        match self
            .read_until(timeout, |event| {
                matches!(
                    event,
                    ServerEvent::Notification {
                        notification_type: NotificationType::Message { .. },
                        ..
                    }
                )
            })
            .await?
        {
            ServerEvent::Notification {
                notification_type: NotificationType::Message { scope, .. },
                ..
            } => Ok(scope),
            other => anyhow::bail!("unexpected notification response: {other:?}"),
        }
    }
}

async fn wait_for_server_socket(
    path: &Path,
    server_task: &mut tokio::task::JoinHandle<Result<()>>,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if server_task.is_finished() {
            let result = server_task.await?;
            return Err(anyhow::anyhow!(
                "server exited before socket became ready: {:?}",
                result
            ));
        }
        match Stream::connect(path).await {
            Ok(stream) => {
                drop(stream);
                return Ok(());
            }
            Err(err) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(err.into());
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    }
}

fn test_ctx(session_id: &str, working_dir: &Path) -> ToolContext {
    ToolContext {
        session_id: session_id.to_string(),
        message_id: "msg-1".to_string(),
        tool_call_id: "call-1".to_string(),
        working_dir: Some(working_dir.to_path_buf()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    }
}

async fn wait_for_member_status(
    client: &mut RawClient,
    requester_session: &str,
    target_session: &str,
    expected_status: &str,
) -> Result<Vec<AgentInfo>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let members = client.comm_list(requester_session).await?;
        if members
            .iter()
            .find(|member| member.session_id == target_session)
            .and_then(|member| member.status.as_deref())
            == Some(expected_status)
        {
            return Ok(members);
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for member {} to reach status {}; members: {:?}",
                target_session,
                expected_status,
                members
                    .iter()
                    .map(|member| (member.session_id.clone(), member.status.clone()))
                    .collect::<Vec<_>>()
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_for_member_presence(
    client: &mut RawClient,
    requester_session: &str,
    target_session: &str,
) -> Result<Vec<AgentInfo>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let members = client.comm_list(requester_session).await?;
        if members
            .iter()
            .any(|member| member.session_id == target_session)
        {
            return Ok(members);
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for member {} to appear", target_session);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

include!("communicate_tests/input_format.rs");
include!("communicate_tests/end_to_end.rs");
include!("communicate_tests/assignment.rs");
