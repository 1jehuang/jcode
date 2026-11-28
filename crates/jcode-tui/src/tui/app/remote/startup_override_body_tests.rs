// A CLI-requested startup model (`jcode --model X` on an already-running
// server) is a session override, not a user-initiated switch.

/// The startup override must reach the server with persist_default=false so a
/// single flag run cannot rewrite the saved default for every later session on
/// a shared server. Bootstrap traffic sends several requests before this one,
/// so drain peer lines until the SetModel frame appears.
#[test]
fn handle_post_connect_startup_model_override_does_not_persist_default() {
    let _guard = crate::storage::lock_test_env();
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _enter = rt.enter();

    let mut app = create_test_app();
    app.is_remote = true;
    app.set_initial_model_spec("gpt-5.6-sol".to_string());

    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let peer = remote
        .take_dummy_peer()
        .expect("dummy remote should retain peer stream");
    let (peer_reader, _peer_writer) = peer.into_split();
    let mut peer_reader = tokio::io::BufReader::new(peer_reader);
    let mut state = RemoteRunState::default();

    let outcome = rt
        .block_on(handle_post_connect(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            Some("session_startup_model"),
        ))
        .expect("post connect should succeed");
    assert!(matches!(outcome, PostConnectOutcome::Ready));

    rt.block_on(async {
        use tokio::io::AsyncBufReadExt;
        let mut line = String::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            line.clear();
            match tokio::time::timeout_at(
                tokio::time::Instant::from_std(deadline),
                peer_reader.read_line(&mut line),
            )
            .await
            {
                Ok(Ok(0)) => panic!("peer closed before the SetModel frame arrived"),
                Ok(Ok(_)) => {}
                Ok(Err(error)) => panic!("failed reading peer request: {error}"),
                Err(_) => panic!("timed out waiting for the SetModel frame"),
            }
            if let Ok(request) = serde_json::from_str::<crate::protocol::Request>(&line) {
                if let crate::protocol::Request::SetModel {
                    persist_default,
                    model,
                    ..
                } = request
                {
                    assert_eq!(model, "gpt-5.6-sol");
                    assert!(
                        !persist_default,
                        "startup overrides must not rewrite the saved config default"
                    );
                    return;
                }
            }
        }
    });
}
