use super::*;

#[test]
fn displayless_linux_background_defaults_to_true_headless() {
    for (display, wayland) in [(None, None), (Some("".into()), Some("".into()))] {
        let display = display_available_with(false, display, wayland);
        let args = chrome_args_with(Path::new("/private-profile"), true, display, false);
        assert!(args.iter().any(|arg| arg == "--headless=new"));
        assert!(args.iter().any(|arg| arg == "--window-size=1280,900"));
        assert!(!args.iter().any(|arg| arg.starts_with("--window-position")));
        assert!(args.iter().any(|arg| arg == "--remote-debugging-pipe"));
        assert!(
            !args
                .iter()
                .any(|arg| arg.starts_with("--remote-debugging-port"))
        );
    }
}

#[test]
fn desktop_background_keeps_offscreen_window_unless_headless_is_forced() {
    for (macos, display, wayland) in [
        (true, None, None),
        (false, Some(":0".into()), None),
        (false, None, Some("wayland-0".into())),
    ] {
        let display = display_available_with(macos, display, wayland);
        let background = chrome_args_with(Path::new("/p"), true, display, false);
        assert!(
            background
                .iter()
                .any(|arg| arg == "--window-position=-32000,-32000")
        );
        assert!(!background.iter().any(|arg| arg.starts_with("--headless")));
        let forced = chrome_args_with(Path::new("/p"), true, display, true);
        assert!(forced.iter().any(|arg| arg == "--headless=new"));
        assert!(
            !forced
                .iter()
                .any(|arg| arg.starts_with("--window-position"))
        );
    }
    // Explicit interactive sign-in must remain visible even with the override.
    let visible = chrome_args_with(Path::new("/p"), false, true, true);
    assert!(!visible.iter().any(|arg| arg.starts_with("--headless")));
    assert!(
        !visible
            .iter()
            .any(|arg| arg.starts_with("--window-position"))
    );
}

#[test]
fn loading_timeout_blocks_session_replacement_instead_of_treating_it_as_sign_out() {
    for probe in [
        json!({"ready": false, "composer": false, "picker": false, "login": false}),
        json!({"ready": true, "composer": false, "picker": false, "login": false}),
        json!({"ready": false, "login": true}),
        Value::Null,
    ] {
        let access = classify_access(&probe);
        assert_eq!(access, PageAccess::Loading);
        // This production guard runs before either session import or interactive
        // recovery, so slow/failed probes cannot replace the private account.
        let error = ensure_page_loaded(access).unwrap_err().to_string();
        assert!(error.contains("did not finish loading"));
        assert!(error.contains(&format!("{} seconds", PAGE_ACCESS_TIMEOUT.as_secs())));
    }
    let signed_out = classify_access(&json!({"ready": true, "login": true}));
    assert_eq!(signed_out, PageAccess::SignedOut);
    assert!(ensure_page_loaded(signed_out).is_ok());
    assert!(ensure_page_loaded(PageAccess::SignedIn).is_ok());
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "launches the real private Chrome headless against chatgpt.com in an isolated JCODE_HOME; never signs in or sends a prompt"]
async fn chatgpt_web_instance_live_headless_launch_reports_signed_out_profile() {
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("chatgpt-web").join("profile");
    let chrome = find_chrome().expect("Chrome or Chromium for this system");
    let instance = Instance::launch(&chrome, &profile, true).await.unwrap();
    let page = instance.open_page(CHATGPT_WEB_URL).await.unwrap();
    let access = wait_for_access(&instance, &page, PAGE_ACCESS_TIMEOUT).await;
    let host = instance.evaluate(&page, "location.hostname").await.unwrap();
    instance.close_page(&page).await.unwrap();
    // A fresh profile is signed out, and the page is ChatGPT itself, not a
    // bot challenge.
    assert_eq!(host, json!("chatgpt.com"));
    assert_eq!(access, PageAccess::SignedOut);
    // A second jcode process waits for the profile lock instead of
    // starting a competing Chrome on the same profile.
    assert!(
        tokio::time::timeout(Duration::from_millis(500), ProfileLock::acquire(&profile))
            .await
            .is_err()
    );
    instance.shutdown().await;
    assert!(
        tokio::time::timeout(Duration::from_secs(2), ProfileLock::acquire(&profile))
            .await
            .is_ok()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn waiting_process_requests_handoff_and_clears_the_request() {
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("chatgpt-web").join("profile");
    let held = ProfileLock::acquire(&profile).await.unwrap();
    assert!(!handoff_requested(&profile));
    let waiter = tokio::spawn({
        let profile = profile.clone();
        async move { ProfileLock::acquire(&profile).await }
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while !handoff_requested(&profile) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "waiter never asked for the profile"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(held);
    let lock = tokio::time::timeout(Duration::from_secs(2), waiter)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        !handoff_requested(&profile),
        "the request is removed once the waiter holds the profile"
    );
    drop(lock);
}
