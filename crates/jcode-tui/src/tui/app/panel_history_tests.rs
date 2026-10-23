use super::*;

#[test]
fn panel_history_archive_reopen_restores_saved_viewport() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(value) = &self.0 {
                crate::env::set_var("JCODE_HOME", value);
            } else {
                crate::env::remove_var("JCODE_HOME");
            }
        }
    }
    let _restore = Restore(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", temp.path());
    let mut app = crate::tui::app::tests::create_test_app();
    let session = app.session.id.clone();
    let initial = crate::side_panel::write_markdown_page(
        &session,
        "plan",
        Some("Plan"),
        "# Synthetic plan",
        true,
    )
    .unwrap();
    app.set_side_panel_snapshot(initial);
    app.diff_pane_scroll = 17;
    app.diff_pane_scroll_x = 6;
    app.side_panel_image_zoom_percent = 140;
    app.diff_pane_auto_scroll = false;
    let archived = crate::side_panel::archive_page(&session, "plan").unwrap();
    app.set_side_panel_snapshot(archived);
    assert!(app.side_panel.pages.is_empty());
    assert!(handle_panels_command(&mut app, "/panels"));
    app.handle_inline_interactive_key(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    )
    .unwrap();
    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("plan"));
    assert_eq!(
        app.side_panel.focused_page().unwrap().content,
        "# Synthetic plan"
    );
    assert_eq!(app.diff_pane_scroll, 17);
    assert_eq!(app.diff_pane_scroll_x, 6);
    assert_eq!(app.side_panel_image_zoom_percent, 140);
    assert!(!app.diff_pane_auto_scroll);
    assert!(!app.side_panel_user_hidden);
}

#[test]
fn panel_history_picker_renders_state_source_and_real_times_at_narrow_widths() {
    let mut app = crate::tui::app::tests::create_test_app();
    app.is_remote = true;
    app.remote_session_id = Some("session-a".into());
    app.side_panel = snapshot();
    app.side_panel_user_hidden = true;
    assert!(handle_panels_command(&mut app, "/panels"));
    for width in [12, 24, 48, 160] {
        let backend = ratatui::backend::TestBackend::new(width, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                crate::tui::ui::draw(frame, &app);
            })
            .unwrap();
        if width == 160 {
            let buffer = terminal.backend().buffer();
            let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
            assert!(text.contains("Design"), "{text}");
            assert!(text.contains("archived"), "{text}");
            assert!(text.contains("notes/design.md"), "{text}");
            assert!(text.contains("opened 1970-01-01"), "{text}");
            assert!(text.contains("updated 1970-01-01"), "{text}");
        }
    }
}

#[test]
fn panels_command_opens_searchable_picker_and_rejects_late_selection() {
    let mut app = crate::tui::app::tests::create_test_app();
    app.is_remote = true;
    app.remote_session_id = Some("session-a".into());
    app.side_panel = snapshot();
    assert!(!handle_panels_command(&mut app, "/panels-extra"));
    assert!(handle_panels_command(&mut app, "/panels design"));
    let picker = app.inline_interactive_state.as_ref().unwrap();
    assert_eq!(picker.kind, PickerKind::Panels);
    assert_eq!(picker.filtered, vec![0]);
    assert_eq!(picker.filter, "design");
    app.reopen_panel_from_history("session-b", "closed");
    assert!(app.pending_panel_action.is_none());
    app.handle_inline_interactive_key(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    )
    .unwrap();
    assert!(app.inline_interactive_state.is_none());
    assert_eq!(
        app.pending_panel_action,
        Some(("session-a".into(), "reopen".into(), "closed".into()))
    );
}

use crate::side_panel::{SidePanelHistoryEntry, SidePanelPage, SidePanelPageFormat};

fn snapshot() -> SidePanelSnapshot {
    let status = SidePanelPageStatus {
        last_opened_at_ms: 1000,
        last_successful_update_ms: 2000,
        ..Default::default()
    };
    SidePanelSnapshot {
        session_id: Some("session-a".into()),
        focused_page_id: Some("open".into()),
        pages: vec![SidePanelPage {
            id: "open".into(),
            title: "Roadmap 界".into(),
            file_path: "notes/roadmap.md".into(),
            content: "BODY MUST NOT BE SEARCHABLE".into(),
            status: status.clone(),
            updated_at_ms: u64::MAX,
            ..Default::default()
        }],
        archived: vec![SidePanelHistoryEntry {
            id: "closed".into(),
            title: "Design".into(),
            file_path: "notes/design.md".into(),
            source: SidePanelPageSource::LinkedFile,
            format: SidePanelPageFormat::Markdown,
            updated_at_ms: u64::MAX,
            status: SidePanelPageStatus {
                last_opened_at_ms: 3000,
                stale_reason: Some("source missing".into()),
                ..status
            },
            archived_at_ms: 4000,
            cache_path: None,
        }],
        ..Default::default()
    }
}

#[test]
fn panel_history_session_scope_and_metadata_only() {
    let snapshot = snapshot();
    assert!(history_entries(&snapshot, "session-b", false).is_empty());
    let entries = history_entries(&snapshot, "session-a", false);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "Design");
    assert_eq!(entries[0].options[0].provider, "unavailable · archived");
    assert_eq!(entries[1].options[0].provider, "open");
    assert!(entries[1].is_current);
    for item in &entries {
        let text = PickerKind::Panels.filter_text(item);
        assert!(!text.contains("BODY MUST NOT BE SEARCHABLE"));
        assert!(!text.contains(&u64::MAX.to_string()));
        assert!(text.contains("1970-01-01"));
        assert!(
            matches!(&item.action, PickerAction::Panel { session_id, .. } if session_id == "session-a")
        );
    }
    assert_eq!(
        history_entries(&snapshot, "session-a", true)[1].options[0].provider,
        "hidden"
    );
}

#[test]
fn panel_history_excludes_transient_pages_and_duplicate_archives() {
    let mut snapshot = snapshot();
    snapshot.pages[0].source = SidePanelPageSource::Ephemeral;
    assert_eq!(history_entries(&snapshot, "session-a", false).len(), 1);
    snapshot.pages[0].source = SidePanelPageSource::Managed;
    snapshot.archived[0].id = "open".into();
    assert_eq!(history_entries(&snapshot, "session-a", false).len(), 1);
}

#[test]
fn panel_history_filter_matches_source_title_and_state() {
    let entries = history_entries(&snapshot(), "session-a", false);
    let mut picker = InlineInteractiveState {
        kind: PickerKind::Panels,
        filtered: vec![],
        entries,
        selected: 9,
        column: 0,
        filter: "design archived".into(),
        preview: false,
    };
    App::apply_inline_interactive_filter(&mut picker);
    assert_eq!(picker.filtered, vec![0]);
    assert_eq!(picker.selected, 0);
    picker.filter = "roadmap.md".into();
    App::apply_inline_interactive_filter(&mut picker);
    assert_eq!(picker.filtered, vec![1]);
    assert!(picker.kind.uses_compact_navigation());
}

#[test]
fn panel_history_unknown_timestamp_is_not_epoch_or_hash() {
    assert_eq!(timestamp(0), "unknown");
    assert_eq!(timestamp(u64::MAX), "unknown");
}
