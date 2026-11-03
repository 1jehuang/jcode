use super::run_interactive_editor_with;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn handoff_waits_for_editor_before_resuming_terminal() {
    let _guard = crate::tui::TERMINAL_OWNERSHIP_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    crate::tui::set_editor_terminal_ownership(false);
    let events = Rc::new(RefCell::new(Vec::new()));
    let child_events = Rc::clone(&events);
    let mut command = std::process::Command::new("sh");
    command.args(["-c", "sleep 0.05"]);

    let started = std::time::Instant::now();
    let status = run_interactive_editor_with(
        &mut command,
        {
            let events = Rc::clone(&events);
            move || events.borrow_mut().push("suspend")
        },
        move || {
            assert!(
                started.elapsed() >= std::time::Duration::from_millis(40),
                "terminal resumed before the editor process exited"
            );
            child_events.borrow_mut().push("resume");
        },
    )
    .expect("editor command should run");

    assert!(status.success());
    assert_eq!(&*events.borrow(), &["suspend", "resume"]);
}

#[test]
fn handoff_resumes_terminal_after_editor_failure_status() {
    let _guard = crate::tui::TERMINAL_OWNERSHIP_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    crate::tui::set_editor_terminal_ownership(false);
    let resumed = Rc::new(RefCell::new(false));
    let resumed_after = Rc::clone(&resumed);
    let mut command = std::process::Command::new("sh");
    command.args(["-c", "exit 7"]);

    let status = run_interactive_editor_with(
        &mut command,
        || {},
        move || {
            *resumed_after.borrow_mut() = true;
        },
    )
    .expect("shell should launch");

    assert_eq!(status.code(), Some(7));
    assert!(*resumed.borrow());
    assert!(!crate::tui::editor_owns_terminal());
}

#[test]
fn handoff_clears_editor_terminal_ownership_when_command_fails_to_launch() {
    let _guard = crate::tui::TERMINAL_OWNERSHIP_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    crate::tui::set_editor_terminal_ownership(false);
    let mut command = std::process::Command::new("/definitely/missing/jcode-editor-test");

    let result = run_interactive_editor_with(&mut command, || {}, || {});

    assert!(result.is_err());
    assert!(!crate::tui::editor_owns_terminal());
}

#[test]
fn handoff_publishes_ownership_only_while_editor_runs() {
    let _guard = crate::tui::TERMINAL_OWNERSHIP_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    crate::tui::set_editor_terminal_ownership(false);
    let saw_ownership = Arc::new(AtomicBool::new(false));
    let owns_during_resume = Rc::new(RefCell::new(true));

    // The child stays alive until the sampler confirms the flag, so a busy
    // runner delaying the sampler cannot shrink the observation window
    // below what the sampler needs (greptile #4231010584). With a fixed
    // child sleep the test failed on correct code whenever the sampler
    // started late. The child gives up after ~5s so broken code that never
    // publishes still fails the assertions instead of hanging.
    let sentinel = std::env::temp_dir().join(format!(
        "jcode-editor-ownership-{}-{}.sentinel",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_file(&sentinel);

    let sampler_sentinel = Arc::clone(&saw_ownership);
    let sampler_saw = sentinel.clone();
    let sampler = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if crate::tui::editor_owns_terminal() {
                sampler_sentinel.store(true, Ordering::SeqCst);
                let _ = std::fs::write(&sampler_saw, b"");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });

    let mut command = std::process::Command::new("sh");
    command
        .env("JCODE_OWNERSHIP_SENTINEL", &sentinel)
        .args([
            "-c",
            "i=0; while [ ! -f \"$JCODE_OWNERSHIP_SENTINEL\" ] && [ \"$i\" -lt 500 ]; do sleep 0.01; i=$((i+1)); done; [ -f \"$JCODE_OWNERSHIP_SENTINEL\" ]",
        ]);
    let owns_during_resume_after = Rc::clone(&owns_during_resume);
    let status = run_interactive_editor_with(
        &mut command,
        // The restores are still in flight here: the flag must stay false
        // so a termination signal in this window cleans up in full instead
        // of skipping it and leaving the shell in raw mode (greptile
        // #4230626960).
        || assert!(!crate::tui::editor_owns_terminal()),
        move || *owns_during_resume_after.borrow_mut() = crate::tui::editor_owns_terminal(),
    )
    .expect("shell should launch");

    sampler.join().expect("sampler thread");
    let _ = std::fs::remove_file(&sentinel);

    // Exit 0 means the sampler confirmed ownership while the child was
    // still alive; a nonzero exit means the child gave up waiting, which
    // only happens when the flag was never published.
    assert_eq!(
        status.code(),
        Some(0),
        "editor exited before the sampler observed ownership"
    );
    assert!(
        saw_ownership.load(Ordering::SeqCst),
        "ownership was never published while the editor ran"
    );
    // Reclaimed before resume re-armed jcode's modes.
    assert!(!*owns_during_resume.borrow());
    assert!(!crate::tui::editor_owns_terminal());
}

#[test]
fn reclaim_resume_does_not_reenable_modes_after_termination_started() {
    let _guard = crate::tui::TERMINAL_OWNERSHIP_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    crate::tui::set_editor_terminal_ownership(false);

    // Greptile #4232838288: reproduce the race window. The signal cleanup
    // saw the editor handoff, skipped its restore, recorded termination
    // under the kitty state lock, and released the lock while the process
    // was still alive. The editor then exited before the process did, so
    // this reclaim runs inside that gap and must not let the resume
    // re-arm modes that no later cleanup would ever disable again.
    crate::tui::publish_editor_terminal_handoff();
    crate::tui::record_termination_started();

    let resumed = Rc::new(RefCell::new(false));
    let mut command = std::process::Command::new("sh");
    command.args(["-c", "exit 0"]);
    let resumed_after = Rc::clone(&resumed);
    let status = run_interactive_editor_with(
        &mut command,
        || {},
        move || {
            *resumed_after.borrow_mut() = true;
        },
    )
    .expect("shell should launch");

    assert!(status.success());
    // The reclaim refused while terminating, so ownership stays published
    // and the resume that re-arms every mode never runs.
    assert!(
        crate::tui::editor_owns_terminal(),
        "reclaim cleared ownership while termination had already started"
    );
    assert!(
        !*resumed.borrow(),
        "resume re-armed terminal modes after termination started"
    );
    // A late keyboard push is refused too, so no entry is left unpopped
    // for the shell to inherit.
    assert!(!crate::tui::enable_keyboard_enhancement());

    // Restore the process-global flags for the other tests in this binary.
    crate::tui::set_editor_terminal_ownership(false);
    crate::tui::reset_termination_started_for_test();
}
