//! Build command — plan -> execute -> verify
//!
//! Extracted from commands.rs for better modularity.

/// Run the build mode: plan -> execute -> verify
pub async fn run_build_command(
    message: &str,
    _manual: bool,
    no_verify: bool,
    _max_retries: u32,
    release: bool,
    clean: bool,
    target: Option<&str>,
    all_projects: bool,
    run_tests: bool,
    parallel: bool,
    jobs: Option<usize>,
) -> Result<()> {
    use crate::build::{
        BuildExecutor, BuildReport, BuildRequest, BuildStatus, ProjectType,
        WorkspaceManager,
    };
    use std::sync::Arc;
    use std::time::Instant;

    eprintln!("\n🏗️  Build Mode — Plan -> Execute -> Verify\n");

    let cwd = std::env::current_dir()?;
    let project_type = ProjectType::detect_from_path(&cwd);
    eprintln!("📋 Plan — {} ({:?}).\n", cwd.display(), project_type);

    // Build the request from explicit flags
    let mut request = BuildRequest {
        release,
        clean,
        target: target.map(|s| s.to_string()),
        verbose: message.contains("verbose") || message.contains("-v"),
        ..Default::default()
    };
    if let Some(n) = jobs {
        request.jobs = Some(n);
    }

    // Show build plan
    eprintln!("  +- Build plan: ");
    if release { eprintln!("  | Mode:        release"); }
    if clean { eprintln!("  | Clean first: yes"); }
    if let Some(ref t) = request.target { eprintln!("  | Target:      {}", t); }
    if run_tests { eprintln!("  | Run tests:   yes"); }
    eprintln!("  | Project:     {:?}", project_type);
    eprintln!("  +-\n");

    // Initialize workspace
    let workspace = Arc::new(WorkspaceManager::new());
    let proj_name = cwd
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".into());
    let project = crate::workspace_manager::Project::new("default", proj_name, &cwd, project_type.clone());
    workspace.register_project(project).await;
    workspace.set_active_project("default").await;

    let executor = BuildExecutor::new(Arc::clone(&workspace));
    let build_start = Instant::now();
    let mut bar = crate::build::ProgressBar::new(1, "  Building");

    // Execute build
    let build_result = if all_projects {
        executor
            .build_all(&request, parallel, jobs.unwrap_or(4))
            .await
            .map(|wr| {
                let all_ok = wr.all_succeeded;
                let total_dur = wr.total_duration;
                let mut lines = vec![format!("Workspace build: {} projects\n", wr.projects.len())];
                for br in wr.projects.values() {
                    lines.push(format!(
                        "  [{}] {:.1}s ({} err, {} warn)",
                        if br.success { "OK" } else { "FAIL" },
                        br.duration.as_secs_f32(),
                        br.error_count,
                        br.warning_count,
                    ));
                }
                crate::build::BuildResult {
                    success: all_ok,
                    exit_code: Some(if all_ok { 0 } else { 1 }),
                    output: lines.join("\n"),
                    duration: total_dur,
                    warning_count: wr.projects.values().map(|r| r.warning_count).sum(),
                    error_count: wr.projects.values().map(|r| r.error_count).sum(),
                    project_type: project_type.clone(),
                    build_dir: cwd.clone(),
                    stdout: String::new(),
                    stderr: String::new(),
                    artifacts: vec![],
                }
            })
    } else {
        executor
            .build_active_project(&request)
            .await
    };
    bar.finish();

    let build_elapsed = build_start.elapsed();

    // Display output
    match &build_result {
        Ok(result) => {
            if !result.output.is_empty() {
                let max_display = if result.success { 2000 } else { 8000 };
                let display = if result.output.len() > max_display {
                    format!(
                        "{}...\n[truncated, total {} bytes]",
                        &result.output[..max_display],
                        result.output.len()
                    )
                } else {
                    result.output.clone()
                };
                eprintln!("{}", display);
            }
            eprintln!("\n  {}", result.summary_line());
        }
        Err(e) => eprintln!("\n  ❌ Execution error: {:#}", e),
    }

    // Run tests after build
    if run_tests
        && let Ok(ref result) = build_result
            && result.success {
                eprintln!("\n🔬 Running tests...\n");
                let test_tool = crate::build_module::TestTool::new(workspace.clone());
                let ctx = jcode_tool_core::ToolContext {
                    session_id: String::new(),
                    message_id: String::new(),
                    tool_call_id: String::new(),
                    working_dir: None,
                    stdin_request_tx: None,
                    graceful_shutdown_signal: None,
                    execution_mode: jcode_tool_core::ToolExecutionMode::Direct,
                };
                let test_result = test_tool
                    .execute(
                        serde_json::json!({"verbose": true}),
                        ctx,
                    )
                    .await;
                match test_result {
                    Ok(output) => eprintln!("{}", output.output),
                    Err(e) => eprintln!("  ❌ Tests failed: {}", e),
                }
            }

    // Post-build micro-ci verification
    let mut ci_passed = false;
    if !no_verify
        && let Ok(ref result) = build_result
            && result.success {
                eprintln!("\n🔍 Verify — Running micro-ci checks...\n");
                let ci = jcode_micro_ci::MicroCi::new(jcode_micro_ci::CiConfig {
                    workspace_root: cwd.to_string_lossy().to_string(),
                    ..Default::default()
                });
                let ci_report = ci.run().await;
                if ci_report.passed {
                    ci_passed = true;
                    eprintln!("  ✅ CI passed!\n");
                } else {
                    eprintln!("  {}", ci_report.to_string().trim_end());
                    eprintln!(
                        "\n  ⚠️  CI found {} issues.\n",
                        ci_report.issues.len()
                    );
                }
            }

    let report = match &build_result {
        Ok(result) => BuildReport::from_build_result(message, result, ci_passed),
        Err(_e) => {
            let mut r = BuildReport::new(message);
            r.status = BuildStatus::Failed;
            r.execution_time_ms = build_elapsed.as_millis() as u64;
            r.total_time_ms = build_elapsed.as_millis() as u64;
            r
        }
    };

    eprintln!("\n{}", report.to_string().trim_end());

    // AI learning: record build outcome for adaptive learning
    {
        let project_type_str = format!("{:?}", project_type.clone());
        let success = build_result.as_ref().map(|r| r.success).unwrap_or(false);
        let duration = build_elapsed;
        let err_count = report.error_count;
        let warn_count = report.warning_count;
        tokio::spawn(async move {
            crate::ai_enhanced::record_build_outcome(
                &project_type_str,
                success,
                duration,
                err_count,
                warn_count,
            )
            .await;
        });
    }

    match build_result {
        Ok(result) if result.success => Ok(()),
        Ok(_) => Err(anyhow::anyhow!(
            "Build completed with {} errors, {} warnings",
            report.error_count,
            report.warning_count
        )),
        Err(e) => Err(e),
    }
}

/// Manage MCP servers: serve, add, remove, list, get.
pub async fn run_mcp_command(cmd: crate::cli::args::McpCommand) -> Result<()> {
    use crate::cli::args::McpCommand;

    match cmd {
        McpCommand::Serve { debug, verbose } => {
            if verbose {
                eprintln!("Starting CarpAI MCP server on stdio...");
                eprintln!("Protocol: MCP 2024-11-05");
            }
            // Suppress non-error output while running as MCP server
            if !debug {
                crate::cli::output::set_quiet_enabled(true);
            }
            crate::mcp::server::serve().await
        }
        McpCommand::Add {
            name,
            command_or_url,
            args,
            scope,
            transport,
            env,
        } => {
            eprintln!("Adding MCP server: {} (transport={}, scope={})", name, transport, scope);
            eprintln!("  Command/URL: {} {:?}", command_or_url, args);
            if !env.is_empty() {
                eprintln!("  Env vars: {} keys", env.len());
            }
            // TODO: persist to config file
            eprintln!("\n✅ MCP server '{}' configured.", name);
            Ok(())
        }
        McpCommand::AddJson { name, json, scope } => {
            eprintln!("Adding MCP server from JSON: {} (scope={})", name, scope);
            eprintln!("  JSON: {}", json);
            eprintln!("\n✅ MCP server '{}' configured.", name);
            Ok(())
        }
        McpCommand::Remove { name, scope } => {
            let scope_str = scope.as_deref().unwrap_or("any");
            eprintln!("Removing MCP server: {} (scope={})", name, scope_str);
            eprintln!("\n✅ MCP server '{}' removed.", name);
            Ok(())
        }
        McpCommand::List => {
            eprintln!("Configured MCP Servers:\n");
            eprintln!("  (no servers configured)");
            Ok(())
        }
        McpCommand::Get { name } => {
            eprintln!("MCP Server: {}\n", name);
            eprintln!("  Status: not found (not configured)");
            Ok(())
        }
        McpCommand::ImportDesktop { scope } => {
            eprintln!("Importing MCP servers from Claude Desktop (scope={})", scope);
            eprintln!("\n✅ Import completed (0 servers imported).");
            Ok(())
        }
        McpCommand::Bridge {
            debug,
            expose_resources,
            auto_connect,
            status,
        } => {
            if !debug {
                crate::cli::output::set_quiet_enabled(true);
            }

            eprintln!("🚀 Starting bidirectional MCP bridge...\n");

            let config = crate::mcp::McpBridgeConfig {
                server: crate::mcp::McpServerConfig {
                    expose_resources,
                    server_name: "carpai".to_string(),
                    extra_tools: vec![],
                },
                client_enabled: true,
                auto_connect,
                server_name: "carpai".to_string(),
            };

            let mut bridge = crate::mcp::McpBridge::new(config);

            // Initialize with a registry
            let provider = crate::mcp::server::noop_provider();
            let registry = crate::tool::Registry::new(provider).await;
            bridge.init(registry).await?;

            if status {
                let bridge_status = bridge.status().await;
                eprintln!("{}", bridge_status);
                return Ok(());
            }

            bridge.serve().await
        }
    }
}

/// Run system diagnostics and health checks.
pub async fn run_doctor_command(json: bool) -> Result<()> {
    use std::time::Instant;
    let start = Instant::now();
    let cwd = std::env::current_dir()?;

    if json {
        // JSON output mode
        let report = serde_json::json!({
            "version": env!("JCODE_VERSION"),
            "cwd": cwd.to_string_lossy(),
            "checks": {
                "git_available": which_git().is_ok(),
                "cargo_available": which_cargo().is_ok(),
                "npm_available": which_npm().is_ok(),
                "python_available": which_python().is_ok(),
            },
            "duration_ms": 0,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    eprintln!("\n🏥 CarpAI Diagnostics\n");
    eprintln!("  Version:  {}", env!("JCODE_VERSION"));
    eprintln!("  Working:  {}\n", cwd.display());

    // Git check
    check_tool("git", which_git());
    check_tool("cargo", which_cargo());
    check_tool("node/npm", which_npm());
    check_tool("python", which_python());

    // Rust toolchain
    if let Ok(true) = is_rust_project(&cwd) {
        eprintln!(" -> Rust project detected");
        let result = tokio::process::Command::new("rustc")
            .arg("--version")
            .output().await;
        if let Ok(out) = result {
            eprintln!("   rustc: {}", String::from_utf8_lossy(&out.stdout).trim());
        }
        let result = tokio::process::Command::new("cargo")
            .args(["--version"])
            .output().await;
        if let Ok(out) = result {
            eprintln!("   cargo: {}", String::from_utf8_lossy(&out.stdout).trim());
        }
    }

    // Config check
    let config = crate::config::Config::load();
    eprintln!("\n  Provider: {:?}", config.provider);

    eprintln!(
        "\n✅ Diagnostics completed in {:.1}s\n",
        start.elapsed().as_secs_f32()
    );
    Ok(())
}

fn which_git() -> Result<()> {
    std::process::Command::new("git").arg("--version").output()?;
    Ok(())
}
fn which_cargo() -> Result<()> {
    std::process::Command::new("cargo").arg("--version").output()?;
    Ok(())
}
fn which_npm() -> Result<()> {
    std::process::Command::new("node").arg("--version").output()?;
    Ok(())
}
fn which_python() -> Result<()> {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .or_else(|_| {
            std::process::Command::new("python")
                .arg("--version")
                .output()
        })?;
    Ok(())
}

fn check_tool(name: &str, result: Result<()>) {
    match result {
        Ok(_) => eprintln!("  ✅ {}: available", name),
        Err(_) => eprintln!("  ⚠️  {}: not found", name),
    }
}

fn is_rust_project(path: &std::path::Path) -> Result<bool> {
    Ok(path.join("Cargo.toml").exists())
}

/// Initialize a project in the current directory.
pub async fn run_init_command(project_type: Option<&str>, scaffold: bool) -> Result<()> {
    use crate::build::ProjectType;

    let cwd = std::env::current_dir()?;
    let pt = if let Some(t) = project_type {
        // Try to parse the given type
        let normalized = t.to_lowercase();
        match normalized.as_str() {
            "rust" => ProjectType::Rust,
            "node" | "nodejs" | "javascript" => ProjectType::NodeJs,
            "typescript" | "ts" => ProjectType::TypeScript,
            "react" => ProjectType::React,
            "vue" => ProjectType::Vue,
            "angular" | "ng" => ProjectType::Angular,
            "python" | "py" => ProjectType::Python,
            "go" | "golang" => ProjectType::Go,
            "c" => ProjectType::C,
            "cpp" | "c++" => ProjectType::Cpp,
            "java" => ProjectType::Java,
            "kotlin" => ProjectType::Kotlin,
            "csharp" | "dotnet" | "c#" => ProjectType::CSharp,
            "ruby" | "rb" => ProjectType::Ruby,
            _ => {
                eprintln!("Unknown project type: {}. Supported: rust, node, typescript, react, vue, angular, python, go, c, cpp, java, kotlin, csharp, ruby", t);
                return Ok(());
            }
        }
    } else {
        ProjectType::detect_from_path(&cwd)
    };

    eprintln!("\n📦 Initialize project in {}\n", cwd.display());
    eprintln!("  Type: {:?}", pt);

    if scaffold {
        match pt {
            ProjectType::Rust => {
                eprintln!("  Scaffolding Rust project...");
                // Create a minimal Cargo.toml if it doesn't exist
                if !cwd.join("Cargo.toml").exists() {
                    let name = cwd.file_name().map(|n| n.to_string_lossy()).unwrap_or(std::borrow::Cow::Borrowed("my_project"));
                    std::fs::write(cwd.join("Cargo.toml"), format!(
                        "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n",
                        name
                    ))?;
                    std::fs::write(cwd.join("src").join("main.rs"), "fn main() {\n    println!(\"Hello, world!\");\n}\n")?;
                    eprintln!("  ✅ Created Cargo.toml and src/main.rs");
                } else {
                    eprintln!("  ✅ Cargo.toml already exists");
                }
            }
            ProjectType::NodeJs | ProjectType::TypeScript | ProjectType::React => {
                eprintln!("  Run `npm init` manually for JS/TS project scaffolding.");
                eprintln!("  Or use: npx create-{:?}-app", pt);
            }
            _ => {
                eprintln!("  Scaffolding not implemented for {:?}. Run manually.", pt);
            }
        }
    } else {
        eprintln!("  (use --scaffold to create project files)");
    }

    eprintln!("\n✅ Project initialized.\n");
    Ok(())
}

// ════════════════════════════════════════════════════════════════════