//! Focused project scope regression tests.
use super::*;

#[test]
fn test_resume_prefers_current_project_filter_when_it_has_sessions() {
    let mut here = make_session("here", "here", false, SessionStatus::Closed);
    here.working_dir = Some("/work/project".to_string());
    let mut sub = make_session("sub", "sub", false, SessionStatus::Closed);
    sub.working_dir = Some("/work/project/crates/a".to_string());
    let mut other = make_session("other", "other", false, SessionStatus::Closed);
    other.working_dir = Some("/work/other".to_string());

    let mut picker = SessionPicker::new(vec![here, sub, other]);
    picker.set_current_dir(Some("/work/project".to_string()));
    picker.prefer_current_dir_filter();

    assert_eq!(picker.filter_mode, SessionFilterMode::CurrentDir);
    let ids: Vec<_> = picker
        .visible_session_iter()
        .map(|session| session.id.clone())
        .collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    assert!(!ids.contains(&"other".to_string()));

    // Once the user picks a filter, reloads must not snap it back.
    while picker.filter_mode != SessionFilterMode::All {
        picker.cycle_filter_mode();
    }
    picker.prefer_current_dir_filter();
    assert_eq!(picker.filter_mode, SessionFilterMode::All);
}

#[test]
fn test_resume_keeps_all_filter_when_current_project_has_no_sessions() {
    let mut other = make_session("other", "other", false, SessionStatus::Closed);
    other.working_dir = Some("/work/other".to_string());

    let mut picker = SessionPicker::new(vec![other]);
    picker.set_current_dir(Some("/work/project".to_string()));
    picker.prefer_current_dir_filter();

    assert_eq!(picker.filter_mode, SessionFilterMode::All);
    assert_eq!(picker.visible_sessions.len(), 1);
}

#[test]
fn test_current_dir_matches_sibling_git_worktree() {
    let root = tempfile::tempdir().expect("tempdir");
    let main = root.path().join("repo");
    let wt_git = main.join(".git/worktrees/feature");
    std::fs::create_dir_all(&wt_git).expect("worktree gitdir");
    std::fs::write(wt_git.join("commondir"), "../..\n").expect("commondir");
    let worktree = root.path().join("repo-feature");
    std::fs::create_dir_all(worktree.join("src")).expect("worktree dir");
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", wt_git.display()),
    )
    .expect(".git file");
    let unrelated = root.path().join("unrelated");
    std::fs::create_dir_all(&unrelated).expect("unrelated dir");

    let mut in_worktree = make_session("wt", "wt", false, SessionStatus::Closed);
    in_worktree.working_dir = Some(worktree.join("src").display().to_string());
    let mut elsewhere = make_session("elsewhere", "elsewhere", false, SessionStatus::Closed);
    elsewhere.working_dir = Some(unrelated.display().to_string());

    let mut picker = SessionPicker::new(vec![in_worktree.clone(), elsewhere.clone()]);
    picker.set_current_dir(Some(main.display().to_string()));

    assert!(picker.session_in_current_dir(&in_worktree));
    assert!(!picker.session_in_current_dir(&elsewhere));
}

#[test]
fn test_compact_dir_keeps_whole_trailing_components() {
    use super::super::render::compact_dir;
    assert_eq!(compact_dir("/srv/app"), "/srv/app");
    assert_eq!(
        compact_dir("/srv/relatorios/sherlocker-formula-20261010/worktrees/base"),
        "…/worktrees/base"
    );
    assert_eq!(
        compact_dir("/srv/a/an-extremely-long-single-directory-name-here"),
        "…/an-extremely-long-single-…"
    );
}

#[test]
fn test_resume_project_scope_is_off_by_default() {
    assert!(
        !crate::config::Config::default()
            .display
            .resume_project_scope
    );
    let mut here = make_session("here", "here", false, SessionStatus::Closed);
    here.working_dir = Some("/work/project".to_string());
    let mut other = make_session("other", "other", false, SessionStatus::Closed);
    other.working_dir = Some("/work/other".to_string());

    let mut picker = SessionPicker::new(vec![here, other]);
    picker.set_current_dir(Some("/work/project".to_string()));
    if !crate::config::config().display.resume_project_scope {
        picker.apply_resume_project_scope_default();
        assert_eq!(picker.filter_mode, SessionFilterMode::All);
        assert_eq!(picker.visible_sessions.len(), 2);
    }
}

#[test]
fn preview_renders_pinned_todos_global_prompt_numbers_and_hides_todo_tool_cards() {
    let _env_lock = crate::storage::lock_test_env();
    crate::env::set_var("JCODE_PIN_TODOS", "1");
    crate::config::invalidate_config_cache();

    let mut session = make_session_with_many_turns("pinned_preview", 60);
    session.messages_preview.push(PreviewMessage {
        role: "tool".to_string(),
        content: "DUPLICATE_TODO_TOOL_CARD".to_string(),
        tool_calls: Vec::new(),
        tool_data: Some(crate::message::ToolCall {
            id: "todo-tool".to_string(),
            name: "todo".to_string(),
            input: serde_json::json!({"todos": []}),
            intent: None,
            thought_signature: None,
        }),
        timestamp: None,
    });
    let mut picker = SessionPicker::new(vec![session]);
    picker.preview_extras.insert(
        "pinned_preview".to_string(),
        PreviewExtras {
            hidden_prompts: 100,
            todos_payload: Some(
                serde_json::json!({
                    "todos": [{
                        "id": "1",
                        "content": "PINNED_PREVIEW_TASK",
                        "status": "in_progress",
                        "priority": "high"
                    }]
                })
                .to_string(),
            ),
            ..Default::default()
        },
    );

    let text = buffer_text(&mut picker, 120, 30);
    crate::env::remove_var("JCODE_PIN_TODOS");
    crate::config::invalidate_config_cache();

    assert!(text.contains("PINNED_PREVIEW_TASK"), "{text}");
    assert!(!text.contains("DUPLICATE_TODO_TOOL_CARD"), "{text}");
    // 60 visible prompts after 100 hidden ones: the last is numbered 160.
    assert!(text.contains("160›"), "{text}");
    // Pinned todos sit at the top of the preview content, under the sticky prompt.
    let rows: Vec<String> = text
        .chars()
        .collect::<Vec<_>>()
        .chunks(120)
        .map(|chunk| chunk.iter().collect())
        .collect();
    let todo_row = rows
        .iter()
        .position(|row| row.contains("PINNED_PREVIEW_TASK"))
        .expect("todo row");
    let header_height = crate::tui::ui::header::build_preview_header(
        picker.selected_session().expect("selected session"),
        70,
    )
    .len();
    assert!(
        todo_row <= header_height + 4,
        "todo band should be pinned at top, row {todo_row}:\n{text}"
    );
}
