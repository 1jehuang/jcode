//! Context cap controls over the public SDK transport.
#![cfg(unix)]

use jcode_harness_api::{
    read_frame, write_frame, ApiEvent, ApiRequest, ClientFrame, ErrorCode, ServerFrame,
};
use jcode_sdk::{ConnectOptions, ErrorKind, JcodeClient, Transport};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

struct PairTransport(UnixStream);
impl Transport for PairTransport {
    fn split(
        self: Box<Self>,
    ) -> jcode_sdk::Result<(Box<dyn BufRead + Send>, Box<dyn Write + Send>)> {
        Ok((
            Box::new(BufReader::new(self.0.try_clone().unwrap())),
            Box::new(self.0),
        ))
    }
}

#[test]
fn context_cap_returns_budget_clears_and_preserves_errors() {
    let (client_socket, server_socket) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        let mut reader = BufReader::new(server_socket.try_clone().unwrap());
        let mut writer = server_socket;
        let hello: ClientFrame = read_frame(&mut reader).unwrap();
        write_frame(
            &mut writer,
            &ServerFrame::reply(
                hello.id,
                ApiEvent::HelloOk {
                    version: 1,
                    server: "context-cap-test".into(),
                    capabilities: vec![],
                },
            ),
        )
        .unwrap();
        for expected in [Some(100_000), Some(0), None] {
            let frame: ClientFrame = read_frame(&mut reader).unwrap();
            let ApiRequest::SetContextCap {
                session_id,
                max_context_tokens,
            } = frame.request
            else {
                panic!("expected context cap request")
            };
            assert_eq!(session_id, "s1");
            assert_eq!(max_context_tokens, expected);
            write_frame(
                &mut writer,
                &ServerFrame::reply(
                    frame.id,
                    ApiEvent::ContextCapChanged {
                        session_id,
                        max_context_tokens: expected.filter(|n| *n != 0),
                        effective_budget: 90_000,
                    },
                ),
            )
            .unwrap();
        }
        let frame: ClientFrame = read_frame(&mut reader).unwrap();
        write_frame(
            &mut writer,
            &ServerFrame::reply(
                frame.id,
                ApiEvent::Error {
                    code: ErrorCode::UnknownSession,
                    message: "no such session".into(),
                },
            ),
        )
        .unwrap();
    });
    let client = JcodeClient::connect_with(
        Box::new(PairTransport(client_socket)),
        ConnectOptions {
            request_timeout: Some(Duration::from_secs(5)),
            ensure_runtime: false,
            ..Default::default()
        },
    )
    .unwrap();
    for cap in [Some(100_000), Some(0), None] {
        assert_eq!(client.set_context_cap("s1", cap).unwrap(), 90_000);
    }
    let error = client.set_context_cap("missing", None).unwrap_err();
    assert!(matches!(
        error.kind,
        ErrorKind::Harness(ErrorCode::UnknownSession)
    ));
    assert_eq!(error.message, "no such session");
    drop(client);
    server.join().unwrap();
}
