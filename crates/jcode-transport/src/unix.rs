pub use tokio::net::UnixListener as Listener;
pub use tokio::net::UnixStream as Stream;
pub use tokio::net::unix::OwnedReadHalf as ReadHalf;
pub use tokio::net::unix::OwnedWriteHalf as WriteHalf;

pub use std::os::unix::net::UnixStream as SyncStream;

pub fn is_socket_path(path: &std::path::Path) -> bool {
    path.exists()
}

pub fn remove_socket(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

/// Create a connected pair of UnixStreams (for in-process bridging).
pub fn stream_pair() -> std::io::Result<(Stream, Stream)> {
    Stream::pair()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// The Unix side had no tests at all, so the API the bridge depends on was
    /// only ever checked indirectly. These pin the same shape the Windows
    /// implementation is checked against, so a future change to either cannot
    /// quietly diverge in what callers can rely on.
    #[tokio::test]
    async fn a_listener_accepts_and_round_trips_bytes() {
        let dir = std::env::temp_dir().join(format!("jcode-transport-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("round-trip.sock");
        remove_socket(&path);

        let listener = Listener::bind(&path).expect("bind");
        assert!(is_socket_path(&path), "a bound socket path should exist");

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 5];
            stream.read_exact(&mut buf).await.expect("read");
            stream.write_all(b"pong\n").await.expect("write");
            buf
        });

        let mut client = Stream::connect(&path).await.expect("connect");
        client.write_all(b"ping\n").await.expect("write");
        let mut reply = [0u8; 5];
        client.read_exact(&mut reply).await.expect("read");

        assert_eq!(&server.await.expect("server task"), b"ping\n");
        assert_eq!(&reply, b"pong\n");

        remove_socket(&path);
        assert!(
            !is_socket_path(&path),
            "remove_socket should unlink the path"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_stream_pair_is_connected_in_both_directions() {
        let (mut a, mut b) = stream_pair().expect("pair");
        a.write_all(b"ab").await.expect("write a");
        let mut seen = [0u8; 2];
        b.read_exact(&mut seen).await.expect("read b");
        assert_eq!(&seen, b"ab");

        b.write_all(b"ba").await.expect("write b");
        a.read_exact(&mut seen).await.expect("read a");
        assert_eq!(&seen, b"ba");
    }

    #[test]
    fn an_absent_path_is_not_a_socket() {
        assert!(!is_socket_path(std::path::Path::new(
            "/nonexistent/jcode-transport-absent.sock"
        )));
    }

    /// `remove_socket` is called on paths that may not exist, so it must not
    /// panic or report failure.
    #[test]
    fn removing_an_absent_socket_is_a_no_op() {
        remove_socket(std::path::Path::new(
            "/nonexistent/jcode-transport-absent.sock",
        ));
    }
}
