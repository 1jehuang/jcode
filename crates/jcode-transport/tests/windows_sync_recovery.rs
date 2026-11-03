//! Native coverage for cancellation and recovery of blocking named-pipe clients.
#![cfg(windows)]

use jcode_transport::{Listener, Stream, SyncStream};
use std::future::Future;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

static NEXT_PIPE: AtomicU64 = AtomicU64::new(0);

struct Peer {
    path: PathBuf,
    done: mpsc::Receiver<()>,
    thread: std::thread::JoinHandle<()>,
}

impl Peer {
    fn start<F, Fut>(script: F) -> Self
    where
        F: FnOnce(Stream) -> Fut + Send + 'static,
        Fut: Future<Output = ()>,
    {
        let path = std::env::temp_dir().join(format!(
            "jcode-sync-recovery-{}-{}.sock",
            std::process::id(),
            NEXT_PIPE.fetch_add(1, Ordering::Relaxed)
        ));
        let server_path = path.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    let mut listener = Listener::bind(&server_path).unwrap();
                    ready_tx.send(()).unwrap();
                    tokio::time::timeout(Duration::from_secs(8), async move {
                        let (stream, _) = listener.accept().await.unwrap();
                        script(stream).await;
                    })
                    .await
                    .expect("peer script deadline");
                });
            done_tx.send(()).unwrap();
        });
        ready_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("peer ready");
        Self { path, done, thread }
    }

    fn finish(self) {
        self.done
            .recv_timeout(Duration::from_secs(10))
            .expect("peer completion");
        self.thread.join().expect("peer thread");
    }
}

// Keep even a broken blocking implementation from hanging the test process.
fn bounded_client(script: impl FnOnce() + Send + 'static) {
    let (done_tx, done_rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        script();
        done_tx.send(()).unwrap();
    });
    done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("client completion deadline");
    thread.join().expect("client thread");
}

#[test]
fn sync_stream_timeouts_cancel_only_the_read_and_allow_recovery() {
    let peer = Peer::start(|mut stream| async move {
        let mut request = [0; 4];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"ping");
        stream.write_all(b"pong").await.unwrap();
        // Keep the server handle alive until the client consumes the response.
        let mut closed = [0];
        assert_eq!(stream.read(&mut closed).await.unwrap(), 0);
    });
    let path = peer.path.clone();
    bounded_client(move || {
        let mut writer = SyncStream::connect(&path).unwrap();
        let mut reader = writer.try_clone().unwrap();
        assert_eq!(
            reader
                .set_read_timeout(Some(Duration::ZERO))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        reader
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        // Repeated cancellations must not poison the pipe or leave old reads
        // queued to consume bytes from the later successful response.
        for _ in 0..5 {
            let mut byte = [0];
            assert_eq!(
                reader.read(&mut byte).unwrap_err().kind(),
                io::ErrorKind::TimedOut
            );
        }
        reader.set_read_timeout(None).unwrap();
        writer.write_all(b"ping").unwrap();
        let mut response = [0; 4];
        reader.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"pong");
    });
    peer.finish();
}

#[test]
fn sync_stream_shutdown_interrupts_pending_read_and_affects_all_clones() {
    let peer = Peer::start(|mut stream| async move {
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
    });
    let path = peer.path.clone();
    bounded_client(move || {
        let mut control = SyncStream::connect(&path).unwrap();
        let mut reader = control.try_clone().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (read_tx, read_rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            read_tx.send(reader.read(&mut [0])).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        std::thread::sleep(Duration::from_millis(250));
        control.shutdown(Shutdown::Both).unwrap();
        let result = read_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("shutdown must wake pending read");
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
        thread.join().unwrap();
        assert_eq!(control.read(&mut [0]).unwrap(), 0);
        assert_eq!(
            control.write(b"x").unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        control
            .shutdown(Shutdown::Both)
            .expect("shutdown is idempotent");
    });
    peer.finish();
}

#[test]
fn sync_stream_partial_reads_then_peer_close_return_eof() {
    let peer = Peer::start(|mut stream| async move {
        stream.write_all(b"abc").await.unwrap();
        let mut ack = [0];
        stream.read_exact(&mut ack).await.unwrap();
        assert_eq!(&ack, b"!");
    });
    let path = peer.path.clone();
    bounded_client(move || {
        let mut stream = SyncStream::connect(&path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = [0; 2];
        stream.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"ab");
        let mut last = [0; 1];
        stream.read_exact(&mut last).unwrap();
        assert_eq!(&last, b"c");
        stream.write_all(b"!").unwrap();
        assert_eq!(stream.read(&mut last).unwrap(), 0);
    });
    peer.finish();
}
