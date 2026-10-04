use super::*;
use crate::protocol::ServerEvent;
use crate::server::{Client, Server};

// How many `ModelChanged` frames does a RESUME actually deliver?
//
// Two server sites emit the frame:
//   * handle_subscribe             client_session.rs:911   (added by 48de3e706)
//   * handle_resume_session        client_session.rs:1754  (pre-dating it)
//
// client_lifecycle.rs:1672-1750 is what ties them together: a `Subscribe` that
// carries an existing `target_session_id` calls `handle_resume_session` FIRST
// (client_lifecycle.rs:1686) and THEN `handle_subscribe` (client_lifecycle.rs:1732),
// so the single resume bootstrap the TUI performs at startup/reload runs BOTH
// emitters. This test measures the frames instead of inferring them:
//
//   A) the startup/reload bootstrap - one target-aware Subscribe, which is what
//      `RemoteConnection::connect_with_session` sends because it skips its
//      GetHistory when `target_session_id` is set (backend.rs:379-391).
//   B) an explicit `Request::ResumeSession`, which is what `/resume`, the
//      session picker and workspace navigation issue (app/remote.rs:178, :204).

fn frame_label(event: &ServerEvent) -> String {
    match event {
        ServerEvent::SessionId { .. } => "SessionId".to_string(),
        ServerEvent::History { .. } => "History".to_string(),
        ServerEvent::ModelChanged { model, .. } => format!("ModelChanged({model})"),
        ServerEvent::AvailableModelsUpdated { .. } => "AvailableModelsUpdated".to_string(),
        ServerEvent::McpStatus { .. } => "McpStatus".to_string(),
        ServerEvent::SwarmPlan { .. } => "SwarmPlan".to_string(),
        ServerEvent::ConnectionPhase { .. } => "ConnectionPhase".to_string(),
        ServerEvent::Ack { .. } => "Ack".to_string(),
        ServerEvent::Done { .. } => "Done".to_string(),
        ServerEvent::Error { message, .. } => format!("Error({message})"),
        other => format!("{other:?}"),
    }
}

struct FrameLog {
    labels: Vec<String>,
    model_changed: usize,
    history: usize,
}

async fn read_until_request_done(client: &mut Client, request_id: u64) -> FrameLog {
    let mut log = FrameLog {
        labels: Vec::new(),
        model_changed: 0,
        history: 0,
    };
    loop {
        let event = client
            .read_event()
            .await
            .expect("client should receive bootstrap frames");
        log.labels.push(frame_label(&event));
        match &event {
            ServerEvent::ModelChanged { .. } => log.model_changed += 1,
            ServerEvent::History { .. } => log.history += 1,
            ServerEvent::Error { message, .. } => panic!("bootstrap error: {message}"),
            ServerEvent::Done { id } if *id == request_id => break,
            _ => {}
        }
    }
    // handle_resume_session sends its ModelChanged AFTER the Done, so drain
    // anything that arrives shortly after the ack rather than stopping at it.
    let drained = tokio::time::timeout(Duration::from_millis(750), async {
        loop {
            let event = client
                .read_event()
                .await
                .expect("client should receive post-ack frames");
            log.labels.push(frame_label(&event));
            match &event {
                ServerEvent::ModelChanged { .. } => log.model_changed += 1,
                ServerEvent::History { .. } => log.history += 1,
                ServerEvent::Error { message, .. } => panic!("post-ack error: {message}"),
                _ => {}
            }
        }
    })
    .await;
    if drained.is_err() {
        // Idle gap: no further bootstrap frames, which is the expected end.
    }
    log
}

#[test]
fn resume_bootstrap_delivers_exactly_one_model_changed_frame() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("isolated server directory");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _runtime_dir = EnvVarGuard::set_path("JCODE_RUNTIME_DIR", temp.path());
    let socket = temp.path().join("resume-modelchanged.sock");
    let _socket = EnvVarGuard::set_path("JCODE_SOCKET", &socket);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(30), async {
            let provider: Arc<dyn Provider> = Arc::new(StreamingTestProvider::default());
            let server = Server::new_with_paths(
                provider,
                socket.clone(),
                temp.path().join("resume-modelchanged-debug.sock"),
            );
            let server_task = tokio::spawn(async move { server.run().await });

            let mut session = Session::create(None, Some("modelchanged resume probe".to_string()));
            session.working_dir = Some(temp.path().display().to_string());
            session.save().expect("save resume target session");

            let connect = || async {
                loop {
                    if let Ok(client) = Client::connect_with_path(socket.clone()).await {
                        break client;
                    }
                    assert!(
                        !server_task.is_finished(),
                        "isolated server exited at startup"
                    );
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            };

            // --- A: the startup/reload resume bootstrap ---
            let mut launched = connect().await;
            let subscribe_id = launched
                .subscribe_with_info(
                    Some(temp.path().display().to_string()),
                    Some(false),
                    Some(session.id.clone()),
                    false,
                    false,
                )
                .await
                .expect("target-aware subscribe");
            let startup = read_until_request_done(&mut launched, subscribe_id).await;
            println!(
                "[startup resume bootstrap] frames={:?} model_changed={} history={}",
                startup.labels, startup.model_changed, startup.history
            );
            assert_eq!(
                startup.model_changed,
                2,
                "MEASURED: one target-aware Subscribe runs handle_resume_session AND \
                 handle_subscribe, so a startup/reload resume receives the route report \
                 TWICE (client_lifecycle.rs:1686 then :1732). frames={:?}",
                startup.labels
            );
            assert_eq!(
                startup.history,
                1,
                "the target-aware Subscribe also carries the History payload, which is why \
                 connect_with_session skips GetHistory. frames={:?}",
                startup.labels
            );
            drop(launched);

            // --- B: an explicit Request::ResumeSession (/resume, picker, workspace) ---
            let mut switcher = connect().await;
            // An in-session resume only happens on an already-subscribed
            // connection (the server rejects a stateful request otherwise), so
            // subscribe first and discard that bootstrap.
            let plain_subscribe_id = switcher
                .subscribe_with_info(
                    Some(temp.path().display().to_string()),
                    Some(false),
                    None,
                    false,
                    false,
                )
                .await
                .expect("plain subscribe");
            let _plain = read_until_request_done(&mut switcher, plain_subscribe_id).await;

            let resume_id = switcher
                .resume_session(&session.id)
                .await
                .expect("resume request should send");
            let switched = read_until_request_done(&mut switcher, resume_id).await;
            println!(
                "[explicit ResumeSession] frames={:?} model_changed={} history={}",
                switched.labels, switched.model_changed, switched.history
            );
            // The startup path is the doubled one; the explicit-resume path is
            // not. So the "two spurious notices on resume" the change silences
            // were real, and they were specific to a startup/reload attach.
            assert_eq!(
                switched.model_changed,
                1,
                "an explicit ResumeSession runs only handle_resume_session, so it reports \
                 the route once. frames={:?}",
                switched.labels
            );
            assert_eq!(
                switched.history,
                1,
                "an explicit resume carries its own History payload. frames={:?}",
                switched.labels
            );
            assert!(
                startup.model_changed > switched.model_changed,
                "the startup/reload bootstrap is the one that reports the route twice \
                 (startup={} explicit={})",
                startup.model_changed,
                switched.model_changed
            );

            drop(switcher);
            server_task.abort();
            let _ = server_task.await;
        })
        .await
        .expect("isolated resume bootstrap test deadline");
    });
}