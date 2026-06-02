use super::*;
pub use jcode_selfdev_types::ReloadRecoveryDirective;

impl ReloadContext {
    fn sanitize_session_id(session_id: &str) -> String {
        session_id
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                    ch
                } else {
                    '_'
                }
            })
            .collect()
    }

    pub fn path_for_session(session_id: &str) -> Result<std::path::PathBuf> {
        let sanitized = Self::sanitize_session_id(session_id);
        Ok(storage::jcode_dir()?.join(format!("reload-context-{}.json", sanitized)))
    }

    fn legacy_path() -> Result<std::path::PathBuf> {
        Ok(storage::jcode_dir()?.join("reload-context.json"))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path_for_session(&self.session_id)?;
        storage::write_json(&path, self)?;
        Ok(())
    }

    pub fn load() -> Result<Option<Self>> {
        let legacy = Self::legacy_path()?;
        if !legacy.exists() {
            return Ok(None);
        }
        let ctx: Self = storage::read_json(&legacy)?;
        let _ = std::fs::remove_file(&legacy);
        Ok(Some(ctx))
    }

    /// Peek at context for a specific session without consuming it.
    pub fn peek_for_session(session_id: &str) -> Result<Option<Self>> {
        let session_path = Self::path_for_session(session_id)?;
        if session_path.exists() {
            let ctx: Self = storage::read_json(&session_path)?;
            return Ok(Some(ctx));
        }

        let legacy = Self::legacy_path()?;
        if !legacy.exists() {
            return Ok(None);
        }

        let ctx: Self = storage::read_json(&legacy)?;
        if ctx.session_id == session_id {
            Ok(Some(ctx))
        } else {
            Ok(None)
        }
    }

    /// Load context only if it belongs to the given session; consumes on success.
    pub fn load_for_session(session_id: &str) -> Result<Option<Self>> {
        let session_path = Self::path_for_session(session_id)?;
        if session_path.exists() {
            let ctx: Self = storage::read_json(&session_path)?;
            let _ = std::fs::remove_file(&session_path);
            return Ok(Some(ctx));
        }

        let legacy = Self::legacy_path()?;
        if !legacy.exists() {
            return Ok(None);
        }

        let ctx: Self = storage::read_json(&legacy)?;
        if ctx.session_id == session_id {
            let _ = std::fs::remove_file(&legacy);
            Ok(Some(ctx))
        } else {
            Ok(None)
        }
    }

    fn task_info_suffix(&self) -> String {
        self.task_context
            .as_ref()
            .map(|task| format!("\nTask context: {}", task))
            .unwrap_or_default()
    }

    pub fn reconnect_notice_line(&self) -> String {
        format!("Reloaded with build {}", self.version_after)
    }

    pub fn continuation_message(
        &self,
        background_task_note: &str,
        restored_turns: Option<usize>,
    ) -> String {
        let task_info = self.task_info_suffix();
        let turns_note = restored_turns
            .map(|turns| format!(" Session restored with {} turns.", turns))
            .unwrap_or_default();
        format!(
            "Reload succeeded ({} → {}).{}{}{} Continue immediately from where you left off. Do not ask the user what to do next. Do not summarize the reload.",
            self.version_before, self.version_after, task_info, background_task_note, turns_note
        )
    }

    pub fn interrupted_session_continuation_message() -> String {
        "Your session was interrupted by a server reload while a tool was running. The tool was aborted and results may be incomplete. Continue exactly where you left off and do not ask the user what to do next.".to_string()
    }

    pub fn recovery_continuation_message(
        reload_ctx: Option<&Self>,
        background_task_note: &str,
        restored_turns: Option<usize>,
    ) -> String {
        reload_ctx
            .map(|ctx| ctx.continuation_message(background_task_note, restored_turns))
            .unwrap_or_else(Self::interrupted_session_continuation_message)
    }

    pub fn recovery_directive(
        reload_ctx: Option<&Self>,
        was_interrupted: bool,
        background_task_note: &str,
        restored_turns: Option<usize>,
    ) -> Option<ReloadRecoveryDirective> {
        if let Some(ctx) = reload_ctx {
            return Some(ReloadRecoveryDirective {
                reconnect_notice: Some(ctx.reconnect_notice_line()),
                continuation_message: ctx
                    .continuation_message(background_task_note, restored_turns),
            });
        }

        if was_interrupted {
            return Some(ReloadRecoveryDirective {
                reconnect_notice: None,
                continuation_message: Self::interrupted_session_continuation_message(),
            });
        }

        None
    }

    pub fn recovery_directive_for_session(
        session_id: &str,
        reload_ctx: Option<&Self>,
        was_interrupted: bool,
        restored_turns: Option<usize>,
    ) -> Option<ReloadRecoveryDirective> {
        Self::recovery_directive(
            reload_ctx,
            was_interrupted,
            &persisted_background_tasks_note(session_id),
            restored_turns,
        )
    }

    pub fn log_recovery_outcome(flow: &str, session_id: &str, outcome: &str, detail: &str) {
        crate::logging::info(&format!(
            "reload recovery flow={} session_id={} outcome={} detail={}",
            flow, session_id, outcome, detail
        ));
    }
}

pub fn persisted_background_tasks_note(session_id: &str) -> String {
    let mut notes = String::new();

    let tasks =
        crate::background::global().persisted_detached_running_tasks_for_session(session_id);
    if !tasks.is_empty() {
        let task_list = tasks
            .iter()
            .map(|task| format!("{} ({})", task.task_id, task.tool_name))
            .collect::<Vec<_>>()
            .join(", ");

        notes.push_str(&format!(
            "\nPersisted background task(s) for this session are still running: {}. Do not rerun those commands. Check them first with the `bg` tool (`bg action=\"list\"`, `bg action=\"status\" task_id=...`, or `bg action=\"output\" task_id=...`).",
            task_list
        ));
    }

    let pending_awaits = crate::server::pending_await_members_for_session(session_id);
    if !pending_awaits.is_empty() {
        let await_list = pending_awaits
            .iter()
            .map(|state| {
                let watch = if state.requested_ids.is_empty() {
                    "entire swarm".to_string()
                } else {
                    state.requested_ids.join(", ")
                };
                let remaining_secs = state.remaining_timeout().as_secs();
                format!(
                    "{} -> [{}], {}s remaining",
                    watch,
                    state.target_status.join(", "),
                    remaining_secs
                )
            })
            .collect::<Vec<_>>()
            .join("; ");

        notes.push_str(&format!(
            "\nPersisted `communicate await_members` wait(s) are still pending: {}. If you still need those coordination points after reload, rerun the same `communicate` call with action `await_members` to resume them with the remaining timeout instead of starting over.",
            await_list
        ));
    }

    notes
}

pub(super) fn resolve_selfdev_reload_repo_dir(
    working_dir: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    resolve_selfdev_reload_repo_dir_from(build::get_repo_dir(), working_dir)
}

pub(super) fn resolve_selfdev_reload_repo_dir_from(
    primary: Option<std::path::PathBuf>,
    working_dir: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    primary.or_else(|| working_dir.and_then(build::find_repo_in_ancestors))
}

impl SelfDevTool {
    pub(super) async fn do_reload(
        &self,
        context: Option<String>,
        session_id: &str,
        execution_mode: ToolExecutionMode,
        working_dir: Option<&std::path::Path>,
    ) -> Result<ToolOutput> {
        let repo_dir = resolve_selfdev_reload_repo_dir(working_dir)
            .ok_or_else(|| anyhow::anyhow!("Could not find jcode repository directory"))?;

        let target_binary = build::find_dev_binary(&repo_dir)
            .unwrap_or_else(|| build::release_binary_path(&repo_dir));
        if !target_binary.exists() {
            return Ok(ToolOutput::new(
                format!(
                    "No binary found at {}.\n\
                     Run 'jcode self-dev --build' first, or build with 'scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode' and then try reload again.",
                    target_binary.display()
                )
                .to_string(),
            ));
        }

        let source = if SelfDevTool::is_test_session() {
            build::SourceState {
                repo_scope: "test-repo-scope".to_string(),
                worktree_scope: "test-worktree-scope".to_string(),
                short_hash: "test-reload-hash".to_string(),
                full_hash: "test-reload-hash-full".to_string(),
                dirty: true,
                fingerprint: "test-reload-fingerprint".to_string(),
                version_label: "test-reload-hash".to_string(),
                changed_paths: 0,
            }
        } else {
            build::current_source_state(&repo_dir)?
        };
        let hash = source.version_label.clone();
        let version_before = jcode_build_meta::VERSION.to_string();
        let published = if SelfDevTool::is_test_session() {
            None
        } else {
            Some(build::publish_local_current_build_for_source(
                &repo_dir, &source,
            )?)
        };
        let previous_shared_server_version = if SelfDevTool::is_test_session() {
            None
        } else {
            let published_build = published.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "published build metadata was missing after publish_local_current_build_for_source"
                )
            })?;
            build::smoke_test_server_binary(&published_build.versioned_path)?;
            build::read_shared_server_version()?
        };

        // Update manifest - track what we're testing
        let mut manifest = build::BuildManifest::load()?;
        manifest.canary = Some(hash.clone());
        manifest.canary_status = Some(build::CanaryStatus::Testing);
        manifest.set_pending_activation(build::PendingActivation {
            session_id: session_id.to_string(),
            new_version: hash.clone(),
            previous_current_version: published
                .as_ref()
                .and_then(|published| published.previous_current_version.clone()),
            previous_shared_server_version,
            source_fingerprint: Some(source.fingerprint.clone()),
            requested_at: chrono::Utc::now(),
        })?;
        manifest.save()?;

        if !SelfDevTool::is_test_session()
            && let Err(error) = build::update_shared_server_symlink(&hash)
        {
            let _ = build::rollback_pending_activation_for_session(session_id);
            return Err(error);
        }

        // Save reload context for continuation after restart
        let reload_ctx = ReloadContext {
            task_context: context,
            version_before,
            version_after: hash.clone(),
            session_id: session_id.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        crate::logging::info(&format!(
            "Saving reload context to {:?}",
            ReloadContext::path_for_session(session_id)
        ));
        if let Err(e) = reload_ctx.save() {
            crate::logging::error(&format!("Failed to save reload context: {}", e));
            let _ = build::rollback_pending_activation_for_session(session_id);
            return Err(e);
        }
        crate::logging::info("Reload context saved successfully");

        // Signal the server via in-process channel (replaces filesystem-based rebuild-signal)
        let request_id =
            server::send_reload_signal(hash.clone(), Some(session_id.to_string()), true);
        crate::logging::info(&format!(
            "selfdev reload: request={} session_id={} hash={} execution_mode={:?}",
            request_id, session_id, hash, execution_mode
        ));

        let timeout = std::time::Duration::from_secs(SelfDevTool::reload_timeout_secs());
        let ack_wait_started = std::time::Instant::now();
        let ack = server::wait_for_reload_ack(&request_id, timeout)
            .await
            .map_err(|error| {
                let _ = build::rollback_pending_activation_for_session(session_id);
                anyhow::anyhow!(
                    "Timed out waiting for the server to begin reload after {}s: {}. The reload signal may not have been picked up; check that the connected server is running a build with unified self-dev reload support and try restarting the shared server.",
                    timeout.as_secs(),
                    error
                )
            })?;

        crate::logging::info(&format!(
            "selfdev reload: acked request={} hash={} after {}ms state={}",
            ack.request_id,
            ack.hash,
            ack_wait_started.elapsed().as_millis(),
            server::reload_state_summary(std::time::Duration::from_secs(60))
        ));

        match execution_mode {
            ToolExecutionMode::Direct => {
                if SelfDevTool::is_test_session() {
                    return Ok(ToolOutput::new(format!(
                        "Reload acknowledged for build {}. Server is restarting now.",
                        ack.hash
                    )));
                }
                match server::await_reload_handoff(&server::socket_path(), timeout).await {
                    server::ReloadWaitStatus::Ready => {
                        let _ = build::complete_pending_activation_for_session(session_id);
                        Ok(ToolOutput::new(format!(
                            "Reload completed successfully for build {}. Server reported ready.",
                            ack.hash
                        )))
                    }
                    server::ReloadWaitStatus::Failed(detail) => {
                        let _ = build::rollback_pending_activation_for_session(session_id);
                        Err(anyhow::anyhow!(
                            "Reload was acknowledged for build {}, but the replacement server failed before becoming ready on {}: {}; recent_state={}",
                            ack.hash,
                            server::socket_path().display(),
                            detail.unwrap_or_else(|| "unknown reload failure".to_string()),
                            server::reload_state_summary(std::time::Duration::from_secs(60))
                        ))
                    }
                    server::ReloadWaitStatus::Idle | server::ReloadWaitStatus::Waiting { .. } => {
                        let _ = build::rollback_pending_activation_for_session(session_id);
                        Err(anyhow::anyhow!(
                            "Reload was acknowledged for build {}, but readiness could not be confirmed within {}s.",
                            ack.hash,
                            timeout.as_secs()
                        ))
                    }
                }
            }
            ToolExecutionMode::AgentTurn => {
                // In normal agent turns the reload will intentionally terminate this
                // process shortly after the server acknowledges the request. Return a
                // tool result immediately so the harness can persist/deliver the tool
                // output before the process exits. Previously this branch waited to be
                // interrupted by shutdown; depending on timing, the process could exit
                // before the in-flight tool result reached the client, producing
                // "Tool output missing" even though reload succeeded.
                Ok(ToolOutput::new(format!(
                    "Reload initiated for build {}. Process restarting...",
                    ack.hash
                )))
            }
        }
    }
}
