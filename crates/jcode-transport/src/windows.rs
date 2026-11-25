use sha2::{Digest, Sha256};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;
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

// ---------------------------------------------------------------------------
// Integrity levels and derived pipe names
// ---------------------------------------------------------------------------

/// Last sub-authority of a mandatory label SID, i.e. the integrity RID.
pub(crate) const INTEGRITY_LOW: u32 = 0x1000;
pub(crate) const INTEGRITY_MEDIUM: u32 = 0x2000;
pub(crate) const INTEGRITY_HIGH: u32 = 0x3000;
pub(crate) const INTEGRITY_SYSTEM: u32 = 0x4000;
pub(crate) const INTEGRITY_PROTECTED: u32 = 0x5000;

/// How many times a busy name is retried before the bind is reported as failed.
const MAX_BUSY_ATTEMPTS: u32 = 10;
/// How many private fallback names a bind may try after access is denied.
const MAX_FALLBACK_ATTEMPTS: u32 = 3;

/// Short, stable tag for an integrity level.
///
/// A high-integrity (elevated) process and a medium-integrity process must not
/// contend for one pipe name: whichever runs first creates the object, and the
/// other is then denied access to an object it may neither open nor add an
/// instance to. Tagging the name keeps the two apart by construction.
pub(crate) fn integrity_tag(level_rid: u32) -> &'static str {
    match level_rid {
        INTEGRITY_LOW => "low",
        INTEGRITY_MEDIUM => "med",
        INTEGRITY_HIGH => "high",
        INTEGRITY_SYSTEM => "system",
        INTEGRITY_PROTECTED => "protected",
        // Medium is the default for an ordinary interactive login. Any
        // unexpected level falls back to it so an unreadable token still
        // reaches the daemon a normal user would have started.
        _ => "med",
    }
}

/// Integrity RID of the current process, or `None` when it cannot be read.
fn current_integrity_rid() -> Option<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
        TOKEN_QUERY, TokenIntegrityLevel,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }

        // The label SID varies in length, so ask for the required size first.
        let mut needed: u32 = 0;
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
        // `usize` words keep the buffer aligned for `TOKEN_MANDATORY_LABEL`.
        let words = (needed as usize)
            .div_ceil(std::mem::size_of::<usize>())
            .max(4);
        let mut buffer = vec![0usize; words];
        let mut written: u32 = 0;
        let read = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * std::mem::size_of::<usize>()) as u32,
            &mut written,
        );
        let _ = CloseHandle(token);
        if read == 0 {
            return None;
        }

        let label = &*buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>();
        let sid = label.Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return None;
        }
        Some(*GetSidSubAuthority(sid, u32::from(count) - 1))
    }
}

/// Insert a tag immediately before the trailing path hash of `base`.
pub(crate) fn tag_pipe_name(base: &str, tag: &str) -> String {
    match base.rsplit_once('-') {
        Some((head, tail)) => format!("{head}-{tag}-{tail}"),
        None => format!("{base}-{tag}"),
    }
}

/// The name a server for `path` publishes at a given integrity level.
///
/// Medium integrity keeps the historical untagged name, so existing medium
/// installations keep talking to their own daemon across an upgrade and the
/// TypeScript SDK's derivation stays valid for them. Any other level is tagged,
/// which is what stops an elevated server and a medium one from contending.
pub(crate) fn shared_pipe_name_for(base: &str, level_rid: Option<u32>) -> String {
    match level_rid {
        Some(rid) if integrity_tag(rid) != "med" => tag_pipe_name(base, integrity_tag(rid)),
        _ => base.to_string(),
    }
}

/// A name private to this process and this bind attempt.
///
/// `ERROR_ACCESS_DENIED` means the restricted name can never be opened by this
/// process, so retrying it is guaranteed to fail again. This name is fresh by
/// construction: it carries the process id and the attempt number.
pub(crate) fn private_pipe_name(base: &str, attempt: u32) -> String {
    tag_pipe_name(base, &format!("p{}-{}", std::process::id(), attempt))
}

// ---------------------------------------------------------------------------
// Failure classification
// ---------------------------------------------------------------------------

/// Why a bind attempt failed, in the terms this module acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindFailure {
    /// Every published instance is occupied. Retrying the same name is right.
    Busy,
    /// The name is owned by an object this process may not touch: another user,
    /// or the same user at a different integrity level. Retrying the same name
    /// can never succeed.
    AccessDenied,
    /// Anything else; report it verbatim.
    Other,
}

impl BindFailure {
    pub(crate) fn classify(error: &io::Error) -> Self {
        match error.raw_os_error() {
            Some(code) if code == windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32 => {
                Self::Busy
            }
            Some(code) if code == windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED as i32 => {
                Self::AccessDenied
            }
            _ => Self::Other,
        }
    }

    /// The cause, in the words the error message uses.
    pub(crate) fn cause(self) -> &'static str {
        match self {
            Self::Busy => "every published instance of the pipe is already connected",
            Self::AccessDenied => {
                "the pipe belongs to a process this one is not permitted to reach \
                 (a different Windows integrity level or another user)"
            }
            Self::Other => "the operating system refused to create the pipe",
        }
    }

    /// Whether the next attempt must target a *different* name.
    pub(crate) fn requires_fresh_name(self) -> bool {
        matches!(self, Self::AccessDenied)
    }
}

/// The name the next bind attempt will use.
///
/// Pure so the fallback decision is testable without elevation. The original
/// bug was a retry that kept targeting the same restricted name.
pub(crate) fn next_attempt_name(base: &str, attempt: u32, failure: BindFailure) -> String {
    if failure.requires_fresh_name() {
        private_pipe_name(base, attempt)
    } else {
        base.to_string()
    }
}

/// The retry notice, emitted only when the next attempt really targets a
/// different pipe name. Announcing a "retry" while re-targeting the same
/// restricted name is what made the original failure unreadable.
pub(crate) fn retry_notice(from: &str, to: &str, failure: BindFailure) -> Option<String> {
    if from == to {
        return None;
    }
    Some(format!(
        "[windows] named pipe {from} unavailable: {}; falling back to private pipe {to}",
        failure.cause()
    ))
}

fn bind_error(name: &str, failure: BindFailure, error: io::Error) -> io::Error {
    let mut message = format!(
        "cannot bind named pipe {name}: {error}; {}",
        failure.cause()
    );
    if failure == BindFailure::AccessDenied {
        message.push_str(
            " Remedy: run jcode at the same integrity level as the running server \
             (typically the same elevated terminal), stop the stale server, or set \
             JCODE_WINDOWS_PIPE_SHARED=1 to publish a shared pipe that admits the \
             interactive user at medium integrity.",
        );
    }
    io::Error::new(error.kind(), message)
}

fn connect_error(path: &Path, last: Option<io::Error>) -> io::Error {
    let tried = candidate_pipe_names(path);
    let detail = last
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| "no candidate pipe name".to_string());
    io::Error::new(
        last.as_ref()
            .map_or(io::ErrorKind::NotFound, io::Error::kind),
        format!(
            "no jcode server is listening for {} (tried {}): {detail}",
            path.display(),
            tried.join(", ")
        ),
    )
}

// ---------------------------------------------------------------------------
// Published-name marker
// ---------------------------------------------------------------------------

/// Sidecar recording the pipe name a server actually published.
///
/// Per-integrity naming and the private fallback both mean the effective name
/// is no longer implied by the socket path alone, so the server records it and
/// every client reads it instead of guessing.
fn pipe_name_marker(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.pipe-name", path.to_string_lossy()))
}

fn publish_pipe_name(path: &Path, pipe_name: &str) {
    let marker = pipe_name_marker(path);
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = std::fs::write(&marker, format!("{pipe_name}\n")) {
        eprintln!(
            "[windows] could not record the pipe name in {}: {error}",
            marker.display()
        );
    }
}

fn recorded_pipe_name(path: &Path) -> Option<String> {
    let recorded = std::fs::read_to_string(pipe_name_marker(path)).ok()?;
    let name = recorded.trim();
    name.starts_with(r"\\.\pipe\").then(|| name.to_string())
}

/// Every pipe name a client should try for `path`, most specific first.
pub(crate) fn candidate_pipe_names(path: &Path) -> Vec<String> {
    let base = path_to_pipe_name(path);
    let mut names: Vec<String> = Vec::with_capacity(3);
    if let Some(recorded) = recorded_pipe_name(path) {
        names.push(recorded);
    }
    let own = shared_pipe_name_for(&base, current_integrity_rid());
    if !names.contains(&own) {
        names.push(own);
    }
    if !names.contains(&base) {
        names.push(base);
    }
    names
}

/// Opt-in for a single server shared across integrity levels.
///
/// Off by default: separate per-integrity servers need no security rewrite,
/// while a shared one has to relax the pipe's DACL and integrity label to admit
/// medium-integrity clients.
fn shared_pipe_requested() -> bool {
    std::env::var_os("JCODE_WINDOWS_PIPE_SHARED").is_some_and(|value| {
        let value = value.to_string_lossy().to_ascii_lowercase();
        !(value.is_empty() || value == "0" || value == "false" || value == "no")
    })
}

/// Grant the interactive user and local administrators access to a shared
/// pipe, and label it medium integrity so a medium-integrity client can open a
/// pipe that an elevated server created.
///
/// UNVERIFIED-BY-BUILD: this path needs an elevated server and a separate
/// medium-integrity client. The manual smoke steps are in the PR. Best effort:
/// a failure is reported and the server keeps running, because the
/// per-integrity name already gave this process a working endpoint.
fn admit_interactive_user(server: &NamedPipeServer) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SE_KERNEL_OBJECT, SetSecurityInfo,
    };
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetSecurityDescriptorSacl,
        LABEL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    };

    // IU: interactive user. SY: local system. BA: built-in administrators.
    // S:(ML;;NW;;;ME) labels the pipe medium integrity, so mandatory integrity
    // policy no longer blocks a medium-integrity client from opening it.
    let sddl: Vec<u16> = "D:(A;;GRGW;;;IU)(A;;GA;;;SY)(A;;GA;;;BA)S:(ML;;NW;;;ME)"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1, // SDDL_REVISION_1
            &mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            eprintln!(
                "[windows] could not build the shared pipe security descriptor: {}",
                io::Error::last_os_error()
            );
            return;
        }

        let mut dacl_present: i32 = 0;
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut dacl_defaulted: i32 = 0;
        let dacl_ok = GetSecurityDescriptorDacl(
            descriptor,
            &mut dacl_present,
            &mut dacl,
            &mut dacl_defaulted,
        );

        let mut sacl_present: i32 = 0;
        let mut sacl: *mut ACL = std::ptr::null_mut();
        let mut sacl_defaulted: i32 = 0;
        let sacl_ok = GetSecurityDescriptorSacl(
            descriptor,
            &mut sacl_present,
            &mut sacl,
            &mut sacl_defaulted,
        );

        if dacl_ok == 0 || sacl_ok == 0 {
            eprintln!(
                "[windows] could not read the shared pipe security descriptor: {}",
                io::Error::last_os_error()
            );
            LocalFree(descriptor as HLOCAL);
            return;
        }

        let status = SetSecurityInfo(
            server.as_raw_handle() as _,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            dacl,
            sacl,
        );
        if status != 0 {
            eprintln!(
                "[windows] could not admit medium-integrity clients to the shared pipe: {}",
                io::Error::from_raw_os_error(status as i32)
            );
        }
        LocalFree(descriptor as HLOCAL);
    }
}

/// Open one candidate pipe, waiting out `ERROR_PIPE_BUSY` for a bounded time.
async fn open_candidate(name: &str) -> io::Result<NamedPipeClient> {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match ClientOptions::new().open(name) {
            Ok(client) => return Ok(client),
            Err(error)
                if error.raw_os_error()
                    == Some(windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32) =>
            {
                if std::time::Instant::now() >= deadline {
                    return Err(error);
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Listener wraps a Windows named pipe server, providing an accept loop
/// that matches the UnixListener interface.
pub struct Listener {
    pipe_name: String,
    current_server: NamedPipeServer,
}

impl Listener {
    /// Bind the endpoint for `path`.
    ///
    /// The name carries this process's integrity level, so an elevated server
    /// and a medium-integrity server never fight over one name. If the name is
    /// denied anyway, this process cannot reach that object at all, so a
    /// private server is started under a fresh name and that name is recorded
    /// for clients to find.
    pub fn bind(path: &Path) -> io::Result<Self> {
        let base = path_to_pipe_name(path);
        let shared = shared_pipe_requested();
        let mut name = if shared {
            base.clone()
        } else {
            shared_pipe_name_for(&base, current_integrity_rid())
        };
        let mut attempt = 1u32;

        loop {
            match ServerOptions::new().first_pipe_instance(true).create(&name) {
                Ok(server) => {
                    if shared {
                        admit_interactive_user(&server);
                    }
                    publish_pipe_name(path, &name);
                    return Ok(Self {
                        pipe_name: name,
                        current_server: server,
                    });
                }
                Err(error) => {
                    let failure = BindFailure::classify(&error);
                    if failure == BindFailure::Busy && attempt < MAX_BUSY_ATTEMPTS {
                        attempt += 1;
                        std::thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    if failure.requires_fresh_name() && attempt <= MAX_FALLBACK_ATTEMPTS {
                        let next = next_attempt_name(&base, attempt, failure);
                        if let Some(notice) = retry_notice(&name, &next, failure) {
                            eprintln!("{notice}");
                        }
                        name = next;
                        attempt += 1;
                        continue;
                    }
                    return Err(bind_error(&name, failure, error));
                }
            }
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
    /// Connect to the endpoint for `path`.
    ///
    /// The published name is recorded beside the socket path, so candidates are
    /// the recorded name, this process's own integrity-tagged name, and the
    /// historical untagged name. A server that is merely busy is waited for
    /// rather than treated as absent, and an absent server is reported with the
    /// names that were tried.
    pub async fn connect(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        loop {
            let mut busy = false;
            let mut last_error = None;
            for name in candidate_pipe_names(path) {
                match open_candidate(&name).await {
                    Ok(client) => return Ok(Stream::Client(client)),
                    Err(error) => {
                        if error.raw_os_error()
                            == Some(windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32)
                        {
                            busy = true;
                        }
                        last_error = Some(error);
                    }
                }
            }
            if !busy {
                return Err(connect_error(path, last_error));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
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

        let mut last_error = None;
        for name in candidate_pipe_names(path) {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            loop {
                match std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .custom_flags(FILE_FLAG_OVERLAPPED)
                    .open(&name)
                {
                    Ok(handle) => {
                        return Ok(Self {
                            inner: std::sync::Arc::new(SyncPipe {
                                handle,
                                state: std::sync::Mutex::new(SyncPipeState::default()),
                            }),
                        });
                    }
                    Err(error) => {
                        let busy = error.raw_os_error()
                            == Some(windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32);
                        if busy && std::time::Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        last_error = Some(error);
                        break;
                    }
                }
            }
        }
        Err(connect_error(path, last_error))
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
    candidate_pipe_names(path)
        .iter()
        .any(|name| match probe_pipe(name) {
            Ok(_) => true,
            Err(error)
                if error.raw_os_error()
                    == Some(windows_sys::Win32::Foundation::ERROR_PIPE_BUSY as i32) =>
            {
                // Every published instance being occupied proves that a server
                // is listening. Do not report the endpoint as absent and race a
                // second daemon against it merely because another client
                // connected first.
                true
            }
            Err(_) => false,
        })
}

pub fn remove_socket(path: &Path) {
    if let Some(name) = candidate_pipe_names(path)
        .into_iter()
        .find(|name| probe_pipe(name).is_ok())
    {
        eprintln!("[windows] Named pipe {name} still open, will be replaced by new server");
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

    fn bind_failure_on(raw: u32) -> BindFailure {
        BindFailure::classify(&io::Error::from_raw_os_error(raw as i32))
    }

    /// The live bug reported access-denied as "busy" and then retried the same
    /// restricted pipe. Access denied is its own cause and never Busy.
    #[test]
    fn access_denied_is_classified_separately_from_busy() {
        use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY};

        let denied = bind_failure_on(ERROR_ACCESS_DENIED);
        let busy = bind_failure_on(ERROR_PIPE_BUSY);

        assert_eq!(denied, BindFailure::AccessDenied);
        assert_eq!(busy, BindFailure::Busy);
        assert_ne!(
            denied,
            BindFailure::Busy,
            "access denied must never be reported as a busy pipe"
        );
        assert_eq!(bind_failure_on(2), BindFailure::Other);
    }

    /// Regression: on access-denied the *second* connect attempt must target a
    /// different pipe name. The live evidence showed both attempts on the same
    /// name (jcode-a930d863d87e3015), which is what the old retry did.
    #[test]
    fn access_denied_retry_targets_a_different_pipe_name() {
        let base = r"\\.\pipe\jcode-a930d863d87e3015";

        let first_attempt = base.to_string();
        let second_attempt = next_attempt_name(&first_attempt, 1, BindFailure::AccessDenied);

        assert_ne!(
            second_attempt, first_attempt,
            "an access-denied retry must not reuse the restricted pipe name"
        );
        assert_eq!(
            second_attempt,
            next_attempt_name(&first_attempt, 1, BindFailure::AccessDenied),
            "fallback naming must be deterministic"
        );

        let third_attempt = next_attempt_name(&first_attempt, 2, BindFailure::AccessDenied);
        assert_ne!(third_attempt, second_attempt);
        assert_ne!(third_attempt, first_attempt);
    }

    #[test]
    fn busy_retry_targets_the_same_pipe_name() {
        let base = r"\\.\pipe\jcode-a930d863d87e3015";
        assert_eq!(next_attempt_name(base, 1, BindFailure::Busy), base);
        assert!(!BindFailure::Busy.requires_fresh_name());
        assert!(BindFailure::AccessDenied.requires_fresh_name());
        assert!(!BindFailure::Other.requires_fresh_name());
    }

    /// The "retrying" notice is only truthful when the next attempt genuinely
    /// uses another name; printing it next to the same name was the misreport.
    #[test]
    fn retry_notice_only_when_the_name_changes() {
        let base = r"\\.\pipe\jcode-a930d863d87e3015";

        assert!(
            retry_notice(base, base, BindFailure::AccessDenied).is_none(),
            "a retry against the same name must not announce a fallback"
        );

        let fresh = next_attempt_name(base, 1, BindFailure::AccessDenied);
        let notice = retry_notice(base, &fresh, BindFailure::AccessDenied)
            .expect("a genuine fallback must be announced");
        assert!(
            notice.contains(base),
            "notice must name the restricted pipe"
        );
        assert!(notice.contains(&fresh), "notice must name the fresh pipe");
        assert!(
            !notice.contains("busy"),
            "access denied must not be described as busy"
        );
    }

    /// Two integrity levels must derive two distinct names, deterministically.
    #[test]
    fn integrity_levels_derive_distinct_names() {
        let base = r"\\.\pipe\jcode-api-5e00c01702e8cfe4";

        let medium = shared_pipe_name_for(base, Some(INTEGRITY_MEDIUM));
        let high = shared_pipe_name_for(base, Some(INTEGRITY_HIGH));

        assert_eq!(medium, base, "medium integrity keeps the historical name");
        assert_ne!(
            high, medium,
            "an elevated server must not contend with a medium-integrity one"
        );
        assert_eq!(
            high,
            shared_pipe_name_for(base, Some(INTEGRITY_HIGH)),
            "name derivation must be deterministic"
        );
        assert_ne!(
            shared_pipe_name_for(base, Some(INTEGRITY_LOW)),
            shared_pipe_name_for(base, Some(INTEGRITY_SYSTEM)),
            "distinct levels must produce distinct names"
        );
        assert_eq!(integrity_tag(INTEGRITY_MEDIUM), "med");
        assert_ne!(
            integrity_tag(INTEGRITY_MEDIUM),
            integrity_tag(INTEGRITY_HIGH)
        );
        assert!(high.starts_with(r"\\.\pipe\jcode-api-high-"));
    }

    /// Clients must be able to find a name a server published, since the
    /// effective name is no longer implied by the socket path alone.
    #[test]
    fn candidate_names_lead_with_the_recorded_pipe_name() {
        let path = std::env::temp_dir().join(format!(
            "jcode-marker-{}-{}.sock",
            std::process::id(),
            BUSY_PIPE_TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let base = path_to_pipe_name(&path);
        let private = private_pipe_name(&base, 1);

        // Without a sidecar the candidates are this process's own name plus the
        // historical one; tests may run elevated, so do not assume which comes
        // first, only that the historical name is last and always tried.
        let unrecorded = candidate_pipe_names(&path);
        assert_eq!(unrecorded.last(), Some(&base));
        assert!(unrecorded.contains(&base));

        publish_pipe_name(&path, &private);
        let candidates = candidate_pipe_names(&path);
        assert_eq!(
            candidates.first(),
            Some(&private),
            "a recorded name must be tried first"
        );
        assert!(
            candidates.contains(&base),
            "the historical name must still be tried"
        );

        let _ = std::fs::remove_file(pipe_name_marker(&path));
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
