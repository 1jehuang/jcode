#![cfg(unix)]

use jcode_harness_api::{
    API_VERSION_MAJOR, ApiEvent, ApiRequest, ClientFrame, ServerFrame, read_frame, write_frame,
};
use jcode_sdk::{ConnectOptions, JcodeClient, SshConnectOptions, Transport};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

struct PairTransport(UnixStream);
impl Transport for PairTransport {
    fn split(
        self: Box<Self>,
    ) -> jcode_sdk::Result<(Box<dyn BufRead + Send>, Box<dyn Write + Send>)> {
        let writer = self.0.try_clone().unwrap();
        Ok((Box::new(BufReader::new(self.0)), Box::new(writer)))
    }
}

#[test]
fn user_response_capability_is_sent_only_when_client_opts_in() {
    assert!(!ConnectOptions::default().supports_user_response);
    assert!(!SshConnectOptions::new("test-host").supports_user_response);
    for enabled in [false, true] {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut reader = BufReader::new(theirs.try_clone().unwrap());
            let mut writer = theirs;
            let frame: ClientFrame = read_frame(&mut reader).unwrap();
            let ApiRequest::Hello {
                supports_user_response,
                ..
            } = frame.request
            else {
                panic!("first frame must be Hello")
            };
            assert_eq!(supports_user_response, enabled);
            write_frame(
                &mut writer,
                &ServerFrame::reply(
                    frame.id,
                    ApiEvent::HelloOk {
                        version: API_VERSION_MAJOR,
                        server: "capability-test".into(),
                        capabilities: vec![],
                    },
                ),
            )
            .unwrap();
        });
        let client = JcodeClient::connect_with(
            Box::new(PairTransport(ours)),
            ConnectOptions {
                supports_user_response: enabled,
                ensure_runtime: false,
                request_timeout: Some(Duration::from_secs(5)),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(client.server, "capability-test");
        server.join().unwrap();
    }
}
