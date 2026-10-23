use sha2::{Digest, Sha256};
use std::io;
use std::path::Path;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};

/// Convert a filesystem path to a Windows named pipe path.
///
/// e.g. `/run/user/1000/jcode.sock` -> `\\.\pipe\jcode`
/// e.g. `/run/user/1000/jcode/myserver.sock` -> `\\.\pipe\jcode-myserver`
fn path_to_pipe_name(path: &Path) -> String {
    let stem: String = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("jcode")
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .take(32)
        .collect();
    let stem = if stem.is_empty() { "jcode" } else { &stem };
    let normalized = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    let hash = hex::encode(digest);
    format!(r"\\.\pipe\{}-{}", stem, &hash[..16])
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
        match ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
        {
            Ok(server) => Ok(Self {
                pipe_name,
                current_server: server,
            }),
            Err(e)
                if e.raw_os_error()
                    == Some(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED as i32) =>
            {
                eprintln!(
                    "[windows] Named pipe {} busy (access denied), retrying without first_pipe_instance",
                    pipe_name
                );
                std::thread::sleep(std::time::Duration::from_millis(200));
                let server = ServerOptions::new().create(&pipe_name)?;
                Ok(Self {
                    pipe_name,
                    current_server: server,
                })
            }
            Err(e) => Err(e),
        }
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
        let (read, write) = tokio::io::split(self);
        (ReadHalf { inner: read }, WriteHalf { inner: write })
    }

    pub fn split(&mut self) -> (SplitReadRef<'_>, SplitWriteRef<'_>) {
        let (read, write) = tokio::io::split(self);
        (SplitReadRef { inner: read }, SplitWriteRef { inner: write })
    }

    pub fn pair() -> io::Result<(Self, Self)> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static PAIR_COUNTER: AtomicU64 = AtomicU64::new(0);
        let counter = PAIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pipe_name = format!(r"\\.\pipe\jcode-pair-{}-{}", std::process::id(), counter);
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)?;
        let client = ClientOptions::new().open(&pipe_name)?;

        // The client connected when we opened it above, but the server must
        // call connect() to transition into the connected state.  For an
        // already-connected client this returns immediately.
        //
        // We use a short-lived runtime-free poll: since the client already
        // connected synchronously, the server's connect future will resolve
        // on the first poll.
        use std::future::Future;
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        fn dummy_raw_waker() -> RawWaker {
            fn no_op(_: *const ()) {}
            fn clone(p: *const ()) -> RawWaker {
                RawWaker::new(p, &VTABLE)
            }
            const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, no_op, no_op, no_op);
            RawWaker::new(std::ptr::null(), &VTABLE)
        }
        let waker = unsafe { Waker::from_raw(dummy_raw_waker()) };
        let mut cx = Context::from_waker(&waker);
        let mut fut = server.connect();
        let pinned = unsafe { std::pin::Pin::new_unchecked(&mut fut) };
        match pinned.poll(&mut cx) {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(e)) => return Err(e),
            Poll::Pending => {
                // Should not happen since the client already connected.
                // Drop the future and proceed - the pipe is still usable.
            }
        }
        drop(fut);

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
pub struct ReadHalf {
    inner: tokio::io::ReadHalf<Stream>,
}

impl AsyncRead for ReadHalf {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

/// Owned write half of a Stream, created by `into_split()`.
pub struct WriteHalf {
    inner: tokio::io::WriteHalf<Stream>,
}

impl AsyncWrite for WriteHalf {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// Borrowed read reference for `stream.split()`.
pub struct SplitReadRef<'a> {
    inner: tokio::io::ReadHalf<&'a mut Stream>,
}

impl<'a> AsyncRead for SplitReadRef<'a> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

/// Borrowed write reference for `stream.split()`.
pub struct SplitWriteRef<'a> {
    inner: tokio::io::WriteHalf<&'a mut Stream>,
}

impl<'a> AsyncWrite for SplitWriteRef<'a> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// Blocking named-pipe facade. The handle is overlapped so a blocked reader
/// never prevents a clone from writing (as synchronous duplicated handles do).
/// Each operation owns its event and OVERLAPPED until completion, including
/// after cancellation. No tokio runtime is required.
pub struct SyncStream {
    inner: std::sync::Arc<SyncPipe>,
}

struct SyncPipe {
    handle: std::fs::File,
    // Only submission and shutdown are serialized, never completion waits.
    state: std::sync::Mutex<SyncPipeState>,
}

#[derive(Default)]
struct SyncPipeState {
    shutdown: bool,
    read_timeout: Option<std::time::Duration>,
}

impl SyncStream {
    pub fn connect(path: &Path) -> io::Result<Self> {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;

        let handle = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            .open(path_to_pipe_name(path))?;
        Ok(Self {
            inner: std::sync::Arc::new(SyncPipe {
                handle,
                state: std::sync::Mutex::new(SyncPipeState::default()),
            }),
        })
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.clone(),
        })
    }

    /// Set the timeout for subsequent reads on this stream and all its clones.
    /// An already submitted read keeps its original timeout. Timed-out reads
    /// are canceled and drained before returning, so a slow kernel cancellation
    /// may extend the wait beyond the requested duration.
    pub fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> io::Result<()> {
        if timeout.is_some_and(|duration| duration.is_zero()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "read timeout must be nonzero",
            ));
        }
        self.state()?.read_timeout = timeout;
        Ok(())
    }

    /// Interrupt both halves, including reads already blocked on other threads.
    /// Like socket shutdown this affects all clones, and is irreversible.
    /// Named pipes do not support independent read/write half shutdown here.
    pub fn shutdown(&self, how: std::net::Shutdown) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::ERROR_NOT_FOUND;
        use windows_sys::Win32::System::IO::CancelIoEx;

        if how != std::net::Shutdown::Both {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "named pipes only support Shutdown::Both",
            ));
        }
        let mut state = self.state()?;
        state.shutdown = true;
        // Keep the submission gate locked until cancellation is requested so
        // no request can slip between the shutdown check and CancelIoEx.
        let canceled = unsafe { CancelIoEx(self.inner.handle.as_raw_handle(), std::ptr::null()) };
        if canceled == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_NOT_FOUND as i32) {
                return Err(error);
            }
        }
        Ok(())
    }

    fn state(&self) -> io::Result<std::sync::MutexGuard<'_, SyncPipeState>> {
        self.inner
            .state
            .lock()
            .map_err(|_| io::Error::other("named-pipe state lock poisoned"))
    }

    fn transfer(
        &self,
        reading: bool,
        start: impl FnOnce(
            windows_sys::Win32::Foundation::HANDLE,
            *mut windows_sys::Win32::System::IO::OVERLAPPED,
        ) -> windows_sys::Win32::Foundation::BOOL,
    ) -> io::Result<usize> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, ERROR_MORE_DATA};

        let mut operation = SyncPipeOperation::new(&self.inner.handle)?;
        let state = self.state()?;
        if state.shutdown {
            return if reading {
                Ok(0)
            } else {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "pipe shut down"))
            };
        }
        let timeout = if reading { state.read_timeout } else { None };
        // The caller borrows its buffer for this entire blocking call. The
        // boxed OVERLAPPED and owned event remain stable through completion.
        let started = start(
            self.inner.handle.as_raw_handle(),
            operation.overlapped.get(),
        );
        if started == 0 {
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(code) if code == ERROR_IO_PENDING as i32 => operation.pending = true,
                // Message-mode reads may complete with a valid short buffer.
                Some(code) if code == ERROR_MORE_DATA as i32 => {}
                _ => return Err(error),
            }
        }
        drop(state);
        operation.complete(timeout)
    }
}

/// RAII cancellation is essential: even an error waiting for the event must
/// drain pending I/O before its OVERLAPPED, event, or caller's buffer is freed.
struct SyncPipeOperation<'a> {
    handle: &'a std::fs::File,
    event: std::os::windows::io::OwnedHandle,
    // The kernel mutates this while requests are pending. Never manufacture a
    // Rust reference to the contents during that time.
    overlapped: Box<std::cell::UnsafeCell<windows_sys::Win32::System::IO::OVERLAPPED>>,
    pending: bool,
}

impl<'a> SyncPipeOperation<'a> {
    fn new(handle: &'a std::fs::File) -> io::Result<Self> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::Threading::CreateEventW;

        // A distinct manual-reset event per operation permits simultaneous
        // reads and writes on the same pipe handle.
        let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        let event = unsafe { OwnedHandle::from_raw_handle(event) };
        let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED =
            unsafe { std::mem::zeroed() };
        overlapped.hEvent = event.as_raw_handle();
        Ok(Self {
            handle,
            event,
            overlapped: Box::new(std::cell::UnsafeCell::new(overlapped)),
            pending: false,
        })
    }

    fn cancel(&self) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::IO::CancelIoEx;
        // Cancellation can race normal completion (ERROR_NOT_FOUND). Either
        // way GetOverlappedResult must observe completion before cleanup.
        unsafe { CancelIoEx(self.handle.as_raw_handle(), self.overlapped.get()) };
    }

    fn complete(&mut self, timeout: Option<std::time::Duration>) -> io::Result<usize> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{
            ERROR_MORE_DATA, ERROR_OPERATION_ABORTED, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
        };
        use windows_sys::Win32::System::IO::GetOverlappedResult;
        use windows_sys::Win32::System::Threading::WaitForSingleObject;

        let mut timed_out = false;
        if self.pending
            && let Some(timeout) = timeout
        {
            let start = std::time::Instant::now();
            loop {
                // Round up sub-millisecond waits and never turn a finite
                // timeout into INFINITE. Long timeouts are waited in chunks.
                let remaining = timeout.saturating_sub(start.elapsed());
                let millis = if remaining.is_zero() {
                    0
                } else {
                    remaining
                        .as_millis()
                        .saturating_add(1)
                        .min(u128::from(u32::MAX - 1)) as u32
                };
                let result = unsafe { WaitForSingleObject(self.event.as_raw_handle(), millis) };
                match result {
                    WAIT_OBJECT_0 => break,
                    WAIT_TIMEOUT if start.elapsed() < timeout => continue,
                    WAIT_TIMEOUT => {
                        timed_out = true;
                        self.cancel();
                        break;
                    }
                    WAIT_FAILED => return Err(io::Error::last_os_error()),
                    _ => return Err(io::Error::other("unexpected named-pipe wait result")),
                }
            }
        }
        let mut transferred = 0;
        // Even after CancelIoEx, wait for terminal completion. Windows does
        // not promise cancellation is instantaneous, so abandoning this wait
        // at a deadline would leave the kernel using freed Rust memory.
        let succeeded = unsafe {
            GetOverlappedResult(
                self.handle.as_raw_handle(),
                self.overlapped.get(),
                &mut transferred,
                1,
            )
        };
        self.pending = false;
        if succeeded != 0 {
            return Ok(transferred as usize);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(code) if code == ERROR_MORE_DATA as i32 => Ok(transferred as usize),
            Some(code) if code == ERROR_OPERATION_ABORTED as i32 => Err(io::Error::new(
                if timed_out {
                    io::ErrorKind::TimedOut
                } else {
                    io::ErrorKind::Interrupted
                },
                if timed_out {
                    "named-pipe read timed out"
                } else {
                    "named-pipe operation canceled"
                },
            )),
            _ => Err(error),
        }
    }
}

impl Drop for SyncPipeOperation<'_> {
    fn drop(&mut self) {
        if self.pending {
            self.cancel();
            let _ = self.complete(None);
        }
    }
}

impl io::Read for SyncStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        use windows_sys::Win32::Foundation::{
            ERROR_BROKEN_PIPE, ERROR_HANDLE_EOF, ERROR_PIPE_NOT_CONNECTED,
        };
        use windows_sys::Win32::Storage::FileSystem::ReadFile;

        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(u32::MAX as usize) as u32;
        let result = self.transfer(true, |handle, overlapped| unsafe {
            ReadFile(
                handle,
                buf.as_mut_ptr(),
                len,
                std::ptr::null_mut(),
                overlapped,
            )
        });
        match result {
            Err(error)
                if matches!(error.raw_os_error(), Some(code) if
                    code == ERROR_BROKEN_PIPE as i32 || code == ERROR_HANDLE_EOF as i32 ||
                    code == ERROR_PIPE_NOT_CONNECTED as i32) =>
            {
                Ok(0)
            }
            result => result,
        }
    }
}

impl io::Write for SyncStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        use windows_sys::Win32::Storage::FileSystem::WriteFile;

        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(u32::MAX as usize) as u32;
        self.transfer(false, |handle, overlapped| unsafe {
            WriteFile(handle, buf.as_ptr(), len, std::ptr::null_mut(), overlapped)
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        // Writes already wait for completion. FlushFileBuffers would instead
        // wait for the peer to read everything, which can deadlock duplex IPC.
        Ok(())
    }
}

pub fn is_socket_path(path: &Path) -> bool {
    let pipe_name = path_to_pipe_name(path);
    match probe_pipe(&pipe_name) {
        Ok(_) => true,
        Err(error)
            if error.raw_os_error()
                == Some(windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32) =>
        {
            // Every published instance being occupied proves that a server is
            // listening. Do not report the endpoint as absent and race a second
            // daemon against it merely because another client connected first.
            true
        }
        Err(_) => false,
    }
}

pub fn remove_socket(path: &Path) {
    let pipe_name = path_to_pipe_name(path);
    if probe_pipe(&pipe_name).is_ok() {
        eprintln!(
            "[windows] Named pipe {} still open, will be replaced by new server",
            pipe_name
        );
    }
}

/// Probe whether a named pipe is currently reachable, without a tokio reactor.
///
/// `ClientOptions::open` registers the pipe handle with the ambient tokio
/// reactor, so it panics with "there is no reactor running" when called from a
/// thread that has no runtime context. The two probes above are synchronous by
/// contract and are reachable from plain `std::thread` callers, so they must
/// not touch the tokio client API. `SyncStream::connect` already opens pipes
/// with `std::fs`, which is the same approach.
fn probe_pipe(pipe_name: &str) -> io::Result<()> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe_name)
        .map(|_handle| ())
}

pub fn stream_pair() -> io::Result<(Stream, Stream)> {
    Stream::pair()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    static BUSY_PIPE_TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Jcode Desktop drives the SDK from plain `std::thread`s, where tokio's
    /// named-pipe client panics with "there is no reactor running". The two
    /// synchronous probes must therefore stay off the tokio client API.
    ///
    /// The pipe has to be a real, bound one: an absent pipe makes the open fail
    /// before tokio reaches the reactor registration, so probing a missing
    /// endpoint would pass with or without the fix and prove nothing.
    ///
    /// The test itself is a `#[tokio::test]` so that `Listener::bind` has a
    /// runtime, while the `std::thread` doing the probe deliberately has none.
    /// That split is the situation this bug is about: the server owns a runtime,
    /// the synchronous probes are called from plain threads that do not.
    #[tokio::test]
    async fn sync_pipe_probes_do_not_panic_without_a_tokio_reactor() {
        let path = std::env::temp_dir().join(format!(
            "jcode-plain-thread-probe-{}-{}.sock",
            std::process::id(),
            BUSY_PIPE_TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _listener = Listener::bind(&path).expect("bind named pipe");

        // A second, separately bound pipe for `remove_socket`. Probing consumes
        // the only available instance of a pipe, so reusing `path` would let the
        // second probe return ERROR_PIPE_BUSY and never reach the code under
        // test, which is exactly the coverage a single shared pipe left out.
        let remove_path = std::env::temp_dir().join(format!(
            "jcode-plain-thread-remove-{}-{}.sock",
            std::process::id(),
            BUSY_PIPE_TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _remove_listener = Listener::bind(&remove_path).expect("bind second named pipe");

        let probe = path.clone();
        let remove_probe = remove_path.clone();
        let joined = std::thread::spawn(move || {
            let reachable = is_socket_path(&probe);
            remove_socket(&remove_probe);
            reachable
        })
        .join();

        assert!(
            joined.is_ok(),
            "probing a live pipe on a plain thread panicked: {:?}",
            joined.err()
        );
        assert_eq!(
            joined.expect("thread joined"),
            true,
            "a bound named pipe must be reported as a live endpoint"
        );
    }

    /// The TypeScript SDK derives this name independently, so the two must
    /// agree exactly or a Windows client dials a pipe nobody is listening on.
    /// Pinning literal values here gives that duplicate a fixed contract to
    /// match, and `sockets.test.ts` asserts the same strings.
    #[test]
    fn pipe_name_matches_the_typescript_sdk() {
        for (path, expected) in [
            (
                r"C:\Users\jeremy\AppData\Local\jcode\run\jcode-api.sock",
                r"\\.\pipe\jcode-api-5e00c01702e8cfe4",
            ),
            (r"C:\a\b\jcode.sock", r"\\.\pipe\jcode-52dfdb00b2f35a71"),
        ] {
            assert_eq!(
                path_to_pipe_name(Path::new(path)),
                expected,
                "pipe name for {path} drifted from the SDK's derivation"
            );
        }
    }

    #[test]
    fn pipe_name_is_stable_and_normalizes_case_and_separators() {
        let a = path_to_pipe_name(Path::new(r"C:\Temp\Jcode\server.sock"));
        let b = path_to_pipe_name(Path::new("c:/temp/jcode/server.sock"));
        assert_eq!(a, b, "pipe names should be normalized consistently");
        assert!(
            a.starts_with(r"\\.\pipe\server-"),
            "unexpected pipe name: {}",
            a
        );
    }

    #[test]
    fn pipe_name_falls_back_when_stem_is_empty() {
        let name = path_to_pipe_name(Path::new("..."));
        assert!(
            name.starts_with(r"\\.\pipe\jcode-"),
            "unexpected pipe name: {}",
            name
        );
    }

    #[tokio::test]
    async fn stream_pair_round_trips_bytes() {
        let (mut a, mut b) = stream_pair().expect("create stream pair");
        a.write_all(b"ping").await.expect("write to pipe");
        a.flush().await.expect("flush pipe");

        let mut buf = [0u8; 4];
        b.read_exact(&mut buf).await.expect("read from pipe");
        assert_eq!(&buf, b"ping");
    }

    #[tokio::test]
    async fn split_stream_supports_concurrent_read_and_write() {
        let (a, mut b) = stream_pair().expect("create stream pair");
        let (mut read, mut write) = a.into_split();

        let peer = tokio::spawn(async move {
            let mut request = [0u8; 4];
            b.read_exact(&mut request).await.expect("read request");
            assert_eq!(&request, b"ping");
            b.write_all(b"pong").await.expect("write response");
            b.flush().await.expect("flush response");
        });

        let exchange = async {
            write.write_all(b"ping").await.expect("write request");
            write.flush().await.expect("flush request");
            let mut response = [0u8; 4];
            read.read_exact(&mut response).await.expect("read response");
            assert_eq!(&response, b"pong");
        };

        tokio::time::timeout(std::time::Duration::from_secs(5), exchange)
            .await
            .expect("split stream exchange should not stall");
        peer.await.expect("peer task");
    }

    /// Exercise the exact cold-start race: a synchronous readiness probe
    /// closes its client handle before the server polls ConnectNamedPipe.
    #[tokio::test]
    async fn accept_survives_closed_readiness_probes() {
        let path = std::env::temp_dir().join(format!(
            "jcode-closed-probe-{}-{}.sock",
            std::process::id(),
            BUSY_PIPE_TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut listener = Listener::bind(&path).expect("bind named pipe");

        for _ in 0..8 {
            assert!(
                is_socket_path(&path),
                "probe must open the waiting instance"
            );
            // Mio treats ERROR_NO_DATA during ConnectNamedPipe as an accepted
            // but already closed stream. Consume that probe just as the server
            // does, then prove a real client can still exchange data.
            let (mut probe, _) = listener.accept().await.expect("accept closed probe");
            let mut byte = [0];
            let read =
                tokio::time::timeout(std::time::Duration::from_secs(5), probe.read(&mut byte))
                    .await
                    .expect("closed probe read must not hang");
            assert!(
                matches!(read, Ok(0)) || read.is_err_and(|e| e.kind() == io::ErrorKind::BrokenPipe),
                "closed readiness probe should report EOF or broken pipe"
            );
            drop(probe);

            let exchange = async {
                let server = async {
                    let (mut stream, _) = listener.accept().await.expect("accept after probe");
                    let mut request = [0; 4];
                    stream.read_exact(&mut request).await.expect("read request");
                    assert_eq!(&request, b"ping");
                    stream.write_all(b"pong").await.expect("write response");
                };
                let client = async {
                    let mut stream = Stream::connect(&path).await.expect("connect after probe");
                    stream.write_all(b"ping").await.expect("write request");
                    let mut response = [0; 4];
                    stream
                        .read_exact(&mut response)
                        .await
                        .expect("read response");
                    assert_eq!(&response, b"pong");
                };
                tokio::join!(server, client);
            };
            tokio::time::timeout(std::time::Duration::from_secs(5), exchange)
                .await
                .expect("listener must remain usable after a closed probe");
        }
    }

    #[tokio::test]
    async fn accepted_client_disconnect_does_not_stop_listener() {
        let path = std::env::temp_dir().join(format!(
            "jcode-client-disconnect-{}-{}.sock",
            std::process::id(),
            BUSY_PIPE_TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut listener = Listener::bind(&path).expect("bind named pipe");
        for _ in 0..8 {
            let client = Stream::connect(&path).await.expect("connect client");
            let (mut server, _) = listener.accept().await.expect("accept client");
            drop(client);
            let mut byte = [0];
            let read =
                tokio::time::timeout(std::time::Duration::from_secs(5), server.read(&mut byte))
                    .await
                    .expect("disconnected read must not hang");
            assert!(
                matches!(read, Ok(0)) || read.is_err_and(|e| e.kind() == io::ErrorKind::BrokenPipe),
                "closed client should report EOF or broken pipe"
            );
        }
    }

    #[tokio::test]
    async fn busy_pipe_is_reported_as_a_live_socket_path() {
        let path = std::env::temp_dir().join(format!(
            "jcode-busy-pipe-probe-{}-{}.sock",
            std::process::id(),
            BUSY_PIPE_TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _listener = Listener::bind(&path).expect("bind named pipe");
        let _client = Stream::connect(&path).await.expect("occupy named pipe");

        assert!(
            is_socket_path(&path),
            "a busy named pipe must still be recognized as a live server endpoint"
        );
    }
}
