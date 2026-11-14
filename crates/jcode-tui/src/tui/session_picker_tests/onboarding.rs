use super::*;

#[test]
fn onboarding_external_filter_picks_latest_visible_transcript() {
    let now = Utc::now();

    let mut older = make_session("codex_older", "older", false, SessionStatus::Closed);
    older.source = SessionSource::Codex;
    older.model = Some("gpt-5-codex".to_string());
    older.last_active_at = Some(now - ChronoDuration::minutes(30));
    older.resume_target = ResumeTarget::CodexSession {
        session_id: "codex_older".to_string(),
        session_path: "/tmp/codex_older.jsonl".to_string(),
    };

    let mut newer = make_session("codex_newer", "newer", false, SessionStatus::Closed);
    newer.source = SessionSource::Codex;
    newer.model = Some("gpt-5-codex".to_string());
    newer.last_active_at = Some(now - ChronoDuration::minutes(2));
    newer.resume_target = ResumeTarget::CodexSession {
        session_id: "codex_newer".to_string(),
        session_path: "/tmp/codex_newer.jsonl".to_string(),
    };

    // A non-Codex session that must be filtered out.
    let jcode = make_session("jcode_one", "jcode", false, SessionStatus::Closed);

    let mut picker = SessionPicker::new(vec![older, jcode, newer]);
    picker.activate_external_cli_filter(SessionFilterMode::Codex);

    assert_eq!(picker.visible_session_count(), 2);

    let latest = picker
        .latest_visible_resume_target()
        .expect("latest visible target");
    assert_eq!(
        latest,
        ResumeTarget::CodexSession {
            session_id: "codex_newer".to_string(),
            session_path: "/tmp/codex_newer.jsonl".to_string(),
        }
    );
}

#[test]
fn onboarding_external_filter_with_no_matches_has_no_target() {
    let jcode = make_session("jcode_only", "jcode", false, SessionStatus::Closed);
    let mut picker = SessionPicker::new(vec![jcode]);
    picker.activate_external_cli_filter(SessionFilterMode::ClaudeCode);

    assert_eq!(picker.visible_session_count(), 0);
    assert!(picker.latest_visible_resume_target().is_none());
}

#[test]
fn onboarding_banner_defaults_to_suggested_review() {
    let mut picker = SessionPicker::new(Vec::new());
    picker.activate_onboarding_banner(vec![Line::from("welcome")]);

    assert!(picker.onboarding_banner_active());
    assert!(picker.onboarding_review_recent_project_highlighted());
    assert!(!picker.onboarding_start_new_highlighted());
}

#[test]
fn onboarding_banner_is_action_only() {
    let mut picker = SessionPicker::new(Vec::new());
    picker.activate_onboarding_banner(vec![Line::from("welcome")]);

    assert_eq!(picker.visible_session_count(), 0);
    assert!(picker.onboarding_review_recent_project_highlighted());
}

#[test]
fn onboarding_banner_space_submits_like_enter() {
    let mut picker = SessionPicker::new(Vec::new());
    picker.activate_onboarding_banner(vec![Line::from("welcome")]);
    let action = picker
        .handle_overlay_key(KeyCode::Char(' '), KeyModifiers::empty())
        .expect("space");
    assert!(matches!(
        action,
        OverlayAction::Selected(PickerResult::ReviewRecentProject)
    ));
    assert!(picker.onboarding_review_recent_project_highlighted());
}

#[test]
fn onboarding_banner_offers_review_then_new_session() {
    let mut picker = SessionPicker::new(Vec::new());
    picker.activate_onboarding_banner(vec![Line::from("welcome")]);

    // The suggested review is the default top action.
    let action = picker
        .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("overlay key");
    assert!(matches!(
        action,
        OverlayAction::Selected(PickerResult::ReviewRecentProject)
    ));

    // Any non-submit key rotates between the two choices.
    picker
        .handle_overlay_key(KeyCode::Char('x'), KeyModifiers::empty())
        .expect("ordinary key");
    assert!(picker.onboarding_start_new_highlighted());
    picker
        .handle_overlay_key(KeyCode::Char('x'), KeyModifiers::empty())
        .expect("ordinary key");
    assert!(picker.onboarding_review_recent_project_highlighted());

    // Keys that normally close the full picker rotate on this action-only page.
    picker
        .handle_overlay_key(KeyCode::Esc, KeyModifiers::empty())
        .expect("escape key");
    assert!(picker.onboarding_start_new_highlighted());
    picker
        .handle_overlay_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
        .expect("control-c");
    assert!(picker.onboarding_review_recent_project_highlighted());

    // Arrow keys use the same rotation behavior.
    picker
        .handle_overlay_key(KeyCode::Down, KeyModifiers::empty())
        .expect("down arrow");
    assert!(picker.onboarding_start_new_highlighted());
    let action = picker
        .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("overlay key");
    assert!(matches!(
        action,
        OverlayAction::Selected(PickerResult::StartNewSession)
    ));

    // There is no session list below the two actions.
    picker.next();
    assert!(picker.onboarding_start_new_highlighted());
    picker
        .handle_overlay_key(KeyCode::Up, KeyModifiers::empty())
        .expect("up arrow");
    assert!(picker.onboarding_review_recent_project_highlighted());
}

pub(super) fn render_onboarding_picker_lines(
    picker: &mut SessionPicker,
    width: u16,
    height: u16,
) -> Vec<String> {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| picker.render(frame))
        .expect("render onboarding picker");
    let buffer = terminal.backend().buffer().clone();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

fn onboarding_test_picker() -> SessionPicker {
    let mut picker = SessionPicker::new(Vec::new());
    picker.set_current_dir(Some("/srv/projects/demo".to_string()));
    picker.activate_onboarding_banner(vec![
        Line::from("Welcome to jcode"),
        Line::from("Choose how to begin."),
    ]);
    picker
}

#[test]
fn onboarding_banner_renders_actions_side_by_side_with_or() {
    let mut picker = onboarding_test_picker();
    let lines = render_onboarding_picker_lines(&mut picker, 120, 40);
    let text = lines.join("\n");

    assert!(text.contains("Welcome to jcode"), "{lines:#?}");
    assert!(
        !text.contains("Start in the current directory"),
        "old start label must be gone: {lines:#?}"
    );
    assert!(
        !text.contains("Sessions"),
        "resume chrome must be absent: {lines:#?}"
    );
    assert!(
        !text.contains('╭') && !text.contains('╰') && !text.contains('│'),
        "onboarding choice should not render an outer boundary: {lines:#?}"
    );

    let row = lines
        .iter()
        .position(|line| line.contains("Find bugs in my most active repo"))
        .expect("review action");
    let line = &lines[row];
    let review_x = line.find("Find bugs in my most active repo").unwrap();
    let or_x = line
        .find(" or ")
        .expect("'or' between the actions on the same row");
    let start_x = line
        .find("New session in /srv/projects/demo")
        .expect("start action names the directory, on the same row");
    assert!(review_x < or_x && or_x < start_x, "{lines:#?}");

    let welcome_y = lines
        .iter()
        .position(|line| line.contains("Welcome to jcode"))
        .unwrap();
    assert!(welcome_y < row, "{lines:#?}");
    assert!(
        row.abs_diff(lines.len() / 2) <= 1,
        "actions vertically centered: {lines:#?}"
    );
}

#[test]
fn onboarding_banner_stacks_actions_when_narrow() {
    let mut picker = onboarding_test_picker();
    let lines = render_onboarding_picker_lines(&mut picker, 50, 30);

    let review_y = lines
        .iter()
        .position(|line| line.contains("Find bugs in my most active repo"))
        .expect("review action");
    let or_y = lines
        .iter()
        .position(|line| line.trim() == "or")
        .expect("'or' on its own line");
    let start_y = lines
        .iter()
        .position(|line| line.contains("New session in /srv/projects/demo"))
        .expect("start action");
    assert_eq!(or_y, review_y + 1, "{lines:#?}");
    assert_eq!(start_y, review_y + 2, "{lines:#?}");
}

#[test]
fn onboarding_start_label_marks_home_directory() {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let mut picker = SessionPicker::new(Vec::new());
    picker.set_current_dir(Some(home.display().to_string()));
    picker.activate_onboarding_banner(vec![Line::from("Welcome to jcode")]);
    let lines = render_onboarding_picker_lines(&mut picker, 120, 40);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("New session in ~ (home)")),
        "home directory should read '~ (home)': {lines:#?}"
    );
}
