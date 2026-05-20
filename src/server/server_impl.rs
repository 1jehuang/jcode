use super::*;

impl Server {
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        use crate::id::{new_memorable_server_id, server_icon};

        let (event_tx, _) = broadcast::channel(1024);
        let (client_debug_response_tx, _) = broadcast::channel(64);

        // Generate a memorable server name
        let (id, name) = new_memorable_server_id();
        let icon = server_icon(&name).to_string();
        let identity = ServerIdentity {
            id,
            name,
            icon,
            git_hash: env!("JCODE_GIT_HASH").to_string(),
            version: env!("JCODE_VERSION").to_string(),
        };
        crate::process_title::set_server_title(&identity.name);

        // Initialize the background runner even when ambient mode is disabled so
        // session-targeted scheduled tasks still have a live delivery loop.
        let ambient_runner = {
            let safety = Arc::new(crate::safety::SafetySystem::new());
            let handle = AmbientRunnerHandle::new(safety);
            crate::tool::ambient::init_schedule_runner(handle.clone());
            Some(handle)
        };

        let LoadedSwarmRuntimeState {
            plans: restored_swarm_plans,
            coordinators: restored_swarm_coordinators,
            members: restored_swarm_members,
            swarms_by_id: restored_swarms_by_id,
        } = load_persisted_swarm_runtime_state();

        // Initialize collaboration server for real-time multi-user editing
        let collab_server = Arc::new(collab::CollaborationServer::new(collab::CollabConfig::default()));

        // Initialize LSP manager, bridge, and conflict detector
        // We create the conflict detector immediately for symbol-level conflict detection,
        // LSP manager will be initialized on-demand when needed
        let (lsp_manager, lsp_event_bridge, conflict_detector) = {
            // Try to create LSP manager if we can
            let manager = Arc::new(jcode_lsp::LspServerManager::new());
            let lsp_manager = Some(Arc::clone(&manager));
            let conflict_detector = Some(Arc::new(SymbolConflictDetector::new(manager)));
            (lsp_manager, None, conflict_detector)
        };

        Self {
            provider,
            socket_path: socket_path(),
            debug_socket_path: debug_socket_path(),
            gateway_config_override: None,
            identity,
            event_tx,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            is_processing: Arc::new(RwLock::new(false)),
            session_id: Arc::new(RwLock::new(String::new())),
            client_count: Arc::new(RwLock::new(0)),
            client_connections: Arc::new(RwLock::new(HashMap::new())),
            file_touches: Arc::new(RwLock::new(HashMap::new())),
            files_touched_by_session: Arc::new(RwLock::new(HashMap::new())),
            swarm_state: SwarmState::new(
                restored_swarm_members,
                restored_swarms_by_id,
                restored_swarm_plans,
                restored_swarm_coordinators,
            ),
            shared_context: Arc::new(RwLock::new(HashMap::new())),
            client_debug_state: Arc::new(RwLock::new(ClientDebugState::default())),
            client_debug_response_tx,
            debug_jobs: Arc::new(RwLock::new(HashMap::new())),
            channel_subscriptions: Arc::new(RwLock::new(HashMap::new())),
            channel_subscriptions_by_session: Arc::new(RwLock::new(HashMap::new())),
            event_history: Arc::new(RwLock::new(std::collections::VecDeque::new())),
            event_counter: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            swarm_event_tx: broadcast::channel(256).0,
            ambient_runner,
            mcp_pool: Arc::new(OnceCell::new()),
            shutdown_signals: Arc::new(RwLock::new(HashMap::new())),
            soft_interrupt_queues: Arc::new(RwLock::new(HashMap::new())),
            await_members_runtime: AwaitMembersRuntime::default(),
            swarm_mutation_runtime: SwarmMutationRuntime::default(),
            collab_server,
            lsp_manager,
            lsp_event_bridge,
            conflict_detector,
        }
    }

    pub fn new_with_paths(
        provider: Arc<dyn Provider>,
        socket_path: PathBuf,
        debug_socket_path: PathBuf,
    ) -> Self {
        let mut server = Self::new(provider);
        server.socket_path = socket_path;
        server.debug_socket_path = debug_socket_path;
        server
    }

    pub fn with_gateway_config(mut self, gateway_config: crate::gateway::GatewayConfig) -> Self {
        self.gateway_config_override = Some(gateway_config);
        self
    }

    /// Get the server identity
    pub fn identity(&self) -> &ServerIdentity {
        &self.identity
    }

    /// Get the collaboration server for real-time editing
    pub fn collab_server(&self) -> Arc<collab::CollaborationServer> {
        self.collab_server.clone()
    }

    /// Get the LSP server manager if available
    pub fn lsp_manager(&self) -> Option<Arc<jcode_lsp::LspServerManager>> {
        self.lsp_manager.clone()
    }

    /// Get the LSP event bridge if available
    pub fn lsp_event_bridge(&self) -> Option<Arc<LspEventBridge>> {
        self.lsp_event_bridge.clone()
    }

    /// Get the symbol conflict detector if available
    pub fn conflict_detector(&self) -> Option<Arc<SymbolConflictDetector>> {
        self.conflict_detector.clone()
    }

    /// Enable LSP features for this server instance.
    ///
    /// This initializes the LSP manager and optionally starts the event bridge
    /// for a specific swarm session. Call this when a swarm session needs LSP support.
    pub async fn enable_lsp_features(
        &mut self,
        swarm_channel: Arc<RwLock<jcode_swarm_core::ChannelIndex>>,
        swarm_id: String,
    ) -> Result<(), anyhow::Error> {
        use tracing::info;

        // Create LSP manager if not already present
        if self.lsp_manager.is_none() {
            info!("Initializing LSP server manager");
            let manager = Arc::new(jcode_lsp::LspServerManager::new());
            self.lsp_manager = Some(manager);
        }

        // Create and start LSP event bridge if we have a manager
        if let Some(ref manager) = self.lsp_manager {
            // Create bridge
            let bridge = Arc::new(LspEventBridge::new(
                manager.clone(),
                swarm_channel,
                swarm_id.clone(),
            ));

            // Start the bridge in background
            info!("Starting LSP event bridge for swarm {}", swarm_id);
            bridge.start();

            // Store the bridge
            self.lsp_event_bridge = Some(bridge);

            // Create conflict detector
            let detector = Arc::new(SymbolConflictDetector::new(manager.clone()));
            self.conflict_detector = Some(detector);

            info!("LSP features enabled successfully for swarm {}", swarm_id);
        }

        Ok(())
    }

    fn runtime(&self) -> ServerRuntime {
        ServerRuntime::from_server(self)
    }

    fn build_registry_info(&self) -> crate::registry::ServerInfo {
        crate::registry::ServerInfo {
            id: self.identity.id.clone(),
            name: self.identity.name.clone(),
            icon: self.identity.icon.clone(),
            socket: self.socket_path.clone(),
            debug_socket: self.debug_socket_path.clone(),
            git_hash: self.identity.git_hash.clone(),
            version: self.identity.version.clone(),
            pid: std::process::id(),
            started_at: chrono::Utc::now().to_rfc3339(),
            sessions: Vec::new(),
        }
    }

    fn spawn_registry_prewarm(&self) {
        let registry_warm_provider = Arc::clone(&self.provider);
        tokio::spawn(async move {
            let start = Instant::now();
            let provider = registry_warm_provider.fork();
            let _ = crate::tool::Registry::new(provider).await;
            crate::logging::info(&format!(
                "Registry prewarm completed in {}ms",
                start.elapsed().as_millis()
            ));
        });
    }

    async fn recover_headless_sessions_on_startup(&self) {
        let sessions_to_restore = {
            let members = self.swarm_state.members.read().await;
            members
                .values()
                .filter(|member| headless_member_should_restore(&member.status, member.is_headless))
                .map(|member| member.session_id.clone())
                .collect::<Vec<_>>()
        };

        if sessions_to_restore.is_empty() {
            return;
        }

        crate::logging::info(&format!(
            "Recovering {} headless session(s) after startup: {:?}",
            sessions_to_restore.len(),
            sessions_to_restore
        ));

        if let Some(delay) = startup_headless_recovery_test_delay() {
            crate::logging::info(&format!(
                "Applying test-only headless startup recovery delay of {}ms",
                delay.as_millis()
            ));
            tokio::time::sleep(delay).await;
        }

        let mcp_pool = get_shared_mcp_pool(&self.mcp_pool).await;
        let recovery_started = Instant::now();
        let mut stats = HeadlessRecoveryStats::default();
        let mut swarms_to_persist = HashSet::new();

        for session_id in sessions_to_restore {
            stats.candidates += 1;
            let session = match crate::session::Session::load(&session_id) {
                Ok(session) => session,
                Err(error) => {
                    stats.failed_to_load += 1;
                    crate::logging::warn(&format!(
                        "Failed to load headless session {} during startup recovery: {}",
                        session_id, error
                    ));
                    update_member_status(
                        &session_id,
                        "failed",
                        Some(truncate_detail(&error.to_string(), 120)),
                        &self.swarm_state.members,
                        &self.swarm_state.swarms_by_id,
                        Some(&self.event_history),
                        Some(&self.event_counter),
                        Some(&self.swarm_event_tx),
                    )
                    .await;
                    if let Some(swarm_id) = {
                        let members = self.swarm_state.members.read().await;
                        members
                            .get(&session_id)
                            .and_then(|member| member.swarm_id.clone())
                    } {
                        persist_swarm_state_for(&swarm_id, &self.swarm_state).await;
                    }
                    continue;
                }
            };

            let previous_status = session.status.clone();
            let provider = self.provider.fork();
            let registry = crate::tool::Registry::new(provider.clone()).await;
            if session.is_canary {
                registry.register_selfdev_tools().await;
            }
            registry
                .register_mcp_tools(
                    None,
                    Some(Arc::clone(&mcp_pool)),
                    Some("headless".to_string()),
                )
                .await;

            let agent = Arc::new(Mutex::new(Agent::new_with_session(
                provider, registry, session, None,
            )));

            {
                let mut sessions = self.sessions.write().await;
                if sessions.contains_key(&session_id) {
                    continue;
                }
                sessions.insert(session_id.clone(), Arc::clone(&agent));
            }

            {
                let agent_guard = agent.lock().await;
                register_session_interrupt_queue(
                    &self.soft_interrupt_queues,
                    &session_id,
                    agent_guard.soft_interrupt_queue(),
                )
                .await;
                let mut shutdown_signals = self.shutdown_signals.write().await;
                shutdown_signals.insert(session_id.clone(), agent_guard.graceful_shutdown_signal());
            }

            let has_stored_recovery_intent = reload_recovery::has_pending_for_session(&session_id);
            let should_resume = has_stored_recovery_intent || {
                let agent_guard = agent.lock().await;
                self::client_session::restored_session_was_interrupted(
                    &session_id,
                    &previous_status,
                    &agent_guard,
                )
            };

            if !should_resume {
                ReloadContext::log_recovery_outcome(
                    "server_startup_headless",
                    &session_id,
                    "skipped",
                    "restored session was not interrupted by reload",
                );
                stats.skipped += 1;
                update_member_status(
                    &session_id,
                    "ready",
                    None,
                    &self.swarm_state.members,
                    &self.swarm_state.swarms_by_id,
                    Some(&self.event_history),
                    Some(&self.event_counter),
                    Some(&self.swarm_event_tx),
                )
                .await;
                if let Some(swarm_id) = {
                    let members = self.swarm_state.members.read().await;
                    members
                        .get(&session_id)
                        .and_then(|member| member.swarm_id.clone())
                } {
                    swarms_to_persist.insert(swarm_id);
                }
                continue;
            }

            let stored_directive = reload_recovery::claim_pending_for_session(&session_id)
                .ok()
                .flatten();
            let reload_ctx = if stored_directive.is_none() {
                ReloadContext::load_for_session(&session_id).ok().flatten()
            } else {
                None
            };
            let reminder = stored_directive
                .map(|directive| directive.continuation_message)
                .or_else(|| headless_reload_continuation_message(reload_ctx));
            let Some(reminder) = reminder else {
                ReloadContext::log_recovery_outcome(
                    "server_startup_headless",
                    &session_id,
                    "failed",
                    "recovery directive missing for interrupted headless session",
                );
                continue;
            };
            stats.resumed += 1;
            ReloadContext::log_recovery_outcome(
                "server_startup_headless",
                &session_id,
                "resuming",
                "restored interrupted headless session after reload",
            );
            let recover_swarm_members = Arc::clone(&self.swarm_state.members);
            let recover_swarms_by_id = Arc::clone(&self.swarm_state.swarms_by_id);
            let recover_event_history = Arc::clone(&self.event_history);
            let recover_event_counter = Arc::clone(&self.event_counter);
            let recover_swarm_event_tx = self.swarm_event_tx.clone();
            let recover_swarm_state = self.swarm_state.clone();

            tokio::spawn(async move {
                update_member_status(
                    &session_id,
                    "running",
                    Some("resuming after reload".to_string()),
                    &recover_swarm_members,
                    &recover_swarms_by_id,
                    Some(&recover_event_history),
                    Some(&recover_event_counter),
                    Some(&recover_swarm_event_tx),
                )
                .await;
                if let Some(swarm_id) = {
                    let members = recover_swarm_members.read().await;
                    members
                        .get(&session_id)
                        .and_then(|member| member.swarm_id.clone())
                } {
                    persist_swarm_state_for(&swarm_id, &recover_swarm_state).await;
                }

                let event_tx = self::state::session_event_fanout_sender(
                    session_id.clone(),
                    Arc::clone(&recover_swarm_members),
                );
                let result = self::client_lifecycle::process_message_streaming_mpsc(
                    Arc::clone(&agent),
                    "",
                    vec![],
                    Some(reminder),
                    event_tx,
                )
                .await;

                let (status, detail) = match result {
                    Ok(()) => {
                        ReloadContext::log_recovery_outcome(
                            "server_startup_headless",
                            &session_id,
                            "resumed",
                            "continuation dispatched successfully",
                        );
                        ("ready", None)
                    }
                    Err(error) => {
                        ReloadContext::log_recovery_outcome(
                            "server_startup_headless",
                            &session_id,
                            "failed",
                            &error.to_string(),
                        );
                        ("failed", Some(truncate_detail(&error.to_string(), 120)))
                    }
                };
                update_member_status(
                    &session_id,
                    status,
                    detail,
                    &recover_swarm_members,
                    &recover_swarms_by_id,
                    Some(&recover_event_history),
                    Some(&recover_event_counter),
                    Some(&recover_swarm_event_tx),
                )
                .await;
                if let Some(swarm_id) = {
                    let members = recover_swarm_members.read().await;
                    members
                        .get(&session_id)
                        .and_then(|member| member.swarm_id.clone())
                } {
                    persist_swarm_state_for(&swarm_id, &recover_swarm_state).await;
                }
            });
        }

        for swarm_id in swarms_to_persist {
            persist_swarm_state_for(&swarm_id, &self.swarm_state).await;
        }

        crate::logging::info(&format!(
            "[TIMING] headless reload startup recovery: candidates={}, resumed={}, skipped={}, failed_to_load={}, total={}ms",
            stats.candidates,
            stats.resumed,
            stats.skipped,
            stats.failed_to_load,
            recovery_started.elapsed().as_millis()
        ));
    }

    async fn finish_startup_after_bind(
        &self,
        main_listener: Listener,
        debug_listener: Listener,
        server_start_time: Instant,
    ) -> (tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>) {
        self.spawn_registry_prewarm();
        let registry_info = self.build_registry_info();

        let runtime = self.runtime();
        let main_handle = runtime.spawn_main_accept_loop(main_listener);
        let debug_handle = runtime.spawn_debug_accept_loop(debug_listener, server_start_time);

        crate::logging::info("Accept loop tasks spawned");

        // Signal readiness to the spawning client only after the accept loops
        // are live, so a "ready" server can immediately handle requests.
        publish_reload_socket_ready();
        signal_ready_fd();

        // Persist auxiliary discovery metadata after the server is already live.
        self.spawn_registry_metadata_publisher(registry_info);

        // Spawn WebSocket gateway for iOS/web clients (if enabled)
        let _gateway_handle = self.spawn_gateway(runtime);

        // Startup recovery can be expensive in multi-session reloads. Run it
        // only after the replacement daemon is already accepting reconnects.
        self.recover_headless_sessions_on_startup().await;

        (main_handle, debug_handle)
    }

    fn spawn_background_tasks(
        &self,
        server_start_time: Instant,
        temporary_server_policy: Option<lifecycle::TemporaryServerPolicy>,
    ) {
        // Preload the embedding model in background so warm startups get fast
        // memory recall. On a cold install, skip eager preload because the
        // first-time model download can make the first spawned client look hung
        // while the daemon finishes bootstrapping.
        if crate::embedding::is_model_available() {
            tokio::task::spawn_blocking(|| {
                let start = std::time::Instant::now();
                match crate::embedding::get_embedder() {
                    Ok(_) => {
                        crate::logging::info(&format!(
                            "Embedding model preloaded in {}ms",
                            start.elapsed().as_millis()
                        ));
                    }
                    Err(e) => {
                        crate::logging::info(&format!(
                            "Embedding model preload failed (non-fatal): {}",
                            e
                        ));
                    }
                }
            });
        } else {
            crate::logging::info(
                "Embedding model not installed yet; skipping eager preload during server startup",
            );
        }

        // Spawn reload monitor (event-driven via in-process channel).
        // In the unified server design, self-dev sessions share the main server,
        // so the shared server must always listen for reload signals.
        let signal_sessions = Arc::clone(&self.sessions);
        let signal_swarm_members = Arc::clone(&self.swarm_state.members);
        let signal_shutdown_signals = Arc::clone(&self.shutdown_signals);
        let signal_swarm_event_tx = self.swarm_event_tx.clone();
        tokio::spawn(async move {
            await_reload_signal(
                signal_sessions,
                signal_swarm_members,
                signal_shutdown_signals,
                signal_swarm_event_tx,
            )
            .await;
        });

        // Log when we receive SIGTERM for debugging
        #[cfg(unix)]
        {
            let sigterm_server_name = self.identity.name.clone();
            tokio::spawn(async move {
                use tokio::signal::unix::{SignalKind, signal};
                if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
                    sigterm.recv().await;
                    crate::logging::info("Server received SIGTERM, shutting down gracefully");

                    // Shutdown cluster service
                    if let Err(e) = crate::distributed::shutdown_cluster_service().await {
                        crate::logging::warn(&format!("Cluster shutdown error: {}", e));
                    }

                    let _ = crate::registry::unregister_server(&sigterm_server_name).await;
                    std::process::exit(0);
                }
            });
        }

        // Cross-platform Ctrl+C handler for graceful shutdown
        {
            tokio::spawn(async move {
                if let Ok(()) = tokio::signal::ctrl_c().await {
                    crate::logging::info("Server received Ctrl+C, shutting down gracefully");

                    // Shutdown cluster service
                    if let Err(e) = crate::distributed::shutdown_cluster_service().await {
                        crate::logging::warn(&format!("Cluster shutdown error: {}", e));
                    }

                    #[cfg(unix)]
                    {
                        let _ = crate::registry::unregister_server("").await;
                    }

                    std::process::exit(0);
                }
            });
        }

        // Spawn the bus monitor for swarm coordination
        let monitor_file_touches = Arc::clone(&self.file_touches);
        let monitor_files_touched_by_session = Arc::clone(&self.files_touched_by_session);
        let monitor_swarm_members = Arc::clone(&self.swarm_state.members);
        let monitor_swarms_by_id = Arc::clone(&self.swarm_state.swarms_by_id);
        let monitor_swarm_plans = Arc::clone(&self.swarm_state.plans);
        let monitor_swarm_coordinators = Arc::clone(&self.swarm_state.coordinators);
        let monitor_shared_context = Arc::clone(&self.shared_context);
        let monitor_sessions = Arc::clone(&self.sessions);
        let monitor_soft_interrupt_queues = Arc::clone(&self.soft_interrupt_queues);
        let monitor_event_history = Arc::clone(&self.event_history);
        let monitor_event_counter = Arc::clone(&self.event_counter);
        let monitor_swarm_event_tx = self.swarm_event_tx.clone();
        tokio::spawn(async move {
            Self::monitor_bus(
                monitor_file_touches,
                monitor_files_touched_by_session,
                monitor_swarm_members,
                monitor_swarms_by_id,
                monitor_swarm_plans,
                monitor_swarm_coordinators,
                monitor_shared_context,
                monitor_sessions,
                monitor_soft_interrupt_queues,
                monitor_event_history,
                monitor_event_counter,
                monitor_swarm_event_tx,
            )
            .await;
        });

        let stale_swarm_members = Arc::clone(&self.swarm_state.members);
        let stale_swarms_by_id = Arc::clone(&self.swarm_state.swarms_by_id);
        let stale_swarm_plans = Arc::clone(&self.swarm_state.plans);
        let stale_swarm_coordinators = Arc::clone(&self.swarm_state.coordinators);
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(crate::server::swarm::swarm_task_sweep_interval());
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                refresh_swarm_task_staleness(
                    &stale_swarm_members,
                    &stale_swarms_by_id,
                    &stale_swarm_plans,
                    &stale_swarm_coordinators,
                )
                .await;
            }
        });

        // Initialize the memory agent early so it's ready for all sessions
        if crate::config::config().features.memory {
            tokio::spawn(async {
                let _ = crate::memory_agent::init().await;
            });
        }

        // Spawn the background ambient/schedule loop.
        if let Some(ref runner) = self.ambient_runner {
            let ambient_handle = runner.clone();
            let ambient_provider = Arc::clone(&self.provider);
            crate::logging::info("Starting ambient/schedule background loop");
            tokio::spawn(async move {
                ambient_handle.run_loop(ambient_provider).await;
            });
        }

        // Spawn embedding idle monitor so the model can be unloaded when this
        // server has been quiet for a while.
        let embedding_idle_secs = embedding_idle_unload_secs();
        tokio::spawn(async move {
            let idle_for = std::time::Duration::from_secs(embedding_idle_secs);
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(EMBEDDING_IDLE_CHECK_SECS));
            loop {
                interval.tick().await;
                let unloaded = crate::embedding::maybe_unload_if_idle(idle_for);
                if unloaded {
                    let stats = crate::embedding::stats();
                    crate::logging::info(&format!(
                        "Embedding idle monitor: model unloaded (loads={}, unloads={}, calls={}, avg_ms={})",
                        stats.load_count,
                        stats.unload_count,
                        stats.embed_calls,
                        stats
                            .avg_embed_ms
                            .map(|v| format!("{:.1}", v))
                            .unwrap_or_else(|| "n/a".to_string())
                    ));
                }
            }
        });

        if crate::runtime_memory_log::server_logging_enabled() {
            let log_identity = self.identity.clone();
            let log_sessions = Arc::clone(&self.sessions);
            let log_client_count = Arc::clone(&self.client_count);
            let (memory_event_tx, mut memory_event_rx) = mpsc::unbounded_channel();
            crate::runtime_memory_log::install_event_sink(memory_event_tx);
            tokio::spawn(async move {
                match crate::runtime_memory_log::prune_old_server_logs() {
                    Ok(removed) if removed > 0 => {
                        crate::logging::info(&format!(
                            "Runtime memory logging pruned {} old log files",
                            removed
                        ));
                    }
                    Ok(_) => {}
                    Err(err) => {
                        crate::logging::info(&format!(
                            "Runtime memory logging could not prune old logs: {}",
                            err
                        ));
                    }
                }

                let log_config = crate::runtime_memory_log::server_logging_config();
                match crate::runtime_memory_log::current_server_log_path() {
                    Ok(path) => crate::logging::info(&format!(
                        "Runtime memory logging enabled: process={}s attribution={}s -> {}",
                        log_config.process_interval.as_secs(),
                        log_config.attribution_interval.as_secs(),
                        path.display()
                    )),
                    Err(err) => crate::logging::info(&format!(
                        "Runtime memory logging enabled: process={}s attribution={}s (path unavailable: {})",
                        log_config.process_interval.as_secs(),
                        log_config.attribution_interval.as_secs(),
                        err
                    )),
                }

                let mut controller = RuntimeMemoryLogController::new(log_config);
                let startup_now = Instant::now();
                let mut startup_sample = capture_runtime_memory_attribution_sample(
                    &log_identity,
                    &log_sessions,
                    &log_client_count,
                    server_start_time,
                    "attribution:startup",
                    RuntimeMemoryLogTrigger {
                        category: "startup".to_string(),
                        reason: "server_start".to_string(),
                        session_id: None,
                        detail: None,
                    },
                    RuntimeMemoryLogSampling {
                        forced: true,
                        threshold_reasons: vec!["initial_attribution".to_string()],
                        pending_event_count: 0,
                        pending_categories: Vec::new(),
                    },
                )
                .await;
                controller.record_process_sample(startup_now);
                controller.finalize_attribution_sample(startup_now, &mut startup_sample);
                if let Err(err) = crate::runtime_memory_log::append_server_sample(&startup_sample) {
                    crate::logging::info(&format!(
                        "Runtime memory logging startup sample failed: {}",
                        err
                    ));
                }

                let mut process_interval =
                    tokio::time::interval(controller.config().process_interval);
                let mut attribution_interval =
                    tokio::time::interval(controller.config().attribution_interval);
                process_interval.tick().await;
                attribution_interval.tick().await;
                loop {
                    tokio::select! {
                        _ = process_interval.tick() => {
                            let now = Instant::now();
                            let process_sample = capture_runtime_memory_process_sample(
                                &log_identity,
                                &log_client_count,
                                server_start_time,
                                "process:heartbeat",
                                RuntimeMemoryLogTrigger {
                                    category: "process_heartbeat".to_string(),
                                    reason: "periodic".to_string(),
                                    session_id: None,
                                    detail: None,
                                },
                                controller.build_sampling_for_process(None),
                            )
                            .await;
                            controller.record_process_sample(now);
                            if let Err(err) = crate::runtime_memory_log::append_server_sample(&process_sample) {
                                crate::logging::info(&format!(
                                    "Runtime memory logging process heartbeat sample failed: {}",
                                    err
                                ));
                            }

                            if let Some(sampling) = controller.build_sampling_for_attribution(
                                now,
                                &process_sample.process,
                                None,
                                None,
                            ) {
                                let mut attribution_sample = capture_runtime_memory_attribution_sample(
                                    &log_identity,
                                    &log_sessions,
                                    &log_client_count,
                                    server_start_time,
                                    "attribution:process-heartbeat",
                                    RuntimeMemoryLogTrigger {
                                        category: "process_heartbeat".to_string(),
                                        reason: "threshold_flush".to_string(),
                                        session_id: None,
                                        detail: None,
                                    },
                                    sampling,
                                )
                                .await;
                                controller.finalize_attribution_sample(now, &mut attribution_sample);
                                if let Err(err) = crate::runtime_memory_log::append_server_sample(&attribution_sample) {
                                    crate::logging::info(&format!(
                                        "Runtime memory logging attribution flush failed: {}",
                                        err
                                    ));
                                }
                            }
                        }
                        _ = attribution_interval.tick() => {
                            let now = Instant::now();
                            let preflight = capture_runtime_memory_process_sample(
                                &log_identity,
                                &log_client_count,
                                server_start_time,
                                "process:attribution-preflight",
                                RuntimeMemoryLogTrigger {
                                    category: "attribution_heartbeat".to_string(),
                                    reason: "preflight".to_string(),
                                    session_id: None,
                                    detail: None,
                                },
                                RuntimeMemoryLogSampling::default(),
                            )
                            .await;
                            if let Some(sampling) = controller.build_sampling_for_attribution(
                                now,
                                &preflight.process,
                                None,
                                Some("attribution_heartbeat"),
                            ) {
                                let mut attribution_sample = capture_runtime_memory_attribution_sample(
                                    &log_identity,
                                    &log_sessions,
                                    &log_client_count,
                                    server_start_time,
                                    "attribution:heartbeat",
                                    RuntimeMemoryLogTrigger {
                                        category: "attribution_heartbeat".to_string(),
                                        reason: "periodic".to_string(),
                                        session_id: None,
                                        detail: None,
                                    },
                                    sampling,
                                )
                                .await;
                                controller.finalize_attribution_sample(now, &mut attribution_sample);
                                if let Err(err) = crate::runtime_memory_log::append_server_sample(&attribution_sample) {
                                    crate::logging::info(&format!(
                                        "Runtime memory logging attribution heartbeat failed: {}",
                                        err
                                    ));
                                }
                            } else {
                                controller.mark_attribution_heartbeat_pending();
                            }
                        }
                        maybe_event = memory_event_rx.recv() => {
                            let Some(event) = maybe_event else {
                                break;
                            };
                            let now = Instant::now();
                            let should_write_process = controller.should_write_process_for_event(now, &event);
                            let process_sample = if should_write_process {
                                Some(
                                    capture_runtime_memory_process_sample(
                                        &log_identity,
                                        &log_client_count,
                                        server_start_time,
                                        &format!("process:event:{}", event.category),
                                        RuntimeMemoryLogTrigger {
                                            category: event.category.clone(),
                                            reason: event.reason.clone(),
                                            session_id: event.session_id.clone(),
                                            detail: event.detail.clone(),
                                        },
                                        controller.build_sampling_for_process(Some(&event)),
                                    )
                                    .await,
                                )
                            } else {
                                None
                            };

                            if let Some(process_sample) = process_sample.as_ref() {
                                controller.record_process_sample(now);
                                if let Err(err) = crate::runtime_memory_log::append_server_sample(process_sample) {
                                    crate::logging::info(&format!(
                                        "Runtime memory logging event process sample failed: {}",
                                        err
                                    ));
                                }
                            }

                            let mut wrote_attribution = false;
                            let preflight_sample = if process_sample.is_none() && controller.can_write_attribution(now) {
                                Some(
                                    capture_runtime_memory_process_sample(
                                        &log_identity,
                                        &log_client_count,
                                        server_start_time,
                                        &format!("process:event-preflight:{}", event.category),
                                        RuntimeMemoryLogTrigger {
                                            category: event.category.clone(),
                                            reason: "preflight".to_string(),
                                            session_id: event.session_id.clone(),
                                            detail: event.detail.clone(),
                                        },
                                        RuntimeMemoryLogSampling::default(),
                                    )
                                    .await,
                                )
                            } else {
                                None
                            };
                            let preflight = process_sample.as_ref().or(preflight_sample.as_ref());
                            if let Some(preflight) = preflight
                                && let Some(sampling) = controller.build_sampling_for_attribution(
                                    now,
                                    &preflight.process,
                                    Some(&event),
                                    None,
                                )
                            {
                                    let mut attribution_sample = capture_runtime_memory_attribution_sample(
                                        &log_identity,
                                        &log_sessions,
                                        &log_client_count,
                                        server_start_time,
                                        &format!("attribution:event:{}", event.category),
                                        RuntimeMemoryLogTrigger {
                                            category: event.category.clone(),
                                            reason: event.reason.clone(),
                                            session_id: event.session_id.clone(),
                                            detail: event.detail.clone(),
                                        },
                                        sampling,
                                    )
                                    .await;
                                    controller.finalize_attribution_sample(now, &mut attribution_sample);
                                    wrote_attribution = true;
                                    if let Err(err) = crate::runtime_memory_log::append_server_sample(&attribution_sample) {
                                        crate::logging::info(&format!(
                                            "Runtime memory logging event attribution sample failed: {}",
                                            err
                                        ));
                                    }
                                }

                            if !wrote_attribution {
                                controller.defer_event(event);
                            }
                        }
                    }
                }
            });
        }

        if let Some(policy) = temporary_server_policy {
            lifecycle::spawn_temporary_lifecycle_monitor(
                Arc::clone(&self.client_count),
                self.socket_path.clone(),
                self.debug_socket_path.clone(),
                self.identity.name.clone(),
                policy,
            );
        } else if debug_control_allowed() {
            crate::logging::info("Debug control enabled; idle timeout monitor disabled.");
        } else {
            let idle_client_count = Arc::clone(&self.client_count);
            let idle_server_name = self.identity.name.clone();
            tokio::spawn(async move {
                let mut idle_since: Option<std::time::Instant> = None;
                let mut check_interval = tokio::time::interval(std::time::Duration::from_secs(10));

                loop {
                    check_interval.tick().await;

                    let count = *idle_client_count.read().await;

                    if count == 0 {
                        // No clients connected
                        if idle_since.is_none() {
                            idle_since = Some(std::time::Instant::now());
                            crate::logging::info(&format!(
                                "No clients connected. Server will exit after {} minutes of idle.",
                                IDLE_TIMEOUT_SECS / 60
                            ));
                        }

                        if let Some(since) = idle_since {
                            let idle_duration = since.elapsed().as_secs();
                            if idle_duration >= IDLE_TIMEOUT_SECS {
                                crate::logging::info(&format!(
                                    "Server idle for {} minutes with no clients. Shutting down.",
                                    idle_duration / 60
                                ));
                                let _ = crate::registry::unregister_server(&idle_server_name).await;
                                std::process::exit(EXIT_IDLE_TIMEOUT);
                            }
                        }
                    } else {
                        // Clients connected - reset idle timer
                        if idle_since.is_some() {
                            crate::logging::info("Client connected. Idle timer cancelled.");
                        }
                        idle_since = None;
                    }
                }
            });
        }
    }

    fn spawn_registry_metadata_publisher(&self, registry_info: crate::registry::ServerInfo) {
        let registry_identity = self.identity.display_name();
        tokio::spawn(async move {
            let hash_path = format!("{}.hash", registry_info.socket.display());
            let _ = std::fs::write(&hash_path, env!("JCODE_GIT_HASH"));

            let mut registry = crate::registry::ServerRegistry::load()
                .await
                .unwrap_or_default();
            registry.register(registry_info);
            let _ = registry.save().await;
            crate::logging::info(&format!(
                "Registered as {} in server registry",
                registry_identity,
            ));

            if let Ok(mut registry) = crate::registry::ServerRegistry::load().await {
                let _ = registry.cleanup_stale().await;
                let _ = registry.save().await;
            }
        });
    }

    /// Monitor the global Bus for FileTouch events and detect conflicts
    #[expect(
        clippy::too_many_arguments,
        reason = "bus monitor needs file state, swarm state, sessions, queues, and event history sinks"
    )]
    async fn monitor_bus(
        file_touches: Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
        files_touched_by_session: Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
        swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
        swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
        _swarm_plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
        _swarm_coordinators: Arc<RwLock<HashMap<String, String>>>,
        _shared_context: Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
        sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
        soft_interrupt_queues: SessionInterruptQueues,
        event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
        event_counter: Arc<std::sync::atomic::AtomicU64>,
        swarm_event_tx: broadcast::Sender<SwarmEvent>,
    ) {
        let mut receiver = Bus::global().subscribe();
        let mut last_cleanup = Instant::now();
        const TOUCH_EXPIRY: Duration = Duration::from_secs(30 * 60); // 30 min
        const CLEANUP_INTERVAL: Duration = Duration::from_secs(5 * 60); // 5 min

        loop {
            // Periodic cleanup of expired file touches
            if last_cleanup.elapsed() > CLEANUP_INTERVAL {
                let mut touches = file_touches.write().await;
                let now = Instant::now();
                touches.retain(|_, accesses| {
                    accesses.retain(|a| now.duration_since(a.timestamp) < TOUCH_EXPIRY);
                    !accesses.is_empty()
                });
                let mut rebuilt_reverse_index: HashMap<String, HashSet<PathBuf>> = HashMap::new();
                for (path, accesses) in touches.iter() {
                    for access in accesses {
                        rebuilt_reverse_index
                            .entry(access.session_id.clone())
                            .or_default()
                            .insert(path.clone());
                    }
                }
                drop(touches);
                *files_touched_by_session.write().await = rebuilt_reverse_index;
                last_cleanup = Instant::now();
            }

            match receiver.recv().await {
                Ok(BusEvent::FileTouch(touch)) => {
                    let path = touch.path.clone();
                    let session_id = touch.session_id.clone();

                    // Record this touch
                    {
                        let mut touches = file_touches.write().await;
                        let accesses = touches.entry(path.clone()).or_insert_with(Vec::new);
                        accesses.push(FileAccess {
                            session_id: session_id.clone(),
                            op: touch.op.clone(),
                            timestamp: Instant::now(),
                            absolute_time: std::time::SystemTime::now(),
                            summary: touch.summary.clone(),
                            detail: touch.detail.clone(),
                        });
                    }
                    {
                        let mut reverse_index = files_touched_by_session.write().await;
                        reverse_index
                            .entry(session_id.clone())
                            .or_default()
                            .insert(path.clone());
                    }

                    // Record event for subscription
                    {
                        let members = swarm_members.read().await;
                        let member = members.get(&session_id);
                        let session_name = member.and_then(|m| m.friendly_name.clone());
                        let swarm_id = member.and_then(|m| m.swarm_id.clone());

                        drop(members);
                        record_swarm_event(
                            &event_history,
                            &event_counter,
                            &swarm_event_tx,
                            session_id.clone(),
                            session_name,
                            swarm_id,
                            SwarmEventType::FileTouch {
                                path: path.to_string_lossy().to_string(),
                                op: touch.op.as_str().to_string(),
                                summary: touch.summary.clone(),
                                detail: touch.detail.clone(),
                            },
                        )
                        .await;
                    }

                    // Find the swarm this session belongs to
                    let swarm_session_ids: Vec<String> = {
                        let members = swarm_members.read().await;
                        if let Some(member) = members.get(&session_id) {
                            if let Some(ref swarm_id) = member.swarm_id {
                                let swarms = swarms_by_id.read().await;
                                if let Some(swarm) = swarms.get(swarm_id) {
                                    swarm.iter().cloned().collect()
                                } else {
                                    vec![]
                                }
                            } else {
                                vec![]
                            }
                        } else {
                            vec![]
                        }
                    };

                    // Only notify on modifications, and only about prior peer modifications.
                    // Plain reads are still tracked for later context/listing but should not
                    // proactively alert the swarm.
                    let is_modification = touch.op.is_modification();
                    if is_modification {
                        crate::logging::info(&format!(
                            "[file-activity] modification by {} on {}, swarm_peers: {:?}",
                            &session_id[..8.min(session_id.len())],
                            path.display(),
                            swarm_session_ids
                                .iter()
                                .map(|s| &s[..8.min(s.len())])
                                .collect::<Vec<_>>()
                        ));
                    }
                    let previous_touches: Vec<FileAccess> = if is_modification {
                        let touches = file_touches.read().await;
                        if let Some(accesses) = touches.get(&path) {
                            let swarm_session_ids_set: HashSet<String> =
                                swarm_session_ids.iter().cloned().collect();
                            let result =
                                latest_peer_touches(accesses, &session_id, &swarm_session_ids_set);
                            crate::logging::info(&format!(
                                "[file-activity] {} prior peer touches ({} total accesses)",
                                result.len(),
                                accesses.len()
                            ));
                            result
                        } else {
                            crate::logging::info("[file-activity] no touches for this path yet");
                            vec![]
                        }
                    } else {
                        vec![]
                    };

                    // If swarm peers previously touched this file, notify both sides so they
                    // can coordinate before the work diverges further.
                    if !previous_touches.is_empty() {
                        crate::logging::info(&format!(
                            "[file-activity] {} touched by peers before modification — sending alerts",
                            path.display()
                        ));
                        let members = swarm_members.read().await;
                        let current_member = members.get(&session_id);
                        let current_name = current_member.and_then(|m| m.friendly_name.clone());

                        // Alert the current agent about previous peer touches (one per agent).
                        if let Some(member) = current_member {
                            for prev in &previous_touches {
                                let prev_member = members.get(&prev.session_id);
                                let prev_name = prev_member.and_then(|m| m.friendly_name.clone());
                                let scope = file_activity_scope_label(prev, &touch);
                                let alert_msg = format!(
                                    "⚠ File activity: {} — {} — {} previously {} this file{}",
                                    path.display(),
                                    scope,
                                    prev_name.as_deref().unwrap_or(&prev.session_id[..8]),
                                    prev.op.as_str(),
                                    prev.summary
                                        .as_ref()
                                        .map(|s| format!(": {}", s))
                                        .unwrap_or_default()
                                );
                                let notification = ServerEvent::Notification {
                                    from_session: prev.session_id.clone(),
                                    from_name: prev_name,
                                    notification_type: NotificationType::FileConflict {
                                        path: path.display().to_string(),
                                        operation: prev.op.as_str().to_string(),
                                        summary: prev.summary.clone(),
                                        detail: prev.detail.clone(),
                                    },
                                    message: alert_msg.clone(),
                                };
                                let _ = member.event_tx.send(notification);

                                if !queue_soft_interrupt_for_session(
                                    &session_id,
                                    alert_msg.clone(),
                                    false,
                                    SoftInterruptSource::System,
                                    &soft_interrupt_queues,
                                    &sessions,
                                )
                                .await
                                {
                                    crate::logging::warn(&format!(
                                        "Failed to queue file-activity soft interrupt for session {}",
                                        session_id
                                    ));
                                }
                            }
                        }

                        // Alert previous agents about the current modification.
                        for prev in &previous_touches {
                            if let Some(prev_member) = members.get(&prev.session_id) {
                                let scope = file_activity_scope_label(prev, &touch);
                                let alert_msg = format!(
                                    "⚠ File activity: {} — {} — {} just {} this file you previously worked with{}",
                                    path.display(),
                                    scope,
                                    current_name
                                        .as_deref()
                                        .unwrap_or(&session_id[..8.min(session_id.len())]),
                                    touch.op.as_str(),
                                    touch
                                        .summary
                                        .as_ref()
                                        .map(|s| format!(": {}", s))
                                        .unwrap_or_default()
                                );
                                let notification = ServerEvent::Notification {
                                    from_session: session_id.clone(),
                                    from_name: current_name.clone(),
                                    notification_type: NotificationType::FileConflict {
                                        path: path.display().to_string(),
                                        operation: touch.op.as_str().to_string(),
                                        summary: touch.summary.clone(),
                                        detail: touch.detail.clone(),
                                    },
                                    message: alert_msg.clone(),
                                };
                                let _ = prev_member.event_tx.send(notification);

                                if !queue_soft_interrupt_for_session(
                                    &prev.session_id,
                                    alert_msg.clone(),
                                    false,
                                    SoftInterruptSource::System,
                                    &soft_interrupt_queues,
                                    &sessions,
                                )
                                .await
                                {
                                    crate::logging::warn(&format!(
                                        "Failed to queue file-activity soft interrupt for session {}",
                                        prev.session_id
                                    ));
                                }
                            }
                        }
                    }
                }
                Ok(BusEvent::BackgroundTaskCompleted(task)) => {
                    dispatch_background_task_completion(
                        &task,
                        &sessions,
                        &soft_interrupt_queues,
                        &swarm_members,
                    )
                    .await;
                }
                Ok(BusEvent::BackgroundTaskProgress(task)) => {
                    dispatch_background_task_progress(&task, &swarm_members).await;
                }
                // Session todos are private. Swarm plans are updated via explicit
                // communication actions (comm_propose_plan / comm_approve_plan), not
                // todowrite broadcasts.
                Ok(BusEvent::TodoUpdated(_)) => {}
                Ok(_) => {
                    // Ignore other events
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    crate::logging::info(&format!("Bus monitor lagged by {} events", n));
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    }

    /// Start the server (both main and debug sockets)
    pub async fn run(&self) -> Result<()> {
        // Ensure socket directory exists (for named sockets like /run/user/1000/jcode/)
        if let Some(parent) = self.socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        #[cfg(unix)]
        let _daemon_lock = acquire_daemon_lock()?;

        if socket_has_live_listener(&self.socket_path).await {
            anyhow::bail!(
                "Refusing to replace active server socket at {}",
                self.socket_path.display()
            );
        }

        // Remove existing sockets (uses transport abstraction for cross-platform cleanup)
        crate::transport::remove_socket(&self.socket_path);
        crate::transport::remove_socket(&self.debug_socket_path);

        let main_listener = Listener::bind(&self.socket_path)?;
        let debug_listener = Listener::bind(&self.debug_socket_path)?;

        #[cfg(unix)]
        {
            // Server reload uses exec. Force the published listener fds to close
            // across exec so the replacement daemon can safely rebind them.
            mark_close_on_exec(&main_listener);
            mark_close_on_exec(&debug_listener);
        }

        // Preserve an in-flight reload marker for exec-based reloads owned by this
        // process, but clear stale markers from unrelated/stale processes.
        clear_reload_marker_if_stale_for_pid(std::process::id());

        // Restrict socket files to owner-only so other local users cannot connect.
        let _ = crate::platform::set_permissions_owner_only(&self.socket_path);
        let _ = crate::platform::set_permissions_owner_only(&self.debug_socket_path);

        // Set logging context for this server
        crate::logging::set_server(&self.identity.name);

        // Log server identity
        crate::logging::info(&format!(
            "Server {} starting ({})",
            self.identity.display_name(),
            self.identity.version
        ));
        crate::logging::info(&format!("Server listening on {:?}", self.socket_path));
        crate::logging::info(&format!("Debug socket on {:?}", self.debug_socket_path));

        // Initialize distributed cluster service (if enabled)
        if let Err(e) = crate::distributed::init_cluster_service(None).await {
            crate::logging::warn(&format!("Cluster service initialization failed: {}", e));
        } else {
            crate::logging::info("Cluster service initialized");
        }

        let temporary_server_policy = lifecycle::temporary_server_policy_from_env();
        if let Some(policy) = temporary_server_policy.as_ref() {
            crate::logging::info(&format!(
                "Temporary server lifecycle enabled: owner_pid={:?}, idle_timeout_secs={}",
                policy.owner_pid, policy.idle_timeout_secs
            ));
            let _ = lifecycle::write_temporary_metadata(
                &self.socket_path,
                &self.debug_socket_path,
                policy,
            );
        }

        let server_start_time = Instant::now();

        self.spawn_background_tasks(server_start_time, temporary_server_policy);
        let (main_handle, debug_handle) = self
            .finish_startup_after_bind(main_listener, debug_listener, server_start_time)
            .await;

        // Wait for both to complete (they won't normally)
        let _ = tokio::join!(main_handle, debug_handle);
        Ok(())
    }

    /// Spawn the WebSocket gateway if enabled in config.
    /// Returns a task handle that accepts gateway clients and feeds them
    /// into handle_client just like Unix socket connections.
    fn spawn_gateway(&self, runtime: ServerRuntime) -> Option<tokio::task::JoinHandle<()>> {
        let config = if let Some(override_config) = &self.gateway_config_override {
            override_config.clone()
        } else {
            let gw_config = &crate::config::config().gateway;
            crate::gateway::GatewayConfig {
                port: gw_config.port,
                bind_addr: gw_config.bind_addr.clone(),
                enabled: gw_config.enabled,
            }
        };

        if !config.enabled {
            return None;
        }

        let (client_tx, client_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::gateway::GatewayClient>();

        // Spawn the TCP/WebSocket listener
        tokio::spawn(async move {
            if let Err(e) = crate::gateway::run_gateway(config, client_tx).await {
                crate::logging::error(&format!("Gateway error: {}", e));
            }
        });

        Some(runtime.spawn_gateway_accept_loop(client_rx))
    }
}

pub use self::client_api::Client;

#[cfg(test)]
mod tests;
