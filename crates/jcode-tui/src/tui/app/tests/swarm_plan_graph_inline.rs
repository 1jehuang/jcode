// Tests for the SwarmPlan -> inline chat plan-graph pipeline and the
// plan-scope notification quieting (status line only, no chat card).
//
// Mermaid enablement is consulted through
// `crate::tui::markdown::mermaid_rendering_enabled()`, which supports a
// scoped thread-local test override
// (`with_mermaid_rendering_override`). Tests here must NOT mutate the
// process-global JCODE_ENABLE_MERMAID env var: doing so races every other
// test thread that consults the same gate (e.g. the side-panel
// placeholder-mode tests in ui_pinned_tests.rs).

fn swarm_plan_graph_item(id: &str, content: &str) -> crate::plan::PlanItem {
    crate::plan::PlanItem {
        content: content.to_string(),
        status: "running".to_string(),
        priority: "high".to_string(),
        id: id.to_string(),
        subsystem: None,
        file_scope: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: Some("worker-fox".to_string()),
    }
}

fn swarm_plan_event(
    version: u64,
    items: Vec<crate::plan::PlanItem>,
) -> crate::protocol::ServerEvent {
    crate::protocol::ServerEvent::SwarmPlan {
        swarm_id: "test-swarm".to_string(),
        version,
        items,
        participants: vec!["session_a".to_string()],
        reason: None,
        summary: None,
    }
}

fn plan_graph_titles(app: &App) -> Vec<String> {
    app.display_messages()
        .iter()
        .filter(|m| {
            m.role == "swarm"
                && m.title
                    .as_deref()
                    .is_some_and(|t| t.starts_with("Plan graph · "))
        })
        .filter_map(|m| m.title.clone())
        .collect()
}

fn rendered_lines_to_text(lines: &[ratatui::text::Line<'static>]) -> String {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Restores the process-global markdown diagram-mode override on drop.
struct DiagramModeOverrideGuard {
    prev: Option<crate::config::DiagramDisplayMode>,
}

impl DiagramModeOverrideGuard {
    fn pinned() -> Self {
        let prev = crate::tui::markdown::get_diagram_mode_override();
        crate::tui::markdown::set_diagram_mode_override(Some(
            crate::config::DiagramDisplayMode::Pinned,
        ));
        Self { prev }
    }

    fn margin() -> Self {
        let prev = crate::tui::markdown::get_diagram_mode_override();
        crate::tui::markdown::set_diagram_mode_override(Some(
            crate::config::DiagramDisplayMode::Margin,
        ));
        Self { prev }
    }
}

impl Drop for DiagramModeOverrideGuard {
    fn drop(&mut self) {
        crate::tui::markdown::set_diagram_mode_override(self.prev);
    }
}

// ---------------------------------------------------------------------------
// wiring-audit.pinned-pane-verify: behavioral checks for the pinned diagram
// pane vs. the upsert-in-place plan-graph message. ACTIVE_DIAGRAMS is a
// process-global registry (mermaid_active.rs), so these tests serialize on
// the same lock the other diagram-mutating tests use
// (`scroll_render_test_lock`) plus this file's mermaid env lock.
// ---------------------------------------------------------------------------

/// Claims 1 + 2: replacing the trailing plan-graph message in place leaves
/// the previously rendered diagram registered in ACTIVE_DIAGRAMS (no
/// unregistration path), so the pinned pane count inflates and Ctrl+arrow
/// cycling walks stale plan versions. Refinement of claim 1: accumulation is
/// per distinct *mermaid content* hash, not per plan version number. A
/// version bump whose items (and therefore graph source) are unchanged does
/// NOT add an entry.
#[test]
fn test_upsert_in_place_plan_bump_accumulates_stale_active_diagrams() {
    let _render_lock = scroll_render_test_lock();
    let _mode_guard = DiagramModeOverrideGuard::pinned();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    crate::tui::mermaid::clear_active_diagrams();

    // v1: task running.
    app.handle_server_event(
        swarm_plan_event(1, vec![swarm_plan_graph_item("haiku-1", "write a haiku")]),
        &mut remote,
    );
    let v1_msg = app
        .display_messages()
        .iter()
        .rev()
        .find(|m| m.role == "swarm")
        .expect("plan graph message")
        .clone();
    // Render through the real swarm-message markdown path (synchronous
    // mermaid render outside the deferred draw context) so the diagram
    // registers exactly like a transcript render would.
    let lines =
        crate::tui::ui::render_swarm_message(&v1_msg, 80, crate::config::DiffDisplayMode::Inline);
    assert!(
        !rendered_lines_to_text(&lines).is_empty(),
        "swarm plan message should render"
    );
    assert_eq!(
        crate::tui::mermaid::active_diagram_count(),
        1,
        "first plan render registers one active diagram"
    );
    let v1_hash = crate::tui::mermaid::get_active_diagrams()[0].hash;

    // v2: same task flips to completed -> graph content changes -> the
    // trailing message is replaced IN PLACE (one transcript message)...
    let mut done = swarm_plan_graph_item("haiku-1", "write a haiku");
    done.status = "completed".to_string();
    app.handle_server_event(swarm_plan_event(2, vec![done.clone()]), &mut remote);
    assert_eq!(
        plan_graph_titles(&app),
        vec!["Plan graph · v2".to_string()],
        "upsert must keep a single transcript plan-graph message"
    );
    let v2_msg = app
        .display_messages()
        .iter()
        .rev()
        .find(|m| m.role == "swarm")
        .expect("plan graph message")
        .clone();
    assert_ne!(
        v1_msg.content, v2_msg.content,
        "status flip changes graph source"
    );
    let _ =
        crate::tui::ui::render_swarm_message(&v2_msg, 80, crate::config::DiffDisplayMode::Inline);

    // ...but ACTIVE_DIAGRAMS now holds BOTH versions: nothing unregisters
    // the stale v1 diagram when its transcript message was overwritten.
    let diagrams = crate::tui::mermaid::get_active_diagrams();
    assert_eq!(
        diagrams.len(),
        2,
        "claim 1 CONFIRMED: in-place plan bump leaks a stale ACTIVE_DIAGRAMS entry"
    );
    assert_ne!(
        diagrams[0].hash, v1_hash,
        "newest-first: index 0 is the v2 diagram"
    );
    assert_eq!(
        diagrams[1].hash, v1_hash,
        "the replaced v1 diagram is still registered (stale)"
    );

    // Ctrl+arrow cycling reaches the stale version and the counter reads 2.
    app.diagram_index = 0;
    app.cycle_diagram(1);
    assert_eq!(
        app.diagram_index, 1,
        "cycling lands on the stale v1 diagram"
    );
    assert_eq!(
        app.last_visible_diagram_hash,
        Some(v1_hash),
        "claim 2 CONFIRMED: the pane can show the outdated plan version"
    );
    let notice = crate::tui::TuiState::status_notice(&app);
    assert_eq!(
        notice.as_deref(),
        Some("Diagram 2/2"),
        "counter inflates to include the stale version"
    );

    // Refinement: a version bump with UNCHANGED items produces identical
    // mermaid source, so it does NOT add a third entry (dedup is by content
    // hash, `register_active_diagram` moves the entry to the fresh end).
    app.handle_server_event(swarm_plan_event(3, vec![done]), &mut remote);
    let v3_msg = app
        .display_messages()
        .iter()
        .rev()
        .find(|m| m.role == "swarm")
        .expect("plan graph message")
        .clone();
    assert_eq!(
        v2_msg.content, v3_msg.content,
        "version-only bump keeps identical graph content"
    );
    let _ =
        crate::tui::ui::render_swarm_message(&v3_msg, 80, crate::config::DiffDisplayMode::Inline);
    assert_eq!(
        crate::tui::mermaid::active_diagram_count(),
        2,
        "claim 1 REFINED: accumulation is per distinct graph content, not per version"
    );

    crate::tui::mermaid::clear_active_diagrams();
}

/// Claim 3: `get_active_diagrams` returns newest-first (insertion order
/// reversed), and `diagram_index` is positional, so a user parked at index
/// k > 0 is silently shifted to a different diagram whenever a new diagram
/// registers. Nothing re-anchors the selection by hash.
#[test]
fn test_new_registration_silently_shifts_parked_diagram_selection() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0xA, 100, 80, None);
    crate::tui::mermaid::register_active_diagram(0xB, 100, 80, None);
    crate::tui::mermaid::register_active_diagram(0xC, 100, 80, None);

    // Newest-first: [C, B, A]. Park the user on B (index 1).
    let before = crate::tui::mermaid::get_active_diagrams();
    assert_eq!(
        before.iter().map(|d| d.hash).collect::<Vec<_>>(),
        vec![0xC, 0xB, 0xA]
    );
    app.diagram_index = 1;
    app.sync_diagram_fit_context();
    assert_eq!(app.last_visible_diagram_hash, Some(0xB));

    // A new diagram registers (e.g. a plan bump): everything shifts by one.
    crate::tui::mermaid::register_active_diagram(0xD, 100, 80, None);
    let after = crate::tui::mermaid::get_active_diagrams();
    assert_eq!(
        after.iter().map(|d| d.hash).collect::<Vec<_>>(),
        vec![0xD, 0xC, 0xB, 0xA]
    );
    assert_eq!(
        after[app.diagram_index].hash, 0xC,
        "claim 3 CONFIRMED: index 1 now points at C, not the parked B"
    );
    // normalize_diagram_state does not re-anchor by hash; it only clamps the
    // index, so the silent shift persists (the fit-context sync then resets
    // the viewport because the hash under the index changed).
    app.normalize_diagram_state();
    assert_eq!(
        app.diagram_index, 1,
        "index is kept, content under it changed"
    );
    assert_eq!(
        app.last_visible_diagram_hash,
        Some(0xC),
        "selection silently moved from B to C"
    );

    // Re-registering an EXISTING hash also reorders (moves it to front),
    // which shifts a parked selection the same way.
    app.diagram_index = 2; // parked on B in [D, C, B, A]
    app.sync_diagram_fit_context();
    assert_eq!(app.last_visible_diagram_hash, Some(0xB));
    crate::tui::mermaid::register_active_diagram(0xA, 100, 80, None);
    let reordered = crate::tui::mermaid::get_active_diagrams();
    assert_eq!(
        reordered.iter().map(|d| d.hash).collect::<Vec<_>>(),
        vec![0xA, 0xD, 0xC, 0xB],
        "re-registration moves an existing hash to the fresh end"
    );
    assert_eq!(
        reordered[app.diagram_index].hash, 0xC,
        "parked index 2 shifted from B to C without any user action"
    );

    crate::tui::mermaid::clear_active_diagrams();
}

/// Claim 5: when the 129th distinct diagram registers, ACTIVE_DIAGRAMS_MAX
/// eviction drops the oldest entry. If the pane was showing that entry, the
/// index silently lands on a different diagram (no crash, no reset: the
/// count stays at the cap so `normalize_diagram_state` never clamps).
#[test]
fn test_active_diagrams_cap_eviction_swaps_currently_shown_diagram() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;

    crate::tui::mermaid::clear_active_diagrams();
    for i in 1..=128u64 {
        crate::tui::mermaid::register_active_diagram(i, 100, 80, None);
    }
    assert_eq!(crate::tui::mermaid::active_diagram_count(), 128);

    // Park on the OLDEST diagram (hash 1, last position in newest-first
    // order).
    app.diagram_index = 127;
    app.sync_diagram_fit_context();
    assert_eq!(app.last_visible_diagram_hash, Some(1));

    // The 129th diagram evicts hash 1 (the one being shown).
    crate::tui::mermaid::register_active_diagram(129, 100, 80, None);
    let diagrams = crate::tui::mermaid::get_active_diagrams();
    assert_eq!(diagrams.len(), 128, "cap holds at ACTIVE_DIAGRAMS_MAX");
    assert!(
        !diagrams.iter().any(|d| d.hash == 1),
        "the shown diagram was evicted from the registry"
    );

    // Count stayed at the cap, so index 127 is still in range: no clamp, no
    // reset, the pane just shows a different diagram.
    app.normalize_diagram_state();
    assert_eq!(app.diagram_index, 127);
    assert_eq!(
        app.last_visible_diagram_hash,
        Some(2),
        "claim 5 CONFIRMED: eviction silently swaps the shown diagram (1 -> 2)"
    );

    crate::tui::mermaid::clear_active_diagrams();
}

/// Claim 4: the chat body prepare path renders EVERY display message at full
/// fidelity regardless of scroll position (`prepare_body` does not take the
/// scroll offset; viewport windowing only slices already-prepared lines in
/// `draw_messages`). So a plan-graph message scrolled far off-screen still
/// goes through the mermaid pipeline and registers in ACTIVE_DIAGRAMS.
/// (`render_markdown_lazy`'s visible-range skipping is not used by the chat
/// body path, and even that function renders mermaid blocks unconditionally.)
#[test]
fn test_offscreen_plan_graph_message_still_registers_active_diagram() {
    let _render_lock = scroll_render_test_lock();
    let _mode_guard = DiagramModeOverrideGuard::pinned();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    crate::tui::mermaid::clear_active_diagrams();

    // Plan graph message lands first...
    app.handle_server_event(
        swarm_plan_event(7, vec![swarm_plan_graph_item("haiku-1", "write a haiku")]),
        &mut remote,
    );
    // ...then enough transcript follows to push it far above a 24-row
    // viewport (tail-follow keeps the view at the bottom).
    for i in 0..80 {
        app.push_display_message(DisplayMessage::system(format!("filler line {i}")));
    }

    // Full-frame draw through the real UI entry point (TestBackend). The
    // draw wraps rendering in the deferred mermaid context; an uncached
    // diagram is queued to the background worker, so poll for registration.
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("draw failed");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut registered = crate::tui::mermaid::active_diagram_count();
    while registered == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(25));
        // Redraw so a completed deferred render (epoch bump) re-runs the
        // message render and registers via the now-warm cache.
        terminal
            .draw(|f| crate::tui::ui::draw(f, &app))
            .expect("draw failed");
        registered = crate::tui::mermaid::active_diagram_count();
    }
    assert!(
        registered >= 1,
        "claim 4 RESOLVED: off-screen plan-graph messages DO register \
         (body prepare renders all messages; windowing only slices lines)"
    );

    crate::tui::mermaid::clear_active_diagrams();
}

/// wiring-audit.session-switch-diagram-leak: a session-changing History event
/// clears display messages and the swarm plan snapshot (server_events.rs
/// ~1592, ~1636-1639) but never touches the process-global ACTIVE_DIAGRAMS
/// registry (mermaid_active.rs: `clear_active_diagrams` is only called from
/// debug/bench/test paths). A plan-graph diagram registered in the PREVIOUS
/// session therefore survives the switch: it still counts in the pinned pane
/// counter, is reachable via Ctrl+arrow cycling, and is listed by
/// `get_active_diagrams` (the Margin info widget source), even though its
/// transcript message is gone.
#[test]
fn test_session_change_history_leaks_previous_session_active_diagram() {
    let _render_lock = scroll_render_test_lock();
    let _mode_guard = DiagramModeOverrideGuard::pinned();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    crate::tui::mermaid::clear_active_diagrams();

    // Session A: a plan-graph message lands and renders, registering its
    // diagram in the global registry (same path as the transcript render).
    app.remote_session_id = Some("session_old".to_string());
    app.handle_server_event(
        swarm_plan_event(1, vec![swarm_plan_graph_item("haiku-1", "write a haiku")]),
        &mut remote,
    );
    let plan_msg = app
        .display_messages()
        .iter()
        .rev()
        .find(|m| m.role == "swarm")
        .expect("plan graph message")
        .clone();
    let _ =
        crate::tui::ui::render_swarm_message(&plan_msg, 80, crate::config::DiffDisplayMode::Inline);
    assert_eq!(
        crate::tui::mermaid::active_diagram_count(),
        1,
        "session A plan render registers one active diagram"
    );
    let stale_hash = crate::tui::mermaid::get_active_diagrams()[0].hash;

    // Switch to session B via a session-changing History event. The handler
    // clears the transcript and the swarm plan snapshot...
    app.handle_server_event(history_event_for_session("session_new"), &mut remote);
    assert!(
        plan_graph_titles(&app).is_empty(),
        "session switch removes the plan-graph transcript message"
    );
    assert!(
        app.swarm_plan_items.is_empty(),
        "session switch clears the swarm plan snapshot"
    );
    assert_eq!(app.swarm_plan_version, None);

    // ...but the diagram registry is NOT re-scoped: the previous session's
    // plan graph is still registered and returned to the info widget.
    let diagrams = crate::tui::mermaid::get_active_diagrams();
    assert_eq!(
        diagrams.len(),
        1,
        "LEAK CONFIRMED: session-changing History leaves the previous \
         session's diagram in ACTIVE_DIAGRAMS"
    );
    assert_eq!(
        diagrams[0].hash, stale_hash,
        "the surviving entry is exactly the stale session-A plan graph"
    );

    // The pinned pane still targets it: the fit-context sync anchors on the
    // stale hash and cycling reports it in the counter, with no transcript
    // message backing it anymore.
    app.diagram_index = 0;
    app.sync_diagram_fit_context();
    assert_eq!(
        app.last_visible_diagram_hash,
        Some(stale_hash),
        "pinned pane shows the previous session's diagram after the switch"
    );
    app.cycle_diagram(1);
    let notice = crate::tui::TuiState::status_notice(&app);
    assert_eq!(
        notice.as_deref(),
        Some("Diagram 1/1"),
        "Ctrl+arrow cycling counts the stale cross-session diagram"
    );

    crate::tui::mermaid::clear_active_diagrams();
}

fn history_event_for_session_with_messages(
    session_id: &str,
    messages: Vec<crate::protocol::HistoryMessage>,
) -> crate::protocol::ServerEvent {
    let mut event = history_event_for_session(session_id);
    if let crate::protocol::ServerEvent::History {
        messages: event_messages,
        ..
    } = &mut event
    {
        *event_messages = messages;
    }
    event
}

fn user_history_message(content: &str) -> crate::protocol::HistoryMessage {
    crate::protocol::HistoryMessage {
        role: "user".to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_data: None,
    }
}

fn history_event_for_session(session_id: &str) -> crate::protocol::ServerEvent {
    crate::protocol::ServerEvent::History {
        id: 1,
        session_id: session_id.to_string(),
        messages: vec![],
        images: vec![],
        provider_name: Some("claude".to_string()),
        provider_model: Some("claude-sonnet-4-20250514".to_string()),
        subagent_model: None,
        autoreview_enabled: None,
        autojudge_enabled: None,
        available_models: vec![],
        available_model_routes: vec![],
        mcp_servers: vec![],
        skills: vec![],
        total_tokens: None,
        token_usage_totals: None,
        all_sessions: vec![],
        client_count: None,
        is_canary: None,
        reload_recovery: None,
        server_version: None,
        server_name: None,
        server_icon: None,
        server_has_update: None,
        was_interrupted: None,
        connection_type: None,
        status_detail: None,
        upstream_provider: None,
        resolved_credential: None,
        reasoning_effort: None,
        service_tier: None,
        compaction_mode: crate::config::CompactionMode::Reactive,
        activity: None,
        side_panel: crate::side_panel::SidePanelSnapshot::default(),
    }
}

#[test]
fn test_swarm_plan_event_pushes_inline_plan_graph_message() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let item = crate::plan::PlanItem {
        content: "write a haiku".to_string(),
        status: "running".to_string(),
        priority: "high".to_string(),
        id: "haiku-1".to_string(),
        subsystem: None,
        file_scope: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: Some("worker-fox".to_string()),
    };

    app.handle_server_event(
        crate::protocol::ServerEvent::SwarmPlan {
            swarm_id: "test-swarm".to_string(),
            version: 3,
            items: vec![item.clone()],
            participants: vec!["session_a".to_string()],
            reason: None,
            summary: None,
        },
        &mut remote,
    );

    let graph_msg = app
        .display_messages()
        .iter()
        .find(|m| m.role == "swarm" && m.title.as_deref() == Some("Plan graph · v3"))
        .expect("SwarmPlan event should push an inline plan graph chat message");
    assert!(
        graph_msg.content.starts_with("```mermaid\nflowchart TD"),
        "plan graph message should carry a mermaid fence: {}",
        &graph_msg.content[..graph_msg.content.len().min(80)]
    );
    assert!(
        graph_msg.content.contains("t_haiku_1") && graph_msg.content.contains("write a haiku"),
        "graph should include the task node: {}",
        graph_msg.content
    );

    // A follow-up plan version updates the trailing graph message in place
    // instead of stacking a second diagram.
    let count_before = app.display_messages().len();
    let mut updated = item;
    updated.status = "completed".to_string();
    app.handle_server_event(
        crate::protocol::ServerEvent::SwarmPlan {
            swarm_id: "test-swarm".to_string(),
            version: 4,
            items: vec![updated],
            participants: vec!["session_a".to_string()],
            reason: None,
            summary: None,
        },
        &mut remote,
    );
    assert_eq!(
        app.display_messages().len(),
        count_before,
        "rapid plan updates must coalesce into the trailing plan graph message"
    );
    let graph_count = app
        .display_messages()
        .iter()
        .filter(|m| {
            m.role == "swarm"
                && m.title
                    .as_deref()
                    .is_some_and(|t| t.starts_with("Plan graph · "))
        })
        .count();
    assert_eq!(
        graph_count, 1,
        "only one trailing plan graph message expected"
    );
    let latest = app
        .display_messages()
        .iter()
        .find(|m| m.title.as_deref() == Some("Plan graph · v4"))
        .expect("trailing graph message should carry the new version");
    assert!(
        latest.content.contains(":::done"),
        "updated status should recolor the node"
    );
}

#[test]
fn test_plan_scope_notification_stays_off_the_transcript() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let count_before = app.display_messages().len();
    app.handle_server_event(
        crate::protocol::ServerEvent::Notification {
            from_session: "session_dove_123".to_string(),
            from_name: Some("dove".to_string()),
            notification_type: crate::protocol::NotificationType::Message {
                scope: Some("plan".to_string()),
                channel: None,
                tldr: None,
            },
            message: "Plan updated: task 'fix-debug-tests' assigned to session_blowfish_9."
                .to_string(),
        },
        &mut remote,
    );

    assert_eq!(
        app.display_messages().len(),
        count_before,
        "plan-scope churn must not add chat messages"
    );

    // Non-plan swarm notifications still land in the transcript.
    app.handle_server_event(
        crate::protocol::ServerEvent::Notification {
            from_session: "session_dove_123".to_string(),
            from_name: Some("dove".to_string()),
            notification_type: crate::protocol::NotificationType::Message {
                scope: Some("dm".to_string()),
                channel: None,
                tldr: None,
            },
            message: "DM from dove: hello".to_string(),
        },
        &mut remote,
    );
    assert_eq!(
        app.display_messages().len(),
        count_before + 1,
        "dm notifications keep their chat card"
    );
}

#[test]
fn test_non_plan_swarm_message_between_plan_versions_moves_plan_graph_to_bottom() {
    // A non-plan-scope swarm chat card (e.g. a DM) landing between two
    // SwarmPlan events must NOT stack a second diagram: the single plan-graph
    // message is moved to the bottom of the transcript instead.
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.handle_server_event(
        swarm_plan_event(3, vec![swarm_plan_graph_item("haiku-1", "write a haiku")]),
        &mut remote,
    );
    assert_eq!(plan_graph_titles(&app), vec!["Plan graph · v3".to_string()]);

    // A DM notification lands as a normal swarm chat card between the two
    // plan versions.
    app.handle_server_event(
        crate::protocol::ServerEvent::Notification {
            from_session: "session_dove_123".to_string(),
            from_name: Some("dove".to_string()),
            notification_type: crate::protocol::NotificationType::Message {
                scope: Some("dm".to_string()),
                channel: None,
                tldr: None,
            },
            message: "DM from dove: hello".to_string(),
        },
        &mut remote,
    );

    let mut updated = swarm_plan_graph_item("haiku-1", "write a haiku");
    updated.status = "completed".to_string();
    app.handle_server_event(swarm_plan_event(4, vec![updated]), &mut remote);

    let titles = plan_graph_titles(&app);
    assert_eq!(
        titles,
        vec!["Plan graph · v4".to_string()],
        "a swarm DM between plan versions must not stack a second diagram: {titles:?}"
    );
    // The single diagram moved BELOW the DM card (bottom of the transcript).
    let last = app
        .display_messages()
        .last()
        .expect("transcript should not be empty");
    assert_eq!(
        last.title.as_deref(),
        Some("Plan graph · v4"),
        "the plan graph must follow the transcript bottom"
    );
}

#[test]
fn test_out_of_order_older_swarm_plan_version_is_dropped() {
    // A stale (older-version) SwarmPlan broadcast racing behind a newer one
    // must be ignored: neither the diagram nor the snapshot state regresses.
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let mut newer_item = swarm_plan_graph_item("haiku-1", "write a haiku");
    newer_item.status = "completed".to_string();
    app.handle_server_event(swarm_plan_event(5, vec![newer_item]), &mut remote);
    assert_eq!(plan_graph_titles(&app), vec!["Plan graph · v5".to_string()]);

    // A stale (older-version) broadcast arrives afterwards.
    app.handle_server_event(
        swarm_plan_event(4, vec![swarm_plan_graph_item("haiku-1", "write a haiku")]),
        &mut remote,
    );

    let titles = plan_graph_titles(&app);
    assert_eq!(
        titles,
        vec!["Plan graph · v5".to_string()],
        "an older plan version must not overwrite the newer diagram: {titles:?}"
    );
    assert_eq!(
        app.swarm_plan_version,
        Some(5),
        "snapshot state must not regress to the older version"
    );

    // A recreated plan (version counter restarted) must still apply: low
    // versions are exempt from the regression guard.
    app.handle_server_event(
        swarm_plan_event(1, vec![swarm_plan_graph_item("fresh-1", "fresh plan")]),
        &mut remote,
    );
    assert_eq!(
        app.swarm_plan_version,
        Some(1),
        "a recreated plan starting over at v1 still applies"
    );
    assert_eq!(plan_graph_titles(&app), vec!["Plan graph · v1".to_string()]);
}

#[test]
fn test_history_session_change_clears_swarm_plan_state_and_plan_graph_does_not_reappear() {
    // Wiring-audit claim 3: the History server event clears swarm_plan_items
    // (server_events.rs ~1637) on session change and the plan-graph chat
    // message does not reappear from the restored history.
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.remote_session_id = Some("session_same".to_string());
    app.handle_server_event(
        swarm_plan_event(3, vec![swarm_plan_graph_item("haiku-1", "write a haiku")]),
        &mut remote,
    );
    assert!(!app.swarm_plan_items.is_empty());
    assert_eq!(plan_graph_titles(&app).len(), 1);

    // Same-session history refresh does NOT clear the plan snapshot or the
    // inline diagram (the clearing block is scoped to session_changed).
    app.handle_server_event(history_event_for_session("session_same"), &mut remote);
    assert!(
        !app.swarm_plan_items.is_empty(),
        "same-session history refresh keeps swarm_plan_items"
    );
    assert_eq!(
        plan_graph_titles(&app).len(),
        1,
        "same-session history refresh keeps the inline plan graph message"
    );

    // Session-changing history clears the plan snapshot and the diagram does
    // not come back from the (empty) restored history.
    app.handle_server_event(history_event_for_session("session_other"), &mut remote);
    assert!(
        app.swarm_plan_items.is_empty(),
        "session-change history must clear swarm_plan_items"
    );
    assert_eq!(app.swarm_plan_version, None);
    assert_eq!(app.swarm_plan_swarm_id, None);
    assert!(
        plan_graph_titles(&app).is_empty(),
        "plan graph message must not reappear after history restore: {:?}",
        plan_graph_titles(&app)
    );
}

#[test]
fn test_swarm_plan_pushes_no_plan_graph_message_when_mermaid_disabled() {
    // Wiring-audit claim 4: with mermaid rendering disabled (opt-out, e.g.
    // JCODE_ENABLE_MERMAID=0) the SwarmPlan handler pushes no inline
    // plan-graph message (raw mermaid source would be noise), while the plan
    // snapshot state is still applied. Uses the scoped thread-local override
    // instead of mutating the process env, which would race parallel tests.
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let count_before = app.display_messages().len();
    crate::tui::markdown::with_mermaid_rendering_override(Some(false), || {
        app.handle_server_event(
            swarm_plan_event(7, vec![swarm_plan_graph_item("haiku-1", "write a haiku")]),
            &mut remote,
        );
    });

    assert_eq!(
        app.display_messages().len(),
        count_before,
        "disabled mermaid rendering must suppress the inline plan graph message"
    );
    assert!(plan_graph_titles(&app).is_empty());
    assert_eq!(
        app.swarm_plan_version,
        Some(7),
        "plan snapshot state still applies even when the diagram is suppressed"
    );
    assert!(!app.swarm_plan_items.is_empty());
}

// ---------------------------------------------------------------------------
// wiring-audit.transcript-clear-diagram-leak: the session-switch audit only
// covered the remote History `session_changed` path (server_events.rs ~1637),
// which is the ONLY transcript-clear path that resets swarm_plan_items /
// swarm_plan_version / swarm_plan_swarm_id. The other transcript-clear paths
// go through `clear_display_messages` (state_ui_messages.rs), which touches
// neither the process-global ACTIVE_DIAGRAMS registry (mermaid_active.rs)
// nor the swarm plan snapshot fields. Whether each path also clears the
// registry is a deliberate per-path decision:
//
// FULL-DISCARD paths now re-scope the registry (clear_active_diagrams),
// because the entire transcript is gone for good and nothing cached can
// re-present it, so every registered diagram is orphaned:
//   1. local `/clear` -> reset_current_session (commands.rs ->
//      commands_review.rs) - creates a brand-new empty session
//   4. remote `/clear` (remote/key_handling.rs) - server session is cleared
//
// PARTIAL-RETENTION / RESTORABLE paths deliberately KEEP the registry,
// because body-cache prefix/exact reuse (ui_prepare.rs build_body_from_base)
// skips re-rendering retained or restored messages, so their diagrams would
// never re-register if the registry were cleared here. Diagrams from removed
// messages leak until ACTIVE_DIAGRAMS_MAX eviction - a pinned tradeoff:
//   2. local `/rewind N` (commands.rs) and `/rewind undo` (commands.rs)
//   3. local session recovery (conversation_state.rs)
//   (`/cls` is the view-only clear `clear_view_keep_context`, which DOES
//   clear the registry: the user asked for an empty view, so the pinned
//   diagram pane empties with it. Ctrl+L / Cmd+L are terminal-style: they
//   append a blank spacer and touch neither the transcript nor the registry.
//   On
//   reconnect a restored transcript may reuse cached bodies without
//   re-registering its diagrams - an accepted cosmetic tradeoff; new renders
//   re-register normally.)
//   (session-changing History: see
//   test_session_change_history_leaks_previous_session_active_diagram -
//   switching BACK to the previous session reuses its cached body without
//   re-rendering, so clearing on switch would blank the pane)
// ---------------------------------------------------------------------------

/// Seeds one plan-graph message via a SwarmPlan event and renders it through
/// the real swarm-message markdown path so its diagram registers in
/// ACTIVE_DIAGRAMS exactly like a transcript render would. Returns the
/// registered diagram hash.
fn seed_rendered_plan_graph(
    app: &mut App,
    remote: &mut crate::tui::backend::RemoteConnection,
) -> u64 {
    crate::tui::mermaid::clear_active_diagrams();
    app.handle_server_event(
        swarm_plan_event(1, vec![swarm_plan_graph_item("haiku-1", "write a haiku")]),
        remote,
    );
    let plan_msg = app
        .display_messages()
        .iter()
        .rev()
        .find(|m| m.role == "swarm")
        .expect("plan graph message")
        .clone();
    let _ =
        crate::tui::ui::render_swarm_message(&plan_msg, 80, crate::config::DiffDisplayMode::Inline);
    assert_eq!(
        crate::tui::mermaid::active_diagram_count(),
        1,
        "seed: plan render registers exactly one active diagram"
    );
    assert!(
        !app.swarm_plan_items.is_empty(),
        "seed: plan snapshot applied"
    );
    crate::tui::mermaid::get_active_diagrams()[0].hash
}

/// Shared post-clear assertions: the plan-graph transcript message is gone,
/// but the ACTIVE_DIAGRAMS registry still holds the orphaned diagram (the
/// pinned pane keeps showing it) and the swarm plan snapshot fields survive
/// untouched.
fn assert_transcript_clear_leaks_diagram_and_plan_state(
    app: &mut App,
    stale_hash: u64,
    path: &str,
) {
    assert!(
        plan_graph_titles(app).is_empty(),
        "{path}: transcript wiped, no plan-graph message remains"
    );
    let diagrams = crate::tui::mermaid::get_active_diagrams();
    assert_eq!(
        diagrams.len(),
        1,
        "{path}: LEAK CONFIRMED - ACTIVE_DIAGRAMS still holds the cleared transcript's diagram"
    );
    assert_eq!(
        diagrams[0].hash, stale_hash,
        "{path}: the surviving entry is exactly the stale plan graph"
    );
    assert!(
        !app.swarm_plan_items.is_empty(),
        "{path}: STALE STATE CONFIRMED - swarm_plan_items persist after the transcript is wiped"
    );
    assert_eq!(
        app.swarm_plan_version,
        Some(1),
        "{path}: stale swarm_plan_version persists"
    );
    assert_eq!(
        app.swarm_plan_swarm_id.as_deref(),
        Some("test-swarm"),
        "{path}: stale swarm_plan_swarm_id persists"
    );
    // The pinned pane still anchors on the orphaned diagram even though no
    // transcript message backs it anymore.
    app.diagram_index = 0;
    app.sync_diagram_fit_context();
    assert_eq!(
        app.last_visible_diagram_hash,
        Some(stale_hash),
        "{path}: pinned pane still shows the orphaned plan graph"
    );
}

/// Shared post-clear assertions for the FULL-DISCARD paths: the transcript
/// and the diagram registry are both wiped (no orphaned diagram can be shown
/// by the pinned pane or the Margin info widget), while the swarm plan
/// snapshot fields are reset with the discarded session.
fn assert_full_discard_clears_diagrams_and_plan_state(app: &mut App, path: &str) {
    assert!(
        plan_graph_titles(app).is_empty(),
        "{path}: transcript wiped, no plan-graph message remains"
    );
    assert!(
        crate::tui::mermaid::get_active_diagrams().is_empty(),
        "{path}: FIX - full transcript discard re-scopes ACTIVE_DIAGRAMS \
         (no orphaned diagram survives)"
    );
    assert!(
        app.swarm_plan_items.is_empty(),
        "{path}: plan items cleared"
    );
    assert_eq!(app.swarm_plan_version, None, "{path}: plan version cleared");
    assert_eq!(app.swarm_plan_swarm_id, None, "{path}: swarm id cleared");
    // With an empty registry the pinned pane has nothing to anchor on.
    app.diagram_index = 0;
    app.sync_diagram_fit_context();
    assert_eq!(
        app.last_visible_diagram_hash, None,
        "{path}: pinned pane no longer anchors on a discarded diagram"
    );
}

/// Path 1: local `/clear` (commands.rs -> reset_current_session at
/// commands_review.rs). A full transcript discard: it now also clears the
/// process-global ACTIVE_DIAGRAMS registry, so neither the pinned pane nor
/// the Margin info widget can keep showing a diagram from the old
/// transcript, including its swarm plan snapshot.
#[test]
fn test_local_clear_command_clears_active_diagrams_and_swarm_plan_state() {
    let _render_lock = scroll_render_test_lock();
    let _mode_guard = DiagramModeOverrideGuard::pinned();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let _stale_hash = seed_rendered_plan_graph(&mut app, &mut remote);

    assert!(super::commands::handle_session_command(&mut app, "/clear"));

    assert_full_discard_clears_diagrams_and_plan_state(
        &mut app,
        "local /clear (reset_current_session)",
    );

    crate::tui::mermaid::clear_active_diagrams();
}

/// Margin-mode counterpart of path 1, the exact user-visible bug the fix
/// targets: immediately after a local `/clear`, the Margin info widget
/// (which draws `get_active_diagrams()[0]`, info_widget.rs) must no longer
/// list a diagram from the discarded transcript.
#[test]
fn test_local_clear_command_empties_margin_info_widget_diagram_list() {
    let _render_lock = scroll_render_test_lock();
    let _mode_guard = DiagramModeOverrideGuard::margin();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Margin;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let stale_hash = seed_rendered_plan_graph(&mut app, &mut remote);
    assert_eq!(
        crate::tui::TuiState::info_widget_data(&app)
            .diagrams
            .first()
            .map(|d| d.hash),
        Some(stale_hash),
        "seed: the margin widget lists the rendered plan graph"
    );

    assert!(super::commands::handle_session_command(&mut app, "/clear"));

    assert!(
        crate::tui::TuiState::info_widget_data(&app).diagrams.is_empty(),
        "FIX: after local /clear the Margin info widget lists no diagram \
         from the discarded transcript"
    );

    crate::tui::mermaid::clear_active_diagrams();
}

/// Path 2: local `/rewind N` (commands.rs ~2004) and `/rewind undo`
/// (commands.rs ~1933). Both rebuild the transcript via
/// `clear_display_messages` + re-render; neither unregisters the plan-graph
/// diagram nor resets the swarm plan snapshot. This survival is DELIBERATE
/// (see the comments at the /rewind handlers): retained/restored messages
/// are served from body-cache prefix reuse without re-rendering, so their
/// diagrams would never re-register if the registry were cleared.
#[test]
fn test_local_rewind_and_undo_leave_stale_active_diagram_and_swarm_plan_state() {
    let _render_lock = scroll_render_test_lock();
    let _mode_guard = DiagramModeOverrideGuard::pinned();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    // `/rewind N` needs rewindable stored messages.
    app.session.replace_messages(Vec::new());
    for idx in 1..=2 {
        let text = format!("msg-{idx}");
        app.add_provider_message(Message::user(&text));
        app.session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text,
                cache_control: None,
            }],
        );
    }

    let stale_hash = seed_rendered_plan_graph(&mut app, &mut remote);

    // Truncating rewind (commands.rs ~2004).
    assert!(super::commands::handle_session_command(
        &mut app,
        "/rewind 1"
    ));
    assert_transcript_clear_leaks_diagram_and_plan_state(&mut app, stale_hash, "local /rewind N");

    // Rewind undo (commands.rs ~1933) restores the transcript from the
    // snapshot; the plan-graph display message was never stored, so it does
    // not come back, but the diagram and plan state stay stale.
    assert!(super::commands::handle_session_command(
        &mut app,
        "/rewind undo"
    ));
    assert_transcript_clear_leaks_diagram_and_plan_state(
        &mut app,
        stale_hash,
        "local /rewind undo",
    );

    crate::tui::mermaid::clear_active_diagrams();
}

/// Path 3: local session recovery (`recover_session_without_tools`,
/// conversation_state.rs ~809). It rebuilds the session into a fresh one but
/// KEEPS every text block, so the registry deliberately survives (the
/// retained messages' diagrams stay backed); the plan state stays stale the
/// same way.
#[test]
fn test_recover_session_without_tools_leaves_stale_active_diagram_and_swarm_plan_state() {
    let _render_lock = scroll_render_test_lock();
    let _mode_guard = DiagramModeOverrideGuard::pinned();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let stale_hash = seed_rendered_plan_graph(&mut app, &mut remote);

    app.recover_session_without_tools();

    assert_transcript_clear_leaks_diagram_and_plan_state(
        &mut app,
        stale_hash,
        "local Ctrl+R recovery (recover_session_without_tools)",
    );

    crate::tui::mermaid::clear_active_diagrams();
}

/// Path 4: remote `/clear` (remote/key_handling.rs). A full transcript
/// discard like path 1: the server session is cleared, so the registry is
/// re-scoped too, along with the old session's swarm plan snapshot.
#[test]
fn test_remote_clear_command_clears_active_diagrams_and_swarm_plan_state() {
    let _render_lock = scroll_render_test_lock();
    let _mode_guard = DiagramModeOverrideGuard::pinned();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.is_remote = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let _stale_hash = seed_rendered_plan_graph(&mut app, &mut remote);

    app.input = "/clear".to_string();
    app.cursor_pos = app.input.len();
    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
        .expect("remote /clear should succeed");
    assert_eq!(
        crate::tui::TuiState::status_notice(&app).as_deref(),
        Some("Session cleared"),
        "remote /clear path executed"
    );

    assert_full_discard_clears_diagrams_and_plan_state(&mut app, "remote /clear");

    crate::tui::mermaid::clear_active_diagrams();
}

/// Path 5: Ctrl+L (all connection states) is a terminal-style clear: it
/// appends a blank spacer and snaps to the bottom. The transcript, queue,
/// diagram registry, and swarm plan snapshot are all untouched. `/cls` is
/// the view-only clear.
#[test]
fn test_disconnected_ctrl_l_only_adds_spacer_and_touches_nothing() {
    let _render_lock = scroll_render_test_lock();
    let _mode_guard = DiagramModeOverrideGuard::pinned();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let _stale_hash = seed_rendered_plan_graph(&mut app, &mut remote);
    app.queued_messages.push("queued".to_string());
    let messages_before = app.display_messages().len();
    app.scroll_offset = 10;
    app.auto_scroll_paused = true;

    super::remote::handle_disconnected_key(&mut app, KeyCode::Char('l'), KeyModifiers::CONTROL)
        .expect("disconnected Ctrl+L should succeed");

    assert_eq!(app.scroll_offset, 0, "Ctrl+L snaps to the bottom");
    assert!(!app.auto_scroll_paused, "Ctrl+L resumes tail-follow");
    assert!(
        app.display_messages().len() >= messages_before,
        "transcript content is untouched (a spacer may be appended)"
    );
    assert_eq!(app.queued_messages.len(), 1, "queue is untouched");
    assert!(
        !crate::tui::mermaid::get_active_diagrams().is_empty(),
        "diagram registry is untouched"
    );

    crate::tui::mermaid::clear_active_diagrams();
}

// ---------------------------------------------------------------------------
// wiring-audit.margin-streaming-preview-verify.margin-stale-entries: Margin
// mode reads the SAME process-global ACTIVE_DIAGRAMS registry as the pinned
// pane, but through a different consumer: `info_widget_data` (tui_state.rs
// ~1456) copies `get_active_diagrams()` into `InfoWidgetData.diagrams` only
// when `self.diagram_mode == DiagramDisplayMode::Margin`, and the margin
// widget renders `data.diagrams[0]` only (info_widget.rs ~1361). The tests
// below pin (a) the mode gate, (b) that plan-graph version bumps accumulate
// the same stale entries in the Margin list as in the pinned pane, and
// (c) Margin-mode selection semantics: `diagram_index` is force-reset and
// keyboard cycling is unreachable, so the widget always shows the newest
// diagram regardless of any previously parked selection.
// ---------------------------------------------------------------------------

/// Mode gate: `info_widget_data().diagrams` is populated from the global
/// registry ONLY in Margin mode (tui_state.rs:1456-1460); Pinned mode (which
/// uses the dedicated pane) gets an empty list.
#[test]
fn test_info_widget_diagram_list_populated_only_in_margin_mode() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0xA1, 100, 80, None);
    crate::tui::mermaid::register_active_diagram(0xA2, 120, 90, None);

    app.diagram_mode = crate::config::DiagramDisplayMode::Margin;
    let margin_data = crate::tui::TuiState::info_widget_data(&app);
    assert_eq!(
        margin_data
            .diagrams
            .iter()
            .map(|d| d.hash)
            .collect::<Vec<_>>(),
        vec![0xA2, 0xA1],
        "Margin mode copies the registry (newest-first) into the info widget"
    );

    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    let pinned_data = crate::tui::TuiState::info_widget_data(&app);
    assert!(
        pinned_data.diagrams.is_empty(),
        "Pinned mode must NOT feed the margin info widget (dedicated pane instead)"
    );

    crate::tui::mermaid::clear_active_diagrams();
}

/// Stale accumulation reproduces in Margin mode: an in-place plan-graph
/// version bump with changed content registers a second content hash and the
/// Margin info-widget list keeps BOTH versions (nothing unregisters the old
/// one). The margin widget itself renders `diagrams[0]`, so the panel shows
/// the fresh version, but the stale entry inflates the list exactly as in
/// the pinned pane (see test_upsert_in_place_plan_bump_accumulates_stale_active_diagrams).
#[test]
fn test_margin_mode_plan_bump_accumulates_stale_diagram_in_info_widget_list() {
    let _render_lock = scroll_render_test_lock();
    let _mode_guard = DiagramModeOverrideGuard::margin();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Margin;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    crate::tui::mermaid::clear_active_diagrams();

    // v1: task running. Render through the real swarm-message markdown path;
    // in Margin mode `mermaid_should_register_active()` is true (only None
    // opts out, jcode-tui-markdown/src/lib.rs mermaid_should_register_active),
    // so the diagram registers like a transcript render would.
    app.handle_server_event(
        swarm_plan_event(1, vec![swarm_plan_graph_item("haiku-1", "write a haiku")]),
        &mut remote,
    );
    let v1_msg = app
        .display_messages()
        .iter()
        .rev()
        .find(|m| m.role == "swarm")
        .expect("plan graph message")
        .clone();
    let _ =
        crate::tui::ui::render_swarm_message(&v1_msg, 80, crate::config::DiffDisplayMode::Inline);
    let v1_list = crate::tui::TuiState::info_widget_data(&app).diagrams;
    assert_eq!(
        v1_list.len(),
        1,
        "Margin mode: first plan render lands in the info-widget diagram list"
    );
    let v1_hash = v1_list[0].hash;

    // v2: status flip changes the graph content; the transcript message is
    // replaced in place, but the registry gains a second entry.
    let mut done = swarm_plan_graph_item("haiku-1", "write a haiku");
    done.status = "completed".to_string();
    app.handle_server_event(swarm_plan_event(2, vec![done.clone()]), &mut remote);
    assert_eq!(
        plan_graph_titles(&app),
        vec!["Plan graph · v2".to_string()],
        "upsert keeps a single transcript plan-graph message"
    );
    let v2_msg = app
        .display_messages()
        .iter()
        .rev()
        .find(|m| m.role == "swarm")
        .expect("plan graph message")
        .clone();
    assert_ne!(
        v1_msg.content, v2_msg.content,
        "status flip changes graph source"
    );
    let _ =
        crate::tui::ui::render_swarm_message(&v2_msg, 80, crate::config::DiffDisplayMode::Inline);

    let diagrams = crate::tui::TuiState::info_widget_data(&app).diagrams;
    assert_eq!(
        diagrams.len(),
        2,
        "STALE ACCUMULATION CONFIRMED in Margin mode: the info-widget list \
         holds both plan-graph versions after an in-place bump"
    );
    assert_ne!(
        diagrams[0].hash, v1_hash,
        "newest-first: index 0 is the fresh v2 diagram (the one the margin \
         widget renders, info_widget.rs render_diagrams_widget)"
    );
    assert_eq!(
        diagrams[1].hash, v1_hash,
        "the replaced v1 diagram is still listed (stale)"
    );

    // Refinement (same as pinned): a version-only bump with identical items
    // produces identical mermaid source and does NOT add a third entry.
    app.handle_server_event(swarm_plan_event(3, vec![done]), &mut remote);
    let v3_msg = app
        .display_messages()
        .iter()
        .rev()
        .find(|m| m.role == "swarm")
        .expect("plan graph message")
        .clone();
    assert_eq!(
        v2_msg.content, v3_msg.content,
        "version-only bump keeps identical graph content"
    );
    let _ =
        crate::tui::ui::render_swarm_message(&v3_msg, 80, crate::config::DiffDisplayMode::Inline);
    assert_eq!(
        crate::tui::TuiState::info_widget_data(&app).diagrams.len(),
        2,
        "accumulation is per distinct graph content, not per version number"
    );

    crate::tui::mermaid::clear_active_diagrams();
}

/// Margin-mode selection semantics: there is no per-diagram selection at all.
/// `diagram_available()` requires Pinned mode (navigation.rs:336-340), so
/// Ctrl+arrow cycling is unreachable; `normalize_diagram_state` force-resets
/// `diagram_index` to 0 in any non-Pinned mode (navigation.rs:342-349); and
/// the margin widget always renders `diagrams[0]` (info_widget.rs:1361). So
/// after the list changes, the "selection" is always the newest diagram:
/// a stale index can never be pointed at a stale entry in Margin mode.
#[test]
fn test_margin_mode_has_no_diagram_selection_and_always_shows_newest() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Margin;
    app.diagram_pane_enabled = true;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0xB1, 100, 80, None);
    crate::tui::mermaid::register_active_diagram(0xB2, 100, 80, None);
    crate::tui::mermaid::register_active_diagram(0xB3, 100, 80, None);

    // Cycling is unreachable: diagram_available() is Pinned-only, so the
    // Ctrl-key handler refuses the cycle keys even with diagrams present.
    assert!(
        !app.diagram_available(),
        "Margin mode reports no cyclable diagram pane"
    );
    app.diagram_focus = true; // even with focus somehow set
    assert!(
        !app.handle_diagram_ctrl_key(KeyCode::Left, app.diagram_available()),
        "Ctrl+Left does not cycle in Margin mode"
    );
    assert!(
        !app.handle_diagram_ctrl_key(KeyCode::Right, app.diagram_available()),
        "Ctrl+Right does not cycle in Margin mode"
    );

    // A parked/stale index from a previous Pinned session is force-reset by
    // normalize_diagram_state's non-Pinned branch, so it can never select a
    // stale entry after the list changes.
    app.diagram_index = 2;
    app.diagram_scroll_x = 5;
    app.diagram_scroll_y = 7;
    app.normalize_diagram_state();
    assert_eq!(
        app.diagram_index, 0,
        "non-Pinned normalize resets the index"
    );
    assert!(
        !app.diagram_focus,
        "non-Pinned normalize drops diagram focus"
    );
    assert_eq!(app.diagram_scroll_x, 0);
    assert_eq!(app.diagram_scroll_y, 0);
    assert_eq!(
        app.last_visible_diagram_hash, None,
        "no visible-diagram anchor is tracked in Margin mode"
    );

    // The widget input is newest-first, and the margin renderer draws only
    // element 0, so a new registration immediately becomes the shown diagram.
    let before = crate::tui::TuiState::info_widget_data(&app).diagrams;
    assert_eq!(before[0].hash, 0xB3, "newest diagram is the rendered one");
    crate::tui::mermaid::register_active_diagram(0xB4, 100, 80, None);
    let after = crate::tui::TuiState::info_widget_data(&app).diagrams;
    assert_eq!(
        after.iter().map(|d| d.hash).collect::<Vec<_>>(),
        vec![0xB4, 0xB3, 0xB2, 0xB1],
        "stale entries stay listed behind the newest one"
    );
    assert_eq!(
        after[0].hash, 0xB4,
        "the margin widget switches to the new diagram (index 0) with no \
         selection to go stale"
    );

    crate::tui::mermaid::clear_active_diagrams();
}

/// Margin-mode counterpart of the transcript-clear leak: after a session
/// switch removes the plan-graph transcript message, the Margin info widget
/// STILL lists (and therefore renders) the orphaned diagram, because nothing
/// re-scopes ACTIVE_DIAGRAMS (mermaid_active.rs) on session change.
#[test]
fn test_margin_mode_session_switch_keeps_orphaned_diagram_in_info_widget() {
    let _render_lock = scroll_render_test_lock();
    let _mode_guard = DiagramModeOverrideGuard::margin();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Margin;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.remote_session_id = Some("session_old".to_string());
    let stale_hash = seed_rendered_plan_graph(&mut app, &mut remote);

    app.handle_server_event(history_event_for_session("session_new"), &mut remote);
    assert!(
        plan_graph_titles(&app).is_empty(),
        "session switch removes the plan-graph transcript message"
    );

    let diagrams = crate::tui::TuiState::info_widget_data(&app).diagrams;
    assert_eq!(
        diagrams.len(),
        1,
        "LEAK CONFIRMED in Margin mode: the info widget still lists the \
         previous session's diagram"
    );
    assert_eq!(
        diagrams[0].hash, stale_hash,
        "the margin widget would render exactly the orphaned session-A plan graph"
    );

    crate::tui::mermaid::clear_active_diagrams();
}

// ---------------------------------------------------------------------------
// wiring-audit.compaction-rewind-clear-verify: transcript-REPLACEMENT paths
// that bypass `clear_display_messages` entirely.
//
//   A. `apply_compacted_history_window` (state_ui_messages.rs:404) assigns
//      `self.display_messages = messages` wholesale. The window is built from
//      server-side session storage, which never contains the client-only
//      "Plan graph · vN" message, so the coalesced diagram message is DROPPED
//      from the transcript while ACTIVE_DIAGRAMS and the swarm_plan_* snapshot
//      leak (the function touches neither).
//   B. Server-driven remote `/rewind N` / `/rewind undo`: `remote.rewind()`
//      (backend.rs:608) flips `has_loaded_history=false` and the server
//      responds with a fresh History payload for the SAME session id, so
//      `session_changed` (server_events.rs:1585) is FALSE and the plan-state
//      clearing block (server_events.rs:1637-1639) never runs. The transcript
//      is replaced via `replace_display_messages` (dropping the plan graph),
//      while ACTIVE_DIAGRAMS and swarm_plan_* leak.
//   C. Local (non-remote) session picker `/resume` current-terminal switch:
//      `handle_session_picker_current_terminal_selection`
//      (inline_interactive.rs:2128) only queues the target on
//      `workspace_client.queue_resume_session`; the queued resume is consumed
//      exclusively by remote::handle_tick (app/remote.rs:136). local::handle_tick
//      (app/local.rs:63-118) never takes it, so in local mode the switch is a
//      silent no-op: no transcript clear ever happens and the plan graph,
//      plan snapshot, and registered diagram all persist trivially.
// ---------------------------------------------------------------------------

/// Path A: a CompactedHistory window replaces the transcript wholesale
/// (`apply_compacted_history_window` bypasses `clear_display_messages`). The
/// server-built window cannot contain the client-only plan-graph message, so
/// the coalesced "Plan graph · vN" message is dropped, while the process-global
/// ACTIVE_DIAGRAMS entry and the swarm plan snapshot fields survive untouched.
#[test]
fn test_compacted_history_window_drops_plan_graph_but_leaks_diagram_and_plan_state() {
    let _render_lock = scroll_render_test_lock();
    let _mode_guard = DiagramModeOverrideGuard::pinned();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    // CompactedHistory is dropped for inactive sessions, so the app must be
    // attached to the same session the event names (server_events.rs ~1968).
    app.remote_session_id = Some("session_same".to_string());
    let stale_hash = seed_rendered_plan_graph(&mut app, &mut remote);

    app.handle_server_event(
        crate::protocol::ServerEvent::CompactedHistory {
            id: 42,
            session_id: "session_same".to_string(),
            messages: vec![
                user_history_message("older prompt 1"),
                user_history_message("older prompt 2"),
            ],
            images: vec![],
            compacted_total: 2,
            compacted_visible: 2,
            compacted_remaining: 0,
            compacted_hidden_prompts: 0,
        },
        &mut remote,
    );

    // The window landed: transcript is exactly the server-built message list.
    assert_eq!(
        app.display_messages().len(),
        2,
        "compacted window replaces the transcript wholesale"
    );
    assert_eq!(
        crate::tui::TuiState::status_notice(&app).as_deref(),
        Some("Loaded all 2 compacted messages"),
        "apply_compacted_history_window ran"
    );
    // The coalesced plan-graph message did NOT survive the replacement: the
    // window comes from server session storage, which never holds the
    // client-side "Plan graph · vN" display message.
    assert_transcript_clear_leaks_diagram_and_plan_state(
        &mut app,
        stale_hash,
        "CompactedHistory window (apply_compacted_history_window)",
    );

    crate::tui::mermaid::clear_active_diagrams();
}

/// Path B: server-driven remote `/rewind N` and `/rewind undo`. Both flip
/// `has_loaded_history=false` (backend.rs:608/622) and the server answers
/// with a History payload for the SAME session, so `session_changed` is false
/// and the swarm-plan clearing block (server_events.rs:1637-1639) is skipped.
/// The truncated payload replaces the transcript (dropping the plan graph),
/// while ACTIVE_DIAGRAMS and swarm_plan_* leak.
#[test]
fn test_remote_rewind_history_response_is_not_session_changed_and_leaks_plan_state() {
    let _render_lock = scroll_render_test_lock();
    let _mode_guard = DiagramModeOverrideGuard::pinned();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.is_remote = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    remote.set_session_id("session_same".to_string());
    app.remote_session_id = Some("session_same".to_string());

    // `/rewind N` requires at least one rewindable (user/assistant) display
    // message.
    app.push_display_message(DisplayMessage::user("hello"));
    let stale_hash = seed_rendered_plan_graph(&mut app, &mut remote);

    // --- /rewind 1 ---------------------------------------------------------
    app.input = "/rewind 1".to_string();
    app.cursor_pos = app.input.len();
    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
        .expect("remote /rewind should succeed");
    assert!(
        !remote.has_loaded_history(),
        "remote.rewind() must re-open the history gate so the server's \
         truncated History payload can replace the display state"
    );

    // The server responds with a History payload for the SAME session id
    // carrying the truncated message list.
    app.handle_server_event(
        history_event_for_session_with_messages(
            "session_same",
            vec![user_history_message("hello")],
        ),
        &mut remote,
    );
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.content.starts_with("✓ Rewound to message 1")),
        "rewind notice confirms the rewind History path executed"
    );
    // session_changed was false, so the plan-clearing block was skipped.
    assert_transcript_clear_leaks_diagram_and_plan_state(
        &mut app,
        stale_hash,
        "remote /rewind N (same-session History response)",
    );

    // --- /rewind undo ------------------------------------------------------
    app.input = "/rewind undo".to_string();
    app.cursor_pos = app.input.len();
    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
        .expect("remote /rewind undo should succeed");
    assert!(
        !remote.has_loaded_history(),
        "remote.rewind_undo() also re-opens the history gate"
    );
    app.handle_server_event(
        history_event_for_session_with_messages(
            "session_same",
            vec![
                user_history_message("hello"),
                user_history_message("restored"),
            ],
        ),
        &mut remote,
    );
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.content.starts_with("✓ Undid rewind")),
        "undo notice confirms the rewind-undo History path executed"
    );
    assert_transcript_clear_leaks_diagram_and_plan_state(
        &mut app,
        stale_hash,
        "remote /rewind undo (same-session History response)",
    );

    crate::tui::mermaid::clear_active_diagrams();
}

/// Path C: local (non-remote) session picker current-terminal switch. Enter on
/// a session queues the target via `workspace_client.queue_resume_session`
/// (inline_interactive.rs:2128), but only remote::handle_tick (app/remote.rs:136)
/// ever consumes that queue; local::handle_tick (app/local.rs:63-118) does not.
/// So in local mode the "switch" never happens: no transcript clear, no
/// History event, and the plan graph message, swarm plan snapshot, and
/// registered diagram all persist. The stale state here is not a clear-path
/// leak but the queued switch silently never executing.
#[test]
fn test_local_session_picker_switch_is_never_consumed_and_keeps_plan_graph_state() {
    let _render_lock = scroll_render_test_lock();
    let _mode_guard = DiagramModeOverrideGuard::pinned();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    assert!(!app.is_remote, "this pins the local (non-remote) mode path");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let stale_hash = seed_rendered_plan_graph(&mut app, &mut remote);

    // Open a picker with one target and select it in the current terminal.
    app.session_picker_mode = SessionPickerMode::Resume;
    app.session_picker_overlay = Some(RefCell::new(
        crate::tui::session_picker::SessionPicker::new(vec![
            crate::tui::session_picker::SessionInfo {
                id: "session_target_456".to_string(),
                parent_id: None,
                short_name: "target".to_string(),
                icon: "t".to_string(),
                title: "Target".to_string(),
                message_count: 1,
                user_message_count: 1,
                assistant_message_count: 0,
                created_at: chrono::Utc::now(),
                last_message_time: chrono::Utc::now(),
                last_active_at: None,
                working_dir: None,
                model: None,
                provider_key: None,
                is_canary: false,
                is_debug: false,
                saved: false,
                save_label: None,
                status: crate::session::SessionStatus::Closed,
                needs_catchup: false,
                estimated_tokens: 0,
                first_user_prompt: None,
                messages_preview: Vec::new(),
                search_index: "target".to_string(),
                server_name: None,
                server_icon: None,
                source: crate::tui::session_picker::SessionSource::Jcode,
                resume_target: crate::tui::session_picker::ResumeTarget::JcodeSession {
                    session_id: "session_target_456".to_string(),
                },
                external_path: None,
            },
        ]),
    ));
    app.handle_session_picker_key(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::empty(),
    )
    .expect("session picker enter should succeed");
    assert!(
        app.session_picker_overlay.is_none(),
        "picker closes and reports 'Switching → …'"
    );

    // The local tick loop never consumes the queued resume (only
    // remote::handle_tick at app/remote.rs:136 does), so nothing switches.
    let _ = crate::tui::app::local::handle_tick(&mut app);
    let _ = crate::tui::app::local::handle_tick(&mut app);
    assert_eq!(
        app.workspace_client
            .take_pending_resume_session()
            .as_deref(),
        Some("session_target_456"),
        "CONFIRMED: local ticks leave the queued switch unconsumed \
         (session switching is remote-only)"
    );

    // Because no switch (and therefore no transcript clear or History event)
    // ever happens locally, the plan graph message, plan snapshot, and
    // registered diagram all remain in place.
    assert_eq!(
        plan_graph_titles(&app),
        vec!["Plan graph · v1".to_string()],
        "no transcript clear happened, the plan graph message persists"
    );
    assert!(!app.swarm_plan_items.is_empty());
    assert_eq!(app.swarm_plan_version, Some(1));
    let diagrams = crate::tui::mermaid::get_active_diagrams();
    assert_eq!(diagrams.len(), 1);
    assert_eq!(diagrams[0].hash, stale_hash);

    crate::tui::mermaid::clear_active_diagrams();
}

// ---------------------------------------------------------------------------
// wiring-audit.margin-streaming-preview-verify.local-paths-preview-leak:
// unlike ACTIVE_DIAGRAMS (whose survival across transcript clears is pinned
// above as a known leak), the ephemeral STREAMING_PREVIEW_DIAGRAM slot
// (mermaid_active.rs) must NOT survive local transcript mutations. The
// typed-command paths (/clear, /rewind) are already protected in practice
// because submit_input commits pending streaming text first
// (input.rs commit_pending_streaming_assistant_message -> take_streaming_text
// clears the slot), but direct dispatch must not rely on that: Ctrl+R
// (recover_session_without_tools) is reachable mid-stream from the turn.rs
// key loops with a live preview and no commit. These tests pin that all three
// local transcript-mutation paths clear the preview slot themselves.
// ---------------------------------------------------------------------------

/// Simulates a mid-stream mermaid preview exactly like the streaming
/// markdown renderer would create it (markdown_render_full.rs
/// set_streaming_preview_diagram on a complete fenced block).
fn seed_streaming_preview(app: &mut App, hash: u64) {
    crate::tui::mermaid::clear_active_diagrams();
    app.streaming.streaming_text = "```mermaid\ngraph TD; A-->B\n```".to_string();
    app.is_processing = true;
    crate::tui::mermaid::set_streaming_preview_diagram(hash, 320, 240, Some("preview".to_string()));
    assert_eq!(
        crate::tui::mermaid::get_active_diagrams()
            .first()
            .map(|d| d.hash),
        Some(hash),
        "seed: streaming preview occupies index 0 (what Margin mode draws)"
    );
}

fn assert_streaming_preview_cleared(hash: u64, path: &str) {
    assert!(
        !crate::tui::mermaid::get_active_diagrams()
            .iter()
            .any(|d| d.hash == hash),
        "{path}: streaming preview diagram must not survive the transcript mutation"
    );
}

/// Local `/clear` -> reset_current_session (commands_review.rs) now clears
/// the streaming render state, including the preview slot.
#[test]
fn test_local_clear_command_clears_streaming_preview_diagram() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    let hash: u64 = 0x0005_17EA_11ED_0001;
    seed_streaming_preview(&mut app, hash);

    assert!(super::commands::handle_session_command(&mut app, "/clear"));

    assert_streaming_preview_cleared(hash, "local /clear");
    assert!(
        app.streaming.streaming_text.is_empty(),
        "local /clear: in-flight streaming text is dropped with the transcript"
    );
    crate::tui::mermaid::clear_active_diagrams();
}

/// Local `/rewind N` and `/rewind undo` (commands.rs) rebuild the transcript;
/// both must drop the streaming preview slot.
#[test]
fn test_local_rewind_and_undo_clear_streaming_preview_diagram() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();

    app.session.replace_messages(Vec::new());
    for idx in 1..=2 {
        let text = format!("msg-{idx}");
        app.add_provider_message(Message::user(&text));
        app.session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text,
                cache_control: None,
            }],
        );
    }

    let hash: u64 = 0x0005_17EA_11ED_0002;
    seed_streaming_preview(&mut app, hash);
    assert!(super::commands::handle_session_command(
        &mut app,
        "/rewind 1"
    ));
    assert_streaming_preview_cleared(hash, "local /rewind N");

    seed_streaming_preview(&mut app, hash);
    assert!(super::commands::handle_session_command(
        &mut app,
        "/rewind undo"
    ));
    assert_streaming_preview_cleared(hash, "local /rewind undo");
    crate::tui::mermaid::clear_active_diagrams();
}

/// Ctrl+R recovery (recover_session_without_tools, conversation_state.rs) is
/// reachable mid-stream from the turn.rs key loops with a live preview and no
/// prior commit, so it must clear the preview slot itself.
#[test]
fn test_recover_session_without_tools_clears_streaming_preview_diagram() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    let hash: u64 = 0x0005_17EA_11ED_0003;
    seed_streaming_preview(&mut app, hash);

    app.recover_session_without_tools();

    assert_streaming_preview_cleared(hash, "local Ctrl+R recovery");
    assert!(
        app.streaming.streaming_text.is_empty(),
        "recovery: in-flight streaming text is dropped with the transcript"
    );
    crate::tui::mermaid::clear_active_diagrams();
}

/// `commit_pending_streaming_assistant_message` early-returns when the live
/// buffer is empty (tool-only boundary). The buffer can become empty *after*
/// a preview was rendered only via `replace_streaming_text` (remote
/// TextReplace, server_events.rs:644, and debug snapshot restore,
/// debug.rs:539), which does not touch the preview slot. The commit boundary
/// is the mirror point: an empty buffer means any surviving preview is stale,
/// so the early return must clear the slot instead of leaking it
/// (input.rs commit_pending_streaming_assistant_message).
#[test]
fn test_commit_with_emptied_stream_buffer_clears_streaming_preview_diagram() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    let hash: u64 = 0x0005_17EA_11ED_0004;
    seed_streaming_preview(&mut app, hash);

    // Simulate a TextReplace-style rewrite that drops the fenced block the
    // preview was rendered from, leaving the buffer empty while the preview
    // slot is still occupied.
    app.replace_streaming_text(String::new());
    assert_eq!(
        crate::tui::mermaid::get_active_diagrams()
            .first()
            .map(|d| d.hash),
        Some(hash),
        "precondition: replace_streaming_text alone leaves the preview live"
    );

    let committed = app.commit_pending_streaming_assistant_message();

    assert!(!committed, "empty buffer commits nothing");
    assert_streaming_preview_cleared(hash, "commit with emptied stream buffer");
    crate::tui::mermaid::clear_active_diagrams();
}
