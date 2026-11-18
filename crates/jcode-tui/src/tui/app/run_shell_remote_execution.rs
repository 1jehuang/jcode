//! Remote shell command execution.

use super::*;

impl App {
    /// Run the TUI in remote mode, connecting to a server
    pub async fn run_remote(
        mut self,
        mut terminal: DefaultTerminal,
        remote_working_dir: Option<String>,
    ) -> Result<RunResult> {
        if crate::tui::is_ssh_remote() {
            self.session.working_dir = remote_working_dir.clone();
        }
        super::super::terminal_liveness::capture_initial_tty();
        let mut event_stream = EventStream::new();
        let mut redraw_period = crate::tui::redraw_interval(&self);
        let mut redraw_interval = redraw_timer(redraw_period);
        let mut status_spinner_interval = status_spinner_interval();
        let mut status_spinner_renderer = StatusSpinnerRenderer::default();
        let mut needs_redraw = true;
        let mut first_frame_reported = false;
        // While unfocused and idle, redraws are throttled to this interval so a
        // backgrounded session does not repaint at full rate on shared-server bus
        // chatter. `None` means "no throttled frame drawn yet since losing focus".
        const UNFOCUSED_IDLE_REDRAW_MIN_INTERVAL: std::time::Duration =
            std::time::Duration::from_millis(1000);
        let mut last_unfocused_draw: Option<std::time::Instant> = None;
        let mut handterm_native_scroll =
            super::super::handterm_native_scroll::HandtermNativeScrollClient::connect_from_env();
        let mut remote_state = remote::RemoteRunState::default();

        'outer: loop {
            // Each pass connects afresh, possibly to an older server. Poll
            // todos again until this server shows that it pushes them.
            self.remote_todos_pushed = false;
            if self.display_messages.is_empty() {
                if self.server_spawning {
                    self.set_remote_startup_phase(super::super::RemoteStartupPhase::StartingServer);
                } else {
                    self.set_remote_startup_phase(super::super::RemoteStartupPhase::Connecting);
                }
            }
            if needs_redraw {
                if !first_frame_reported {
                    // The git probe was kicked off when the app was built. Let
                    // it land (bounded) so the first frame already has the
                    // branch, dirty counts, and commits rather than growing
                    // them a frame later.
                    super::super::helpers::prime_git_info(FIRST_FRAME_GIT_BUDGET);
                }
                status_spinner_renderer.draw_full(&mut self, &mut terminal)?;
                if !first_frame_reported {
                    first_frame_reported = true;
                    report_reload_interaction_gap();
                }
                // Close the startup-profile gap: `pre_run_remote` is the last
                // pre-loop mark, so the first completed paint here is the real
                // process-to-first-frame point. Logged once via a static guard so
                // the end-to-end launch cost (including the ~5ms first draw) is
                // visible in the startup profile without re-marking every frame.
                {
                    use std::sync::atomic::{AtomicBool, Ordering};
                    static FIRST_FRAME_MARKED: AtomicBool = AtomicBool::new(false);
                    if !FIRST_FRAME_MARKED.swap(true, Ordering::Relaxed) {
                        crate::startup_profile::mark("first_frame");
                        crate::startup_profile::report_to_log();
                    }
                }
                reset_status_spinner_interval(&mut status_spinner_interval, &self);
                needs_redraw = false;
            }

            let session_to_resume = self.reconnect_target_session_id();

            let mut remote_conn = match remote::connect_with_retry(
                &mut self,
                &mut terminal,
                &mut event_stream,
                &mut remote_state,
                session_to_resume.as_deref(),
                remote_working_dir.as_deref(),
            )
            .await?
            {
                remote::ConnectOutcome::Connected(remote) => remote,
                remote::ConnectOutcome::Retry => continue,
                remote::ConnectOutcome::Quit => break 'outer,
            };
            status_spinner_renderer.invalidate();

            match remote::handle_post_connect(
                &mut self,
                &mut terminal,
                &mut remote_conn,
                &mut remote_state,
                session_to_resume.as_deref(),
            )
            .await?
            {
                remote::PostConnectOutcome::Ready => {}
                remote::PostConnectOutcome::Quit => break 'outer,
            }
            status_spinner_renderer.invalidate();
            needs_redraw = true;
            // The Pong lists what this server supports. Until it arrives no
            // focus is reported, which the server treats as focused.
            if let Err(error) = remote_conn.request_capabilities().await {
                crate::logging::warn(&format!("Cannot request remote capabilities: {error}"));
            }

            let mut bus_receiver_remote = Bus::global().subscribe();
            if let Some(status) = Bus::global().latest_update_status() {
                self.handle_update_status(status);
                needs_redraw = true;
            }

            let mut input_redraw = true;
            let mut deferred_redraw_at: Option<tokio::time::Instant> = None;
            // See `spinner_drives_deadlines`.
            let mut deadlines_were_on_spinner = false;
            let mut last_loop_tick = tokio::time::Instant::now();
            // While set, server events stay in the socket buffer. An unfocused
            // client mirroring a turn was woken about 19 times a second by
            // them, one event per wakeup, and nothing on screen needs that
            // rate: text is revealed in REDRAW_UNFOCUSED_STREAM batches.
            let mut server_events_held_until: Option<tokio::time::Instant> = None;
            // Main event loop
            loop {
                self.sync_sleep_guard();
                // Sends only when focus or visibility changed since the last report.
                let focused = self.client_focused();
                remote_conn
                    .sync_focus(focused, crate::tui::window_visibility::hidden(focused))
                    .await;
                // The reload fast-start transcript is rendered off-thread. Show
                // it as soon as it is ready instead of on the next tick: during
                // remote startup ticks are 1s apart, which held a ready
                // transcript back 0.5-0.7s until server History landed anyway.
                needs_redraw |= self.poll_pending_startup_transcript();
                let desired_redraw = if self.pending_startup_transcript.is_some() {
                    // Nothing wakes the loop when the transcript thread
                    // finishes, so poll briefly until it does (~tens of ms).
                    STARTUP_TRANSCRIPT_POLL
                } else if self.cloud_job.is_some() {
                    Duration::from_millis(100)
                } else {
                    crate::tui::redraw_interval(&self)
                };
                if desired_redraw != redraw_period {
                    redraw_period = desired_redraw;
                    redraw_interval = redraw_timer(redraw_period);
                }

                if needs_redraw {
                    // Throttle idle full-frame renders while the terminal is
                    // backgrounded (FocusLost). An unfocused, idle session has
                    // nothing changing worth a 60fps repaint, so it should not
                    // repaint at full rate just because other sessions on the
                    // shared server broadcast bus updates -- that is what made a
                    // swarm of background windows saturate the CPU. We keep full-
                    // rate redraws while streaming/processing so visible-but-
                    // unfocused windows in a tiling WM still show live progress,
                    // and set_client_focused(true) forces a full repaint on refocus.
                    let allow_redraw = self.client_focused()
                        || self.unfocused_redraw_warranted()
                        || last_unfocused_draw
                            .map(|t| t.elapsed() >= UNFOCUSED_IDLE_REDRAW_MIN_INTERVAL)
                            .unwrap_or(true);
                    // Coalesce event-driven bursts: server events can land a
                    // few ms apart and each used to trigger a full draw that
                    // changed nothing. Input-driven and forced redraws still
                    // paint immediately so typing latency is unaffected.
                    let coalesce_until =
                        (!input_redraw && !self.force_full_redraw && !self.force_full_repaint)
                            .then_some(status_spinner_renderer.last_full_frame_at)
                            .flatten()
                            .map(|at| at + EVENT_REDRAW_MIN_INTERVAL)
                            .filter(|deadline| *deadline > Instant::now());
                    deferred_redraw_at = coalesce_until.map(tokio::time::Instant::from_std);
                    if allow_redraw && deferred_redraw_at.is_none() {
                        input_redraw = false;
                        // Idle animated screen: repaint only the animation rows
                        // when nothing else is due (see the local loop).
                        if status_spinner_renderer.idle_animation_only_available(&self)
                            && status_spinner_renderer
                                .draw_idle_animation_only(&self, &mut terminal)?
                        {
                            needs_redraw = false;
                        } else {
                            status_spinner_renderer.draw_full(&mut self, &mut terminal)?;
                            reset_status_spinner_interval(&mut status_spinner_interval, &self);
                            if let Some(native) = handterm_native_scroll.as_mut() {
                                native.sync_from_app(&self);
                            }
                            last_unfocused_draw =
                                (!self.client_focused()).then(std::time::Instant::now);
                            needs_redraw = false;
                        }
                    }
                    // When unfocused and throttled, leave needs_redraw set so the
                    // pending update is coalesced into the next allowed frame.
                }

                refresh_preview_redraw_timer(&self, &mut redraw_period, &mut redraw_interval);

                if self.should_quit {
                    break 'outer;
                }

                if self.pending_queued_dispatch && !self.cloud_handoff_blocks_work() {
                    self.pending_queued_dispatch = false;
                    remote::process_remote_followups(&mut self, &mut remote_conn).await;
                    needs_redraw = true;
                    continue;
                }

                let spinner_available = status_spinner_renderer.spinner_only_available(&self);
                let on_spinner = spinner_drives_deadlines(
                    spinner_available,
                    self.client_focused(),
                    redraw_period,
                );
                if on_spinner != deadlines_were_on_spinner {
                    deadlines_were_on_spinner = on_spinner;
                    last_loop_tick = tokio::time::Instant::now();
                    if !on_spinner {
                        // The runtime timer was not polled meanwhile. Restart
                        // it so it does not fire at once for the gap.
                        redraw_interval = redraw_timer(redraw_period);
                    }
                }

                tokio::select! {
                    // Poll in declaration order so user input always wins the
                    // race against server/bus chatter. During heavy streaming
                    // the remote event branch is almost always ready; with the
                    // default random polling it repeatedly outcompetes buffered
                    // keystrokes, which shows up as a laggy, stuttering input
                    // line while a turn is running.
                    biased;
                    event = event_stream.next() => {
                        super::super::wake_stats::note("terminal");
                        if event.is_some() {
                            let event_redraw = remote::handle_terminal_event(&mut self, &mut terminal, &mut remote_conn, event).await?;
                            needs_redraw |= event_redraw;
                            input_redraw |= event_redraw;
                        } else if super::super::terminal_liveness::terminal_abandoned() {
                            // Input EOF with the controlling terminal gone:
                            // orphaned client (see local loop). Exit; the
                            // server-side session keeps running and can be
                            // reattached with --resume.
                            crate::logging::warn(
                                "Terminal input closed and controlling terminal is gone; exiting orphaned client",
                            );
                            self.should_quit = true;
                        } else {
                            tokio::time::sleep(redraw_period).await;
                        }
                    }
                    _ = status_spinner_interval.tick(), if spinner_available => {
                        super::super::wake_stats::note("spinner");
                        if !status_spinner_renderer.draw_status_spinner_only(&self, &mut terminal)? {
                            needs_redraw = true;
                        }
                        if on_spinner {
                            let now = tokio::time::Instant::now();
                            if server_events_held_until
                                .is_some_and(|at| deadline_due_on_spinner_tick(at, now))
                            {
                                super::super::wake_stats::note("server_events_released");
                                server_events_held_until = None;
                            }
                            if deferred_redraw_at
                                .is_some_and(|at| deadline_due_on_spinner_tick(at, now))
                            {
                                super::super::wake_stats::note("deferred_redraw");
                                deferred_redraw_at = None;
                            }
                            if deadline_due_on_spinner_tick(last_loop_tick + redraw_period, now) {
                                last_loop_tick = now;
                                super::super::wake_stats::note("tick");
                                needs_redraw |=
                                    remote::handle_tick(&mut self, &mut remote_conn).await;
                            }
                        }
                    }
                    _ = redraw_interval.tick(), if !on_spinner => {
                        super::super::wake_stats::note("tick");
                        needs_redraw |= remote::handle_tick(&mut self, &mut remote_conn).await;
                    }
                    _ = async {
                        match deferred_redraw_at {
                            Some(at) => tokio::time::sleep_until(at).await,
                            None => futures::future::pending::<()>().await,
                        }
                    }, if !on_spinner => {
                        super::super::wake_stats::note("deferred_redraw");
                        deferred_redraw_at = None;
                    }
                    _ = async {
                        match server_events_held_until {
                            Some(at) => tokio::time::sleep_until(at).await,
                            None => futures::future::pending::<()>().await,
                        }
                    }, if !on_spinner => {
                        super::super::wake_stats::note("server_events_released");
                        server_events_held_until = None;
                    }
                    event = remote_conn.next_event(), if server_events_held_until.is_none() || self.client_focused() => {
                        server_events_held_until = None;
                        let mut event = event;
                        let mut drained = 0usize;
                        loop {
                            match &event {
                                crate::tui::backend::RemoteRead::Event(crate::protocol::ServerEvent::Pong { capabilities, .. }) => {
                                    remote_conn.note_server_capabilities(capabilities);
                                    super::super::wake_stats::note("server:pong");
                                }
                                crate::tui::backend::RemoteRead::Event(event) => super::super::wake_stats::note_debug("server", event),
                                crate::tui::backend::RemoteRead::Disconnected(_) => super::super::wake_stats::note("server:closed"),
                            }
                            let (outcome, event_redraw) = remote::handle_remote_event(
                                &mut self,
                                &mut terminal,
                                &mut remote_conn,
                                &mut remote_state,
                                event,
                            )
                            .await?;
                            needs_redraw |= event_redraw;
                            match outcome {
                                remote::RemoteEventOutcome::Continue => {}
                                remote::RemoteEventOutcome::Reconnect => continue 'outer,
                                remote::RemoteEventOutcome::Quit => break 'outer,
                            }
                            // Unfocused mid-turn: take everything already
                            // delivered in this one wakeup, then leave the
                            // socket alone for one text batch. Bounded so a
                            // flood cannot starve input or drawing.
                            if !hold_server_events_while_unfocused(&self) {
                                break;
                            }
                            drained += 1;
                            if drained >= SERVER_EVENT_DRAIN_MAX {
                                break;
                            }
                            match futures::FutureExt::now_or_never(remote_conn.next_event()) {
                                Some(next) => event = next,
                                None => {
                                    server_events_held_until = Some(
                                        tokio::time::Instant::now()
                                            + crate::tui::redraw_schedule::unfocused_event_hold(
                                                self.client_focused(),
                                            ),
                                    );
                                    break;
                                }
                            }
                        }
                    }
                    command = async {
                        match handterm_native_scroll.as_mut() {
                            Some(native) => native.recv().await,
                            None => futures::future::pending::<Option<super::super::handterm_native_scroll::HostToApp>>().await,
                        }
                    } => {
                        if let Some(command) = command {
                            self.apply_handterm_native_scroll(command);
                            self.request_full_redraw();
                            needs_redraw = true;
                        } else {
                            handterm_native_scroll = None;
                        }
                    }
                    bus_event = bus_receiver_remote.recv() => {
                        match &bus_event {
                            Ok(event) => super::super::wake_stats::note_debug("bus", event),
                            Err(_) => super::super::wake_stats::note("bus:lagged"),
                        }
                        needs_redraw |= remote::handle_bus_event(&mut self, &mut remote_conn, bus_event).await;
                    }
                }
            }
        }

        Ok(RunResult {
            reload_session: self.reload_requested.take(),
            rebuild_session: self.rebuild_requested.take(),
            update_session: self.update_requested.take(),
            restart_session: self.restart_requested.take(),
            hibernate_session: self.hibernate_exit_requested.take(),
            cloud_handoff: self.cloud_handoff_requested.take(),
            exit_code: self.requested_exit_code,
            session_id: if self.is_remote {
                self.remote_session_id.clone()
            } else {
                Some(self.session.id.clone())
            },
        })
    }
}
