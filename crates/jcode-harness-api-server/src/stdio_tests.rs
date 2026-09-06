//! Exercises the actual stream entry point without touching user's state/socket.
use super::*;
use jcode_harness_api::{ApiRequest, ClientFrame};
use tokio::io::{AsyncWriteExt, BufReader};

#[tokio::test(flavor = "multi_thread")]
async fn stdio_stream_handshake_ping_and_eof_release_daemon_connection() {
    let root = std::env::temp_dir().join(format!(
        "jcode-stdio-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let socket = root.join("daemon.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let (client, bridge) = tokio::io::duplex(8192);
    let (bridge_read, bridge_write) = tokio::io::split(bridge);
    let task = tokio::spawn(run_bridge_stream(bridge_read, bridge_write, socket));
    let (read, mut write) = tokio::io::split(client);
    let mut read = BufReader::new(read);
    write_json_line(
        &mut write,
        &ClientFrame::new(
            1,
            ApiRequest::Hello {
                min_version: API_VERSION_MAJOR,
                max_version: API_VERSION_MAJOR,
                client: "stdio-test".into(),
            },
        ),
    )
    .await
    .unwrap();
    let mut line = String::new();
    read_frame(&mut read, &mut line).await.unwrap();
    let hello: ServerFrame = serde_json::from_str(&line).unwrap();
    assert_eq!(hello.reply_to, Some(1));
    assert!(matches!(hello.event, ApiEvent::HelloOk { .. }));
    let (mut daemon, _) = listener.accept().await.unwrap();
    write_json_line(&mut write, &ClientFrame::new(2, ApiRequest::Ping))
        .await
        .unwrap();
    read_frame(&mut read, &mut line).await.unwrap();
    let pong: ServerFrame = serde_json::from_str(&line).unwrap();
    assert_eq!(pong.reply_to, Some(2));
    assert!(matches!(pong.event, ApiEvent::Pong));
    write.shutdown().await.unwrap();
    task.await.unwrap().unwrap();
    let mut buffer = [0; 1];
    assert_eq!(
        tokio::io::AsyncReadExt::read(&mut daemon, &mut buffer)
            .await
            .unwrap(),
        0
    );
    drop(listener);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn stdio_stream_rejects_bad_hello_without_dialing_daemon() {
    let (mut client, bridge) = tokio::io::duplex(8192);
    let (read, write) = tokio::io::split(bridge);
    let task = tokio::spawn(run_bridge_stream(
        read,
        write,
        PathBuf::from("/nonexistent-stdio-test.sock"),
    ));
    client.write_all(b"not JSON\n").await.unwrap();
    let mut reader = BufReader::new(client);
    let mut line = String::new();
    read_frame(&mut reader, &mut line).await.unwrap();
    let frame: ServerFrame = serde_json::from_str(&line).unwrap();
    assert!(matches!(
        frame.event,
        ApiEvent::Error {
            code: ErrorCode::InvalidRequest,
            ..
        }
    ));
    task.await.unwrap().unwrap();
}
