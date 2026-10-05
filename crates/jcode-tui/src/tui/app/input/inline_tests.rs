#[test]
fn ssh_clipboard_image_bytes_work_without_local_file_or_url_fetch() {
    if crate::tui::app::commands_dispatch::ssh_test_runs_in_child(
        "ssh_clipboard_image_bytes_work_without_local_file_or_url_fetch",
    ) {
        return;
    }
    let content = super::read_clipboard_for_paste_with(
        &super::ClipboardPasteKind::Smart,
        || None,
        || Some(("image/png".to_string(), "aW1hZ2U=".to_string())),
        |_| panic!("clipboard image bytes must not fetch a URL"),
    );
    assert!(matches!(
        content,
        super::ClipboardPasteContent::Image { .. }
    ));
    assert!(super::download_image_url_content("http://127.0.0.1/secret.png").is_none());
    let content = super::read_clipboard_for_paste_with(
        &super::ClipboardPasteKind::Smart,
        || Some("http://127.0.0.1/secret.png".to_string()),
        || panic!("text must stay text"),
        super::download_image_url_content,
    );
    assert!(matches!(content, super::ClipboardPasteContent::Text(_)));
    // The TUI reads the local clipboard over SSH too. A picture copied
    // locally (file name plus image bytes) still attaches the bytes.
    let content = super::read_clipboard_for_paste_with(
        &super::ClipboardPasteKind::Smart,
        || Some("screenshot.png".to_string()),
        || Some(("image/png".to_string(), "aW1hZ2U=".to_string())),
        |_| panic!("clipboard image bytes must not fetch a URL"),
    );
    assert!(matches!(
        content,
        super::ClipboardPasteContent::Image { .. }
    ));
    // A remote path copied from the terminal has no image bytes on the
    // clipboard, so it stays text.
    let content = super::read_clipboard_for_paste_with(
        &super::ClipboardPasteKind::Smart,
        || Some("/home/me/screenshot.png".to_string()),
        || None,
        |_| None,
    );
    assert!(
        matches!(content, super::ClipboardPasteContent::Text(ref t) if t == "/home/me/screenshot.png")
    );
}

use super::{
    ClipboardPasteContent, ClipboardPasteKind, dropped_image_files, is_clipboard_paste_shortcut,
    parse_dropped_paths, preferred_wayland_text_type, read_clipboard_for_paste_with,
    shifted_printable_fallback, text_input_for_key,
};
use crossterm::event::{KeyCode, KeyModifiers};

/// Drop parsing is disabled under SSH. Other tests set `JCODE_SSH_REMOTE`
/// in-process while holding the env lock, so hold it too and run local.
fn with_local_env<T>(f: impl FnOnce() -> T) -> T {
    let _guard = crate::storage::lock_test_env();
    let previous = std::env::var_os("JCODE_SSH_REMOTE");
    crate::env::remove_var("JCODE_SSH_REMOTE");
    let result = f();
    if let Some(previous) = previous {
        crate::env::set_var("JCODE_SSH_REMOTE", previous);
    }
    result
}

#[test]
fn dropped_paths_accept_quotes_shell_escapes_and_file_urls() {
    with_local_env(|| {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first image.png");
        let second = dir.path().join("second.jpg");
        std::fs::write(&first, b"png").unwrap();
        std::fs::write(&second, b"jpeg").unwrap();

        let quoted = parse_dropped_paths(&format!("'{}'", first.display())).unwrap();
        assert_eq!(quoted, vec![first.clone()]);
        let escaped =
            parse_dropped_paths(&first.display().to_string().replace(' ', "\\ ")).unwrap();
        assert_eq!(escaped, vec![first.clone()]);
        let url = url::Url::from_file_path(&second).unwrap();
        assert_eq!(parse_dropped_paths(url.as_str()).unwrap(), vec![second]);
    });
}

#[test]
fn dropped_images_load_all_supported_files_and_reject_mixed_text() {
    with_local_env(|| {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("a.png");
        let jpeg = dir.path().join("b.jpeg");
        let png_bytes = super::tiny_png_bytes_for_test();
        let mut jpeg_bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(1, 1, image::Rgb([4, 5, 6])))
            .write_to(&mut jpeg_bytes, image::ImageFormat::Jpeg)
            .unwrap();
        let jpeg_bytes = jpeg_bytes.into_inner();
        std::fs::write(&png, &png_bytes).unwrap();
        std::fs::write(&jpeg, &jpeg_bytes).unwrap();

        let images =
            dropped_image_files(&format!("'{}' '{}'", png.display(), jpeg.display())).unwrap();
        assert_eq!(images[0], ("image/png".to_string(), png_bytes));
        assert_eq!(images[1], ("image/jpeg".to_string(), jpeg_bytes));
        assert!(dropped_image_files("ordinary pasted text").is_none());
    });
}

/// #1712: a dropped BMP is attached as PNG, never `image/bmp`, and a file
/// that is not really an image is not attached at all.
#[test]
fn dropped_bmp_is_converted_and_fake_image_is_rejected() {
    with_local_env(|| {
        let dir = tempfile::tempdir().unwrap();
        let bmp = dir.path().join("pic.bmp");
        let mut bmp_bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([7, 8, 9])))
            .write_to(&mut bmp_bytes, image::ImageFormat::Bmp)
            .unwrap();
        std::fs::write(&bmp, bmp_bytes.into_inner()).unwrap();
        let images = dropped_image_files(&bmp.display().to_string()).unwrap();
        assert_eq!(images[0].0, "image/png");
        assert!(images[0].1.starts_with(b"\x89PNG"));

        let fake = dir.path().join("fake.png");
        std::fs::write(&fake, b"png bytes").unwrap();
        assert!(dropped_image_files(&fake.display().to_string()).is_none());
    });
}

#[test]
fn smart_paste_prefers_image_when_text_is_its_file_name() {
    for name in [
        "screenshot.png",
        "/Users/me/Desktop/photo.JPG",
        "FILE:///tmp/Diagram.PNG",
        "file:///tmp/diagram.webp",
        "'/tmp/with space.gif'",
    ] {
        let content = read_clipboard_for_paste_with(
            &ClipboardPasteKind::Smart,
            || Some(name.to_string()),
            || Some(("image/png".to_string(), "base64".to_string())),
            |_| None,
        );
        assert!(
            matches!(content, ClipboardPasteContent::Image { .. }),
            "{name}: expected image, got {content:?}"
        );
    }
}

#[test]
fn smart_paste_keeps_prose_ending_in_an_image_name() {
    // Greptile #1450: a sentence that ends in a file name must not be
    // replaced by image bytes that happen to be on the clipboard.
    for text in [
        "Please check figure.png",
        "fix the layout in screenshot.png",
        "see /tmp/a.png",
        "file:///tmp/no such dir/a.png",
        "/definitely/missing dir/a.png",
    ] {
        let content = super::read_clipboard_for_paste_with_files(
            &ClipboardPasteKind::Smart,
            || Some(text.to_string()),
            || Some(("image/png".to_string(), "base64".to_string())),
            |_| None,
            Vec::new,
        );
        assert!(
            matches!(content, ClipboardPasteContent::Text(ref t) if t == text),
            "{text}: expected text, got {content:?}"
        );
    }
}

#[test]
fn smart_paste_attaches_spaced_names_the_clipboard_confirms() {
    let dir = tempfile::tempdir().unwrap();
    let existing = dir.path().join("my shot.png");
    std::fs::write(&existing, b"png").unwrap();
    let finder_name = "Screenshot 2026-09-24 at 10.41.00.png";
    let finder_file = std::path::PathBuf::from("/Users/me/Desktop").join(finder_name);
    let cases: [(String, Vec<std::path::PathBuf>); 4] = [
        // Finder/Preview: bare name, the copied file is on the clipboard.
        (finder_name.to_string(), vec![finder_file.clone()]),
        // Full path of the copied file.
        (finder_file.display().to_string(), vec![finder_file.clone()]),
        // Unquoted local path that exists.
        (existing.display().to_string(), Vec::new()),
        // Quoted path with spaces.
        ("'/tmp/with space.gif'".to_string(), Vec::new()),
    ];
    for (text, files) in cases {
        let content = super::read_clipboard_for_paste_with_files(
            &ClipboardPasteKind::Smart,
            || Some(text.clone()),
            || Some(("image/png".to_string(), "base64".to_string())),
            |_| None,
            move || files,
        );
        assert!(
            matches!(content, ClipboardPasteContent::Image { .. }),
            "{text}: expected image, got {content:?}"
        );
    }
    // Without clipboard confirmation the same spaced name stays text.
    let content = super::read_clipboard_for_paste_with_files(
        &ClipboardPasteKind::Smart,
        || Some(finder_name.to_string()),
        || Some(("image/png".to_string(), "base64".to_string())),
        |_| None,
        || vec![std::path::PathBuf::from("/Users/me/Desktop/other.png")],
    );
    assert!(matches!(content, ClipboardPasteContent::Text(_)));
}

#[test]
fn existing_relative_and_home_names_with_spaces_are_recognized() {
    // Greptile #1450: an existing file named relative to the working
    // directory (or under ~/), with no clipboard file list, must count
    // as an image name. Uses an injected base directory so the test does
    // not need a writable working directory.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("shots")).unwrap();
    std::fs::write(dir.path().join("shots/my shot.png"), b"png").unwrap();
    let base = Some(dir.path().to_path_buf());
    assert!(super::names_existing_file(
        "shots/my shot.png",
        None,
        base.clone()
    ));
    assert!(super::names_existing_file(
        "~/shots/my shot.png",
        base.clone(),
        None
    ));
    let absolute = dir.path().join("shots/my shot.png");
    assert!(super::names_existing_file(
        absolute.to_str().unwrap(),
        None,
        None
    ));
    assert!(!super::names_existing_file(
        "shots/missing shot.png",
        None,
        base.clone()
    ));
    assert!(!super::names_existing_file("shots/my shot.png", None, None));
    assert!(!super::names_existing_file("shots", None, base));
}

#[test]
fn smart_paste_keeps_ordinary_text_without_probing_for_images() {
    for text in [
        "plain text",
        "see screenshot.png and fix the layout",
        "line one\n/tmp/a.png",
        "https://example.com/a.png",
    ] {
        let content = read_clipboard_for_paste_with(
            &ClipboardPasteKind::Smart,
            || Some(text.to_string()),
            || panic!("ordinary text must not wait on an image probe"),
            |_| None,
        );
        assert!(
            matches!(content, ClipboardPasteContent::Text(ref t) if t == text),
            "{text}: expected text, got {content:?}"
        );
    }
}

#[test]
fn smart_paste_uses_image_only_when_no_text_is_available() {
    let content = read_clipboard_for_paste_with(
        &ClipboardPasteKind::Smart,
        || None,
        || Some(("image/png".to_string(), "base64".to_string())),
        |_| None,
    );

    match content {
        ClipboardPasteContent::Image {
            media_type,
            base64_data,
        } => {
            assert_eq!(media_type, "image/png");
            assert_eq!(base64_data, "base64");
        }
        other => panic!("expected image paste, got {other:?}"),
    }
}

#[test]
fn smart_paste_uses_text_when_no_image_is_available() {
    let content = read_clipboard_for_paste_with(
        &ClipboardPasteKind::Smart,
        || Some("plain text".to_string()),
        || None,
        |_| None,
    );

    match content {
        ClipboardPasteContent::Text(text) => assert_eq!(text, "plain text"),
        other => panic!("expected text paste, got {other:?}"),
    }
}

#[test]
fn smart_paste_empty_clipboard_stays_empty_not_dictation() {
    let content =
        read_clipboard_for_paste_with(&ClipboardPasteKind::Smart, || None, || None, |_| None);

    assert!(
        matches!(content, ClipboardPasteContent::Empty),
        "expected empty paste, got {content:?}"
    );
}

#[test]
fn smart_paste_uses_image_when_text_target_is_blank() {
    // Image-only clipboards can advertise an empty text target; the image
    // must still be pasted instead of producing a silent empty text paste.
    let content = read_clipboard_for_paste_with(
        &ClipboardPasteKind::Smart,
        || Some("   ".to_string()),
        || Some(("image/png".to_string(), "base64".to_string())),
        |_| None,
    );

    match content {
        ClipboardPasteContent::Image {
            media_type,
            base64_data,
        } => {
            assert_eq!(media_type, "image/png");
            assert_eq!(base64_data, "base64");
        }
        other => panic!("expected image paste, got {other:?}"),
    }
}

#[test]
fn paste_shortcut_accepts_control_alt_command_and_meta_v() {
    for modifiers in [
        KeyModifiers::CONTROL,
        KeyModifiers::ALT,
        KeyModifiers::SUPER,
        KeyModifiers::META,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        KeyModifiers::ALT | KeyModifiers::SHIFT,
        KeyModifiers::SUPER | KeyModifiers::SHIFT,
    ] {
        assert!(
            is_clipboard_paste_shortcut(KeyCode::Char('v'), modifiers),
            "{modifiers:?}+v should paste clipboard contents"
        );
        assert!(
            is_clipboard_paste_shortcut(KeyCode::Char('V'), modifiers),
            "{modifiers:?}+V should paste clipboard contents"
        );
    }

    assert!(!is_clipboard_paste_shortcut(
        KeyCode::Char('v'),
        KeyModifiers::empty()
    ));
}

#[test]
fn wayland_text_type_prefers_utf8_plain_text() {
    let types = "text/plain\ntext/plain;charset=utf-8\nTEXT\nSTRING\nUTF8_STRING\n";

    assert_eq!(
        preferred_wayland_text_type(types),
        Some("text/plain;charset=utf-8")
    );
}

#[test]
fn shifted_printable_fallback_uppercases_ascii_letters() {
    assert_eq!(shifted_printable_fallback('a', KeyModifiers::SHIFT), 'A');
    assert_eq!(shifted_printable_fallback('z', KeyModifiers::SHIFT), 'Z');
}

#[test]
fn shifted_printable_fallback_preserves_terminal_translated_symbols() {
    assert_eq!(shifted_printable_fallback('/', KeyModifiers::SHIFT), '/');
    assert_eq!(shifted_printable_fallback('?', KeyModifiers::SHIFT), '?');
    assert_eq!(shifted_printable_fallback('(', KeyModifiers::SHIFT), '(');
    assert_eq!(shifted_printable_fallback('&', KeyModifiers::SHIFT), '&');
}

#[test]
fn shifted_printable_fallback_does_not_synthesize_us_symbol_layout() {
    assert_eq!(shifted_printable_fallback('7', KeyModifiers::SHIFT), '7');
    assert_eq!(shifted_printable_fallback('8', KeyModifiers::SHIFT), '8');
    assert_eq!(shifted_printable_fallback('=', KeyModifiers::SHIFT), '=');
}

#[test]
fn text_input_for_shifted_symbols_preserves_layout_translated_char() {
    for c in ['/', '?', '(', ')', '&', '=', '"'] {
        assert_eq!(
            text_input_for_key(KeyCode::Char(c), KeyModifiers::SHIFT),
            Some(c.to_string()),
            "shifted {c:?} should be treated as terminal/layout-translated text"
        );
    }
}

#[test]
fn text_input_for_altgr_symbols_preserves_layout_translated_char() {
    let altgr = KeyModifiers::CONTROL | KeyModifiers::ALT;

    for c in ['@', '{', '}', '\\', '€', 'ą'] {
        assert_eq!(
            text_input_for_key(KeyCode::Char(c), altgr),
            Some(c.to_string()),
            "AltGr-style {c:?} should be treated as terminal/layout-translated text"
        );
    }
}

#[test]
fn text_input_for_control_shortcut_letters_stays_non_text() {
    assert_eq!(
        text_input_for_key(
            KeyCode::Char('q'),
            KeyModifiers::CONTROL | KeyModifiers::ALT
        ),
        None
    );
    assert_eq!(
        text_input_for_key(KeyCode::Char('@'), KeyModifiers::CONTROL),
        None
    );
}
