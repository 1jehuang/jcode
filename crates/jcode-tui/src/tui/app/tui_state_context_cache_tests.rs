use super::*;
use crate::tui::app::tests::create_test_app;

fn cached(app: &App) -> CachedContextSnapshot {
    CachedContextSnapshot {
        session_key: "remote-session".into(),
        is_remote: app.is_remote,
        display_messages_version: app.display_messages_version,
        context_revision: app.context_revision,
        message_count: 2,
        compaction_count: 0,
        compaction_summary_chars: 0,
        is_compacting: false,
        snapshot: crate::tui::ContextSnapshot {
            info: Some(crate::prompt::ContextInfo::default()),
            revision: app.context_revision,
            fresh: true,
        },
    }
}

#[test]
fn remote_context_cache_survives_idle_time_but_invalidates_on_revisions() {
    let mut app = create_test_app();
    app.is_remote = true;
    let entry = cached(&app);
    let age = Duration::from_secs(3600);
    let lookup = |app: &App| {
        reusable_context_snapshot(app, age, &entry, "remote-session", 2, (0, 0, false)).is_some()
    };
    assert!(
        lookup(&app),
        "unchanged remote history must not be rescanned after TTL"
    );
    app.display_messages_version += 1;
    assert!(
        !lookup(&app),
        "same-length edits must invalidate the snapshot"
    );
    app.display_messages_version -= 1;
    app.context_revision += 1;
    assert!(!lookup(&app), "prompt changes must invalidate the snapshot");
    app.context_revision -= 1;
    assert!(lookup(&app));
    assert!(
        reusable_context_snapshot(&app, age, &entry, "other-session", 2, (0, 0, false)).is_none()
    );
    assert!(
        reusable_context_snapshot(&app, age, &entry, "remote-session", 3, (0, 0, false)).is_none()
    );
    assert!(
        reusable_context_snapshot(&app, age, &entry, "remote-session", 2, (1, 0, false)).is_none()
    );
    assert!(
        reusable_context_snapshot(&app, age, &entry, "remote-session", 2, (0, 1, false)).is_none()
    );
    assert!(
        reusable_context_snapshot(&app, age, &entry, "remote-session", 2, (0, 0, true)).is_none()
    );
    app.is_remote = false;
    assert!(
        !lookup(&app),
        "local/remote transitions cannot share snapshots"
    );
}

#[test]
fn local_context_cache_retains_ttl_for_unversioned_provider_inputs() {
    let mut app = create_test_app();
    app.is_remote = false;
    let entry = cached(&app);
    let lookup = |age| {
        reusable_context_snapshot(&app, age, &entry, "remote-session", 2, (0, 0, false)).is_some()
    };
    assert!(lookup(CONTEXT_SNAPSHOT_TTL - Duration::from_nanos(1)));
    assert!(!lookup(CONTEXT_SNAPSHOT_TTL));
    assert!(!lookup(Duration::from_secs(3600)));
}
