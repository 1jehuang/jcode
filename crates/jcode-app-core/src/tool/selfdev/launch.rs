use super::*;

pub fn enter_selfdev_session(
    parent_session_id: Option<&str>,
    working_dir: Option<&Path>,
) -> Result<SelfDevLaunchResult> {
    let repo_dir = SelfDevTool::resolve_repo_dir(working_dir).ok_or_else(|| {
        anyhow::anyhow!("Could not find the jcode repository to enter self-dev mode")
    })?;

    let mut inherited_context = false;
    let mut session = if let Some(parent_session_id) = parent_session_id {
        match session::Session::load(parent_session_id) {
            Ok(parent) => {
                let mut child = session::Session::create(
                    Some(parent_session_id.to_string()),
                    Some("Self-development session".to_string()),
                );
                child.replace_messages(parent.messages.clone());
                child.compaction = parent.compaction.clone();
                child.model = parent.model.clone();
                child.provider_key = parent.provider_key.clone();
                child.route_api_method = parent.route_api_method.clone();
                child.subagent_model = parent.subagent_model.clone();
                child.improve_mode = parent.improve_mode;
                child.autoreview_enabled = parent.autoreview_enabled;
                child.autojudge_enabled = parent.autojudge_enabled;
                child.memory_injections = parent.memory_injections.clone();
                child.replay_events = parent.replay_events.clone();
                inherited_context = true;
                child
            }
            Err(err) => {
                crate::logging::warn(&format!(
                    "Failed to load parent session {} for self-dev enter; starting fresh session: {}",
                    parent_session_id, err
                ));
                session::Session::create(None, Some("Self-development session".to_string()))
            }
        }
    } else {
        session::Session::create(None, Some("Self-development session".to_string()))
    };
    session.set_canary("self-dev");
    session.working_dir = Some(repo_dir.display().to_string());
    session.status = session::SessionStatus::Closed;
    session.save()?;

    let session_id = session.id.clone();

    if SelfDevTool::is_test_session() {
        return Ok(SelfDevLaunchResult {
            session_id,
            repo_dir,
            launched: false,
            test_mode: true,
            exe: None,
            inherited_context,
        });
    }

    let exe = SelfDevTool::launch_binary()?;
    let launched = session_launch::spawn_selfdev_in_new_terminal(&exe, &session_id, &repo_dir)?;

    Ok(SelfDevLaunchResult {
        session_id,
        repo_dir,
        launched,
        test_mode: false,
        exe: Some(exe),
        inherited_context,
    })
}

pub fn schedule_selfdev_prompt_delivery(session_id: String, prompt: String) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        match runtime {
            Ok(runtime) => {
                if let Err(err) =
                    runtime.block_on(SelfDevTool::send_prompt_to_session(&session_id, &prompt))
                {
                    crate::logging::warn(&format!(
                        "Failed to auto-deliver prompt to spawned self-dev session {}: {}",
                        session_id, err
                    ));
                }
            }
            Err(err) => crate::logging::warn(&format!(
                "Failed to initialize runtime for self-dev prompt delivery: {}",
                err
            )),
        }
    });
}

impl SelfDevTool {
    async fn send_prompt_to_session(session_id: &str, prompt: &str) -> Result<()> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let mut last_error: Option<String> = None;

        while std::time::Instant::now() < deadline {
            match Self::try_send_prompt_once(session_id, prompt).await {
                Ok(()) => return Ok(()),
                Err(err) => {
                    last_error = Some(err.to_string());
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
        }

        Err(anyhow::anyhow!(
            "Timed out delivering prompt to spawned self-dev session {}: {}",
            session_id,
            last_error.unwrap_or_else(|| "unknown error".to_string())
        ))
    }

    async fn try_send_prompt_once(session_id: &str, prompt: &str) -> Result<()> {
        let mut client = server::Client::connect_debug().await?;
        let request_id = client
            .send_transcript(prompt, TranscriptMode::Send, Some(session_id.to_string()))
            .await?;

        loop {
            match client.read_event().await? {
                ServerEvent::Ack { id } if id == request_id => {}
                ServerEvent::Done { id } if id == request_id => return Ok(()),
                ServerEvent::Error { id, message, .. } if id == request_id => {
                    anyhow::bail!(message)
                }
                _ => {}
            }
        }
    }

    pub(super) async fn do_enter(
        &self,
        prompt: Option<String>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let launch = enter_selfdev_session(Some(&ctx.session_id), ctx.working_dir.as_deref())?;

        if launch.test_mode {
            let mut output = format!(
                "Created self-dev session {} in {}.\n\nTest mode skipped launching a new terminal.",
                launch.session_id,
                launch.repo_dir.display()
            );
            if let Some(prompt) = prompt {
                output.push_str(&format!(
                    "\n\nSeed prompt captured ({} chars) but not delivered in test mode.",
                    prompt.chars().count()
                ));
            }
            return Ok(ToolOutput::new(output).with_metadata(json!({
                "session_id": launch.session_id,
                "repo_dir": launch.repo_dir,
                "launched": false,
                "test_mode": true,
                "inherited_context": launch.inherited_context
            })));
        }

        if !launch.launched {
            let command_preview = launch
                .command_preview()
                .unwrap_or_else(|| format!("jcode --resume {} self-dev", launch.session_id));
            return Ok(ToolOutput::new(format!(
                "Created self-dev session {} but could not find a supported terminal to spawn automatically.\n\nRun manually:\n`{} --resume {} self-dev`",
                launch.session_id,
                launch.exe.as_ref().map(|exe| exe.display().to_string()).unwrap_or_else(|| "jcode".to_string()),
                launch.session_id
            ))
            .with_metadata(json!({
                "session_id": launch.session_id,
                "repo_dir": launch.repo_dir,
                "launched": false,
                "inherited_context": launch.inherited_context
            }))
            .with_title(format!("selfdev enter: {}", command_preview)));
        }

        let mut output = format!(
            "Spawned a new self-dev session in a separate terminal.\n\n- Session: `{}`\n- Repo: `{}`\n- Command: `{} --resume {} self-dev`",
            launch.session_id,
            launch.repo_dir.display(),
            launch
                .exe
                .as_ref()
                .map(|exe| exe.display().to_string())
                .unwrap_or_else(|| "jcode".to_string()),
            launch.session_id
        );

        let prompt_delivery = if let Some(prompt_text) = prompt {
            match SelfDevTool::send_prompt_to_session(&launch.session_id, &prompt_text).await {
                Ok(()) => {
                    output.push_str("\n- Prompt: delivered to the spawned self-dev session");
                    Some(true)
                }
                Err(err) => {
                    output.push_str(&format!("\n- Prompt: failed to auto-deliver ({})", err));
                    Some(false)
                }
            }
        } else {
            None
        };

        if launch.inherited_context {
            output.push_str("\n- Context: cloned from the current session");
        }

        Ok(ToolOutput::new(output).with_metadata(json!({
            "session_id": launch.session_id,
            "repo_dir": launch.repo_dir,
            "launched": true,
            "prompt_delivered": prompt_delivery,
            "inherited_context": launch.inherited_context
        })))
    }
}
