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
