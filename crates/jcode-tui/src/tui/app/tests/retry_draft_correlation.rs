type RetryDraftSnapshot = (super::input_images::InputUndoSnapshot, String, String);
const RETRY_DRAFT_PASTE: &str = "first\nsecond\nthird\nfourth\nfifth\nsixth";

fn retry_draft_test_app() -> App {
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = super::AppRuntimeMode::RemoteClient;
    app.auto_poke_incomplete_todos = false;
    app
}

async fn retry_draft_submit(
    app: &mut App,
    remote: &mut crate::tui::backend::RemoteConnection,
) -> RetryDraftSnapshot {
    super::input_images::attach_image(app, "image/png".into(), "retry-image-a".into());
    super::input::insert_input_text(app, " !echo literal [image 1] and [Paste #99: 6 lines] ");
    super::input::handle_text_paste(app, RETRY_DRAFT_PASTE.into());
    super::input::insert_input_text(app, " tail ");
    super::input_images::attach_image(app, "image/png".into(), "retry-image-b".into());
    assert_eq!(app.pasted_contents, vec![RETRY_DRAFT_PASTE]);
    let snapshot = app.input_draft_snapshot();
    let prepared = super::input::take_prepared_input(app);
    let expected = (
        snapshot,
        prepared.expanded.clone(),
        prepared.routing_input().to_string(),
    );
    assert!(expected.1.contains(RETRY_DRAFT_PASTE));
    assert!(expected.1.contains("literal [image 1]"));
    super::remote::input_dispatch::submit_prepared_remote_input(app, remote, prepared)
        .await
        .unwrap();
    expected
}

fn retry_draft_rate_error(app: &mut App, remote: &mut crate::tui::backend::RemoteConnection) {
    let id = app.current_message_id.expect("a real submitted request");
    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id,
            message: "429 rate_limit_exceeded: retry later".into(),
            retry_after_secs: Some(1),
        },
        remote,
    );
}

async fn retry_draft_dispatch(app: &mut App, remote: &mut crate::tui::backend::RemoteConnection) {
    assert!(
        app.rate_limit_reset.is_some(),
        "controller must schedule the retry"
    );
    assert!(!app.is_processing);
    app.rate_limit_reset = Some(Instant::now() - Duration::from_secs(1));
    assert!(super::remote::dispatch_due_remote_retry(app, remote).await);
}

fn retry_draft_assert_restored(app: &mut App, expected: &RetryDraftSnapshot) {
    assert_eq!(
        app.input_draft_snapshot(),
        expected.0,
        "terminal recovery lost the complete submitted draft"
    );
    assert!(app.rate_limit_pending_message.is_none());
    assert!(app.rate_limit_reset.is_none());
    app.cursor_pos = app.input.len();
    app.handle_key(KeyCode::Backspace, KeyModifiers::NONE)
        .unwrap();
    assert_eq!(app.pending_images.len(), 1);
    app.undo_input_change();
    assert_eq!(
        app.input_draft_snapshot(),
        expected.0,
        "undo must restore image and paste ownership"
    );
    let prepared = super::input::take_prepared_input(app);
    assert_eq!(prepared.expanded, expected.1);
    assert_eq!(prepared.routing_input(), expected.2);
    assert_eq!(prepared.images.as_slice(), expected.0.2.as_ref().as_slice());
    assert!(super::input::extract_input_shell_command(prepared.routing_input()).is_none());
}

#[test]
fn retry_draft_controller_429_repeated_retry_terminal_recovery_is_lossless() {
    with_temp_jcode_home(|| {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut app = tokio::task::block_in_place(retry_draft_test_app);
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            remote.mark_history_loaded();
            let peer = remote.take_dummy_peer().unwrap();
            let (reader, _writer) = peer.into_split();
            let mut reader = tokio::io::BufReader::new(reader);
            let expected = retry_draft_submit(&mut app, &mut remote).await;
            let initial = attachment_routing_read_request(&mut reader).await;
            assert_eq!(initial["type"], "message");
            assert_eq!(initial["content"], expected.1);
            for attempt in 1..=App::AUTO_RETRY_MAX_ATTEMPTS {
                retry_draft_rate_error(&mut app, &mut remote);
                assert_eq!(app.current_message_id, None);
                assert_eq!(
                    app.rate_limit_pending_message
                        .as_ref()
                        .unwrap()
                        .retry_attempts,
                    attempt
                );
                retry_draft_dispatch(&mut app, &mut remote).await;
                let request = attachment_routing_read_request(&mut reader).await;
                assert_eq!(request["type"], "message");
                assert_eq!(request["content"], expected.1);
                assert_eq!(request["images"], initial["images"]);
            }
            retry_draft_rate_error(&mut app, &mut remote);
            retry_draft_assert_restored(&mut app, &expected);
        });
    });
}

#[test]
fn retry_draft_held_reload_after_resend_keeps_exact_provenance() {
    with_temp_jcode_home(|| {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut app = tokio::task::block_in_place(retry_draft_test_app);
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            remote.mark_history_loaded();
            let expected = retry_draft_submit(&mut app, &mut remote).await;
            app.rate_limit_pending_message.as_mut().unwrap().auto_retry = true;
            retry_draft_rate_error(&mut app, &mut remote);
            retry_draft_dispatch(&mut app, &mut remote).await;
            retry_draft_rate_error(&mut app, &mut remote);
            app.save_input_for_reload("retry-draft-held");
            let restored = App::restore_input_for_reload("retry-draft-held").unwrap();
            let mut next = tokio::task::block_in_place(retry_draft_test_app);
            next.apply_restored_reload_input(restored);
            let mut connection = crate::tui::backend::RemoteConnection::dummy();
            connection.mark_history_loaded();
            retry_draft_dispatch(&mut next, &mut connection).await;
            retry_draft_rate_error(&mut next, &mut connection);
            retry_draft_dispatch(&mut next, &mut connection).await;
            retry_draft_rate_error(&mut next, &mut connection);
            retry_draft_assert_restored(&mut next, &expected);
        });
    });
}

#[test]
fn retry_draft_failed_retry_writes_preserve_original_and_newer_composer() {
    with_temp_jcode_home(|| {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            for newer in [false, true] {
                let mut app = tokio::task::block_in_place(retry_draft_test_app);
                let mut remote = crate::tui::backend::RemoteConnection::dummy();
                remote.mark_history_loaded();
                let expected = retry_draft_submit(&mut app, &mut remote).await;
                retry_draft_rate_error(&mut app, &mut remote);
                let newer_draft = if newer {
                    super::input::insert_input_text(&mut app, "newer literal [image 1] ");
                    super::input::handle_text_paste(
                        &mut app,
                        "new1\nnew2\nnew3\nnew4\nnew5\nnew6".into(),
                    );
                    super::input_images::attach_image(
                        &mut app,
                        "image/png".into(),
                        "newer-image".into(),
                    );
                    Some((
                        app.input_draft_snapshot(),
                        super::input::expand_paste_placeholders_with(
                            &app.pasted_contents,
                            &app.input_without_owned_image_markers(),
                        ),
                    ))
                } else {
                    None
                };
                drop(remote.take_dummy_peer());
                while app.rate_limit_pending_message.is_some() {
                    retry_draft_dispatch(&mut app, &mut remote).await;
                }
                if let Some((snapshot, expanded)) = newer_draft {
                    assert!(app.input.starts_with(&snapshot.0));
                    assert_eq!(app.cursor_pos, snapshot.1);
                    assert_eq!(&app.pasted_contents, snapshot.4.as_ref());
                    assert_eq!(
                        super::input::expand_paste_placeholders_with(
                            &app.pasted_contents,
                            &app.input_without_owned_image_markers()
                        ),
                        expanded
                    );
                    assert_eq!(app.pending_images[0].1, "newer-image");
                    assert_eq!(app.pending_images.len(), 3);
                    assert!(
                        app.display_messages()
                            .iter()
                            .any(|message| message.content.contains(RETRY_DRAFT_PASTE)),
                        "the recovered old prompt must show paste text, not a dead label"
                    );
                    app.cursor_pos = app.input.len();
                    let merged = app.input_draft_snapshot();
                    app.handle_key(KeyCode::Backspace, KeyModifiers::NONE)
                        .unwrap();
                    app.undo_input_change();
                    assert_eq!(app.input_draft_snapshot(), merged);
                } else {
                    retry_draft_assert_restored(&mut app, &expected);
                }
            }
        });
    });
}

#[test]
fn retry_draft_canceled_or_mismatched_requests_never_rebind_stale_payloads() {
    with_temp_jcode_home(|| {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            for canceled in [false, true] {
                let mut app = tokio::task::block_in_place(retry_draft_test_app);
                let mut remote = crate::tui::backend::RemoteConnection::dummy();
                remote.mark_history_loaded();
                retry_draft_submit(&mut app, &mut remote).await;
                if canceled {
                    app.handle_server_event(crate::protocol::ServerEvent::Interrupted, &mut remote);
                    assert!(app.rate_limit_pending_message.is_none());
                    assert!(app.last_submitted_draft.is_none());
                    assert!(!super::remote::dispatch_due_remote_retry(&mut app, &mut remote).await);
                } else {
                    let id = app.current_message_id.unwrap();
                    app.last_submitted_draft.as_mut().unwrap().0 = Some(id + 99);
                    retry_draft_rate_error(&mut app, &mut remote);
                    retry_draft_dispatch(&mut app, &mut remote).await;
                    assert!(
                        app.last_submitted_draft.is_none(),
                        "equal payload cannot admit a mismatched request"
                    );
                    super::input::insert_input_text(&mut app, "new ordinary intent");
                    let prepared = super::input::take_prepared_input(&mut app);
                    super::remote::input_dispatch::submit_prepared_remote_input(
                        &mut app,
                        &mut remote,
                        prepared,
                    )
                    .await
                    .unwrap();
                    let (_, draft) = app.last_submitted_draft.as_ref().unwrap();
                    assert_eq!(draft.raw_input, "new ordinary intent");
                    assert!(draft.images.is_empty());
                    assert!(draft.pasted_contents.is_empty());
                }
            }
        });
    });
}
