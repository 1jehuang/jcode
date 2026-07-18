use super::*;
use crate::tool::selfdev::ReloadContext;
use crate::tui::TuiState;
use crate::tui::app as app_mod;
use crate::tui::app::remote::swarm_plan_core::RemoteSwarmPlanSnapshot;
use crate::tui::app::remote::swarm_status_core::swarm_status_transition_notice;

fn allow_runtime_identity_mismatch() -> bool {
    std::env::var_os("JCODE_ALLOW_SERVER_VERSION_MISMATCH").is_some()
}

/// Parse a jcode version string into an orderable `(major, minor, patch)`, but
/// only for *clean release* builds.
///
/// Dev/dirty builds share a base semver and cannot be ordered against each other
/// or against releases (issue #277/#291: a self-dev / branched daemon must never
/// be force-downgraded just because its version string differs). So we refuse to
/// classify anything carrying a `-dev` or `dirty` marker as an orderable version
/// and return `None`, leaving such daemons to the existing `server_has_update`
/// (mtime-directional) path.
fn parse_release_semver(version: &str) -> Option<(u32, u32, u32)> {
    let lower = version.trim().to_ascii_lowercase();
    if lower.contains("-dev") || lower.contains("dirty") {
        return None;
    }
    // Take the leading token, e.g. "v0.17.0 (d741696f)" -> "0.17.0".
    let token = lower
        .split([' ', '(', ')', ','])
        .next()
        .unwrap_or(&lower)
        .trim();
    let token = token.strip_prefix('v').unwrap_or(token);
    let mut parts = token.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// True when the connected server reports a clean release version strictly older
/// than this client's own clean release version.
///
/// This is the missing client-side staleness signal behind issue #295: a server
/// old enough to predate the self-reported staleness machinery reports
/// `server_has_update: None`, so it can never tell us it is stale and the client
/// happily attaches to it (then a `set_route`-shaped request explodes against the
/// ancient protocol). We detect that case independently here.
///
/// Gated on clean release semvers on BOTH sides, so dev/dirty/self-dev daemons
/// (which cannot be ordered) are never affected.
fn server_release_is_older_than_client(server_version: Option<&str>, client_version: &str) -> bool {
    let Some(server) = server_version.and_then(parse_release_semver) else {
        return false;
    };
    let Some(client) = parse_release_semver(client_version) else {
        return false;
    };
    server < client
}

/// Decide whether to defer applying remote session state because the server we
/// attached to is not running the binary we expect.
///
/// Precedence:
/// - The client independently measured the server's release version as strictly
///   older than its own clean release version -> defer. This wins even over the
///   server's own `server_has_update: Some(false)` self-report, because a stale
///   long-lived daemon legitimately reports "no newer binary to reload into"
///   (its `shared-server` channel still points at its own old build) while the
///   client can plainly see it is an older release. Trusting the server here is
///   exactly what left "current client, stale server" stuck (the daemon's reload
///   decision runs old code that can never drag itself forward). The newer
///   client is authoritative, so it defers and repairs the channel before
///   reloading.
/// - `Some(true)`: the server self-reported a newer binary on disk -> defer.
/// - `Some(false)`: the server is new enough to self-assess and found nothing
///   newer to reload into, AND the client could not prove it is older -> trust
///   it, do not fight it with a forced reload.
/// - `None`: the server is too old to self-report. Fall back to our own
///   client-side release-version comparison, which is the only signal that can
///   catch a pre-self-heal daemon.
fn should_defer_history_for_runtime_identity_with_allow(
    server_has_update: Option<bool>,
    client_detected_stale: bool,
    allow_mismatch: bool,
) -> bool {
    if allow_mismatch {
        return false;
    }
    // A client-proven-older server always wins: never let an old daemon's
    // (locally correct but globally wrong) "no update" self-report veto the
    // client's own release-order comparison.
    if client_detected_stale {
        return true;
    }
    match server_has_update {
        Some(true) => true,
        Some(false) => false,
        None => false,
    }
}

/// The client's own version string, used for release-staleness comparison.
///
/// Production always reads the compiled-in build metadata. A test-only env
/// override exists so the end-to-end `handle_server_event` path can be exercised
/// from a dev/dirty test binary (whose real version would otherwise be
/// unorderable and short-circuit the comparison).
fn client_release_version() -> String {
    if (cfg!(test) || cfg!(debug_assertions))
        && let Some(v) = std::env::var_os("JCODE_TEST_CLIENT_VERSION_OVERRIDE")
    {
        return v.to_string_lossy().into_owned();
    }
    jcode_build_meta::version().to_string()
}

fn should_defer_history_for_runtime_identity(
    server_has_update: Option<bool>,
    server_version: Option<&str>,
) -> bool {
    let client_detected_stale =
        server_release_is_older_than_client(server_version, &client_release_version());
    should_defer_history_for_runtime_identity_with_allow(
        server_has_update,
        client_detected_stale,
        allow_runtime_identity_mismatch(),
    )
}

#[cfg(test)]
mod runtime_identity_tests {
    use super::{
        parse_release_semver, server_release_is_older_than_client,
        should_defer_history_for_runtime_identity_with_allow,
    };

    #[test]
    fn runtime_identity_gate_defers_stale_server_history_by_default() {
        assert!(should_defer_history_for_runtime_identity_with_allow(
            Some(true),
            false,
            false
        ));
        assert!(!should_defer_history_for_runtime_identity_with_allow(
            Some(false),
            false,
            false
        ));
        assert!(!should_defer_history_for_runtime_identity_with_allow(
            None, false, false
        ));
    }

    #[test]
    fn runtime_identity_gate_allows_explicit_mismatch_escape_hatch() {
        assert!(!should_defer_history_for_runtime_identity_with_allow(
            Some(true),
            false,
            true
        ));
        assert!(!should_defer_history_for_runtime_identity_with_allow(
            None, true, true
        ));
    }

    #[test]
    fn client_detected_older_server_always_defers() {
        // Ancient server (server_has_update: None) that the client independently
        // measured as older -> defer. This is the issue #295 macOS case where a
        // pre-self-heal daemon can never set server_has_update itself.
        assert!(should_defer_history_for_runtime_identity_with_allow(
            None, true, false
        ));
        // A server that self-reports "no newer binary" (Some(false)) but that the
        // client can PROVE is an older release -> still defer. The daemon's
        // self-report is locally correct (its own shared-server channel points at
        // its old build) but globally wrong; the newer client is authoritative.
        // This is the "current client, stale server" report: trusting Some(false)
        // here is exactly what left the server stuck on the old version forever.
        assert!(should_defer_history_for_runtime_identity_with_allow(
            Some(false),
            true,
            false
        ));
        // Same-release/newer server (client could not prove it is older) that
        // self-reports "no newer binary" -> trust it, do not force a reload loop.
        assert!(!should_defer_history_for_runtime_identity_with_allow(
            Some(false),
            false,
            false
        ));
    }

    #[test]
    fn parse_release_semver_refuses_unorderable_dev_builds() {
        assert_eq!(parse_release_semver("v0.17.0 (d741696f)"), Some((0, 17, 0)));
        assert_eq!(parse_release_semver("0.14.2"), Some((0, 14, 2)));
        // Dev/dirty builds share a base semver and must not be ordered.
        assert_eq!(parse_release_semver("v0.18.4-dev (102e9750, dirty)"), None);
        assert_eq!(parse_release_semver("v0.14.2-dev (38452185, dirty)"), None);
        assert_eq!(parse_release_semver("unknown"), None);
    }

    #[test]
    fn server_release_older_than_client_is_selfdev_safe() {
        // Clean release older than clean client -> stale.
        assert!(server_release_is_older_than_client(
            Some("v0.14.2 (38452185)"),
            "v0.17.0 (d741696f)"
        ));
        // Equal or newer -> not stale.
        assert!(!server_release_is_older_than_client(
            Some("v0.17.0"),
            "v0.17.0"
        ));
        assert!(!server_release_is_older_than_client(
            Some("v0.18.0"),
            "v0.17.0"
        ));
        // Either side dev/dirty/unparseable -> never claim staleness (protects
        // self-dev and branched daemons from a forced downgrade).
        assert!(!server_release_is_older_than_client(
            Some("v0.14.2-dev (abc, dirty)"),
            "v0.17.0"
        ));
        assert!(!server_release_is_older_than_client(
            Some("v0.14.2"),
            "v0.17.0-dev (abc, dirty)"
        ));
        assert!(!server_release_is_older_than_client(None, "v0.17.0"));
    }
}

/// Fingerprint of the last fully-applied History payload for one client
/// instance, so byte-identical bootstrap redeliveries can be dropped without
/// rebuilding the display transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AppliedHistoryFingerprint {
    session_id: String,
    fingerprint: u64,
}

/// Last fully-applied History payload fingerprint, keyed by
/// `App::remote_client_instance_id`.
///
/// This is deliberately NOT stored on `RemoteConnection`: a reconnect builds a
/// fresh connection (resetting `has_loaded_history`), and that reconnect
/// re-bootstrap is exactly the duplicate full-payload delivery this state must
/// survive to dedup. Keeping it module-local also keeps the dedup concern
/// entirely inside the History handler. Entries are tiny (session id + u64);
/// the map is bounded because many short-lived `App`s only exist in tests.
static LAST_APPLIED_HISTORY: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, AppliedHistoryFingerprint>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

fn last_applied_history_fingerprint(instance_id: &str) -> Option<AppliedHistoryFingerprint> {
    LAST_APPLIED_HISTORY
        .lock()
        .ok()
        .and_then(|map| map.get(instance_id).cloned())
}

fn record_applied_history_fingerprint(instance_id: &str, session_id: &str, fingerprint: u64) {
    if let Ok(mut map) = LAST_APPLIED_HISTORY.lock() {
        // Bound growth from short-lived test/replay Apps; one entry per live
        // client is the steady state, so clearing is harmless (worst case one
        // extra full re-apply per client).
        if !map.contains_key(instance_id) && map.len() >= 64 {
            map.clear();
        }
        map.insert(
            instance_id.to_string(),
            AppliedHistoryFingerprint {
                session_id: session_id.to_string(),
                fingerprint,
            },
        );
    }
}

/// Hash a JSON value structurally without serializing it to a string, so large
/// tool inputs contribute to the fingerprint in one allocation-free pass.
fn hash_json_value(value: &serde_json::Value, hasher: &mut impl std::hash::Hasher) {
    use std::hash::Hash;
    match value {
        serde_json::Value::Null => 0u8.hash(hasher),
        serde_json::Value::Bool(b) => {
            1u8.hash(hasher);
            b.hash(hasher);
        }
        serde_json::Value::Number(n) => {
            2u8.hash(hasher);
            n.to_string().hash(hasher);
        }
        serde_json::Value::String(s) => {
            3u8.hash(hasher);
            s.hash(hasher);
        }
        serde_json::Value::Array(items) => {
            4u8.hash(hasher);
            items.len().hash(hasher);
            for item in items {
                hash_json_value(item, hasher);
            }
        }
        serde_json::Value::Object(map) => {
            5u8.hash(hasher);
            map.len().hash(hasher);
            for (key, item) in map {
                key.hash(hasher);
                hash_json_value(item, hasher);
            }
        }
    }
}

/// Cheap structural fingerprint of a full History payload.
///
/// Reconnects, session-switch storms, and the history-recovery watchdog can
/// redeliver the same multi-megabyte bootstrap payload within seconds.
/// Re-applying it rebuilds the whole display transcript (~3-4x the wire size
/// in transient arenas) for zero visible change. One pass over message bytes,
/// no allocations proportional to payload size.
fn history_payload_fingerprint(messages: &[crate::protocol::HistoryMessage]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    messages.len().hash(&mut hasher);
    for message in messages {
        message.role.hash(&mut hasher);
        message.content.hash(&mut hasher);
        match &message.tool_calls {
            Some(calls) => {
                calls.len().hash(&mut hasher);
                for call in calls {
                    call.hash(&mut hasher);
                }
            }
            None => usize::MAX.hash(&mut hasher),
        }
        match &message.tool_data {
            Some(tool) => {
                1u8.hash(&mut hasher);
                tool.id.hash(&mut hasher);
                tool.name.hash(&mut hasher);
                tool.intent.hash(&mut hasher);
                tool.thought_signature.hash(&mut hasher);
                hash_json_value(&tool.input, &mut hasher);
            }
            None => 0u8.hash(&mut hasher),
        }
    }
    hasher.finish()
}

/// Pure skip decision for a full History payload: skip only when the session
/// did not change, the display still has content to preserve, and the payload
/// fingerprints identical to the one most recently applied for this session.
/// Session switches and rewinds always re-apply (a rewind's truncated payload
/// fingerprints differently, and a session switch flips `session_changed`).
fn should_skip_identical_history_payload(
    session_changed: bool,
    display_is_empty: bool,
    last_applied: Option<&AppliedHistoryFingerprint>,
    session_id: &str,
    fingerprint: u64,
) -> bool {
    !session_changed
        && !display_is_empty
        && last_applied
            .is_some_and(|entry| entry.session_id == session_id && entry.fingerprint == fingerprint)
}

/// True when the incoming rendered-image set is (cheaply) identical to the
/// already-retained set: same count and, per image, same data length plus
/// equal cheap metadata. Image data is compared by length only so duplicate
/// multi-megabyte base64 payloads are never traversed byte-by-byte.
fn history_images_match_retained(
    incoming: &[crate::session::RenderedImage],
    retained: &[crate::session::RenderedImage],
) -> bool {
    incoming.len() == retained.len()
        && incoming.iter().zip(retained.iter()).all(|(a, b)| {
            a.data.len() == b.data.len()
                && a.media_type == b.media_type
                && a.label == b.label
                && a.source == b.source
                && a.anchor == b.anchor
        })
}

#[cfg(test)]
mod history_dedup_tests {
    use super::{
        AppliedHistoryFingerprint, history_images_match_retained, history_payload_fingerprint,
        should_skip_identical_history_payload,
    };
    use crate::protocol::HistoryMessage;
    use crate::session::{RenderedImage, RenderedImageSource};

    fn message(role: &str, content: &str) -> HistoryMessage {
        HistoryMessage {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_data: None,
        }
    }

    fn image(data: &str) -> RenderedImage {
        RenderedImage {
            media_type: "image/png".to_string(),
            data: data.to_string(),
            label: None,
            source: RenderedImageSource::UserInput,
            anchor: None,
        }
    }

    #[test]
    fn identical_payloads_fingerprint_equal() {
        let a = vec![message("user", "hi"), message("assistant", "hello")];
        let b = vec![message("user", "hi"), message("assistant", "hello")];
        assert_eq!(
            history_payload_fingerprint(&a),
            history_payload_fingerprint(&b)
        );
    }

    #[test]
    fn fingerprint_changes_on_content_role_count_and_tool_data() {
        let base = vec![message("user", "hi"), message("assistant", "hello")];
        let fp = history_payload_fingerprint(&base);

        let content = vec![message("user", "hi"), message("assistant", "hello!")];
        assert_ne!(fp, history_payload_fingerprint(&content));

        let role = vec![message("user", "hi"), message("system", "hello")];
        assert_ne!(fp, history_payload_fingerprint(&role));

        let count = vec![message("user", "hi")];
        assert_ne!(fp, history_payload_fingerprint(&count));

        let mut tool = base.clone();
        tool[1].tool_data = Some(super::ToolCall {
            id: "t1".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "ls"}),
            intent: None,
            thought_signature: None,
        });
        assert_ne!(fp, history_payload_fingerprint(&tool));

        // Same tool call with different input must differ too.
        let mut tool_other = tool.clone();
        tool_other[1].tool_data.as_mut().unwrap().input = serde_json::json!({"command": "pwd"});
        assert_ne!(
            history_payload_fingerprint(&tool),
            history_payload_fingerprint(&tool_other)
        );
    }

    #[test]
    fn skip_decision_requires_same_session_same_fingerprint_and_intact_display() {
        let entry = AppliedHistoryFingerprint {
            session_id: "ses_a".to_string(),
            fingerprint: 42,
        };

        // Exact match with intact display and unchanged session -> skip.
        assert!(should_skip_identical_history_payload(
            false,
            false,
            Some(&entry),
            "ses_a",
            42
        ));
        // Session switch must always re-apply.
        assert!(!should_skip_identical_history_payload(
            true,
            false,
            Some(&entry),
            "ses_a",
            42
        ));
        // A cleared display must be repopulated even for an identical payload.
        assert!(!should_skip_identical_history_payload(
            false,
            true,
            Some(&entry),
            "ses_a",
            42
        ));
        // Different session id -> re-apply.
        assert!(!should_skip_identical_history_payload(
            false,
            false,
            Some(&entry),
            "ses_b",
            42
        ));
        // Different payload (e.g. rewind truncation) -> re-apply.
        assert!(!should_skip_identical_history_payload(
            false,
            false,
            Some(&entry),
            "ses_a",
            43
        ));
        // Nothing applied yet -> re-apply.
        assert!(!should_skip_identical_history_payload(
            false, false, None, "ses_a", 42
        ));
    }

    #[test]
    fn images_match_retained_compares_count_and_lengths() {
        let retained = vec![image("aaaa"), image("bbbbbb")];
        let same = vec![image("aaaa"), image("bbbbbb")];
        assert!(history_images_match_retained(&same, &retained));
        // Length-only comparison: equal lengths count as identical.
        let same_len = vec![image("cccc"), image("dddddd")];
        assert!(history_images_match_retained(&same_len, &retained));

        assert!(!history_images_match_retained(&[], &retained));
        let fewer = vec![image("aaaa")];
        assert!(!history_images_match_retained(&fewer, &retained));
        let diff_len = vec![image("aaaa"), image("bbbbb")];
        assert!(!history_images_match_retained(&diff_len, &retained));
        let mut diff_meta = vec![image("aaaa"), image("bbbbbb")];
        diff_meta[0].media_type = "image/jpeg".to_string();
        assert!(!history_images_match_retained(&diff_meta, &retained));
        assert!(history_images_match_retained(&[], &[]));
    }
}

pub(in crate::tui::app) fn handle_server_event(
    app: &mut App,
    event: ServerEvent,
    remote: &mut impl RemoteEventState,
) -> bool {
    let eager_stream_redraw = !crate::perf::tui_policy().enable_decorative_animations;
    if app.is_processing {
        app.last_stream_activity = Some(Instant::now());
    }

    let had_remote_resume_activity = app.remote_resume_activity.is_some();

    // A turn can start in this session without this client sending a message:
    // swarm wake delivery, background-task wakes, scheduled tasks, resume-all,
    // or another window attached to the same session. When live turn-stream
    // events arrive while this client thinks the session is idle, adopt the
    // turn so the status line/spinner reflect the in-progress work and the
    // terminal Done/Error event can settle it like a resumed remote turn.
    let externally_started_turn_event = app.current_message_id.is_none()
        && !app.is_processing
        && matches!(
            &event,
            ServerEvent::TextDelta { .. }
                | ServerEvent::TextReplace { .. }
                | ServerEvent::ReasoningDelta { .. }
                | ServerEvent::ReasoningDone { .. }
                | ServerEvent::ToolStart { .. }
                | ServerEvent::ToolInput { .. }
                | ServerEvent::ToolExec { .. }
                | ServerEvent::ToolDone { .. }
                | ServerEvent::BatchProgress { .. }
                | ServerEvent::ConnectionPhase { .. }
                | ServerEvent::StatusDetail { .. }
        );
    if externally_started_turn_event {
        crate::logging::info(
            "Adopting externally started turn: stream event received while idle with no current_message_id",
        );
        app.is_processing = true;
        if app.processing_started.is_none() {
            app.processing_started = Some(Instant::now());
        }
        app.last_stream_activity = Some(Instant::now());
    }

    if matches!(
        &event,
        ServerEvent::TextDelta { .. }
            | ServerEvent::TextReplace { .. }
            | ServerEvent::ReasoningDelta { .. }
            | ServerEvent::ReasoningDone { .. }
            | ServerEvent::ToolStart { .. }
            | ServerEvent::ToolInput { .. }
            | ServerEvent::ToolExec { .. }
            | ServerEvent::ToolDone { .. }
            | ServerEvent::SidePaneImages { .. }
            | ServerEvent::GeneratedImage { .. }
            | ServerEvent::BatchProgress { .. }
            | ServerEvent::TokenUsage { .. }
            | ServerEvent::KvCacheRequest { .. }
            | ServerEvent::ConnectionType { .. }
            | ServerEvent::ConnectionPhase { .. }
            | ServerEvent::StatusDetail { .. }
            | ServerEvent::MessageEnd
            | ServerEvent::RetryRollback { .. }
            | ServerEvent::UpstreamProvider { .. }
            | ServerEvent::Interrupted
            | ServerEvent::Done { .. }
            | ServerEvent::Error { .. }
    ) {
        app.remote_resume_activity = None;
    }

    let call_output_tokens_seen = remote.call_output_tokens_seen();

    match event {
        ServerEvent::TextDelta { text } => {
            if let Some(thought_line) = App::extract_thought_line(&text) {
                let ops = app.stream_buffer.flush();
                app.apply_stream_ops(ops);
                app.insert_thought_line(thought_line);
                return eager_stream_redraw;
            }
            let mut needs_redraw = false;
            if matches!(
                app.status,
                ProcessingStatus::Sending
                    | ProcessingStatus::Connecting(_)
                    | ProcessingStatus::Thinking(_)
            ) || (app.is_processing && matches!(app.status, ProcessingStatus::Idle))
            {
                app.status = ProcessingStatus::Streaming;
                needs_redraw = true;
            }
            app.resume_streaming_tps();
            let ops = app.stream_buffer.push_text(&text);
            if app.apply_stream_ops(ops) {
                needs_redraw = true;
            }
            app.last_stream_activity = Some(Instant::now());
            eager_stream_redraw && needs_redraw
        }
        ServerEvent::TextReplace { text } => {
            let ops = app.stream_buffer.flush();
            app.apply_stream_ops(ops);
            app.replace_streaming_text(text);
            app.resume_streaming_tps();
            true
        }
        ServerEvent::ReasoningDelta { text } => {
            // Reasoning streams live (dim+italic) before the answer, paced through
            // the same segment-aware StreamBuffer as normal text so provider
            // bursts trickle in smoothly and ordering is preserved without
            // flushing the backlog.
            // Surface active reasoning in the status line. The server emits a
            // `ConnectionPhase::Streaming` when reasoning starts (to kick off the
            // client TPS timer), so the status arrives here as `Streaming`; flip it
            // to `Thinking` while reasoning deltas flow. The next `TextDelta` moves
            // it back to `Streaming`.
            if !matches!(app.status, ProcessingStatus::RunningTool(_)) {
                let thinking_start = *app.thinking_start.get_or_insert_with(Instant::now);
                if !matches!(app.status, ProcessingStatus::Thinking(_)) {
                    app.status = ProcessingStatus::Thinking(thinking_start);
                }
            }
            app.resume_streaming_tps();
            let ops = app.stream_buffer.push_reasoning(&text);
            app.apply_stream_ops(ops);
            app.last_stream_activity = Some(Instant::now());
            eager_stream_redraw
        }
        ServerEvent::ReasoningDone { .. } => {
            app.thinking_start = None;
            // Queue the region close behind any still-buffered reasoning so it
            // lands exactly after the final reasoning character reveals.
            let ops = app.stream_buffer.push_close_reasoning();
            app.apply_stream_ops(ops);
            eager_stream_redraw
        }
        ServerEvent::ToolStart { id, name } => {
            // Tool-call JSON is provider-generated output and is included in output-token
            // usage. Keep the TPS timer running until the server reports ToolExec; actual
            // tool execution time is excluded after that point.
            app.resume_streaming_tps();
            app.clear_active_experimental_feature_notice();
            remote.handle_tool_start(&id, &name);
            app.commit_pending_streaming_assistant_message();
            if matches!(name.as_str(), "memory") {
                crate::memory::set_state(crate::tui::info_widget::MemoryState::Embedding);
            }
            app.status = ProcessingStatus::RunningTool(name.clone());
            app.streaming_tool_calls.push(ToolCall {
                id,
                name,
                input: serde_json::Value::Null,
                intent: None,
                thought_signature: None,
            });
            eager_stream_redraw
        }
        ServerEvent::ToolInput { delta } => {
            remote.handle_tool_input(&delta);
            false
        }
        ServerEvent::ToolExec { id, name } => {
            // Provider output generation for this tool call is complete, but final usage
            // snapshots often arrive later. Keep collecting deltas while excluding tool
            // runtime from the elapsed TPS denominator.
            app.pause_streaming_tps(true);
            let parsed_input = remote.get_current_tool_input();
            let tool_call = ToolCall {
                id: id.clone(),
                name: name.clone(),
                input: parsed_input.clone(),
                intent: ToolCall::intent_from_input(&parsed_input),
                thought_signature: None,
            };
            if let Some(key) = App::experimental_feature_key_for_tool(&tool_call) {
                app.note_experimental_feature_use(key);
            }
            if tool_call.name == "swarm" {
                app.maybe_surface_swarm_config_hint();
            }
            if let Some(tc) = app.streaming_tool_calls.iter_mut().find(|tc| tc.id == id) {
                tc.input = parsed_input;
                tc.refresh_intent_from_input();
            }
            remote.handle_tool_exec(&id, &name);
            app.observe_tool_call(&tool_call);
            eager_stream_redraw
                || app.side_panel.focused_page_id.as_deref()
                    == Some(app_mod::observe::OBSERVE_PAGE_ID)
        }
        ServerEvent::ToolDone {
            id,
            name,
            output,
            error,
        } => super::server_event_handlers::handle_tool_done(app, remote, id, name, output, error),
        ServerEvent::GeneratedImage {
            id,
            path,
            metadata_path,
            output_format,
            revised_prompt,
        } => super::server_event_handlers::handle_generated_image(
            app,
            id,
            path,
            metadata_path,
            output_format,
            revised_prompt,
        ),
        ServerEvent::BatchProgress { progress } => {
            app.batch_progress = Some(progress);
            false
        }
        ServerEvent::TokenUsage {
            input,
            output,
            cache_read_input,
            cache_creation_input,
        } => {
            let previous_input = app.streaming.streaming_input_tokens;
            let previous_output = app.streaming.streaming_output_tokens;
            let previous_cache_read = app.streaming.streaming_cache_read_tokens;
            let previous_cache_creation = app.streaming.streaming_cache_creation_tokens;
            let was_recorded = app.kv_cache.current_api_usage_recorded;
            app.accumulate_streaming_output_tokens(output, call_output_tokens_seen);
            // Per-call replace semantics for input/cache counters: a stale
            // cache-read figure from a previous call must not leak into this
            // call's context accounting (issue #441).
            app.apply_stream_usage_input_report(
                Some(input),
                cache_read_input,
                cache_creation_input,
            );
            app.streaming.streaming_output_tokens = output;
            if app.record_completed_stream_cache_usage() {
                app.token_accounting.total_input_tokens = app
                    .token_accounting
                    .total_input_tokens
                    .saturating_add(input);
                app.token_accounting.total_output_tokens = app
                    .token_accounting
                    .total_output_tokens
                    .saturating_add(output);
                // The server only reports tokens, never a dollar cost, so the
                // remote client prices each completed call itself. This is the
                // first usage snapshot for this call, so bill the full counts.
                app.accrue_remote_call_cost(
                    input,
                    output,
                    app.streaming.streaming_cache_read_tokens.unwrap_or(0),
                    app.streaming.streaming_cache_creation_tokens.unwrap_or(0),
                );
                app.last_api_completed = Some(Instant::now());
                app.last_api_completed_provider = Some(<App as TuiState>::provider_name(app));
                app.last_api_completed_model = Some(<App as TuiState>::provider_model(app));
                // Effective prompt (input + read + creation), matching the
                // local push_turn_footer path: this feeds the cache
                // countdown/cold indicators as "what gets resent".
                let effective = crate::tui::info_widget::effective_prompt_tokens(
                    input,
                    app.streaming.streaming_cache_read_tokens.unwrap_or(0),
                    app.streaming.streaming_cache_creation_tokens.unwrap_or(0),
                );
                app.last_turn_input_tokens = (effective > 0).then_some(effective);
            } else if was_recorded && app.kv_cache.current_api_usage_recorded {
                app.token_accounting.total_input_tokens = app
                    .token_accounting
                    .total_input_tokens
                    .saturating_add(input.saturating_sub(previous_input));
                app.token_accounting.total_output_tokens = app
                    .token_accounting
                    .total_output_tokens
                    .saturating_add(output.saturating_sub(previous_output));
                // Bill only the new tokens since the previous snapshot for this
                // same call, so a call that reports usage multiple times while
                // streaming is billed exactly once overall.
                app.accrue_remote_call_cost(
                    input.saturating_sub(previous_input),
                    output.saturating_sub(previous_output),
                    app.streaming
                        .streaming_cache_read_tokens
                        .unwrap_or(0)
                        .saturating_sub(previous_cache_read.unwrap_or(0)),
                    app.streaming
                        .streaming_cache_creation_tokens
                        .unwrap_or(0)
                        .saturating_sub(previous_cache_creation.unwrap_or(0)),
                );

                let had_cache_telemetry =
                    previous_cache_read.is_some() || previous_cache_creation.is_some();
                let has_cache_telemetry = app.streaming.streaming_cache_read_tokens.is_some()
                    || app.streaming.streaming_cache_creation_tokens.is_some();
                if has_cache_telemetry {
                    let reported_delta = if had_cache_telemetry {
                        input.saturating_sub(previous_input)
                    } else {
                        input
                    };
                    app.token_accounting.total_cache_reported_input_tokens = app
                        .token_accounting
                        .total_cache_reported_input_tokens
                        .saturating_add(reported_delta);
                    app.token_accounting.total_cache_read_tokens =
                        app.token_accounting.total_cache_read_tokens.saturating_add(
                            app.streaming
                                .streaming_cache_read_tokens
                                .unwrap_or(0)
                                .saturating_sub(previous_cache_read.unwrap_or(0)),
                        );
                    app.token_accounting.total_cache_creation_tokens = app
                        .token_accounting
                        .total_cache_creation_tokens
                        .saturating_add(
                            app.streaming
                                .streaming_cache_creation_tokens
                                .unwrap_or(0)
                                .saturating_sub(previous_cache_creation.unwrap_or(0)),
                        );
                    app.token_accounting.last_cache_reported_input_tokens = Some(input);
                    app.token_accounting.last_cache_read_tokens =
                        Some(app.streaming.streaming_cache_read_tokens.unwrap_or(0));
                    app.token_accounting.last_cache_creation_tokens =
                        Some(app.streaming.streaming_cache_creation_tokens.unwrap_or(0));
                }

                let effective_prompt_tokens = crate::tui::info_widget::effective_prompt_tokens(
                    input,
                    app.streaming.streaming_cache_read_tokens.unwrap_or(0),
                    app.streaming.streaming_cache_creation_tokens.unwrap_or(0),
                );
                if let Some(baseline) = app.kv_cache.kv_cache_baseline.as_mut() {
                    // Store the effective prompt (input + read + creation): for
                    // split-accounting providers bare `input` is only the
                    // uncached remainder, while the whole effective prompt is
                    // what gets resent when the cache goes cold.
                    baseline.input_tokens = effective_prompt_tokens;
                    baseline.completed_at = Instant::now();
                }
                app.token_accounting.cache_next_optimal_input_tokens =
                    Some(effective_prompt_tokens);
                app.last_api_completed = Some(Instant::now());
                app.last_api_completed_provider = Some(<App as TuiState>::provider_name(app));
                app.last_api_completed_model = Some(<App as TuiState>::provider_model(app));
                app.last_turn_input_tokens =
                    (effective_prompt_tokens > 0).then_some(effective_prompt_tokens);
            }
            eager_stream_redraw && matches!(app.status, ProcessingStatus::Streaming)
        }
        ServerEvent::KvCacheRequest {
            system_static_hash,
            tools_hash,
            messages_hash,
            message_hashes,
            message_count,
            tool_count,
            system_static_chars,
            tools_json_chars,
            messages_json_chars,
            ephemeral_hash,
            ephemeral_chars,
            ephemeral_message_count,
        } => {
            remote.reset_call_output_tokens_seen();
            app.begin_remote_kv_cache_request(app_mod::KvCacheRequestSignature {
                system_static_hash,
                tools_hash,
                messages_hash,
                message_hashes,
                message_count,
                tool_count,
                system_static_chars,
                tools_json_chars,
                messages_json_chars,
                ephemeral_hash,
                ephemeral_chars,
                ephemeral_message_count,
            });
            false
        }
        ServerEvent::ConnectionType { connection } => {
            app.connection_type = Some(connection);
            app.update_terminal_title();
            false
        }
        ServerEvent::Pong { .. } => false,
        ServerEvent::ConnectionPhase { phase } => {
            let cp = match phase.as_str() {
                "authenticating" => crate::message::ConnectionPhase::Authenticating,
                "connecting" => crate::message::ConnectionPhase::Connecting,
                "waiting for response" => crate::message::ConnectionPhase::WaitingForResponse,
                "streaming" => crate::message::ConnectionPhase::Streaming,
                _ if phase.starts_with("retrying (") && phase.ends_with(')') => {
                    let inner = &phase[10..phase.len() - 1];
                    let (attempt, max) = inner
                        .split_once('/')
                        .and_then(|(a, m)| Some((a.parse::<u32>().ok()?, m.parse::<u32>().ok()?)))
                        .unwrap_or((1, 1));
                    crate::message::ConnectionPhase::Retrying { attempt, max }
                }
                _ => crate::message::ConnectionPhase::Connecting,
            };
            app.status = if matches!(cp, crate::message::ConnectionPhase::Streaming) {
                app.resume_streaming_tps();
                app.connection_phase_started = None;
                ProcessingStatus::Streaming
            } else {
                // Start the "suspiciously long" timer when we first enter the
                // connecting group so later round-trips in a turn don't inherit
                // the whole-turn elapsed and immediately render yellow.
                if !matches!(app.status, ProcessingStatus::Connecting(_)) {
                    app.connection_phase_started = Some(Instant::now());
                }
                ProcessingStatus::Connecting(cp)
            };
            eager_stream_redraw
        }
        ServerEvent::StatusDetail { detail } => {
            app.status_detail = Some(detail);
            eager_stream_redraw
        }
        ServerEvent::MessageEnd => {
            app.pause_streaming_tps(true);
            app.stream_message_ended = true;
            true
        }
        ServerEvent::RetryRollback { attempt, max } => {
            // A transient transport fault interrupted the provider mid-response
            // and the server is retrying the request from the top. The retry is
            // a fresh sample, not a deterministic replay, so all partial output
            // from the aborted attempt must be discarded: the live streaming
            // buffer, in-progress tool calls, and any assistant text already
            // committed to the transcript by a mid-stream ToolStart boundary.
            crate::logging::warn(&format!(
                "Retry rollback (attempt {}/{}): discarding partial streamed output",
                attempt, max
            ));
            app.rollback_streaming_attempt();
            remote.clear_pending();
            app.connection_phase_started = Some(Instant::now());
            app.status = ProcessingStatus::Connecting(crate::message::ConnectionPhase::Retrying {
                attempt,
                max,
            });
            true
        }
        ServerEvent::UpstreamProvider { provider } => {
            app.upstream_provider = Some(provider);
            false
        }
        ServerEvent::Ack { id } => {
            let _ = app.acknowledge_pending_soft_interrupt(id);
            false
        }
        ServerEvent::Interrupted => {
            crate::logging::info(&format!(
                "REMOTE_INTERRUPT_EVENT_RECEIVED kind=interrupted session={:?} current_message_id={:?} is_processing={} status={:?} streaming_text_bytes={} pending_soft_interrupts={} queued_messages={}",
                app.remote_session_id,
                app.current_message_id,
                app.is_processing,
                app.status,
                app.streaming.streaming_text.len(),
                app.pending_soft_interrupts.len(),
                app.queued_messages.len()
            ));
            let keep_pending_retry = app
                .rate_limit_pending_message
                .as_ref()
                .is_some_and(|pending| pending.auto_retry && app.rate_limit_reset.is_some());
            if !keep_pending_retry {
                app.clear_pending_remote_retry();
            }
            let recovered_local = recover_local_interleave_to_queue(app, "interrupt");
            let ops = app.stream_buffer.flush();
            app.apply_stream_ops(ops);
            if !app.streaming.streaming_text.is_empty() {
                let content = app.take_streaming_text();
                let content = app.collapse_reasoning_for_commit(content);
                if !content.trim().is_empty() {
                    app.push_display_message(DisplayMessage {
                        role: "assistant".to_string(),
                        content,
                        tool_calls: Vec::new(),
                        duration_secs: app.display_turn_duration_secs(),
                        title: None,
                        tool_data: None,
                    });
                }
            }
            app.clear_streaming_render_state();
            app.stream_buffer.clear();
            app.streaming_tool_calls.clear();
            app.batch_progress = None;
            app.thought_line_inserted = false;
            app.thinking_prefix_emitted = false;
            app.thinking_buffer.clear();
            if recovered_local || !app.pending_soft_interrupts.is_empty() {
                crate::logging::info(&format!(
                    "Preserving {} pending soft interrupt(s) across interrupt",
                    app.pending_soft_interrupts.len()
                ));
            }
            app.schedule_queued_dispatch_after_interrupt();
            app.push_display_message(DisplayMessage::system("Interrupted"));
            app.is_processing = false;
            app.status = ProcessingStatus::Idle;
            app.stream_message_ended = false;
            app.processing_started = None;
            app.current_message_id = None;
            remote.clear_pending();
            remote.reset_call_output_tokens_seen();
            let auto_poked = app.schedule_turn_end_followups();
            if !auto_poked {
                app.clear_visible_turn_started();
            }
            auto_poked
        }
        ServerEvent::ProviderGuardrail {
            stop_reason,
            message,
        } => {
            crate::logging::warn(&format!(
                "PROVIDER_GUARDRAIL_EVENT session={:?} stop_reason={:?}",
                app.remote_session_id, stop_reason
            ));
            let label = stop_reason
                .as_deref()
                .filter(|r| !r.trim().is_empty())
                .unwrap_or("guardrail");
            // Mark the turn so the Done handler can count consecutive
            // guardrail stops and stop auto-poke/overnight loops that would
            // otherwise re-send the refused request forever.
            app.turn_guardrail_stopped = true;
            // Plain text prefix: U+1F6E1 shield renders poorly in some
            // terminals (kitty shows a narrow monochrome glyph).
            app.push_display_message(DisplayMessage::system(format!("[guardrail] {}", message)));
            app.set_status_notice(format!("Provider guardrail: {}", label));
            // Guardrail refusals are model-side policy stops: retrying the
            // same model usually refuses again, but a stronger model often
            // handles the same legitimate request. Offer a one-keypress
            // reroute to the strongest Anthropic route and resend. The offer
            // sets its own (more actionable) status notice when armed.
            app.offer_guardrail_reroute();
            true
        }
        ServerEvent::Done { id } => {
            let mut auto_poked = false;
            let mut completed_current_message = false;
            crate::logging::info(&format!(
                "Client received Done id={}, current_message_id={:?}",
                id, app.current_message_id
            ));
            let has_resumed_turn_evidence = had_remote_resume_activity
                || app.stream_message_ended
                || app.has_streaming_footer_stats()
                || !app.streaming.streaming_text.is_empty()
                || !app.streaming_tool_calls.is_empty()
                || matches!(
                    app.status,
                    ProcessingStatus::Streaming | ProcessingStatus::RunningTool(_)
                );
            let completes_resumed_turn =
                app.current_message_id.is_none() && app.is_processing && has_resumed_turn_evidence;
            if app.current_message_id == Some(id) || completes_resumed_turn {
                let turn_duration_secs = app.display_turn_duration_secs();
                if completes_resumed_turn {
                    crate::logging::info(&format!(
                        "Treating Done id={} as completion for resumed remote activity",
                        id
                    ));
                }
                completed_current_message = true;
                app.clear_pending_remote_retry();
                app.reset_credential_failure_breaker();
                let ops = app.stream_buffer.flush();
                app.apply_stream_ops(ops);
                // The turn can finish with a reasoning region still open (the
                // model streamed reasoning but never sent ReasoningDone and never
                // began answer text). Close it as a hard message boundary so the
                // live-rendered reasoning is anchored/retained instead of being
                // silently stripped by `collapse_reasoning_for_commit` below.
                if app.reasoning_streaming {
                    app.close_reasoning_region(None);
                }
                app.pause_streaming_tps(false);
                if !app.streaming.streaming_text.is_empty() {
                    let duration = app.display_turn_duration_secs();
                    let content = app.take_streaming_text();
                    let content = app.collapse_reasoning_for_commit(content);
                    if !content.trim().is_empty() {
                        app.push_display_message(DisplayMessage {
                            role: "assistant".to_string(),
                            content,
                            tool_calls: vec![],
                            duration_secs: duration,
                            title: None,
                            tool_data: None,
                        });
                    }
                    app.push_turn_footer(duration);
                } else if app.has_streaming_footer_stats() {
                    let duration = app.display_turn_duration_secs();
                    app.push_turn_footer(duration);
                }
                crate::tui::mermaid::clear_streaming_preview_diagram();
                app.is_processing = false;
                app.status = ProcessingStatus::Idle;
                app.stream_message_ended = false;
                // Turn completed successfully; drop the saved prompt so a later
                // unrelated failure cannot restore stale text into the input box.
                app.last_submitted_input = None;
                app.processing_started = None;
                app.replay_processing_started_ms = None;
                app.replay_elapsed_override = None;
                app.remote_resume_activity = None;
                app.batch_progress = None;
                app.streaming_tool_calls.clear();
                app.current_message_id = None;
                app.thought_line_inserted = false;
                app.thinking_prefix_emitted = false;
                app.thinking_buffer.clear();
                remote.clear_pending();
                remote.reset_call_output_tokens_seen();
                app.note_runtime_memory_event_force("turn_completed", "remote_turn_finished");
                crate::process_memory::release_retained_heap_debounced(
                    "client_turn_completed",
                    std::time::Duration::from_secs(30),
                );
                auto_poked = app.schedule_turn_end_followups();
                if !auto_poked {
                    app.clear_visible_turn_started();
                    if app.queued_messages.is_empty() {
                        app.maybe_notify_turn_complete(turn_duration_secs);
                    }
                }
            } else if app.is_processing {
                let is_stale = app.current_message_id.is_some_and(|mid| id < mid);
                if is_stale {
                    crate::logging::info(&format!(
                        "Ignoring stale Done id={} (current_message_id={:?}), likely from Subscribe/ResumeSession",
                        id, app.current_message_id
                    ));
                } else {
                    crate::logging::info(&format!(
                        "Ignoring unrelated Done id={} while processing current_message_id={:?}; preserving active/queued turn",
                        id, app.current_message_id
                    ));
                }
            }
            completed_current_message || auto_poked
        }
        ServerEvent::Error {
            message,
            retry_after_secs,
            ..
        } => {
            // The server rejects a Message request with this error while its
            // previous turn is still running. This typically happens when a
            // reload/reconnect raced the turn-end dispatch: the history
            // activity snapshot said "idle", the client dequeued and sent a
            // queued follow-up, but the server-side turn had not actually
            // finished. Dropping the pending send here would silently lose the
            // user's queued message (issue #391). Instead, put it back on the
            // queue and re-adopt the running-turn state so the queue
            // dispatches once the real turn completes.
            if message == "Already processing a message"
                && recover_undelivered_queued_continuation(app, "server busy rejection")
            {
                app.is_processing = true;
                app.status = ProcessingStatus::Thinking(Instant::now());
                app.current_message_id = None;
                app.processing_started.get_or_insert_with(Instant::now);
                app.last_stream_activity = Some(Instant::now());
                app.remote_resume_activity = Some(RemoteResumeActivity {
                    session_id: app.remote_session_id.clone().unwrap_or_default(),
                    observed_at: Instant::now(),
                    current_tool_name: None,
                });
                app.set_status_notice("Server still busy; follow-up stays queued");
                crate::logging::info(
                    "Server rejected queued continuation because a turn is still running; re-queued it and re-adopted the running turn",
                );
                return true;
            }
            let reset_duration = retry_after_secs
                .map(Duration::from_secs)
                .or_else(|| parse_rate_limit_error(&message));
            if let Some(reset_duration) = reset_duration {
                app.rate_limit_reset = Some(Instant::now() + reset_duration);
                if let Some(is_system) = app
                    .rate_limit_pending_message
                    .as_ref()
                    .map(|pending| pending.is_system)
                {
                    app.push_display_message(DisplayMessage::system(format!(
                        "⏳ Rate limit hit. Will auto-retry in {} seconds...",
                        reset_duration.as_secs()
                    )));
                    if is_system {
                        app.set_status_notice("Rate limited; queued system retry");
                    } else {
                        app.set_status_notice("Rate limited; queued retry");
                    }
                    app.is_processing = false;
                    app.status = ProcessingStatus::Idle;
                    app.stream_message_ended = false;
                    app.processing_started = None;
                    app.clear_visible_turn_started();
                    app.current_message_id = None;
                    remote.clear_pending();
                    remote.reset_call_output_tokens_seen();
                    return false;
                }
            }
            let is_failover_prompt =
                crate::provider::parse_failover_prompt_message(&message).is_some();
            // Snapshot the failed turn's payload before the cleanup below (and
            // the retry-budget bookkeeping) clears it, so a fallback offer
            // armed at a terminal no-retry point can resend it after the user
            // accepts a switch to a working route.
            let failed_fallback_payload = app.rate_limit_pending_message.as_ref().map(|pending| {
                app_mod::FallbackResendPayload {
                    content: pending.content.clone(),
                    images: pending.images.clone(),
                    is_system: pending.is_system,
                    auto_retry: pending.auto_retry,
                    system_reminder: pending.system_reminder.clone(),
                    raw_input: app.last_submitted_input.clone(),
                }
            });
            app.push_display_message(DisplayMessage {
                role: "error".to_string(),
                content: message.clone(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            app.is_processing = false;
            app.status = ProcessingStatus::Idle;
            app.stream_message_ended = false;
            let recovered_local = recover_local_interleave_to_queue(app, "request error");
            crate::tui::mermaid::clear_streaming_preview_diagram();
            app.thought_line_inserted = false;
            app.thinking_prefix_emitted = false;
            app.thinking_buffer.clear();
            if recovered_local || !app.pending_soft_interrupts.is_empty() {
                crate::logging::info(&format!(
                    "Preserving {} pending soft interrupt(s) across remote error",
                    app.pending_soft_interrupts.len()
                ));
            }
            remote.clear_pending();
            remote.reset_call_output_tokens_seen();
            // Connectivity failures (DNS, connection reset, no route, transient
            // TLS, timeouts) are always transient: the request never reached the
            // provider. Hold the turn and resume when the network recovers,
            // regardless of the pending message's auto_retry flag. This must run
            // before the non-retryable auto-poke check so a transient disconnect
            // is never misclassified as a permanent failure that stops auto-poke.
            let is_connectivity_error =
                crate::tui::app::commands::is_auto_poke_connectivity_error(&message)
                    || crate::network_retry::classify_message(&message).is_some();
            if is_connectivity_error
                && app.schedule_pending_remote_network_wait_with_force(&message, true)
            {
                return false;
            }
            // Credential-failure circuit breaker: repeated auth failures mean
            // the login/API key is dead. Resending the identical request can
            // never succeed and (before this breaker) produced runaway retry
            // loops logging thousands of 401s per session. Stop every
            // automatic resend path and tell the user to /login or /model.
            if !is_connectivity_error && app.note_error_for_credential_breaker(&message) {
                app.trip_credential_failure_breaker(&message);
                app.offer_fallback_after_error_with_payload(
                    &message,
                    failed_fallback_payload.clone(),
                );
                return false;
            }
            // Deterministic model/endpoint-capability failures (e.g. Volcengine
            // Ark's coding-plan endpoint returning 404 UnsupportedModel, or a
            // model-not-found) can never succeed by resending the identical
            // request. Fail fast with an actionable hint instead of burning the
            // auto-retry budget on guaranteed 4xx responses (#387).
            if crate::tui::app::commands::is_fatal_model_endpoint_error(&message) {
                app.clear_pending_remote_retry();
                if app.auto_poke_incomplete_todos {
                    crate::tui::app::commands::stop_auto_poke_for_non_retryable_error(
                        app, &message,
                    );
                }
                app.push_display_message(DisplayMessage::system(
                    "🛑 Not retrying: the model is not valid for the configured endpoint (e.g. an Ark coding-plan endpoint rejecting a model without the coding plan feature, or a model-not-found). Check the model name and base URL (the coding endpoint `/api/coding/v3` only accepts coding-plan models; use `/api/v3` otherwise), then send again.".to_string(),
                ));
                app.set_status_notice("Stopped: model/endpoint mismatch");
                app.restore_failed_input_to_box();
                // Switching models is exactly the right fix for a
                // model/endpoint mismatch: offer the next best route.
                app.offer_fallback_after_error_with_payload(
                    &message,
                    failed_fallback_payload.clone(),
                );
                return false;
            }
            if app.auto_poke_incomplete_todos
                && crate::tui::app::commands::is_non_retryable_auto_poke_error(&message)
            {
                if app.schedule_pending_remote_retry_with_limit(
                    "⚠ Remote request failed with a likely non-retryable error.",
                    2,
                ) {
                    return false;
                }
                crate::tui::app::commands::stop_auto_poke_for_non_retryable_error(app, &message);
                // Terminal: no retry will fire. Offer a one-keypress switch to
                // the next best model/auth-method (e.g. an expired OAuth login
                // -> a working provider) with the failed payload staged.
                app.offer_fallback_after_error_with_payload(
                    &message,
                    failed_fallback_payload.clone(),
                );
                return false;
            }
            if app.stop_overnight_auto_poke_for_non_retryable_error(&message) {
                app.offer_fallback_after_error_with_payload(
                    &message,
                    failed_fallback_payload.clone(),
                );
                return false;
            }
            if !is_failover_prompt && !app.schedule_pending_remote_retry("⚠ Remote request failed.")
            {
                app.clear_pending_remote_retry();
                // No automatic retry will resend this turn, so restore the prompt the
                // user typed back into the input box instead of dropping it.
                app.restore_failed_input_to_box();
                // Offer a one-keypress switch to the next best model/auth-method
                // and resend (e.g. expired OpenAI OAuth session -> a provider
                // that is known to work), instead of leaving the user to run
                // /login or /model manually.
                app.offer_fallback_after_error_with_payload(&message, failed_fallback_payload);
                return app.schedule_turn_end_followups();
            }
            false
        }
        ServerEvent::SessionId { session_id } => {
            remote.set_session_id(session_id.clone());
            app.remote_session_id = Some(session_id.clone());
            crate::set_current_session(&session_id);
            app.note_client_focus(true);
            app.update_terminal_title();
            false
        }
        ServerEvent::SessionCloseRequested { reason } => {
            app.push_display_message(DisplayMessage::system(format!(
                "Session close requested by coordinator: {reason}"
            )));
            app.set_status_notice("Session close requested by coordinator".to_string());
            app.should_quit = true;
            true
        }
        ServerEvent::SessionRenamed {
            session_id,
            title,
            display_title,
        } => {
            crate::tui::session_picker::invalidate_session_list_cache();
            let active_session_id = app
                .remote_session_id
                .as_deref()
                .or(app.resume_session_id.as_deref())
                .unwrap_or(app.session.id.as_str());
            if active_session_id == session_id {
                app.session.rename_title(title.clone());
                if title.is_none()
                    && app.session.title.is_none()
                    && display_title != app.session.display_name()
                {
                    app.session.title = Some(display_title.clone());
                }
                app.update_terminal_title();
                if title.is_some() {
                    app.push_display_message(DisplayMessage::system(format!(
                        "Renamed session to {}.",
                        display_title
                    )));
                    app.set_status_notice("Session renamed");
                } else {
                    app.push_display_message(DisplayMessage::system(format!(
                        "Cleared custom name. Session title is now {}.",
                        display_title
                    )));
                    app.set_status_notice("Session name cleared");
                }
                true
            } else {
                false
            }
        }
        ServerEvent::Reloading { .. } => {
            app.append_reload_message("🔄 Server reload initiated...");
            // In-process server reloads (self-dev build-reload) keep the same
            // server PID and never disconnect this client, so the reconnect-time
            // client re-exec never fires. If a newer client binary is on disk and
            // we are idle, re-exec now so client-side (TUI) changes also take
            // effect. No-op for non-selfdev sessions or when already current.
            app.maybe_self_reload_after_server_reload()
        }
        ServerEvent::ReloadProgress {
            step,
            message,
            success,
            output,
        } => {
            let mut content = if let Some(ok) = success {
                let status_icon = if ok { "✓" } else { "✗" };
                format!("[{}] {} {}", step, status_icon, message)
            } else {
                format!("[{}] {}", step, message)
            };

            if let Some(out) = output
                && !out.is_empty()
            {
                content.push('\n');
                for line in out.lines() {
                    content.push_str("  ");
                    content.push_str(line);
                    content.push('\n');
                }
            }

            app.append_reload_message(&content);

            if step == "verify" || step == "git" {
                app.reload_info.push(message.clone());
            }

            app.status_notice = Some((format!("Reload: {}", message), std::time::Instant::now()));
            false
        }
        ServerEvent::History {
            messages,
            images,
            session_id,
            provider_name,
            provider_model,
            subagent_model,
            autoreview_enabled,
            autojudge_enabled,
            available_models,
            available_model_routes,
            mcp_servers,
            skills,
            total_tokens,
            all_sessions,
            client_count,
            is_canary,
            server_version,
            server_name,
            server_icon,
            server_has_update,
            was_interrupted,
            reload_recovery,
            connection_type,
            status_detail,
            upstream_provider,
            resolved_credential,
            reasoning_effort,
            service_tier,
            compaction_mode,
            activity,
            token_usage_totals,
            side_panel,
            ..
        } => {
            let prev_session_id = app.remote_session_id.clone();
            let history_message_count = messages.len();
            let history_mcp_count = mcp_servers.len();
            let history_model = provider_model.clone();

            if should_defer_history_for_runtime_identity(
                server_has_update,
                server_version.as_deref(),
            ) {
                let client_detected_stale = server_release_is_older_than_client(
                    server_version.as_deref(),
                    &client_release_version(),
                );
                app.remote_server_version = server_version;
                app.remote_server_short_name = server_name.clone();
                app.remote_server_icon = server_icon.clone();
                app.remote_server_has_update = server_has_update;
                app.pending_server_reload = true;
                // Remember the session the server told us about *before* bailing
                // out. We deliberately return below without assigning
                // `app.remote_session_id` (history stays deferred until after the
                // server reloads), but the client reload handoff still needs a
                // real session id to resume. Without this, the handoff falls back
                // to a freshly fabricated `ses_<ts>_<rand>` id that no store can
                // ever resolve, leaving the user at a "No session found matching
                // ..." shell prompt after an auto-update (issue #328).
                if !session_id.is_empty() {
                    app.pending_reload_session_id = Some(session_id.clone());
                }
                app.clear_remote_startup_phase();
                if client_detected_stale {
                    // The client independently measured the server's release as
                    // older than its own. This covers both a pre-self-heal daemon
                    // (server_has_update: None) AND a daemon that self-reports
                    // "no update" because its own shared-server channel still
                    // points at its old binary (the "current client, stale
                    // server" report). Repair the channel client-side so the
                    // forced reload below has a strictly-newer binary to exec
                    // into instead of re-execing the same old build.
                    match crate::build::repair_stale_shared_server_channel() {
                        Ok(crate::build::SharedServerRepair::Repaired { repaired_to, .. }) => {
                            crate::logging::info(&format!(
                                "stale-server repair: repointed shared-server channel to {} before reloading older server",
                                repaired_to
                            ));
                        }
                        Ok(crate::build::SharedServerRepair::AlreadyCurrent) => {}
                        Err(err) => {
                            crate::logging::warn(&format!(
                                "stale-server repair: failed to repoint shared-server channel: {}",
                                err
                            ));
                        }
                    }
                    app.set_status_notice(
                        "Connected server is an older release; reloading it before attach",
                    );
                    app.push_display_message(DisplayMessage::system(format!(
                        "ℹ Connected server is running an older release ({}) than this client ({}). Reloading it before applying session state. If reload does not take, run `jcode server stop` and relaunch. Set JCODE_ALLOW_SERVER_VERSION_MISMATCH=1 only for intentional compatibility testing.",
                        app.remote_server_version.as_deref().unwrap_or("unknown"),
                        jcode_build_meta::version(),
                    )));
                } else {
                    app.set_status_notice(
                        "Server/runtime mismatch detected; reloading server before attach",
                    );
                    app.push_display_message(DisplayMessage::system(
                        "ℹ Connected server binary differs from the installed client channel. Reloading the server before applying remote session state. Set JCODE_ALLOW_SERVER_VERSION_MISMATCH=1 only for intentional compatibility testing."
                            .to_string(),
                    ));
                }
                app.update_terminal_title();
                return false;
            }

            remote.set_session_id(session_id.clone());
            app.remote_session_id = Some(session_id.clone());
            crate::set_current_session(&session_id);
            app.note_client_focus(true);
            let session_changed = prev_session_id.as_deref() != Some(session_id.as_str());

            if session_changed {
                app.rate_limit_pending_message = None;
                app.rate_limit_reset = None;
                app.connection_type = None;
                app.status_detail = None;
                app.clear_display_messages();
                app.clear_streaming_render_state();
                app.streaming_tool_calls.clear();
                app.thought_line_inserted = false;
                app.thinking_prefix_emitted = false;
                app.thinking_buffer.clear();
                app.streaming.streaming_input_tokens = 0;
                app.streaming.streaming_output_tokens = 0;
                app.streaming.streaming_cache_read_tokens = None;
                app.streaming.streaming_cache_creation_tokens = None;
                app.kv_cache.current_api_usage_recorded = false;
                app.token_accounting.total_cache_reported_input_tokens = 0;
                app.token_accounting.total_cache_read_tokens = 0;
                app.token_accounting.total_cache_creation_tokens = 0;
                app.token_accounting.total_cache_optimal_input_tokens = 0;
                app.token_accounting.last_cache_reported_input_tokens = None;
                app.token_accounting.last_cache_read_tokens = None;
                app.token_accounting.last_cache_creation_tokens = None;
                app.token_accounting.last_cache_optimal_input_tokens = None;
                app.token_accounting.cache_next_optimal_input_tokens = None;
                app.kv_cache.kv_cache_baseline = None;
                app.kv_cache.pending_kv_cache_request = None;
                app.kv_cache.kv_cache_turn_number = None;
                app.kv_cache.kv_cache_turn_call_index = 0;
                app.kv_cache.kv_cache_miss_samples.clear();
                app.processing_started = None;
                app.clear_visible_turn_started();
                app.replay_processing_started_ms = None;
                app.replay_elapsed_override = None;
                app.reset_streaming_tps();
                app.last_stream_activity = None;
                app.stream_message_ended = false;
                app.remote_resume_activity = None;
                app.is_processing = false;
                app.status = ProcessingStatus::Idle;
                app.follow_chat_bottom();
                if prev_session_id.is_some() {
                    app.queued_messages.clear();
                    app.interleave_message = None;
                    app.clear_pending_soft_interrupt_tracking();
                }
                app.remote_total_tokens = None;
                app.remote_token_usage_totals = None;
                app.remote_side_pane_images.clear();
                app.invalidate_side_pane_images_signature();
                app.remote_swarm_members.clear();
                app.swarm_plan_items.clear();
                app.swarm_plan_version = None;
                app.swarm_plan_swarm_id = None;
                remote.reset_call_output_tokens_seen();
            }
            let model_catalog_snapshot = jcode_provider_core::ModelCatalogSnapshot::new(
                provider_name,
                provider_model,
                available_models,
                available_model_routes,
            );
            app.replace_remote_model_catalog_snapshot(model_catalog_snapshot);
            app.clear_remote_startup_phase();
            app.session.subagent_model = subagent_model;
            app.session.autoreview_enabled = autoreview_enabled;
            app.session.autojudge_enabled = autojudge_enabled;
            app.autoreview_enabled =
                autoreview_enabled.unwrap_or(crate::config::config().autoreview.enabled);
            app.autojudge_enabled =
                autojudge_enabled.unwrap_or(crate::config::config().autojudge.enabled);
            if upstream_provider.is_some() {
                app.upstream_provider = upstream_provider;
            }
            if session_changed || resolved_credential.is_some() {
                app.remote_resolved_credential = resolved_credential;
            }
            if session_changed || connection_type.is_some() {
                app.connection_type = connection_type;
            }
            if session_changed || status_detail.is_some() {
                app.status_detail = status_detail;
            }
            app.remote_reasoning_effort = reasoning_effort;
            app.remote_service_tier = service_tier;
            app.remote_compaction_mode = Some(compaction_mode);
            app.set_side_panel_snapshot(side_panel);
            if history_images_match_retained(&images, &app.remote_side_pane_images) {
                // The already-retained image set is identical (count + per-image
                // byte length + metadata). Drop the incoming copy immediately so
                // two full base64 payloads are never alive side by side.
                if !images.is_empty() {
                    crate::logging::info(&format!(
                        "History images identical to retained set ({} images); dropping incoming copy",
                        images.len()
                    ));
                }
                drop(images);
            } else {
                app.remote_side_pane_images = images;
                app.invalidate_side_pane_images_signature();
            }
            app.persist_remote_model_catalog_cache();
            app.remote_skills = skills;
            app.invalidate_command_candidates_cache();
            app.remote_sessions = all_sessions;
            app.remote_client_count = client_count;
            app.remote_is_canary = is_canary;
            app.remote_server_version = server_version;
            app.remote_server_short_name = server_name.clone();
            app.remote_server_icon = server_icon.clone();
            app.remote_server_has_update = server_has_update;
            let history_total_tokens = total_tokens.or_else(|| {
                token_usage_totals.map(|totals| (totals.input_tokens, totals.output_tokens))
            });
            if session_changed || history_total_tokens.is_some() {
                app.remote_total_tokens = history_total_tokens;
            }
            if session_changed || token_usage_totals.is_some() {
                app.remote_token_usage_totals = token_usage_totals;
            }
            if let Some(totals) = token_usage_totals {
                app.token_accounting.total_input_tokens = 0;
                app.token_accounting.total_output_tokens = 0;
                app.token_accounting.total_cache_reported_input_tokens = 0;
                app.token_accounting.total_cache_read_tokens = 0;
                app.token_accounting.total_cache_creation_tokens = 0;
                app.token_accounting.total_cache_optimal_input_tokens = 0;
                // Token totals are restored from history above, but the dollar
                // cost was never reconstructed, so resumed sessions showed `$0`
                // in the cost widget until a new call happened. Price the
                // restored totals once to seed the displayed cost.
                app.seed_cost_from_history_totals(&totals);
            }
            if let Some(totals) = token_usage_totals {
                crate::logging::info(&format!(
                    "Remote history token totals: session={} messages_with_usage={} input={} output={} cache_reported={} cache_read={} cache_write={}",
                    session_id,
                    totals.messages_with_token_usage,
                    totals.input_tokens,
                    totals.output_tokens,
                    totals.cache_reported_input_tokens,
                    totals.cache_read_input_tokens,
                    totals.cache_creation_input_tokens
                ));
            }
            app.workspace_client
                .sync_after_history(&session_id, &app.remote_sessions);

            if server_has_update == Some(true) && !app.pending_server_reload {
                app.pending_server_reload = true;
                app.set_status_notice("Server update available");
            }
            app.remote_server_short_name = server_name;
            if let Some(icon) = server_icon {
                app.remote_server_icon = Some(icon);
            }

            app.update_terminal_title();

            if !mcp_servers.is_empty() {
                app.mcp_server_names = mcp_servers
                    .iter()
                    .filter_map(|s| {
                        let (name, count_str) = s.split_once(':')?;
                        let count = count_str.parse::<usize>().unwrap_or(0);
                        Some((name.to_string(), count))
                    })
                    .collect();
            }

            let should_apply_history_payload = session_changed || !remote.has_loaded_history();
            if should_apply_history_payload {
                if let Some(activity) = activity.filter(|activity| activity.is_processing) {
                    let current_tool_name = activity.current_tool_name.clone();
                    app.is_processing = true;
                    if app.processing_started.is_none() {
                        app.processing_started = Some(Instant::now());
                    }
                    if app.last_stream_activity.is_none() {
                        app.last_stream_activity = Some(Instant::now());
                    }
                    app.remote_resume_activity = Some(RemoteResumeActivity {
                        session_id: session_id.clone(),
                        observed_at: Instant::now(),
                        current_tool_name: current_tool_name.clone(),
                    });
                    app.status = match current_tool_name {
                        Some(tool_name) => ProcessingStatus::RunningTool(tool_name),
                        None => ProcessingStatus::Thinking(Instant::now()),
                    };
                } else {
                    app.remote_resume_activity = None;
                }
            }
            if should_apply_history_payload {
                crate::logging::info(&format!(
                    "[TIMING] remote bootstrap: history after {}ms (session={}, resumed={}, messages={}, mcp_servers={}, model={})",
                    app.app_started.elapsed().as_millis(),
                    session_id,
                    app.resume_session_id.is_some(),
                    history_message_count,
                    history_mcp_count,
                    history_model.as_deref().unwrap_or("<none>")
                ));
                remote.mark_history_loaded();
                // History arrived: cancel the "stuck on loading session…"
                // recovery watchdog so it doesn't re-request on a later tick.
                app.clear_remote_history_wait();
                if messages.is_empty() && !session_changed && !app.display_messages().is_empty() {
                    crate::logging::info(
                        "Preserving locally restored display history for metadata-only History bootstrap",
                    );
                } else {
                    let fingerprint = history_payload_fingerprint(&messages);
                    let last_applied =
                        last_applied_history_fingerprint(&app.remote_client_instance_id);
                    if should_skip_identical_history_payload(
                        session_changed,
                        app.display_messages().is_empty(),
                        last_applied.as_ref(),
                        &session_id,
                        fingerprint,
                    ) {
                        // Watchdog re-requests and reconnect re-bootstraps can
                        // redeliver a byte-identical full payload seconds apart.
                        // Rebuilding the transcript would stack multi-megabyte
                        // transient arenas for zero visible change, so drop the
                        // payload here instead of re-applying it.
                        crate::logging::info(&format!(
                            "Skipping re-apply of identical History payload (session={}, messages={}, fingerprint={:x})",
                            session_id, history_message_count, fingerprint
                        ));
                        drop(messages);
                    } else {
                        let restored_messages = messages
                            .into_iter()
                            .map(|msg| DisplayMessage {
                                role: msg.role,
                                content: msg.content,
                                tool_calls: msg.tool_calls.unwrap_or_default(),
                                duration_secs: None,
                                title: None,
                                tool_data: msg.tool_data,
                            })
                            .collect();
                        app.replace_display_messages(restored_messages);
                        // A same-session forced re-apply (rewind / rewind-undo
                        // truncation, or a deferred bootstrap) rebuilds the
                        // transcript without running the session_changed
                        // clears above. Drop any streaming preview diagram so
                        // it cannot keep rendering a mermaid block from a
                        // message that was just truncated away. This is safe
                        // for a genuinely live stream: every streaming render
                        // frame re-registers the preview
                        // (markdown_render_full.rs set_streaming_preview_diagram).
                        if !session_changed {
                            crate::tui::mermaid::clear_streaming_preview_diagram();
                            // A rewind (or rewind-undo) re-apply can race a
                            // stale `Done` from the just-finished turn: the
                            // History payload is written directly to the
                            // socket by handle_get_history while the Done is
                            // still queued in the per-client event forwarder
                            // (server/client_lifecycle.rs), so the client can
                            // apply the truncated transcript FIRST and process
                            // the Done SECOND. The Done handler flushes
                            // stream_buffer and commits any non-empty
                            // streaming_text as an assistant message plus a
                            // turn footer, resurrecting content that was just
                            // rewound away. Drop all stale streaming state
                            // here so the late Done settles the turn without
                            // appending anything. This is gated on the pending
                            // rewind notice (armed by the client /rewind path
                            // before the redelivery) because other same-session
                            // re-applies, like a reconnect bootstrap during a
                            // live turn, may hold legitimately buffered stream
                            // chunks. The server rejects rewinds while a turn
                            // is processing, so streaming state present at this
                            // point is stale by construction.
                            if app.pending_remote_rewind_notice.is_some() {
                                app.stream_buffer.clear();
                                app.clear_streaming_render_state();
                                app.streaming_tool_calls.clear();
                                app.batch_progress = None;
                                app.thought_line_inserted = false;
                                app.thinking_prefix_emitted = false;
                                app.thinking_buffer.clear();
                                app.streaming.streaming_input_tokens = 0;
                                app.streaming.streaming_output_tokens = 0;
                                app.streaming.streaming_cache_read_tokens = None;
                                app.streaming.streaming_cache_creation_tokens = None;
                                app.reset_streaming_tps();
                            }
                        }
                        record_applied_history_fingerprint(
                            &app.remote_client_instance_id,
                            &session_id,
                            fingerprint,
                        );
                    }
                }

                if history_matches_pending_startup_prompt(app) {
                    crate::logging::info(
                        "Reload-restored startup prompt already present in server history; skipping client resubmit",
                    );
                    app.submit_input_on_startup = false;
                    app.input.clear();
                    app.cursor_pos = 0;
                    app.pending_images.clear();
                    app.set_status_notice("Reload complete - prompt preserved");
                }
                app.note_runtime_memory_event_force("history_loaded", "remote_history_applied");
                crate::process_memory::release_retained_heap("client_history_loaded");
                if let Some(notice) = app.pending_remote_rewind_notice.take() {
                    let content = if notice.undo {
                        "✓ Undid rewind. Restored the messages removed by the last rewind."
                            .to_string()
                    } else {
                        format!(
                            "✓ Rewound to message {}. Removed {} message{}. Undo anytime with /rewind undo.",
                            notice.message_index.unwrap_or_default(),
                            notice.changed_messages,
                            if notice.changed_messages == 1 {
                                ""
                            } else {
                                "s"
                            }
                        )
                    };
                    app.push_display_message(DisplayMessage::system(content));
                }
            } else {
                crate::logging::info(
                    "Ignoring duplicate History event for active session after local state was restored",
                );
            }

            app.maybe_show_catchup_after_history(&session_id);

            // The bootstrap above may have cleared/replaced the transcript for a
            // brand-new session, wiping the startup notice card (launch-hotkeys /
            // welcome tip). Re-apply it so it stays visible on the idle screen
            // instead of flashing for a moment and disappearing.
            app.reapply_pending_startup_notice_if_cleared();

            let should_consume_pending_reload_status = match app
                .pending_reload_reconnect_status
                .as_ref()
            {
                Some(PendingReloadReconnectStatus::AwaitingHistory {
                    session_id: Some(expected),
                }) => expected == &session_id,
                Some(PendingReloadReconnectStatus::AwaitingHistory { session_id: None }) => true,
                _ => false,
            };
            let pending_reload_reconnect_status = if should_consume_pending_reload_status {
                app.pending_reload_reconnect_status.take()
            } else {
                None
            };

            let reload_recovery = reload_recovery.or_else(|| {
                ReloadContext::recovery_directive(None, was_interrupted == Some(true), "", None)
            });
            if let Some(reload_recovery) = reload_recovery
                && !app.display_messages.is_empty()
            {
                let continuation_message = reload_recovery.continuation_message;
                crate::logging::info(&format!(
                    "History payload requested reload recovery continuation: session={} was_interrupted={:?}",
                    session_id, was_interrupted
                ));
                if let Some(notice) = reload_recovery.reconnect_notice
                    && !app.reload_info.iter().any(|existing| existing == &notice)
                {
                    app.reload_info.push(notice);
                }
                let already_queued = app
                    .hidden_queued_system_messages
                    .iter()
                    .any(|queued| queued == &continuation_message)
                    || app
                        .rate_limit_pending_message
                        .as_ref()
                        .and_then(|pending| pending.system_reminder.as_ref())
                        .is_some_and(|queued| queued == &continuation_message);
                if already_queued {
                    crate::logging::info(&format!(
                        "History payload reload recovery continuation already queued/in-flight: session={}",
                        session_id
                    ));
                } else {
                    app.push_display_message(DisplayMessage::system(
                        "Reload complete - continuing because a recovery directive was pending."
                            .to_string(),
                    ));
                    app.hidden_queued_system_messages.push(continuation_message);
                }
            } else if pending_reload_reconnect_status.is_some() {
                let message = match was_interrupted {
                    Some(false) => {
                        "Reload complete - no continuation needed because the previous response had already finished."
                    }
                    Some(true) => {
                        "Reload complete - no continuation queued because no recovery directive was available for the interrupted turn."
                    }
                    None => {
                        "Reload complete - no continuation needed because the server did not report an interrupted turn."
                    }
                };
                crate::logging::info(&format!(
                    "History payload completed reload reconnect without continuation: session={} was_interrupted={:?}",
                    session_id, was_interrupted
                ));
                app.push_display_message(DisplayMessage::system(message.to_string()));
            }

            // History is the completion signal for a session attach/resume and
            // can replace the entire visible transcript. Request a frame now so
            // the new session does not appear stuck until another event arrives.
            true
        }
        ServerEvent::CompactedHistory {
            session_id,
            messages,
            images,
            compacted_total,
            compacted_visible,
            compacted_remaining,
            compacted_hidden_prompts,
            ..
        } => {
            if app.remote_session_id.as_deref() != Some(session_id.as_str()) {
                crate::logging::info(&format!(
                    "Ignoring compacted history for inactive session {}",
                    session_id
                ));
                return false;
            }
            let restored_messages = messages
                .into_iter()
                .map(|msg| DisplayMessage {
                    role: msg.role,
                    content: msg.content,
                    tool_calls: msg.tool_calls.unwrap_or_default(),
                    duration_secs: None,
                    title: None,
                    tool_data: msg.tool_data,
                })
                .collect();
            app.apply_compacted_history_window(
                restored_messages,
                images,
                compacted_total,
                compacted_visible,
                compacted_remaining,
                compacted_hidden_prompts,
            );
            true
        }
        ServerEvent::SidePaneImages { session_id, images } => {
            if app.remote_session_id.as_deref() != Some(session_id.as_str()) {
                crate::logging::info(&format!(
                    "SidePaneImages: ignoring {} live image(s) for inactive session {}",
                    images.len(),
                    session_id
                ));
                return false;
            }
            if images.is_empty() {
                return false;
            }
            // Append the freshly-produced images so the inline transcript
            // updates immediately, without waiting for the next full History
            // reload. A later History payload replaces this list wholesale.
            let added = images.len();
            app.append_live_inline_images(images);
            crate::logging::info(&format!(
                "SidePaneImages: appended {} live inline image(s) (total={}) session={}",
                added,
                app.remote_side_pane_images.len(),
                session_id
            ));
            true
        }
        ServerEvent::SidePanelState { snapshot } => {
            app.set_side_panel_snapshot(snapshot);
            false
        }
        ServerEvent::SwarmStatus { members } => {
            if app.swarm_enabled {
                // Surface member lifecycle transitions (done/failed/blocked/
                // stopped) as a status notice, the same way plan syncs are
                // surfaced. Diff within the subtree this session manages (the
                // same scoping the inline strip uses), so agents belonging to
                // other sessions in a shared swarm stay silent.
                let self_id = if app.is_remote {
                    app.remote_session_id.clone()
                } else {
                    Some(app.session.id.clone())
                };
                if let Some(self_id) = self_id.as_deref() {
                    let prev = app_mod::tui_state::filter_inline_swarm_subtree(
                        &app.remote_swarm_members,
                        self_id,
                    );
                    let next = app_mod::tui_state::filter_inline_swarm_subtree(&members, self_id);
                    if let Some(notice) = swarm_status_transition_notice(&prev, &next) {
                        app.set_status_notice(notice);
                    }
                }
                app.remote_swarm_members = members;
                persist_swarm_status_snapshot(app);
            } else {
                app.remote_swarm_members.clear();
            }
            // The dedicated swarm page and strip render directly from this live
            // snapshot. The transcript uses its own stable summary signature, so
            // do not bump the global message version for output-tail, timer, todo,
            // or tool-progress updates.
            true
        }
        ServerEvent::SwarmPlan {
            swarm_id,
            version,
            items,
            participants,
            reason,
            summary,
            ..
        } => {
            // Drop stale out-of-order broadcasts. Server-side plan mutations
            // snapshot under the lock but send after releasing it, so two
            // racing mutations can deliver an older version after a newer
            // one; applying it would regress both the snapshot state and the
            // inline diagram. Same-swarm version regressions are ignored,
            // except near v1 (a deleted-and-recreated plan restarts its
            // version counter and must still render).
            let stale_regression = app.swarm_plan_swarm_id.as_deref() == Some(swarm_id.as_str())
                && app
                    .swarm_plan_version
                    .is_some_and(|current| version < current)
                && version > 2;
            if !stale_regression {
                let snapshot = RemoteSwarmPlanSnapshot {
                    swarm_id: swarm_id.clone(),
                    version,
                    items: items.clone(),
                    participants: participants.clone(),
                    reason: reason.clone(),
                    summary,
                };
                let notice = snapshot.status_notice();
                app.swarm_plan_swarm_id = Some(snapshot.swarm_id.clone());
                app.swarm_plan_version = Some(snapshot.version);
                app.swarm_plan_items = snapshot.items.clone();
                persist_swarm_plan_snapshot(
                    app,
                    snapshot.swarm_id,
                    snapshot.version,
                    snapshot.items,
                    snapshot.participants,
                    snapshot.reason,
                );
                app.set_status_notice(notice);
            }
            false
        }
        ServerEvent::SwarmPlanProposal {
            swarm_id,
            proposer_session,
            proposer_name,
            summary,
            ..
        } => {
            let proposer =
                proposer_name.unwrap_or_else(|| proposer_session.chars().take(8).collect());
            let message = format!(
                "Plan proposal received in swarm {}\nFrom: {}\nSummary: {}",
                swarm_id, proposer, summary
            );
            app.push_display_message(DisplayMessage::system(message.clone()));
            persist_replay_display_message(app, "system", None, &message);
            app.set_status_notice("Plan proposal received");
            false
        }
        ServerEvent::McpStatus { servers } => {
            app.mcp_server_names = servers
                .iter()
                .filter_map(|s| {
                    let (name, count_str) = s.split_once(':')?;
                    let count = count_str.parse::<usize>().unwrap_or(0);
                    Some((name.to_string(), count))
                })
                .collect();
            // Keep MCP readiness non-intrusive. The footer/tool indicator reads
            // `mcp_server_names` directly, so avoid a transient status notice here:
            // status notices render near the prompt and can cover text while the
            // user is typing during startup.
            false
        }
        ServerEvent::ModelChanged {
            model,
            provider_name,
            error,
            ..
        } => {
            app.remote_model_switch_in_flight = false;
            if let Some(err) = error {
                if let Some(prepared) = app.pending_prompt_after_model_switch.take() {
                    super::input_dispatch::restore_prepared_remote_input(app, prepared);
                }
                // A fallback-offer resend cannot go out on the failed switch;
                // drop it and put the prompt back in the input box instead.
                if let Some(payload) = app.pending_fallback_resend.take()
                    && let Some(raw_input) = payload.raw_input
                    && !raw_input.trim().is_empty()
                    && app.input.is_empty()
                {
                    app.input = raw_input;
                    app.cursor_pos = app.input.len();
                }
                app.push_display_message(DisplayMessage::error(
                    crate::tui::app::model_context::model_switch_failure_message(&err, true),
                ));
                app.set_status_notice("Model switch failed");
            } else {
                app.update_context_limit_for_model(&model);
                app.remote_provider_model = Some(model.clone());
                app.clear_remote_startup_phase();
                if let Some(ref pname) = provider_name {
                    app.remote_provider_name = Some(pname.clone());
                }
                app.invalidate_model_picker_cache();
                if !app.auth_catalog_refresh_pending {
                    app.push_display_message(DisplayMessage::system(format!(
                        "✓ Switched to model: {}",
                        model
                    )));
                }
                app.set_status_notice(format!("Model → {}", model));
            }
            false
        }
        ServerEvent::AvailableModelsUpdated {
            provider_name,
            provider_model,
            available_models,
            available_model_routes,
        } => {
            let model_catalog_snapshot = jcode_provider_core::ModelCatalogSnapshot::new(
                provider_name,
                provider_model,
                available_models,
                available_model_routes,
            );
            if let Some((before_models, before_routes)) =
                app.pending_remote_model_refresh_snapshot.take()
            {
                let summary = crate::provider::summarize_model_catalog_refresh(
                    before_models,
                    model_catalog_snapshot.available_models.clone(),
                    before_routes,
                    model_catalog_snapshot.model_routes.clone(),
                );
                app.push_display_message(DisplayMessage::system(
                    app_mod::model_context::format_model_refresh_summary(&summary),
                ));
                app.set_status_notice(format!(
                    "Model list refreshed: +{} models, +{} routes, ~{} changed",
                    summary.models_added, summary.routes_added, summary.routes_changed
                ));
            }
            let provider_meta_changed =
                app.replace_remote_model_catalog_snapshot(model_catalog_snapshot);
            app.remote_model_catalog_generation =
                app.remote_model_catalog_generation.saturating_add(1);
            app.persist_remote_model_catalog_cache();
            if provider_meta_changed {
                app.update_terminal_title();
            }
            // The catalog event can arrive while the client is otherwise idle.
            // Returning false here leaves the updated picker, refresh summary,
            // and status notice invisible until an unrelated input or periodic
            // redraw happens.
            true
        }
        ServerEvent::ReasoningEffortChanged { effort, error, .. } => {
            if let Some(err) = error {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set effort: {}",
                    err
                )));
            } else {
                app.remote_reasoning_effort = effort.clone();
                let label = effort
                    .as_deref()
                    .map(app_mod::effort_display_label)
                    .unwrap_or("default");
                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Reasoning effort → {}",
                    label
                )));
                app.set_status_notice(format!("Effort: {}", label));
            }
            false
        }
        ServerEvent::ServiceTierChanged {
            service_tier,
            error,
            ..
        } => {
            if let Some(err) = error {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set fast mode: {}",
                    err
                )));
            } else {
                app.remote_service_tier = service_tier.clone();
                let enabled = service_tier.as_deref() == Some("priority");
                let label = service_tier
                    .as_deref()
                    .map(app_mod::service_tier_display_label)
                    .unwrap_or("Standard");
                let applies_next_request = app.is_processing;
                app.push_display_message(DisplayMessage::system(
                    app_mod::fast_mode_success_message(enabled, label, applies_next_request),
                ));
                app.set_status_notice(app_mod::fast_mode_status_notice(
                    enabled,
                    applies_next_request,
                ));
            }
            false
        }
        ServerEvent::TransportChanged {
            transport, error, ..
        } => {
            if let Some(err) = error {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set transport: {}",
                    err
                )));
            } else {
                app.remote_transport = transport.clone();
                let label = transport.as_deref().unwrap_or("unknown");
                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Transport → {}",
                    label
                )));
                app.set_status_notice(format!("Transport: {}", label));
            }
            false
        }
        ServerEvent::CompactionModeChanged { mode, error, .. } => {
            if let Some(err) = error {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set compaction mode: {}",
                    err
                )));
            } else {
                let label = mode.as_str();
                app.remote_compaction_mode = Some(mode);
                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Compaction mode → {}",
                    label
                )));
                app.set_status_notice(format!("Compaction: {}", label));
            }
            false
        }
        ServerEvent::SoftInterruptInjected {
            content,
            display_role,
            point,
            tools_skipped,
        } => {
            crate::logging::info(&format!(
                "REMOTE_INTERRUPT_EVENT_RECEIVED kind=soft_interrupt_injected session={:?} point={} display_role={:?} tools_skipped={:?} content_bytes={} content_chars={} pending_soft_interrupts={}",
                app.remote_session_id,
                point,
                display_role,
                tools_skipped,
                content.len(),
                content.chars().count(),
                app.pending_soft_interrupts.len()
            ));
            let ops = app.stream_buffer.flush();
            app.apply_stream_ops(ops);
            if !app.streaming.streaming_text.is_empty() {
                let duration = app.display_turn_duration_secs();
                let flushed = app.take_streaming_text();
                let flushed = app.collapse_reasoning_for_commit(flushed);
                if !flushed.trim().is_empty() {
                    app.push_display_message(DisplayMessage {
                        role: "assistant".to_string(),
                        content: flushed,
                        tool_calls: vec![],
                        duration_secs: duration,
                        title: None,
                        tool_data: None,
                    });
                }
                app.push_turn_footer(duration);
            }
            app.mark_soft_interrupt_injected(&content);
            let role = display_role.unwrap_or_else(|| "user".to_string());
            app.push_display_message(DisplayMessage {
                role,
                content: content.clone(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            if let Some(n) = tools_skipped {
                app.set_status_notice(format!("⚡ {} tool(s) skipped", n));
            }
            false
        }
        ServerEvent::MemoryInjected {
            count,
            prompt,
            display_prompt,
            prompt_chars: _,
            computed_age_ms,
        } => {
            if app.memory_enabled {
                let plural = if count == 1 { "memory" } else { "memories" };
                let display_prompt = if let Some(display_prompt) = display_prompt {
                    display_prompt.clone()
                } else if prompt.trim().is_empty() {
                    "# Memory\n\n## Notes\n1. (content unavailable from server event)".to_string()
                } else {
                    prompt.clone()
                };
                crate::memory::record_injected_prompt(&prompt, count, computed_age_ms);
                let summary = if count == 1 {
                    "🧠 auto-recalled 1 memory".to_string()
                } else {
                    format!("🧠 auto-recalled {} memories", count)
                };
                app.push_display_message(DisplayMessage::memory(summary, display_prompt));
                app.set_status_notice(format!("🧠 {} relevant {} injected", count, plural));
            }
            false
        }
        ServerEvent::MemoryActivity { activity } => {
            if app.memory_enabled {
                crate::memory::apply_remote_activity_snapshot(&activity);
            }
            false
        }
        ServerEvent::Notification {
            from_session,
            from_name,
            notification_type,
            message,
        } => {
            let sender = from_name
                .clone()
                .or_else(|| crate::id::extract_session_name(&from_session).map(str::to_string))
                .unwrap_or_else(|| from_session[..8.min(from_session.len())].to_string());

            let background_task_scope = matches!(
                &notification_type,
                crate::protocol::NotificationType::Message {
                    scope: Some(scope),
                    ..
                } if scope == "background_task"
            );

            let runtime_activity_scope = match &notification_type {
                crate::protocol::NotificationType::Message {
                    scope: Some(scope), ..
                } if matches!(
                    scope.as_str(),
                    "auth_activity" | "catalog_activity" | "background_activity"
                ) =>
                {
                    Some(scope.as_str())
                }
                _ => None,
            };

            if background_task_scope {
                let presentation = present_swarm_notification(
                    &sender,
                    &notification_type,
                    &message,
                    crate::config::config().display.compact_notifications,
                );
                if crate::message::parse_background_task_progress_notification_markdown(&message)
                    .is_some()
                {
                    app.upsert_background_task_progress_message(message.clone());
                } else {
                    app.push_display_message(DisplayMessage::background_task(message.clone()));
                }
                persist_replay_display_message(app, "background_task", None, &message);
                app.set_status_notice(presentation.status_notice);
                return false;
            }

            if let Some(scope) = runtime_activity_scope {
                if message.trim().is_empty() {
                    app.set_status_notice(runtime_activity_status_notice(&message));
                    return false;
                }
                if scope == "catalog_activity"
                    && (message.starts_with("**Model ready:**")
                        || message.starts_with("**Model access refreshed**"))
                {
                    app.finish_auth_catalog_refresh();
                }
                if app.onboarding_flow_active()
                    && matches!(scope, "auth_activity" | "catalog_activity")
                {
                    app.set_status_notice(runtime_activity_status_notice(&message));
                    return false;
                }
                if scope == "catalog_activity"
                    && let Some(progress) =
                        crate::message::parse_background_task_progress_notification_markdown(
                            &message,
                        )
                {
                    let status_notice = progress.summary.clone();
                    app.upsert_background_task_progress_message(message.clone());
                    persist_replay_display_message(app, "background_task", None, &message);
                    app.set_status_notice(status_notice);
                    return false;
                } else if scope == "background_activity" {
                    app.push_display_message(DisplayMessage::background_task(message.clone()));
                    persist_replay_display_message(app, "background_task", None, &message);
                } else {
                    app.push_display_message(DisplayMessage::system(message.clone()));
                    persist_replay_display_message(app, "system", None, &message);
                }
                app.set_status_notice(runtime_activity_status_notice(&message));
                return false;
            }

            let presentation = present_swarm_notification(
                &sender,
                &notification_type,
                &message,
                crate::config::config().display.compact_notifications,
            );
            // Plan bookkeeping churn (assignments, version bumps, approvals)
            // arrives constantly while a plan runs. It only needs to pass by
            // on the status line; the inline plan graph message already shows
            // the resulting DAG state in the transcript.
            let plan_scope = matches!(
                &notification_type,
                crate::protocol::NotificationType::Message {
                    scope: Some(scope),
                    ..
                } if scope == "plan"
            );
            if plan_scope {
                app.set_status_notice(format!("{} · {}", presentation.title, presentation.message));
                return false;
            }
            let swarm_report_scope = matches!(
                &notification_type,
                crate::protocol::NotificationType::Message {
                    scope: Some(scope),
                    ..
                } if scope == "swarm"
            );
            if swarm_report_scope {
                // A report is the terminal snapshot of an agent, not another
                // prose message to read. Keep the live card under the spawn call
                // and insert a duplicate snapshot where the report arrived.
                if let Some(mut member) = app
                    .remote_swarm_members
                    .iter()
                    .find(|member| member.session_id == from_session)
                    .cloned()
                {
                    if matches!(member.status.as_str(), "running" | "streaming" | "thinking") {
                        member.status = "completed".to_string();
                    }
                    if let Some(snapshot) = crate::tui::ui::encode_swarm_agent_snapshot(&member) {
                        app.push_display_message(DisplayMessage::swarm(
                            crate::tui::ui::SWARM_AGENT_SNAPSHOT_TITLE,
                            snapshot.clone(),
                        ));
                        persist_replay_display_message(
                            app,
                            "swarm",
                            Some(crate::tui::ui::SWARM_AGENT_SNAPSHOT_TITLE.to_string()),
                            &snapshot,
                        );
                    }
                }
                app.set_status_notice(format!(
                    "{} {} finished",
                    crate::id::session_icon(&sender),
                    sender
                ));
                return false;
            }
            app.push_display_message(DisplayMessage::swarm(
                presentation.title.clone(),
                presentation.message.clone(),
            ));
            persist_replay_display_message(
                app,
                "swarm",
                Some(presentation.title.clone()),
                &presentation.message,
            );
            app.set_status_notice(presentation.status_notice);
            false
        }
        ServerEvent::Transcript { text, mode } => {
            apply_transcript_event(app, text, mode);
            false
        }
        ServerEvent::InputShellResult { result } => {
            app.push_display_message(DisplayMessage::system(
                crate::message::format_input_shell_result_markdown(&result),
            ));
            app.set_status_notice(crate::message::input_shell_status_notice(&result));
            false
        }
        ServerEvent::Compaction {
            trigger,
            pre_tokens,
            post_tokens,
            tokens_saved,
            duration_ms,
            messages_dropped,
            messages_compacted,
            summary_chars,
            active_messages,
        } => {
            app.handle_compaction_event(crate::compaction::CompactionEvent {
                trigger,
                pre_tokens,
                post_tokens,
                tokens_saved,
                duration_ms,
                messages_dropped,
                messages_compacted,
                summary_chars,
                active_messages,
            });
            false
        }
        ServerEvent::SplitResponse {
            new_session_id,
            new_session_name,
            ..
        } => {
            if app.workspace_client.handle_split_response(&new_session_id) {
                finish_remote_split_launch(app);
                app.pending_split_request = false;
                app.pending_split_startup_message = None;
                app.pending_split_parent_session_id = None;
                app.pending_split_prompt = None;
                app.pending_split_model_override = None;
                app.pending_split_provider_key_override = None;
                app.pending_split_label = None;
                app.push_display_message(DisplayMessage::system(format!(
                    "Added {} to workspace.",
                    new_session_name,
                )));
                app.set_status_notice(format!("Workspace + {}", new_session_name));
                return false;
            }
            finish_remote_split_launch(app);
            app.pending_split_request = false;
            let startup_message = app.pending_split_startup_message.take();
            let parent_session_id_override = app.pending_split_parent_session_id.take();
            let startup_prompt = app.pending_split_prompt.take();
            let model_override = app.pending_split_model_override.take();
            let provider_key_override = app.pending_split_provider_key_override.take();
            let split_label = app.pending_split_label.take();
            if let Some(startup_message) = startup_message {
                app_mod::commands::prepare_review_spawned_session(
                    &new_session_id,
                    startup_message,
                    model_override,
                    provider_key_override,
                    split_label.clone().map(|label| label.to_ascii_lowercase()),
                    parent_session_id_override,
                );
            } else if let Some(startup_prompt) = startup_prompt {
                App::save_startup_submission_for_session(
                    &new_session_id,
                    startup_prompt.content,
                    startup_prompt.images,
                );
            }
            let exe = app_mod::launch_client_executable();
            let cwd = crate::session::Session::load(&new_session_id)
                .ok()
                .and_then(|session| session.working_dir)
                .map(std::path::PathBuf::from)
                .filter(|path| path.is_dir())
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let socket = std::env::var("JCODE_SOCKET").ok();
            match spawn_in_new_terminal(&exe, &new_session_id, &cwd, socket.as_deref()) {
                Ok(true) => {
                    if let Some(label) = split_label.as_deref() {
                        app.push_display_message(DisplayMessage::system(format!(
                            "🔍 {} launched in {}.",
                            label, new_session_name,
                        )));
                        app.set_status_notice(format!("{} launched", label));
                    } else {
                        app.push_display_message(DisplayMessage::system(format!(
                            "✂ Split → {} (opened in new window)",
                            new_session_name,
                        )));
                        app.set_status_notice(format!("Split → {}", new_session_name));
                    }
                }
                Ok(false) => {
                    if let Some(label) = split_label.as_deref() {
                        app.push_display_message(DisplayMessage::system(format!(
                            "🔍 {} session {} created.\n\nNo terminal found. Resume manually:\n  jcode --resume {}",
                            label, new_session_name, new_session_id,
                        )));
                        app.set_status_notice(format!("{} session created", label));
                    } else {
                        app.push_display_message(DisplayMessage::system(format!(
                            "✂ Split → {}\n\nNo terminal found. Resume manually:\n  jcode --resume {}",
                            new_session_name, new_session_id,
                        )));
                    }
                }
                Err(e) => {
                    if let Some(label) = split_label.as_deref() {
                        app.push_display_message(DisplayMessage::error(format!(
                            "{} session {} was created but failed to open a window: {}\n\nResume manually: jcode --resume {}",
                            label, new_session_name, e, new_session_id,
                        )));
                        app.set_status_notice(format!("{} open failed", label));
                    } else {
                        app.push_display_message(DisplayMessage::error(format!(
                            "Split created {} but failed to open window: {}\n\nResume manually: jcode --resume {}",
                            new_session_name, e, new_session_id,
                        )));
                    }
                }
            }
            false
        }
        ServerEvent::CompactResult {
            message, success, ..
        } => {
            if success {
                app.push_display_message(DisplayMessage::system(message));
                app.set_status_notice("Compacting context");
            } else {
                app.push_display_message(DisplayMessage::system(message));
                app.set_status_notice("Compaction failed");
            }
            false
        }
        ServerEvent::ResumeAllResult {
            resumed, message, ..
        } => {
            app.push_display_message(DisplayMessage::system(message));
            if resumed == 0 {
                app.set_status_notice("No sessions to resume");
            } else if resumed == 1 {
                app.set_status_notice("Resuming 1 session");
            } else {
                app.set_status_notice(format!("Resuming {} sessions", resumed));
            }
            false
        }
        ServerEvent::StdinRequest { .. } => {
            app.set_status_notice("⌨ Interactive terminal detected (command will timeout)");
            false
        }
        _ => false,
    }
}

fn runtime_activity_status_notice(message: &str) -> String {
    message
        .lines()
        .find_map(|line| {
            let line = line.trim();
            (!line.is_empty()).then_some(line)
        })
        .unwrap_or("Jcode activity")
        .trim_matches('*')
        .trim()
        .trim_end_matches('.')
        .to_string()
}
