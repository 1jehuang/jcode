use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DesktopReloadWindowPlacement {
    pub(crate) position: Option<PhysicalPosition<i32>>,
    pub(crate) inner_size: PhysicalSize<u32>,
}

impl DesktopReloadWindowPlacement {
    pub(crate) fn from_window(window: &Window) -> Option<Self> {
        let inner_size = window.inner_size();
        if !desktop_reload_window_size_is_valid(inner_size) {
            return None;
        }
        Some(Self {
            position: window.outer_position().ok(),
            inner_size,
        })
    }

    pub(crate) fn from_env_value(raw: &str) -> Option<Self> {
        let parts = raw.split(',').collect::<Vec<_>>();
        if parts.len() != 4 {
            return None;
        }

        let position = match (parts[0], parts[1]) {
            ("_", "_") => None,
            (x, y) => Some(PhysicalPosition::new(x.parse().ok()?, y.parse().ok()?)),
        };
        let inner_size = PhysicalSize::new(parts[2].parse().ok()?, parts[3].parse().ok()?);
        if !desktop_reload_window_size_is_valid(inner_size) {
            return None;
        }
        Some(Self {
            position,
            inner_size,
        })
    }

    pub(crate) fn to_env_value(self) -> String {
        let (x, y) = match self.position {
            Some(position) => (position.x.to_string(), position.y.to_string()),
            None => ("_".to_string(), "_".to_string()),
        };
        format!(
            "{x},{y},{},{}",
            self.inner_size.width, self.inner_size.height
        )
    }

    pub(crate) fn apply_to_window_builder(
        self,
        mut window_builder: WindowBuilder,
    ) -> WindowBuilder {
        window_builder = window_builder.with_inner_size(self.inner_size);
        if let Some(position) = self.position {
            window_builder = window_builder.with_position(position);
        }
        window_builder
    }
}

pub(crate) fn desktop_reload_window_size_is_valid(size: PhysicalSize<u32>) -> bool {
    (1..=DESKTOP_RELOAD_MAX_RESTORED_DIMENSION).contains(&size.width)
        && (1..=DESKTOP_RELOAD_MAX_RESTORED_DIMENSION).contains(&size.height)
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DesktopReloadStartup {
    pub(crate) window_placement: Option<DesktopReloadWindowPlacement>,
    pub(crate) handoff: Option<DesktopReloadStartupHandoff>,
}

impl DesktopReloadStartup {
    pub(crate) fn from_env() -> Self {
        let raw_window_placement = std::env::var(DESKTOP_RELOAD_WINDOW_ENV).ok();
        let ready_file = std::env::var_os(DESKTOP_RELOAD_HANDOFF_READY_ENV).map(PathBuf::from);
        let release_file = std::env::var_os(DESKTOP_RELOAD_HANDOFF_RELEASE_ENV).map(PathBuf::from);
        let placement_file =
            std::env::var_os(DESKTOP_RELOAD_HANDOFF_PLACEMENT_ENV).map(PathBuf::from);
        unsafe {
            std::env::remove_var(DESKTOP_RELOAD_WINDOW_ENV);
            std::env::remove_var(DESKTOP_RELOAD_HANDOFF_READY_ENV);
            std::env::remove_var(DESKTOP_RELOAD_HANDOFF_RELEASE_ENV);
            std::env::remove_var(DESKTOP_RELOAD_HANDOFF_PLACEMENT_ENV);
        }

        let window_placement = raw_window_placement.as_deref().and_then(|raw| {
            let placement = DesktopReloadWindowPlacement::from_env_value(raw);
            if placement.is_none() {
                desktop_log::warn(format_args!(
                    "jcode-desktop: ignoring invalid reload window placement {raw:?}"
                ));
            }
            placement
        });
        let handoff = match (ready_file, release_file) {
            (Some(ready_file), Some(release_file)) => Some(DesktopReloadStartupHandoff {
                ready_file,
                release_file,
                placement_file,
            }),
            (None, None) => None,
            _ => {
                desktop_log::warn(format_args!(
                    "jcode-desktop: ignoring incomplete reload handoff environment"
                ));
                None
            }
        };

        Self {
            window_placement,
            handoff,
        }
    }

    pub(crate) fn hidden_until_handoff_release(&self) -> bool {
        self.handoff.is_some()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DesktopReloadStartupHandoff {
    pub(crate) ready_file: PathBuf,
    pub(crate) release_file: PathBuf,
    pub(crate) placement_file: Option<PathBuf>,
}

impl DesktopReloadStartupHandoff {
    pub(crate) fn signal_ready_and_wait_for_release(&self) -> Option<DesktopReloadWindowPlacement> {
        if let Err(error) = write_desktop_reload_marker(&self.ready_file) {
            desktop_log::warn(format_args!(
                "jcode-desktop: failed to signal reload readiness: {error:#}"
            ));
            return None;
        }

        desktop_log::info(format_args!(
            "jcode-desktop: reload child ready, waiting for parent release"
        ));
        let deadline = Instant::now() + DESKTOP_RELOAD_STARTUP_RELEASE_TIMEOUT;
        while Instant::now() < deadline {
            if self.release_file.exists() {
                let placement = self
                    .placement_file
                    .as_deref()
                    .and_then(read_desktop_reload_window_placement);
                if let Some(placement_file) = self.placement_file.as_deref() {
                    let _ = fs::remove_file(placement_file);
                }
                cleanup_desktop_reload_handoff_files(&self.ready_file, &self.release_file);
                return placement;
            }
            std::thread::sleep(DESKTOP_RELOAD_HANDOFF_POLL_INTERVAL);
        }

        desktop_log::warn(format_args!(
            "jcode-desktop: reload parent did not release handoff within {}ms; showing replacement window anyway",
            DESKTOP_RELOAD_STARTUP_RELEASE_TIMEOUT.as_millis()
        ));
        cleanup_desktop_reload_handoff_files(&self.ready_file, &self.release_file);
        if let Some(placement_file) = self.placement_file.as_deref() {
            let _ = fs::remove_file(placement_file);
        }
        None
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DesktopReloadHandoff {
    pub(crate) ready_file: PathBuf,
    pub(crate) release_file: PathBuf,
    pub(crate) placement_file: PathBuf,
    pub(crate) window_placement: Option<DesktopReloadWindowPlacement>,
}

impl DesktopReloadHandoff {
    pub(crate) fn new(window: &Window) -> Result<Self> {
        let dir = desktop_reload_handoff_temp_dir();
        fs::create_dir_all(&dir).with_context(|| {
            format!(
                "failed to create desktop reload handoff directory {}",
                dir.display()
            )
        })?;
        Ok(Self {
            ready_file: dir.join("ready"),
            release_file: dir.join("release"),
            placement_file: dir.join("placement"),
            window_placement: DesktopReloadWindowPlacement::from_window(window),
        })
    }

    pub(crate) fn apply_to_command(&self, command: &mut Command) {
        if let Some(placement) = self.window_placement {
            command.env(DESKTOP_RELOAD_WINDOW_ENV, placement.to_env_value());
        }
        command.env(DESKTOP_RELOAD_HANDOFF_READY_ENV, &self.ready_file);
        command.env(DESKTOP_RELOAD_HANDOFF_RELEASE_ENV, &self.release_file);
        command.env(DESKTOP_RELOAD_HANDOFF_PLACEMENT_ENV, &self.placement_file);
    }

    pub(crate) fn watcher(&self) -> DesktopReloadHandoffWatcher {
        DesktopReloadHandoffWatcher {
            ready_file: self.ready_file.clone(),
            release_file: self.release_file.clone(),
            placement_file: self.placement_file.clone(),
            spawned_at: Instant::now(),
        }
    }

    pub(crate) fn cleanup(&self) {
        let _ = fs::remove_file(&self.placement_file);
        cleanup_desktop_reload_handoff_files(&self.ready_file, &self.release_file);
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DesktopReloadHandoffWatcher {
    pub(crate) ready_file: PathBuf,
    pub(crate) release_file: PathBuf,
    pub(crate) placement_file: PathBuf,
    pub(crate) spawned_at: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DesktopReloadHandoffPoll {
    Waiting,
    Ready,
    TimedOut,
}

impl DesktopReloadHandoffWatcher {
    pub(crate) fn poll(&self) -> Result<DesktopReloadHandoffPoll> {
        self.poll_with_placement(None)
    }

    pub(crate) fn poll_with_placement(
        &self,
        placement: Option<DesktopReloadWindowPlacement>,
    ) -> Result<DesktopReloadHandoffPoll> {
        if self.ready_file.exists() {
            if let Some(placement) = placement {
                if let Err(error) =
                    write_desktop_reload_window_placement(&self.placement_file, placement)
                {
                    desktop_log::warn(format_args!(
                        "jcode-desktop: failed to synchronize final reload window placement: {error:#}"
                    ));
                }
            }
            write_desktop_reload_marker(&self.release_file)?;
            return Ok(DesktopReloadHandoffPoll::Ready);
        }
        if self.spawned_at.elapsed() >= DESKTOP_RELOAD_HANDOFF_TIMEOUT {
            return Ok(DesktopReloadHandoffPoll::TimedOut);
        }
        Ok(DesktopReloadHandoffPoll::Waiting)
    }

    pub(crate) fn cleanup(&self) {
        let _ = fs::remove_file(&self.placement_file);
        cleanup_desktop_reload_handoff_files(&self.ready_file, &self.release_file);
    }
}

pub(crate) fn desktop_reload_handoff_temp_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "jcode-desktop-reload-{}-{nonce}",
        std::process::id()
    ))
}

pub(crate) fn write_desktop_reload_marker(path: &Path) -> Result<()> {
    fs::write(path, format!("{}\n", std::process::id()))
        .with_context(|| format!("failed to write {}", path.display()))
}

pub(crate) fn write_desktop_reload_window_placement(
    path: &Path,
    placement: DesktopReloadWindowPlacement,
) -> Result<()> {
    fs::write(path, placement.to_env_value())
        .with_context(|| format!("failed to write {}", path.display()))
}

pub(crate) fn read_desktop_reload_window_placement(
    path: &Path,
) -> Option<DesktopReloadWindowPlacement> {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| DesktopReloadWindowPlacement::from_env_value(raw.trim()))
}

pub(crate) fn cleanup_desktop_reload_handoff_files(ready_file: &Path, release_file: &Path) {
    let _ = fs::remove_file(ready_file);
    let _ = fs::remove_file(release_file);
    if ready_file.parent() == release_file.parent()
        && let Some(parent) = ready_file.parent()
        && parent
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("jcode-desktop-reload-"))
    {
        let _ = fs::remove_dir(parent);
    }
}

pub(crate) struct DesktopHotReloader {
    pub(crate) relaunch: Option<DesktopRelaunch>,
    pub(crate) strategy: DesktopReloadStrategy,
    pub(crate) observed_modified: Option<std::time::SystemTime>,
    pub(crate) last_checked: Instant,
    pub(crate) pending_handoff: Option<DesktopReloadHandoffWatcher>,
    pub(crate) app_worker: Option<DesktopWorkerConnection>,
    pub(crate) event_loop_proxy: EventLoopProxy<DesktopUserEvent>,
}

#[derive(Default)]
pub(crate) struct DesktopWorkerDrain {
    pub(crate) latest_scene: Option<DesktopScene>,
    pub(crate) reload_requested: bool,
}

impl DesktopHotReloader {
    const CHECK_INTERVAL: Duration = Duration::from_millis(750);

    pub(crate) fn new(
        strategy: DesktopReloadStrategy,
        event_loop_proxy: EventLoopProxy<DesktopUserEvent>,
    ) -> Self {
        let relaunch = DesktopRelaunch::from_current_process();
        let observed_modified = relaunch.as_ref().and_then(|relaunch| {
            binary_modified_time(&desktop_reload_binary_candidate(&relaunch.binary))
        });
        Self {
            relaunch,
            strategy,
            observed_modified,
            last_checked: Instant::now(),
            pending_handoff: None,
            app_worker: None,
            event_loop_proxy,
        }
    }

    pub(crate) fn next_wake(&self, now: Instant) -> Option<Instant> {
        if self.pending_handoff.is_some() {
            return Some(now + DESKTOP_RELOAD_HANDOFF_POLL_INTERVAL);
        }
        self.relaunch.as_ref()?;
        Some(std::cmp::max(now, self.last_checked + Self::CHECK_INTERVAL))
    }

    pub(crate) fn drain_app_worker_messages(&mut self) -> DesktopWorkerDrain {
        let Some(worker) = self.app_worker.as_mut() else {
            return DesktopWorkerDrain::default();
        };
        let mut drained = DesktopWorkerDrain::default();
        let mut should_drop_worker = false;
        while let Some(message) = worker.try_recv() {
            match message {
                Ok(DesktopWorkerToHostMessage::Ready(ready)) => {
                    desktop_log::info(format_args!(
                        "jcode-desktop: app worker ready; pid={} mode={:?}",
                        ready.worker_pid, ready.mode
                    ));
                }
                Ok(DesktopWorkerToHostMessage::Scene(scene_update)) => {
                    drained.latest_scene = Some(scene_update.scene);
                }
                Ok(DesktopWorkerToHostMessage::ReloadRequested) => {
                    drained.reload_requested = true;
                }
                Ok(DesktopWorkerToHostMessage::Snapshot(snapshot)) => {
                    desktop_log::info(format_args!(
                        "jcode-desktop: app worker snapshot response {}; mode={}",
                        snapshot.request_id, snapshot.snapshot.mode
                    ));
                }
                Ok(DesktopWorkerToHostMessage::Metrics(metrics)) => {
                    desktop_log::info(format_args!(
                        "jcode-desktop: app worker reported {} metric(s)",
                        metrics.metrics.len()
                    ));
                }
                Ok(DesktopWorkerToHostMessage::Log(log)) => {
                    desktop_log::info(format_args!(
                        "jcode-desktop: app worker log {:?}: {}",
                        log.level, log.message
                    ));
                }
                Ok(DesktopWorkerToHostMessage::Exited(exit)) => {
                    desktop_log::warn(format_args!(
                        "jcode-desktop: app worker exited code={:?} reason={:?}",
                        exit.code, exit.reason
                    ));
                }
                Err(error) => {
                    desktop_log::error(format_args!(
                        "jcode-desktop: failed to read app worker message: {error:#}"
                    ));
                    should_drop_worker = true;
                    break;
                }
            }
        }
        if !should_drop_worker {
            match worker.try_wait() {
                Ok(Some(status)) => {
                    desktop_log::warn(format_args!(
                        "jcode-desktop: app worker process exited unexpectedly: {status}"
                    ));
                    should_drop_worker = true;
                }
                Ok(None) => {}
                Err(error) => {
                    desktop_log::warn(format_args!(
                        "jcode-desktop: failed to poll app worker process: {error:#}"
                    ));
                    should_drop_worker = true;
                }
            }
        }
        if should_drop_worker
            && let Some(worker) = self.app_worker.take()
            && let Err(error) = worker.kill()
        {
            desktop_log::warn(format_args!(
                "jcode-desktop: failed to clean up stopped app worker: {error:#}"
            ));
        }
        drained
    }

    pub(crate) fn has_app_worker(&self) -> bool {
        self.app_worker.is_some()
    }

    pub(crate) fn send_app_worker_input(&mut self, input: DesktopInputEvent) -> Result<()> {
        self.send_app_worker_message(DesktopHostToWorkerMessage::Input(input))
    }

    pub(crate) fn send_app_worker_message(
        &mut self,
        message: DesktopHostToWorkerMessage,
    ) -> Result<()> {
        let Some(worker) = self.app_worker.as_mut() else {
            return Ok(());
        };
        worker.send(message)
    }

    pub(crate) fn start_app_worker_for_current_binary(
        &mut self,
        app: &DesktopApp,
        window: &Window,
        reason: &'static str,
    ) {
        let Some(relaunch) = self.relaunch.clone() else {
            desktop_log::warn(format_args!(
                "jcode-desktop: cannot start app worker for {reason}; current process cannot be relaunched"
            ));
            return;
        };
        let binary = desktop_reload_binary_candidate(&relaunch.binary);
        self.restart_app_worker(app, window, &relaunch, binary, reason);
    }

    pub(crate) fn poll(&mut self, app: &DesktopApp, window: &Window) -> bool {
        if self.poll_pending_handoff(window) {
            return true;
        }
        if self.pending_handoff.is_some() {
            return false;
        }
        if self.last_checked.elapsed() < Self::CHECK_INTERVAL {
            return false;
        }
        self.last_checked = Instant::now();

        let Some(relaunch) = self.relaunch.clone() else {
            return false;
        };
        let binary = desktop_reload_binary_candidate(&relaunch.binary);
        let Some(current_modified) = binary_modified_time(&binary) else {
            return false;
        };
        let observed_modified = self.observed_modified;
        self.observed_modified = Some(current_modified);
        if observed_modified.is_some_and(|observed| current_modified > observed) {
            return self.reload_with_strategy(app, window, &relaunch, binary, "hot reload");
        }
        false
    }

    pub(crate) fn force_reload(&mut self, app: &DesktopApp, window: &Window) -> bool {
        if self.poll_pending_handoff(window) {
            return true;
        }
        if self.pending_handoff.is_some() {
            desktop_log::warn(format_args!(
                "jcode-desktop: force reload requested while another reload handoff is pending"
            ));
            return false;
        }
        let Some(relaunch) = self.relaunch.clone() else {
            desktop_log::warn(format_args!(
                "jcode-desktop: force reload requested but current process cannot be relaunched"
            ));
            return false;
        };
        let binary = desktop_reload_binary_candidate(&relaunch.binary);
        self.reload_with_strategy(app, window, &relaunch, binary, "force reload")
    }

    pub(crate) fn reload_with_strategy(
        &mut self,
        app: &DesktopApp,
        window: &Window,
        relaunch: &DesktopRelaunch,
        binary: PathBuf,
        reason: &'static str,
    ) -> bool {
        match self.strategy {
            DesktopReloadStrategy::FullProcessHandoff => {
                self.reload_full_process_handoff(app, window, relaunch, binary, reason)
            }
            DesktopReloadStrategy::AppWorkerRestart => {
                desktop_log::info(format_args!(
                    "jcode-desktop: {reason} requested app-worker restart; keeping stable host window alive"
                ));
                self.restart_app_worker(app, window, relaunch, binary, reason);
                false
            }
        }
    }

    pub(crate) fn restart_app_worker(
        &mut self,
        app: &DesktopApp,
        window: &Window,
        relaunch: &DesktopRelaunch,
        binary: PathBuf,
        reason: &'static str,
    ) {
        if let Some(worker) = self.app_worker.take()
            && let Err(error) = worker.kill()
        {
            desktop_log::warn(format_args!(
                "jcode-desktop: failed to stop previous app worker before {reason}: {error:#}"
            ));
        }

        let worker_relaunch = relaunch.for_app(app, binary).for_app_worker();
        match worker_relaunch.spawn_app_worker(self.event_loop_proxy.clone()) {
            Ok(mut worker) => {
                if let Err(error) =
                    worker.send(DesktopHostToWorkerMessage::Initialize(DesktopWorkerInit {
                        mode: desktop_worker_mode_for_app(app),
                        snapshot: Some(app.snapshot()),
                        window: desktop_window_state(window),
                    }))
                {
                    desktop_log::error(format_args!(
                        "jcode-desktop: failed to initialize app worker for {reason}: {error:#}"
                    ));
                    if let Err(kill_error) = worker.kill() {
                        desktop_log::warn(format_args!(
                            "jcode-desktop: failed to kill uninitialized app worker: {kill_error:#}"
                        ));
                    }
                    return;
                }
                desktop_log::info(format_args!(
                    "jcode-desktop: app worker restarted for {reason}; pid={}",
                    worker.child_id()
                ));
                self.app_worker = Some(worker);
            }
            Err(error) => desktop_log::error(format_args!(
                "jcode-desktop: failed to restart app worker for {reason}: {error:#}"
            )),
        }
    }

    pub(crate) fn reload_full_process_handoff(
        &mut self,
        app: &DesktopApp,
        window: &Window,
        relaunch: &DesktopRelaunch,
        binary: PathBuf,
        reason: &'static str,
    ) -> bool {
        let relaunch = relaunch.for_app(app, binary);
        match relaunch.spawn_for_window(window) {
            Ok(Some(handoff)) => {
                self.pending_handoff = Some(handoff);
                false
            }
            Ok(None) => true,
            Err(error) => {
                desktop_log::error(format_args!(
                    "jcode-desktop: failed to {reason} desktop: {error:#}"
                ));
                false
            }
        }
    }

    pub(crate) fn poll_pending_handoff(&mut self, window: &Window) -> bool {
        let Some(pending_handoff) = self.pending_handoff.as_ref() else {
            return false;
        };
        match pending_handoff.poll_with_placement(DesktopReloadWindowPlacement::from_window(window))
        {
            Ok(DesktopReloadHandoffPoll::Waiting) => false,
            Ok(DesktopReloadHandoffPoll::Ready) => {
                desktop_log::info(format_args!(
                    "jcode-desktop: reload replacement is ready; exiting old process"
                ));
                true
            }
            Ok(DesktopReloadHandoffPoll::TimedOut) => {
                desktop_log::warn(format_args!(
                    "jcode-desktop: reload replacement did not become ready within {}ms; keeping old process alive",
                    DESKTOP_RELOAD_HANDOFF_TIMEOUT.as_millis()
                ));
                if let Some(pending_handoff) = self.pending_handoff.take() {
                    pending_handoff.cleanup();
                }
                false
            }
            Err(error) => {
                desktop_log::error(format_args!(
                    "jcode-desktop: failed to release reload replacement: {error:#}"
                ));
                true
            }
        }
    }
}

pub(crate) fn desktop_worker_mode_for_app(app: &DesktopApp) -> DesktopWorkerMode {
    match app {
        DesktopApp::SingleSession(_) => DesktopWorkerMode::SingleSession,
        DesktopApp::Workspace(_) => DesktopWorkerMode::Workspace,
    }
}

pub(crate) fn desktop_window_state(window: &Window) -> DesktopWindowState {
    let size = window.inner_size();
    DesktopWindowState {
        width: size.width,
        height: size.height,
        scale_factor: window.scale_factor() as f32,
        focused: window.has_focus(),
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DesktopRelaunch {
    pub(crate) binary: PathBuf,
    pub(crate) args: Vec<OsString>,
}

impl DesktopRelaunch {
    pub(crate) fn from_current_process() -> Option<Self> {
        let mut args = std::env::args_os();
        let argv0 = args.next()?;
        let binary = match resolve_invoked_binary(&argv0) {
            Some(binary) => binary,
            None => match std::env::current_exe() {
                Ok(binary) => binary,
                Err(_) => return None,
            },
        };
        Some(Self {
            binary,
            args: args.collect(),
        })
    }

    pub(crate) fn spawn_for_window(
        &self,
        window: &Window,
    ) -> Result<Option<DesktopReloadHandoffWatcher>> {
        let handoff = match DesktopReloadHandoff::new(window) {
            Ok(handoff) => Some(handoff),
            Err(error) => {
                desktop_log::warn(format_args!(
                    "jcode-desktop: reload handoff unavailable, falling back to immediate relaunch: {error:#}"
                ));
                None
            }
        };
        desktop_log::info(format_args!(
            "jcode-desktop: hot reloading into {} with args {:?}{}",
            self.binary.display(),
            self.args,
            if handoff.is_some() {
                " using handoff"
            } else {
                ""
            }
        ));
        let mut command = Command::new(&self.binary);
        command.args(&self.args);
        command.env_remove(DESKTOP_RELOAD_WINDOW_ENV);
        command.env_remove(DESKTOP_RELOAD_HANDOFF_READY_ENV);
        command.env_remove(DESKTOP_RELOAD_HANDOFF_RELEASE_ENV);
        command.env_remove(DESKTOP_RELOAD_HANDOFF_PLACEMENT_ENV);
        if let Some(handoff) = handoff.as_ref() {
            handoff.apply_to_command(&mut command);
        }
        if let Err(error) = command.spawn() {
            if let Some(handoff) = handoff.as_ref() {
                handoff.cleanup();
            }
            return Err(error)
                .with_context(|| format!("failed to spawn {}", self.binary.display()));
        }
        Ok(handoff.as_ref().map(DesktopReloadHandoff::watcher))
    }

    pub(crate) fn for_app(&self, app: &DesktopApp, binary: PathBuf) -> Self {
        if let DesktopApp::Workspace(workspace) = app
            && let Err(error) = desktop_prefs::save_preferences(&workspace.preferences())
        {
            desktop_log::error(format_args!(
                "jcode-desktop: failed to persist workspace state before hot reload: {error:#}"
            ));
        }

        let mut args = desktop_args_without_resume(&self.args);
        match app {
            DesktopApp::Workspace(_) => ensure_desktop_workspace_arg(&mut args),
            DesktopApp::SingleSession(_) => {
                if let Some(session_id) = app.single_session_live_id() {
                    args.push(OsString::from("--resume"));
                    args.push(OsString::from(session_id));
                }
            }
        }
        Self { binary, args }
    }

    pub(crate) fn for_app_worker(&self) -> Self {
        let mut args = desktop_args_without_process_role(&self.args);
        args.push(OsString::from("--desktop-process-role"));
        args.push(OsString::from("app-worker"));
        Self {
            binary: self.binary.clone(),
            args,
        }
    }

    fn spawn_app_worker(
        &self,
        event_loop_proxy: EventLoopProxy<DesktopUserEvent>,
    ) -> Result<DesktopWorkerConnection> {
        desktop_log::info(format_args!(
            "jcode-desktop: spawning app worker {} with args {:?}",
            self.binary.display(),
            self.args
        ));
        let mut command = Command::new(&self.binary);
        command.args(&self.args);
        command.env_remove(DESKTOP_RELOAD_WINDOW_ENV);
        command.env_remove(DESKTOP_RELOAD_HANDOFF_READY_ENV);
        command.env_remove(DESKTOP_RELOAD_HANDOFF_RELEASE_ENV);
        command.env_remove(DESKTOP_RELOAD_HANDOFF_PLACEMENT_ENV);
        DesktopWorkerConnection::spawn(&mut command, move || {
            let _ = event_loop_proxy.send_event(DesktopUserEvent::AppWorkerActivity);
        })
        .with_context(|| format!("failed to spawn app worker {}", self.binary.display()))
    }
}

pub(crate) fn ensure_desktop_workspace_arg(args: &mut Vec<OsString>) {
    let has_mode_arg = args.iter().any(|arg| {
        arg == "--workspace"
            || arg == "--new"
            || arg == "--resume"
            || arg.to_str().is_some_and(|value| {
                value.starts_with("--resume=") || value.starts_with("jcode://")
            })
    });
    if !has_mode_arg {
        args.push(OsString::from("--workspace"));
    }
}

pub(crate) fn desktop_args_without_resume(args: &[OsString]) -> Vec<OsString> {
    let mut filtered = Vec::with_capacity(args.len());
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--resume" {
            skip_next = true;
            continue;
        }
        if arg
            .to_str()
            .is_some_and(|value| value.starts_with("--resume="))
        {
            continue;
        }
        filtered.push(arg.clone());
    }
    filtered
}

pub(crate) fn desktop_args_without_process_role(args: &[OsString]) -> Vec<OsString> {
    let mut filtered = Vec::with_capacity(args.len());
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--desktop-process-role" {
            skip_next = true;
            continue;
        }
        if arg == "--desktop-host" || arg == "--desktop-app-worker" {
            continue;
        }
        if arg
            .to_str()
            .is_some_and(|value| value.starts_with("--desktop-process-role="))
        {
            continue;
        }
        filtered.push(arg.clone());
    }
    filtered
}

pub(crate) fn desktop_reload_binary_candidate(invoked_binary: &Path) -> PathBuf {
    let Some(repo_dir) = find_desktop_repo_dir() else {
        return invoked_binary.to_path_buf();
    };
    desktop_reload_binary_candidate_from(invoked_binary, &repo_dir)
}

pub(crate) fn desktop_reload_binary_candidate_from(
    invoked_binary: &Path,
    repo_dir: &Path,
) -> PathBuf {
    let selfdev = desktop_selfdev_binary_path(repo_dir);
    if paths_refer_to_same_file(invoked_binary, &selfdev)
        || binary_is_newer_than(&selfdev, invoked_binary)
    {
        selfdev
    } else {
        invoked_binary.to_path_buf()
    }
}

pub(crate) fn desktop_selfdev_binary_path(repo_dir: &Path) -> PathBuf {
    repo_dir
        .join("target")
        .join("selfdev")
        .join(desktop_binary_name())
}

pub(crate) fn desktop_binary_name() -> &'static str {
    if cfg!(windows) {
        "jcode-desktop.exe"
    } else {
        "jcode-desktop"
    }
}

pub(crate) fn binary_is_newer_than(candidate: &Path, baseline: &Path) -> bool {
    let Some(candidate_modified) = binary_modified_time(candidate) else {
        return false;
    };
    match binary_modified_time(baseline) {
        Some(baseline_modified) => candidate_modified > baseline_modified,
        None => true,
    }
}

pub(crate) fn paths_refer_to_same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

pub(crate) fn find_desktop_repo_dir() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    find_desktop_repo_in_ancestors(&manifest_dir)
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|path| find_desktop_repo_in_ancestors(&path))
        })
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|path| find_desktop_repo_in_ancestors(&path))
        })
}

pub(crate) fn find_desktop_repo_in_ancestors(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|candidate| is_jcode_desktop_repo(candidate))
        .map(Path::to_path_buf)
}

pub(crate) fn is_jcode_desktop_repo(candidate: &Path) -> bool {
    if !candidate.join("crates/jcode-desktop/Cargo.toml").is_file() {
        return false;
    }
    std::fs::read_to_string(candidate.join("Cargo.toml"))
        .map(|cargo_toml| cargo_toml.contains("name = \"jcode\""))
        .unwrap_or(false)
}

pub(crate) fn binary_modified_time(path: &Path) -> Option<std::time::SystemTime> {
    let metadata = match path.metadata() {
        Ok(metadata) => metadata,
        Err(_) => return None,
    };
    metadata.modified().ok()
}

pub(crate) fn resolve_invoked_binary(argv0: &OsString) -> Option<PathBuf> {
    let path = PathBuf::from(argv0);
    if path.components().count() > 1 {
        return Some(path);
    }

    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(&path))
        .find(|candidate| candidate.is_file())
}

pub(crate) fn show_desktop_reload_notice(app: &mut DesktopApp) {
    app.set_single_session_status_label("desktop UI reloaded");
}
