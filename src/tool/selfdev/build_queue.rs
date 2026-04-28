use super::*;

impl SelfDevTool {
    async fn append_output_line(file: &mut tokio::fs::File, line: impl AsRef<str>) {
        let _ = file.write_all(line.as_ref().as_bytes()).await;
        let _ = file.write_all(b"\n").await;
        let _ = file.flush().await;
    }

    async fn wait_for_turn(
        request_id: &str,
        worktree_scope: &str,
        file: &mut tokio::fs::File,
    ) -> Result<BuildLockGuard> {
        let mut last_note: Option<String> = None;
        loop {
            let pending = BuildRequest::pending_requests_for_scope(worktree_scope)?;
            let my_index = pending
                .iter()
                .position(|request| request.request_id == request_id)
                .ok_or_else(|| {
                    anyhow::anyhow!("Queued build request {} disappeared", request_id)
                })?;

            if my_index == 0
                && let Some(lock) = Self::try_acquire_build_lock(worktree_scope)?
            {
                return Ok(lock);
            }

            let note = if my_index == 0 {
                Some("Waiting for the self-dev build lock to become available".to_string())
            } else {
                pending.get(my_index - 1).map(|request| {
                    format!(
                        "Waiting in queue behind {} — {}",
                        request.display_owner(),
                        request.reason
                    )
                })
            };
            if note.as_ref() != last_note.as_ref() {
                if let Some(note) = note.as_ref() {
                    Self::append_output_line(file, note).await;
                }
                last_note = note;
            }

            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    async fn stream_build_command(
        repo_dir: PathBuf,
        command: SelfDevBuildCommand,
        output_path: PathBuf,
    ) -> Result<TaskResult> {
        let mut cmd = tokio::process::Command::new(&command.program);
        cmd.args(&command.args)
            .current_dir(repo_dir)
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::platform::suppress_child_console_async(&mut cmd);

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn build command: {}", e))?;

        let mut file = tokio::fs::File::create(&output_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create output file: {}", e))?;
        Self::append_output_line(
            &mut file,
            format!("Starting build with {}", command.display),
        )
        .await;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let mut stdout_lines = stdout.map(|s| BufReader::new(s).lines());
        let mut stderr_lines = stderr.map(|s| BufReader::new(s).lines());
        let mut stdout_done = stdout_lines.is_none();
        let mut stderr_done = stderr_lines.is_none();

        while !stdout_done || !stderr_done {
            tokio::select! {
                line = async {
                    match stdout_lines.as_mut() {
                        Some(r) => r.next_line().await,
                        None => std::future::pending().await,
                    }
                }, if !stdout_done => {
                    match line {
                        Ok(Some(line)) => Self::append_output_line(&mut file, line).await,
                        _ => stdout_done = true,
                    }
                }
                line = async {
                    match stderr_lines.as_mut() {
                        Some(r) => r.next_line().await,
                        None => std::future::pending().await,
                    }
                }, if !stderr_done => {
                    match line {
                        Ok(Some(line)) => Self::append_output_line(&mut file, format!("[stderr] {}", line)).await,
                        _ => stderr_done = true,
                    }
                }
            }
        }

        let status = child.wait().await?;
        let exit_code = status.code();
        Self::append_output_line(
            &mut file,
            format!(
                "--- Command finished with exit code: {} ---",
                exit_code.unwrap_or(-1)
            ),
        )
        .await;

        if status.success() {
            Ok(TaskResult::completed(exit_code))
        } else {
            Ok(TaskResult::failed(
                exit_code,
                format!("Command exited with code {}", exit_code.unwrap_or(-1)),
            ))
        }
    }

    async fn run_test_build(output_path: PathBuf, reason: &str) -> Result<TaskResult> {
        let mut file = tokio::fs::File::create(&output_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create output file: {}", e))?;
        Self::append_output_line(
            &mut file,
            format!("[test mode] Simulated selfdev build for reason: {}", reason),
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        Self::append_output_line(&mut file, "--- Command finished with exit code: 0 ---").await;
        Ok(TaskResult::completed(Some(0)))
    }

    async fn follow_existing_build(
        request_id: String,
        original_request_id: String,
        output_path: PathBuf,
    ) -> Result<TaskResult> {
        let mut file = tokio::fs::File::create(&output_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create output file: {}", e))?;
        Self::append_output_line(
            &mut file,
            format!(
                "Attached to existing selfdev build request {} instead of spawning a duplicate build.",
                original_request_id
            ),
        )
        .await;

        loop {
            let Some(original) = BuildRequest::load(&original_request_id)? else {
                anyhow::bail!("Original build request {} disappeared", original_request_id);
            };
            match original.state {
                BuildRequestState::Queued | BuildRequestState::Building => {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                BuildRequestState::Completed => {
                    let mut request = BuildRequest::load(&request_id)?.ok_or_else(|| {
                        anyhow::anyhow!("Attached build request {} disappeared", request_id)
                    })?;
                    request.state = BuildRequestState::Completed;
                    request.completed_at = Some(Utc::now().to_rfc3339());
                    request.error = None;
                    request.save()?;
                    Self::append_output_line(
                        &mut file,
                        format!(
                            "Original build {} completed successfully.",
                            original_request_id
                        ),
                    )
                    .await;
                    return Ok(TaskResult::completed(Some(0)));
                }
                BuildRequestState::Superseded => {
                    let mut request = BuildRequest::load(&request_id)?.ok_or_else(|| {
                        anyhow::anyhow!("Attached build request {} disappeared", request_id)
                    })?;
                    request.state = BuildRequestState::Superseded;
                    request.completed_at = Some(Utc::now().to_rfc3339());
                    request.error = original.error.clone();
                    request.save()?;
                    let detail = original.error.clone().unwrap_or_else(|| {
                        format!(
                            "Original build {} completed but was superseded before activation",
                            original_request_id
                        )
                    });
                    Self::append_output_line(&mut file, &detail).await;
                    return Ok(TaskResult::superseded(Some(0), detail));
                }
                BuildRequestState::Failed | BuildRequestState::Cancelled => {
                    let mut request = BuildRequest::load(&request_id)?.ok_or_else(|| {
                        anyhow::anyhow!("Attached build request {} disappeared", request_id)
                    })?;
                    request.state = original.state.clone();
                    request.completed_at = Some(Utc::now().to_rfc3339());
                    request.error = original.error.clone();
                    request.save()?;
                    let error = original.error.clone().unwrap_or_else(|| {
                        format!("Original build {} did not complete", original_request_id)
                    });
                    Self::append_output_line(&mut file, &error).await;
                    return Ok(TaskResult::failed(None, error));
                }
                BuildRequestState::Attached => {
                    anyhow::bail!(
                        "Original build request {} is attached, not build-producing",
                        original_request_id
                    );
                }
            }
        }
    }

    async fn run_build_request(
        request_id: String,
        repo_dir: PathBuf,
        command: SelfDevBuildCommand,
        reason: String,
        output_path: PathBuf,
    ) -> Result<TaskResult> {
        let mut request = BuildRequest::load(&request_id)?
            .ok_or_else(|| anyhow::anyhow!("Missing queued build request {}", request_id))?;
        let mut queue_file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&output_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to open output file: {}", e))?;

        let worktree_scope = request.worktree_scope.clone();
        let _lock = Self::wait_for_turn(&request_id, &worktree_scope, &mut queue_file).await?;
        let expected_source = request
            .requested_source
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Missing requested source state for {}", request_id))?;
        let actual_source = if Self::is_test_session() {
            expected_source.clone()
        } else {
            build::ensure_source_state_matches(&repo_dir, &expected_source)?
        };
        request.state = BuildRequestState::Building;
        request.started_at = Some(Utc::now().to_rfc3339());
        request.version = Some(expected_source.version_label.clone());
        request.built_source = Some(actual_source.clone());
        request.last_progress = Some("building".to_string());
        request.save()?;
        Self::append_output_line(&mut queue_file, format!("Build starting now: {}", reason)).await;
        drop(queue_file);

        let result = if Self::is_test_session() {
            Self::run_test_build(output_path.clone(), &reason).await?
        } else {
            let result =
                Self::stream_build_command(repo_dir.clone(), command.clone(), output_path.clone())
                    .await?;
            if result.error.is_none() {
                match build::ensure_source_state_matches(&repo_dir, &expected_source) {
                    Ok(source_after_build) => {
                        build::write_current_dev_binary_source_metadata(
                            &repo_dir,
                            &source_after_build,
                        )?;
                        let published = build::publish_local_current_build_for_source(
                            &repo_dir,
                            &source_after_build,
                        )?;
                        let mut manifest = build::BuildManifest::load()?;
                        manifest.add_to_history(build::current_build_info(&repo_dir)?)?;
                        let mut request = BuildRequest::load(&request_id)?.ok_or_else(|| {
                            anyhow::anyhow!("Missing queued build request {}", request_id)
                        })?;
                        request.published_version = Some(published.version.clone());
                        request.validated = true;
                        request.last_progress = Some("published and smoke-tested".to_string());
                        request.save()?;
                        result
                    }
                    Err(err) => {
                        let detail = format!(
                            "Build completed successfully, but the source changed before activation. Marking this result as superseded instead of failed. {}",
                            err
                        );
                        let mut file = tokio::fs::OpenOptions::new()
                            .append(true)
                            .open(&output_path)
                            .await
                            .map_err(|e| anyhow::anyhow!("Failed to append output note: {}", e))?;
                        Self::append_output_line(&mut file, &detail).await;
                        TaskResult::superseded(result.exit_code.or(Some(0)), detail)
                    }
                }
            } else {
                result
            }
        };

        let mut request = BuildRequest::load(&request_id)?
            .ok_or_else(|| anyhow::anyhow!("Missing queued build request {}", request_id))?;
        request.completed_at = Some(Utc::now().to_rfc3339());
        request.state = match result
            .status
            .as_ref()
            .unwrap_or(&BackgroundTaskStatus::Failed)
        {
            BackgroundTaskStatus::Completed => BuildRequestState::Completed,
            BackgroundTaskStatus::Superseded => BuildRequestState::Superseded,
            BackgroundTaskStatus::Failed => BuildRequestState::Failed,
            BackgroundTaskStatus::Running => BuildRequestState::Building,
        };
        request.error = result.error.clone();
        request.last_progress = match request.state {
            BuildRequestState::Completed => request
                .last_progress
                .take()
                .or_else(|| Some("completed".to_string())),
            BuildRequestState::Superseded => Some("superseded by newer source state".to_string()),
            BuildRequestState::Failed => Some("failed".to_string()),
            BuildRequestState::Building => Some("building".to_string()),
            BuildRequestState::Queued => Some("queued".to_string()),
            BuildRequestState::Attached => Some("attached".to_string()),
            BuildRequestState::Cancelled => Some("cancelled".to_string()),
        };
        request.save()?;
        Ok(result)
    }

    pub(super) async fn do_build(
        &self,
        reason: Option<String>,
        notify: Option<bool>,
        wake: Option<bool>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let reason = reason
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "`selfdev build` requires a non-empty `reason` so other queued agents can see why the build is needed."
                )
            })?;
        let repo_dir =
            SelfDevTool::resolve_repo_dir(ctx.working_dir.as_deref()).ok_or_else(|| {
                anyhow::anyhow!("Could not find the jcode repository directory for selfdev build")
            })?;

        let requested_source = SelfDevTool::requested_source_state(&repo_dir)?;
        let command = SelfDevTool::build_command(&repo_dir);
        let dedupe_key = SelfDevTool::build_dedupe_key(&requested_source, &command);
        let blocker = SelfDevTool::newest_active_request(&requested_source.worktree_scope)?;
        let duplicate =
            BuildRequest::find_duplicate_pending(&requested_source.worktree_scope, &dedupe_key)?;
        let (session_short_name, session_title) = SelfDevTool::load_session_labels(&ctx.session_id);
        let request_id = SelfDevTool::next_request_id();
        let wake = wake.unwrap_or(true);
        let notify = notify.unwrap_or(true) || wake;

        if let Some(existing) = duplicate {
            let mut request = BuildRequest {
                request_id: request_id.clone(),
                background_task_id: None,
                session_id: ctx.session_id.clone(),
                session_short_name,
                session_title,
                reason: reason.clone(),
                repo_dir: repo_dir.display().to_string(),
                repo_scope: requested_source.repo_scope.clone(),
                worktree_scope: requested_source.worktree_scope.clone(),
                command: command.display.clone(),
                requested_at: Utc::now().to_rfc3339(),
                started_at: None,
                completed_at: None,
                state: BuildRequestState::Attached,
                version: Some(requested_source.version_label.clone()),
                dedupe_key: Some(dedupe_key.clone()),
                requested_source: Some(requested_source.clone()),
                built_source: None,
                published_version: None,
                last_progress: Some("attached to existing build".to_string()),
                validated: false,
                error: None,
                output_file: None,
                status_file: None,
                attached_to_request_id: Some(existing.request_id.clone()),
            };
            request.save()?;

            let request_id_for_task = request_id.clone();
            let existing_request_id = existing.request_id.clone();
            let info = background::global()
                .spawn_with_notify(
                    "selfdev-build-watch",
                    Some("build watch".to_string()),
                    &ctx.session_id,
                    notify,
                    wake,
                    move |output_path| async move {
                        SelfDevTool::follow_existing_build(
                            request_id_for_task,
                            existing_request_id,
                            output_path,
                        )
                        .await
                    },
                )
                .await;

            request.background_task_id = Some(info.task_id.clone());
            request.output_file = Some(info.output_file.display().to_string());
            request.status_file = Some(info.status_file.display().to_string());
            request.save()?;

            let delivery = if wake {
                "The requesting agent will be woken when the existing build finishes."
            } else if notify {
                "You will be notified when the existing build finishes."
            } else {
                "Completion delivery is disabled for this watcher."
            };
            let output = format!(
                "Matching self-dev build already queued/running, so this request was attached instead of spawning a duplicate build.\n\n- Your request ID: `{}`\n- Watcher task ID: `{}`\n- Existing request: `{}`\n- Requested by: {}\n- Reason: {}\n- Target version: `{}`\n- Source fingerprint: `{}`\n\n{}",
                request_id,
                info.task_id,
                existing.request_id,
                existing.display_owner(),
                existing.reason,
                requested_source.version_label,
                requested_source.fingerprint,
                delivery
            );

            return Ok(ToolOutput::new(output).with_metadata(json!({
                "background": true,
                "deduped": true,
                "request_id": request_id,
                "task_id": info.task_id,
                "output_file": info.output_file.to_string_lossy(),
                "status_file": info.status_file.to_string_lossy(),
                "duplicate_of": {
                    "request_id": existing.request_id,
                    "task_id": existing.background_task_id,
                    "session_id": existing.session_id,
                    "session_short_name": existing.session_short_name,
                    "session_title": existing.session_title,
                    "reason": existing.reason,
                    "version": existing.version,
                    "source_fingerprint": existing
                        .requested_source
                        .as_ref()
                        .map(|source| source.fingerprint.clone()),
                }
            })));
        }

        let mut request = BuildRequest {
            request_id: request_id.clone(),
            background_task_id: None,
            session_id: ctx.session_id.clone(),
            session_short_name,
            session_title,
            reason: reason.clone(),
            repo_dir: repo_dir.display().to_string(),
            repo_scope: requested_source.repo_scope.clone(),
            worktree_scope: requested_source.worktree_scope.clone(),
            command: command.display.clone(),
            requested_at: Utc::now().to_rfc3339(),
            started_at: None,
            completed_at: None,
            state: BuildRequestState::Queued,
            version: Some(requested_source.version_label.clone()),
            dedupe_key: Some(dedupe_key),
            requested_source: Some(requested_source.clone()),
            built_source: None,
            published_version: None,
            last_progress: Some("queued".to_string()),
            validated: false,
            error: None,
            output_file: None,
            status_file: None,
            attached_to_request_id: None,
        };
        request.save()?;

        let queue_position =
            SelfDevTool::current_queue_position(&request_id, &requested_source.worktree_scope)?
                .unwrap_or(1);

        let request_id_for_task = request_id.clone();
        let repo_dir_for_task = repo_dir.clone();
        let command_for_task = command.clone();
        let reason_for_task = reason.clone();
        let info = background::global()
            .spawn_with_notify(
                "selfdev-build",
                Some("selfdev build".to_string()),
                &ctx.session_id,
                notify,
                wake,
                move |output_path| async move {
                    SelfDevTool::run_build_request(
                        request_id_for_task,
                        repo_dir_for_task,
                        command_for_task,
                        reason_for_task,
                        output_path,
                    )
                    .await
                },
            )
            .await;

        request.background_task_id = Some(info.task_id.clone());
        request.output_file = Some(info.output_file.display().to_string());
        request.status_file = Some(info.status_file.display().to_string());
        request.save()?;
        let delivery = if wake {
            "The requesting agent will be woken when the build completes."
        } else if notify {
            "You will be notified when the build completes."
        } else {
            "Completion delivery is disabled for this build request."
        };
        let mut output = format!(
            "Self-dev build queued in background.\n\n- Request ID: `{}`\n- Task ID: `{}`\n- Reason: {}\n- Target version: `{}`\n- Source fingerprint: `{}`\n- Command: `{}`\n- Queue position: {}\n- Output file: `{}`\n- Status file: `{}`\n\n{}",
            request_id,
            info.task_id,
            reason,
            requested_source.version_label,
            requested_source.fingerprint,
            command.display,
            queue_position,
            info.output_file.display(),
            info.status_file.display(),
            delivery
        );

        if let Some(ref blocker) = blocker {
            output.push_str(&format!(
                "\n\nCurrently blocked by: {}\nReason: {}",
                blocker.display_owner(),
                blocker.reason
            ));
        }

        output.push_str(&format!(
            "\n\nUse `bg action=\"wait\" task_id=\"{}\"` to wait for completion/checkpoints, `bg action=\"status\" task_id=\"{}\"` to check progress immediately, or `selfdev status` to inspect the build queue.\nAfter it finishes, use `selfdev reload` when you want to restart onto the new binary.",
            info.task_id,
            info.task_id
        ));

        Ok(ToolOutput::new(output).with_metadata(json!({
            "background": true,
            "request_id": request_id,
            "task_id": info.task_id,
            "output_file": info.output_file.to_string_lossy(),
            "status_file": info.status_file.to_string_lossy(),
            "queue_position": queue_position,
            "version": requested_source.version_label,
            "source_fingerprint": requested_source.fingerprint,
            "blocked_by": blocker.as_ref().map(|request| json!({
                "session_id": request.session_id,
                "session_short_name": request.session_short_name,
                "session_title": request.session_title,
                "reason": request.reason,
                "version": request.version,
                "source_fingerprint": request
                    .requested_source
                    .as_ref()
                    .map(|source| source.fingerprint.clone()),
            }))
        })))
    }

    pub(super) async fn do_cancel_build(
        &self,
        request_id: Option<String>,
        task_id: Option<String>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let Some(mut request) =
            BuildRequest::find_by_request_or_task(request_id.as_deref(), task_id.as_deref())?
        else {
            return Ok(ToolOutput::new(
                "No self-dev build request matched the provided request_id/task_id.",
            ));
        };

        if request.session_id != ctx.session_id {
            return Ok(ToolOutput::new(format!(
                "That self-dev build request belongs to {}, not this session ({}).",
                request.display_owner(),
                ctx.session_id
            )));
        }

        if matches!(
            request.state,
            BuildRequestState::Completed | BuildRequestState::Failed | BuildRequestState::Cancelled
        ) {
            return Ok(ToolOutput::new(format!(
                "Build request `{}` is already in terminal state `{}`.",
                request.request_id,
                match request.state {
                    BuildRequestState::Completed => "completed",
                    BuildRequestState::Failed => "failed",
                    BuildRequestState::Cancelled => "cancelled",
                    _ => unreachable!(),
                }
            )));
        }

        let cancelled_task = if let Some(task_id) = request.background_task_id.as_deref() {
            background::global().cancel(task_id).await?
        } else {
            false
        };

        request.state = BuildRequestState::Cancelled;
        request.completed_at = Some(Utc::now().to_rfc3339());
        request.error = Some("Cancelled by user".to_string());
        request.save()?;

        Ok(ToolOutput::new(format!(
            "Cancelled self-dev build request `{}`.\n\n- Task cancelled: {}\n- Reason: {}\n- Target version: {}",
            request.request_id,
            if cancelled_task { "yes" } else { "no (task may have already finished)" },
            request.reason,
            request.version.as_deref().unwrap_or("unknown")
        ))
        .with_metadata(json!({
            "request_id": request.request_id,
            "task_id": request.background_task_id,
            "cancelled": true,
            "cancelled_task": cancelled_task,
        })))
    }
}
