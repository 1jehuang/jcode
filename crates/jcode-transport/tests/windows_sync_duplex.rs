//! Native Windows regression for the SDK's reader-before-hello startup order.
#![cfg(windows)]

use jcode_transport::{Listener, SyncStream};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn sync_stream_reader_before_writer_does_not_deadlock() {
    let path = std::env::temp_dir().join(format!(
        "jcode-sync-duplex-{}-{}.sock",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let (ready_tx, ready_rx) = mpsc::channel();
    let (server_tx, server_rx) = mpsc::channel();
    let server_path = path.clone();
    let server = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("server runtime");
        let watchdog_fired = runtime.block_on(async move {
            let mut listener = Listener::bind(&server_path).expect("bind named pipe");
            ready_tx.send(()).unwrap();
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .expect("accept deadline")
                .expect("accept client");
            let mut request = [0; 4];
            let received =
                tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut request)).await;
            let watchdog_fired = received.is_err();
            if let Ok(result) = received {
                result.expect("read request");
                assert_eq!(&request, b"ping");
            }

            // A duplicated synchronous File handle serializes ReadFile and
            // WriteFile. On that broken implementation the request never
            // arrives. Send the response anyway to release the blocked reader
            // and allow the writer to finish, so a regression FAILS, not hangs.
            tokio::time::timeout(Duration::from_secs(5), stream.write_all(b"pong"))
                .await
                .expect("response deadline")
                .expect("write response");
            if watchdog_fired {
                eprintln!("duplex watchdog: no request arrived while client reader was blocked");
                tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut request))
                    .await
                    .expect("writer must recover after watchdog releases reader")
                    .expect("read recovered request");
                assert_eq!(&request, b"ping");
            }
            watchdog_fired
        });
        server_tx.send(watchdog_fired).unwrap();
    });
    ready_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("server ready");

    // Both halves deliberately run on plain threads, with no Tokio context,
    // matching the native SDK. Clone BEFORE starting the blocking read.
    let mut writer = SyncStream::connect(&path).expect("connect synchronous client");
    let mut reader = writer.try_clone().expect("clone synchronous client");
    let (reading_tx, reading_rx) = mpsc::channel();
    let (read_tx, read_rx) = mpsc::channel();
    let reader_thread = std::thread::spawn(move || {
        reading_tx.send(()).unwrap();
        let mut response = [0; 4];
        let result = reader.read_exact(&mut response).map(|()| response);
        read_tx.send(result).unwrap();
    });
    reading_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("reader started");
    // Let the reader enter the OS call before submitting the hello/request.
    // The peer sends nothing during this interval, so the read stays pending.
    std::thread::sleep(Duration::from_millis(250));
    let (write_tx, write_rx) = mpsc::channel();
    let writer_thread = std::thread::spawn(move || {
        write_tx.send(writer.write_all(b"ping")).unwrap();
    });

    // Never unconditionally join a blocked I/O thread. Every receive is
    // bounded, including cleanup of the intentionally broken baseline.
    write_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("writer deadline")
        .expect("write request");
    let response = read_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("reader deadline")
        .expect("read response");
    let watchdog_fired = server_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("server completion deadline");
    writer_thread.join().expect("writer thread");
    reader_thread.join().expect("reader thread");
    server.join().expect("server thread");
    assert_eq!(&response, b"pong");
    assert!(
        !watchdog_fired,
        "SyncStream duplex deadlock: a pending read prevented the cloned writer from sending the request"
    );
}
