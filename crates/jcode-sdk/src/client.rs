//! Connected harness client: handshake, reply correlation, event streaming.
//!
//! Mirrors the TypeScript SDK's `JcodeClient` capability for capability. The
//! shapes differ where Rust and TS differ honestly (`Result` instead of
//! throwing, channels instead of `EventEmitter`), but every method here has a
//! counterpart there and vice versa; `parity.rs` enforces that.
//!
//! Blocking, thread-based, and `Clone`. A desktop app has a UI thread that
//! must never block on a socket and a worker that reads it forever, so the
//! client is a handle both can hold: one reader thread owns the stream and
//! fans frames out to whoever asked for them.

use crate::errors::{Error, ErrorKind, Result};
use crate::launch::{LaunchOptions, LaunchedInstance, ensure_runtime, launch_instance};
use jcode_harness_api::{
    API_VERSION_MAJOR, ApiEvent, ApiRequest, ClientFrame, HistoryMessage, ModelRouteInfo,
    PermissionDecision, ServerFrame, SessionInfo, TextMatch, api_socket_path, read_frame,
    write_frame,
};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{
    Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError, channel, sync_channel,
};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// How a client reaches the harness.
pub struct ConnectOptions {
    /// Defaults to the resolved harness API socket path.
    pub socket_path: Option<std::path::PathBuf>,
    /// Client identity sent in the handshake, e.g. "my-app/1.0".
    pub client_name: String,
    /// How long a request waits for its reply. `None` disables the timeout.
    pub request_timeout: Option<Duration>,
    /// Start the daemon and bridge if they are not already listening.
    ///
    /// On by default: an app that only works once the user has launched two
    /// daemons by hand is indistinguishable from a broken one.
    pub ensure_runtime: bool,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            socket_path: None,
            client_name: concat!("jcode-sdk-rs/", env!("CARGO_PKG_VERSION")).to_string(),
            request_timeout: Some(Duration::from_secs(30)),
            ensure_runtime: true,
        }
    }
}

/// A duplex byte transport. Lets tests and future WebSockets plug in.
pub trait Transport: Send {
    /// A handle which interrupts both halves after `split`. Custom transports
    /// may omit this; native sockets provide it so Drop closes promptly.
    fn shutdown_handle(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        None
    }

    fn split(self: Box<Self>) -> Result<(Box<dyn BufRead + Send>, Box<dyn Write + Send>)>;
}

/// A Unix-domain socket transport, the default.
type PlatformUnixStream = jcode_transport::SyncStream;

pub struct UnixTransport(PlatformUnixStream);

impl UnixTransport {
    pub fn connect(path: &std::path::Path) -> Result<Self> {
        // A bare `No such file or directory` names the syscall and hides the
        // cause: the bridge is not running. Connecting is the first thing
        // anyone does with this SDK, so say what to do about it.
        let stream = PlatformUnixStream::connect(path).map_err(|cause| {
            Error::new(
                ErrorKind::ConnectFailed,
                match cause.kind() {
                    std::io::ErrorKind::NotFound => format!(
                        "no harness API socket at {}: the jcode harness is not running. \
                         Start it with `jcode serve` and `jcode-harness-api-bridge`, or \
                         connect with ensure_runtime enabled.",
                        path.display()
                    ),
                    std::io::ErrorKind::ConnectionRefused => format!(
                        "{} exists but refuses connections: a previous harness left a \
                         stale socket behind. Remove it and start the harness again.",
                        path.display()
                    ),
                    std::io::ErrorKind::PermissionDenied => format!(
                        "permission denied on {}: the socket belongs to another user.",
                        path.display()
                    ),
                    _ => format!("could not connect to {}: {cause}", path.display()),
                },
            )
        })?;
        Ok(Self(stream))
    }
}

impl Transport for UnixTransport {
    fn shutdown_handle(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        #[cfg(unix)]
        {
            let socket = self.0.try_clone().ok()?;
            return Some(Arc::new(move || {
                let _ = socket.shutdown(std::net::Shutdown::Both);
            }));
        }
        #[cfg(windows)]
        {
            // Closing the reader and writer handles interrupts named-pipe I/O.
            // They are owned by the client and dropped during shutdown.
            None
        }
    }

    fn split(self: Box<Self>) -> Result<(Box<dyn BufRead + Send>, Box<dyn Write + Send>)> {
        let writer = self
            .0
            .try_clone()
            .map_err(|e| Error::new(ErrorKind::Transport, e.to_string()))?;
        Ok((Box::new(BufReader::new(self.0)), Box::new(writer)))
    }
}

/// A subscription to the event stream.
///
/// Streaming events are fanned out to every live subscription, so an app can
/// have one loop rendering the attached session while another waits for a
/// single acknowledgement without either stealing frames from the other.
pub struct EventStream {
    rx: Receiver<ApiEvent>,
    id: u64,
    inner: Arc<Inner>,
}

impl EventStream {
    /// Block for the next event. `None` once the connection closes.
    pub fn next(&self) -> Option<ApiEvent> {
        self.rx.recv().ok()
    }

    /// Block for the next event, up to `timeout`.
    pub fn next_timeout(&self, timeout: Duration) -> Option<ApiEvent> {
        match self.rx.recv_timeout(timeout) {
            Ok(event) => Some(event),
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => None,
        }
    }
}

impl Iterator for EventStream {
    type Item = ApiEvent;
    fn next(&mut self) -> Option<ApiEvent> {
        EventStream::next(self)
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        if let Ok(mut subs) = self.inner.subscribers.lock() {
            subs.retain(|(id, _, _)| *id != self.id);
        }
    }
}

/// Discovery and buffering controls for [`JcodeClient::global_events`].
#[derive(Debug, Clone, Copy)]
pub struct GlobalEventsOptions {
    /// How often persisted sessions are rescanned. Zero performs one scan.
    pub discovery_interval: Duration,
    /// Maximum events waiting for the consumer before the stream fails loudly.
    pub max_buffered_events: usize,
}

impl Default for GlobalEventsOptions {
    fn default() -> Self {
        Self {
            discovery_interval: Duration::from_secs(1),
            max_buffered_events: 10_000,
        }
    }
}

struct GlobalEventControl {
    stopped: AtomicBool,
    terminal_error: Mutex<Option<Error>>,
    children: Mutex<HashMap<String, JcodeClient>>,
    tx: SyncSender<ApiEvent>,
    max_buffered_events: usize,
    wake_lock: Mutex<()>,
    wake: Condvar,
}

/// Events fanned in from every persisted and newly-created session.
///
/// Delivery begins when each per-session child attaches. Ordering is preserved
/// within a session; no total order across sessions is promised. Dropping this
/// stream cancels discovery and closes all child connections.
pub struct GlobalEventStream {
    rx: Receiver<ApiEvent>,
    control: Arc<GlobalEventControl>,
    discovery: Option<std::thread::JoinHandle<()>>,
}

impl GlobalEventStream {
    /// Block for the next event, returning the terminal stream error once.
    pub fn next(&self) -> Result<Option<ApiEvent>> {
        loop {
            if let Some(error) = take_global_error(&self.control) {
                return Err(error);
            }
            if self.control.stopped.load(Ordering::Acquire) {
                return Ok(None);
            }
            match self.rx.recv_timeout(Duration::from_millis(50)) {
                Ok(event) => {
                    if let Some(error) = take_global_error(&self.control) {
                        return Err(error);
                    }
                    if self.control.stopped.load(Ordering::Acquire) {
                        return Ok(None);
                    }
                    return Ok(Some(event));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.control.stopped.load(Ordering::Acquire) {
                        if let Some(error) = take_global_error(&self.control) {
                            return Err(error);
                        }
                        return Ok(None);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }

    /// Wait up to `timeout` for an event. `Ok(None)` means timeout or shutdown.
    pub fn next_timeout(&self, timeout: Duration) -> Result<Option<ApiEvent>> {
        if let Some(error) = take_global_error(&self.control) {
            return Err(error);
        }
        if self.control.stopped.load(Ordering::Acquire) {
            return Ok(None);
        }
        match self.rx.recv_timeout(timeout) {
            Ok(event) => {
                if let Some(error) = take_global_error(&self.control) {
                    Err(error)
                } else if self.control.stopped.load(Ordering::Acquire) {
                    Ok(None)
                } else {
                    Ok(Some(event))
                }
            }
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {
                if let Some(error) = take_global_error(&self.control) {
                    Err(error)
                } else {
                    Ok(None)
                }
            }
        }
    }
}

impl Iterator for GlobalEventStream {
    type Item = Result<ApiEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        GlobalEventStream::next(self).transpose()
    }
}

impl Drop for GlobalEventStream {
    fn drop(&mut self) {
        stop_global_stream(&self.control, None);
        if let Some(discovery) = self.discovery.take() {
            let _ = discovery.join();
        }
    }
}

fn take_global_error(control: &GlobalEventControl) -> Option<Error> {
    control.terminal_error.lock().ok()?.take()
}

fn stop_global_stream(control: &GlobalEventControl, error: Option<Error>) {
    if let Some(error) = error
        && let Ok(mut terminal) = control.terminal_error.lock()
        && terminal.is_none()
    {
        *terminal = Some(error);
    }
    control.stopped.store(true, Ordering::Release);
    control.wake.notify_all();
    let children = control
        .children
        .lock()
        .ok()
        .map(|mut children| std::mem::take(&mut *children));
    drop(children);
}

struct Inner {
    writer: Mutex<Box<dyn Write + Send>>,
    /// Requests waiting for their `reply_to` frame.
    pending: Mutex<HashMap<u64, Sender<ServerFrame>>>,
    /// Live subscriptions: (id, session filter, sink).
    subscribers: Mutex<Vec<(u64, Option<String>, Sender<ApiEvent>)>>,
    next_id: AtomicU64,
    next_sub: AtomicU64,
    closed: AtomicBool,
    request_timeout: Option<Duration>,
    socket_path: std::path::PathBuf,
    client_name: String,
    native_socket: bool,
    shutdown: Option<Arc<dyn Fn() + Send + Sync>>,
    client_handles: AtomicUsize,
}

/// Connected harness client.
///
/// Replies are correlated by the `reply_to` id the server echoes; anything
/// without one is a stream event and goes to every subscriber.
pub struct JcodeClient {
    inner: Arc<Inner>,
    instance: Option<Arc<LaunchedInstance>>,
    /// State directory of the private instance this client owns, if any.
    pub instance_home: Option<std::path::PathBuf>,
    /// Server identity from the handshake, e.g. "jcode-harness-api-bridge/0.1.0".
    pub server: String,
    /// Capability strings advertised by the server.
    pub capabilities: Vec<String>,
}

impl Clone for JcodeClient {
    fn clone(&self) -> Self {
        self.inner.client_handles.fetch_add(1, Ordering::Relaxed);
        Self {
            inner: Arc::clone(&self.inner),
            instance: self.instance.clone(),
            instance_home: self.instance_home.clone(),
            server: self.server.clone(),
            capabilities: self.capabilities.clone(),
        }
    }
}

impl Drop for JcodeClient {
    fn drop(&mut self) {
        if self.inner.client_handles.fetch_sub(1, Ordering::AcqRel) == 1 {
            if let Some(shutdown) = &self.inner.shutdown {
                shutdown();
            }
        }
    }
}

impl JcodeClient {
    /// Start and own a private jcode instance, then connect to it.
    ///
    /// Its state and sockets are isolated from the user's interactive jcode.
    /// The last clone of this client shuts the instance down through Drop.
    pub fn launch(options: LaunchOptions) -> Result<Self> {
        let instance = Arc::new(launch_instance(&options)?);
        let connect = ConnectOptions {
            socket_path: Some(instance.socket_path.clone()),
            client_name: options.client_name.clone(),
            request_timeout: options.request_timeout,
            ensure_runtime: false,
        };
        let mut client = Self::connect(connect)?;
        client.instance_home = Some(instance.jcode_home.clone());
        client.instance = Some(instance);
        Ok(client)
    }

    /// Connect to the jcode running on this machine.
    ///
    /// Use this to automate the user's own jcode: a desktop app, an editor
    /// plugin, a status dashboard. It shares the user's live sessions.
    pub fn connect(options: ConnectOptions) -> Result<Self> {
        let path = options.socket_path.clone().unwrap_or_else(api_socket_path);
        if options.ensure_runtime {
            ensure_runtime(&LaunchOptions::default(), &|_| {})?;
        }
        Self::over(
            Box::new(UnixTransport::connect(&path)?),
            &options,
            path,
            true,
        )
    }

    /// Connect over a caller-supplied transport. The seam tests use.
    pub fn connect_with(transport: Box<dyn Transport>, options: ConnectOptions) -> Result<Self> {
        let path = options.socket_path.clone().unwrap_or_else(api_socket_path);
        Self::over(transport, &options, path, false)
    }

    fn over(
        transport: Box<dyn Transport>,
        options: &ConnectOptions,
        socket_path: std::path::PathBuf,
        native_socket: bool,
    ) -> Result<Self> {
        let shutdown = transport.shutdown_handle();
        let (reader, writer) = transport.split()?;
        let inner = Arc::new(Inner {
            writer: Mutex::new(writer),
            pending: Mutex::new(HashMap::new()),
            subscribers: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
            next_sub: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            request_timeout: options.request_timeout,
            socket_path,
            client_name: options.client_name.clone(),
            native_socket,
            shutdown,
            client_handles: AtomicUsize::new(1),
        });
        spawn_reader(Arc::clone(&inner), reader);

        let mut client = Self {
            inner,
            instance: None,
            instance_home: None,
            server: String::new(),
            capabilities: Vec::new(),
        };
        let frame = client.request(ApiRequest::Hello {
            min_version: API_VERSION_MAJOR,
            max_version: API_VERSION_MAJOR,
            client: options.client_name.clone(),
        })?;
        match frame.event {
            ApiEvent::HelloOk {
                server,
                capabilities,
                ..
            } => {
                client.server = server;
                client.capabilities = capabilities;
                Ok(client)
            }
            ApiEvent::Error { code, message } => Err(Error::new(ErrorKind::Harness(code), message)),
            other => Err(Error::new(
                ErrorKind::HandshakeFailed,
                format!("unexpected reply to hello: {other:?}"),
            )),
        }
    }

    /// The socket this client is talking to.
    pub fn socket_path(&self) -> &std::path::Path {
        &self.inner.socket_path
    }

    /// Whether the server advertises a capability.
    ///
    /// Capabilities are how a client learns what this particular server can do
    /// before depending on it. `permissions`, for instance, is absent from the
    /// current bridge: it never issues permission prompts, so a client that
    /// waits for one waits forever. Check rather than assume.
    pub fn supports(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }

    /// Send a raw request and wait for its reply frame.
    pub fn request(&self, request: ApiRequest) -> Result<ServerFrame> {
        let (tx, rx) = channel();
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        self.inner.pending.lock().map_err(poisoned)?.insert(id, tx);
        if let Err(error) = self.write(ClientFrame::new(id, request.clone())) {
            self.inner.pending.lock().map_err(poisoned)?.remove(&id);
            return Err(error);
        }
        let received = match self.inner.request_timeout {
            Some(timeout) => rx.recv_timeout(timeout).map_err(|error| match error {
                RecvTimeoutError::Timeout => Error::new(
                    ErrorKind::Timeout,
                    format!("no reply to {} within {timeout:?}", request_name(&request)),
                ),
                RecvTimeoutError::Disconnected => closed_error(),
            }),
            None => rx.recv().map_err(|_| closed_error()),
        };
        if received.is_err() {
            self.inner.pending.lock().map_err(poisoned)?.remove(&id);
        }
        received
    }

    /// Write a request without expecting a request-level reply.
    pub fn notify(&self, request: ApiRequest) -> Result<()> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        self.write(ClientFrame::new(id, request))
    }

    fn write(&self, frame: ClientFrame) -> Result<()> {
        if self.inner.closed.load(Ordering::Relaxed) {
            return Err(closed_error());
        }
        let mut writer = self.inner.writer.lock().map_err(poisoned)?;
        write_frame(&mut *writer, &frame).map_err(Error::from)
    }

    /// Send a request, failing when the server replies with an error frame.
    fn request_ok(&self, request: ApiRequest) -> Result<ServerFrame> {
        let frame = self.request(request)?;
        match frame.event {
            ApiEvent::Error { code, message } => Err(Error::new(ErrorKind::Harness(code), message)),
            _ => Ok(frame),
        }
    }

    /// Subscribe to stream events, optionally filtered to one session.
    ///
    /// Frames that arrive between reads are buffered, so a consumer that does
    /// slow work in the loop body does not silently drop deltas.
    pub fn events(&self, session_id: Option<&str>) -> EventStream {
        let (tx, rx) = channel();
        let id = self.inner.next_sub.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut subs) = self.inner.subscribers.lock() {
            subs.push((id, session_id.map(str::to_string), tx));
        }
        EventStream {
            rx,
            id,
            inner: Arc::clone(&self.inner),
        }
    }

    /// Stream events from all persisted and newly-created sessions.
    ///
    /// The bridge attaches one session per connection, so this discovers
    /// sessions repeatedly, owns one native child connection per session, and
    /// fans their events into a bounded queue. A disconnected child is removed
    /// and attached again by a later discovery pass.
    pub fn global_events(&self, options: GlobalEventsOptions) -> Result<GlobalEventStream> {
        if !self.inner.native_socket {
            return Err(Error::new(
                ErrorKind::UnsupportedTransport,
                "global_events requires a native socket connection; custom transports cannot be cloned into per-session child connections",
            ));
        }
        if self.is_closed() {
            return Err(closed_error());
        }
        if options.max_buffered_events == 0 {
            return Err(Error::new(
                ErrorKind::InvalidOption,
                "max_buffered_events must be positive",
            ));
        }

        let (tx, rx) = sync_channel(options.max_buffered_events);
        let control = Arc::new(GlobalEventControl {
            stopped: AtomicBool::new(false),
            terminal_error: Mutex::new(None),
            children: Mutex::new(HashMap::new()),
            tx,
            max_buffered_events: options.max_buffered_events,
            wake_lock: Mutex::new(()),
            wake: Condvar::new(),
        });
        let parent = self.clone();
        let discovery_control = Arc::clone(&control);
        let discovery = std::thread::spawn(move || {
            discover_global_sessions(parent, discovery_control, options.discovery_interval);
        });
        Ok(GlobalEventStream {
            rx,
            control,
            discovery: Some(discovery),
        })
    }

    // --- Curated surface -----------------------------------------------------

    pub fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        match self
            .request_ok(ApiRequest::ListSessions {
                include_archived: false,
                limit: None,
            })?
            .event
        {
            ApiEvent::Sessions { sessions } => Ok(sessions),
            other => Err(unexpected("sessions", &other)),
        }
    }

    /// List only the newest persisted sessions, for bounded dashboard startup.
    pub fn list_sessions_limited(&self, limit: u32) -> Result<Vec<SessionInfo>> {
        match self
            .request_ok(ApiRequest::ListSessions {
                include_archived: false,
                limit: Some(limit),
            })?
            .event
        {
            ApiEvent::Sessions { sessions } => Ok(sessions),
            other => Err(unexpected("sessions", &other)),
        }
    }

    /// Reversibly hide a session from the default list. Its transcript remains
    /// available for a later restore.
    pub fn archive_session(&self, session_id: &str) -> Result<()> {
        self.request_ok(ApiRequest::ArchiveSession {
            session_id: session_id.to_string(),
        })
        .map(drop)
    }

    /// Put an archived session back in the default session list.
    pub fn restore_session(&self, session_id: &str) -> Result<()> {
        self.request_ok(ApiRequest::RestoreSession {
            session_id: session_id.to_string(),
        })
        .map(drop)
    }

    /// Automatically archive inactive sessions after `archive_after_days`.
    /// `None` disables automatic archival.
    pub fn set_retention_policy(&self, archive_after_days: Option<u32>) -> Result<()> {
        self.request_ok(ApiRequest::SetRetentionPolicy { archive_after_days })
            .map(drop)
    }

    pub fn create_session(&self, working_dir: Option<String>) -> Result<SessionInfo> {
        match self
            .request_ok(ApiRequest::CreateSession { working_dir })?
            .event
        {
            ApiEvent::Attached { session } => Ok(session),
            other => Err(unexpected("attached", &other)),
        }
    }

    pub fn attach_session(&self, session_id: &str) -> Result<SessionInfo> {
        match self
            .request_ok(ApiRequest::AttachSession {
                session_id: session_id.to_string(),
            })?
            .event
        {
            ApiEvent::Attached { session } => Ok(session),
            other => Err(unexpected("attached", &other)),
        }
    }

    /// Clone a session's complete persisted context into a new idle session.
    pub fn fork_session(&self, session_id: &str) -> Result<SessionInfo> {
        match self
            .request_ok(ApiRequest::ForkSession {
                session_id: session_id.to_string(),
            })?
            .event
        {
            ApiEvent::SessionForked { session } => Ok(session),
            other => Err(unexpected("session_forked", &other)),
        }
    }

    pub fn detach_session(&self, session_id: &str) -> Result<()> {
        self.request_ok(ApiRequest::DetachSession {
            session_id: session_id.to_string(),
        })
        .map(drop)
    }

    /// Send a user message.
    ///
    /// The harness does not reply to `send_message` at the request level: it
    /// acknowledges by emitting `message_accepted` once the agent has the
    /// message. Waiting for a reply here would always time out, so the frame
    /// is written and the acknowledgement event is awaited instead. Pass a
    /// `None` timeout for pure fire-and-forget.
    pub fn send_message(
        &self,
        session_id: &str,
        content: &str,
        images: Vec<(String, String)>,
        wait_for_accept: Option<Duration>,
    ) -> Result<()> {
        // Subscribe *before* writing: an ack that lands between the write and
        // the subscribe would otherwise be missed and time out a healthy turn.
        let stream = wait_for_accept.map(|_| self.events(Some(session_id)));
        self.notify(ApiRequest::SendMessage {
            session_id: session_id.to_string(),
            content: content.to_string(),
            images,
            no_reply: false,
        })?;
        if let (Some(stream), Some(timeout)) = (stream, wait_for_accept) {
            let deadline = std::time::Instant::now() + timeout;
            while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
                match stream.next_timeout(remaining) {
                    // Returning on timeout rather than failing keeps a missing
                    // ack from failing an otherwise healthy turn: the stream is
                    // the source of truth.
                    None => break,
                    Some(ApiEvent::MessageAccepted { .. }) => break,
                    Some(_) => continue,
                }
            }
        }
        Ok(())
    }

    pub fn cancel(&self, session_id: &str) -> Result<()> {
        self.request_ok(ApiRequest::Cancel {
            session_id: session_id.to_string(),
        })
        .map(drop)
    }

    pub fn soft_interrupt(&self, session_id: &str, content: &str, urgent: bool) -> Result<()> {
        self.request_ok(ApiRequest::SoftInterrupt {
            session_id: session_id.to_string(),
            content: content.to_string(),
            urgent,
        })
        .map(drop)
    }

    pub fn get_history(&self, session_id: &str) -> Result<Vec<HistoryMessage>> {
        self.get_history_with_images(session_id)
            .map(|(messages, _)| messages)
    }

    pub fn get_history_with_images(
        &self,
        session_id: &str,
    ) -> Result<(Vec<HistoryMessage>, Vec<jcode_harness_api::RenderedImage>)> {
        match self
            .request_ok(ApiRequest::GetHistory {
                session_id: session_id.to_string(),
            })?
            .event
        {
            ApiEvent::History {
                messages, images, ..
            } => Ok((messages, images)),
            other => Err(unexpected("history", &other)),
        }
    }

    pub fn peek_session(
        &self,
        session_id: &str,
        limit: Option<u32>,
    ) -> Result<Vec<HistoryMessage>> {
        match self
            .request_ok(ApiRequest::PeekSession {
                session_id: session_id.to_string(),
                limit,
            })?
            .event
        {
            ApiEvent::History { messages, .. } => Ok(messages),
            other => Err(unexpected("history", &other)),
        }
    }

    pub fn clear(&self, session_id: &str) -> Result<()> {
        self.request_ok(ApiRequest::Clear {
            session_id: session_id.to_string(),
        })
        .map(drop)
    }

    pub fn rewind(&self, session_id: &str, message_index: usize) -> Result<()> {
        self.request_ok(ApiRequest::Rewind {
            session_id: session_id.to_string(),
            message_index,
        })
        .map(drop)
    }

    pub fn respond_to_permission(
        &self,
        session_id: &str,
        request_id: &str,
        decision: PermissionDecision,
    ) -> Result<()> {
        self.request_ok(ApiRequest::PermissionResponse {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            decision,
        })
        .map(drop)
    }

    /// Models this session can switch to, and which one is serving it.
    pub fn list_models(&self, session_id: &str) -> Result<(Vec<String>, Option<String>)> {
        match self
            .request_ok(ApiRequest::ListModels {
                session_id: session_id.to_string(),
            })?
            .event
        {
            ApiEvent::Models {
                models, current, ..
            } => Ok((models, current)),
            other => Err(unexpected("models", &other)),
        }
    }

    /// Runtime identity, route catalog, protocol metadata, and a live health
    /// check for a session.
    pub fn get_runtime_info(&self, session_id: &str) -> Result<RuntimeInfo> {
        self.ping()?;
        match self
            .request_ok(ApiRequest::GetRuntimeInfo {
                session_id: session_id.to_string(),
            })?
            .event
        {
            ApiEvent::RuntimeInfo {
                session_id,
                provider,
                model,
                reasoning_effort,
                routes,
            } => {
                let mut providers = Vec::new();
                if let Some(provider) = provider.as_ref() {
                    providers.push(provider.clone());
                }
                for route in &routes {
                    if !providers.contains(&route.provider) {
                        providers.push(route.provider.clone());
                    }
                }
                Ok(RuntimeInfo {
                    server: self.server.clone(),
                    protocol_version: API_VERSION_MAJOR,
                    capabilities: self.capabilities.clone(),
                    healthy: true,
                    session_id,
                    provider,
                    model,
                    reasoning_effort,
                    providers,
                    routes,
                })
            }
            other => Err(unexpected("runtime_info", &other)),
        }
    }

    /// Persist an API key in jcode's owner-only provider store and hot-reload
    /// provider credentials.
    pub fn set_api_key(&self, provider: &str, api_key: &str) -> Result<()> {
        match self
            .request_ok(ApiRequest::SetApiKey {
                provider: provider.to_string(),
                api_key: api_key.to_string(),
            })?
            .event
        {
            ApiEvent::CredentialUpdated { .. } => Ok(()),
            other => Err(unexpected("credential_updated", &other)),
        }
    }

    /// Remove a persisted API-key credential and hot-reload provider
    /// credentials.
    pub fn clear_api_key(&self, provider: &str) -> Result<()> {
        match self
            .request_ok(ApiRequest::ClearApiKey {
                provider: provider.to_string(),
            })?
            .event
        {
            ApiEvent::CredentialUpdated { .. } => Ok(()),
            other => Err(unexpected("credential_updated", &other)),
        }
    }

    /// Read one UTF-8 file under the session working directory.
    pub fn read_file(
        &self,
        session_id: &str,
        path: &str,
        max_bytes: Option<u64>,
    ) -> Result<FileContent> {
        match self
            .request_ok(ApiRequest::ReadFile {
                session_id: session_id.to_string(),
                path: path.to_string(),
                max_bytes,
            })?
            .event
        {
            ApiEvent::FileContent {
                path,
                content,
                size,
                truncated,
                ..
            } => Ok(FileContent {
                path,
                content,
                size,
                truncated,
            }),
            other => Err(unexpected("file_content", &other)),
        }
    }

    /// Find files by case-insensitive path substring under the session root.
    pub fn find_files(
        &self,
        session_id: &str,
        query: &str,
        limit: Option<u32>,
    ) -> Result<Vec<String>> {
        match self
            .request_ok(ApiRequest::FindFiles {
                session_id: session_id.to_string(),
                query: query.to_string(),
                limit,
            })?
            .event
        {
            ApiEvent::Files { paths, .. } => Ok(paths),
            other => Err(unexpected("files", &other)),
        }
    }

    /// Search UTF-8 files under the session root for a literal string.
    pub fn search_text(
        &self,
        session_id: &str,
        query: &str,
        options: SearchTextOptions,
    ) -> Result<Vec<TextMatch>> {
        match self
            .request_ok(ApiRequest::SearchText {
                session_id: session_id.to_string(),
                query: query.to_string(),
                path: options.path,
                limit: options.limit,
            })?
            .event
        {
            ApiEvent::TextMatches { matches, .. } => Ok(matches),
            other => Err(unexpected("text_matches", &other)),
        }
    }

    /// Read safe filesystem metadata for a path under the session root.
    pub fn file_status(&self, session_id: &str, path: &str) -> Result<FileStatus> {
        match self
            .request_ok(ApiRequest::FileStatus {
                session_id: session_id.to_string(),
                path: path.to_string(),
            })?
            .event
        {
            ApiEvent::FileStatus {
                path,
                exists,
                kind,
                size,
                modified_ms,
                ..
            } => Ok(FileStatus {
                path,
                exists,
                kind,
                size,
                modified_ms,
            }),
            other => Err(unexpected("file_status", &other)),
        }
    }

    /// Switch the session to a different model. `model` is an id from
    /// `list_models`.
    pub fn set_model(&self, session_id: &str, model: &str) -> Result<()> {
        self.request_ok(ApiRequest::SetModel {
            session_id: session_id.to_string(),
            model: model.to_string(),
        })
        .map(drop)
    }

    /// Set how much the model deliberates before answering. The accepted set
    /// is per-provider, so this takes a string rather than a union that would
    /// go stale.
    pub fn set_reasoning_effort(&self, session_id: &str, effort: &str) -> Result<()> {
        self.request_ok(ApiRequest::SetReasoningEffort {
            session_id: session_id.to_string(),
            effort: effort.to_string(),
        })
        .map(drop)
    }

    /// Schedule compaction of the transcript so far, freeing context. Not
    /// synchronous: returning means the request was accepted.
    pub fn compact(&self, session_id: &str) -> Result<String> {
        match self
            .request_ok(ApiRequest::Compact {
                session_id: session_id.to_string(),
            })?
            .event
        {
            ApiEvent::Compacted { message, .. } => Ok(message),
            other => Err(unexpected("compacted", &other)),
        }
    }

    /// Set a session's title. `None` restores the generated one.
    pub fn rename_session(&self, session_id: &str, title: Option<String>) -> Result<()> {
        self.request_ok(ApiRequest::RenameSession {
            session_id: session_id.to_string(),
            title,
        })
        .map(drop)
    }

    /// Restore the history the last `rewind` removed.
    pub fn rewind_undo(&self, session_id: &str) -> Result<()> {
        self.request_ok(ApiRequest::RewindUndo {
            session_id: session_id.to_string(),
        })
        .map(drop)
    }

    /// Drop soft interrupts that are queued but not yet delivered.
    pub fn cancel_soft_interrupts(&self, session_id: &str) -> Result<()> {
        self.request_ok(ApiRequest::CancelSoftInterrupts {
            session_id: session_id.to_string(),
        })
        .map(drop)
    }

    pub fn ping(&self) -> Result<()> {
        self.request_ok(ApiRequest::Ping).map(drop)
    }

    /// Send a message and collect the assistant reply until the turn ends.
    ///
    /// The convenience path for scripts: one call in, the text and tool calls
    /// of one turn out. Streaming consumers should use `events()` instead.
    pub fn run(&self, session_id: &str, content: &str, options: RunOptions) -> Result<TurnResult> {
        let stream = self.events(Some(session_id));
        self.send_message(
            session_id,
            content,
            options.images.clone(),
            Some(Duration::from_secs(10)),
        )?;
        let mut result = TurnResult::default();
        while let Some(event) = stream.next() {
            if let Some(on_event) = &options.on_event {
                on_event(&event);
            }
            match event {
                ApiEvent::TextDelta { text, .. } => result.text.push_str(&text),
                ApiEvent::ReasoningDelta { text, .. } => result.reasoning.push_str(&text),
                ApiEvent::ToolDone {
                    call_id,
                    name,
                    output,
                    error,
                    ..
                } => result.tool_calls.push(ToolCall {
                    call_id,
                    name,
                    output,
                    error,
                }),
                ApiEvent::TokenUsage {
                    input,
                    output,
                    cache_read_input,
                    ..
                } => {
                    result.usage = Some(Usage {
                        input,
                        output,
                        cache_read_input,
                    })
                }
                ApiEvent::PermissionRequest { request_id, .. } if options.auto_approve => {
                    self.respond_to_permission(session_id, &request_id, PermissionDecision::Allow)?;
                }
                ApiEvent::TurnDone { .. } => return Ok(result),
                ApiEvent::Error { code, message } => {
                    return Err(Error::new(ErrorKind::Harness(code), message));
                }
                _ => {}
            }
        }
        Err(closed_error())
    }

    /// Whether the connection has been closed or lost.
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Relaxed)
    }
}

/// Options for a one-shot turn.
#[derive(Default)]
pub struct RunOptions {
    pub images: Vec<(String, String)>,
    /// Called for every event of the turn, for progress rendering.
    #[allow(clippy::type_complexity)]
    pub on_event: Option<Box<dyn Fn(&ApiEvent) + Send>>,
    /// Auto-answer permission prompts. Only meaningful when the server
    /// advertises the `permissions` capability.
    pub auto_approve: bool,
}

/// Runtime identity and protocol metadata returned by [`JcodeClient::get_runtime_info`].
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeInfo {
    pub server: String,
    pub protocol_version: u32,
    pub capabilities: Vec<String>,
    pub healthy: bool,
    pub session_id: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    /// Reasoning effort, e.g. `high`, when the provider exposes it.
    pub reasoning_effort: Option<String>,
    pub providers: Vec<String>,
    pub routes: Vec<ModelRouteInfo>,
}

/// Content and truncation metadata returned by [`JcodeClient::read_file`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileContent {
    pub path: String,
    pub content: String,
    pub size: u64,
    pub truncated: bool,
}

/// Optional constraints for [`JcodeClient::search_text`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchTextOptions {
    /// Restrict the search to this relative path under the session root.
    pub path: Option<String>,
    /// Maximum number of matches to return.
    pub limit: Option<u32>,
}

/// Safe filesystem metadata returned by [`JcodeClient::file_status`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStatus {
    pub path: String,
    pub exists: bool,
    pub kind: String,
    pub size: Option<u64>,
    pub modified_ms: Option<u64>,
}

/// What one turn produced.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TurnResult {
    pub text: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    pub output: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read_input: Option<u64>,
}

fn discover_global_sessions(
    parent: JcodeClient,
    control: Arc<GlobalEventControl>,
    interval: Duration,
) {
    loop {
        if control.stopped.load(Ordering::Acquire) {
            return;
        }
        let sessions = match parent
            .request_ok(ApiRequest::ListSessions {
                include_archived: true,
                limit: None,
            })
            .and_then(|frame| match frame.event {
                ApiEvent::Sessions { sessions } => Ok(sessions),
                other => Err(unexpected("sessions", &other)),
            }) {
            Ok(sessions) => sessions,
            Err(error) => {
                stop_global_stream(&control, Some(error));
                return;
            }
        };

        for session in sessions {
            if control.stopped.load(Ordering::Acquire) {
                return;
            }
            start_global_child(&parent, &control, session.session_id);
        }

        if interval.is_zero() {
            return;
        }
        let Ok(guard) = control.wake_lock.lock() else {
            stop_global_stream(
                &control,
                Some(Error::new(
                    ErrorKind::Transport,
                    "global event lock poisoned",
                )),
            );
            return;
        };
        let _ = control.wake.wait_timeout(guard, interval);
    }
}

fn start_global_child(parent: &JcodeClient, control: &Arc<GlobalEventControl>, session_id: String) {
    if control
        .children
        .lock()
        .map(|children| children.contains_key(&session_id))
        .unwrap_or(true)
    {
        return;
    }

    let child = match JcodeClient::connect(ConnectOptions {
        socket_path: Some(parent.inner.socket_path.clone()),
        client_name: format!("{}/global-events", parent.inner.client_name),
        request_timeout: parent.inner.request_timeout,
        ensure_runtime: false,
    }) {
        Ok(child) => child,
        Err(error) => {
            stop_global_stream(control, Some(error));
            return;
        }
    };
    let stream = child.events(Some(&session_id));
    if let Err(error) = child.attach_session(&session_id) {
        if !matches!(
            error.kind,
            ErrorKind::Harness(jcode_harness_api::ErrorCode::UnknownSession)
        ) {
            stop_global_stream(control, Some(error));
        }
        return;
    }
    if control.stopped.load(Ordering::Acquire) {
        return;
    }
    if let Ok(mut children) = control.children.lock() {
        children.insert(session_id.clone(), child);
    } else {
        stop_global_stream(
            control,
            Some(Error::new(
                ErrorKind::Transport,
                "global event lock poisoned",
            )),
        );
        return;
    }

    let pump_control = Arc::clone(control);
    std::thread::spawn(move || {
        while let Some(event) = stream.next() {
            if pump_control.stopped.load(Ordering::Acquire) {
                break;
            }
            match pump_control.tx.try_send(event) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    stop_global_stream(
                        &pump_control,
                        Some(Error::new(
                            ErrorKind::EventBufferOverflow,
                            format!(
                                "global_events consumer fell behind {} buffered events",
                                pump_control.max_buffered_events
                            ),
                        )),
                    );
                    break;
                }
                Err(TrySendError::Disconnected(_)) => {
                    stop_global_stream(&pump_control, None);
                    break;
                }
            }
        }
        if let Ok(mut children) = pump_control.children.lock() {
            children.remove(&session_id);
        }
    });
}

/// The reader thread: correlates replies, fans stream events out.
fn spawn_reader(inner: Arc<Inner>, mut reader: Box<dyn BufRead + Send>) {
    std::thread::spawn(move || {
        loop {
            let frame: ServerFrame = match read_frame(&mut reader) {
                Ok(frame) => frame,
                Err(_) => break,
            };
            // Unknown kinds are skipped silently, per the protocol's
            // forward-compatibility rule.
            if matches!(frame.event, ApiEvent::Unknown) {
                continue;
            }
            if let Some(reply_to) = frame.reply_to {
                let waiter = inner
                    .pending
                    .lock()
                    .ok()
                    .and_then(|mut pending| pending.remove(&reply_to));
                if let Some(waiter) = waiter {
                    let _ = waiter.send(frame);
                    continue;
                }
            }
            // Not a reply anyone is waiting on: a stream event. Dead
            // subscriptions are dropped here as well as in `EventStream::drop`,
            // so a leaked stream cannot grow the fan-out list forever.
            if let Ok(mut subs) = inner.subscribers.lock() {
                let session = event_session(&frame.event);
                subs.retain(|(_, filter, sink)| {
                    match (filter, session) {
                        // A filtered subscription only wants its own session's
                        // events. Events that name *no* session still go to it:
                        // `error` is the one that matters, since the harness
                        // sends it instead of `turn_done`, and dropping it
                        // leaves a turn waiting forever for an end that will
                        // never come.
                        (Some(want), Some(got)) if want != got => true,
                        _ => sink.send(frame.event.clone()).is_ok(),
                    }
                });
            }
        }
        inner.closed.store(true, Ordering::Relaxed);
        // Fail everything in flight rather than leaving callers blocked on a
        // reply that can never arrive.
        if let Ok(mut pending) = inner.pending.lock() {
            pending.clear();
        }
        if let Ok(mut subs) = inner.subscribers.lock() {
            subs.clear();
        }
    });
}

/// The session an event belongs to, when it names one.
fn event_session(event: &ApiEvent) -> Option<&str> {
    use ApiEvent::*;
    match event {
        TextDelta { session_id, .. }
        | ReasoningDelta { session_id, .. }
        | ReasoningDone { session_id, .. }
        | ToolStart { session_id, .. }
        | ToolInputDelta { session_id, .. }
        | ToolExec { session_id, .. }
        | ToolDone { session_id, .. }
        | TokenUsage { session_id, .. }
        | TurnDone { session_id, .. }
        | BackgroundProgress { session_id, .. }
        | MessageAccepted { session_id, .. }
        | PermissionRequest { session_id, .. }
        | SessionStatus { session_id, .. }
        | ModelInfo { session_id, .. }
        | Models { session_id, .. }
        | Compacted { session_id, .. }
        | SessionRenamed { session_id, .. }
        | History { session_id, .. } => Some(session_id.as_str()),
        Attached { session } => Some(session.session_id.as_str()),
        _ => None,
    }
}

fn closed_error() -> Error {
    Error::new(ErrorKind::Disconnected, "harness connection closed")
}

fn poisoned<T>(_: std::sync::PoisonError<T>) -> Error {
    Error::new(ErrorKind::Transport, "harness client state was poisoned")
}

fn unexpected(want: &str, got: &ApiEvent) -> Error {
    Error::new(
        ErrorKind::UnexpectedReply,
        format!("expected {want}, got {got:?}"),
    )
}

/// The wire name of a request, for error text.
fn request_name(request: &ApiRequest) -> String {
    serde_json::to_value(request)
        .ok()
        .and_then(|value| {
            value
                .get("req")
                .and_then(|r| r.as_str().map(str::to_string))
        })
        .unwrap_or_else(|| "request".to_string())
}
