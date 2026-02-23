use std::io;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
use tokio::sync::Mutex;

/// Convert a filesystem path to a Windows named pipe path.
///
/// e.g. `/run/user/1000/jcode.sock` -> `\\.\pipe\jcode`
/// e.g. `/run/user/1000/jcode/myserver.sock` -> `\\.\pipe\jcode-myserver`
fn path_to_pipe_name(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("jcode");
    format!(r"\\.\pipe\{}", stem)
}

/// Listener wraps a Windows named pipe server, providing an accept loop
/// that matches the UnixListener interface.
pub struct Listener {
    pipe_name: String,
    current_server: NamedPipeServer,
}

impl Listener {
    pub fn bind(path: &Path) -> io::Result<Self> {
        let pipe_name = path_to_pipe_name(path);
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)?;
        Ok(Self {
            pipe_name,
            current_server: server,
        })
    }

    pub async fn accept(&mut self) -> io::Result<(Stream, PipeAddr)> {
        self.current_server.connect().await?;

        let connected = std::mem::replace(
            &mut self.current_server,
            ServerOptions::new().create(&self.pipe_name)?,
        );

        Ok((Stream::Server(connected), PipeAddr))
    }
}

/// Placeholder for the "address" of a named pipe connection.
pub struct PipeAddr;

/// Stream wraps either a NamedPipeServer (accepted connection) or
/// NamedPipeClient (outgoing connection).
pub enum Stream {
    Server(NamedPipeServer),
    Client(NamedPipeClient),
}

impl Stream {
    pub async fn connect(path: impl AsRef<Path>) -> io::Result<Self> {
        let pipe_name = path_to_pipe_name(path.as_ref());
        loop {
            match ClientOptions::new().open(&pipe_name) {
                Ok(client) => return Ok(Stream::Client(client)),
                Err(e)
                    if e.raw_os_error()
                        == Some(windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub fn into_split(self) -> (ReadHalf, WriteHalf) {
        let shared = Arc::new(Mutex::new(self));
        (
            ReadHalf {
                inner: Arc::clone(&shared),
            },
            WriteHalf { inner: shared },
        )
    }

    pub fn split(&mut self) -> (SplitReadRef<'_>, SplitWriteRef<'_>) {
        (
            SplitReadRef { stream: self },
            SplitWriteRef { stream: self },
        )
    }

    pub fn pair() -> io::Result<(Self, Self)> {
        let pipe_name = format!(r"\\.\pipe\jcode-pair-{}", std::process::id());
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)?;
        let client = ClientOptions::new().open(&pipe_name)?;

        // We need to accept the connection on the server side
        // For pair(), the client is already connected since the pipe exists
        Ok((Stream::Server(server), Stream::Client(client)))
    }
}

impl AsyncRead for Stream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Server(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            Stream::Client(c) => std::pin::Pin::new(c).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        match self.get_mut() {
            Stream::Server(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            Stream::Client(c) => std::pin::Pin::new(c).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Server(s) => std::pin::Pin::new(s).poll_flush(cx),
            Stream::Client(c) => std::pin::Pin::new(c).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Server(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            Stream::Client(c) => std::pin::Pin::new(c).poll_shutdown(cx),
        }
    }
}

/// Owned read half of a Stream, created by `into_split()`.
/// Uses a shared Arc<Mutex<Stream>> since named pipes don't support native splitting.
pub struct ReadHalf {
    inner: Arc<Mutex<Stream>>,
}

impl AsyncRead for ReadHalf {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let mut guard = match self.inner.try_lock() {
            Ok(g) => g,
            Err(_) => return std::task::Poll::Pending,
        };
        std::pin::Pin::new(&mut *guard).poll_read(cx, buf)
    }
}

/// Owned write half of a Stream, created by `into_split()`.
pub struct WriteHalf {
    inner: Arc<Mutex<Stream>>,
}

impl AsyncWrite for WriteHalf {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        let mut guard = match self.inner.try_lock() {
            Ok(g) => g,
            Err(_) => return std::task::Poll::Pending,
        };
        std::pin::Pin::new(&mut *guard).poll_write(cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let mut guard = match self.inner.try_lock() {
            Ok(g) => g,
            Err(_) => return std::task::Poll::Pending,
        };
        std::pin::Pin::new(&mut *guard).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let mut guard = match self.inner.try_lock() {
            Ok(g) => g,
            Err(_) => return std::task::Poll::Pending,
        };
        std::pin::Pin::new(&mut *guard).poll_shutdown(cx)
    }
}

/// Borrowed read reference for `stream.split()`.
pub struct SplitReadRef<'a> {
    stream: &'a mut Stream,
}

impl<'a> AsyncRead for SplitReadRef<'a> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut *self.get_mut().stream).poll_read(cx, buf)
    }
}

/// Borrowed write reference for `stream.split()`.
pub struct SplitWriteRef<'a> {
    stream: &'a mut Stream,
}

impl<'a> AsyncWrite for SplitWriteRef<'a> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        std::pin::Pin::new(&mut *self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut *self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut *self.get_mut().stream).poll_shutdown(cx)
    }
}

/// Synchronous named pipe stream for blocking IPC (used by communicate tool).
pub struct SyncStream {
    handle: std::fs::File,
}

impl SyncStream {
    pub fn connect(path: &Path) -> io::Result<Self> {
        use std::fs::OpenOptions;
        let pipe_name = path_to_pipe_name(path);
        let file = OpenOptions::new().read(true).write(true).open(&pipe_name)?;
        Ok(Self { handle: file })
    }
}

impl io::Read for SyncStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.handle.read(buf)
    }
}

impl io::Write for SyncStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.handle.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.handle.flush()
    }
}

pub fn is_socket_path(path: &Path) -> bool {
    // Named pipes don't exist on the filesystem, so check
    // if connecting would succeed
    let pipe_name = path_to_pipe_name(path);
    std::fs::metadata(&pipe_name).is_ok()
}

pub fn remove_socket(_path: &Path) {
    // Named pipes are automatically cleaned up when all handles are closed.
    // No filesystem cleanup needed.
}
