use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

fn preview() -> LoadedSessionPreview {
    LoadedSessionPreview {
        messages: Vec::new(),
        extras: PreviewExtras {
            hidden_prompts: 7,
            todos_payload: Some("selected todos".into()),
            info_data: Some(InfoWidgetData {
                working_dir: Some("/selected/workspace".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
    }
}

#[test]
fn transcript_arrives_while_workspace_probe_is_still_blocked() {
    let (tx, rx) = mpsc::channel();
    let (workspace_tx, workspace_rx) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        publish_then_hydrate(Some(preview()), tx, workspace_tx, |data| {
            blocked.recv_timeout(Duration::from_secs(10)).unwrap();
            data.reasoning_effort = Some("hydrated later".into());
        });
    });
    // A rendezvous, not a sleep-based timing assertion: the transcript must be
    // delivered before the test allows any filesystem/SSH work to complete.
    let loaded = rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    assert_eq!(loaded.extras.hidden_prompts, 7);
    assert_eq!(
        loaded.extras.todos_payload.as_deref(),
        Some("selected todos")
    );
    assert_eq!(
        loaded
            .extras
            .info_data
            .as_ref()
            .unwrap()
            .working_dir
            .as_deref(),
        Some("/selected/workspace")
    );
    assert!(matches!(
        workspace_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    release.send(()).unwrap();
    let (generation, data) = workspace_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(Some(generation), loaded.extras.loaded_at);
    assert_eq!(data.reasoning_effort.as_deref(), Some("hydrated later"));
    worker.join().unwrap();
}

#[test]
fn abandoned_or_failed_transcript_does_not_probe_workspace() {
    for failed in [false, true] {
        let (tx, rx) = mpsc::channel();
        let (workspace_tx, workspace_rx) = mpsc::channel();
        let called = Arc::new(AtomicBool::new(false));
        let probe = called.clone();
        if !failed {
            drop(rx);
        }
        publish_then_hydrate(
            if failed { None } else { Some(preview()) },
            tx,
            workspace_tx,
            move |_| {
                probe.store(true, Ordering::SeqCst);
            },
        );
        assert!(!called.load(Ordering::SeqCst));
        assert!(workspace_rx.try_recv().is_err());
    }
}

#[test]
fn workspace_updates_only_matching_preview_generation() {
    for stale in [false, true] {
        let mut picker = SessionPicker::new(Vec::new());
        let generation = Instant::now();
        let mut extras = preview().extras;
        extras.loaded_at = Some(if stale {
            generation - Duration::from_secs(1)
        } else {
            generation
        });
        picker.preview_extras.insert("selected".into(), extras);
        let (tx, rx) = mpsc::channel();
        picker.pending_workspace_load = Some(PendingWorkspaceLoad {
            session_id: "selected".into(),
            receiver: rx,
        });
        let data = InfoWidgetData {
            reasoning_effort: Some("new workspace".into()),
            ..Default::default()
        };
        tx.send((generation, data)).unwrap();
        assert_eq!(picker.poll_workspace_load(), !stale);
        assert_eq!(
            picker.preview_extras["selected"]
                .info_data
                .as_ref()
                .unwrap()
                .reasoning_effort
                .is_some(),
            !stale
        );
        assert!(picker.pending_workspace_load.is_none());
    }
}

#[test]
fn refreshed_preview_without_extras_rejects_old_workspace() {
    let mut picker = SessionPicker::new(Vec::new());
    let (tx, rx) = mpsc::channel();
    picker.pending_workspace_load = Some(PendingWorkspaceLoad {
        session_id: "selected".into(),
        receiver: rx,
    });
    tx.send((Instant::now(), InfoWidgetData::default()))
        .unwrap();
    assert!(!picker.poll_workspace_load());
    assert!(picker.preview_extras.is_empty());
}

#[test]
fn polling_is_fast_only_while_preview_work_is_pending() {
    let mut picker = SessionPicker::new(Vec::new());
    assert_eq!(picker.preview_poll_interval(), Duration::from_millis(100));
    let (tx, rx) = mpsc::channel();
    picker.pending_workspace_load = Some(PendingWorkspaceLoad {
        session_id: "selected".into(),
        receiver: rx,
    });
    assert_eq!(picker.preview_poll_interval(), Duration::from_millis(16));
    assert!(!picker.poll_workspace_load());
    assert!(picker.pending_workspace_load.is_some());
    drop(tx);
    assert!(!picker.poll_workspace_load());
    assert!(picker.pending_workspace_load.is_none());
    assert_eq!(picker.preview_poll_interval(), Duration::from_millis(100));
    let (_tx, rx) = mpsc::channel();
    picker.pending_preview_load = Some(PendingSessionPreviewLoad {
        session_id: "selected".into(),
        receiver: rx,
    });
    assert_eq!(picker.preview_poll_interval(), Duration::from_millis(16));
}
