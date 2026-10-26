// Keep the process environment pinned and restore it even when an assertion fails.
struct LatexCommandEnv {
    home: Option<std::ffi::OsString>,
    latex: Option<std::ffi::OsString>,
    _temp: tempfile::TempDir,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl LatexCommandEnv {
    fn new() -> Self {
        let lock = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let home = std::env::var_os("JCODE_HOME");
        let latex = std::env::var_os("JCODE_LATEX_RENDERING");
        crate::env::set_var("JCODE_HOME", temp.path());
        crate::env::remove_var("JCODE_LATEX_RENDERING");
        crate::config::Config::invalidate_cache();
        Self {
            home,
            latex,
            _temp: temp,
            _lock: lock,
        }
    }
}

impl Drop for LatexCommandEnv {
    fn drop(&mut self) {
        for (name, value) in [
            ("JCODE_HOME", &self.home),
            ("JCODE_LATEX_RENDERING", &self.latex),
        ] {
            if let Some(value) = value {
                crate::env::set_var(name, value);
            } else {
                crate::env::remove_var(name);
            }
        }
        crate::config::Config::invalidate_cache();
    }
}

#[test]
fn latex_command_modes_persistence_and_status() {
    let _env = LatexCommandEnv::new();
    let mut app = create_test_app();
    use crate::config::LatexRenderingMode::{Image, None, Unicode};
    app.input = "/latex".into();
    app.submit_input();
    assert!(
        app.display_messages()
            .last()
            .unwrap()
            .content
            .contains("LaTeX rendering: image")
    );
    assert!(!crate::config::Config::path().unwrap().exists());
    for (command, expected) in [
        ("/latex off", Unicode),
        ("/latex ON", Image),
        ("/latex none", None),
        ("/latex on", Image),
        ("/latex text", Unicode),
        ("/latex png", Image),
        ("/latex raw", None),
    ] {
        app.input = command.into();
        app.submit_input();
        assert_eq!(
            crate::config::config().display.latex_rendering,
            expected,
            "{command}"
        );
        assert_eq!(
            app.status_notice(),
            Some(format!("LaTeX rendering: {}", expected.as_str()))
        );
        assert_eq!(
            crate::config::Config::load_strict()
                .unwrap()
                .display
                .latex_rendering,
            expected
        );
    }
    app.input = "/latex".into();
    app.submit_input();
    assert!(
        app.display_messages()
            .last()
            .unwrap()
            .content
            .contains("LaTeX rendering: none")
    );
    app.input = "/latex status".into();
    app.submit_input();
    let msg = app.display_messages().last().unwrap();
    assert!(msg.content.contains("LaTeX rendering: none"));
    assert!(msg.content.contains("on|off"));
    assert_eq!(crate::config::config().display.latex_rendering, None);
}

#[test]
fn latex_command_rejects_unknown_mode_and_reports_save_failure() {
    let _env = LatexCommandEnv::new();
    let mut app = create_test_app();
    app.input = "/latex prettypng".into();
    app.submit_input();
    assert_eq!(app.display_messages().last().unwrap().role, "error");
    assert!(
        app.display_messages()
            .last()
            .unwrap()
            .content
            .contains("Usage: /latex")
    );
    assert_eq!(
        crate::config::config().display.latex_rendering,
        crate::config::LatexRenderingMode::Image
    );
    let path = crate::config::Config::path().unwrap();
    std::fs::create_dir_all(path).unwrap();
    app.input = "/latex off".into();
    app.submit_input();
    let msg = app.display_messages().last().unwrap();
    assert_eq!(msg.role, "error");
    assert!(msg.content.contains("keeps image"));
    assert_eq!(
        crate::config::config().display.latex_rendering,
        crate::config::LatexRenderingMode::Image
    );
}

#[test]
fn latex_command_rerenders_existing_math_in_the_terminal() {
    let _env = LatexCommandEnv::new();
    let mut app = create_test_app();
    crate::tui::markdown::install_jcode_markdown_hooks();
    app.push_display_message(DisplayMessage::assistant(
        "Inline: $x^2 + \\alpha$\n\n$$\ny^2 + \\beta\n$$",
    ));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 50))
        .expect("terminal backend");

    // Keep the same app, transcript and rendering caches across commands. This
    // catches stale prefix reuse, not just a correct value in config.toml.
    for (mode, inline, display) in [
        ("none", "$x^2 + \\alpha$", "y^2 + \\beta"),
        ("unicode", "x² + α", "y² + β"),
        ("none", "$x^2 + \\alpha$", "y^2 + \\beta"),
    ] {
        app.input = format!("/latex {mode}");
        app.submit_input();
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            screen.contains(inline),
            "{mode} inline math missing: {screen}"
        );
        assert!(
            screen.contains(display),
            "{mode} display math missing: {screen}"
        );
        let stale = if mode == "unicode" {
            "$x^2 + \\alpha$"
        } else {
            "x² + α"
        };
        assert!(
            !screen.contains(stale),
            "{mode} retained old math output: {screen}"
        );
        println!(
            "Observed terminal mode {mode}: inline={inline:?}, display={display:?}; no stale math"
        );
    }
}

#[test]
fn latex_command_env_override_and_help() {
    let _env = LatexCommandEnv::new();
    crate::env::set_var("JCODE_LATEX_RENDERING", "unicode");
    crate::config::Config::invalidate_cache();
    let mut app = create_test_app();
    app.input = "/latex on".into();
    app.submit_input();
    assert_eq!(app.status_notice(), Some("LaTeX rendering: unicode".into()));
    assert!(
        app.display_messages()
            .last()
            .unwrap()
            .content
            .contains("JCODE_LATEX_RENDERING overrides")
    );
    let raw = std::fs::read_to_string(crate::config::Config::path().unwrap()).unwrap();
    assert!(raw.contains("latex_rendering = \"image\""));
    app.input = "/help latex".into();
    app.submit_input();
    let msg = app.display_messages().last().unwrap();
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/latex on|off"));
    assert!(msg.content.contains("JCODE_LATEX_RENDERING"));
}
