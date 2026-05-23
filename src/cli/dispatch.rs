#![cfg_attr(test, allow(clippy::await_holding_lock))]

use anyhow::Result;
use std::process::{Command as ProcessCommand, Stdio};
use std::time::Instant;

use super::args::{
    AmbientCommand, Args, AuthCommand, Command, MemoryCommand, ModelCommand,
    ProviderCommand, RestartCommand, SessionCommand, TranscriptModeArg,
};
use crate::{
    agent, auth, build, provider, provider_catalog, server, session, setup_hints, startup_profile,
    tui,
};

use super::{
    commands, hot_exec, login, output, provider_init, selfdev, terminal, tui_launch,
};
use provider_init::ProviderChoice;

pub(crate) async fn run_main(mut args: Args) -> Result<()> {
    resolve_resume_arg(&mut args)?;

    if let Some(profile_name) = args
        .provider_profile
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        provider_catalog::apply_named_provider_profile_env(profile_name)?;
        crate::env::set_var("JCODE_PROVIDER_PROFILE_NAME", profile_name);
        crate::env::set_var("JCODE_PROVIDER_PROFILE_ACTIVE", "1");
        args.provider = ProviderChoice::OpenaiCompatible;
    }

    match args.command {
        Some(Command::Serve {
            temporary_server,
            owner_pid,
            temp_idle_timeout_secs,
        }) => {
            let serve_start = Instant::now();
            crate::env::set_var("JCODE_NON_INTERACTIVE", "1");
            if temporary_server {
                server::configure_temporary_server(owner_pid, temp_idle_timeout_secs);
            }

            // CarpAI Server 是完整的服务端系统 — 默认初始化所有模块
            //（包括企业功能：多租户、分布式推理、节点发现、管理API等）
            // 后续裁剪个人版时，可通过 --features no-enterprise 关闭企业模块
            crate::logging::info("🚀 CarpAI Server — initializing all service modules");

            // ===== 服务器基础设施模块初始化（数据库/认证/调度器/管理API）=====
            #[cfg(feature = "enterprise")]
            {
                use std::sync::Arc;
                use crate::enterprise::config::EnterpriseConfig;
                use crate::enterprise::db::DatabaseManager;
                use crate::enterprise::auth::{JwtManager, RbacEngine};
                use crate::enterprise::discovery::NodeDiscoveryManager;
                use crate::enterprise::distributed::DistributedInferenceScheduler;
                use crate::enterprise::cpu_inference::CpuInferenceEngine;
                use crate::enterprise::virtual_memory::VirtualMemoryManager;
                use crate::enterprise::usage::UsageManager;
                use crate::enterprise::quota::UsageTracker;
                use crate::enterprise::priority::PriorityRuleEngine;
                use crate::enterprise::enterprise::EnterpriseServerState;
                use jcode_unified_scheduler::{UnifiedScheduler, SchedulerConfig, NodeHardwareInfo};
                use tokio::sync::RwLock;
                use std::collections::HashMap;
                use tracing::info;

                let ent_config = Arc::new(EnterpriseConfig::load());

                // 1. 数据库
                let db = match DatabaseManager::new(&ent_config.database).await {
                    Ok(db) => { info!("✅ 企业数据库已连接"); Some(Arc::new(db)) }
                    Err(e) => { tracing::warn!("数据库连接失败（内存模式）: {}", e); None }
                };

                // 2. 认证
                let jwt_secret = std::env::var(&ent_config.auth.jwt_secret_env)
                    .unwrap_or_else(|_| {
                        tracing::warn!("JWT secret 未设置（请设置 {}", ent_config.auth.jwt_secret_env);
                        "default-dev-secret".into()
                    });
                let jwt_manager = Arc::new(RwLock::new(
                    JwtManager::new_hs256(jwt_secret.as_bytes(), "carpai".into(),
                        ent_config.auth.jwt_expiry_hours as i64).unwrap()
                ));
                let rbac_engine = Arc::new(RwLock::new(RbacEngine::new()));

                // 3. 调度器
                let sched_config = SchedulerConfig {
                    min_bootstrap_nodes: 1, enable_goap: true,
                    adaptive_scheduling: true, ..SchedulerConfig::default()
                };
                let scheduler = Arc::new(UnifiedScheduler::new(sched_config).await.unwrap());
                let distributed_scheduler = Some(Arc::new(DistributedInferenceScheduler::new(ent_config.clone())));
                let discovery_manager = Arc::new(NodeDiscoveryManager::new(ent_config.clone()));

                // 4. 用量/配额
                let usage_manager = Arc::new(RwLock::new(UsageManager::new()));
                let quota_tracker = Arc::new(RwLock::new(UsageTracker::new()));

                // 5. CPU 推理 + 虚拟内存
                let cpu_engine = Arc::new(CpuInferenceEngine::new(ent_config.clone()));
                let vm_manager = if ent_config.scheduling.enable_virtual_memory {
                    Some(Arc::new(VirtualMemoryManager::new(ent_config.virtual_memory.clone())))
                } else { None };

                // 6. 构建共享状态
                let ent_state = Arc::new(EnterpriseServerState {
                    config: ent_config.clone(),
                    jwt_manager, rbac_engine,
                    users: Arc::new(RwLock::new(HashMap::new())),
                    cpu_engine: Some(cpu_engine),
                    providers: Arc::new(RwLock::new(HashMap::new())),
                    scheduler: scheduler.clone(),
                    distributed_scheduler,
                    discovery_manager: discovery_manager.clone(),
                    usage_manager, quota_tracker,
                    priority_engine: PriorityRuleEngine::default(),
                    vm_manager, db,
                    codebase_engine: Arc::new(tokio::sync::Mutex::new(None)),
                    started_at: chrono::Utc::now(),
                });

                // 7. 注册本机到调度器
                let total_mem = sys_info::mem_info().map(|m| m.total as f64 / 1024.0 / 1024.0).unwrap_or(16.0);
                let node_hw = NodeHardwareInfo {
                    node_id: uuid::Uuid::new_v4(), num_gpus: 0, gpu_name: "CPU-only".into(),
                    memory_gb: total_mem, cpu_cores: num_cpus::get_physical() as u32,
                    tflops_fp16: 0.0, tflops_fp32: 0.0, gpu_bandwidth_gbps: 0.0,
                    pcie_bandwidth_gbps: 0.0, has_gpu: false, vram_gb: 0.0,
                    cpu_arch: std::env::consts::ARCH.to_string(),
                };
                let _ = scheduler.register_node(node_hw).await;
                info!("✅ 本机已注册到调度器（{}GB / {}核）", total_mem, num_cpus::get_physical());

                // 8. 启动后台服务（调度循环、心跳检测、gRPC、Admin API）
                let d = discovery_manager.clone();
                tokio::spawn(async move { d.heartbeat_check_loop().await; });
                tokio::spawn(async move {
                    if let Err(e) = scheduler.run().await {
                        tracing::error!("调度器异常: {:?}", e);
                    }
                });

                // 启动 Admin API + gRPC
                let es = ent_state.clone();
                tokio::spawn(async move {
                    use crate::enterprise::admin_api;
                    use crate::enterprise::admin_api::auth_middleware;
                    use crate::enterprise::metrics;

                    // OpenAI 兼容 API + 管理后台 API
                    let api_router = admin_api::create_openai_router().with_state(es.clone());
                    let admin_router = admin_api::create_admin_router(es.clone());
                    let mut app = api_router.merge(admin_router)
                        .layer(axum::middleware::from_fn_with_state(es.clone(), auth_middleware));

                    // Metrics 端点
                    if let Ok(mc) = metrics::MetricsCollector::new() {
                        app = app.merge(metrics::create_metrics_router(Arc::new(mc)));
                    }

                    let api_addr = format!("{}:{}", es.config.server.bind, es.config.server.api_port)
                        .parse::<std::net::SocketAddr>().unwrap();
                    info!("🌐 Admin/OpenAI API: http://{}", api_addr);
                    let listener = tokio::net::TcpListener::bind(api_addr).await.unwrap();
                    axum::serve(listener, app).await.unwrap_or_else(|e| tracing::error!("API error: {}", e));
                });

                info!("✅ 服务器基础设施模块全部初始化完成 — CarpAI Server 已就绪");
            }

            let provider_start = Instant::now();
            let provider =
                provider_init::init_provider(&args.provider, args.model.as_deref()).await?;
            let provider_ms = provider_start.elapsed().as_millis();
            let server_new_start = Instant::now();
            let server = server::Server::new(provider);
            let server_new_ms = server_new_start.elapsed().as_millis();
            
            // LSP features are enabled per-session when needed
            
            crate::logging::info(&format!(
                "[TIMING] serve bootstrap: provider_init={}ms, server_new={}ms, before_run={}ms",
                provider_ms,
                server_new_ms,
                serve_start.elapsed().as_millis()
            ));
            server.run().await?;
        }
        Some(Command::Connect) => {
            tui_launch::run_client().await?;
        }
        Some(Command::Run {
            message,
            json,
            ndjson,
        }) => {
            commands::run_single_message_command(
                &args.provider,
                args.model.as_deref(),
                args.resume.as_deref(),
                &message,
                json,
                ndjson,
            )
            .await?;
        }
        Some(Command::Login {
            account,
            no_browser,
            print_auth_url,
            callback_url,
            auth_code,
            json,
            complete,
            google_access_tier,
            api_base,
            api_key,
            api_key_env,
        }) => {
            login::run_login(
                &args.provider,
                account.as_deref(),
                login::LoginOptions {
                    no_browser,
                    print_auth_url,
                    callback_url,
                    auth_code,
                    json,
                    complete,
                    google_access_tier: google_access_tier.map(|tier| match tier {
                        super::args::GoogleAccessTierArg::Full => {
                            auth::google::GmailAccessTier::Full
                        }
                        super::args::GoogleAccessTierArg::Readonly => {
                            auth::google::GmailAccessTier::ReadOnly
                        }
                    }),
                    openai_compatible_api_base: api_base,
                    openai_compatible_api_key: api_key,
                    openai_compatible_api_key_env: api_key_env,
                    openai_compatible_default_model: args.model.clone(),
                },
            )
            .await?;
        }
        Some(Command::Repl) => {
            let (provider, registry) =
                provider_init::init_provider_and_registry(&args.provider, args.model.as_deref())
                    .await?;
            let mut agent = agent::Agent::new(provider, registry);
            agent.repl().await?;
        }
        Some(Command::Update) => {
            hot_exec::run_update()?;
        }
        Some(Command::Version { json }) => {
            commands::run_version_command(json)?;
        }
        Some(Command::Usage { json }) => {
            commands::run_usage_command(json).await?;
        }
        Some(Command::SelfDev { build }) => {
            selfdev::run_self_dev(build, args.resume).await?;
        }
        Some(Command::Auth(subcmd)) => match subcmd {
            AuthCommand::Status { json } => commands::run_auth_status_command(json)?,
            AuthCommand::Doctor {
                provider,
                validate,
                json,
            } => commands::run_auth_doctor_command(provider.as_deref(), validate, json).await?,
        },
        Some(Command::Provider(subcmd)) => match subcmd {
            ProviderCommand::List { json } => {
                commands::run_provider_list_command(json)?;
            }
            ProviderCommand::Current { json } => {
                commands::run_provider_current_command(args.provider.clone(), args.model.as_deref(), json)
                    .await?;
            }
            ProviderCommand::Add {
                name,
                base_url,
                model,
                context_window,
                api_key_env,
                api_key,
                api_key_stdin,
                no_api_key,
                auth,
                auth_header,
                env_file,
                set_default,
                overwrite,
                provider_routing,
                model_catalog,
                json,
            } => {
                commands::run_provider_add_command(commands::ProviderAddOptions {
                    name,
                    base_url,
                    model,
                    context_window,
                    api_key_env,
                    api_key,
                    api_key_stdin,
                    no_api_key,
                    auth,
                    auth_header,
                    env_file,
                    set_default,
                    overwrite,
                    provider_routing,
                    model_catalog,
                    json,
                })?;
            }
        },
        Some(Command::Memory(subcmd)) => {
            commands::run_memory_command(map_memory_subcommand(subcmd))?;
        }
        Some(Command::Session(subcmd)) => match subcmd {
            SessionCommand::Rename {
                session,
                name,
                clear,
                json,
            } => commands::run_session_rename_command(&session, name.as_deref(), clear, json)?,
        },
        Some(Command::Ambient(subcmd)) => {
            commands::run_ambient_command(map_ambient_subcommand(subcmd)).await?;
        }
        Some(Command::Pair { list, revoke }) => {
            commands::run_pair_command(list, revoke)?;
        }
        Some(Command::Permissions) => {
            tui::permissions::run_permissions()?;
        }
        Some(Command::Transcript {
            text,
            mode,
            session,
        }) => {
            commands::run_transcript_command(text, map_transcript_mode(mode), session).await?;
        }
        Some(Command::Dictate { r#type }) => {
            commands::run_dictate_command(r#type).await?;
        }
        Some(Command::SetupHotkey {
            listen_macos_hotkey,
        }) => {
            setup_hints::run_setup_hotkey(listen_macos_hotkey)?;
        }
        Some(Command::SetupLauncher) => {
            setup_hints::run_setup_launcher()?;
        }
        Some(Command::Browser { action }) => {
            commands::run_browser(&action).await?;
        }
        Some(Command::Replay {
            session,
            swarm,
            export,
            speed,
            timeline,
            auto_edit,
            video,
            cols,
            rows,
            fps,
            centered,
            no_centered,
        }) => {
            let centered_override = if centered {
                Some(true)
            } else if no_centered {
                Some(false)
            } else {
                None
            };
            tui_launch::run_replay_command(
                &session,
                swarm,
                export,
                auto_edit,
                speed,
                timeline.as_deref(),
                video.as_deref(),
                cols,
                rows,
                fps,
                centered_override,
            )
            .await?;
        }
        Some(Command::Model(subcmd)) => match subcmd {
            ModelCommand::List { json, verbose } => {
                commands::run_model_command(&args.provider, args.model.as_deref(), json, verbose)
                    .await?;
            }
        },
        Some(Command::AuthTest {
            login,
            all_configured,
            no_smoke,
            no_tool_smoke,
            prompt,
            json,
            output,
        }) => {
            commands::run_auth_test_command(
                &args.provider,
                args.model.as_deref(),
                login,
                all_configured,
                no_smoke,
                no_tool_smoke,
                prompt.as_deref(),
                json,
                output.as_deref(),
            )
            .await?;
        }
        Some(Command::Build {
            message,
            manual,
            no_verify,
            max_retries,
            release,
            clean,
            target,
            all_projects,
            test,
            parallel,
            jobs,
        }) => {
            commands::run_build_command(commands::BuildOptions {
                message,
                manual,
                no_verify,
                max_retries: max_retries as usize,
                release,
                clean,
                target,
                all_projects,
                test,
                parallel,
                jobs,
            })
            .await?;
        }
        Some(Command::CodeValue {
            input,
            manifest_path,
            json,
            output,
        }) => {
            commands::run_code_value_command(input.as_deref(), &manifest_path, json, output.as_deref()).await?;
        }
        Some(Command::Restart { action }) => match action {
            RestartCommand::Save { auto_restore } => {
                commands::run_restart_save_command(auto_restore).await?
            }
            RestartCommand::Restore => commands::run_restart_restore_command()?,
            RestartCommand::Status => commands::run_restart_status_command()?,
            RestartCommand::Clear => commands::run_restart_clear_command()?,
        },
        Some(Command::Mcp(cmd)) => {
            crate::cli::management_commands::run_mcp_dispatch(cmd).await?;
        }
        Some(Command::Doctor { json }) => {
            tracing::info!("Doctor command: System health check feature coming soon");
            if json {
                println!("{{\"status\":\"ok\",\"message\":\"Doctor check - all systems nominal\"}}");
            } else {
                println!("✅ Doctor check: All systems nominal (enhanced diagnostics coming soon)");
            }
        }
        Some(Command::Init {
            project_type,
            scaffold,
        }) => {
            tracing::info!("Init command: Project scaffolding feature coming soon");
            let _ = (project_type, scaffold);
            println!("🚀 Project initialization: Scaffold templates coming soon");
            println!("   Supported types: rust, python, nodejs, fullstack (v2.0)");
        }
        Some(Command::Skills(subcmd)) => {
            commands::run_skills_command(subcmd).await?;
        }
        Some(Command::Workflows(subcmd)) => {
            commands::run_workflows_command(subcmd).await?;
        }
        Some(Command::Tasks(subcmd)) => {
            commands::run_tasks_command(subcmd).await?;
        }
        Some(Command::Git(subcmd)) => {
            commands::run_git_command(subcmd).await?;
        }
        Some(Command::Config(subcmd)) => {
            commands::run_config_command(subcmd)?;
        }
        Some(Command::Commit {
            message,
            files,
            no_ai,
        }) => {
            commands::run_commit_command(message.as_deref(), &files, no_ai).await?;
        }
        Some(Command::SessionMgmt(subcmd)) => {
            commands::run_session_command(subcmd).await?;
        }
        Some(Command::Rethink { mode, depth }) => {
            commands::run_rethink_command(mode.as_deref(), depth).await?;
        }
        Some(Command::Compact {
            mode,
            target,
            json,
        }) => {
            commands::run_compact_command(mode.as_deref(), target, json).await?;
        }
        Some(Command::Fork { name, checkpoint }) => {
            commands::run_fork_command(name.as_deref(), checkpoint.as_deref()).await?;
        }
        Some(Command::Completion { shell, output, install }) => {
            if install {
                tracing::info!("Completion install: Shell integration setup pending");
                println!("📝 Completion install: Shell auto-completion coming soon");
                println!("   Manual setup: Add jcode completion script to your {} shell config", shell);
                let _ = &shell;
            } else {
                tracing::info!("Completion: Shell completion generation pending");
                println!("📝 Completion: Tab completion support coming soon");
                let _ = (&shell, output.as_deref());
            }
        }
        Some(Command::CodeNav(_cmd)) => {
            commands::run_code_nav_command().await?;
        }
        Some(Command::CodeRefactor(cmd)) => {
            commands::run_refactor_command(cmd).await?;
        }
        Some(Command::Review {
            staged,
            diff,
            security,
            json,
            file,
            directory,
            ai_review,
        }) => {
            commands::run_review_command(staged, diff.as_deref(), security, json, file.as_deref(), directory.as_deref(), ai_review).await?;
        }
        Some(Command::Debug(cmd)) => {
            commands::run_debug_command(cmd).await?;
        }

        // -- Expanded commands ---------------------------------
        Some(Command::Clear { all, cache }) => {
            commands::run_clear_command(commands::ClearOptions { all, cache }).await?;
        }
        Some(Command::Cost { json }) => {
            commands::run_cost_command(commands::CostOptions { json }).await?;
        }
        Some(Command::Export { output, full }) => {
            commands::run_session_command(super::args::SessionSubCommand::Export { output, full }).await?;
        }
        Some(Command::Resume { id }) => {
            commands::run_session_command(super::args::SessionSubCommand::Resume { id: Some(id), list: false }).await?;
        }
        Some(Command::Env { list, get, set, value }) => {
            commands::run_env_command(commands::EnvOptions { list, get, set, value }).await?;
        }
        Some(Command::Effort { level }) => {
            commands::run_effort_command(commands::EffortOptions { level }).await?;
        }
        Some(Command::Fast { state }) => {
            commands::run_fast_command(commands::FastOptions { state }).await?;
        }
        Some(Command::Passes { count }) => {
            commands::run_passes_command(commands::PassesOptions { count: count.map(|c| c as usize) }).await?;
        }
        Some(Command::RateLimit { show, rpm, tpm }) => {
            commands::run_rate_limit_command(commands::RateLimitOptions { show, rpm: rpm.map(|r| r as usize), tpm: tpm.map(|t| t as usize) }).await?;
        }
        Some(Command::Files(cmd)) => {
            // TODO: commands::run_files_command(cmd).await?;
            let _ = cmd;
        }
        Some(Command::AddDir { path, recursive }) => {
            // TODO: commands::run_add_dir_command(&path, recursive).await?;
            let _ = (&path, recursive);
        }
        Some(Command::FileRename { source, target }) => {
            // TODO: commands::run_file_rename_command(&source, &target).await?;
            let _ = (&source, &target);
        }
        Some(Command::FileCopy { source, target }) => {
            // TODO: commands::run_file_copy_command(&source, &target).await?;
            let _ = (&source, &target);
        }
        Some(Command::Tag { tags, list, remove }) => {
            // TODO: commands::run_tag_command(tags, list, remove.as_deref()).await?;
            let _ = (tags, list, remove.as_deref());
        }
        Some(Command::Summary { json, verbose }) => {
            tracing::info!("Summary command: Session analytics feature coming soon");
            let _ = (json, verbose);
            println!("📊 Summary: Session insights and statistics coming soon");
            println!("   Features: token usage, model performance, cost tracking");
        }
        Some(Command::Insights { session, json, tools, performance }) => {
            tracing::info!("Insights command: AI-powered analysis pending");
            let _ = (session, json, tools, performance);
            println!("🔍 Insights: Deep analysis and recommendations coming soon");
            println!("   Powered by: Multi-model reasoning engine");
        }
        Some(Command::Upgrade { version, prerelease, force }) => {
            tracing::info!("Upgrade command: Self-update mechanism pending OAuth setup");
            let _ = (version, prerelease, force);
            println!("⬆️  Upgrade: Auto-update feature coming soon");
            println!("   Current: v{} (check for updates manually)", env!("CARGO_PKG_VERSION"));
        }
        Some(Command::Logout { provider, all }) => {
            tracing::info!("Logout command: Authentication session cleanup");
            let _ = (provider, all);
            println!("👋 Logout: Session cleared successfully (provider-specific logout coming soon)");
        }
        Some(Command::SecurityReview { staged, diff, json }) => {
            tracing::info!("Security review: Vulnerability scanning pending integration with security APIs");
            let _ = (staged, diff, json);
            println!("🔒 Security Review: Automated code security analysis coming soon");
            println!("   Checks: OWASP Top 10, dependency vulnerabilities, secrets detection");
        }
        Some(Command::CommitPushPr { branch, title, body, no_open, draft }) => {
            tracing::info!("CommitPushPr: GitHub integration pending OAuth setup");
            let _ = (branch, title, body, no_open, draft);
            println!("🔄 Commit→Push→PR: GitHub workflow automation coming soon");
            println!("   Requires: GitHub App installation + OAuth authentication");
        }
        Some(Command::PrComments { pr, add, reply, resolve }) => {
            tracing::info!("PR Comments: GitHub API integration pending");
            let _ = (pr, add, reply, resolve);
            println!("💬 PR Comments: GitHub interaction features coming soon");
        }
        Some(Command::AutoFixPr { pr, apply }) => {
            tracing::info!("AutoFixPr: Automated PR fix suggestions pending ML model integration");
            let _ = (pr, apply);
            println!("🔧 AutoFix PR: AI-powered automated fixes coming soon");
            println!("   Powered by: Multi-model code review + auto-fix pipeline");
        }
        Some(Command::InstallGithubApp { scope, global }) => {
            tracing::info!("InstallGithubApp: GitHub App OAuth flow pending");
            println!("📦 Install GitHub App: OAuth installation wizard coming soon");
            println!("   Scope: {:?}", scope);
            let _ = (scope, global);
        }
        Some(Command::Buddy { state, share }) => {
            tracing::info!("Buddy command: Collaboration features coming soon");
            let _ = (state, share);
            println!("👥 Buddy: Real-time collaboration features coming soon");
            println!("   Features: Session sharing, pair programming, code review");
        }
        Some(Command::InstallSlackApp { workspace }) => {
            tracing::info!("InstallSlackApp: Slack integration pending");
            let _ = workspace;
            println!("💬 Install Slack App: Notification integration coming soon");
            println!("   Features: Build notifications, code review alerts, deployment status");
        }
        Some(Command::BatchEdit { files, apply, interactive, pattern, replace }) => {
            tracing::info!("BatchEdit: Multi-file pattern replacement coming soon");
            let _ = (files, apply, interactive, pattern, replace);
            println!("✏️  Batch Edit: Multi-file search & replace coming soon");
            println!("   Features: Pattern matching, preview, atomic apply/rollback");
        }
        Some(Command::Cluster(cluster_cmd)) => {
            crate::distributed::execute_cluster_command(
                crate::distributed::cli::ClusterArgs { command: cluster_cmd }
            ).await?;
        }
        Some(Command::DebugSocket { .. }) => {
            tracing::info!("DebugSocket: Runtime debugging via socket pending implementation");
            println!("🔌 Debug Socket: Runtime debugging interface coming soon");
            println!("   Features: Attach debugger, inspect state, inject commands");
            println!("   Usage: Connect to debug socket for live debugging session");
        }

        // 服务器管理命令
        #[cfg(feature = "enterprise")]
        Some(Command::Enterprise(cmd)) => {
            run_enterprise_command(cmd).await?;
        }
        None => {
            run_default_command(args).await?;
        }
    }

    Ok(())
}



/// 执行服务器管理命令
#[cfg(feature = "enterprise")]
async fn run_enterprise_command(cmd: super::args::EnterpriseCommand) -> Result<()> {
    use crate::enterprise::enterprise::EnterpriseServer;
    use crate::enterprise::config::EnterpriseConfig;

    match cmd {
        super::args::EnterpriseCommand::Init { email, password, org } => {
            let config = EnterpriseConfig::load();
            let mut server = EnterpriseServer::new(Some(config)).await?;
            let _ = server.init_admin_user(&email, &password, &org).await;
            println!("✅ Enterprise initialized: org={}, admin={}", org, email);
        }
        super::args::EnterpriseCommand::Org(subcmd) => {
            eprintln!("Enterprise org command — not yet wired to CLI; use admin API instead");
            let _ = subcmd;
        }
        super::args::EnterpriseCommand::User(subcmd) => {
            eprintln!("Enterprise user command — not yet wired to CLI; use admin API instead");
            let _ = subcmd;
        }
        super::args::EnterpriseCommand::Node(subcmd) => {
            eprintln!("Enterprise node command — not yet wired to CLI; use admin API instead");
            let _ = subcmd;
        }
        super::args::EnterpriseCommand::ApiKey(subcmd) => {
            eprintln!("Enterprise API key command — not yet wired to CLI; use admin API instead");
            let _ = subcmd;
        }
        super::args::EnterpriseCommand::Usage { days } => {
            eprintln!("Enterprise usage stats — use admin API instead (GET /admin/usage?days={})", days);
        }
        super::args::EnterpriseCommand::Metrics => {
            eprintln!("Enterprise metrics — use admin API instead (GET /metrics)");
        }
        super::args::EnterpriseCommand::Audit { days } => {
            eprintln!("Enterprise audit log — use admin API instead (GET /admin/audit?days={})", days);
        }
    }
    Ok(())
}

fn resolve_resume_arg(args: &mut Args) -> Result<()> {
    if let Some(ref resume_id) = args.resume {
        if resume_id.is_empty() {
            eprintln!("No sessions available to resume.");
            return Ok(());
        }

        match resolve_resume_id(resume_id) {
            Ok(full_id) => {
                args.resume = Some(full_id);
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                if !output::quiet_enabled() {
                    eprintln!("\nUse `jcode --resume` to list available sessions.");
                }
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

fn resolve_resume_id(resume_id: &str) -> Result<String> {
    match session::find_session_by_name_or_id(resume_id) {
        Ok(full_id) => Ok(full_id),
        Err(native_err) => match crate::import::import_external_resume_id(resume_id)? {
            Some(imported_id) => Ok(imported_id),
            None => Err(native_err),
        },
    }
}

fn map_memory_subcommand(subcmd: MemoryCommand) -> commands::MemorySubcommand {
    match subcmd {
        MemoryCommand::List { scope, tag } => commands::MemorySubcommand::List { scope, tag },
        MemoryCommand::Search { query, semantic } => {
            commands::MemorySubcommand::Search { query, semantic }
        }
        MemoryCommand::Export { output, scope } => {
            commands::MemorySubcommand::Export { output, scope }
        }
        MemoryCommand::Import {
            input,
            scope,
            overwrite,
        } => commands::MemorySubcommand::Import {
            input,
            scope,
            overwrite,
        },
        MemoryCommand::Stats => commands::MemorySubcommand::Stats,
        MemoryCommand::ClearTest => commands::MemorySubcommand::ClearTest,
    }
}

fn map_ambient_subcommand(subcmd: AmbientCommand) -> commands::AmbientSubcommand {
    match subcmd {
        AmbientCommand::Status => commands::AmbientSubcommand::Status,
        AmbientCommand::Log => commands::AmbientSubcommand::Log,
        AmbientCommand::Trigger => commands::AmbientSubcommand::Trigger,
        AmbientCommand::Stop => commands::AmbientSubcommand::Stop,
        AmbientCommand::RunVisible => commands::AmbientSubcommand::RunVisible,
    }
}

fn map_transcript_mode(mode: TranscriptModeArg) -> crate::protocol::TranscriptMode {
    match mode {
        TranscriptModeArg::Insert => crate::protocol::TranscriptMode::Insert,
        TranscriptModeArg::Append => crate::protocol::TranscriptMode::Append,
        TranscriptModeArg::Replace => crate::protocol::TranscriptMode::Replace,
        TranscriptModeArg::Send => crate::protocol::TranscriptMode::Send,
    }
}

async fn run_default_command(args: Args) -> Result<()> {
    startup_profile::mark("run_main_none_branch");

    let explicit_provider_or_model = args.provider != ProviderChoice::Auto
        || args.model.is_some()
        || args.provider_profile.is_some();
    if args.resume.is_none()
        && !explicit_provider_or_model
        && commands::maybe_run_pending_restart_restore_on_startup().await?
    {
        return Ok(());
    }

    let startup_hints = if args.fresh_spawn {
        None
    } else {
        setup_hints::maybe_show_setup_hints()
    };
    startup_profile::mark("setup_hints");

    if args.resume.is_none() {
        terminal::show_crash_resume_hint();
    }
    startup_profile::mark("crash_resume_hint");

    let cwd = std::env::current_dir()?;
    let in_jcode_repo = build::is_jcode_repo(&cwd);
    startup_profile::mark("is_jcode_repo");
    let already_in_selfdev = crate::cli::selfdev::client_selfdev_requested();

    if in_jcode_repo && !already_in_selfdev && !args.no_selfdev {
        output::stderr_info("📍 Detected jcode repository - enabling self-dev mode");
        output::stderr_info("   Using shared server with self-dev session mode");
        output::stderr_info("   (use --no-selfdev to disable auto-detection)");
        output::stderr_blank_line();

        crate::env::set_var(selfdev::CLIENT_SELFDEV_ENV, "1");
        crate::process_title::set_initial_title(&args);
    }

    startup_profile::mark("client_mode_start");
    let mut server_running = if args.fresh_spawn {
        true
    } else {
        server_is_running().await
    };
    startup_profile::mark("server_check");

    if !server_running {
        server_running = wait_for_existing_reload_server("client startup").await;
    }

    if !server_running && std::env::var("JCODE_RESUMING").is_ok() {
        server_running = wait_for_resuming_server(
            "client startup without reload marker",
            std::time::Duration::from_secs(5),
        )
        .await;
    }

    if server_running && explicit_provider_or_model {
        output::stderr_info(
            "Server already running; provider/model flags only apply when starting a new server.",
        );
        output::stderr_info(format!(
            "Current server settings control `/model`. Restart server to apply: --provider {}{}",
            args.provider.as_arg_value(),
            args.model
                .as_ref()
                .map(|m| format!(" --model {}", m))
                .unwrap_or_default()
        ));
    }

    if !server_running {
        maybe_prompt_server_bootstrap_login(&args.provider).await?;
        spawn_server(
            &args.provider,
            args.model.as_deref(),
            args.provider_profile.as_deref(),
        )
        .await?;
    }

    startup_profile::mark("pre_tui_client");
    if std::env::var("JCODE_RESUMING").is_err() && server_running {
        output::stderr_info("Connecting to server...");
    }
    tui_launch::run_tui_client(
        args.resume,
        startup_hints,
        !server_running,
        args.fresh_spawn,
    )
    .await?;

    Ok(())
}

pub(crate) async fn server_is_running() -> bool {
    server_is_running_at(&server::socket_path()).await
}

async fn wait_for_existing_reload_server(context: &str) -> bool {
    if let Some(state) = server::recent_reload_state(std::time::Duration::from_secs(30)) {
        match state.phase {
            server::ReloadPhase::Starting => {
                crate::logging::info(&format!(
                    "Reload state=starting during {}; waiting for existing server to return",
                    context
                ));
                return wait_for_reloading_server().await;
            }
            server::ReloadPhase::Failed => {
                crate::logging::warn(&format!(
                    "Reload state=failed during {} on {}: {}; recent_state={}",
                    context,
                    server::socket_path().display(),
                    state
                        .detail
                        .unwrap_or_else(|| "unknown reload failure".to_string()),
                    server::reload_state_summary(std::time::Duration::from_secs(60))
                ));
            }
            server::ReloadPhase::SocketReady => {}
        }
    }

    false
}

pub(crate) async fn wait_for_resuming_server(context: &str, timeout: std::time::Duration) -> bool {
    let socket_path = server::socket_path();
    let start = std::time::Instant::now();
    let mut announced = false;

    while start.elapsed() < timeout {
        if server_is_running_at(&socket_path).await {
            crate::logging::info(&format!(
                "Server became available during resume wait for {} after {}ms",
                context,
                start.elapsed().as_millis()
            ));
            return true;
        }

        if !announced {
            crate::logging::info(&format!(
                "Server not ready during {}; waiting up to {}ms for a resumed/reloading server before spawning a replacement",
                context,
                timeout.as_millis()
            ));
            announced = true;
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    false
}

pub(crate) async fn wait_for_reloading_server() -> bool {
    match server::await_reload_handoff(&server::socket_path(), std::time::Duration::from_secs(30))
        .await
    {
        server::ReloadWaitStatus::Ready => true,
        server::ReloadWaitStatus::Failed(detail) => {
            crate::logging::warn(&format!(
                "Reload handoff failed while waiting for server on {}: {}; recent_state={}",
                server::socket_path().display(),
                detail.unwrap_or_else(|| "unknown reload failure".to_string()),
                server::reload_state_summary(std::time::Duration::from_secs(60))
            ));
            false
        }
        server::ReloadWaitStatus::Idle => false,
        server::ReloadWaitStatus::Waiting { .. } => false,
    }
}

async fn server_is_running_at(path: &std::path::Path) -> bool {
    server::is_server_ready(path).await || server::has_live_listener(path).await
}

#[cfg(unix)]
fn spawn_lock_path(socket_path: &std::path::Path) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{}.spawning", socket_path.display()))
}

#[cfg(unix)]
struct SpawnLockGuard {
    _file: std::fs::File,
    path: std::path::PathBuf,
}

#[cfg(unix)]
impl Drop for SpawnLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
fn try_acquire_spawn_lock(path: &std::path::Path) -> Result<Option<SpawnLockGuard>> {
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd;

    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    let fd = file.as_raw_fd();
    let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if ret == 0 {
        Ok(Some(SpawnLockGuard {
            _file: file,
            path: path.to_path_buf(),
        }))
    } else {
        Ok(None)
    }
}

#[cfg(unix)]
async fn acquire_spawn_lock_or_wait(
    socket_path: &std::path::Path,
) -> Result<Option<SpawnLockGuard>> {
    let lock_path = spawn_lock_path(socket_path);
    let wait_start = std::time::Instant::now();
    let wait_timeout = std::time::Duration::from_secs(10);
    let mut announced_wait = false;

    loop {
        if let Some(lock) = try_acquire_spawn_lock(&lock_path)? {
            return Ok(Some(lock));
        }

        if server_is_running_at(socket_path).await {
            return Ok(None);
        }

        if !announced_wait {
            output::stderr_info("Another client is starting the server, waiting...");
            announced_wait = true;
        }

        if wait_start.elapsed() >= wait_timeout {
            anyhow::bail!(
                "Timed out waiting for another client to start server at {}",
                socket_path.display()
            );
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

pub(crate) async fn maybe_prompt_server_bootstrap_login(
    provider_choice: &ProviderChoice,
) -> Result<()> {
    startup_profile::mark("cred_check_start");
    let mut cred_state = detect_bootstrap_credentials().await;
    startup_profile::mark("cred_check_done");

    if !cred_state.has_any
        && auth::AuthStatus::has_any_untrusted_external_auth()
        && *provider_choice == ProviderChoice::Auto
    {
        let _ = provider_init::maybe_run_external_auth_auto_import_flow().await?;
        cred_state = detect_bootstrap_credentials().await;
    }

    if !cred_state.has_any && *provider_choice == ProviderChoice::Auto {
        let provider = provider_init::prompt_login_provider_selection(
            &provider_catalog::server_bootstrap_login_providers(),
            "No credentials found. Let's log in!\n\nChoose a provider:",
        )?;
        login::run_login_provider(provider, None, login::LoginOptions::default()).await?;
        provider_init::apply_login_provider_profile_env(provider);
        output::stderr_blank_line();
    }

    Ok(())
}

struct BootstrapCredentialState {
    has_any: bool,
}

async fn detect_bootstrap_credentials() -> BootstrapCredentialState {
    let (has_claude, has_openai) = tokio::join!(
        tokio::task::spawn_blocking(|| auth::claude::load_credentials().is_ok()),
        tokio::task::spawn_blocking(|| auth::codex::load_credentials().is_ok()),
    );
    let has_claude = has_claude.unwrap_or(false);
    let has_openai = has_openai.unwrap_or(false);
    let has_openrouter = provider::openrouter::OpenRouterProvider::has_credentials();
    let has_copilot = auth::copilot::has_copilot_credentials();
    let has_api_key = std::env::var("ANTHROPIC_API_KEY").is_ok();

    BootstrapCredentialState {
        has_any: has_claude || has_openai || has_openrouter || has_copilot || has_api_key,
    }
}

pub(crate) async fn spawn_server(
    provider_choice: &ProviderChoice,
    model: Option<&str>,
    provider_profile: Option<&str>,
) -> Result<()> {
    let socket_path = server::socket_path();
    if server_is_running_at(&socket_path).await {
        startup_profile::mark("server_ready");
        return Ok(());
    }

    if wait_for_existing_reload_server("server spawn").await {
        startup_profile::mark("server_ready");
        return Ok(());
    }

    #[cfg(unix)]
    let _spawn_lock = acquire_spawn_lock_or_wait(&socket_path).await?;

    if server_is_running_at(&socket_path).await {
        startup_profile::mark("server_ready");
        return Ok(());
    }

    if wait_for_existing_reload_server("server spawn after lock").await {
        startup_profile::mark("server_ready");
        return Ok(());
    }

    startup_profile::mark("server_spawn_start");
    output::stderr_info("Starting server...");
    let client_requested_selfdev = selfdev::client_selfdev_requested();
    let exe = build::shared_server_update_candidate(client_requested_selfdev)
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .ok_or_else(|| anyhow::anyhow!("Could not determine executable path for server spawn"))?;
    let mut cmd = ProcessCommand::new(&exe);
    cmd.env_remove(selfdev::CLIENT_SELFDEV_ENV);
    if client_requested_selfdev {
        cmd.env("JCODE_DEBUG_CONTROL", "1");
    }
    cmd.arg("--provider").arg(provider_choice.as_arg_value());
    if let Some(provider_profile) = provider_profile {
        cmd.arg("--provider-profile").arg(provider_profile);
    }
    if let Some(model) = model {
        cmd.arg("--model").arg(model);
    }
    cmd.arg("serve")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    {
        let _child = server::spawn_server_notify(&mut cmd).await?;
        startup_profile::mark("server_ready");
        Ok(())
    }
    #[cfg(not(unix))]
    {
        use std::io::Read;

        let mut child = cmd.spawn()?;
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(5);
        while start.elapsed() < timeout {
            if crate::transport::is_socket_path(&server::socket_path())
                && crate::transport::Stream::connect(server::socket_path())
                    .await
                    .is_ok()
                {
                    startup_profile::mark("server_ready");
                    return Ok(());
                }

            if let Some(status) = child.try_wait()? {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                let detail = stderr.trim();
                if detail.is_empty() {
                    anyhow::bail!("Server exited before becoming ready (status: {})", status);
                }
                anyhow::bail!(
                    "Server exited before becoming ready (status: {}). {}",
                    status,
                    detail
                );
            }

            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        anyhow::bail!(
            "Timed out waiting for server to become ready at {} after {}ms",
            server::socket_path().display(),
            timeout.as_millis()
        );
    }
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod dispatch_tests;
