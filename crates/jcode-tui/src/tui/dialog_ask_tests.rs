use super::*;
use crate::protocol::{AskUserAnswer, AskUserOption, AskUserQuestion};
use ratatui::backend::TestBackend;

fn question(text: &str, header: &str, labels: &[(&str, &str)], multi: bool) -> AskUserQuestion {
    AskUserQuestion {
        question: text.into(),
        header: header.into(),
        options: labels
            .iter()
            .map(|(label, description)| AskUserOption {
                label: (*label).into(),
                description: (*description).into(),
            })
            .collect(),
        multi_select: multi,
    }
}

fn questions() -> Vec<AskUserQuestion> {
    vec![
        question(
            "Which database should the service use for persistent storage?",
            "Database",
            &[
                ("Postgres (Recommended)", "Server database with full SQL"),
                ("SQLite", "Embedded file database"),
            ],
            false,
        ),
        question(
            "Which extras?",
            "Extras",
            &[("Auth", "Login"), ("Cache", "Redis"), ("Queue", "Jobs")],
            true,
        ),
    ]
}

fn press(dialog: &mut Dialog, code: KeyCode) -> DialogKey {
    dialog.handle_key(code, KeyModifiers::empty())
}

fn screen(dialog: &Dialog, width: u16) -> Vec<String> {
    let height = dialog.desired_height(width);
    let mut terminal = ratatui::Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| dialog.render(frame, frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

#[test]
fn renders_one_question_with_progress_options_and_other() {
    let dialog = Dialog::ask_user("ask-1".into(), "call_1".into(), questions());
    let rows = screen(&dialog, 80);
    assert_eq!(
        rows[1..].to_vec(),
        [
            "   Question 1/2 · Database",
            "",
            "   Which database should the service use for persistent storage?",
            "",
            "   ❯ 1. Postgres (Recommended)   Server database with full SQL",
            "     2. SQLite                   Embedded file database",
            "     3. Other (type your answer)",
            "",
            "   ↑/↓ to navigate · Enter to select · Esc to dismiss",
        ]
    );
}

#[test]
fn narrow_rows_truncate_by_display_cells_and_prose_wraps() {
    let dialog = Dialog::ask_user("ask-1".into(), "call_1".into(), questions());
    let rows = screen(&dialog, 24);
    for row in &rows {
        assert!(row.width() <= 24, "{row:?} wider than 24 cells");
    }
    assert!(
        rows.iter().any(|row| row == "   ❯ 1. Postgres (Recom…"),
        "{rows:#?}"
    );
    // The question is prose: it wraps instead of truncating.
    let question: String = rows
        .iter()
        .skip(3)
        .take_while(|row| !row.is_empty())
        .map(|row| row.trim())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        question,
        "Which database should the service use for persistent storage?"
    );
}

#[test]
fn single_then_multi_select_with_other_text() {
    let mut dialog = Dialog::ask_user("ask-1".into(), "call_1".into(), questions());
    assert_eq!(press(&mut dialog, KeyCode::Char('2')), DialogKey::Handled);
    let rows = screen(&dialog, 80);
    assert_eq!(rows[1], "   Question 2/2 · Extras");
    assert!(rows.iter().any(|row| row.contains("[ ] Auth")), "{rows:#?}");
    assert!(rows.iter().any(|row| row.ends_with("Submit")), "{rows:#?}");

    press(&mut dialog, KeyCode::Char(' ')); // toggle Auth
    press(&mut dialog, KeyCode::Char('2')); // Cache on
    press(&mut dialog, KeyCode::Char('2')); // and off again
    press(&mut dialog, KeyCode::Char('3')); // Queue on
    press(&mut dialog, KeyCode::Char('4')); // focus Other
    for ch in "S3 bucket".chars() {
        press(&mut dialog, KeyCode::Char(ch));
    }
    press(&mut dialog, KeyCode::Backspace);
    press(&mut dialog, KeyCode::Char('t'));
    let rows = screen(&dialog, 80);
    assert!(rows.iter().any(|row| row.contains("[x] Auth")), "{rows:#?}");
    assert!(
        rows.iter().any(|row| row.contains("[x] Other: S3 bucket")),
        "{rows:#?}"
    );
    press(&mut dialog, KeyCode::Down); // Submit
    assert_eq!(
        press(&mut dialog, KeyCode::Enter),
        DialogKey::AskUser {
            request_id: "ask-1".into(),
            answers: Some(vec![
                AskUserAnswer {
                    question: questions()[0].question.clone(),
                    answers: vec!["SQLite".into()],
                },
                AskUserAnswer {
                    question: "Which extras?".into(),
                    answers: vec!["Auth".into(), "Queue".into(), "S3 bucket".into()],
                },
            ]),
        }
    );
    assert!(!dialog.is_ask_user());
}

#[test]
fn other_needs_text_in_single_select_and_esc_dismisses() {
    let mut dialog = Dialog::ask_user("ask-1".into(), "call_1".into(), questions());
    press(&mut dialog, KeyCode::Up); // wraps to Other
    assert_eq!(press(&mut dialog, KeyCode::Enter), DialogKey::Handled);
    assert!(screen(&dialog, 80)[1].contains("Question 1/2"));
    for ch in "MySQL".chars() {
        press(&mut dialog, KeyCode::Char(ch));
    }
    press(&mut dialog, KeyCode::Enter);
    assert!(screen(&dialog, 80)[1].contains("Question 2/2"));
    assert_eq!(
        press(&mut dialog, KeyCode::Esc),
        DialogKey::AskUser {
            request_id: "ask-1".into(),
            answers: None,
        }
    );
}

#[test]
fn parallel_requests_queue_and_finished_calls_drop_out() {
    let one = vec![question("A?", "A", &[("x", "1"), ("y", "2")], false)];
    let mut dialog = Dialog::ask_user("ask-1".into(), "call_1".into(), one.clone());
    dialog.queue_ask_user("ask-2".into(), "call_2".into(), one.clone());
    dialog.queue_ask_user("ask-3".into(), "call_3".into(), one);
    assert!(!dialog.ask_user_finished("call_2"));
    assert!(matches!(
        press(&mut dialog, KeyCode::Enter),
        DialogKey::AskUser { request_id, .. } if request_id == "ask-1"
    ));
    assert!(dialog.is_ask_user(), "the queued call is shown next");
    assert!(dialog.ask_user_finished("call_3"));
    assert!(!dialog.is_ask_user());
}
