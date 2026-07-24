//! Reference harness API client.
//!
//! Connects to a harness API endpoint over a Unix socket, performs the
//! handshake, creates a session, sends one message, and prints streamed
//! events until the turn completes.
//!
//! Usage:
//!   cargo run -p jcode-harness-api --example harness_repl -- \
//!     [socket_path] [message]
//!
//! Until the server-side adapter lands (milestone 2), run with `--demo` to
//! exercise the client against an in-process scripted server:
//!   cargo run -p jcode-harness-api --example harness_repl -- --demo

use jcode_harness_api::{
    API_VERSION_MAJOR, ApiEvent, ApiRequest, HarnessClient, ServerFrame, write_frame,
};
use std::io::{BufRead, BufReader, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--demo") {
        run_demo();
        return;
    }
    let socket = args
        .first()
        .cloned()
        .unwrap_or_else(default_socket_path);
    let message = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "hello from the harness API reference client".to_string());
    let stream = std::os::unix::net::UnixStream::connect(&socket)
        .unwrap_or_else(|e| panic!("connect {socket}: {e}"));
    let reader = BufReader::new(stream.try_clone().expect("clone stream"));
    run_session(HarnessClient::new(reader, stream), &message);
}

fn default_socket_path() -> String {
    let home = std::env::var("HOME").expect("HOME not set");
    format!("{home}/.jcode/jcode-api.sock")
}

fn run_session<R: BufRead, W: Write>(mut client: HarnessClient<R, W>, message: &str) {
    let hello = client.hello("harness_repl/0.1").expect("handshake");
    print_event(&hello);

    client
        .send(ApiRequest::CreateSession { working_dir: None })
        .expect("create session");
    let session_id = loop {
        let frame = client.recv().expect("recv");
        print_event(&frame);
        if let ApiEvent::Attached { session } = &frame.event {
            break session.session_id.clone();
        }
    };

    client
        .send(ApiRequest::SendMessage {
            session_id: session_id.clone(),
            content: message.to_string(),
            images: vec![],
        })
        .expect("send message");

    loop {
        let frame = client.recv().expect("recv");
        print_event(&frame);
        if matches!(&frame.event, ApiEvent::TurnDone { session_id: s } if *s == session_id) {
            break;
        }
        if matches!(frame.event, ApiEvent::Error { .. }) {
            break;
        }
    }
}

fn print_event(frame: &ServerFrame) {
    match &frame.event {
        ApiEvent::TextDelta { text, .. } => {
            print!("{text}");
            let _ = std::io::stdout().flush();
        }
        ApiEvent::ReasoningDelta { .. } => {}
        other => println!("[event] {other:?}"),
    }
}

/// Scripted in-process server so the client flow can be exercised before the
/// real server adapter exists.
fn run_demo() {
    let (client_stream, server_stream) =
        std::os::unix::net::UnixStream::pair().expect("socketpair");

    let server = std::thread::spawn(move || {
        let mut reader = BufReader::new(server_stream.try_clone().expect("clone"));
        let mut writer = server_stream;
        let mut line = String::new();
        let mut reply = |frame: &ServerFrame| write_frame(&mut writer, frame).expect("write");
        while {
            line.clear();
            reader.read_line(&mut line).expect("read") > 0
        } {
            let req: serde_json::Value = serde_json::from_str(line.trim()).expect("json");
            let id = req["id"].as_u64().unwrap_or(0);
            match req["req"].as_str().unwrap_or("") {
                "hello" => reply(&ServerFrame::reply(
                    id,
                    ApiEvent::HelloOk {
                        version: API_VERSION_MAJOR,
                        server: "jcode-demo/0".into(),
                        capabilities: vec!["sessions".into()],
                    },
                )),
                "create_session" => reply(&ServerFrame::reply(
                    id,
                    ApiEvent::Attached {
                        session: jcode_harness_api::SessionInfo {
                            session_id: "demo-1".into(),
                            working_dir: None,
                            title: Some("demo".into()),
                            status: "idle".into(),
                        },
                    },
                )),
                "send_message" => {
                    for word in ["Hello ", "from ", "the ", "demo ", "server.\n"] {
                        reply(&ServerFrame::event(ApiEvent::TextDelta {
                            session_id: "demo-1".into(),
                            text: word.into(),
                        }));
                    }
                    reply(&ServerFrame::event(ApiEvent::TurnDone {
                        session_id: "demo-1".into(),
                    }));
                    return;
                }
                _ => reply(&ServerFrame::reply(id, ApiEvent::Ok)),
            }
        }
    });

    let reader = BufReader::new(client_stream.try_clone().expect("clone"));
    run_session(HarnessClient::new(reader, client_stream), "demo message");
    server.join().expect("server thread");
    println!("demo complete");
}
