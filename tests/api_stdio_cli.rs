#![cfg(unix)]
//! Native CLI boundary, using a fake daemon and fully isolated state/config.
use jcode_harness_api::{API_VERSION_MAJOR, ApiEvent, ApiRequest, ClientFrame, ServerFrame};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn exercise_stdio(close_daemon_first: bool) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let runtime = root.path().join("run");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&runtime).unwrap();
    let socket = runtime.join("daemon.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_jcode"))
            .current_dir(root.path())
            .env_clear()
            .env("HOME", root.path())
            .env("JCODE_HOME", &home)
            .env("JCODE_RUNTIME_DIR", &runtime)
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("JCODE_NO_TELEMETRY", "1")
            .env("JCODE_SOCKET", &socket)
            .env("JCODE_API_SOCKET", runtime.join("unused-api.sock"))
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("XDG_CACHE_HOME", root.path().join("cache"))
            .env("PATH", "/usr/bin:/bin")
            .args(["--no-update", "api", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut input = child.0.stdin.take().unwrap();
    let output = child.0.stdout.take().unwrap();
    let stderr = child.0.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let stdout_reader = std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let stderr_reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut text = String::new();
        BufReader::new(stderr).read_to_string(&mut text).unwrap();
        text
    });
    let send = |input: &mut std::process::ChildStdin, id, request| {
        writeln!(
            input,
            "{}",
            serde_json::to_string(&ClientFrame::new(id, request)).unwrap()
        )
        .unwrap();
        input.flush().unwrap();
    };
    send(
        &mut input,
        1,
        ApiRequest::Hello {
            min_version: API_VERSION_MAJOR,
            max_version: API_VERSION_MAJOR,
            client: "isolated-stdio-cli-test".into(),
        },
    );
    let hello = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("CLI hello deadline");
    let hello: ServerFrame =
        serde_json::from_str(&hello).expect("stdout must contain only API JSON");
    assert!(matches!(hello.event, ApiEvent::HelloOk { .. }));
    // The first connection is the daemon-readiness probe, the second is the
    // actual API bridge. Neither needs a real provider or a real session.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut connections = Vec::new();
    while connections.len() < 2 {
        match listener.accept() {
            Ok((stream, _)) => connections.push(stream),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "daemon dial deadline");
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("accept: {error}"),
        }
    }
    send(&mut input, 2, ApiRequest::Ping);
    let pong: ServerFrame =
        serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap();
    assert!(matches!(pong.event, ApiEvent::Pong));
    if close_daemon_first {
        drop(connections);
    } else {
        drop(input);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "stdio CLI did not exit when a peer closed"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    stdout_reader.join().unwrap();
    let stderr = stderr_reader.join().unwrap();
    assert!(status.success(), "CLI status {status}: {stderr}");
    assert!(
        !runtime.join("unused-api.sock").exists(),
        "stdio must not create an API listener"
    );
}

#[test]
fn api_stdio_cli_serves_protocol_and_exits_when_stdin_closes() {
    exercise_stdio(false);
}

#[test]
fn api_stdio_cli_exits_on_daemon_eof_even_with_stdin_open() {
    exercise_stdio(true);
}
