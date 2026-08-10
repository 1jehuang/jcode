//! Stable native host to reloadable application-code boundary.
//!
//! The winit `ApplicationHandler` implementation in `main` is deliberately a
//! tiny, permanently linked dispatcher. Scene/Model/App behavior lives behind
//! this table and can be replaced by a newly built shared library without
//! recreating the process, window, Wayland surface, or GPU renderer.
//! The App allocation itself is retained, so draft, attached session, scroll,
//! animation, and selection state cross an activation without serialization.

use crate::App;
use anyhow::{Context, Result};
use libloading::Library;
use std::path::{Path, PathBuf};
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

const API_VERSION: u32 = 1;
const STATE_ABI: u64 = parse_hex(env!("JCODE_DESKTOP2_STATE_ABI"));
const WORKER_BUILD: u64 = parse_hex(env!("JCODE_DESKTOP2_WORKER_BUILD"));
const ENTRYPOINT: &[u8] = b"jcode_desktop2_worker_api_v1\0";

type Resumed = unsafe fn(*mut App, *const ActiveEventLoop);
type Window = unsafe fn(*mut App, *const ActiveEventLoop, WindowId, *mut Option<WindowEvent>);
type NewEvents = unsafe fn(*mut App, *const ActiveEventLoop, *mut Option<StartCause>);
type AboutToWait = unsafe fn(*mut App, *const ActiveEventLoop);

/// Operations that must execute from the permanently linked host image because
/// wgpu owns process-local backend registries. The worker may build a new Vello
/// scene, but surface configure and present always return through these pointers.
#[derive(Clone, Copy)]
pub struct HostApi {
    resize: unsafe fn(*mut App, u32, u32),
    present: unsafe fn(*mut App, *const vello::Scene),
}

impl Default for HostApi {
    fn default() -> Self {
        Self {
            resize: host_resize,
            present: host_present,
        }
    }
}

impl HostApi {
    pub fn resize(self, app: *mut App, width: u32, height: u32) {
        unsafe { (self.resize)(app, width, height) };
    }

    pub fn present(self, app: *mut App, scene: &vello::Scene) {
        unsafe { (self.present)(app, scene) };
    }
}

unsafe fn host_resize(app: *mut App, width: u32, height: u32) {
    let app = unsafe { &mut *app };
    if let Some(state) = app.state.as_mut() {
        state.resize(width, height);
    }
}

unsafe fn host_present(app: *mut App, scene: *const vello::Scene) {
    // This function pointer was installed by the executable, so even after a
    // worker swap all wgpu calls resolve against the host's original registry.
    // Only the freshly built, backend-agnostic Scene crosses this boundary.
    let app = unsafe { &mut *app };
    let scene = unsafe { &*scene };
    if let Some(state) = app.state.as_mut() {
        app.frame_meter.end_build();
        if let Err(error) = state.render(scene, &mut app.frame_meter) {
            eprintln!("render error: {error:#}");
        }
    }
}

/// Fixed-layout handshake. The callbacks use Rust's native ABI intentionally:
/// host and worker are products of the same workspace build and state layout is
/// verified before use. The C entrypoint only returns this fixed table.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WorkerApi {
    api_version: u32,
    _reserved: u32,
    state_abi: u64,
    worker_build: u64,
    resumed: Resumed,
    window_event: Window,
    new_events: NewEvents,
    about_to_wait: AboutToWait,
}

pub trait ApplicationWorker {
    fn worker_resumed(&mut self, event_loop: &ActiveEventLoop);
    fn worker_window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    );
    fn worker_new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause);
    fn worker_about_to_wait(&mut self, event_loop: &ActiveEventLoop);
}

unsafe fn resumed(app: *mut App, event_loop: *const ActiveEventLoop) {
    // SAFETY: the stable dispatcher supplies its live App and event-loop refs.
    unsafe { (&mut *app).worker_resumed(&*event_loop) };
}

unsafe fn window_event(
    app: *mut App,
    event_loop: *const ActiveEventLoop,
    window_id: WindowId,
    event: *mut Option<WindowEvent>,
) {
    // Taking the event transfers its owned strings/IME payload exactly once.
    let event = unsafe { (&mut *event).take() }.expect("host supplied window event");
    unsafe { (&mut *app).worker_window_event(&*event_loop, window_id, event) };
}

unsafe fn new_events(
    app: *mut App,
    event_loop: *const ActiveEventLoop,
    cause: *mut Option<StartCause>,
) {
    let cause = unsafe { (&mut *cause).take() }.expect("host supplied start cause");
    unsafe { (&mut *app).worker_new_events(&*event_loop, cause) };
}

unsafe fn about_to_wait(app: *mut App, event_loop: *const ActiveEventLoop) {
    unsafe { (&mut *app).worker_about_to_wait(&*event_loop) };
}

static BUILTIN_API: WorkerApi = WorkerApi {
    api_version: API_VERSION,
    _reserved: 0,
    state_abi: STATE_ABI,
    worker_build: WORKER_BUILD,
    resumed,
    window_event,
    new_events,
    about_to_wait,
};

/// Exported by `libjcode_desktop2.so`, not used to create another native app.
#[unsafe(no_mangle)]
pub extern "C" fn jcode_desktop2_worker_api_v1() -> *const WorkerApi {
    &BUILTIN_API
}

pub struct Worker {
    api: WorkerApi,
    active_path: Option<PathBuf>,
    // A worker can start long-lived harness threads. Keep generations resident
    // so no thread or cached function pointer ever returns into unmapped code.
    generations: Vec<Library>,
}

impl Default for Worker {
    fn default() -> Self {
        Self {
            api: BUILTIN_API,
            active_path: None,
            generations: Vec::new(),
        }
    }
}

impl Worker {
    pub fn worker_build(&self) -> u64 {
        self.api.worker_build
    }

    pub fn snapshot(&self) -> WorkerApi {
        self.api
    }

    /// Load and validate a generation completely before publishing its table.
    /// Failure leaves `api`, the current worker, and the visible frame untouched.
    pub fn activate(&mut self, path: &Path) -> Result<bool> {
        if self.active_path.as_deref() == Some(path) {
            return Ok(false);
        }
        // SAFETY: constructors run here, while all symbols are validated before
        // the handle is retained. Self-dev artifacts are trusted local builds.
        let library = unsafe { Library::new(path) }
            .with_context(|| format!("load worker {}", path.display()))?;
        let api = unsafe {
            let entry: libloading::Symbol<unsafe extern "C" fn() -> *const WorkerApi> = library
                .get(ENTRYPOINT)
                .context("resolve desktop2 worker entrypoint")?;
            let pointer = entry();
            pointer
                .as_ref()
                .copied()
                .context("desktop2 worker returned a null API")?
        };
        validate(api)?;

        self.api = api;
        self.active_path = Some(path.to_owned());
        self.generations.push(library);
        Ok(true)
    }
}

impl WorkerApi {
    pub fn resumed(self, app: *mut App, event_loop: &ActiveEventLoop) {
        // SAFETY: API validation happened before publication and inputs live for
        // the duration of this synchronous callback.
        unsafe { (self.resumed)(app, event_loop) };
    }

    pub fn window_event(
        self,
        app: *mut App,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let mut event = Some(event);
        unsafe { (self.window_event)(app, event_loop, window_id, &mut event) };
    }

    pub fn new_events(self, app: *mut App, event_loop: &ActiveEventLoop, cause: StartCause) {
        let mut cause = Some(cause);
        unsafe { (self.new_events)(app, event_loop, &mut cause) };
    }

    pub fn about_to_wait(self, app: *mut App, event_loop: &ActiveEventLoop) {
        unsafe { (self.about_to_wait)(app, event_loop) };
    }
}

/// Non-window smoke test used by selfdev publication. Loading through the same
/// path and handshake as a live host prevents signalling windows with a broken
/// or stale shared object.
pub fn check_artifact(path: &Path) -> Result<u64> {
    let mut worker = Worker::default();
    if !worker.activate(path)? {
        anyhow::bail!("worker artifact was not activated");
    }
    Ok(worker.worker_build())
}

pub const fn builtin_build() -> u64 {
    WORKER_BUILD
}

fn validate(api: WorkerApi) -> Result<()> {
    if api.api_version != API_VERSION {
        anyhow::bail!(
            "worker API {} is incompatible with host API {API_VERSION}",
            api.api_version
        );
    }
    if api.state_abi != STATE_ABI {
        anyhow::bail!(
            "worker state ABI {:016x} is incompatible with host {:016x}; keep this window on the old worker or launch a migrated host",
            api.state_abi,
            STATE_ABI
        );
    }
    Ok(())
}

const fn parse_hex(value: &str) -> u64 {
    let bytes = value.as_bytes();
    let mut index = 0;
    let mut output = 0u64;
    while index < bytes.len() {
        output = output * 16
            + match bytes[index] {
                b'0'..=b'9' => (bytes[index] - b'0') as u64,
                b'a'..=b'f' => (bytes[index] - b'a' + 10) as u64,
                b'A'..=b'F' => (bytes[index] - b'A' + 10) as u64,
                _ => panic!("invalid hexadecimal build fingerprint"),
            };
        index += 1;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_worker_matches_stable_host_contract() {
        validate(BUILTIN_API).unwrap();
        assert_ne!(BUILTIN_API.worker_build, 0);
        assert_ne!(BUILTIN_API.state_abi, 0);
    }

    #[test]
    fn incompatible_state_is_rejected_before_callbacks_are_published() {
        let mut incompatible = BUILTIN_API;
        incompatible.state_abi ^= 1;
        assert!(
            validate(incompatible)
                .unwrap_err()
                .to_string()
                .contains("state ABI")
        );
    }
}
