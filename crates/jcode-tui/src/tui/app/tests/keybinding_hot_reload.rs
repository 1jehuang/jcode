// Editing `[keybindings]` in config.toml must take effect on the very next
// keystroke, without a restart and without waiting for an idle tick (which can
// be as slow as the 5s deep-idle cadence).
#[test]
fn keybinding_edit_applies_to_the_next_key_press() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::config::Config::invalidate_cache();

    let config_path = crate::config::Config::path().expect("config path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("create config parent");
    std::fs::write(&config_path, "[keybindings]\nscroll_bookmark = \"ctrl+g\"\n")
        .expect("write initial config");

    let mut app = create_test_app();
    assert!(
        app.scroll_keys
            .is_bookmark(KeyCode::Char('g'), KeyModifiers::CONTROL),
        "initial config should bind the bookmark key to Ctrl+G"
    );

    // Rebind on disk. Length differs as well as mtime so the config
    // fingerprint notices the edit on coarse-timestamp filesystems.
    std::fs::write(
        &config_path,
        "[keybindings]\nscroll_bookmark = \"ctrl+y\"\n# edited\n",
    )
    .expect("rewrite config");

    // The config cache re-stats the file on a 500ms throttle, so wait past it
    // to model a user who edits the file and then reaches for the keyboard.
    std::thread::sleep(std::time::Duration::from_millis(600));

    // The very next key press must already see the new binding.
    app.handle_key_press_event(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL))
        .expect("handle key press");

    assert!(
        app.scroll_keys
            .is_bookmark(KeyCode::Char('y'), KeyModifiers::CONTROL),
        "edited config should rebind the bookmark key to Ctrl+Y without a restart"
    );
    assert!(
        !app.scroll_keys
            .is_bookmark(KeyCode::Char('g'), KeyModifiers::CONTROL),
        "the old Ctrl+G bookmark binding should no longer match"
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    crate::config::Config::invalidate_cache();
}
