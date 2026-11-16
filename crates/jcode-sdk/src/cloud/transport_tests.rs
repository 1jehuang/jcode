//! Real loopback HTTP coverage for control-plane transport reuse.
use super::*;
use std::io::{Read, Write};

fn serve(count: usize, connect_delay: Duration) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let thread = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut workers = Vec::new();
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        while requests.load(std::sync::atomic::Ordering::SeqCst) < count {
            assert!(Instant::now() < deadline, "fixture missing HTTP request");
            let (mut socket, _) = match listener.accept() {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            let requests = requests.clone();
            workers.push(std::thread::spawn(move || {
            socket.set_nodelay(true).unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            std::thread::sleep(connect_delay);
            let mut headers_on_connection = String::new();
            while requests.load(std::sync::atomic::Ordering::SeqCst) < count {
                let mut bytes = Vec::new();
                while !bytes.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    match socket.read(&mut byte) {
                        Ok(0) => break,
                        Ok(_) => bytes.push(byte[0]),
                        Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => break,
                        Err(error) => panic!("{error}"),
                    }
                }
                if bytes.is_empty() { break; }
                assert!(bytes.ends_with(b"\r\n\r\n"));
                let headers = String::from_utf8(bytes).unwrap();
                let length = headers.lines().find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                }).unwrap_or(0);
                let mut body = vec![0; length];
                socket.read_exact(&mut body).unwrap();
                headers_on_connection.push_str(&headers);
                let body = br#"{"ok":true}"#;
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len()).unwrap();
                socket.write_all(body).unwrap();
                socket.flush().unwrap();
                requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            headers_on_connection
            }));
        }
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect()
    });
    (base, thread)
}

#[test]
fn control_plane_reuses_socket_without_retaining_authorization() {
    let (base, server) = serve(4, Duration::ZERO);
    for key in [
        "first-fixture-key",
        "second-fixture-key",
        "second-fixture-key",
        "first-fixture-key",
    ] {
        // The production mobile path constructs a client for each operation.
        let cloud = CloudClient::new("socket-reuse-fixture").with_api_base(&base);
        assert_eq!(
            cloud
                .send_raw("POST", "/host/connect", Some(key), serde_json::json!({}))
                .unwrap()
                .0,
            200
        );
    }
    let connections = server.join().unwrap();
    assert_eq!(
        connections.len(),
        1,
        "do not repeat connection setup for every wake poll"
    );
    let requests: Vec<_> = connections[0].split("POST /host/connect").skip(1).collect();
    for (request, key) in requests.iter().zip([
        "first-fixture-key",
        "second-fixture-key",
        "second-fixture-key",
        "first-fixture-key",
    ]) {
        assert!(
            request
                .to_ascii_lowercase()
                .contains(&format!("authorization: bearer {key}\r\n"))
        );
    }
}

#[test]
fn control_plane_pool_routes_distinct_origins_and_configurations() {
    let (a, first) = serve(2, Duration::ZERO);
    let (b, second) = serve(2, Duration::ZERO);
    for base in [&a, &b, &a, &b] {
        CloudClient::new("origin-fixture")
            .with_api_base(base)
            .send_raw(
                "GET",
                "/auth/verify",
                Some("fixture"),
                serde_json::Value::Null,
            )
            .unwrap();
    }
    assert_eq!(first.join().unwrap().len(), 1);
    assert_eq!(second.join().unwrap().len(), 1);
    let (base, server) = serve(2, Duration::ZERO);
    let mut cloud = CloudClient::new("configuration-fixture").with_api_base(base);
    cloud
        .send_raw("GET", "/auth/verify", None, serde_json::Value::Null)
        .unwrap();
    cloud.timeout = Duration::from_secs(7);
    cloud.client_name = "changed-configuration-fixture".into();
    cloud
        .send_raw("GET", "/auth/verify", None, serde_json::Value::Null)
        .unwrap();
    let connections = server.join().unwrap();
    assert_eq!(
        connections.len(),
        2,
        "changed timeout/name must select a new configured agent"
    );
    assert!(connections[1].contains("changed-configuration-fixture jcode-sdk-rs/"));
}

#[test]
#[ignore = "controlled connection setup benchmark, not production/device latency"]
fn benchmark_control_plane_connection_reuse() {
    for pooled in [false, true, true, false] {
        let (base, server) = serve(8, Duration::from_millis(100));
        let started = Instant::now();
        for n in 0..8 {
            let name = if pooled {
                "pooled-benchmark".into()
            } else {
                format!("fresh-benchmark-{n}")
            };
            let cloud = CloudClient::new(name).with_api_base(&base);
            cloud
                .send_raw(
                    "POST",
                    "/host/connect",
                    Some("fixture"),
                    serde_json::json!({}),
                )
                .unwrap();
        }
        let elapsed = started.elapsed();
        let connections = server.join().unwrap();
        eprintln!(
            "control_plane pooled={pooled} requests=8 connections={} elapsed_ms={:.3} injected_connect_ms=100",
            connections.len(),
            elapsed.as_secs_f64() * 1000.
        );
        assert_eq!(connections.len(), if pooled { 1 } else { 8 });
    }
}

#[test]
fn control_plane_changed_timeout_is_honored() {
    let (base, first) = serve(1, Duration::ZERO);
    let mut cloud = CloudClient::new("timeout-isolation-fixture").with_api_base(base);
    cloud.timeout = Duration::from_secs(2);
    cloud
        .send_raw("GET", "/auth/verify", None, serde_json::Value::Null)
        .unwrap();
    first.join().unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    cloud.api_base = format!("http://{}", listener.local_addr().unwrap());
    cloud.timeout = Duration::from_millis(20);
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            if socket.read(&mut byte).unwrap_or(0) == 0 {
                return;
            }
            bytes.extend_from_slice(&byte);
        }
        std::thread::sleep(Duration::from_millis(100));
        // A timed-out client may already have closed the connection.
        let _ = socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
    });
    assert!(
        cloud
            .send_raw("GET", "/auth/verify", None, serde_json::Value::Null)
            .is_err()
    );
    server.join().unwrap();
}
