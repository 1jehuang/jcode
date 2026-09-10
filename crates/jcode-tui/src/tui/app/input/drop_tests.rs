use super::*;
use crate::tui::app::tests::create_test_app;

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("drop directory");
    std::fs::create_dir(&nested).unwrap();
    let file = nested.join("my notes.txt");
    std::fs::write(&file, b"readable notes").unwrap();
    (dir, file)
}

#[test]
fn issue_1206_complete_non_image_drop_resolves_to_openable_path() {
    let (_dir, file) = fixture();
    let path = file.to_str().unwrap();
    for dropped in [
        path.replace(' ', "\\ "),
        format!("'{path}'"),
        format!("\"{path}\""),
        url::Url::from_file_path(&file).unwrap().to_string(),
    ] {
        let mut app = create_test_app();
        app.set_input_for_test(dropped.clone());
        assert!(promote_dropped_images(&mut app), "{dropped}");
        assert!(std::path::Path::new(&app.input).is_file(), "{}", app.input);
        assert_eq!(std::fs::read(&app.input).unwrap(), b"readable notes");
        assert_eq!(app.cursor_pos, app.input.len());
        assert_eq!(app.input_undo_stack.last().unwrap().0, dropped);
        assert!(app.pending_images.is_empty());
    }
}

#[test]
fn issue_1206_clean_paths_and_prose_do_not_change_or_add_undo() {
    let (_dir, file) = fixture();
    for input in [
        file.display().to_string(),
        format!("Review {}", file.display().to_string().replace(' ', "\\ ")),
        format!("'{}' missing-file.txt", file.display()),
    ] {
        let mut app = create_test_app();
        app.set_input_for_test(input.clone());
        let undo = app.input_undo_stack.clone();
        assert!(!promote_dropped_images(&mut app));
        assert_eq!(app.input, input);
        assert_eq!(app.input_undo_stack, undo);
    }
}

#[test]
fn issue_1206_multi_drop_stays_separable_and_idempotent() {
    let (dir, first) = fixture();
    let second = dir.path().join("second notes.rs");
    std::fs::write(&second, b"code").unwrap();
    let mut app = create_test_app();
    app.set_input_for_test(format!("'{}' '{}'", first.display(), second.display()));
    assert!(promote_dropped_images(&mut app));
    assert_eq!(
        parse_dropped_paths(&app.input).unwrap(),
        vec![first, second]
    );
    let undo = app.input_undo_stack.clone();
    assert!(!promote_dropped_images(&mut app));
    assert_eq!(app.input_undo_stack, undo);
}

#[cfg(unix)]
#[test]
fn issue_1206_multi_drop_preserves_literal_shell_punctuation() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first's.txt");
    let second = dir.path().join("second\\file.txt");
    for file in [&first, &second] {
        std::fs::write(file, b"notes").unwrap();
    }
    let mut app = create_test_app();
    app.set_input_for_test(format!("\"{}\" '{}'", first.display(), second.display()));
    promote_dropped_images(&mut app);
    assert_eq!(
        parse_dropped_paths(&app.input).unwrap(),
        vec![first, second]
    );
}

#[test]
fn issue_1206_key_stream_preserves_escaped_prefix_until_submission() {
    let (dir, first) = fixture();
    let second = dir.path().join("second notes.txt");
    std::fs::write(&second, b"second").unwrap();
    let stream = format!("{} {}", first.display(), second.display()).replace(' ', "\\ ");
    // A literal separator, unlike each escaped space within the two paths.
    let stream = stream.replacen(".txt\\ ", ".txt ", 1);
    let mut app = create_test_app();
    for ch in stream.chars() {
        handle_text_input(&mut app, &ch.to_string());
    }
    assert_eq!(
        app.input, stream,
        "do not rewrite partially received key streams"
    );
    let prepared = take_prepared_input(&mut app);
    assert_eq!(
        parse_dropped_paths(&prepared.expanded).unwrap(),
        vec![first, second]
    );
    assert!(!prepared.expanded.contains("\\ "));
}

#[test]
fn issue_1206_local_submit_and_bracketed_paste_both_resolve_paths() {
    let (_dir, file) = fixture();
    let escaped = file.display().to_string().replace(' ', "\\ ");
    let mut app = create_test_app();
    handle_paste(&mut app, escaped.clone());
    assert!(std::path::Path::new(&app.input).is_file());
    app.set_input_for_test(escaped);
    app.submit_input();
    let message = app.session.messages.last().unwrap();
    assert!(
        matches!(message.content.as_slice(), [ContentBlock::Text { text, .. }] if std::path::Path::new(text).is_file())
    );
}

#[test]
fn issue_1206_image_drop_still_attaches_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("sample image.png");
    std::fs::write(&image, b"image payload").unwrap();
    let mut app = create_test_app();
    app.set_input_for_test(image.display().to_string().replace(' ', "\\ "));
    assert!(promote_dropped_images(&mut app));
    assert_eq!(app.input, "[image 1]");
    assert_eq!(
        app.pending_images,
        vec![(
            "image/png".into(),
            base64::engine::general_purpose::STANDARD.encode(b"image payload")
        )]
    );
}
