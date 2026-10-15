//! Private, invisible Chrome instance for ChatGPT web turns.
//!
//! Prompts never run in the user's own browser. jcode keeps a dedicated
//! profile under its data directory (`~/.jcode/chatgpt-web/profile` by
//! default) and drives a headless Chrome or Chromium over the DevTools pipe.
//! A visible window opens only when ChatGPT is signed out, so the user can
//! sign in. jcode closes it and continues headless.
//!
//! Portable by design: no TCP ports, no browser extension, no particular
//! browser vendor or install path. Works on Linux, macOS and WSL2 (WSLg or any
//! X11/Wayland display for the sign-in window). On WSL the Linux build of
//! Chrome or Chromium is required, Windows browsers are never driven.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, oneshot};

pub const CHATGPT_WEB_URL: &str = "https://chatgpt.com/?temporary-chat=true";
const SIGN_IN_URL: &str = "https://chatgpt.com/";
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const PAGE_ACCESS_TIMEOUT: Duration = Duration::from_secs(45);

/// What a freshly loaded ChatGPT page shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageAccess {
    SignedIn,
    SignedOut,
    Loading,
}

/// Classifies [`ACCESS_PROBE_SCRIPT`] output. The composer and the model
/// picker together prove a usable signed-in session. Visible Log in or Sign up
/// buttons on a loaded page mean signed out.
pub fn classify_access(probe: &Value) -> PageAccess {
    let flag = |key: &str| probe.get(key).and_then(Value::as_bool) == Some(true);
    if flag("composer") && flag("picker") && !flag("login") {
        PageAccess::SignedIn
    } else if flag("login") && flag("ready") {
        PageAccess::SignedOut
    } else {
        PageAccess::Loading
    }
}

pub const ACCESS_PROBE_SCRIPT: &str = r#"(() => {
  const visible = e => !!e && e.getClientRects().length > 0;
  return {
    ready: document.readyState === 'complete',
    composer: !!document.querySelector('[contenteditable=true][data-composer-markdown], [contenteditable=true][aria-label="Chat with ChatGPT"]'),
    picker: Array.from(document.querySelectorAll('button[aria-label="Select ChatGPT model"]')).some(visible),
    login: Array.from(document.querySelectorAll('button,a')).some(e => visible(e) && /^(log in|sign up)$/i.test((e.innerText || '').trim()))
  };
})()"#;

/// Dedicated browser profile. Holds the ChatGPT session cookies, so it is
/// created owner-only and never shared with the user's own browser.
pub fn profile_dir() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?
        .join("chatgpt-web")
        .join("profile"))
}

/// Chrome or Chromium for the private instance.
/// `JCODE_CHATGPT_WEB_CHROME` overrides discovery.
pub fn find_chrome() -> Option<PathBuf> {
    find_chrome_in(
        std::env::var_os("JCODE_CHATGPT_WEB_CHROME").map(PathBuf::from),
        std::env::var_os("PATH"),
        APP_CANDIDATES,
        dirs::home_dir(),
    )
}

const PATH_CANDIDATES: &[&str] = &[
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "chrome",
];

#[cfg(target_os = "macos")]
const APP_CANDIDATES: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
];
#[cfg(not(target_os = "macos"))]
const APP_CANDIDATES: &[&str] = &[];

fn find_chrome_in(
    explicit: Option<PathBuf>,
    path: Option<std::ffi::OsString>,
    apps: &[&str],
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(explicit) = explicit {
        return explicit.is_file().then_some(explicit);
    }
    let on_path = path.as_ref().and_then(|path| {
        PATH_CANDIDATES.iter().find_map(|name| {
            std::env::split_paths(path)
                // On WSL, PATH usually includes Windows directories. Windows
                // executables cannot use the DevTools pipe, so never pick them.
                .filter(|dir| !is_windows_mount(dir))
                .map(|dir| dir.join(name))
                .find(|candidate| candidate.is_file())
        })
    });
    if on_path.is_some() {
        return on_path;
    }
    if let Some(app) = apps.iter().map(PathBuf::from).find(|app| app.is_file()) {
        return Some(app);
    }
    // Playwright's downloaded Chromium (`npx playwright install chromium`),
    // newest build first. Linux and macOS layouts.
    let cache = home?.join(".cache").join("ms-playwright");
    let mut builds: Vec<(u32, PathBuf)> = std::fs::read_dir(&cache)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let build = name.strip_prefix("chromium-")?.parse::<u32>().ok()?;
            [
                "chrome-linux64/chrome",
                "chrome-linux/chrome",
                "chrome-mac/Chromium.app/Contents/MacOS/Chromium",
                "chrome-mac-arm64/Chromium.app/Contents/MacOS/Chromium",
            ]
            .iter()
            .map(|relative| entry.path().join(relative))
            .find(|binary| binary.is_file())
            .map(|binary| (build, binary))
        })
        .collect();
    builds.sort();
    builds.pop().map(|(_, binary)| binary)
}

/// `/mnt/<drive>/...` on WSL.
fn is_windows_mount(dir: &Path) -> bool {
    let mut components = dir.components();
    matches!(components.next(), Some(std::path::Component::RootDir))
        && components.next().map(|c| c.as_os_str()) == Some(std::ffi::OsStr::new("mnt"))
        && components
            .next()
            .and_then(|c| c.as_os_str().to_str())
            .is_some_and(|drive| drive.len() == 1 && drive.chars().all(|c| c.is_ascii_alphabetic()))
}

/// Whether a visible sign-in window can be shown in this session.
pub fn display_available() -> bool {
    display_available_with(
        cfg!(target_os = "macos"),
        std::env::var_os("DISPLAY"),
        std::env::var_os("WAYLAND_DISPLAY"),
    )
}

fn display_available_with(
    macos: bool,
    display: Option<std::ffi::OsString>,
    wayland: Option<std::ffi::OsString>,
) -> bool {
    let set = |value: Option<std::ffi::OsString>| value.is_some_and(|value| !value.is_empty());
    macos || set(display) || set(wayland)
}

/// Command-line arguments for the private instance. Headless runs never show
/// a window. The visible variant is used only for signing in.
pub fn chrome_args(profile: &Path, headless: bool) -> Vec<String> {
    let mut args = vec![
        format!("--user-data-dir={}", profile.display()),
        "--remote-debugging-pipe".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--disable-features=Translate,MediaRouter".to_string(),
        "--disable-background-timer-throttling".to_string(),
        "--disable-renderer-backgrounding".to_string(),
        "--disable-backgrounding-occluded-windows".to_string(),
        "--password-store=basic".to_string(),
    ];
    if headless {
        args.push("--headless=new".to_string());
        args.push("--window-size=1280,900".to_string());
    }
    args.push("about:blank".to_string());
    args
}

type Pending = Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;

/// DevTools protocol client over Chrome's `--remote-debugging-pipe` (fd 3 in,
/// fd 4 out, NUL-delimited JSON). No port is opened, so other local programs
/// cannot reach the signed-in session, and Chrome exits when jcode exits.
pub struct Cdp {
    outgoing: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    pending: Pending,
    next_id: AtomicU64,
    closed: Arc<AtomicBool>,
}

impl Cdp {
    fn spawn_io<W, R>(mut writer: W, reader: R) -> Self
    where
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let (outgoing, mut outgoing_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let pending: Pending = Default::default();
        let closed = Arc::new(AtomicBool::new(false));
        tokio::spawn(async move {
            while let Some(mut frame) = outgoing_rx.recv().await {
                frame.push(0);
                if writer.write_all(&frame).await.is_err() || writer.flush().await.is_err() {
                    break;
                }
            }
        });
        let reader_pending = Arc::clone(&pending);
        let reader_closed = Arc::clone(&closed);
        tokio::spawn(async move {
            let mut reader = tokio::io::BufReader::new(reader);
            let mut frame = Vec::new();
            loop {
                frame.clear();
                match reader.read_until(0, &mut frame).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if frame.last() == Some(&0) {
                    frame.pop();
                }
                let Ok(value) = serde_json::from_slice::<Value>(&frame) else {
                    continue;
                };
                let Some(id) = value.get("id").and_then(Value::as_u64) else {
                    continue;
                };
                let reply = reader_pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                if let Some(reply) = reply {
                    let result = match value.get("error") {
                        Some(error) => Err(anyhow::anyhow!("DevTools error: {error}")),
                        None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    let _ = reply.send(result);
                }
            }
            reader_closed.store(true, Ordering::SeqCst);
            for (_, reply) in reader_pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .drain()
            {
                let _ = reply.send(Err(anyhow::anyhow!(
                    "jcode's private ChatGPT browser closed its DevTools connection"
                )));
            }
        });
        Self {
            outgoing,
            pending,
            next_id: AtomicU64::new(1),
            closed,
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub async fn send(&self, method: &str, params: Value, session: Option<&str>) -> Result<Value> {
        self.send_with_timeout(method, params, session, COMMAND_TIMEOUT)
            .await
    }

    async fn send_with_timeout(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
        timeout: Duration,
    ) -> Result<Value> {
        anyhow::ensure!(
            !self.is_closed(),
            "jcode's private ChatGPT browser is not running"
        );
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (reply, response) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, reply);
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        if self
            .outgoing
            .send(message.to_string().into_bytes())
            .is_err()
        {
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            anyhow::bail!("jcode's private ChatGPT browser is not running");
        }
        match tokio::time::timeout(timeout, response).await {
            Ok(Ok(result)) => result.with_context(|| format!("DevTools {method} failed")),
            Ok(Err(_)) => anyhow::bail!("DevTools {method} lost its reply"),
            Err(_) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                anyhow::bail!("DevTools {method} timed out")
            }
        }
    }
}

/// Cross-process lock on the private profile. Chrome allows one process per
/// profile, so concurrent jcode processes (daemon and CLI) take turns.
struct ProfileLock {
    _file: std::fs::File,
}

/// Marker another jcode process writes while it waits for the profile. An
/// idle holder sees it and hands the profile over instead of keeping it until
/// its idle timeout.
fn handoff_request_path(profile: &Path) -> PathBuf {
    profile.parent().unwrap_or(profile).join("profile.wanted")
}

fn handoff_requested(profile: &Path) -> bool {
    handoff_request_path(profile).exists()
}

impl ProfileLock {
    #[cfg(unix)]
    async fn acquire(profile: &Path) -> Result<Self> {
        use std::os::unix::io::AsRawFd;
        let parent = profile.parent().unwrap_or(profile);
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(parent.join("profile.lock"))
            .context("Failed to open the ChatGPT web profile lock")?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15 * 60);
        let mut requested = false;
        let result = loop {
            // SAFETY: flock on an owned, open file descriptor.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                break Ok(Self { _file: file });
            }
            if !requested {
                // Ask an idle holder (for example the daemon) to hand over.
                let _ = std::fs::write(
                    handoff_request_path(profile),
                    std::process::id().to_string(),
                );
                requested = true;
            }
            if tokio::time::Instant::now() >= deadline {
                break Err(anyhow::anyhow!(
                    "Another jcode process kept ChatGPT web busy for 15 minutes"
                ));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        };
        if requested {
            let _ = std::fs::remove_file(handoff_request_path(profile));
        }
        result
    }
}

/// A running private Chrome process with its DevTools connection.
pub struct Instance {
    child: tokio::process::Child,
    pub cdp: Cdp,
    pub headless: bool,
    _lock: ProfileLock,
}

impl Instance {
    #[cfg(unix)]
    async fn launch(chrome: &Path, profile: &Path, headless: bool) -> Result<Self> {
        use std::os::unix::io::{AsRawFd, OwnedFd};
        let lock = ProfileLock::acquire(profile).await?;
        std::fs::create_dir_all(profile)
            .with_context(|| format!("Failed to create {}", profile.display()))?;
        {
            use std::os::unix::fs::PermissionsExt;
            let owner_only = std::fs::Permissions::from_mode(0o700);
            let _ = std::fs::set_permissions(profile, owner_only.clone());
            if let Some(parent) = profile.parent() {
                let _ = std::fs::set_permissions(parent, owner_only);
            }
        }
        // Chrome reads commands on fd 3 and writes replies on fd 4.
        // std::io::pipe is close-on-exec on every Unix; macOS has no pipe2.
        let pipe = || -> Result<(OwnedFd, OwnedFd)> {
            let (reader, writer) = std::io::pipe().context("Failed to create a DevTools pipe")?;
            Ok((reader.into(), writer.into()))
        };
        let (to_chrome_read, to_chrome_write) = pipe()?;
        let (from_chrome_read, from_chrome_write) = pipe()?;
        let child_in = to_chrome_read.as_raw_fd();
        let child_out = from_chrome_write.as_raw_fd();
        let mut command = tokio::process::Command::new(chrome);
        command
            .args(chrome_args(profile, headless))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        // SAFETY: only async-signal-safe dup2 calls run between fork and exec.
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(child_in, 3) < 0 || libc::dup2(child_out, 4) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command
            .spawn()
            .with_context(|| format!("Failed to start {}", chrome.display()))?;
        drop(to_chrome_read);
        drop(from_chrome_write);
        let writer = tokio::net::unix::pipe::Sender::from_owned_fd(to_chrome_write)
            .context("Failed to attach the DevTools command pipe")?;
        let reader = tokio::net::unix::pipe::Receiver::from_owned_fd(from_chrome_read)
            .context("Failed to attach the DevTools reply pipe")?;
        let mut instance = Self {
            child,
            cdp: Cdp::spawn_io(writer, reader),
            headless,
            _lock: lock,
        };
        match instance
            .cdp
            .send_with_timeout(
                "Browser.getVersion",
                json!({}),
                None,
                Duration::from_secs(30),
            )
            .await
        {
            Ok(_) => Ok(instance),
            Err(error) => {
                let status = instance.child.try_wait().ok().flatten();
                let _ = instance.child.kill().await;
                Err(error.context(match status {
                    Some(status) => {
                        format!("{} exited during startup ({status})", chrome.display())
                    }
                    None => format!("{} did not answer on its DevTools pipe", chrome.display()),
                }))
            }
        }
    }

    #[cfg(not(unix))]
    async fn launch(_chrome: &Path, _profile: &Path, _headless: bool) -> Result<Self> {
        anyhow::bail!(
            "ChatGPT web models currently need Linux, macOS or WSL2. Native Windows is not supported yet"
        )
    }

    async fn shutdown(mut self) {
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            self.cdp.send("Browser.close", json!({}), None),
        )
        .await;
        if tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
        }
    }

    pub async fn open_page(&self, url: &str) -> Result<Page> {
        let target = self
            .cdp
            .send("Target.createTarget", json!({"url": url}), None)
            .await?;
        let target_id = target
            .get("targetId")
            .and_then(Value::as_str)
            .context("Private browser did not return a target id")?
            .to_string();
        let attached = self
            .cdp
            .send(
                "Target.attachToTarget",
                json!({"targetId": target_id, "flatten": true}),
                None,
            )
            .await;
        let Some(session_id) = attached
            .as_ref()
            .ok()
            .and_then(|value| value.get("sessionId"))
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            let _ = self
                .cdp
                .send("Target.closeTarget", json!({"targetId": target_id}), None)
                .await;
            attached?;
            anyhow::bail!("Private browser did not return a page session");
        };
        let page = Page {
            target_id,
            session_id,
        };
        // Headless pages never lose focus to the user. Make it explicit so
        // focus-dependent UI such as menus behaves like a foreground tab.
        let _ = self
            .cdp
            .send("Page.bringToFront", json!({}), Some(&page.session_id))
            .await;
        let _ = self
            .cdp
            .send(
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled": true}),
                Some(&page.session_id),
            )
            .await;
        Ok(page)
    }

    pub async fn close_page(&self, page: &Page) -> Result<()> {
        self.cdp
            .send(
                "Target.closeTarget",
                json!({"targetId": page.target_id}),
                None,
            )
            .await
            .map(|_| ())
    }

    /// Evaluates an expression in the page main world. DevTools evaluation is
    /// not subject to the page CSP. Thrown errors and rejected promises
    /// surface with the page's message.
    pub async fn evaluate(&self, page: &Page, expression: &str) -> Result<Value> {
        let response = self
            .cdp
            .send(
                "Runtime.evaluate",
                json!({"expression": expression, "awaitPromise": true, "returnByValue": true, "userGesture": true}),
                Some(&page.session_id),
            )
            .await?;
        if let Some(details) = response.get("exceptionDetails") {
            let message = details
                .pointer("/exception/description")
                .and_then(Value::as_str)
                .or_else(|| details.get("text").and_then(Value::as_str))
                .unwrap_or("unknown page error");
            anyhow::bail!(
                "Evaluate error: {}",
                message.lines().next().unwrap_or(message)
            );
        }
        Ok(response
            .pointer("/result/value")
            .cloned()
            .unwrap_or(Value::Null))
    }
}

/// An owned page in the private instance.
pub struct Page {
    pub target_id: String,
    pub session_id: String,
}

static INSTANCE: std::sync::LazyLock<Mutex<Option<Instance>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));
static SIGN_IN: std::sync::LazyLock<Mutex<()>> = std::sync::LazyLock::new(|| Mutex::new(()));
static LAST_USED: std::sync::LazyLock<std::sync::Mutex<tokio::time::Instant>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(tokio::time::Instant::now()));
static IDLE_REAPER: std::sync::Once = std::sync::Once::new();
/// The hidden browser exits after this long without ChatGPT web activity.
const IDLE_SHUTDOWN: Duration = Duration::from_secs(10 * 60);

fn touch() {
    *LAST_USED.lock().unwrap_or_else(|e| e.into_inner()) = tokio::time::Instant::now();
}

fn start_idle_reaper() {
    IDLE_REAPER.call_once(|| {
        tokio::spawn(async {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let idle = LAST_USED
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .elapsed();
                let wanted = profile_dir().is_ok_and(|profile| handoff_requested(&profile));
                if idle < IDLE_SHUTDOWN && !wanted {
                    continue;
                }
                // Only stop an instance nobody is using right now. A busy
                // instance keeps the profile until its turn is done.
                if let Ok(mut guard) = INSTANCE.try_lock()
                    && let Some(instance) = guard.take()
                {
                    instance.shutdown().await;
                    crate::logging::info(if wanted {
                        "Handed the private ChatGPT web browser to another jcode process"
                    } else {
                        "Stopped the idle private ChatGPT web browser"
                    });
                }
            }
        });
    });
}

fn require_chrome() -> Result<PathBuf> {
    find_chrome().context(
        "ChatGPT web models run in a private, invisible Chrome. Install Google Chrome or Chromium for this system (on WSL: the Linux build inside WSL), or set JCODE_CHATGPT_WEB_CHROME to its executable",
    )
}

/// Exclusive access to the headless instance, starting it if needed.
pub async fn headless() -> Result<tokio::sync::MappedMutexGuard<'static, Instance>> {
    let mut guard = INSTANCE.lock().await;
    if guard
        .as_ref()
        .is_some_and(|instance| instance.cdp.is_closed() || !instance.headless)
        && let Some(instance) = guard.take()
    {
        instance.shutdown().await;
    }
    if guard.is_none() {
        *guard = Some(Instance::launch(&require_chrome()?, &profile_dir()?, true).await?);
        start_idle_reaper();
    }
    touch();
    Ok(tokio::sync::MutexGuard::map(guard, |instance| {
        instance.as_mut().expect("instance was just started")
    }))
}

/// Polls a page until ChatGPT shows a signed-in or signed-out state.
pub async fn wait_for_access(instance: &Instance, page: &Page, timeout: Duration) -> PageAccess {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let access = instance
            .evaluate(page, ACCESS_PROBE_SCRIPT)
            .await
            .map(|probe| classify_access(&probe))
            .unwrap_or(PageAccess::Loading);
        if access != PageAccess::Loading || tokio::time::Instant::now() >= deadline {
            return access;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Opens one visible window on the private profile so the user can sign in
/// to ChatGPT, waits until the session works, then closes it. The headless
/// instance stops first because Chrome allows one process per profile.
pub async fn sign_in_interactively() -> Result<()> {
    anyhow::ensure!(
        display_available(),
        "ChatGPT web needs a one-time sign-in, but no display is available for the sign-in window. Run `jcode browser chatgpt-login` from a desktop session (WSLg, X11 or Wayland)"
    );
    let _only_one_window = SIGN_IN.lock().await;
    if let Some(instance) = INSTANCE.lock().await.take() {
        instance.shutdown().await;
    }
    crate::logging::info("ChatGPT web is signed out: opening a visible sign-in window");
    let visible = Instance::launch(&require_chrome()?, &profile_dir()?, false).await?;
    let result = async {
        let page = visible.open_page(SIGN_IN_URL).await?;
        let deadline = tokio::time::Instant::now() + SIGN_IN_TIMEOUT;
        loop {
            let probe = visible
                .evaluate(&page, ACCESS_PROBE_SCRIPT)
                .await
                .unwrap_or(Value::Null);
            if classify_access(&probe) == PageAccess::SignedIn {
                return Ok(());
            }
            anyhow::ensure!(
                !visible.cdp.is_closed(),
                "The ChatGPT sign-in window was closed before sign-in finished"
            );
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "ChatGPT sign-in was not finished within 10 minutes"
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    .await;
    visible.shutdown().await;
    crate::logging::info(&format!(
        "ChatGPT web sign-in window closed ({})",
        if result.is_ok() {
            "signed in"
        } else {
            "not signed in"
        }
    ));
    result
}

/// Whether a signed-out page may open the visible sign-in window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignIn {
    /// A user-initiated chat turn or explicit refresh.
    Interactive,
    /// Background catalog refresh. Never shows a window.
    Never,
}

/// ChatGPT session read from the user's main browser. Held only in memory.
pub struct ExportedSession {
    session_token: String,
    expires_unix: Option<i64>,
    cookies: Vec<(String, String)>,
}

impl std::fmt::Debug for ExportedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportedSession")
            .field("session_token", &"<redacted>")
            .field("expires_unix", &self.expires_unix)
            .field(
                "cookies",
                &self
                    .cookies
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Non-secret ChatGPT cookies copied alongside the session token so the
/// private browser uses the same workspace, region and device identity.
const COPIED_COOKIES: &[&str] = &[
    "oai-did",
    "_account",
    "_account_is_fedramp",
    "_account_residency_region",
];

/// Runs in a chatgpt.com page of the user's main browser. Returns the session
/// token and the copied cookies, or `signedIn: false`.
pub const EXPORT_SESSION_SCRIPT: &str = r#"return (async () => {
  if (location.hostname !== 'chatgpt.com') return { signedIn: false, reason: 'not chatgpt.com' };
  const response = await fetch('/api/auth/session', { credentials: 'include', cache: 'no-store' });
  if (!response.ok) return { signedIn: false, reason: 'session ' + response.status };
  const session = await response.json();
  if (!session || !session.user || typeof session.sessionToken !== 'string' || !session.sessionToken) {
    return { signedIn: false, reason: 'signed out' };
  }
  const names = ['oai-did', '_account', '_account_is_fedramp', '_account_residency_region'];
  const cookies = {};
  for (const part of document.cookie.split(';')) {
    const index = part.indexOf('=');
    const name = part.slice(0, index).trim();
    if (index > 0 && names.includes(name)) cookies[name] = part.slice(index + 1).trim();
  }
  return { signedIn: true, sessionToken: session.sessionToken, expires: session.expires || null, cookies };
})();"#;

impl ExportedSession {
    /// Parses [`EXPORT_SESSION_SCRIPT`] output.
    pub fn parse(value: &Value) -> Result<Self> {
        anyhow::ensure!(
            value.get("signedIn").and_then(Value::as_bool) == Some(true),
            "ChatGPT is not signed in in your main browser ({})",
            value
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        );
        let session_token = value
            .get("sessionToken")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty() && token.len() < 64 * 1024)
            .context("Main browser returned no ChatGPT session token")?
            .to_string();
        anyhow::ensure!(
            session_token.chars().all(|c| c.is_ascii_graphic()),
            "Main browser returned a malformed ChatGPT session token"
        );
        let expires_unix = value
            .get("expires")
            .and_then(Value::as_str)
            .and_then(|expires| chrono::DateTime::parse_from_rfc3339(expires).ok())
            .map(|expires| expires.timestamp());
        let cookies = value
            .get("cookies")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter(|(name, _)| COPIED_COOKIES.contains(&name.as_str()))
            .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_string())))
            .filter(|(_, value)| value.len() < 4096 && value.chars().all(|c| c.is_ascii_graphic()))
            .collect();
        Ok(Self {
            session_token,
            expires_unix,
            cookies,
        })
    }

    /// CDP `Storage.setCookies` entries. Long tokens are split like next-auth
    /// does (`...session-token.0`, `.1`, ...) to stay under cookie size limits.
    pub fn cdp_cookies(&self, now_unix: i64) -> Vec<Value> {
        const CHUNK: usize = 3800;
        let expires = self
            .expires_unix
            .filter(|expires| *expires > now_unix)
            .unwrap_or(now_unix + 30 * 24 * 60 * 60);
        let cookie = |name: String, value: &str, http_only: bool| {
            json!({
                "name": name, "value": value, "domain": ".chatgpt.com", "path": "/",
                "secure": true, "httpOnly": http_only, "sameSite": "Lax", "expires": expires
            })
        };
        let base = "__Secure-next-auth.session-token";
        let mut cookies: Vec<Value> = if self.session_token.len() <= CHUNK {
            vec![cookie(base.to_string(), &self.session_token, true)]
        } else {
            self.session_token
                .as_bytes()
                .chunks(CHUNK)
                .enumerate()
                .map(|(index, chunk)| {
                    // The token is ASCII (checked in `parse`).
                    cookie(
                        format!("{base}.{index}"),
                        std::str::from_utf8(chunk).unwrap_or_default(),
                        true,
                    )
                })
                .collect()
        };
        cookies.extend(
            self.cookies
                .iter()
                .map(|(name, value)| cookie(name.clone(), value, false)),
        );
        cookies
    }
}

/// Reads the ChatGPT session from the user's main browser through the
/// installed Browser Agent Bridge, in an owned background tab that is closed
/// again. Returns `Ok(None)` when no bridge is installed or connected.
pub async fn export_session_from_main_browser() -> Result<Option<ExportedSession>> {
    let binary = crate::browser::browser_binary_path();
    if !binary.is_file() {
        return Ok(None);
    }
    let bridge = |action: &'static str, params: Value| {
        let binary = binary.clone();
        async move {
            let output = tokio::time::timeout(
                Duration::from_secs(45),
                tokio::process::Command::new(&binary)
                    .arg(action)
                    .arg(params.to_string())
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .with_context(|| format!("Browser bridge {action} timed out"))??;
            if !output.status.success() {
                // Only the last stderr line: never echo page content.
                let stderr = String::from_utf8_lossy(&output.stderr);
                let detail = stderr
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty() && !line.contains(": weak version `"))
                    .unwrap_or("failed")
                    .trim()
                    .to_string();
                anyhow::bail!("Browser bridge {action}: {detail}");
            }
            serde_json::from_slice::<Value>(&output.stdout)
                .with_context(|| format!("Browser bridge {action} returned invalid JSON"))
        }
    };
    let Ok(ping) = bridge("ping", json!({})).await else {
        return Ok(None);
    };
    if ping.get("pong").is_none() {
        return Ok(None);
    }
    let source = bridge("getActiveTab", json!({})).await?;
    let source_tab = source
        .get("tabId")
        .and_then(Value::as_u64)
        .context("Browser bridge did not return an active tab")?;
    let fork_name = format!("jcode-chatgpt-auth-{}", uuid::Uuid::new_v4());
    let result = async {
        let fork = bridge(
            "fork",
            json!({"tabId": source_tab, "paths": [{"name": fork_name}]}),
        )
        .await?;
        let tab_id = fork
            .pointer("/forks/0/tabId")
            .and_then(Value::as_u64)
            .context("Browser bridge did not return the owned tab")?;
        bridge(
            "navigate",
            json!({"tabId": tab_id, "url": SIGN_IN_URL, "wait": true}),
        )
        .await?;
        // A fresh tab can briefly reject evaluation (CSP fallback race).
        let mut last_error = None;
        for _ in 0..20 {
            match bridge(
                "evaluate",
                json!({"tabId": tab_id, "script": EXPORT_SESSION_SCRIPT}),
            )
            .await
            {
                Ok(value) => {
                    return ExportedSession::parse(value.get("result").unwrap_or(&Value::Null));
                }
                Err(error) => last_error = Some(error),
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Browser bridge evaluate failed")))
    }
    .await;
    let closed = bridge("killFork", json!({"fork": fork_name})).await;
    if let Err(error) = closed {
        crate::logging::warn(&format!(
            "Failed to close the ChatGPT auth tab in the main browser: {error:#}"
        ));
    }
    result.map(Some)
}

/// Copies the main browser's ChatGPT session into the private instance.
pub async fn import_session(instance: &Instance, session: &ExportedSession) -> Result<()> {
    instance
        .cdp
        .send(
            "Storage.setCookies",
            json!({"cookies": session.cdp_cookies(chrono::Utc::now().timestamp())}),
            None,
        )
        .await
        .context("Failed to copy the ChatGPT session into jcode's private browser")?;
    Ok(())
}

/// Signs the private instance in by copying the session from the user's main
/// browser. Returns `false` when no connected bridge is available.
async fn sign_in_from_main_browser() -> Result<bool> {
    let Some(session) = export_session_from_main_browser().await? else {
        return Ok(false);
    };
    let instance = headless().await?;
    import_session(&instance, &session).await?;
    crate::logging::info(
        "Copied the ChatGPT session from the main browser into jcode's private browser",
    );
    Ok(true)
}

/// Opens an owned, signed-in Temporary Chat page in the invisible instance.
/// When signed out, first copies the session from the user's main browser
/// through the bridge (no window). Only if that is impossible, and the caller
/// allows it, shows the private sign-in window once.
pub async fn open_signed_in_page(
    sign_in: SignIn,
) -> Result<(tokio::sync::MappedMutexGuard<'static, Instance>, Page)> {
    let mut imported = false;
    let mut shown_window = false;
    loop {
        let instance = headless().await?;
        let page = instance.open_page(CHATGPT_WEB_URL).await?;
        match wait_for_access(&instance, &page, PAGE_ACCESS_TIMEOUT).await {
            PageAccess::SignedIn => return Ok((instance, page)),
            PageAccess::SignedOut => {
                let _ = instance.close_page(&page).await;
                drop(instance);
                if !imported {
                    imported = true;
                    match sign_in_from_main_browser().await {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(error) => crate::logging::info(&format!(
                            "Could not copy the ChatGPT session from the main browser: {error:#}"
                        )),
                    }
                }
                anyhow::ensure!(
                    sign_in == SignIn::Interactive && !shown_window,
                    "ChatGPT is signed out. Sign in to chatgpt.com in your browser (with the jcode browser bridge connected), or run `jcode browser chatgpt-login`"
                );
                shown_window = true;
                sign_in_interactively().await?;
            }
            PageAccess::Loading => {
                let _ = instance.close_page(&page).await;
                anyhow::bail!(
                    "ChatGPT did not finish loading in jcode's private browser within {} seconds",
                    PAGE_ACCESS_TIMEOUT.as_secs()
                );
            }
        }
    }
}

/// Reports whether the private profile is signed in, without showing anything.
pub async fn signed_in_status() -> Result<PageAccess> {
    let instance = headless().await?;
    let page = instance.open_page(CHATGPT_WEB_URL).await?;
    let access = wait_for_access(&instance, &page, PAGE_ACCESS_TIMEOUT).await;
    let _ = instance.close_page(&page).await;
    Ok(access)
}

/// Runs one `chatgpt_web_ui.js` operation on an owned page.
pub async fn ui_request(instance: &Instance, page: &Page, request: &Value) -> Result<Value> {
    let op = request
        .get("op")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let script = format!(
        "(async () => {{ const request={request};\n{} }})()",
        super::chatgpt_web::UI_SCRIPT
    );
    instance
        .evaluate(page, &script)
        .await
        .with_context(|| format!("ChatGPT UI step '{op}'"))
}

/// Stops the private instance.
pub async fn shutdown() {
    if let Some(instance) = INSTANCE.lock().await.take() {
        instance.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_args_never_open_a_window_or_a_port() {
        let args = chrome_args(Path::new("/p"), true);
        assert!(args.contains(&"--headless=new".to_string()));
        assert!(args.contains(&"--user-data-dir=/p".to_string()));
        assert!(args.contains(&"--remote-debugging-pipe".to_string()));
        assert!(
            !args
                .iter()
                .any(|arg| arg.starts_with("--remote-debugging-port"))
        );
        let visible = chrome_args(Path::new("/p"), false);
        assert!(!visible.iter().any(|arg| arg.starts_with("--headless")));
    }

    #[test]
    fn access_requires_composer_and_picker_and_detects_sign_out() {
        let signed_in = json!({"ready":true,"composer":true,"picker":true,"login":false});
        assert_eq!(classify_access(&signed_in), PageAccess::SignedIn);
        let signed_out = json!({"ready":true,"composer":true,"picker":false,"login":true});
        assert_eq!(classify_access(&signed_out), PageAccess::SignedOut);
        let loading = json!({"ready":false,"composer":false,"picker":false,"login":true});
        assert_eq!(classify_access(&loading), PageAccess::Loading);
        assert_eq!(classify_access(&Value::Null), PageAccess::Loading);
    }

    #[test]
    fn chrome_discovery_prefers_override_then_path_then_newest_playwright() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let home = temp.path().join("home");
        for (build, layout) in [
            ("chromium-1208", "chrome-linux64/chrome"),
            ("chromium-1243", "chrome-linux/chrome"),
        ] {
            let binary = home.join(".cache/ms-playwright").join(build).join(layout);
            std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
            std::fs::write(&binary, "").unwrap();
        }
        let newest = home.join(".cache/ms-playwright/chromium-1243/chrome-linux/chrome");
        assert_eq!(
            find_chrome_in(None, Some(bin.clone().into()), &[], Some(home.clone())),
            Some(newest)
        );
        std::fs::write(bin.join("chromium"), "").unwrap();
        assert_eq!(
            find_chrome_in(None, Some(bin.clone().into()), &[], Some(home.clone())),
            Some(bin.join("chromium"))
        );
        let explicit = temp.path().join("custom-chrome");
        std::fs::write(&explicit, "").unwrap();
        assert_eq!(
            find_chrome_in(
                Some(explicit.clone()),
                Some(bin.clone().into()),
                &[],
                Some(home.clone())
            ),
            Some(explicit)
        );
        assert_eq!(
            find_chrome_in(
                Some(temp.path().join("missing")),
                Some(bin.into()),
                &[],
                Some(home)
            ),
            None
        );
    }

    #[test]
    fn exported_session_parses_redacts_and_splits_long_tokens_like_next_auth() {
        let token = "a".repeat(4649);
        let probe = json!({
            "signedIn": true, "sessionToken": token, "expires": "2027-01-12T08:43:44.404Z",
            "cookies": {"oai-did": "device", "_account": "workspace", "evil": "x", "_account_residency_region": "bad value"}
        });
        let session = ExportedSession::parse(&probe).unwrap();
        assert!(
            !format!("{session:?}").contains("aaaa"),
            "Debug must never show the token"
        );
        // "Now" is before the token's 2027 expiry, so the real expiry is used.
        let cookies = session.cdp_cookies(1_790_000_000);
        let names: Vec<&str> = cookies
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "__Secure-next-auth.session-token.0",
                "__Secure-next-auth.session-token.1",
                "_account",
                "oai-did"
            ]
        );
        let joined: String = cookies[..2]
            .iter()
            .map(|c| c["value"].as_str().unwrap())
            .collect();
        assert_eq!(joined, token);
        for cookie in &cookies {
            assert_eq!(cookie["domain"], ".chatgpt.com");
            assert_eq!(cookie["secure"], true);
            assert_eq!(cookie["expires"], 1_799_743_424);
        }
        assert_eq!(cookies[0]["httpOnly"], true);
        assert_eq!(cookies[2]["httpOnly"], false);

        let short =
            ExportedSession::parse(&json!({"signedIn": true, "sessionToken": "short"})).unwrap();
        let cookies = short.cdp_cookies(1_800_000_000);
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0]["name"], "__Secure-next-auth.session-token");
        // Missing or past expiry falls back to 30 days.
        assert_eq!(cookies[0]["expires"], 1_800_000_000 + 30 * 24 * 60 * 60);
    }

    #[test]
    fn exported_session_rejects_signed_out_and_malformed_tokens() {
        assert!(
            ExportedSession::parse(&json!({"signedIn": false, "reason": "signed out"})).is_err()
        );
        assert!(ExportedSession::parse(&json!({"signedIn": true})).is_err());
        assert!(ExportedSession::parse(&json!({"signedIn": true, "sessionToken": ""})).is_err());
        assert!(
            ExportedSession::parse(&json!({"signedIn": true, "sessionToken": "has space"}))
                .is_err()
        );
        assert!(
            ExportedSession::parse(&json!({"signedIn": true, "sessionToken": "x".repeat(70_000)}))
                .is_err()
        );
        assert!(ExportedSession::parse(&Value::Null).is_err());
    }

    #[test]
    fn wsl_windows_mounts_are_never_used_as_chrome() {
        assert!(is_windows_mount(Path::new(
            "/mnt/c/Program Files/Google/Chrome/Application"
        )));
        assert!(is_windows_mount(Path::new("/mnt/d")));
        assert!(!is_windows_mount(Path::new("/mnt/wslg/runtime-dir")));
        assert!(!is_windows_mount(Path::new("/usr/bin")));
        assert!(!is_windows_mount(Path::new("mnt/c")));
    }

    #[test]
    fn sign_in_window_needs_a_display_except_on_macos() {
        assert!(display_available_with(true, None, None));
        assert!(display_available_with(false, Some(":0".into()), None));
        assert!(display_available_with(
            false,
            None,
            Some("wayland-0".into())
        ));
        assert!(!display_available_with(false, Some("".into()), None));
        assert!(!display_available_with(false, None, None));
    }

    #[tokio::test]
    async fn devtools_pipe_frames_are_nul_delimited_and_matched_by_id() {
        let (client_end, browser_end) = tokio::io::duplex(4096);
        let (client_read, client_write) = tokio::io::split(client_end);
        let cdp = Cdp::spawn_io(client_write, client_read);
        let browser = tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
            let (read, mut write) = tokio::io::split(browser_end);
            let mut read = tokio::io::BufReader::new(read);
            let mut frame = Vec::new();
            read.read_until(0, &mut frame).await.unwrap();
            frame.pop();
            let request: Value = serde_json::from_slice(&frame).unwrap();
            assert_eq!(request["method"], "Runtime.evaluate");
            assert_eq!(request["sessionId"], "s1");
            let id = request["id"].as_u64().unwrap();
            // An unrelated event and an unknown reply id are ignored.
            write
                .write_all(b"{\"method\":\"Target.targetCreated\",\"params\":{}}\0")
                .await
                .unwrap();
            write
                .write_all(format!("{{\"id\":{},\"result\":{{}}}}\0", id + 100).as_bytes())
                .await
                .unwrap();
            write
                .write_all(
                    format!("{{\"id\":{id},\"result\":{{\"result\":{{\"value\":7}}}}}}\0")
                        .as_bytes(),
                )
                .await
                .unwrap();
            frame.clear();
            read.read_until(0, &mut frame).await.unwrap();
            frame.pop();
            let failing: Value = serde_json::from_slice(&frame).unwrap();
            write
                .write_all(
                    format!(
                        "{{\"id\":{},\"error\":{{\"message\":\"nope\"}}}}\0",
                        failing["id"]
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let reply = cdp
            .send("Runtime.evaluate", json!({"expression":"7"}), Some("s1"))
            .await
            .unwrap();
        assert_eq!(reply.pointer("/result/value"), Some(&json!(7)));
        let error = cdp
            .send("Target.closeTarget", json!({}), None)
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("nope"));
        browser.await.unwrap();
    }

    #[tokio::test]
    #[ignore = "launches the real private Chrome headless against chatgpt.com in an isolated JCODE_HOME; never signs in or sends a prompt"]
    async fn chatgpt_web_instance_live_headless_launch_reports_signed_out_profile() {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("chatgpt-web").join("profile");
        let chrome = find_chrome().expect("Chrome or Chromium for this system");
        let instance = Instance::launch(&chrome, &profile, true).await.unwrap();
        let page = instance.open_page(CHATGPT_WEB_URL).await.unwrap();
        let access = wait_for_access(&instance, &page, PAGE_ACCESS_TIMEOUT).await;
        let host = instance.evaluate(&page, "location.hostname").await.unwrap();
        instance.close_page(&page).await.unwrap();
        // A fresh profile is signed out, and the page is ChatGPT itself, not a
        // bot challenge.
        assert_eq!(host, json!("chatgpt.com"));
        assert_eq!(access, PageAccess::SignedOut);
        // A second jcode process waits for the profile lock instead of
        // starting a competing Chrome on the same profile.
        assert!(
            tokio::time::timeout(Duration::from_millis(500), ProfileLock::acquire(&profile))
                .await
                .is_err()
        );
        instance.shutdown().await;
        assert!(
            tokio::time::timeout(Duration::from_secs(2), ProfileLock::acquire(&profile))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn waiting_process_requests_handoff_and_clears_the_request() {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("chatgpt-web").join("profile");
        let held = ProfileLock::acquire(&profile).await.unwrap();
        assert!(!handoff_requested(&profile));
        let waiter = tokio::spawn({
            let profile = profile.clone();
            async move { ProfileLock::acquire(&profile).await }
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while !handoff_requested(&profile) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "waiter never asked for the profile"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        drop(held);
        let lock = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            !handoff_requested(&profile),
            "the request is removed once the waiter holds the profile"
        );
        drop(lock);
    }

    #[tokio::test]
    async fn closed_pipe_fails_pending_and_later_commands() {
        let (client_end, browser_end) = tokio::io::duplex(1024);
        let (client_read, client_write) = tokio::io::split(client_end);
        let cdp = Cdp::spawn_io(client_write, client_read);
        let pending = cdp.send("Browser.getVersion", json!({}), None);
        drop(browser_end);
        assert!(pending.await.is_err());
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(cdp.is_closed());
        assert!(
            cdp.send("Browser.getVersion", json!({}), None)
                .await
                .is_err()
        );
    }
}
