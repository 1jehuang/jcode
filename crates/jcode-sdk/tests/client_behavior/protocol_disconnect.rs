use super::*;

#[test]
fn malformed_frames_fail_pending_requests_and_preserve_protocol_reason() {
    for bytes in [b"not JSON\n".as_slice(), b"\xff\n".as_slice()] {
        let client = fake_harness(move |_, writer| {
            writer.write_all(bytes).unwrap();
            writer.flush().unwrap();
        });
        let events = client.events(None);
        assert!(client.disconnect_reason().is_none());
        let error = client
            .ping()
            .expect_err("malformed frame must fail the waiter");
        assert_eq!(error.kind, jcode_sdk::ErrorKind::Protocol);
        assert_eq!(error.code(), "protocol");
        assert_eq!(
            client.disconnect_reason().unwrap().kind,
            jcode_sdk::ErrorKind::Protocol
        );
        assert!(client.is_closed());
        assert!(events.next_timeout(Duration::from_secs(1)).is_none());
        assert_eq!(
            client.ping().unwrap_err().kind,
            jcode_sdk::ErrorKind::Protocol
        );
    }
}

/// When the harness goes away, in-flight requests fail instead of blocking on
/// a reply that can never arrive.
#[test]
fn a_lost_connection_fails_requests_in_flight() {
    // A server that reads the request and then hangs up without replying.
    let (ours, theirs) = UnixStream::pair().expect("socket pair");
    std::thread::spawn(move || {
        let mut reader = BufReader::new(theirs.try_clone().expect("clone"));
        let mut writer = theirs;
        let hello: ClientFrame = read_frame(&mut reader).expect("hello");
        write_frame(
            &mut writer,
            &ServerFrame {
                v: API_VERSION_MAJOR,
                reply_to: Some(hello.id),
                event: ApiEvent::HelloOk {
                    version: API_VERSION_MAJOR,
                    server: "fake/1.0".to_string(),
                    capabilities: vec![],
                },
            },
        )
        .expect("hello reply");
        let _: ClientFrame = read_frame(&mut reader).expect("request");
        // Drop both ends: the client's pending request must be failed.
    });

    let client = JcodeClient::connect_with(
        Box::new(PairTransport(ours)),
        ConnectOptions {
            request_timeout: Some(Duration::from_secs(5)),
            ensure_runtime: false,
            ..Default::default()
        },
    )
    .expect("handshake");
    let error = client
        .ping()
        .expect_err("a dropped harness must fail the request");
    assert_eq!(error.code(), "disconnected");
    assert_eq!(client.disconnect_reason().unwrap().code(), "disconnected");
}
