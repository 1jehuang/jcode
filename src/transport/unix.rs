pub use tokio::net::UnixListener as Listener;
pub use tokio::net::UnixStream as Stream;
pub use tokio::net::unix::OwnedWriteHalf as WriteHalf;
pub use tokio::net::unix::OwnedReadHalf as ReadHalf;

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
