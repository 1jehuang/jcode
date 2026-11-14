use super::*;
use crate::replay::{TimelineEvent, TimelineEventKind};

fn todo(name: &str) -> Value {
    json!({"id":"historical", "content":name, "status":"in_progress", "priority":"high"})
}

#[test]
fn parses_saved_and_public_todos_and_labeled_metadata() {
    let array = json!([todo("Recorded task")]);
    let output = format!("{}\n\nPlan:\n{{}}\n\nGoals:\n[]", array);
    let payload: Value =
        serde_json::from_str(&todo_snapshot_from_output(&output).unwrap()).unwrap();
    assert_eq!(payload["todos"][0]["content"], "Recorded task");
    assert_eq!(payload["goals"], json!([]));
    for prefix in [
        "[todo] [tool timing: start=2026-07-13T19:51:50.261Z finish=2026-07-13T19:51:50.265Z duration=4ms] ",
        "[2026-07-13T19:51:50.261Z] [todo] ",
    ] {
        let decorated = todo_snapshot_from_output(&format!("{prefix}{output}")).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&decorated).unwrap(), payload);
    }
    let public = json!([{"name":"Named task", "description":"Details", "phase":2,
        "status":"pending", "priority":"high"}]);
    let payload: Value = serde_json::from_str(&normalize_todo_snapshot(public).unwrap()).unwrap();
    assert_eq!(payload["todos"][0]["content"], "Named task");
    assert_eq!(payload["todos"][0]["details"], "Details");
    assert_eq!(payload["todos"][0]["stage"], 2);
    assert!(todo_snapshot_from_output("not JSON").is_none());
    assert!(normalize_todo_snapshot(json!({"todos":[{}]})).is_err());
}

#[test]
fn ansi_preserves_rgb_blank_backgrounds_and_wide_cells() {
    use ratatui::{layout::Rect, style::Color};
    let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 2));
    buffer[(0, 0)]
        .set_symbol("界")
        .set_fg(Color::Rgb(10, 20, 30));
    buffer[(3, 1)].set_bg(Color::Rgb(40, 50, 60));
    let text = String::from_utf8(App::replay_frame_ansi(&buffer).unwrap()).unwrap();
    assert!(text.starts_with("\x1b[0m\x1b[2J\x1b[H"));
    assert!(text.contains("界"));
    assert!(text.contains("10;20;30"));
    assert!(text.contains("40;50;60"));
    assert!(!text.contains('\0'));
    assert!(App::replay_frame_ansi(&Buffer::empty(Rect::default())).is_err());
}

fn frame_text(buffer: &Buffer) -> String {
    buffer.content.iter().map(|cell| cell.symbol()).collect()
}

#[test]
fn historical_frames_use_production_draw_and_never_current_todo_files() {
    const CHILD: &str = "JCODE_TEST_HISTORICAL_EXPORT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[display]\npin_todos = true\n",
        )
        .unwrap();
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture"])
            .env(CHILD, "1")
            .env("JCODE_HOME", home.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated renderer regression failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("HISTORICAL_EXPORT_VERIFIED"));
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    assert!(crate::config::config().display.pin_todos);
    let session =
        crate::session::Session::create_with_id("historical_export_test".into(), None, None);
    let live: Vec<crate::todo::TodoItem> =
        serde_json::from_value(json!([todo("TODAY_ONLY_TASK")])).unwrap();
    crate::todo::save_todos(&session.id, &live).unwrap();
    let timeline = vec![
        TimelineEvent {
            t: 0,
            kind: TimelineEventKind::UserMessage {
                text: "Real recorded message".into(),
            },
        },
        TimelineEvent {
            t: 10,
            kind: TimelineEventKind::ToolDone {
                name: "todo".into(),
                output: json!([todo("HISTORICAL_FIRST")]).to_string(),
                is_error: false,
            },
        },
        TimelineEvent {
            t: 30,
            kind: TimelineEventKind::ToolDone {
                name: "todo".into(),
                output: json!([todo("HISTORICAL_LATEST")]).to_string(),
                is_error: false,
            },
        },
    ];
    let snapshots = vec![(20, json!([todo("EXPLICIT_MIDDLE")]))];
    let frames = App::new_for_replay_silent(session.clone())
        .render_replay_frames(&timeline, &[0, 10, 20, 30], &snapshots, 120, 45)
        .unwrap();
    for frame in &frames {
        assert!(!frame_text(frame).contains("TODAY_ONLY_TASK"));
    }
    assert!(frame_text(&frames[0]).contains("Real recorded message"));
    assert!(frame_text(&frames[1]).contains("HISTORICAL_FIRST"));
    assert!(frame_text(&frames[2]).contains("EXPLICIT_MIDDLE"));
    assert!(frame_text(&frames[3]).contains("HISTORICAL_LATEST"));

    // Snapshot-only frames must equal a direct production render of that state.
    let snapshot = json!({"todos":[todo("DIRECT_PARITY")], "plan":{}, "goals":[]});
    let exported = App::new_for_replay_silent(session.clone())
        .render_replay_frames(&[], &[0], &[(0, snapshot.clone())], 120, 45)
        .unwrap()
        .remove(0);
    let mut app = App::new_for_replay_silent(session.clone());
    app.restore_replay_todos_json(&snapshot.to_string());
    assert!(!app.refresh_pinned_todos_if_needed());
    let mut terminal = Terminal::new(TestBackend::new(120, 45)).unwrap();
    let direct = terminal
        .draw(|frame| crate::tui::render_frame(frame, &app))
        .unwrap()
        .buffer
        .clone();
    assert_eq!(exported, direct);
    assert!(
        App::new_for_replay_silent(session.clone())
            .render_replay_frames(&[], &[20, 10], &[], 120, 45)
            .is_err()
    );
    assert!(
        App::new_for_replay_silent(session.clone())
            .render_replay_frames(&[], &[0], &[], 0, 45)
            .is_err()
    );
    // Export must not mutate the user's current todo files.
    assert_eq!(crate::todo::load_todos(&session.id).unwrap(), live);
    println!("HISTORICAL_EXPORT_VERIFIED");
}
