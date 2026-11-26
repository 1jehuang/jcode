use super::*;

thread_local! {
    static LEGACY_EFFORT_BOUNDARY: std::cell::RefCell<Option<std::sync::mpsc::Sender<()>>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn legacy_effort_boundary() {
    LEGACY_EFFORT_BOUNDARY.with(|signal| {
        if let Some(signal) = signal.borrow_mut().take() {
            signal.send(()).unwrap();
        }
    });
}

#[test]
fn legacy_effort_releases_catalog_before_waiting_for_workspace_binding() {
    let root = tempfile::tempdir().unwrap();
    let provider = Arc::new(CursorAcpProvider::with_command(CursorAcpCommand::new(
        "unused-test-agent",
        [root.path().to_str().unwrap()],
    )));
    let low = "future[effort=low]";
    let high = "future[effort=high]";
    let catalog = ModelCatalog {
        models: vec![low.into(), high.into()],
        current: Some(low.into()),
        ..Default::default()
    };
    provider.published().publish(&catalog, false);
    let binding = provider.binding.write().unwrap();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let setter_provider = provider.clone();
    let setter = std::thread::spawn(move || {
        LEGACY_EFFORT_BOUNDARY.with(|signal| *signal.borrow_mut() = Some(ready_tx));
        let result = setter_provider.set_reasoning_effort("high");
        done_tx.send(result).unwrap();
    });
    let ready = ready_rx.recv_timeout(Duration::from_secs(2));
    // This is the catalog write a workspace rebind needs while owning binding.
    let catalog_available = provider.catalog.try_write().is_ok();
    drop(binding);
    let done = done_rx.recv_timeout(Duration::from_secs(2));
    let joined = done.as_ref().ok().map(|_| setter.join());
    assert!(
        ready.is_ok(),
        "legacy selector did not reach binding boundary"
    );
    assert!(
        catalog_available,
        "legacy selector retained the catalog reader"
    );
    assert!(done.unwrap().is_ok());
    assert!(joined.unwrap().is_ok());
    assert_eq!(provider.model(), high);
}

#[test]
fn default_executable_prefers_agent_and_uses_cursor_agent_only_when_agent_is_missing() {
    let root = tempfile::tempdir().unwrap();
    let path = std::env::join_paths([root.path()]).unwrap();
    assert_eq!(
        CursorAcpCommand::preferred_program(Some(path.as_os_str()), ""),
        "agent"
    );
    std::fs::write(root.path().join("cursor-agent"), "").unwrap();
    assert_eq!(
        CursorAcpCommand::preferred_program(Some(path.as_os_str()), ""),
        "cursor-agent"
    );
    std::fs::write(root.path().join("agent"), "").unwrap();
    assert_eq!(
        CursorAcpCommand::preferred_program(Some(path.as_os_str()), ""),
        "agent"
    );
}

fn snapshot(model: &str, choices: &[&str], effort: &[&str], current_effort: &str) -> Value {
    json!({"configOptions": [
        {"id": "model-selector", "category": "model", "currentValue": model,
         "options": [{"group": "Future family", "options": choices.iter().map(|id| json!({"value": id, "name": id})).collect::<Vec<_>>() }]},
        {"id": "thinking-selector", "category": "thought_level", "currentValue": current_effort,
         "options": effort.iter().map(|id| json!({"value": id, "name": id})).collect::<Vec<_>>()}
    ]})
}

#[test]
fn grouped_snapshots_replace_obsolete_models_and_preserve_unrelated_updates() {
    let mut catalog = ModelCatalog::default();
    catalog.merge(&snapshot(
        "arbitrary-future-2099",
        &["arbitrary-future-2099", "obsolete"],
        &["low", "cosmic"],
        "cosmic",
    ));
    assert_eq!(catalog.models, ["arbitrary-future-2099", "obsolete"]);
    assert_eq!(
        catalog.effort.as_ref().unwrap().current.as_deref(),
        Some("cosmic")
    );
    catalog.merge(&snapshot(
        "arbitrary-future-2100",
        &["arbitrary-future-2100"],
        &["low"],
        "low",
    ));
    assert_eq!(catalog.models, ["arbitrary-future-2100"]);
    assert_eq!(catalog.effort.as_ref().unwrap().choices, ["low"]);
    catalog.merge(&json!({"sessionUpdate":"agent_message_chunk", "content":{"type":"text","text":"unrelated"}}));
    catalog.merge(&json!({"configOptions": []}));
    catalog.merge(&json!({"configOptions": [{"id":"model-selector","category":"model","currentValue":17,"options":[{"value":"broken"}]}]}));
    assert_eq!(catalog.models, ["arbitrary-future-2100"]);
    assert_eq!(catalog.current.as_deref(), Some("arbitrary-future-2100"));
}

#[test]
fn scoped_cache_isolates_program_args_endpoint_and_workspace() {
    let root = tempfile::tempdir().unwrap();
    let command = CursorAcpCommand::new("test-agent", ["acp"]);
    let same = shared_catalog(&command, root.path(), Some("endpoint-a".into()));
    assert!(Arc::ptr_eq(
        &same,
        &shared_catalog(&command, root.path(), Some("endpoint-a".into()))
    ));
    for other in [
        shared_catalog(&command, root.path(), Some("endpoint-b".into())),
        shared_catalog(
            &CursorAcpCommand::new("other-agent", ["acp"]),
            root.path(),
            Some("endpoint-a".into()),
        ),
        shared_catalog(
            &CursorAcpCommand::new("test-agent", ["--other", "acp"]),
            root.path(),
            Some("endpoint-a".into()),
        ),
        shared_catalog(
            &command,
            &root.path().join("other"),
            Some("endpoint-a".into()),
        ),
    ] {
        assert!(!Arc::ptr_eq(&same, &other));
    }
}

#[test]
fn published_state_keeps_requested_model_separate_and_filters_efforts() {
    let root = tempfile::tempdir().unwrap();
    let provider = CursorAcpProvider::with_command(CursorAcpCommand::new(
        "sh",
        [root.path().to_str().unwrap()],
    ));
    let mut catalog = ModelCatalog::default();
    catalog.merge(&snapshot(
        "arbitrary-future-2099",
        &["arbitrary-future-2099", "other"],
        &["low", "cosmic"],
        "cosmic",
    ));
    provider.published().publish(&catalog, true);
    provider.set_model("cursor-acp:other").unwrap();
    assert_eq!(provider.model(), "other");
    assert_eq!(
        provider.model.read().unwrap().as_deref(),
        Some("arbitrary-future-2099")
    );
    assert_eq!(provider.reasoning_effort().as_deref(), Some("cosmic"));
    assert_eq!(provider.available_efforts(), ["low"]);
    provider.set_reasoning_effort("cosmic").unwrap();
    assert!(provider.set_reasoning_effort("high").is_err());
    assert_eq!(
        provider.requested_effort.read().unwrap().as_deref(),
        Some("cosmic")
    );
    assert_eq!(provider.model_routes()[0].model, "arbitrary-future-2099");
    let fork = provider.fork();
    assert_eq!(fork.model(), "other");
    assert_eq!(fork.reasoning_effort().as_deref(), Some("cosmic"));
    assert!(provider.session.try_lock().unwrap().is_none());
    assert_eq!(
        fork.available_models_for_switching(),
        provider.available_models_for_switching()
    );
    assert!(fork.supports_image_input());
    provider.requested_effort.write().unwrap().take();
    catalog.merge(&snapshot("other", &["other"], &["medium"], "medium"));
    provider.published().publish(&catalog, false);
    assert_eq!(provider.model(), "other");
    assert_eq!(provider.reasoning_effort().as_deref(), Some("medium"));
    assert_eq!(provider.available_efforts(), ["medium"]);
    assert_eq!(fork.available_models_for_switching(), ["other"]);
    assert!(!fork.supports_image_input());
}

#[test]
fn incremental_tail_retains_new_reminder_without_replaying_assistant_mirror() {
    let first = Message::user("first");
    let reminder = Message::user("<system-reminder>new skill instructions</system-reminder>");
    assert!(reminder.is_internal_system_reminder());
    let history = vec![
        first.clone(),
        Message::assistant_text("answer"),
        reminder,
        Message::user("second"),
    ];
    let covered = sent_prefix_len(&history, &fingerprint_all(&[first])).unwrap();
    let tail = &history[covered..];
    let payload = build_prompt("", &tail[new_turn_start(tail).unwrap()..]);
    let text = payload[0]["text"].as_str().unwrap();
    assert!(text.contains("new skill instructions"));
    assert!(text.contains("second"));
    assert!(!text.contains("answer"));
    assert!(!text.contains("first"));
}
