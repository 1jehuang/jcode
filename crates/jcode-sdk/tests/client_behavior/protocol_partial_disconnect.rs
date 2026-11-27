use super::*;

#[test]
fn partial_frame_transport_eof_is_not_a_fatal_protocol_error() {
    let (ours, theirs) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        let mut reader = BufReader::new(theirs.try_clone().unwrap());
        let mut writer = theirs;
        let hello: ClientFrame = read_frame(&mut reader).unwrap();
        write_frame(
            &mut writer,
            &ServerFrame::reply(
                hello.id,
                ApiEvent::HelloOk {
                    version: API_VERSION_MAJOR,
                    server: "partial-frame-test".into(),
                    capabilities: vec![],
                },
            ),
        )
        .unwrap();
        let _: ClientFrame = read_frame(&mut reader).unwrap();
        writer
            .write_all(b"{\"v\":1,\"type\":\"history\",\"messages\":[")
            .unwrap();
        writer.flush().unwrap();
        // The network closes in the middle of a large history frame.
    });
    let client = JcodeClient::connect_with(
        Box::new(PairTransport(ours)),
        ConnectOptions {
            ensure_runtime: false,
            request_timeout: Some(Duration::from_secs(2)),
            ..Default::default()
        },
    )
    .unwrap();
    let error = client.ping().unwrap_err();
    assert!(matches!(
        error.kind,
        jcode_sdk::ErrorKind::Disconnected | jcode_sdk::ErrorKind::Transport
    ));
    assert_ne!(
        client.disconnect_reason().unwrap().kind,
        jcode_sdk::ErrorKind::Protocol
    );
    server.join().unwrap();
}
