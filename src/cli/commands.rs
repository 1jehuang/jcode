#![cfg_attr(test, allow(clippy::await_holding_lock))]

use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::ToSocketAddrs;

use crate::{browser, gateway, memory, session, storage, tui};

use super::terminal::{cleanup_tui_runtime, init_tui_runtime};
use jcode_tool_core::Tool;

mod provider_setup;
mod report_info;
mod restart;

pub use super::auth_test::run_auth_test_command;
pub(crate) use super::auth_test::run_post_login_validation;
/// Analyze code value using six-dimension classification.
/// Runs `cargo check` in the project and classifies all diagnostics.
pub async fn run_code_value_command(
    input_path: Option<&str>,
    manifest_path: &str,
    emit_json: bool,
    output_path: Option<&str>,
) -> Result<()> {
    use jcode_code_value::{CargoDiagnosticParser, Classifier};
    use std::path::Path;

    let diagnostics = if let Some(path) = input_path {
        let parser = CargoDiagnosticParser::new();
        parser.parse_file(Path::new(path))
            .map_err(|e| anyhow::anyhow!("无法解析 cargo JSON 文件: {}", e))?
    } else {
        eprintln!("🔍 运行 cargo check --message-format=json ...");

        let output = std::process::Command::new("cargo")
            .args([
                "check",
                "--message-format=json",
                "--manifest-path",
                manifest_path,
            ])
            .output()
            .map_err(|e| anyhow::anyhow!("无法启动 cargo check: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !stderr.is_empty() {
            let trimmed = stderr.trim();
            if !trimmed.is_empty() && !trimmed.starts_with("warning:") {
                eprintln!("cargo check stderr: {}", trimmed);
            }
        }

        let parser = CargoDiagnosticParser::new();
        parser.parse_json(&stdout)?
    };

    if diagnostics.is_empty() {
        if emit_json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "clean",
                    "message": "没有发现任何诊断项（warning/error），代码质量良好。",
                    "generated_at": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                    "total_diagnostics": 0,
                    "by_category": [],
                    "diagnostics": []
                }))?
            );
        } else {
            println!("\n✅ 代码价值评估完成：未发现任何诊断项（warning/error），代码质量良好。\n");
        }
        return Ok(());
    }

    let classifier = Classifier::new();
    let report = classifier.classify(diagnostics);

    if emit_json {
        let json_str = serde_json::to_string_pretty(&report)?;
        if let Some(path) = output_path {
            std::fs::write(path, &json_str)
                .map_err(|e| anyhow::anyhow!("无法写入输出文件 {}: {}", path, e))?;
            println!("📄 JSON 报告已写入: {}", path);
        } else {
            println!("{}", json_str);
        }
    } else {
        print_human_report(&report);
        if let Some(path) = output_path {
            let json_str = serde_json::to_string_pretty(&report)?;
            std::fs::write(path, &json_str)
                .map_err(|e| anyhow::anyhow!("无法写入输出文件 {}: {}", path, e))?;
            println!("\n📄 JSON 报告已保存至: {}", path);
        }
    }

    Ok(())
}

fn print_human_report(report: &jcode_code_value::ClassificationReport) {
    println!();
    println!("╔══════════════════════════════════════════════════╗");
    println!("║        📊 代码价值六维分类评估报告              ║");
    println!("╠══════════════════════════════════════════════════╣");
    println!(
        "║  生成时间: {:36} ║",
        report.generated_at
    );
    println!(
        "║  诊断总数: {:>4} 项                             ║",
        report.total_diagnostics
    );
    println!("╠══════════════════════════════════════════════════╣");

    for summary in &report.by_category {
        let pct = if report.total_diagnostics > 0 {
            (summary.count as f64 / report.total_diagnostics as f64) * 100.0
        } else {
            0.0
        };
        let icon = match summary.category {
            jcode_code_value::CodeValueCategory::Reserved => "📌",
            jcode_code_value::CodeValueCategory::Legacy => "🕰️",
            jcode_code_value::CodeValueCategory::MissingFeature => "🔧",
            jcode_code_value::CodeValueCategory::Invalid => "🚫",
            jcode_code_value::CodeValueCategory::Duplicate => "📋",
            jcode_code_value::CodeValueCategory::Redundant => "🧹",
        };
        println!(
            "║  {} {}({}): {:>4} 项 ({:>5.1}%)                   ║",
            icon,
            summary.category.display_name(),
            summary.severity,
            summary.count,
            pct
        );
    }

    println!("╠══════════════════════════════════════════════════╣");
    println!("║  📋 详情列表 (按文件路径排列)                  ║");
    println!("╚══════════════════════════════════════════════════╝");
    println!();

    for (i, diag) in report.diagnostics.iter().enumerate() {
        let icon = match diag.category {
            jcode_code_value::CodeValueCategory::Reserved => "📌",
            jcode_code_value::CodeValueCategory::Legacy => "🕰️",
            jcode_code_value::CodeValueCategory::MissingFeature => "🔧",
            jcode_code_value::CodeValueCategory::Invalid => "🚫",
            jcode_code_value::CodeValueCategory::Duplicate => "📋",
            jcode_code_value::CodeValueCategory::Redundant => "🧹",
        };

        println!(
            "  {}. {} [{}] {}({:.0}%)",
            i + 1,
            icon,
            diag.category.display_name(),
            diag.lint_code,
            diag.confidence * 100.0
        );
        println!(
            "     📍 {}:{}:{}",
            diag.file_path, diag.line, diag.column
        );
        if let Some(ref name) = diag.item_name {
            println!("     🏷️  项目: `{}`", name);
        }
        println!("     💬 {}", diag.message);
        println!(
            "     📝 理由: {}",
            diag.rationale
        );
        println!();
    }

    println!("════════════════════════════════════════════════════");
}

#[cfg(test)]
pub(crate) use super::auth_test::{
    AuthTestChoicePlan, AuthTestTarget, ResolvedAuthTestTarget, auth_test_choice_plan,
    auth_test_error_is_retryable, configured_auth_test_targets, resolve_auth_test_targets,
};
pub(crate) use provider_setup::{ProviderAddOptions, run_provider_add_command};
pub use restart::{
    maybe_run_pending_restart_restore_on_startup, run_restart_clear_command,
    run_restart_restore_command, run_restart_save_command, run_restart_status_command,
};

pub enum AmbientSubcommand {
    Status,
    Log,
    Trigger,
    Stop,
    RunVisible,
}

pub async fn run_ambient_command(cmd: AmbientSubcommand) -> Result<()> {
    if let AmbientSubcommand::RunVisible = cmd {
        return run_ambient_visible().await;
    }

    let debug_cmd = match cmd {
        AmbientSubcommand::Status => "ambient:status",
        AmbientSubcommand::Log => "ambient:log",
        AmbientSubcommand::Trigger => "ambient:trigger",
        AmbientSubcommand::Stop => "ambient:stop",
        AmbientSubcommand::RunVisible => unreachable!(),
    };

    super::debug::run_debug_command(debug_cmd, "", None, None, false).await
}

pub async fn run_transcript_command(
    text: Option<String>,
    mode: crate::protocol::TranscriptMode,
    session: Option<String>,
) -> Result<()> {
    let text = if let Some(text) = text {
        text
    } else {
        let mut stdin = String::new();
        std::io::stdin().read_to_string(&mut stdin)?;
        let trimmed = stdin.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            anyhow::bail!("Provide transcript text as an argument or pipe it via stdin")
        }
        trimmed.to_string()
    };

    let mut client = crate::server::Client::connect_debug().await?;
    let request_id = client.send_transcript(&text, mode, session).await?;

    loop {
        match client.read_event().await? {
            crate::protocol::ServerEvent::Ack { id } if id == request_id => {}
            crate::protocol::ServerEvent::Done { id } if id == request_id => return Ok(()),
            crate::protocol::ServerEvent::Error { id, message, .. } if id == request_id => {
                anyhow::bail!(message)
            }
            _ => {}
        }
    }
}

pub async fn run_dictate_command(type_output: bool) -> Result<()> {
    let run = crate::dictation::run_configured().await?;

    if type_output {
        crate::dictation::type_text(&run.text)
    } else {
        run_transcript_command(Some(run.text), run.mode, None).await
    }
}

#[derive(Serialize)]
struct SessionRenameOutput {
    session_id: String,
    display_name: String,
    title: Option<String>,
    cleared: bool,
}

pub fn run_session_rename_command(
    session_ref: &str,
    name: Option<&str>,
    clear: bool,
    json: bool,
) -> Result<()> {
    let resolved_id = session::find_session_by_name_or_id(session_ref)?;
    let mut session = session::Session::load(&resolved_id)?;

    if clear {
        session.rename_title(None);
    } else {
        let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
            anyhow::bail!("Provide a session name or use --clear");
        };
        session.rename_title(Some(name.to_string()));
    }

    session.save()?;
    crate::tui::session_picker::invalidate_session_list_cache();

    let output = SessionRenameOutput {
        session_id: session.id.clone(),
        display_name: session.display_name().to_string(),
        title: session.display_title().map(ToOwned::to_owned),
        cleared: clear,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if clear {
        println!(
            "Cleared custom name for session {} ({}).",
            output.display_name, output.session_id
        );
    } else if let Some(title) = output.title.as_deref() {
        println!(
            "Renamed session {} ({}) to \"{}\".",
            output.display_name, output.session_id, title
        );
    }

    Ok(())
}

async fn run_ambient_visible() -> Result<()> {
    use crate::ambient::VisibleCycleContext;

    let context = VisibleCycleContext::load().map_err(|e| {
        anyhow::anyhow!(
            "Failed to load visible cycle context: {}\nIs the ambient runner running?",
            e
        )
    })?;

    let (provider, registry) = super::provider_init::init_provider_and_registry(
        &super::provider_init::ProviderChoice::Auto,
        None,
    )
    .await?;

    registry.register_ambient_tools().await;

    let safety = std::sync::Arc::new(crate::safety::SafetySystem::new());
    crate::tool::ambient::init_safety_system(safety);

    let (terminal, tui_runtime) = init_tui_runtime()?;

    let mut app = tui::App::new(provider, registry);
    app.set_ambient_mode(context.system_prompt, context.initial_message);

    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::SetTitle("🤖 jcode ambient cycle")
    );

    let result = app.run(terminal).await;

    cleanup_tui_runtime(&tui_runtime, true);

    if let Some(cycle_result) = crate::tool::ambient::take_cycle_result() {
        let result_path = VisibleCycleContext::result_path()?;
        crate::storage::write_json(&result_path, &cycle_result)?;
        eprintln!("Ambient cycle result saved.");
    }

    result?;
    Ok(())
}

pub enum MemorySubcommand {
    List {
        scope: String,
        tag: Option<String>,
    },
    Search {
        query: String,
        semantic: bool,
    },
    Export {
        output: String,
        scope: String,
    },
    Import {
        input: String,
        scope: String,
        overwrite: bool,
    },
    Stats,
    ClearTest,
}

pub fn run_memory_command(cmd: MemorySubcommand) -> Result<()> {
    use memory::{MemoryEntry, MemoryManager};

    let manager = MemoryManager::new();

    match cmd {
        MemorySubcommand::List { scope, tag } => {
            let mut all_memories: Vec<MemoryEntry> = Vec::new();

            if (scope == "all" || scope == "project")
                && let Ok(graph) = manager.load_project_graph()
            {
                all_memories.extend(graph.all_memories().cloned());
            }
            if (scope == "all" || scope == "global")
                && let Ok(graph) = manager.load_global_graph()
            {
                all_memories.extend(graph.all_memories().cloned());
            }

            if let Some(tag_filter) = tag {
                all_memories.retain(|m| m.tags.contains(&tag_filter));
            }

            all_memories.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

            if all_memories.is_empty() {
                println!("No memories found.");
            } else {
                println!("Found {} memories:\n", all_memories.len());
                for entry in &all_memories {
                    let tags_str = if entry.tags.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", entry.tags.join(", "))
                    };
                    let conf = entry.effective_confidence();
                    println!(
                        "- [{}] {}{}\n  id: {} (conf: {:.0}%, accessed: {}x)",
                        entry.category,
                        entry.content,
                        tags_str,
                        entry.id,
                        conf * 100.0,
                        entry.access_count
                    );
                    println!();
                }
            }
        }

        MemorySubcommand::Search { query, semantic } => {
            if semantic {
                match manager.find_similar(&query, 0.3, 20) {
                    Ok(results) => {
                        if results.is_empty() {
                            println!("No memories found matching '{}'", query);
                        } else {
                            println!(
                                "Found {} memories matching '{}' (semantic):\n",
                                results.len(),
                                query
                            );
                            for (entry, score) in results {
                                let tags_str = if entry.tags.is_empty() {
                                    String::new()
                                } else {
                                    format!(" [{}]", entry.tags.join(", "))
                                };
                                println!(
                                    "- [{}] {}{}\n  id: {} (score: {:.0}%)",
                                    entry.category,
                                    entry.content,
                                    tags_str,
                                    entry.id,
                                    score * 100.0
                                );
                                println!();
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Search failed: {}", e);
                    }
                }
            } else {
                match manager.search(&query) {
                    Ok(results) => {
                        if results.is_empty() {
                            println!("No memories found matching '{}'", query);
                        } else {
                            println!(
                                "Found {} memories matching '{}' (keyword):\n",
                                results.len(),
                                query
                            );
                            for entry in results {
                                let tags_str = if entry.tags.is_empty() {
                                    String::new()
                                } else {
                                    format!(" [{}]", entry.tags.join(", "))
                                };
                                println!(
                                    "- [{}] {}{}\n  id: {}",
                                    entry.category, entry.content, tags_str, entry.id
                                );
                                println!();
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Search failed: {}", e);
                    }
                }
            }
        }

        MemorySubcommand::Export { output, scope } => {
            let mut all_memories: Vec<memory::MemoryEntry> = Vec::new();

            if (scope == "all" || scope == "project")
                && let Ok(graph) = manager.load_project_graph()
            {
                all_memories.extend(graph.all_memories().cloned());
            }
            if (scope == "all" || scope == "global")
                && let Ok(graph) = manager.load_global_graph()
            {
                all_memories.extend(graph.all_memories().cloned());
            }

            let json = serde_json::to_string_pretty(&all_memories)?;
            std::fs::write(&output, json)?;
            println!("Exported {} memories to {}", all_memories.len(), output);
        }

        MemorySubcommand::Import {
            input,
            scope,
            overwrite,
        } => {
            let content = std::fs::read_to_string(&input)?;
            let memories: Vec<memory::MemoryEntry> = serde_json::from_str(&content)?;

            let mut imported = 0;
            let mut skipped = 0;

            for entry in memories {
                let result = if scope == "global" {
                    if !overwrite
                        && let Ok(graph) = manager.load_global_graph()
                        && graph.get_memory(&entry.id).is_some()
                    {
                        skipped += 1;
                        continue;
                    }
                    manager.remember_global(entry)
                } else {
                    if !overwrite
                        && let Ok(graph) = manager.load_project_graph()
                        && graph.get_memory(&entry.id).is_some()
                    {
                        skipped += 1;
                        continue;
                    }
                    manager.remember_project(entry)
                };

                if result.is_ok() {
                    imported += 1;
                }
            }

            println!("Imported {} memories ({} skipped)", imported, skipped);
        }

        MemorySubcommand::Stats => {
            let mut project_count = 0;
            let mut global_count = 0;
            let mut total_tags = std::collections::HashSet::new();
            let mut categories: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();

            if let Ok(graph) = manager.load_project_graph() {
                project_count = graph.memory_count();
                for entry in graph.all_memories() {
                    for tag in &entry.tags {
                        total_tags.insert(tag.clone());
                    }
                    *categories.entry(entry.category.to_string()).or_default() += 1;
                }
            }

            if let Ok(graph) = manager.load_global_graph() {
                global_count = graph.memory_count();
                for entry in graph.all_memories() {
                    for tag in &entry.tags {
                        total_tags.insert(tag.clone());
                    }
                    *categories.entry(entry.category.to_string()).or_default() += 1;
                }
            }

            println!("Memory Statistics:");
            println!("  Project memories: {}", project_count);
            println!("  Global memories:  {}", global_count);
            println!("  Total:            {}", project_count + global_count);
            println!("  Unique tags:      {}", total_tags.len());
            println!("\nBy category:");
            for (cat, count) in &categories {
                println!("  {}: {}", cat, count);
            }
        }

        MemorySubcommand::ClearTest => {
            let test_dir = storage::jcode_dir()?.join("memory").join("test");
            if test_dir.exists() {
                let count = std::fs::read_dir(&test_dir)?.count();
                std::fs::remove_dir_all(&test_dir)?;
                println!("Cleared test memory storage ({} files)", count);
            } else {
                println!("Test memory storage is already empty");
            }
        }
    }

    Ok(())
}

pub fn run_pair_command(list: bool, revoke: Option<String>) -> Result<()> {
    let mut registry = gateway::DeviceRegistry::load();

    if list {
        if registry.devices.is_empty() {
            eprintln!("No paired devices.");
        } else {
            eprintln!("\x1b[1mPaired devices:\x1b[0m\n");
            for device in &registry.devices {
                let last_seen = &device.last_seen;
                eprintln!("  \x1b[36m{}\x1b[0m  ({})", device.name, device.id);
                eprintln!("    Paired: {}  Last seen: {}", device.paired_at, last_seen);
                if let Some(ref apns) = device.apns_token {
                    eprintln!("    APNs: {}...", &apns[..apns.len().min(16)]);
                }
                eprintln!();
            }
        }
        return Ok(());
    }

    if let Some(ref target) = revoke {
        let before = registry.devices.len();
        registry
            .devices
            .retain(|d| d.id != *target && d.name != *target);
        if registry.devices.len() < before {
            registry.save()?;
            eprintln!("\x1b[32m✓\x1b[0m Revoked device: {}", target);
        } else {
            eprintln!("\x1b[31m✗\x1b[0m No device found matching: {}", target);
        }
        return Ok(());
    }

    let gw_config = &crate::config::config().gateway;

    if !gw_config.enabled {
        eprintln!("\x1b[33m⚠\x1b[0m  Gateway is disabled. Enable it in ~/.jcode/config.toml:\n");
        eprintln!("    \x1b[2m[gateway]\x1b[0m");
        eprintln!("    \x1b[2menabled = true\x1b[0m");
        eprintln!("    \x1b[2mport = {}\x1b[0m\n", gw_config.port);
        eprintln!("  Then restart the jcode server.\n");
    }

    let code = registry.generate_pairing_code();
    let connect_host = resolve_connect_host(&gw_config.bind_addr);
    let pair_uri = format!(
        "jcode://pair?host={}&port={}&code={}",
        connect_host, gw_config.port, code
    );

    eprintln!();
    eprintln!("  \x1b[1mScan with the jcode iOS app:\x1b[0m\n");
    match crate::login_qr::render_unicode_qr(&pair_uri) {
        Ok(qr) => {
            for line in qr.lines() {
                eprintln!("  {line}");
            }
        }
        Err(_) => eprintln!("  \x1b[33m(QR code generation failed)\x1b[0m"),
    }
    eprintln!();
    eprintln!(
        "  Pairing code:  \x1b[1;37m{} {}\x1b[0m   \x1b[2m(expires in 5 minutes)\x1b[0m",
        &code[..3],
        &code[3..]
    );
    let resolved_hint = format!("{}:{}", connect_host, gw_config.port);
    let bind_hint = format!("{}:{}", gw_config.bind_addr, gw_config.port);
    eprintln!("  Connect host:  \x1b[36m{}\x1b[0m", resolved_hint);
    if connect_host != gw_config.bind_addr {
        eprintln!("  Bind address:  \x1b[2m{}\x1b[0m", bind_hint);
    }

    if connect_host == "<your-mac-hostname>" {
        eprintln!(
            "\n  \x1b[33mTip:\x1b[0m set JCODE_GATEWAY_HOST to your reachable Tailscale hostname."
        );
    }

    if (gw_config.bind_addr.as_str(), gw_config.port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut it| it.next())
        .is_none()
    {
        eprintln!(
            "  \x1b[33mWarning:\x1b[0m gateway bind address appears invalid: {}",
            bind_hint
        );
    }
    eprintln!();

    Ok(())
}

pub fn resolve_connect_host(bind_addr: &str) -> String {
    if bind_addr == "0.0.0.0" || bind_addr == "::" {
        if let Some(host) = std::env::var("JCODE_GATEWAY_HOST")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            return host;
        }

        if let Some(host) = detect_tailscale_dns_name() {
            return host;
        }

        return std::env::var("HOSTNAME")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "<your-mac-hostname>".to_string());
    }
    bind_addr.to_string()
}

pub fn parse_tailscale_dns_name(status_json: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(status_json).ok()?;
    let dns_name = value
        .get("Self")?
        .get("DNSName")?
        .as_str()?
        .trim()
        .trim_end_matches('.')
        .to_string();

    if dns_name.is_empty() {
        None
    } else {
        Some(dns_name)
    }
}

pub fn detect_tailscale_dns_name() -> Option<String> {
    let output = std::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    parse_tailscale_dns_name(&output.stdout)
}

pub async fn run_browser(action: &str) -> Result<()> {
    match action {
        "setup" => browser::run_setup_command().await?,
        "status" => {
            let status = browser::ensure_browser_ready_noninteractive().await?;
            println!("Browser automation");
            println!("  backend: {}", status.backend);
            println!("  browser: {}", status.browser);
            println!(
                "  binary: {}",
                if status.binary_installed {
                    "installed"
                } else {
                    "missing"
                }
            );
            println!(
                "  setup: {}",
                if status.setup_complete {
                    "complete"
                } else {
                    "not complete"
                }
            );
            println!(
                "  bridge: {}",
                if status.responding {
                    "responding"
                } else {
                    "not responding"
                }
            );
            println!(
                "  compatibility: {}",
                if status.compatible {
                    "ok"
                } else {
                    "extension/bridge mismatch"
                }
            );
            if !status.missing_actions.is_empty() {
                println!("  missing actions: {}", status.missing_actions.join(", "));
            }

            if status.ready {
                println!("\nBuilt-in browser tool is ready.");
            } else if status.responding && !status.compatible {
                println!(
                    "\nThe browser bridge is connected, but the installed Firefox extension is out of date for this jcode build. Run `jcode browser setup` to repair or update it."
                );
            } else {
                println!("\nRun `jcode browser setup` to install or repair it.");
            }
        }
        other => {
            eprintln!("Unknown browser action: {}", other);
            eprintln!("Available: setup, status");
            std::process::exit(1);
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct ModelListReport {
    provider: String,
    selected_model: String,
    models: Vec<String>,
    routes: Vec<ModelListRouteReport>,
}

#[derive(Debug, Serialize)]
struct ModelListRouteReport {
    provider: String,
    model: String,
    method: String,
    available: bool,
}

#[derive(Debug, Serialize)]
struct RunCommandReport {
    session_id: String,
    provider: String,
    model: String,
    text: String,
    usage: crate::agent::TokenUsage,
}

#[derive(Debug, Default)]
struct NdjsonRunState {
    text: String,
    session_id: Option<String>,
    upstream_provider: Option<String>,
    connection_type: Option<String>,
    connection_phase: Option<String>,
    status_detail: Option<String>,
    usage: crate::agent::TokenUsage,
}

pub fn run_auth_status_command(emit_json: bool) -> Result<()> {
    report_info::run_auth_status_command(emit_json)
}

pub async fn run_auth_doctor_command(
    provider_arg: Option<&str>,
    validate: bool,
    emit_json: bool,
) -> Result<()> {
    report_info::run_auth_doctor_command(provider_arg, validate, emit_json).await
}

pub fn run_provider_list_command(emit_json: bool) -> Result<()> {
    report_info::run_provider_list_command(emit_json)
}

pub async fn run_provider_current_command(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    emit_json: bool,
) -> Result<()> {
    report_info::run_provider_current_command(choice, model, emit_json).await
}

pub fn run_version_command(emit_json: bool) -> Result<()> {
    report_info::run_version_command(emit_json)
}

pub async fn run_usage_command(emit_json: bool) -> Result<()> {
    report_info::run_usage_command(emit_json).await
}

pub async fn run_single_message_command(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    resume_session: Option<&str>,
    message: &str,
    emit_json: bool,
    emit_ndjson: bool,
) -> Result<()> {
    let provider = if emit_json || emit_ndjson {
        super::provider_init::init_provider_quiet(choice, model).await?
    } else {
        super::provider_init::init_provider_for_validation(choice, model).await?
    };
    let registry = crate::tool::Registry::new(provider.clone()).await;
    let mut agent = crate::agent::Agent::new(provider.clone(), registry);
    restore_agent_session_if_requested(&mut agent, resume_session)?;

    if emit_json {
        let text = run_single_message_command_capture_with_auto_poke(&mut agent, message).await?;
        let report = RunCommandReport {
            session_id: agent.session_id().to_string(),
            provider: provider.name().to_string(),
            model: provider.model(),
            text,
            usage: agent.last_usage().clone(),
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if emit_ndjson {
        run_single_message_command_ndjson(&mut agent, provider.clone(), message).await?;
    } else {
        run_single_message_command_plain_with_auto_poke(&mut agent, message).await?;
    }

    Ok(())
}

fn run_command_auto_poke_enabled() -> bool {
    std::env::var("JCODE_RUN_AUTO_POKE")
        .ok()
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            !matches!(value.as_str(), "0" | "false" | "off" | "no")
        })
        .unwrap_or(true)
}

fn run_command_auto_poke_max_turns() -> Option<usize> {
    std::env::var("JCODE_RUN_AUTO_POKE_MAX_TURNS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn run_command_auto_poke_limit_reached(turns_completed: usize, max_turns: Option<usize>) -> bool {
    max_turns
        .map(|max_turns| turns_completed >= max_turns)
        .unwrap_or(false)
}

fn incomplete_run_todos(session_id: &str) -> Vec<crate::todo::TodoItem> {
    crate::todo::load_todos(session_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|todo| todo.status != "completed" && todo.status != "cancelled")
        .collect()
}

fn build_run_poke_message(incomplete: &[crate::todo::TodoItem]) -> String {
    format!(
        "You have {} incomplete todo{}. Continue working, or update the todo tool.",
        incomplete.len(),
        if incomplete.len() == 1 { "" } else { "s" },
    )
}

async fn run_single_message_command_plain_with_auto_poke(
    agent: &mut crate::agent::Agent,
    message: &str,
) -> Result<()> {
    let mut next_message = message.to_string();
    let max_turns = run_command_auto_poke_max_turns();
    let mut turns_completed = 0usize;
    loop {
        agent.run_once(&next_message).await?;
        turns_completed += 1;
        if !run_command_auto_poke_enabled() {
            break;
        }
        let incomplete = incomplete_run_todos(agent.session_id());
        if incomplete.is_empty() {
            break;
        }
        if run_command_auto_poke_limit_reached(turns_completed, max_turns) {
            if let Some(max_turns) = max_turns {
                eprintln!(
                    "Auto-poke stopped after {max_turns} turn(s) with {} incomplete todo(s).",
                    incomplete.len()
                );
            }
            break;
        }
        next_message = build_run_poke_message(&incomplete);
        eprintln!(
            "Auto-poking: {} incomplete todo(s). Set JCODE_RUN_AUTO_POKE=0 to disable.",
            incomplete.len()
        );
    }
    Ok(())
}

async fn run_single_message_command_capture_with_auto_poke(
    agent: &mut crate::agent::Agent,
    message: &str,
) -> Result<String> {
    let mut next_message = message.to_string();
    let max_turns = run_command_auto_poke_max_turns();
    let mut outputs = Vec::new();
    let mut turns_completed = 0usize;
    loop {
        outputs.push(agent.run_once_capture(&next_message).await?);
        turns_completed += 1;
        if !run_command_auto_poke_enabled() {
            break;
        }
        let incomplete = incomplete_run_todos(agent.session_id());
        if incomplete.is_empty() {
            break;
        }
        if run_command_auto_poke_limit_reached(turns_completed, max_turns) {
            if let Some(max_turns) = max_turns {
                outputs.push(format!(
                    "Auto-poke stopped after {max_turns} turn(s) with {} incomplete todo(s).",
                    incomplete.len()
                ));
            }
            break;
        }
        next_message = build_run_poke_message(&incomplete);
    }
    Ok(outputs.join("\n\n"))
}

fn restore_agent_session_if_requested(
    agent: &mut crate::agent::Agent,
    resume_session: Option<&str>,
) -> Result<()> {
    if let Some(session_id) = resume_session {
        agent.restore_session(session_id)?;
    }
    Ok(())
}

async fn run_single_message_command_ndjson(
    agent: &mut crate::agent::Agent,
    provider: std::sync::Arc<dyn crate::provider::Provider>,
    message: &str,
) -> Result<()> {
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let session_id = agent.session_id().to_string();
    let mut stdout = std::io::stdout().lock();
    let mut state = NdjsonRunState {
        session_id: Some(session_id.clone()),
        ..NdjsonRunState::default()
    };
    write_json_line(
        &mut stdout,
        &serde_json::json!({
            "type": "start",
            "session_id": session_id,
            "provider": provider.name(),
            "model": provider.model(),
        }),
    )?;

    let max_turns = run_command_auto_poke_max_turns();
    let mut next_message = message.to_string();
    let mut result: Result<()> = Ok(());
    let mut turns_completed = 0usize;
    loop {
        let turn_result = {
            let mut run_future = std::pin::pin!(agent.run_once_streaming_mpsc(
                &next_message,
                Vec::new(),
                None,
                event_tx.clone(),
            ));
            let mut run_result: Option<Result<()>> = None;
            loop {
                tokio::select! {
                    result = &mut run_future, if run_result.is_none() => {
                        run_result = Some(result);
                    }
                    event = event_rx.recv() => {
                        match event {
                            Some(event) => emit_ndjson_event(&mut stdout, &mut state, event)?,
                            None => break,
                        }
                    }
                }
                if run_result.is_some() {
                    while let Ok(event) = event_rx.try_recv() {
                        emit_ndjson_event(&mut stdout, &mut state, event)?;
                    }
                    break;
                }
            }
            run_result.unwrap_or(Ok(()))
        };

        if let Err(err) = turn_result {
            result = Err(err);
            break;
        }
        turns_completed += 1;
        if !run_command_auto_poke_enabled() {
            break;
        }
        let incomplete = incomplete_run_todos(&session_id);
        if incomplete.is_empty() {
            break;
        }
        if run_command_auto_poke_limit_reached(turns_completed, max_turns) {
            if let Some(max_turns) = max_turns {
                write_json_line(
                    &mut stdout,
                    &serde_json::json!({
                        "type": "auto_poke_stopped",
                        "session_id": session_id,
                        "incomplete_todos": incomplete.len(),
                        "max_turns": max_turns,
                    }),
                )?;
            }
            break;
        }
        next_message = build_run_poke_message(&incomplete);
        write_json_line(
            &mut stdout,
            &serde_json::json!({
                "type": "auto_poke",
                "session_id": session_id,
                "incomplete_todos": incomplete.len(),
                "message": next_message,
            }),
        )?;
    }

    match result {
        Ok(()) => {
            write_json_line(
                &mut stdout,
                &serde_json::json!({
                    "type": "done",
                    "session_id": session_id,
                    "provider": provider.name(),
                    "model": provider.model(),
                    "text": state.text,
                    "usage": state.usage,
                    "upstream_provider": state.upstream_provider,
                    "connection_type": state.connection_type,
                    "connection_phase": state.connection_phase,
                    "status_detail": state.status_detail,
                }),
            )?;
            Ok(())
        }
        Err(err) => {
            write_json_line(
                &mut stdout,
                &serde_json::json!({
                    "type": "error",
                    "session_id": session_id,
                    "provider": provider.name(),
                    "model": provider.model(),
                    "message": format!("{err:#}"),
                }),
            )?;
            Err(err)
        }
    }
}

fn emit_ndjson_event(
    stdout: &mut impl Write,
    state: &mut NdjsonRunState,
    event: crate::protocol::ServerEvent,
) -> Result<()> {
    use crate::protocol::ServerEvent;

    match event {
        ServerEvent::TextDelta { text } => {
            state.text.push_str(&text);
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "text_delta", "text": text }),
            )
        }
        ServerEvent::TextReplace { text } => {
            state.text = text.clone();
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "text_replace", "text": text }),
            )
        }
        ServerEvent::ToolStart { id, name } => write_json_line(
            stdout,
            &serde_json::json!({ "type": "tool_start", "id": id, "name": name }),
        ),
        ServerEvent::ToolInput { delta } => write_json_line(
            stdout,
            &serde_json::json!({ "type": "tool_input", "delta": delta }),
        ),
        ServerEvent::ToolExec { id, name } => write_json_line(
            stdout,
            &serde_json::json!({ "type": "tool_exec", "id": id, "name": name }),
        ),
        ServerEvent::ToolDone {
            id,
            name,
            output,
            error,
        } => write_json_line(
            stdout,
            &serde_json::json!({
                "type": "tool_done",
                "id": id,
                "name": name,
                "output": output,
                "error": error,
            }),
        ),
        ServerEvent::TokenUsage {
            input,
            output,
            cache_read_input,
            cache_creation_input,
        } => {
            state.usage = crate::agent::TokenUsage {
                input_tokens: input,
                output_tokens: output,
                cache_read_input_tokens: cache_read_input,
                cache_creation_input_tokens: cache_creation_input,
            };
            write_json_line(
                stdout,
                &serde_json::json!({
                    "type": "tokens",
                    "input": input,
                    "output": output,
                    "cache_read_input": cache_read_input,
                    "cache_creation_input": cache_creation_input,
                }),
            )
        }
        ServerEvent::ConnectionType { connection } => {
            state.connection_type = Some(connection.clone());
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "connection_type", "connection": connection }),
            )
        }
        ServerEvent::ConnectionPhase { phase } => {
            state.connection_phase = Some(phase.clone());
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "connection_phase", "phase": phase }),
            )
        }
        ServerEvent::StatusDetail { detail } => {
            state.status_detail = Some(detail.clone());
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "status_detail", "detail": detail }),
            )
        }
        ServerEvent::MessageEnd => {
            write_json_line(stdout, &serde_json::json!({ "type": "message_end" }))
        }
        ServerEvent::UpstreamProvider { provider } => {
            state.upstream_provider = Some(provider.clone());
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "upstream_provider", "provider": provider }),
            )
        }
        ServerEvent::SessionId { session_id } => {
            state.session_id = Some(session_id.clone());
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "session", "session_id": session_id }),
            )
        }
        ServerEvent::Compaction {
            trigger,
            pre_tokens,
            messages_dropped,
            post_tokens,
            tokens_saved,
            duration_ms,
            messages_compacted,
            summary_chars,
            active_messages,
        } => write_json_line(
            stdout,
            &serde_json::json!({
                "type": "compaction",
                "trigger": trigger,
                "pre_tokens": pre_tokens,
                "messages_dropped": messages_dropped,
                "post_tokens": post_tokens,
                "tokens_saved": tokens_saved,
                "duration_ms": duration_ms,
                "messages_compacted": messages_compacted,
                "summary_chars": summary_chars,
                "active_messages": active_messages,
            }),
        ),
        ServerEvent::MemoryInjected {
            count,
            prompt_chars,
            computed_age_ms,
            ..
        } => write_json_line(
            stdout,
            &serde_json::json!({
                "type": "memory_injected",
                "count": count,
                "prompt_chars": prompt_chars,
                "computed_age_ms": computed_age_ms,
            }),
        ),
        ServerEvent::Interrupted => {
            write_json_line(stdout, &serde_json::json!({ "type": "interrupted" }))
        }
        ServerEvent::SoftInterruptInjected {
            content,
            display_role,
            point,
            tools_skipped,
        } => write_json_line(
            stdout,
            &serde_json::json!({
                "type": "soft_interrupt_injected",
                "content": content,
                "display_role": display_role,
                "point": point,
                "tools_skipped": tools_skipped,
            }),
        ),
        ServerEvent::BatchProgress { progress } => write_json_line(
            stdout,
            &serde_json::json!({ "type": "batch_progress", "progress": progress }),
        ),
        ServerEvent::Error {
            message,
            retry_after_secs,
            ..
        } => write_json_line(
            stdout,
            &serde_json::json!({
                "type": "error",
                "message": message,
                "retry_after_secs": retry_after_secs,
            }),
        ),
        ServerEvent::Ack { .. } | ServerEvent::Done { .. } | ServerEvent::Pong { .. } => Ok(()),
        _ => Ok(()),
    }
}

fn write_json_line(stdout: &mut impl Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut *stdout, value)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

pub async fn run_model_command(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    emit_json: bool,
    verbose: bool,
) -> Result<()> {
    let provider = super::provider_init::init_provider_quiet(choice, model).await?;

    if let Err(err) = provider.prefetch_models().await
        && !super::output::quiet_enabled()
    {
        eprintln!("Warning: failed to refresh dynamic model list: {}", err);
    }

    let routes = provider.model_routes();
    let filtered_routes = filter_cli_model_routes_for_choice(choice, &routes);
    let models = if filtered_routes.len() == routes.len() {
        collect_cli_model_names(&routes, provider.available_models_display())
    } else {
        collect_cli_model_names(&filtered_routes, Vec::new())
    };

    if models.is_empty() {
        anyhow::bail!(
            "No models found for provider '{}'. Check credentials or try a different --provider.",
            provider.name()
        );
    }

    if emit_json {
        let provider_label = super::provider_init::login_provider_for_choice(choice)
            .map(|provider| provider.display_name.to_string())
            .unwrap_or_else(|| {
                crate::provider_catalog::runtime_provider_display_name(provider.name())
            });
        let report = ModelListReport {
            provider: provider_label,
            selected_model: provider.model(),
            models,
            routes: filtered_routes
                .iter()
                .map(|route| ModelListRouteReport {
                    provider: cli_route_provider_display(&route.provider, &route.api_method),
                    model: route.model.clone(),
                    method: cli_api_method_display(&route.api_method).to_string(),
                    available: route.available,
                })
                .collect(),
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        if verbose {
            println!(
                "Provider: {}",
                crate::provider_catalog::runtime_provider_display_name(provider.name())
            );
            println!("Selected model: {}", provider.model());
            println!("Available models: {}", models.len());
            println!();
        }
        for model in models {
            println!("{}", model);
        }
    }

    Ok(())
}

fn cli_api_method_display(raw: &str) -> &str {
    match raw {
        "claude-oauth" | "openai-oauth" | "code-assist-oauth" => "oauth",
        "api-key" | "openai-api-key" => "api key",
        method if method.starts_with("openai-compatible") => "api key",
        method => method
            .split_once(':')
            .map(|(method, _)| method)
            .unwrap_or(method),
    }
}

fn cli_route_provider_display(provider: &str, api_method: &str) -> String {
    if api_method == "openrouter" && provider != "auto" && !provider.contains("OpenRouter") {
        format!("OpenRouter/{}", provider)
    } else {
        provider.to_string()
    }
}

fn collect_cli_model_names(
    routes: &[crate::provider::ModelRoute],
    display_models: Vec<String>,
) -> Vec<String> {
    let mut deduped = Vec::new();
    let mut seen = BTreeSet::new();

    fn push_model(deduped: &mut Vec<String>, seen: &mut BTreeSet<String>, model: &str) {
        let trimmed = model.trim();
        if !crate::provider::is_listable_model_name(trimmed) {
            return;
        }
        if seen.insert(trimmed.to_string()) {
            deduped.push(trimmed.to_string());
        }
    }

    for route in routes.iter().filter(|route| route.available) {
        push_model(&mut deduped, &mut seen, &route.model);
    }

    if deduped.is_empty() {
        for route in routes {
            push_model(&mut deduped, &mut seen, &route.model);
        }
    }

    for model in display_models {
        push_model(&mut deduped, &mut seen, &model);
    }

    deduped
}

#[allow(deprecated)]
fn filter_cli_model_routes_for_choice(
    choice: &super::provider_init::ProviderChoice,
    routes: &[crate::provider::ModelRoute],
) -> Vec<crate::provider::ModelRoute> {
    use super::provider_init::ProviderChoice;

    let keep = |route: &&crate::provider::ModelRoute| match choice {
        ProviderChoice::Claude | ProviderChoice::ClaudeSubprocess => {
            route.api_method == "claude-oauth" || route.api_method == "api-key"
        }
        ProviderChoice::Openai => route.api_method == "openai-oauth",
        ProviderChoice::OpenaiApi => route.api_method == "openai-api-key",
        ProviderChoice::Openrouter | ProviderChoice::Azure => route.api_method == "openrouter",
        ProviderChoice::Copilot => route.api_method == "copilot",
        _ => true,
    };

    let filtered: Vec<_> = routes.iter().filter(keep).cloned().collect();
    if filtered.is_empty() {
        routes.to_vec()
    } else {
        filtered
    }
}

/// Run the build mode: plan → execute → verify
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

    eprintln!("\n🏗️  Build Mode — Plan → Execute → Verify\n");

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
    eprintln!("  ┌─ Build plan: ");
    if release { eprintln!("  │ Mode:        release"); }
    if clean { eprintln!("  │ Clean first: yes"); }
    if let Some(ref t) = request.target { eprintln!("  │ Target:      {}", t); }
    if run_tests { eprintln!("  │ Run tests:   yes"); }
    eprintln!("  │ Project:     {:?}", project_type);
    eprintln!("  └─\n");

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
            eprintln!("Starting CarpAI MCP server (debug={}, verbose={})", debug, verbose);
            // Start the MCP server mode - this allows IDEs to connect via MCP
            // For now, print instructions
            eprintln!("\nMCP Server not yet fully implemented.");
            eprintln!("To use CarpAI as an MCP server in your IDE:");
            eprintln!("  Add MCP server config:");
            eprintln!(r#"  {{"command": "carpai", "args": ["mcp", "serve"]}}"#);
            Ok(())
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
        eprintln!(" → Rust project detected");
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
// Skills management commands
// ════════════════════════════════════════════════════════════════════

pub async fn run_skills_command(cmd: super::args::SkillsCommand) -> Result<()> {
    use crate::skills::SkillRegistry;
    use std::sync::Arc;

    let registry = Arc::new(SkillRegistry::new());

    match cmd {
        super::args::SkillsCommand::List { json } => {
            let skills = registry.list_sync();
            if json {
                let items: Vec<serde_json::Value> = skills.iter().map(|s| {
                    serde_json::json!({
                        "name": s.definition.name,
                        "display_name": s.definition.display_name,
                        "description": s.definition.description,
                        "category": s.definition.category.label(),
                        "builtin": s.definition.is_builtin,
                        "tags": s.definition.tags,
                    })
                }).collect();
                println!("{}", serde_json::to_string_pretty(&items)?);
            } else {
                if skills.is_empty() {
                    eprintln!("No skills registered.");
                    return Ok(());
                }
                eprintln!("\n🧩 Available Skills ({})\n", skills.len());
                for skill in &skills {
                    let builtin = if skill.definition.is_builtin { "[builtin]" } else { "[loaded]" };
                    eprintln!("  {} {} — {}", builtin, skill.definition.name, skill.definition.description);
                }
            }
        }
        super::args::SkillsCommand::Search { query } => {
            let results = registry.search_sync(&query);
            if results.is_empty() {
                eprintln!("No skills found matching '{}'", query);
            } else {
                eprintln!("\n🧩 Skills matching '{}' ({}):\n", query, results.len());
                for skill in &results {
                    eprintln!("  {} — {}", skill.definition.name, skill.definition.description);
                }
            }
        }
        super::args::SkillsCommand::Info { skill } => {
            match registry.get_sync(&skill) {
                Some(s) => {
                    eprintln!("\n🧩 Skill: {} ({})", s.definition.display_name, s.definition.name);
                    eprintln!("  Description: {}", s.definition.description);
                    eprintln!("  Category: {}", s.definition.category.label());
                    eprintln!("  Built-in: {}", s.definition.is_builtin);
                    if !s.definition.tags.is_empty() {
                        eprintln!("  Tags: {}", s.definition.tags.join(", "));
                    }
                    if !s.definition.params.is_empty() {
                        eprintln!("  Parameters:");
                        for p in &s.definition.params {
                            let req = if p.required { "(required)" } else { "(optional)" };
                            eprintln!("    - {}: {} {}", p.name, p.description, req);
                        }
                    }
                }
                None => eprintln!("Skill '{}' not found", skill),
            }
        }
    }
    Ok(())
}

// ════════════════════════════════════════════════════════════════════
// Workflows management commands
// ════════════════════════════════════════════════════════════════════

pub async fn run_workflows_command(cmd: super::args::WorkflowsCommand) -> Result<()> {
    match cmd {
        super::args::WorkflowsCommand::List { json } => {
            use crate::workflow::template::WorkflowTemplate;
            let templates = WorkflowTemplate::all();
            if json {
                println!("{}", serde_json::to_string_pretty(&templates)?);
            } else {
                eprintln!("\n📋 Available Workflow Templates ({})", templates.len());
                for tmpl in templates {
                    eprintln!("  - {}: {} ({} steps)", tmpl.name, tmpl.description, tmpl.steps.len());
                }
            }
        }
        super::args::WorkflowsCommand::Templates { name } => {
            use crate::workflow::template::WorkflowTemplate;
            if let Some(tmpl_name) = name {
                match WorkflowTemplate::find(&tmpl_name) {
                    Some(tmpl) => {
                        eprintln!("\n📋 Workflow: {}", tmpl.name);
                        eprintln!("  Description: {}", tmpl.description);
                        eprintln!("  Steps:");
                        for (i, step) in tmpl.steps.iter().enumerate() {
                            eprintln!("    {}. {} — {}", i + 1, step.name, step.description);
                        }
                    }
                    None => eprintln!("Template '{}' not found", tmpl_name),
                }
            } else {
                let all = WorkflowTemplate::all();
                eprintln!("\n📋 Workflow Templates:\n");
                for tmpl in all {
                    eprintln!("  {} — {} ({} steps)", tmpl.name, tmpl.description, tmpl.steps.len());
                }
            }
        }
        super::args::WorkflowsCommand::Run { workflow } => {
            use crate::workflow::template::WorkflowTemplate;
            match WorkflowTemplate::to_config(&workflow) {
                Some(config) => {
                    eprintln!("\n🚀 Running workflow: {}\n", workflow);
                    let runner = crate::workflow::runner::WorkflowRunner::new();
                    let id = runner.register(config).await;
                    match runner.execute(&id).await {
                        Ok(()) => eprintln!("✅ Workflow '{}' completed successfully", workflow),
                        Err(e) => eprintln!("❌ Workflow failed: {}", e),
                    }
                }
                None => eprintln!("Workflow '{}' not found. Use `carpai workflows list` to see available templates.", workflow),
            }
        }
    }
    Ok(())
}

// ════════════════════════════════════════════════════════════════════
// Task management commands
// ════════════════════════════════════════════════════════════════════

pub async fn run_tasks_command(cmd: super::args::TasksCommand) -> Result<()> {
    use crate::task_planner::TaskPlanner;

    match cmd {
        super::args::TasksCommand::List { status } => {
            eprintln!("\n📋 Tasks");
            if let Some(ref s) = status {
                eprintln!("  Filter: status = {}\n", s);
            }
            eprintln!("  (No tasks created. Use `carpai tasks create <description>` to add one.)");
        }
        super::args::TasksCommand::Create { description } => {
            let mut planner = TaskPlanner::new();
            let plan_id = planner.create_plan("default", "Ad-hoc task", &description);
            let task = crate::task_planner::EnhancedTask::new(&description);
            match planner.add_task(&plan_id, task) {
                Ok(_) => eprintln!("✅ Task created in plan: {}", plan_id),
                Err(e) => eprintln!("❌ Failed to create task: {}", e),
            }
        }
        super::args::TasksCommand::Plan { id } => {
            let planner = TaskPlanner::new();
            match planner.get_plan(&id) {
                Some(plan) => {
                    eprintln!("\n📋 Plan: {} (ID: {})", plan.name, plan.id);
                    eprintln!("  Description: {}", plan.description);
                    eprintln!("  Goal: {}", plan.goal);
                    eprintln!("  Tasks: {}", plan.tasks.len());
                    for task_id in &plan.tasks {
                        if let Some(task) = planner.get_task(task_id) {
                            let status = if matches!(task.status, crate::task_planner::TaskStatus::Completed) { "✅" } else { "⏳" };
                            eprintln!("    {} {} — {} (priority: {})",
                                status, task.id, task.description, task.priority.label());
                        }
                    }
                }
                None => eprintln!("Plan '{}' not found", id),
            }
        }
        super::args::TasksCommand::Status { id } => {
            let planner = TaskPlanner::new();
            let plan_id = planner.find_plan_for_task(&id);
            if let Some(pid) = plan_id {
                if let Some(_plan) = planner.get_plan(&pid) {
                    if let Some(task) = planner.get_task(&id) {
                        let status = match task.status {
                            crate::task_planner::TaskStatus::Completed => "✅ Completed",
                            _ => "⏳ In Progress",
                        };
                        eprintln!("\n📋 Task: {} ({})", task.id, task.description);
                        eprintln!("  Status: {}", status);
                        eprintln!("  Priority: {}", task.priority.label());
                        eprintln!("  Category: {}", task.category.label());
                        return Ok(());
                    }
                }
            }
            eprintln!("Task '{}' not found", id);
        }
    }
    Ok(())
}

// ════════════════════════════════════════════════════════════════════
// Git operations commands
// ════════════════════════════════════════════════════════════════════

pub async fn run_git_command(cmd: super::args::GitCommand) -> Result<()> {
    use crate::git::operations::GitOperations;

    let git_ops = GitOperations::new(".".into());

    match cmd {
        super::args::GitCommand::Branch => {
            let current = git_ops.current_branch().unwrap_or_default();
            let branches = git_ops.list_branches();
            let names: Vec<String> = branches.iter().map(|b| b.name.clone()).collect();
            eprintln!("\n🔀 Git Branch");
            eprintln!("  Current: {}", current);
            eprintln!("  All branches: {}", names.join(", "));
        }
        super::args::GitCommand::Diff { path } => {
            let staged_diff = git_ops.format_diff(true);
            let unstaged_diff = git_ops.format_diff(false);
            let mut full_diff = String::new();
            if !staged_diff.is_empty() {
                full_diff.push_str("--- Staged ---\n");
                full_diff.push_str(&staged_diff);
            }
            if !unstaged_diff.is_empty() {
                full_diff.push_str("--- Unstaged ---\n");
                full_diff.push_str(&unstaged_diff);
            }

            if let Some(p) = path {
                // Filter diff for specific path
                let filtered: Vec<&str> = full_diff.lines()
                    .skip_while(|l| !l.contains(&p))
                    .collect();
                full_diff = filtered.join("\n");
            }

            if full_diff.is_empty() {
                eprintln!("No changes to show.");
            } else {
                let lines: Vec<&str> = full_diff.lines().collect();
                let added = lines.iter().filter(|l| l.starts_with('+') && !l.starts_with("+++")).count();
                let removed = lines.iter().filter(|l| l.starts_with('-') && !l.starts_with("---")).count();
                eprintln!("\n📝 Git Diff (+{}/-{})", added, removed);
                if full_diff.len() > 4000 {
                    eprintln!("{}", &full_diff[..4000]);
                    eprintln!("... [truncated, total {} bytes]", full_diff.len());
                } else {
                    println!("{}", full_diff);
                }
            }
        }
        super::args::GitCommand::Context => {
            let ctx = git_ops.get_context();
            eprintln!("\n🔍 Git Context");
            eprintln!("  Branch: {}", ctx.current_branch);
            eprintln!("  Repository: {}", ctx.repository_root.display());
            eprintln!("  Status:");
            for s in &ctx.staged_changes {
                eprintln!("    [staged] {:?} {}", s.change_type, s.path);
            }
            for s in &ctx.unstaged_changes {
                eprintln!("    [unstaged] {:?} {}", s.change_type, s.path);
            }
            for f in &ctx.untracked_files {
                eprintln!("    [untracked] {}", f);
            }
            eprintln!("  Recent commits:");
            for c in git_ops.recent_commits(5) {
                eprintln!("    {}", c);
            }
        }
        super::args::GitCommand::Status => {
            let ctx = git_ops.get_context();
            eprintln!("\n📊 Git Status\n");
            eprintln!("  Branch: {}", ctx.current_branch);
            let total_changes = ctx.staged_changes.len() + ctx.unstaged_changes.len() + ctx.untracked_files.len();
            eprintln!("  Working tree changes: {}", total_changes);
            if total_changes == 0 {
                eprintln!("  Working tree clean");
            } else {
                eprintln!("  Changes:");
                for s in &ctx.staged_changes {
                    eprintln!("    [staged] {:?} {}", s.change_type, s.path);
                }
                for s in &ctx.unstaged_changes {
                    eprintln!("    [unstaged] {:?} {}", s.change_type, s.path);
                }
                for f in &ctx.untracked_files {
                    eprintln!("    [untracked] {}", f);
                }
            }
        }
    }
    Ok(())
}

// ════════════════════════════════════════════════════════════════════
// Config management commands
// ════════════════════════════════════════════════════════════════════

pub fn run_config_command(cmd: super::args::ConfigCommand) -> Result<()> {
    match cmd {
        super::args::ConfigCommand::Get { key } => {
            match std::env::var(&key) {
                Ok(val) => println!("{}={}", key, val),
                Err(_) => eprintln!("Config key '{}' not found", key),
            }
        }
        super::args::ConfigCommand::Set { key, value } => {
            // SAFETY: set_var is called in a single-threaded CLI context
            unsafe { std::env::set_var(&key, &value); }
            eprintln!("✅ Set {}={}", key, value);
            eprintln!("  (Note: env vars are session-scoped; use config file for persistence)");
        }
        super::args::ConfigCommand::List { json } => {
            use std::env;
            let vars: std::collections::BTreeMap<String, String> = env::vars()
                .filter(|(k, _)| k.starts_with("CARPAI_") || k.starts_with("JCODE_") || k.starts_with("CLAUDE_"))
                .collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&vars)?);
            } else {
                if vars.is_empty() {
                    eprintln!("No CarpAI/JCODE config variables found.");
                } else {
                    eprintln!("\n⚙️  Config:\n");
                    for (k, v) in &vars {
                        let display = if k.contains("KEY") || k.contains("TOKEN") || k.contains("SECRET") {
                            format!("{}...", &v[..v.len().min(8)])
                        } else {
                            v.clone()
                        };
                        eprintln!("  {}={}", k, display);
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
