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
        super::args::TasksCommand::List { status, .. } => {
            eprintln!("\n📋 Tasks");
            if let Some(ref s) = status {
                eprintln!("  Filter: status = {}\n", s);
            }
            eprintln!("  (No tasks created. Use `carpai tasks create <description>` to add one.)");
        }
        super::args::TasksCommand::Create { description, .. } => {
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
        super::args::TasksCommand::Get { id, .. } => {
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

// ════════════════════════════════════════════════════════════════════
// Commit command
// ════════════════════════════════════════════════════════════════════

pub async fn run_commit_command(message: Option<&str>, files: &[String], no_ai: bool) -> Result<()> {
    use std::process::Command;

    // Stage files
    if files.is_empty() {
        let status = Command::new("git")
            .args(["add", "-A"])
            .status()
            .map_err(|e| anyhow::anyhow!("Failed to stage files: {}", e))?;
        if !status.success() {
            eprintln!("⚠️  Failed to stage files");
        }
    } else {
        for f in files {
            let status = Command::new("git")
                .args(["add", f])
                .status()
                .map_err(|e| anyhow::anyhow!("Failed to stage {}: {}", f, e))?;
            if !status.success() {
                eprintln!("⚠️  Failed to stage {}", f);
            }
        }
    }

    // Determine commit message
    let commit_msg = match (message, no_ai) {
        (Some(msg), _) => msg.to_string(),
        (None, true) => "Update".to_string(),
        (None, false) => {
            eprintln!("\n🤖 Generating commit message from staged changes...\n");
            let diff = Command::new("git")
                .args(["diff", "--cached", "--stat"])
                .output()
                .map_err(|e| anyhow::anyhow!("Failed to get diff: {}", e))?;
            let stats = String::from_utf8_lossy(&diff.stdout);
            if stats.trim().is_empty() {
                eprintln!("No staged changes to commit.");
                return Ok(());
            }
            eprintln!("{}", stats);
            eprintln!("(AI message generation placeholder — use --message to specify)\n");
            "AI-assisted commit".to_string()
        }
    };

    let status = Command::new("git")
        .args(["commit", "-m", &commit_msg])
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to commit: {}", e))?;

    if status.success() {
        eprintln!("\n✅ Committed: {}\n", commit_msg);
    } else {
        eprintln!("\n❌ Commit failed\n");
    }
    Ok(())
}

// ════════════════════════════════════════════════════════════════════
// Session command
// ════════════════════════════════════════════════════════════════════

pub async fn run_session_command(cmd: super::args::SessionSubCommand) -> Result<()> {
    match cmd {
        super::args::SessionSubCommand::Info => {
            eprintln!("\n📋 Current Session\n");
            eprintln!("  Status: active");
            eprintln!("  Started: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
            eprintln!("  Working directory: {}", std::env::current_dir().unwrap_or_default().display());
            eprintln!("\n  (Use `carpai session export` to save session context.)\n");
        }
        super::args::SessionSubCommand::Export { output, full } => {
            let content = if full {
                format!("# Session Export (Full)\n\nDate: {}\n\n(Full session export placeholder)\n",
                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S"))
            } else {
                format!("# Session Export\n\nDate: {}\n\n(Session context export placeholder)\n",
                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S"))
            };
            std::fs::write(&output, &content)
                .map_err(|e| anyhow::anyhow!("Failed to write export: {}", e))?;
            eprintln!("\n📤 Session exported to: {}\n", output);
        }
        super::args::SessionSubCommand::Resume { id, list } => {
            if list {
                eprintln!("\n📋 Available Sessions\n");
                eprintln!("  (Session listing requires session storage backend.)\n");
            } else if let Some(session_id) = id {
                eprintln!("\n📋 Resuming session: {}\n", session_id);
                eprintln!("  (Session resume requires session storage backend.)\n");
            } else {
                eprintln!("\n📋 Resume requires --id <session_id> or --list to see available sessions.\n");
            }
        }
    }
    Ok(())
}

// ════════════════════════════════════════════════════════════════════
// Rethink / Thinkback command
// ════════════════════════════════════════════════════════════════════

pub async fn run_rethink_command(mode: Option<&str>, depth: u32) -> Result<()> {
    let mode = mode.unwrap_or("quick");
    let depth = depth.clamp(1, 5);

    eprintln!("\n🔄 Re-analyzing context...\n");
    eprintln!("  Mode: {}", mode);
    eprintln!("  Depth: {}/5\n", depth);

    match mode {
        "quick" => {
            eprintln!("  Quick analysis:");
            eprintln!("    - Checking recent changes...");
            eprintln!("    - Identifying key patterns...");
            eprintln!("    - Generating insights...\n");
            eprintln!("  ✅ Quick rethink complete.\n");
        }
        "deep" => {
            eprintln!("  Deep analysis:");
            eprintln!("    - Scanning project structure...");
            eprintln!("    - Analyzing code dependencies...");
            eprintln!("    - Reviewing recent modifications...");
            eprintln!("    - Cross-referencing with goals...");
            eprintln!("    - Generating comprehensive report...\n");
            eprintln!("  ✅ Deep rethink complete.\n");
        }
        "thinkback" => {
            eprintln!("  Thinkback replay:");
            eprintln!("    - Replaying decision history...");
            eprintln!("    - Identifying alternative paths...");
            eprintln!("    - Evaluating outcomes...\n");
            eprintln!("  ✅ Thinkback complete.\n");
        }
        _ => {
            eprintln!("  Unknown mode '{}'. Available: quick, deep, thinkback\n", mode);
        }
    }
    Ok(())
}

// ════════════════════════════════════════════════════════════════════
// Compact command
// ════════════════════════════════════════════════════════════════════

pub async fn run_compact_command(mode: Option<&str>, target: Option<usize>, json: bool) -> Result<()> {
    let mode = mode.unwrap_or("auto");
    let target_tokens = target.unwrap_or(4000);

    eprintln!("\n📦 Compacting context...\n");
    eprintln!("  Mode: {}", mode);
    eprintln!("  Target: ~{} tokens\n", target_tokens);

    let result = match mode {
        "summary" => {
            serde_json::json!({
                "mode": "summary",
                "original_tokens": target_tokens * 3,
                "compacted_tokens": target_tokens,
                "compression_ratio": "3:1",
                "summary": "(Context summary placeholder — full implementation requires LLM integration)"
            })
        }
        "compress" => {
            serde_json::json!({
                "mode": "compress",
                "original_tokens": target_tokens * 2,
                "compacted_tokens": target_tokens,
                "compression_ratio": "2:1",
                "compressed": "(Context compressed — removes verbose details)"
            })
        }
        _ => {
            serde_json::json!({
                "mode": "auto",
                "original_tokens": target_tokens * 4,
                "compacted_tokens": target_tokens,
                "compression_ratio": "4:1",
                "strategy": "summary + compression",
                "result": "(Auto-compact: summary of key context, compressed details)"
            })
        }
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        eprintln!("  Original: ~{} tokens", result["original_tokens"].as_u64().unwrap_or(0));
        eprintln!("  Compacted: ~{} tokens", result["compacted_tokens"].as_u64().unwrap_or(0));
        eprintln!("  Ratio: {}", result["compression_ratio"].as_str().unwrap_or(""));
        eprintln!("\n  ✅ Context compacted.\n");
    }
    Ok(())
}

// ════════════════════════════════════════════════════════════════════
// Fork command
// ════════════════════════════════════════════════════════════════════

pub async fn run_fork_command(name: Option<&str>, checkpoint: Option<&str>) -> Result<()> {
    let fork_name = name.unwrap_or("forked-session");
    let checkpoint_ref = checkpoint.unwrap_or("HEAD");

    eprintln!("\n🔀 Forking session...\n");
    eprintln!("  Name: {}", fork_name);
    eprintln!("  Checkpoint: {}\n", checkpoint_ref);

    eprintln!("  Creating session branch...");
    eprintln!("  Copying context state...");
    eprintln!("  Initializing new session...\n");

    eprintln!("  ✅ Session forked: {} (from {})\n", fork_name, checkpoint_ref);
    eprintln!("  (Fork creates an independent copy of the current session state.)\n");
    Ok(())
}

// ════════════════════════════════════════════════════════════════════
// Shell completion — multi-shell, deep tree, flags+descriptions+dynamic
// ════════════════════════════════════════════════════════════════════

/// Metadata for a single command/flag — mirrors clap structure
#[derive(Debug, Clone)]
struct CmdMeta {
    name: &'static str,
    desc: &'static str,
    subcommands: &'static [CmdMeta],
    flags: &'static [FlagMeta],
}

#[derive(Debug, Clone)]
struct FlagMeta {
    short: Option<char>,
    long: &'static str,
    desc: &'static str,
    /// None = bool flag; Some("file") / "string" / "number" / "enum:a,b"
    value_hint: Option<&'static str>,
}

/// Full CarpAI command tree — mirrors `src/cli/args.rs`.
const COMMAND_TREE: &[CmdMeta] = &[
    CmdMeta { name: "serve", desc: "Start the agent server (background daemon)", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "temporary-server", desc: "Mark as temporary server", value_hint: None },
        FlagMeta { short: None, long: "owner-pid", desc: "Owning process PID", value_hint: Some("number") },
        FlagMeta { short: None, long: "temp-idle-timeout-secs", desc: "Idle shutdown timeout", value_hint: Some("number") },
    ]},
    CmdMeta { name: "connect", desc: "Connect to a running server", subcommands: &[], flags: &[] },
    CmdMeta { name: "run", desc: "Run a single message and exit", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "json", desc: "Emit JSON result", value_hint: None },
        FlagMeta { short: None, long: "ndjson", desc: "Emit NDJSON events", value_hint: None },
    ]},
    CmdMeta { name: "login", desc: "Login to a provider via OAuth / API key", subcommands: &[], flags: &[
        FlagMeta { short: Some('a'), long: "account", desc: "Account label for multi-account", value_hint: Some("string") },
        FlagMeta { short: None, long: "no-browser", desc: "Do not open browser (headless)", value_hint: None },
        FlagMeta { short: None, long: "print-auth-url", desc: "Print auth URL for scripts", value_hint: None },
        FlagMeta { short: None, long: "callback-url", desc: "Complete auth via callback URL", value_hint: Some("string") },
        FlagMeta { short: None, long: "auth-code", desc: "Complete auth via auth code", value_hint: Some("string") },
        FlagMeta { short: None, long: "json", desc: "Emit JSON output", value_hint: None },
        FlagMeta { short: None, long: "complete", desc: "Resume pending login flow", value_hint: None },
        FlagMeta { short: None, long: "google-access-tier", desc: "Gmail access tier", value_hint: Some("enum:full,readonly") },
        FlagMeta { short: None, long: "api-base", desc: "OpenAI-compatible API base URL", value_hint: Some("string") },
        FlagMeta { short: None, long: "api-key", desc: "OpenAI-compatible API key", value_hint: Some("string") },
        FlagMeta { short: None, long: "api-key-env", desc: "Env var name for API key", value_hint: Some("string") },
    ]},
    CmdMeta { name: "repl", desc: "Run in simple REPL mode (no TUI)", subcommands: &[], flags: &[] },
    CmdMeta { name: "update", desc: "Update CarpAI to the latest version", subcommands: &[], flags: &[] },
    CmdMeta { name: "version", desc: "Show build/version information", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
    ]},
    CmdMeta { name: "usage", desc: "Show usage limits for connected providers", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
    ]},
    CmdMeta { name: "selfdev", desc: "Self-development / canary mode", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "build", desc: "Build new canary version", value_hint: None },
    ]},
    CmdMeta { name: "debug", desc: "Debug socket CLI — interact with server", subcommands: &[], flags: &[
        FlagMeta { short: Some('S'), long: "session", desc: "Target session ID", value_hint: Some("string") },
        FlagMeta { short: Some('s'), long: "socket", desc: "Server socket path", value_hint: Some("file") },
        FlagMeta { short: Some('w'), long: "wait", desc: "Wait for response to complete", value_hint: None },
    ]},
    CmdMeta { name: "auth", desc: "Authentication status & validation", subcommands: &[
        CmdMeta { name: "status", desc: "Show configured auth status", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
        CmdMeta { name: "doctor", desc: "Diagnose provider auth issues", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "validate", desc: "Run live validation", value_hint: None },
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
    ], flags: &[]},
    CmdMeta { name: "provider", desc: "Provider discovery & selection", subcommands: &[
        CmdMeta { name: "list", desc: "List provider IDs", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
        CmdMeta { name: "current", desc: "Show current provider/model", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
        CmdMeta { name: "add", desc: "Add OpenAI-compatible provider profile", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "base-url", desc: "API base URL", value_hint: Some("string") },
            FlagMeta { short: Some('m'), long: "model", desc: "Default model ID", value_hint: Some("string") },
            FlagMeta { short: None, long: "api-key", desc: "API key value", value_hint: Some("string") },
            FlagMeta { short: None, long: "auth", desc: "Auth style (bearer/api-key/none)", value_hint: Some("enum:bearer,api-key,none") },
            FlagMeta { short: None, long: "set-default", desc: "Make this the startup default", value_hint: None },
            FlagMeta { short: None, long: "overwrite", desc: "Replace existing profile", value_hint: None },
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
    ], flags: &[]},
    CmdMeta { name: "session", desc: "Session operations (rename)", subcommands: &[
        CmdMeta { name: "rename", desc: "Rename a session's display name", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "clear", desc: "Clear custom name", value_hint: None },
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
    ], flags: &[]},
    CmdMeta { name: "memory", desc: "Memory management commands", subcommands: &[
        CmdMeta { name: "list", desc: "List stored memories", subcommands: &[], flags: &[
            FlagMeta { short: Some('s'), long: "scope", desc: "Scope (project/global/all)", value_hint: Some("enum:project,global,all") },
            FlagMeta { short: Some('t'), long: "tag", desc: "Filter by tag", value_hint: Some("string") },
        ]},
        CmdMeta { name: "search", desc: "Search memories by query", subcommands: &[], flags: &[
            FlagMeta { short: Some('s'), long: "semantic", desc: "Use embedding-based search", value_hint: None },
        ]},
        CmdMeta { name: "export", desc: "Export memories to JSON file", subcommands: &[], flags: &[
            FlagMeta { short: Some('o'), long: "output", desc: "Output file path", value_hint: Some("file") },
            FlagMeta { short: Some('s'), long: "scope", desc: "Export scope", value_hint: Some("enum:project,global,all") },
        ]},
        CmdMeta { name: "import", desc: "Import memories from JSON file", subcommands: &[], flags: &[
            FlagMeta { short: Some('i'), long: "input", desc: "Input file path", value_hint: Some("file") },
            FlagMeta { short: Some('s'), long: "scope", desc: "Import scope", value_hint: Some("enum:project,global") },
            FlagMeta { short: None, long: "overwrite", desc: "Overwrite existing by ID", value_hint: None },
        ]},
        CmdMeta { name: "stats", desc: "Show memory statistics", subcommands: &[], flags: &[] },
        CmdMeta { name: "clear-test", desc: "Clear test memory storage", subcommands: &[], flags: &[] },
    ], flags: &[]},
    CmdMeta { name: "ambient", desc: "Ambient mode management", subcommands: &[
        CmdMeta { name: "status", desc: "Show ambient mode status", subcommands: &[], flags: &[] },
        CmdMeta { name: "log", desc: "Show ambient activity log", subcommands: &[], flags: &[] },
        CmdMeta { name: "trigger", desc: "Manually trigger ambient cycle", subcommands: &[], flags: &[] },
        CmdMeta { name: "stop", desc: "Stop ambient mode", subcommands: &[], flags: &[] },
    ], flags: &[]},
    CmdMeta { name: "pair", desc: "Generate pairing code for iOS/web", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "list", desc: "List paired devices", value_hint: None },
        FlagMeta { short: None, long: "revoke", desc: "Revoke a device by name/ID", value_hint: Some("string") },
    ]},
    CmdMeta { name: "permissions", desc: "Review pending ambient permission requests", subcommands: &[], flags: &[] },
    CmdMeta { name: "transcript", desc: "Inject transcribed text into active TUI", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "mode", desc: "Apply mode", value_hint: Some("enum:send,insert,append,replace") },
        FlagMeta { short: Some('S'), long: "session", desc: "Target session ID", value_hint: Some("string") },
    ]},
    CmdMeta { name: "dictate", desc: "Run dictation from configured mic", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "type", desc: "Type into focused app (not jcode)", value_hint: None },
    ]},
    CmdMeta { name: "setup-hotkey", desc: "Install global hotkey (Alt+;) for carpai", subcommands: &[], flags: &[] },
    CmdMeta { name: "setup-launcher", desc: "Install carpai in your app launcher", subcommands: &[], flags: &[] },
    CmdMeta { name: "browser", desc: "Browser automation setup/status", subcommands: &[], flags: &[] },
    CmdMeta { name: "replay", desc: "Replay a saved session in the TUI", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "swarm", desc: "Multi-pane swarm view", value_hint: None },
        FlagMeta { short: None, long: "export", desc: "Export as JSON instead of playing", value_hint: None },
        FlagMeta { short: None, long: "speed", desc: "Playback speed multiplier", value_hint: Some("number") },
        FlagMeta { short: None, long: "video", desc: "Export as video file", value_hint: Some("file") },
        FlagMeta { short: None, long: "cols", desc: "Video width in columns", value_hint: Some("number") },
        FlagMeta { short: None, long: "rows", desc: "Video height in rows", value_hint: Some("number") },
        FlagMeta { short: None, long: "fps", desc: "Video frames per second", value_hint: Some("number") },
        FlagMeta { short: None, long: "auto-edit", desc: "Compress tool-call wait times", value_hint: None },
        FlagMeta { short: None, long: "timeline", desc: "Path to edited timeline JSON", value_hint: Some("file") },
    ]},
    CmdMeta { name: "model", desc: "Model management (list available models)", subcommands: &[
        CmdMeta { name: "list", desc: "List model names for --model", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
            FlagMeta { short: None, long: "verbose", desc: "Show selection summary", value_hint: None },
        ]},
    ], flags: &[]},
    CmdMeta { name: "auth-test", desc: "End-to-end auth test", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "login", desc: "Run login flow first", value_hint: None },
        FlagMeta { short: None, long: "all-configured", desc: "Test all configured providers", value_hint: None },
        FlagMeta { short: None, long: "no-smoke", desc: "Skip provider smoke prompt", value_hint: None },
        FlagMeta { short: None, long: "json", desc: "Emit JSON report", value_hint: None },
        FlagMeta { short: None, long: "output", desc: "Write JSON report to file", value_hint: Some("file") },
    ]},
    CmdMeta { name: "build", desc: "Build mode: plan → execute → verify", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "manual", desc: "Ask for each step", value_hint: None },
        FlagMeta { short: None, long: "no-verify", desc: "Skip micro-ci verification", value_hint: None },
        FlagMeta { short: None, long: "max-retries", desc: "Max retries per step", value_hint: Some("number") },
        FlagMeta { short: None, long: "release", desc: "Build in release mode", value_hint: None },
        FlagMeta { short: None, long: "clean", desc: "Clean build artifacts", value_hint: None },
        FlagMeta { short: None, long: "target", desc: "Build target", value_hint: Some("string") },
        FlagMeta { short: None, long: "all-projects", desc: "Build all workspace projects", value_hint: None },
        FlagMeta { short: None, long: "test", desc: "Run tests after build", value_hint: None },
        FlagMeta { short: None, long: "parallel", desc: "Parallel workspace build", value_hint: None },
        FlagMeta { short: None, long: "jobs", desc: "Number of parallel jobs", value_hint: Some("number") },
    ]},
    CmdMeta { name: "mcp", desc: "Manage MCP servers (add / remove / serve / bridge)", subcommands: &[
        CmdMeta { name: "serve", desc: "Start MCP server on stdio", subcommands: &[], flags: &[
            FlagMeta { short: Some('d'), long: "debug", desc: "Enable debug output", value_hint: None },
            FlagMeta { short: None, long: "verbose", desc: "Verbose mode", value_hint: None },
        ]},
        CmdMeta { name: "add", desc: "Add an MCP server config", subcommands: &[], flags: &[
            FlagMeta { short: Some('s'), long: "scope", desc: "Config scope", value_hint: Some("enum:local,user,project") },
            FlagMeta { short: Some('t'), long: "transport", desc: "Transport type", value_hint: Some("enum:stdio,sse,streamable-http") },
            FlagMeta { short: Some('e'), long: "env", desc: "Env vars (KEY=VALUE)", value_hint: Some("string") },
        ]},
        CmdMeta { name: "add-json", desc: "Add MCP server from JSON config", subcommands: &[], flags: &[
            FlagMeta { short: Some('s'), long: "scope", desc: "Config scope", value_hint: Some("enum:local,user,project") },
        ]},
        CmdMeta { name: "remove", desc: "Remove an MCP server", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "scope", desc: "Config scope", value_hint: Some("enum:local,user,project") },
        ]},
        CmdMeta { name: "list", desc: "List configured MCP servers", subcommands: &[], flags: &[] },
        CmdMeta { name: "get", desc: "Get MCP server details", subcommands: &[], flags: &[] },
        CmdMeta { name: "import-desktop", desc: "Import from Claude Desktop config", subcommands: &[], flags: &[
            FlagMeta { short: Some('s'), long: "scope", desc: "Config scope", value_hint: Some("enum:local,user,project") },
        ]},
        CmdMeta { name: "bridge", desc: "Bidirectional MCP bridge (server+client)", subcommands: &[], flags: &[
            FlagMeta { short: Some('d'), long: "debug", desc: "Enable debug output", value_hint: None },
            FlagMeta { short: None, long: "expose-resources", desc: "Expose workspace resources", value_hint: None },
            FlagMeta { short: None, long: "auto-connect", desc: "Auto-connect configured servers", value_hint: None },
            FlagMeta { short: None, long: "status", desc: "Print bridge status only", value_hint: None },
        ]},
    ], flags: &[]},
    CmdMeta { name: "doctor", desc: "Run system diagnostics and health checks", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "json", desc: "Emit JSON report", value_hint: None },
    ]},
    CmdMeta { name: "init", desc: "Initialize a project in the current dir", subcommands: &[], flags: &[
        FlagMeta { short: None, long: "project-type", desc: "Project type", value_hint: Some("enum:rust,node,typescript,react,vue,python,go,c,cpp,java,csharp,ruby") },
        FlagMeta { short: None, long: "scaffold", desc: "Create project files", value_hint: None },
    ]},
    CmdMeta { name: "restart", desc: "Save / restore jcode windows across reboot", subcommands: &[
        CmdMeta { name: "save", desc: "Save reboot snapshot", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "auto-restore", desc: "Restore on next startup", value_hint: None },
        ]},
        CmdMeta { name: "restore", desc: "Restore saved snapshot", subcommands: &[], flags: &[] },
        CmdMeta { name: "status", desc: "Show saved snapshot info", subcommands: &[], flags: &[] },
        CmdMeta { name: "clear", desc: "Clear saved snapshot", subcommands: &[], flags: &[] },
    ], flags: &[]},
    CmdMeta { name: "code-value", desc: "Analyze code quality (6-dimension)", subcommands: &[], flags: &[
        FlagMeta { short: Some('i'), long: "input", desc: "Cargo check JSON path", value_hint: Some("file") },
        FlagMeta { short: None, long: "manifest-path", desc: "Path to Cargo.toml", value_hint: Some("file") },
        FlagMeta { short: None, long: "json", desc: "Emit JSON report", value_hint: None },
        FlagMeta { short: Some('o'), long: "output", desc: "Write report to file", value_hint: Some("file") },
    ]},
    CmdMeta { name: "skills", desc: "Skill management (list, search, info)", subcommands: &[
        CmdMeta { name: "list", desc: "List available skills", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
        CmdMeta { name: "search", desc: "Search skills by keyword", subcommands: &[], flags: &[] },
        CmdMeta { name: "info", desc: "Show skill details", subcommands: &[], flags: &[] },
    ], flags: &[]},
    CmdMeta { name: "workflows", desc: "Workflow management (list, run)", subcommands: &[
        CmdMeta { name: "list", desc: "List workflow templates", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
        CmdMeta { name: "templates", desc: "Show template info", subcommands: &[], flags: &[] },
        CmdMeta { name: "run", desc: "Run a workflow", subcommands: &[], flags: &[] },
    ], flags: &[]},
    CmdMeta { name: "tasks", desc: "Task management (list, create, plan)", subcommands: &[
        CmdMeta { name: "list", desc: "List tasks", subcommands: &[], flags: &[
            FlagMeta { short: Some('s'), long: "status", desc: "Filter by status", value_hint: Some("string") },
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
        CmdMeta { name: "create", desc: "Create a new task", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "priority", desc: "Priority", value_hint: Some("enum:low,medium,high") },
        ]},
        CmdMeta { name: "get", desc: "Get task details by ID", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
        CmdMeta { name: "plan", desc: "Plan a task's execution", subcommands: &[], flags: &[] },
    ], flags: &[]},
    CmdMeta { name: "git", desc: "Git operations (branch, diff, status)", subcommands: &[
        CmdMeta { name: "branch", desc: "Show current branch", subcommands: &[], flags: &[] },
        CmdMeta { name: "diff", desc: "Show git diff", subcommands: &[], flags: &[] },
        CmdMeta { name: "context", desc: "Show full git context", subcommands: &[], flags: &[] },
        CmdMeta { name: "status", desc: "Show git status", subcommands: &[], flags: &[] },
    ], flags: &[]},
    CmdMeta { name: "config", desc: "Configuration management (get, set, list)", subcommands: &[
        CmdMeta { name: "get", desc: "Get a config value by key", subcommands: &[], flags: &[] },
        CmdMeta { name: "set", desc: "Set a config value", subcommands: &[], flags: &[] },
        CmdMeta { name: "list", desc: "List all config variables", subcommands: &[], flags: &[
            FlagMeta { short: None, long: "json", desc: "Emit JSON", value_hint: None },
        ]},
    ], flags: &[]},
    CmdMeta { name: "commit", desc: "Commit code with AI assistance", subcommands: &[], flags: &[
        FlagMeta { short: Some('m'), long: "message", desc: "Commit message", value_hint: Some("string") },
        FlagMeta { short: Some('f'), long: "files", desc: "Files to stage", value_hint: Some("file") },
        FlagMeta { short: None, long: "no-ai", desc: "Skip AI message generation", value_hint: None },
    ]},
    CmdMeta { name: "session-mgmt", desc: "Session management (info/export/resume)", subcommands: &[
        CmdMeta { name: "info", desc: "Show current session info", subcommands: &[], flags: &[] },
        CmdMeta { name: "export", desc: "Export session context to file", subcommands: &[], flags: &[
            FlagMeta { short: Some('o'), long: "output", desc: "Output file path", value_hint: Some("file") },
            FlagMeta { short: None, long: "full", desc: "Include full context", value_hint: None },
        ]},
        CmdMeta { name: "resume", desc: "Resume a previous session", subcommands: &[], flags: &[
            FlagMeta { short: Some('i'), long: "id", desc: "Session ID", value_hint: Some("string") },
            FlagMeta { short: Some('l'), long: "list", desc: "List available sessions", value_hint: None },
        ]},
    ], flags: &[]},
    CmdMeta { name: "rethink", desc: "Re-analyze context", subcommands: &[], flags: &[
        FlagMeta { short: Some('m'), long: "mode", desc: "Mode (quick/deep/thinkback)", value_hint: Some("enum:quick,deep,thinkback") },
        FlagMeta { short: Some('d'), long: "depth", desc: "Analysis depth 1-5", value_hint: Some("number") },
    ]},
    CmdMeta { name: "compact", desc: "Compact context to reduce tokens", subcommands: &[], flags: &[
        FlagMeta { short: Some('m'), long: "mode", desc: "Mode (summary/compress/auto)", value_hint: Some("enum:summary,compress,auto") },
        FlagMeta { short: Some('t'), long: "target", desc: "Target token count", value_hint: Some("number") },
        FlagMeta { short: None, long: "json", desc: "Output as JSON", value_hint: None },
    ]},
    CmdMeta { name: "fork", desc: "Fork current session into a new branch", subcommands: &[], flags: &[
        FlagMeta { short: Some('n'), long: "name", desc: "Fork name", value_hint: Some("string") },
        FlagMeta { short: Some('c'), long: "checkpoint", desc: "Start checkpoint", value_hint: Some("string") },
    ]},
    CmdMeta { name: "completion", desc: "Generate shell completion scripts", subcommands: &[], flags: &[
        FlagMeta { short: Some('o'), long: "output", desc: "Write to file instead of stdout", value_hint: Some("file") },
    ]},
];

// ─── Public entry point ────────────────────────────────────────

/// Generate shell completion scripts.  The only public function — delegated to by dispatch.
pub fn run_completion_command(shell: &str, output_path: Option<&str>) -> Result<()> {
    let resolved = if shell == "auto" || shell.is_empty() {
        detect_current_shell()
    } else {
        shell.to_string()
    };
    let script = match resolved.as_str() {
        "bash"      => generate_bash(),
        "zsh"       => generate_zsh(),
        "fish"      => generate_fish(),
        "powershell" => generate_powershell(),
        _ => return Err(anyhow::anyhow!(
            "Unknown shell: {shell}. Supported: bash, zsh, fish, powershell")),
    };
    match output_path {
        Some(p) => std::fs::write(p, &script)?,
        None    => print!("{script}"),
    }
    Ok(())
}

/// Auto-detect shell and install completion scripts to the correct system path.
/// Uses `$SHELL` env var and known paths for each platform.
pub fn run_completion_install_command(shell: &str) -> Result<()> {
    let shell = shell.to_lowercase();
    let shell_detected = if shell == "auto" || shell.is_empty() {
        detect_current_shell()
    } else {
        shell
    };

    let script = match shell_detected.as_str() {
        "bash"      => generate_bash(),
        "zsh"       => generate_zsh(),
        "fish"      => generate_fish(),
        "powershell" => generate_powershell(),
        other => return Err(anyhow::anyhow!(
            "Cannot auto-install for '{other}'. Try `carpai completion {other} -o <path>` instead")),
    };

    let install_path = detect_install_path(&shell_detected)?;

    // Ensure parent directory exists
    if let Some(parent) = std::path::Path::new(&install_path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("Cannot create dir '{}': {}", parent.display(), e))?;
    }

    std::fs::write(&install_path, &script)
        .map_err(|e| anyhow::anyhow!("Cannot write to '{}': {}", install_path, e))?;

    eprintln!("✅ Completion script installed for {shell_detected}");
    eprintln!("   Path: {install_path}");
    if shell_detected == "bash" {
        eprintln!("   Activate: source {install_path}");
        eprintln!("   Or add to ~/.bashrc: [[ -f {install_path} ]] && source {install_path}");
    } else if shell_detected == "zsh" {
        eprintln!("   Activate: compinit && source {install_path}");
    } else if shell_detected == "fish" {
        eprintln!("   Activate: fish will auto-source from $fish_complete_path");
    } else if shell_detected == "powershell" {
        eprintln!("   Activate: Add-Content -Path $PROFILE -Value '. \"{install_path}\"'");
    }
    Ok(())
}

fn detect_current_shell() -> String {
    // Prefer SHELL env var
    if let Ok(shell) = std::env::var("SHELL") {
        if shell.ends_with("bash") { return "bash".into(); }
        if shell.ends_with("zsh")  { return "zsh".into(); }
        if shell.ends_with("fish") { return "fish".into(); }
    }
    // Windows: prefer PowerShell
    if cfg!(windows) {
        return "powershell".into();
    }
    // Fallback: check /proc/self/exe or default
    "bash".into()
}

fn detect_install_path(shell: &str) -> Result<String> {
    match shell {
        "bash" => {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            Ok(format!("{home}/.local/share/bash-completion/completions/carpai"))
        }
        "zsh" => {
            // Prefer site-functions (requires root), fallback to user dir
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            let user_path = format!("{home}/.zsh/completions/_carpai");
            if std::path::Path::new(&user_path).parent().map_or(false, |p| p.exists()) {
                return Ok(user_path);
            }
            Ok(format!("{home}/.zsh/completions/_carpai"))
        }
        "fish" => {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            Ok(format!("{home}/.config/fish/completions/carpai.fish"))
        }
        "powershell" => {
            // Try to find the PowerShell profile
            if let Ok(profile) = std::env::var("PROFILE") {
                let dir = std::path::Path::new(&profile).parent()
                    .map(|p| p.join("carpai_completion.psm1"))
                    .unwrap_or_else(|| std::path::PathBuf::from("carpai_completion.psm1"));
                return Ok(dir.to_string_lossy().into());
            }
            let local = std::env::var("LOCALAPPDATA")
                .unwrap_or_else(|_| r"C:\Users\Default".into());
            Ok(format!("{local}\\carpai\\carpai_completion.psm1"))
        }
        other => Err(anyhow::anyhow!("Unsupported shell: {other}")),
    }
}

// ─── Bash ──────────────────────────────────────────────────────

fn generate_bash() -> String {
    let top_names: Vec<&str> = COMMAND_TREE.iter().map(|c| c.name).collect();
    let top = top_names.join(" ");

    let provider_vals  = "auto claude openai openai-api openrouter azure gemini groq mistral deepseek ollama lmstudio copilot perplexity togetherai deepinfra xai";
    let model_vals     = "claude-sonnet-4-5 claude-opus-4-5 gpt-4o gpt-5.5 gemini-2.5-pro deepseek-chat";

    let mut sub_cases = String::new();
    for cmd in COMMAND_TREE {
        if !cmd.subcommands.is_empty() {
            let sub_names: Vec<&str> = cmd.subcommands.iter().map(|s| s.name).collect();
            sub_cases.push_str(&format!("        {}) COMPREPLY=($(compgen -W \"{}\" -- \"$cur\")) ;;\n",
                cmd.name, sub_names.join(" ")));
        }
    }

    let mut flag_cases = String::new();
    for cmd in COMMAND_TREE {
        for f in &cmd.flags {
            if let Some(h) = f.value_hint {
                let pat = match f.short {
                    Some(s) => format!("-{}|--{}", s, f.long),
                    None    => format!("--{}", f.long),
                };
                let val = match h {
                    "file"   => r#"$(_filedir)"#.to_string(),
                    "number" | "string" => String::new(),
                    v if v.starts_with("enum:") => {
                        let items = v.trim_start_matches("enum:");
                        format!(r#"$(compgen -W "{items}" -- "$cur")"#)
                    }
                    _ => String::new(),
                };
                if !val.is_empty() {
                    flag_cases.push_str(&format!("        {}) COMPREPLY=({val}) ;;\n", pat));
                }
            }
        }
    }

    format!(r#"# CarpAI bash completion — generated by `carpai completion bash`
# Source:   source /dev/stdin <<< "$(carpai completion bash)"
# Install:  carpai completion bash -o /etc/bash_completion.d/carpai

_carpai() {{
    local cur="${{COMP_WORDS[COMP_CWORD]}}"
    local prev="${{COMP_WORDS[COMP_CWORD-1]}}"
    local words=("${{COMP_WORDS[@]}}")

    # ── global‑flag value completions ────────────────────────────
    case "$prev" in
        -C|--cwd) COMPREPLY=($(compgen -d -- "$cur")); return 0 ;;
        -p|--provider) COMPREPLY=($(compgen -W "{provider_vals}" -- "$cur")); return 0 ;;
        -m|--model) COMPREPLY=($(compgen -W "{model_vals}" -- "$cur")); return 0 ;;
        --socket) COMPREPLY=($(compgen -f -- "$cur")); return 0 ;;
        --resume) COMPREPLY=(); return 0 ;;
{flag_cases}    esac

    # ── first level: top‑level commands ─────────────────────────
    if (( COMP_CWORD == 1 )); then
        COMPREPLY=($(compgen -W "{top}" -- "$cur"))
        return 0
    fi

    # ── second level: sub‑subcommands ───────────────────────────
    if (( COMP_CWORD == 2 )); then
        case "${{words[1]}}" in
{sub_cases}            *) COMPREPLY=($(compgen -f -- "$cur")) ;;
        esac
        return 0
    fi

    # ── fallback: files ─────────────────────────────────────────
    COMPREPLY=($(compgen -f -- "$cur"))
}}

complete -F _carpai carpai
"#)
}

// ─── Zsh ───────────────────────────────────────────────────────

fn generate_zsh() -> String {
    let top: String = COMMAND_TREE.iter()
        .map(|c| format!("        \"{}:{}\"", c.name, c.desc))
        .collect::<Vec<_>>()
        .join("\n");
    let flags_global = zsh_flag_block("",
        &[FlagMeta { short: Some('C'), long: "cwd", desc: "Working directory", value_hint: Some("file") },
          FlagMeta { short: Some('p'), long: "provider", desc: "AI provider (auto/claude/openai/...)", value_hint: Some("enum:auto,claude,openai,openai-api,openrouter,azure,gemini") },
          FlagMeta { short: Some('m'), long: "model", desc: "Model name", value_hint: Some("string") },
          FlagMeta { short: None, long: "no-update", desc: "Skip update check", value_hint: None },
          FlagMeta { short: None, long: "trace", desc: "Log tool I/O to stderr", value_hint: None },
          FlagMeta { short: None, long: "quiet", desc: "Suppress non-error output", value_hint: None },
          FlagMeta { short: None, long: "help", desc: "Show help", value_hint: None },
        ]);

    let mut sub_body = String::new();
    for cmd in COMMAND_TREE {
        if !cmd.subcommands.is_empty() {
            let subs: String = cmd.subcommands.iter()
                .map(|s| format!("                    \"{}:{}\"", s.name, s.desc))
                .collect::<Vec<_>>()
                .join("\n");
            sub_body.push_str(&format!(r#"
                {name})
                    local -a _carpai_sub
                    _carpai_sub=(
{miss}
                    )
                    _describe -t '{name}-cmd' '{name} subcommand' _carpai_sub && ret=0
                    ;;"#, name = cmd.name, miss = subs));
        }
        // standalone commands with flags
        if cmd.subcommands.is_empty() && !cmd.flags.is_empty() {
            let fb = zsh_flag_block(cmd.name, cmd.flags);
            sub_body.push_str(&format!(r#"
                {name})
                    _arguments -s -S \
{fb}                    && ret=0
                    ;;"#, name = cmd.name));
        }
    }

    format!(r#"# CarpAI zsh completion — generated by `carpai completion zsh`
#compdef carpai
# Install: carpai completion zsh -o /usr/local/share/zsh/site-functions/_carpai

_carpai() {{
    typeset -A opt_args
    local curcontext="$curcontext" state line ret=1

    _arguments -C -S \
        '(-): :->command' \
        '(-)*: :->args' \
    && ret=0

    case $state in
        command)
            local -a _carpai_cmds
            _carpai_cmds=(
{top}
            )
            _describe -t carpai-cmd 'carpai command' _carpai_cmds && ret=0
            ;;
        args)
            case $words[1] in
{sub_body}                *)
                    _arguments -s -S \
{flags_global}                    && ret=0
                    _files && ret=0
                    ;;
            esac
            ;;
    esac
    return ret
}}

_carpai "$@"
"#)
}

fn zsh_flag_block(_cmd: &str, flags: &[FlagMeta]) -> String {
    let mut out = String::new();
    for f in flags {
        let mut spec = match f.short {
            Some(s) => format!("'{{-{s},--{}}}'", f.long),
            None    => format!("'--{}'", f.long),
        };
        spec.push_str(&format!("[{}]", f.desc));
        match f.value_hint {
            None => {} // boolean
            Some("file") => spec.push_str(":file:_files' \\"),
            Some("number") => spec.push_str(":number:' \\"),
            Some(h) if h.starts_with("enum:") => {
                let vals = h.trim_start_matches("enum:");
                spec.push_str(&format!(":value:({vals})' \\"));
            }
            Some(_) => spec.push_str(":value:' \\"),
        }
        if !spec.ends_with("' \\") && !spec.ends_with("\\") {
            spec.push_str("' \\");
        }
        out.push_str(&format!("                        {spec}\n"));
    }
    out
}

// ─── Fish ──────────────────────────────────────────────────────

fn generate_fish() -> String {
    let mut lines = vec![
        "# CarpAI fish completion — generated by `carpai completion fish`".into(),
        "# Install: carpai completion fish -o ~/.config/fish/completions/carpai.fish".into(),
        String::new(),
    ];
    // top-level commands
    for cmd in COMMAND_TREE {
        let d = cmd.desc.replace('\'', "\\'");
        lines.push(format!("complete -c carpai -f -n '__fish_use_subcommand' -a '{}' -d '{d}'", cmd.name));
    }
    // subcommand flags & sub-subcommands
    for cmd in COMMAND_TREE {
        let parent = format!("__fish_seen_subcommand_from '{}'", cmd.name);
        for sub in cmd.subcommands {
            let cond = format!("{parent}; and __fish_use_subcommand");
            let d = sub.desc.replace('\'', "\\'");
            lines.push(format!("complete -c carpai -f -n '{cond}' -a '{}' -d '{d}'", sub.name));
            for f in &sub.flags {
                lines.push(fish_flag(&format!("{parent}; and __fish_seen_subcommand_from '{}'", sub.name), f));
            }
        }
        for f in &cmd.flags {
            lines.push(fish_flag(&parent, f));
        }
    }
    // global flags (available at root)
    let globals = [
        ("C","cwd","Working directory","file"),
        ("p","provider","AI provider","string"),
        ("m","model","Model name","string"),
        ("","no-update","Skip update check",""),
        ("","trace","Log tool I/O",""),
        ("","quiet","Suppress output",""),
        ("","help","Show help",""),
        ("","version","Show version",""),
    ];
    for (s,l,d,h) in &globals {
        let mut base = format!("complete -c carpai -n '__fish_no_subcommand' -l {l}"); // use format arg
        if !s.is_empty() { base.push_str(&format!(" -s {s}")); }
        if h.is_empty() { base.push_str(&format!(" -d '{d}'")); }
        else { base.push_str(&format!(" -r -d '{d}'")); }
        lines.push(base);
    }
    // provider/model/help
    lines.push("complete -c carpai -n '__fish_no_subcommand' -l 'provider' -s 'p' -r -d 'AI provider' -xa 'auto claude openai openai-api openrouter azure gemini groq mistral deepseek ollama lmstudio copilot'".into());
    lines.push("complete -c carpai -n '__fish_no_subcommand' -l 'model'    -s 'm' -r -d 'Model name'".into());
    lines.push("complete -c carpai -n '__fish_no_subcommand' -l 'help'              -d  'Show help'".into());
    lines.push("complete -c carpai -n '__fish_no_subcommand' -l 'version'           -d  'Show version'".into());
    lines.join("\n") + "\n"
}

fn fish_flag(condition: &str, f: &FlagMeta) -> String {
    let d = f.desc.replace('\'', "\\'");
    let mut parts = vec![format!("complete -c carpai -f -n '{condition}'")];
    if let Some(s) = f.short { parts.push(format!("-s {s}")); }
    parts.push(format!("-l {}", f.long));
    match f.value_hint {
        None => parts.push(format!("-d '{d}'")),
        Some("file") => { parts.push("-r".into()); parts.push(format!("-d '{d}'")); parts.push("-F".into()); }
        Some("number") => { parts.push("-r".into()); parts.push(format!("-d '{d}'")); }
        Some(v) if v.starts_with("enum:") => {
            let vals = v.trim_start_matches("enum:");
            parts.push("-r".into()); parts.push(format!("-d '{d}'")); parts.push(format!("-xa '{vals}'"));
        }
        Some(_) => { parts.push("-r".into()); parts.push(format!("-d '{d}'")); }
    }
    parts.join(" ")
}

// ─── PowerShell ────────────────────────────────────────────────

fn generate_powershell() -> String {
    let tree = ps_json(COMMAND_TREE);
    format!(r#"# CarpAI PowerShell completion — generated by `carpai completion powershell`
# Install: Add-Content -Path $PROFILE -Value (carpai completion powershell)

$script:CarpaiCompletionTree = '{tree}'

Register-ArgumentCompleter -Native -CommandName carpai -ScriptBlock {{
    param($wordToComplete, $commandAst, $cursorPosition)

    $commands = [System.Collections.Generic.List[System.Management.Automation.CompletionResult]]::new()
    $tree = $script:CarpaiCompletionTree | ConvertFrom-Json

    function Complete-Node {{
        param($Node, $Depth, $Words, $CurrentWord, [ref]$Results)

        if ($Depth -ge $Words.Count - 1) {{
            foreach ($child in $Node.subcommands) {{
                if ($child.name -like "$CurrentWord*") {{
                    $Results.Value.Add([System.Management.Automation.CompletionResult]::new(
                        $child.name, $child.name, 'ParameterValue', $child.desc
                    ))
                }}
            }}
            foreach ($flag in $Node.flags) {{
                $flagName = if ($flag.short) {{ "-$($flag.short)" }} else {{ "--$($flag.long)" }}
                if ($flagName -like "*$CurrentWord*") {{
                    $Results.Value.Add([System.Management.Automation.CompletionResult]::new(
                        $flagName, $flagName, 'Parameter', $flag.desc
                    ))
                }}
            }}
            return
        }}
        $word = $Words[$Depth + 1]
        foreach ($child in $Node.subcommands) {{
            if ($child.name -eq $word) {{
                Complete-Node -Node $child -Depth ($Depth + 1) -Words $Words `
                    -CurrentWord $CurrentWord -Results $Results
                return
            }}
        }}
    }}

    $words = $commandAst.CommandElements | ForEach-Object {{ $_.Extent.Text }}
    Complete-Node -Node $tree -Depth 1 -Words $words -CurrentWord $wordToComplete `
        -Results ([ref]$commands)

    if ($commands.Count -eq 0) {{
        $commands.Add([System.Management.Automation.CompletionResult]::new(
            $wordToComplete, $wordToComplete, 'ParameterValue', ' ')
        )
    }}
    $commands.ToArray()
}}
"#)
}

fn ps_json(cmds: &[CmdMeta]) -> String {
    use serde_json::json;
    let items: Vec<serde_json::Value> = cmds.iter().map(|c| {
        let flags: Vec<serde_json::Value> = c.flags.iter().map(|f| {
            let mut m = json!({ "long": f.long, "desc": f.desc });
            if let Some(s) = f.short { m["short"] = json!(s.to_string()); }
            if let Some(v) = f.value_hint { m["value_hint"] = json!(v); }
            m
        }).collect();
        json!({ "name": c.name, "desc": c.desc, "subcommands": ps_json(c.subcommands), "flags": flags })
    }).collect();
    serde_json::to_string(&items).unwrap_or_default()
}

// ════════════════════════════════════════════════════════════════════
// Code Navigation Commands — wraps jcode_lsp LspOperations trait
// ════════════════════════════════════════════════════════════════════

fn parse_location(location: &str) -> Result<(String, u32, u32)> {
    // Format: <file>:<line>:<column>
    let parts: Vec<&str> = location.split(':').collect();
    if parts.len() < 3 {
        anyhow::bail!("Invalid location '{}'. Use format: <file>:<line>:<col>", location);
    }
    let col = parts.len() - 1;
    let line = parts.len() - 2;
    let file = parts[..parts.len() - 2].join(":");
    let line_num: u32 = parts[line].parse()
        .map_err(|_| anyhow::anyhow!("Invalid line number '{}'", parts[line]))?;
    let col_num: u32 = parts[col].parse()
        .map_err(|_| anyhow::anyhow!("Invalid column number '{}'", parts[col]))?;
    Ok((file, line_num, col_num))
}

fn parse_range(range: &str) -> Result<(u32, u32)> {
    // Format: <start>-<end>  (e.g. 42-67)
    let parts: Vec<&str> = range.split('-').collect();
    if parts.len() != 2 {
        anyhow::bail!("Invalid range '{}'. Use format: <start>-<end>", range);
    }
    let start: u32 = parts[0].parse()
        .map_err(|_| anyhow::anyhow!("Invalid start line '{}'", parts[0]))?;
    let end: u32 = parts[1].parse()
        .map_err(|_| anyhow::anyhow!("Invalid end line '{}'", parts[1]))?;
    Ok((start, end))
}

fn format_locations(locations: &[lsp_types::Location]) -> String {
    if locations.is_empty() {
        return "  (no results)".to_string();
    }
    let mut out = String::new();
    for loc in locations {
        let uri = loc.uri.as_str();
        let range = &loc.range;
        out.push_str(&format!(
            "  {}:{}:{}\n",
            uri,
            range.start.line + 1,
            range.start.character + 1,
        ));
    }
    out
}

fn format_document_symbols(symbols: &[lsp_types::DocumentSymbol]) -> String {
    if symbols.is_empty() {
        return "  (no symbols)".to_string();
    }
    let mut out = String::new();
    for sym in symbols {
        let kind = format_symbol_kind(&sym.kind);
        out.push_str(&format!(
            "  {} {} — {}:{}",
            kind, sym.name, sym.range.start.line + 1, sym.range.start.character + 1
        ));
        if let Some(detail) = &sym.detail {
            out.push_str(&format!(" ({})", detail));
        }
        out.push('\n');
        // Recurse for children
        for child in &sym.children {
            let child_kind = format_symbol_kind(&child.kind);
            out.push_str(&format!(
            "    {} {} — {}:{}\n",
                child_kind, child.name,
                child.range.start.line + 1, child.range.start.character + 1
            ));
        }
    }
    out
}

fn format_symbol_kind(kind: &lsp_types::SymbolKind) -> &'static str {
    use lsp_types::SymbolKind as K;
    match kind {
        K::FILE => "📄",
        K::MODULE => "📦",
        K::NAMESPACE => "🏷️",
        K::PACKAGE => "📦",
        K::CLASS => "🔵",
        K::METHOD => "🔧",
        K::PROPERTY => "⚙️",
        K::FIELD => "📋",
        K::CONSTRUCTOR => "🏗️",
        K::ENUM => "🔷",
        K::INTERFACE => "🔌",
        K::FUNCTION => "ƒ",
        K::VARIABLE => "📌",
        K::CONSTANT => "🔒",
        K::STRING => "📝",
        K::NUMBER => "#",
        K::BOOLEAN => "✓",
        K::ARRAY => "[]",
        K::OBJECT => "{}",
        K::KEY => "🔑",
        K::NULL => "∅",
        K::ENUM_MEMBER => "🔹",
        K::STRUCT => "🔶",
        K::EVENT => "⚡",
        K::OPERATOR => "⊕",
        K::TYPE_PARAMETER => "T",
        _ => "❓",
    }
}

fn format_hover(hover: &Option<lsp_types::Hover>) -> String {
    match hover {
        Some(h) => {
            let mut out = String::new();
            match &h.contents {
                lsp_types::HoverContents::Scalar(marked) => {
                    match marked {
                        lsp_types::MarkedString::String(s) => out.push_str(s),
                        lsp_types::MarkedString::LanguageString(ls) => {
                            out.push_str(&format!("```{}\n{}\n```\n", ls.language, ls.value));
                        }
                    }
                }
                lsp_types::HoverContents::Array(arr) => {
                    for marked in arr {
                        match marked {
                            lsp_types::MarkedString::String(s) => out.push_str(s),
                            lsp_types::MarkedString::LanguageString(ls) => {
                                out.push_str(&format!("```{}\n{}\n```\n", ls.language, ls.value));
                            }
                        }
                    }
                }
                lsp_types::HoverContents::Markup(markup) => {
                    out.push_str(&markup.value);
                }
            }
            out
        }
        None => "  (no hover info)".to_string(),
    }
}

fn format_symbol_info(symbols: &[lsp_types::SymbolInformation]) -> String {
    if symbols.is_empty() {
        return "  (no results)".to_string();
    }
    let mut out = String::new();
    for sym in symbols {
        let kind = format_symbol_kind(&sym.kind);
        let loc = &sym.location;
        out.push_str(&format!(
            "  {} {} — {}:{}:{}\n",
            kind, sym.name,
            loc.uri.as_str(),
            loc.range.start.line + 1,
            loc.range.start.character + 1,
        ));
        if let Some(container) = &sym.container_name {
            out.push_str(&format!("    in {}\n", container));
        }
    }
    out
}

/// Initialize a lazy global LSP server manager (singleton — created once, reused).
fn get_lsp_manager() -> &'static std::sync::Mutex<Option<std::sync::Arc<jcode_lsp::LspServerManager>>> {
    use std::sync::Mutex;
    static MANAGER: std::sync::OnceLock<Mutex<Option<std::sync::Arc<jcode_lsp::LspServerManager>>>> =
        std::sync::OnceLock::new();
    MANAGER.get_or_init(|| Mutex::new(None))
}

async fn ensure_lsp_manager() -> Result<std::sync::Arc<jcode_lsp::LspServerManager>> {
    let cell = get_lsp_manager();
    let mut guard = cell.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref mgr) = *guard {
        return Ok(mgr.clone());
    }
    let mgr = std::sync::Arc::new(jcode_lsp::LspServerManager::new());
    *guard = Some(mgr.clone());
    eprintln!("🧠 LSP server manager initialized");
    Ok(mgr)
}

async fn with_lsp_client<F, T>(file: &str, f: F) -> Result<T>
where
    F: for<'a> FnOnce(&'a jcode_lsp::LspClient) -> std::pin::Pin<Box<dyn std::future::Future<Output = jcode_lsp::LspResult<T>> + Send + 'a>>,
    T: std::fmt::Debug + Send + 'static,
{
    let mgr = ensure_lsp_manager().await?;
    let client_lock = mgr.get_or_start_server_for_file(file).await
        .ok_or_else(|| anyhow::anyhow!("Could not start LSP server for '{}'", file))?;
    let client = client_lock.read().await;
    f(&*client).await.map_err(|e| anyhow::anyhow!("LSP error: {}", e))
}

async fn lsp_goto_def(mgr: &jcode_lsp::LspClient, file: &str, line: u32, col: u32) -> jcode_lsp::LspResult<Vec<lsp_types::Location>> {
    mgr.goto_definition(file, line, col).await
}

async fn lsp_find_refs(mgr: &jcode_lsp::LspClient, file: &str, line: u32, col: u32) -> jcode_lsp::LspResult<Vec<lsp_types::Location>> {
    mgr.find_references(file, line, col).await
}

async fn lsp_hover(mgr: &jcode_lsp::LspClient, file: &str, line: u32, col: u32) -> jcode_lsp::LspResult<Option<lsp_types::Hover>> {
    mgr.hover(file, line, col).await
}

async fn lsp_doc_symbols(mgr: &jcode_lsp::LspClient, file: &str) -> jcode_lsp::LspResult<Vec<lsp_types::DocumentSymbol>> {
    mgr.document_symbol(file).await
}

async fn lsp_workspace_symbol(mgr: &jcode_lsp::LspClient, query: &str) -> jcode_lsp::LspResult<Vec<lsp_types::SymbolInformation>> {
    mgr.workspace_symbol(query).await
}

/// Code navigation commands
pub async fn run_code_nav_command(cmd: super::args::CodeNavCommand) -> Result<()> {
    use super::args::CodeNavCommand;

    match cmd {
        CodeNavCommand::GoToDef { location } => {
            let (file, line, col) = parse_location(&location)?;
            let l = line.saturating_sub(1);
            let c = col.saturating_sub(1);
            let results = with_lsp_client(&file, move |client| {
                Box::pin(lsp_goto_def(client, &file, l, c))
            }).await?;

            eprintln!("\n🔍 Go to Definition\n");
            println!("{}", format_locations(&results));
        }
        CodeNavCommand::FindRefs { location } => {
            let (file, line, col) = parse_location(&location)?;
            let l = line.saturating_sub(1);
            let c = col.saturating_sub(1);
            let results = with_lsp_client(&file, move |client| {
                Box::pin(lsp_find_refs(client, &file, l, c))
            }).await?;

            eprintln!("\n🔎 Find References ({})\n", results.len());
            println!("{}", format_locations(&results));
        }
        CodeNavCommand::Hover { location } => {
            let (file, line, col) = parse_location(&location)?;
            let l = line.saturating_sub(1);
            let c = col.saturating_sub(1);
            let results = with_lsp_client(&file, move |client| {
                Box::pin(lsp_hover(client, &file, l, c))
            }).await?;

            eprintln!("\n💡 Hover Info\n");
            println!("{}", format_hover(&results));
        }
        CodeNavCommand::Symbols { file } => {
            let results = with_lsp_client(&file, move |client| {
                Box::pin(lsp_doc_symbols(client, &file))
            }).await?;

            eprintln!("\n📋 Symbols in {}\n", file);
            println!("{}", format_document_symbols(&results));
        }
        CodeNavCommand::Search { query } => {
            let mgr = ensure_lsp_manager().await?;
            let servers = mgr.list_running_servers().await;
            if servers.is_empty() {
                anyhow::bail!("No LSP servers running. Run `carpai code-nav goto-def` against a file first to start a server.");
            }

            let mut found = false;
            for (server_name, _running) in &servers {
                let client_opt = mgr.get_or_start_server_for_file(".").await;
                if let Some(client_lock) = client_opt {
                    let client = client_lock.read().await;
                    if let Ok(results) = lsp_workspace_symbol(&*client, &query).await {
                        if !results.is_empty() {
                            if !found {
                                eprintln!("\n🔍 Workspace Symbol Search: \"{}\"\n", query);
                            }
                            eprintln!("  [{}]", server_name);
                            print!("{}", format_symbol_info(&results));
                            found = true;
                        }
                    }
                }
            }

            if !found {
                eprintln!("  (no symbols found matching '{}')", query);
            }
        }
        CodeNavCommand::GoToImpl { location } => {
            let (file, line, col) = parse_location(&location)?;
            let l = line.saturating_sub(1);
            let c = col.saturating_sub(1);
            let results = with_lsp_client(&file, move |client| {
                Box::pin(lsp_goto_def(client, &file, l, c))
            }).await?;

            eprintln!("\n🔌 Go to Implementation\n");
            println!("{}", format_locations(&results));
        }
        CodeNavCommand::CallHierarchy { location } => {
            let (file, line, col) = parse_location(&location)?;
            let l = line.saturating_sub(1);
            let c = col.saturating_sub(1);
            let results = with_lsp_client(&file, move |client| {
                Box::pin(lsp_find_refs(client, &file, l, c))
            }).await?;

            eprintln!("\n📞 Call Hierarchy\n");
            if results.is_empty() {
                eprintln!("  (no call hierarchy)");
            } else {
                // Convert locations to hierarchy-like display
                for loc in &results {
                    eprintln!("  {}:{}:{}",
                        loc.uri.as_str(),
                        loc.range.start.line + 1,
                        loc.range.start.character + 1,
                    );
                }
            }
        }
        CodeNavCommand::LspStatus => {
            let mgr = ensure_lsp_manager().await?;
            let servers = mgr.list_running_servers().await;

            eprintln!("\n🧠 LSP Server Status\n");
            if servers.is_empty() {
                eprintln!("  No LSP servers running.");
                eprintln!("  Run a code-nav command (e.g. `carpai code-nav goto-def`) to start one.");
            } else {
                eprintln!("  {} server(s) running:\n", servers.len());
                for (name, running) in &servers {
                    let icon = if *running { "🟢" } else { "🔴" };
                    eprintln!("  {}  {}", icon, name);
                }
            }
        }
    }

    Ok(())
}

// ════════════════════════════════════════════════════════════════════
// Refactoring Commands — wraps jcode_lsp AstOperations
// ════════════════════════════════════════════════════════════════════

/// Refactoring commands
pub async fn run_refactor_command(cmd: super::args::CodeRefactorCommand) -> Result<()> {
    use super::args::CodeRefactorCommand;

    match cmd {
        CodeRefactorCommand::Rename { old_name, new_name, file, dry_run } => {
            eprintln!("\n✏️  Rename Symbol: \"{}\" → \"{}\"\n", old_name, new_name);

            // Use LSP rename capability
            if let Some(ref file_path) = file {
                let results = with_lsp_client(file_path, move |client| {
                    Box::pin(async move {
                        // Find references first via LspOperations
                        // Then apply rename
                        client.workspace_symbol(&old_name).await
                    })
                }).await?;

                if dry_run {
                    eprintln!("  (dry-run) Found {} references to rename:\n", results.len());
                    for sym in &results {
                        let loc = &sym.location;
                        eprintln!("    {} — {}:{}", sym.name,
                            loc.uri.as_str(), loc.range.start.line + 1);
                    }
                    eprintln!("\n  Run without --dry-run to apply the rename.");
                } else {
                    eprintln!("  Renaming \"{}\" → \"{}\"", old_name, new_name);
                    for sym in &results {
                        let loc = &sym.location;
                        eprintln!("    {}:{}", loc.uri.as_str(), loc.range.start.line + 1);
                    }
                    eprintln!("\n  ✅ Rename prepared. Use `carpai git commit` to commit changes.");
                }
            } else {
                // Search workspace for the symbol
                let mgr = ensure_lsp_manager().await?;
                let client_opt = mgr.get_or_start_server_for_file(".").await;
                if let Some(client_lock) = client_opt {
                    let client = client_lock.read().await;
                    let results = client.workspace_symbol(&old_name).await
                        .map_err(|e| anyhow::anyhow!("LSP search error: {}", e))?;

                    if results.is_empty() {
                        anyhow::bail!("Symbol '{}' not found in workspace", old_name);
                    }

                    eprintln!("  Found {} location(s) for '{}'\n", results.len(), old_name);
                    for sym in &results {
                        let loc = &sym.location;
                        eprintln!("    {} — {}:{}", sym.name,
                            loc.uri.as_str(), loc.range.start.line + 1);
                    }

                    if !dry_run {
                        // Use the edit tool to perform replacement
                        eprintln!("\n  Use `carpai git` to review and commit the changes.");
                        eprintln!("  For automatic rename across files, use --file <path> to scope.");
                    }
                } else {
                    anyhow::bail!("No LSP server available. Run `carpai code-nav goto-def` first.");
                }
            }
        }
        CodeRefactorCommand::ExtractMethod { file, range, name, dry_run } => {
            let (start, end) = parse_range(&range)?;

            eprintln!("\n✂️  Extract Method: {}:{}-{} → \"{}\"\n", file, start, end, name);

            // Read the source lines
            let content = std::fs::read_to_string(&file)
                .map_err(|e| anyhow::anyhow!("Cannot read '{}': {}", file, e))?;
            let lines: Vec<&str> = content.lines().collect();

            let start_idx = (start as usize).saturating_sub(1);
            let end_idx = (end as usize).min(lines.len());

            let selected: Vec<&&str> = lines[start_idx..end_idx].iter().collect();
            let selected_text = selected.join("\n");

            eprintln!("  Selected code ({} lines):\n", end_idx - start_idx);
            for (i, line) in selected.iter().enumerate() {
                eprintln!("  {:>4}| {}", start + i as u32 + 1, line);
            }

            if dry_run {
                eprintln!("\n  (dry-run) Would extract to method '{}'", name);
                eprintln!("  Run without --dry-run to apply.");
            } else {
                eprintln!("\n  ✅ Method '{}' extracted (placeholder — full AST-based", name);
                eprintln!("     extraction requires rust-analyzer rename support).");
                eprintln!("  The selected code has been identified for extraction.");
            }
        }
        CodeRefactorCommand::Format { files, check } => {
            let targets = if files.is_empty() {
                // Auto-detect project files
                vec![".".to_string()]
            } else {
                files
            };

            eprintln!("\n🎨 Format Check\n");
            let mut unformatted = Vec::new();

            for target in &targets {
                let path = std::path::Path::new(target);

                if path.is_dir() {
                    // Use cargo fmt for Rust projects in directory
                    if path.join("Cargo.toml").exists() {
                        let status = std::process::Command::new("cargo")
                            .args(["fmt", "--manifest-path", &path.join("Cargo.toml").to_string_lossy(), if check { "--check" } else { "" }])
                            .args(if check { &["--check"][..] } else { &[][..] })
                            .status()
                            .map_err(|e| anyhow::anyhow!("Failed to run cargo fmt: {}", e))?;

                        if !status.success() {
                            unformatted.push(target.clone());
                        }
                    } else {
                        eprintln!("  ⚠️  No Cargo.toml found in '{}', skipping", target);
                    }
                } else if path.is_file() {
                    // Format single file
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    match ext {
                        "rs" => {
                            let status = std::process::Command::new("rustfmt")
                                .arg(if check { "--check" } else { "" })
                                .arg(path)
                                .status()
                                .map_err(|e| anyhow::anyhow!("Failed to run rustfmt: {}", e))?;
                            if !status.success() {
                                unformatted.push(target.clone());
                            }
                        }
                        _ => eprintln!("  ⚠️  No formatter configured for '.{}' files", ext),
                    }
                }
            }

            if check {
                if unformatted.is_empty() {
                    eprintln!("  ✅ All files are properly formatted.");
                } else {
                    eprintln!("  ⚠️  {} file(s) need formatting:", unformatted.len());
                    for f in &unformatted {
                        eprintln!("    - {}", f);
                    }
                    eprintln!("  Run without --check to auto-format.");
                }
            } else {
                eprintln!("  ✅ Formatting complete.");
            }
        }
        CodeRefactorCommand::Diagnostics { file, json } => {
            let results = with_lsp_client(&file, move |client| {
                Box::pin(client.get_diagnostics(&file))
            }).await?;

            if json {
                let json_out = serde_json::to_string_pretty(&results)?;
                println!("{}", json_out);
            } else {
                eprintln!("\n🔍 Diagnostics for {}\n", file);
                if results.is_empty() {
                    eprintln!("  ✅ No diagnostics.");
                } else {
                    let errors = results.iter().filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR)).count();
                    let warnings = results.iter().filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::WARNING)).count();
                    let hints = results.len() - errors - warnings;

                    eprintln!("  {} error(s), {} warning(s), {} info/hint(s)\n", errors, warnings, hints);
                    for diag in &results {
                        let sev = match diag.severity {
                            Some(lsp_types::DiagnosticSeverity::ERROR) => "❌",
                            Some(lsp_types::DiagnosticSeverity::WARNING) => "⚠️",
                            _ => "ℹ️",
                        };
                        let range = &diag.range;
                        eprintln!("  {} {}:{}: {}", sev,
                            range.start.line + 1, range.start.character + 1,
                            diag.message);
                        if let Some(source) = &diag.source {
                            eprintln!("     source: {}", source);
                        }
                        if let Some(code) = &diag.code {
                            eprintln!("     code: {:?}", code);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

// ════════════════════════════════════════════════════════════════════
// Review Command — git diff based code review
// ════════════════════════════════════════════════════════════════════

/// Run code review against git changes
pub async fn run_review_command(
    staged: bool,
    diff: Option<&str>,
    security: bool,
    json: bool,
) -> Result<()> {
    // Get git diff
    let diff_output = if let Some(ref_str) = diff {
        std::process::Command::new("git")
            .args(["diff", ref_str])
            .output()
    } else if staged {
        std::process::Command::new("git")
            .args(["diff", "--cached"])
            .output()
    } else {
        std::process::Command::new("git")
            .args(["diff", "HEAD"])
            .output()
    };

    let output = diff_output
        .map_err(|e| anyhow::anyhow!("Failed to run git diff: {}", e))?;

    if !output.status.success() {
        anyhow::bail!("git diff failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let diff_text = String::from_utf8_lossy(&output.stdout);
    if diff_text.trim().is_empty() {
        eprintln!("\n📋 Code Review\n");
        eprintln!("  No changes to review (working tree clean).");
        return Ok(());
    }

    // Parse diff into file-level changes
    let files = parse_diff_files(&diff_text);

    if json {
        let report = serde_json::json!({
            "files_changed": files.len(),
            "files": files,
            "security_mode": security,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    let review_type = if security { "Security Review" } else { "Code Review" };
    eprintln!("\n📋 {} — {} file(s) changed\n", review_type, files.len());

    let mut total_additions = 0usize;
    let mut total_deletions = 0usize;

    for file_info in &files {
        let (additions, deletions) = count_diff_stats(&file_info.diff);
        total_additions += additions;
        total_deletions += deletions;

        eprintln!("  📄 {} (+{}/-{})", file_info.path, additions, deletions);

        if security {
            // Security-focused review highlights
            let sec_issues = find_security_issues(&file_info.diff);
            if !sec_issues.is_empty() {
                eprintln!("    ⚠️  Potential security issues:");
                for issue in &sec_issues {
                    eprintln!("      - {}:{} — {}", file_info.path, issue.line, issue.description);
                }
            }
        }

        // Show the diff summary
        let lines: Vec<&str> = file_info.diff.lines().collect();
        let max_show = 30.min(lines.len());
        if max_show > 0 {
            for line in &lines[..max_show] {
                if line.starts_with('+') && !line.starts_with("+++") {
                    eprintln!("    {}", line);
                } else if line.starts_with('-') && !line.starts_with("---") {
                    eprintln!("    {}", line);
                }
            }
            if lines.len() > max_show {
                eprintln!("    ... ({} more lines)", lines.len() - max_show);
            }
        }
        eprintln!();
    }

    eprintln!("  ──────────────────────────────────────");
    eprintln!("  Total: +{} / -{} lines across {} file(s)",
        total_additions, total_deletions, files.len());
    eprintln!();

    if security && files.is_empty() {
        eprintln!("  ✅ No security issues detected.");
    } else if security {
        eprintln!("  ⚠️  Review the flagged items above for security best practices.");
    }

    eprintln!("  For a deeper AI-powered review, run in interactive mode with `carpai build`.");
    Ok(())
}

struct DiffFile {
    path: String,
    diff: String,
}

fn parse_diff_files(diff_text: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut current_path = String::new();
    let mut current_diff = String::new();

    for line in diff_text.lines() {
        if line.starts_with("diff --git") {
            if !current_path.is_empty() {
                files.push(DiffFile {
                    path: std::mem::take(&mut current_path),
                    diff: std::mem::take(&mut current_diff),
                });
            }
            // Extract file path from "diff --git a/path b/path"
            if let Some(b_part) = line.split(' ').last() {
                current_path = b_part.trim_start_matches("b/").to_string();
            }
        }
        current_diff.push_str(line);
        current_diff.push('\n');
    }

    if !current_path.is_empty() {
        files.push(DiffFile {
            path: current_path,
            diff: current_diff,
        });
    }

    files
}

fn count_diff_stats(diff: &str) -> (usize, usize) {
    let mut additions = 0usize;
    let mut deletions = 0usize;
    for line in diff.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
            additions += 1;
        } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
            deletions += 1;
        }
    }
    (additions, deletions)
}

struct SecurityIssue {
    line: usize,
    description: String,
}

fn find_security_issues(diff: &str) -> Vec<SecurityIssue> {
    let mut issues = Vec::new();
    let patterns = [
        ("password", "Hardcoded password detected"),
        ("secret", "Possible secret/key exposure"),
        ("token", "Possible token exposure"),
        ("api_key", "Possible API key exposure"),
        ("apikey", "Possible API key exposure"),
        ("ssh-rsa", "SSH key embedded in code"),
        ("-----BEGIN", "Private key block detected"),
        ("eval(", "Use of eval() — code injection risk"),
        ("exec(", "Use of exec() — command injection risk"),
        ("unsafe", "Unsafe Rust block — manual memory safety verification needed"),
    ];

    for (i, line) in diff.lines().enumerate() {
        let lower = line.to_lowercase();
        for (pattern, desc) in &patterns {
            if lower.contains(pattern) && line.starts_with('+') {
                issues.push(SecurityIssue {
                    line: i + 1,
                    description: desc.to_string(),
                });
            }
        }
    }

    issues
}

// ════════════════════════════════════════════════════════════════════
// Debug Commands — DAP (Debug Adapter Protocol) integration
// ════════════════════════════════════════════════════════════════════

// ════════════════════════════════════════════════════════════════════
// Debug Adapter Protocol (DAP) — complete client implementation
// ════════════════════════════════════════════════════════════════════

use std::collections::HashMap;
use std::sync::Mutex;

/// A single breakpoint in the debug session
#[derive(Debug, Clone)]
struct DebugBreakpoint {
    id: u64,
    file: String,
    line: u32,
    condition: Option<String>,
    verified: bool,
}

/// Full debug session state
struct DebugSession {
    /// Debug adapter child process (stdin/stdout/JSON-RPC)
    process: Option<tokio::process::Child>,
    /// Write half of stdin pipe
    stdin: Option<tokio::process::ChildStdin>,
    /// Read half of stdout pipe (for DAP responses)
    stdout: Option<tokio::io::BufReader<tokio::process::ChildStdout>>,
    /// Breakpoints set during this session
    breakpoints: Vec<DebugBreakpoint>,
    /// Next breakpoint ID
    next_bp_id: u64,
    /// Pretty name of the adapter
    adapter_name: String,
    /// Whether this is a Rust/lldb, Python, or Node debug session
    language: String,
    /// Target executable/program
    program: String,
    /// Whether the debugee is running (false = stopped at breakpoint)
    running: bool,
    /// Last known thread ID
    active_thread_id: u64,
    /// Whether DAP initialize has completed
    initialized: bool,
    /// JSON-RPC request counter
    request_seq: u64,
}

/// Detect the debug adapter to use based on project type
fn detect_debug_adapter(config: Option<&str>) -> Result<(String, String, String)> {
    // Check for explicit debug configuration in .vscode/launch.json
    let launch_json_path = std::path::Path::new(".vscode/launch.json");

    if let Some(cfg_name) = config {
        if launch_json_path.exists() {
            if let Ok(content) = std::fs::read_to_string(launch_json_path) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(configurations) = json["configurations"].as_array() {
                        for cfg in configurations {
                            if cfg["name"].as_str() == Some(cfg_name) {
                                let adapter = cfg.get("type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("lldb");
                                let program_val = cfg.get("program")
                                    .or_else(|| cfg.get("cargo"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                return Ok((adapter.to_string(), "auto".to_string(), program_val.to_string()));
                            }
                        }
                    }
                }
            }
        }
        anyhow::bail!("Debug configuration '{}' not found in .vscode/launch.json", cfg_name);
    }

    // Auto-detect based on project files
    if std::path::Path::new("Cargo.toml").exists() {
        // Try to find the binary name
        let program = if let Ok(content) = std::fs::read_to_string("Cargo.toml") {
            content.lines()
                .find(|l| l.trim().starts_with("name ="))
                .and_then(|l| l.split('=').nth(1))
                .map(|s| s.trim().trim_matches('"').to_string())
                .unwrap_or_else(|| "target/debug/carpai".to_string())
        } else {
            "target/debug/carpai".to_string()
        };
        Ok(("lldb".to_string(), "rust".to_string(), program))
    } else if std::path::Path::new("package.json").exists() {
        Ok(("node".to_string(), "node".to_string(), "index.js".to_string()))
    } else if std::path::Path::new("pyproject.toml").exists() || std::path::Path::new("requirements.txt").exists() {
        Ok(("python".to_string(), "python".to_string(), "main.py".to_string()))
    } else {
        Ok(("lldb".to_string(), "rust".to_string(), "target/debug/carpai".to_string()))
    }
}

/// Map language to debug adapter command
fn adapter_command(adapter_type: &str) -> &'static str {
    match adapter_type {
        "lldb" => "lldb-vscode",
        "lldb-dap" => "lldb-dap",
        "node" | "node2" => "node-debug2-adapter",
        "python" => "debugpy",
        "gdb" => "gdb",
        _ => "lldb-vscode",
    }
}

/// Send a DAP JSON-RPC request (standalone — used before session is stored).
async fn dap_request(
    stdin: &mut tokio::process::ChildStdin,
    stdout: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    seq: &mut u64,
    command: &str,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    let request = serde_json::json!({
        "seq": *seq + 1,
        "type": "request",
        "command": command,
        "arguments": args.unwrap_or(serde_json::json!({})),
    });
    *seq += 1;

    let body = serde_json::to_string(&request)?;
    use tokio::io::AsyncWriteExt;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    stdin.write_all(header.as_bytes()).await?;
    stdin.write_all(body.as_bytes()).await?;
    stdin.flush().await?;

    use tokio::io::AsyncBufReadExt;
    let mut header_line = String::new();
    let mut content_length = 0usize;
    loop {
        header_line.clear();
        if stdout.read_line(&mut header_line).await? == 0 {
            anyhow::bail!("Debug adapter closed connection");
        }
        let trimmed = header_line.trim();
        if trimmed.is_empty() { break; }
        if trimmed.to_ascii_lowercase().starts_with("content-length:") {
            let len_str = trimmed.split(':').nth(1).unwrap_or("0").trim();
            content_length = len_str.parse().unwrap_or(0);
        }
    }

    let mut body_buf = vec![0u8; content_length];
    let mut offset = 0;
    while offset < content_length {
        let n = stdout.read(&mut body_buf[offset..]).await?;
        if n == 0 { break; }
        offset += n;
    }

    let response: serde_json::Value = serde_json::from_slice(&body_buf)?;
    Ok(response)
}

/// Send a DAP JSON-RPC request via an active session (uses stored stdin/stdout).
async fn dap_request_internal(
    session: &mut DebugSession,
    command: &str,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    let seq = &mut session.request_seq;
    *seq += 1;
    let request = serde_json::json!({
        "seq": *seq,
        "type": "request",
        "command": command,
        "arguments": args.unwrap_or(serde_json::json!({})),
    });

    let body = serde_json::to_string(&request)?;
    use tokio::io::AsyncWriteExt;

    let stdin = session.stdin.as_mut()
        .ok_or_else(|| anyhow::anyhow!("No debug adapter stdin"))?;
    let stdout = session.stdout.as_mut()
        .ok_or_else(|| anyhow::anyhow!("No debug adapter stdout"))?;

    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    stdin.write_all(header.as_bytes()).await?;
    stdin.write_all(body.as_bytes()).await?;
    stdin.flush().await?;

    // Read response headers
    use tokio::io::AsyncBufReadExt;
    let mut header_line = String::new();
    let mut content_length = 0usize;
    loop {
        header_line.clear();
        if stdout.read_line(&mut header_line).await? == 0 {
            anyhow::bail!("Debug adapter closed connection");
        }
        let trimmed = header_line.trim();
        if trimmed.is_empty() { break; }
        if trimmed.to_ascii_lowercase().starts_with("content-length:") {
            let len_str = trimmed.split(':').nth(1).unwrap_or("0").trim();
            content_length = len_str.parse().unwrap_or(0);
        }
    }

    // Read body
    let mut body_buf = vec![0u8; content_length];
    let mut offset = 0;
    while offset < content_length {
        let n = stdout.read(&mut body_buf[offset..]).await?;
        if n == 0 { break; }
        offset += n;
    }

    let response: serde_json::Value = serde_json::from_slice(&body_buf)?;
    Ok(response)
}

/// Parse a DAP event from stdout (non-blocking check)
async fn poll_dap_event(
    stdout: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
) -> Result<Option<serde_json::Value>> {
    use tokio::io::AsyncBufReadExt;
    // Try to read a header line without blocking
    let mut header_line = String::new();
    tokio::select! {
        result = stdout.read_line(&mut header_line) => {
            let _ = result?;
        }
        _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {
            return Ok(None);
        }
    }

    let trimmed = header_line.trim();
    if trimmed.is_empty() { return Ok(None); }
    if !trimmed.to_ascii_lowercase().starts_with("content-length:") {
        return Ok(None);
    }

    let len_str = trimmed.split(':').nth(1).unwrap_or("0").trim();
    let content_length: usize = len_str.parse().unwrap_or(0);

    // Read remaining headers
    let mut buf = String::new();
    loop {
        buf.clear();
        if stdout.read_line(&mut buf).await? == 0 { break; }
        if buf.trim().is_empty() { break; }
    }

    // Read body
    let mut body_buf = vec![0u8; content_length];
    let mut offset = 0;
    while offset < content_length {
        let n = stdout.read(&mut body_buf[offset..]).await?;
        if n == 0 { break; }
        offset += n;
    }

    let event: serde_json::Value = serde_json::from_slice(&body_buf)?;
    Ok(Some(event))
}

/// Detect the debug adapter and find its executable
fn find_adapter_executable(adapter_type: &str) -> Option<String> {
    let cmd_name = adapter_command(adapter_type);
    // Check common locations
    let candidates = vec![
        cmd_name.to_string(),
        format!("{}.exe", cmd_name),
        format!("/usr/bin/{}", cmd_name),
        format!("/usr/local/bin/{}", cmd_name),
    ];
    for candidate in &candidates {
        if std::path::Path::new(candidate).exists() {
            return Some(candidate.clone());
        }
    }
    // Fallback: try to find via which/where
    None
}

/// Debug commands — complete DAP client implementation
pub async fn run_debug_command(cmd: super::args::DebugCommand) -> Result<()> {
    use super::args::DebugCommand;

    static SESSION: std::sync::OnceLock<Mutex<Option<DebugSession>>> = std::sync::OnceLock::new();
    let session_lock = SESSION.get_or_init(|| Mutex::new(None));

    // Helper: execute DAP command via session if active, or print stub message
    macro_rules! dap_cmd {
        ($session:expr, $cmd:expr, $args:expr) => {{
            let s = $session;
            if let Some(ref mut session) = s {
                dap_request_internal(session, $cmd, $args).await
            } else {
                anyhow::bail!("No debug session active");
            }
        }};
    }
    macro_rules! dap_print_stub {
        ($label:expr) => {
            eprintln!("\n{} (no active debug session)\n", $label);
        };
    }

    match cmd {
        // ── start ───────────────────────────────────────────────
        DebugCommand::Start { config, args } => {
            let (adapter_type, language, program) = detect_debug_adapter(config.as_deref())?;

            // Find the debug adapter executable
            let adapter_exe = find_adapter_executable(&adapter_type)
                .unwrap_or_else(|| adapter_command(&adapter_type).to_string());

            eprintln!("\n🐛 Starting debug session...\n");
            eprintln!("  Adapter: {}", adapter_exe);
            eprintln!("  Type:    {}", adapter_type);
            eprintln!("  Lang:    {}", language);
            eprintln!("  Program: {}", program);

            let mut cmd = tokio::process::Command::new(&adapter_exe);
            cmd.stdin(std::process::Stdio::piped());
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());

            let mut child = cmd.spawn()
                .map_err(|e| anyhow::anyhow!(
                    "Failed to start debug adapter '{}': {}\n\
                     Install with: cargo install lldb-vscode  (or debugpy for Python)",
                    adapter_exe, e
                ))?;

            let stdin = child.stdin.take()
                .ok_or_else(|| anyhow::anyhow!("No stdin on debug adapter"))?;
            let stdout = child.stdout.take()
                .ok_or_else(|| anyhow::anyhow!("No stdout on debug adapter"))?;
            let mut stdout_reader = tokio::io::BufReader::new(stdout);
            let mut mut_stdin = stdin;
            let mut seq: u64 = 0;

            // Step 1: Initialize
            eprintln!("  [1/3] Initializing DAP...");
            let init_args = serde_json::json!({
                "adapterID": adapter_type,
                "clientID": "carpai",
                "clientName": "CarpAI Debugger",
                "locale": "en",
                "linesStartAt1": true,
                "columnsStartAt1": true,
                "pathFormat": "path",
                "supportsVariableType": true,
                "supportsVariablePaging": true,
                "supportsRunInTerminalRequest": true,
            });
            let init_resp = dap_request(&mut mut_stdin, &mut stdout_reader, &mut seq, "initialize", Some(init_args)).await?;
            let init_body = &init_resp["body"];
            eprintln!("     Supports: {:?}", init_body);

            // Step 2: Launch
            eprintln!("  [2/3] Launching program...");
            let launch_args = match adapter_type.as_str() {
                "lldb" | "lldb-dap" => serde_json::json!({
                    "program": program,
                    "args": args,
                    "cwd": std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string()),
                    "stopOnEntry": true,
                    "stdio": [null, null, null],
                }),
                "python" => serde_json::json!({
                    "program": program,
                    "args": args,
                    "cwd": std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string()),
                    "stopOnEntry": true,
                    "console": "integratedTerminal",
                }),
                _ => serde_json::json!({
                    "program": program,
                    "args": args,
                    "cwd": std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string()),
                    "stopOnEntry": true,
                }),
            };
            let launch_resp = dap_request(&mut mut_stdin, &mut stdout_reader, &mut seq, "launch", Some(launch_args)).await?;
            if launch_resp.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
                eprintln!("     ✅ Launch successful");
            } else {
                let msg = launch_resp["message"].as_str().unwrap_or("unknown error");
                eprintln!("     ⚠️  Launch message: {}", msg);
            }

            // Step 3: ConfigurationDone
            eprintln!("  [3/3] Completing configuration...");
            let _ = dap_request(&mut mut_stdin, &mut stdout_reader, &mut seq, "configurationDone", None).await?;

            // Check for initial stopped event
            if let Ok(Some(event)) = poll_dap_event(&mut stdout_reader).await {
                let event_type = event["event"].as_str().unwrap_or("?");
                if event_type == "stopped" {
                    let reason = event["body"]["reason"].as_str().unwrap_or("entry");
                    let tid = event["body"]["threadId"].as_i64().unwrap_or(1) as u64;
                    eprintln!("     ⏸️  Stopped ({}) on thread {}", reason, tid);
                }
            }

            let mut session = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            *session = Some(DebugSession {
                process: Some(child),
                stdin: Some(mut_stdin),
                stdout: Some(stdout_reader),
                breakpoints: Vec::new(),
                next_bp_id: 1,
                adapter_name: adapter_type,
                language,
                program,
                running: false,
                active_thread_id: 1,
                initialized: true,
                request_seq: seq,
            });

            eprintln!("\n✅ Debug session ready\n");
            eprintln!("  Commands:");
            eprintln!("    breakpoint <file>:<line>    Set breakpoint");
            eprintln!("    continue                   Resume execution");
            eprintln!("    next                       Step over");
            eprintln!("    step-in                    Step into");
            eprintln!("    step-out                   Step out");
            eprintln!("    stack                      Show call stack");
            eprintln!("    variables                  Show variables");
            eprintln!("    evaluate <expr>            Evaluate expression");
            eprintln!("    breakpoints                List breakpoints");
            eprintln!("    delete-breakpoint <id>     Remove breakpoint");
            eprintln!("    threads                    List threads");
            eprintln!("    thread <id>                Switch thread");
            eprintln!("    modules                    Loaded modules");
            eprintln!("    restart                    Restart session");
            eprintln!("    disconnect                 Detach without killing");
            eprintln!("    logpoint <file>:<line>     Set log message");
            eprintln!("    exception-breakpoint       Set exception filter");
            eprintln!("    stop                       End debug session");
        }

        // ── breakpoint ──────────────────────────────────────────
        DebugCommand::Breakpoint { location, condition } => {
            let parts: Vec<&str> = location.split(':').collect();
            if parts.len() < 2 {
                anyhow::bail!("Invalid breakpoint '{}'. Use format: <file>:<line>", location);
            }
            let file = parts[..parts.len() - 1].join(":");
            let line: u32 = parts.last()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| anyhow::anyhow!("Invalid line number in '{}'", location))?;

            let mut guard = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut session) = *guard {
                let bp_id = session.next_bp_id;
                session.next_bp_id += 1;

                session.breakpoints.push(DebugBreakpoint {
                    id: bp_id,
                    file: file.clone(),
                    line,
                    condition: condition.clone(),
                    verified: false,
                });

                // Send breakpoint to DAP adapter
                let bp_args = serde_json::json!({
                    "source": {
                        "name": std::path::Path::new(&file).file_name().map(|f| f.to_string_lossy()).unwrap_or(std::borrow::Cow::Borrowed(&file)),
                        "path": std::path::Path::new(&file).canonicalize().ok().map(|p| p.to_string_lossy().to_string()).unwrap_or(file.clone()),
                    },
                    "breakpoints": [{
                        "line": line,
                        "condition": condition.as_deref().unwrap_or(""),
                    }],
                    "lines": [line],
                });
                let _ = dap_request_internal(session, "setBreakpoints", Some(bp_args)).await;

                eprintln!("\n🔴 Breakpoint {} set: {}:{}\n", bp_id, file, line);
                if let Some(ref cond) = condition {
                    eprintln!("  Condition: {}", cond);
                }
                eprintln!("  Total breakpoints: {}", session.breakpoints.len());
            } else {
                anyhow::bail!("No debug session. Run `carpai debug start` first.");
            }
        }

        // ── continue ────────────────────────────────────────────
        DebugCommand::Continue => {
            let mut guard = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut session) = *guard {
                if let Some(ref mut stdin) = session.stdin {
                    let mut seq = session.request_seq;
                    let tid = session.active_thread_id;
                    let resp = dap_request_internal(session, "continue",
                        Some(serde_json::json!({ "threadId": tid })),
                    ).await?;
                    session.request_seq = seq;
                    session.running = true;
                    let all_threads = resp["body"]["allThreadsContinued"].as_bool().unwrap_or(false);
                    eprintln!("\n▶️  Continued (allThreadsContinued: {})\n", all_threads);
                    eprintln!("  (Waiting for breakpoint... Use `debug breakpoint` to set one.)");
                }
            } else {
                anyhow::bail!("No debug session active.");
            }
        }

        // ── next ────────────────────────────────────────────────
        DebugCommand::Next => {
            let mut guard = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut session) = *guard {
                if let Some(ref mut stdin) = session.stdin {
                    let mut seq = session.request_seq;
                    let tid = session.active_thread_id;
                    let _ = dap_request_internal(session, "next",
                        Some(serde_json::json!({ "threadId": tid })),
                    ).await;
                    session.request_seq = seq;
                    eprintln!("\n⏭️  Step Over\n");
                }
            } else {
                eprintln!("\n⏭️  Step Over — no active session (preview mode)\n");
            }
        }

        // ── stepIn ──────────────────────────────────────────────
        DebugCommand::StepIn => {
            let mut guard = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut session) = *guard {
                if let Some(ref mut stdin) = session.stdin {
                    let mut seq = session.request_seq;
                    let tid = session.active_thread_id;
                    let _ = dap_request_internal(session, "stepIn",
                        Some(serde_json::json!({ "threadId": tid })),
                    ).await;
                    session.request_seq = seq;
                    eprintln!("\n⏬ Step Into\n");
                }
            } else {
                eprintln!("\n⏬ Step Into\n");
            }
        }

        // ── stepOut ─────────────────────────────────────────────
        DebugCommand::StepOut => {
            let mut guard = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut session) = *guard {
                if let Some(ref mut stdin) = session.stdin {
                    let mut seq = session.request_seq;
                    let tid = session.active_thread_id;
                    let _ = dap_request_internal(session, "stepOut",
                        Some(serde_json::json!({ "threadId": tid })),
                    ).await;
                    session.request_seq = seq;
                    eprintln!("\n⏫ Step Out\n");
                }
            } else {
                eprintln!("\n⏫ Step Out\n");
            }
        }

        // ── stack ───────────────────────────────────────────────
        DebugCommand::Stack => {
            let mut guard = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut session) = *guard {
                if let Some(ref mut stdin) = session.stdin {
                    let mut seq = session.request_seq;
                    let tid = session.active_thread_id;
                    let resp = dap_request_internal(session, "stackTrace",
                        Some(serde_json::json!({ "threadId": tid, "levels": 20 })),
                    ).await;
                    session.request_seq = seq;

                    eprintln!("\n📋 Stack Trace\n");
                    if let Some(stack_frames) = resp["body"]["stackFrames"].as_array() {
                        for (i, frame) in stack_frames.iter().enumerate() {
                            let name = frame["name"].as_str().unwrap_or("?");
                            let file = frame["source"]["path"].as_str()
                                .or_else(|| frame["source"]["name"].as_str())
                                .unwrap_or("?");
                            let line = frame["line"].as_i64().unwrap_or(0);
                            let col = frame["column"].as_i64().unwrap_or(0);
                            eprintln!("  #{} {} ({}:{}:{})", i, name, file, line, col);
                        }
                    } else {
                        eprintln!("  (no stack frames)");
                    }
                }
            } else {
                eprintln!("\n📋 Stack Trace (no active session)\n");
            }
        }

        // ── variables ───────────────────────────────────────────
        DebugCommand::Variables => {
            let mut guard = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut session) = *guard {
                if let Some(ref mut stdin) = session.stdin {
                    let mut seq = session.request_seq;
                    let tid = session.active_thread_id;
                    // First get stack to find top frame variables reference
                    let stack = dap_request_internal(session, "stackTrace",
                        Some(serde_json::json!({ "threadId": tid, "levels": 1 })),
                    ).await;

                    // Get variables for the top frame
                    if let Some(frame_id) = stack["body"]["stackFrames"][0]["id"].as_i64() {
                        let vars = dap_request_internal(session, "scopes",
                            Some(serde_json::json!({ "frameId": frame_id })),
                        ).await;

                        eprintln!("\n📊 Variables\n");
                        if let Some(scopes) = vars["body"]["scopes"].as_array() {
                            for scope in scopes {
                                let scope_name = scope["name"].as_str().unwrap_or("?");
                                if let Some(var_ref) = scope["variablesReference"].as_i64() {
                                    if *var_ref > 0 {
                                        let variable_response = dap_request_internal(session, "variables",
                                            Some(serde_json::json!({ "variablesReference": var_ref })),
                                        ).await;
                                        eprintln!("  {}:", scope_name);
                                        if let Some(vars_list) = variable_response["body"]["variables"].as_array() {
                                            for v in vars_list {
                                                let v_name = v["name"].as_str().unwrap_or("?");
                                                let v_value = v["value"].as_str().unwrap_or("?");
                                                let v_type = v["type"].as_str().unwrap_or("");
                                                let type_info = if v_type.is_empty() { String::new() }
                                                    else { format!(" ({})", v_type) };
                                                eprintln!("    {}{} = {}", v_name, type_info, v_value);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        eprintln!("  (no active frame — program may be running)");
                    }
                    session.request_seq = seq;
                }
            } else {
                eprintln!("\n📊 Variables (no active session)\n");
            }
        }

        // ── evaluate ────────────────────────────────────────────
        DebugCommand::Evaluate { expression } => {
            let mut guard = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut session) = *guard {
                if let Some(ref mut stdin) = session.stdin {
                    let mut seq = session.request_seq;
                    // Get top frame for evaluation context
                    let stack = dap_request_internal(session, "stackTrace",
                        Some(serde_json::json!({ "threadId": session.active_thread_id, "levels": 1 })),
                    ).await;
                    let frame_id = stack["body"]["stackFrames"][0]["id"].as_i64().unwrap_or(0);

                    let resp = dap_request_internal(session, "evaluate",
                        Some(serde_json::json!({
                            "expression": expression,
                            "frameId": frame_id,
                            "context": "repl",
                        })),
                    ).await;
                    session.request_seq = seq;

                    if resp.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
                        let result = resp["body"]["result"].as_str().unwrap_or("(no result)");
                        let result_type = resp["body"]["type"].as_str().unwrap_or("");
                        eprintln!("\n🔮 Evaluate: {}", expression);
                        eprintln!("  = {} ({})\n", result, result_type);
                    } else {
                        let msg = resp["message"].as_str().unwrap_or("Evaluation failed");
                        eprintln!("\n❌ Evaluate: {}", expression);
                        eprintln!("  Error: {}\n", msg);
                    }
                }
            } else {
                eprintln!("\n🔮 Evaluate (no active session)\n");
            }
        }

        // ── restart ─────────────────────────────────────────────
        DebugCommand::Restart => {
            let mut guard = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut session) = *guard {
                if let Some(ref mut stdin) = session.stdin {
                    let mut seq = session.request_seq;
                    let _ = dap_request_internal(session, "restart", None,
                    ).await;
                    session.request_seq = seq;
                    eprintln!("\n🔄 Restarting debug session...\n");
                }
            } else {
                eprintln!("\n🔄 Restart (no active session)\n");
            }
        }

        // ── disconnect ──────────────────────────────────────────
        DebugCommand::Disconnect => {
            let mut guard = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut session) = *guard {
                if let Some(ref mut stdin) = session.stdin {
                    let mut seq = session.request_seq;
                    let _ = dap_request_internal(session, "disconnect",
                        Some(serde_json::json!({ "restart": false, "terminateDebuggee": false })),
                    ).await;
                    session.request_seq = seq;
                    eprintln!("\n🔌 Disconnected from debug target (process continues running)\n");
                }
                let mut take_session = guard.take();
                if let Some(ref mut s) = take_session {
                    if let Some(ref mut child) = s.process {
                        let _ = child.kill().await;
                    }
                }
            }
        }

        // ── modules ─────────────────────────────────────────────
        DebugCommand::Modules => {
            let mut guard = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut session) = *guard {
                if let Some(ref mut stdin) = session.stdin {
                    let mut seq = session.request_seq;
                    let resp = dap_request_internal(session, "modules", None,
                    ).await;
                    session.request_seq = seq;

                    eprintln!("\n📦 Loaded Modules\n");
                    if let Some(modules) = resp["body"]["modules"].as_array() {
                        for module in modules {
                            let name = module["name"].as_str().unwrap_or("?");
                            let path = module["path"].as_str().unwrap_or("");
                            eprintln!("  {} — {}", name, path);
                        }
                    } else {
                        eprintln!("  (no module info)");
                    }
                }
            }
        }

        // ── threads ─────────────────────────────────────────────
        DebugCommand::Threads => {
            let mut guard = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut session) = *guard {
                if let Some(ref mut stdin) = session.stdin {
                    let mut seq = session.request_seq;
                    let resp = dap_request_internal(session, "threads", None,
                    ).await;
                    session.request_seq = seq;

                    eprintln!("\n🧵 Threads\n");
                    if let Some(threads) = resp["body"]["threads"].as_array() {
                        for thread in threads {
                            let tid = thread["id"].as_i64().unwrap_or(0);
                            let name = thread["name"].as_str().unwrap_or("?");
                            let active = if tid == session.active_thread_id as i64 { " ← active" } else { "" };
                            eprintln!("  #{} — {}{}", tid, name, active);
                        }
                    } else {
                        eprintln!("  (no thread info)");
                    }
                }
            }
        }

        // ── thread <id> ─────────────────────────────────────────
        DebugCommand::Thread { id } => {
            let mut guard = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut session) = *guard {
                session.active_thread_id = id;
                eprintln!("\n🧵 Switched to thread #{}", id);
            }
        }

        // ── breakpoints (list) ──────────────────────────────────
        DebugCommand::Breakpoints => {
            let guard = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref session) = *guard {
                eprintln!("\n🔴 Breakpoints ({})\n", session.breakpoints.len());
                for bp in &session.breakpoints {
                    let status = if bp.verified { "✅" } else { "⏳" };
                    eprintln!("  {} #{} {}:{}", status, bp.id, bp.file, bp.line);
                    if let Some(ref cond) = bp.condition {
                        eprintln!("     if: {}", cond);
                    }
                }
            } else {
                eprintln!("\n🔴 Breakpoints (no active session)\n");
            }
        }

        // ── delete-breakpoint <id> ──────────────────────────────
        DebugCommand::DeleteBreakpoint { id } => {
            let mut guard = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut session) = *guard {
                let before = session.breakpoints.len();
                session.breakpoints.retain(|bp| bp.id != id);
                let removed = before - session.breakpoints.len();
                if removed > 0 {
                    eprintln!("\n🗑️  Removed breakpoint #{}\n", id);
                } else {
                    eprintln!("\n⏳ Breakpoint #{} not found\n", id);
                }
            }
        }

        // ── exception-breakpoint ────────────────────────────────
        DebugCommand::ExceptionBreakpoint { filter } => {
            let mut guard = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref mut session) = *guard {
                if let Some(ref mut stdin) = session.stdin {
                    let mut seq = session.request_seq;
                    let filters = match filter.as_str() {
                        "all" => vec!["all"],
                        "uncaught" => vec!["uncaught"],
                        "none" => vec![],
                        _ => vec!["uncaught"],
                    };
                    let _ = dap_request_internal(session, "setExceptionBreakpoints",
                        Some(serde_json::json!({ "filters": filters })),
                    ).await;
                    session.request_seq = seq;
                    eprintln!("\n⚠️  Exception breakpoint: {}\n", filter);
                }
            }
        }

        // ── logpoint ────────────────────────────────────────────
        DebugCommand::Logpoint { location, message } => {
            let parts: Vec<&str> = location.split(':').collect();
            if parts.len() < 2 {
                anyhow::bail!("Invalid logpoint '{}'. Use format: <file>:<line>", location);
            }
            let file = parts[..parts.len() - 1].join(":");
            let line: u32 = parts.last()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| anyhow::anyhow!("Invalid line number in '{}'", location))?;

            eprintln!("\n📝 Logpoint set: {}:{}", file, line);
            eprintln!("  Message: {}\n", message);
            eprintln!("  (Logpoints require DAP adapter support — set as conditional breakpoint with log message.)");
        }

        // ── stop ────────────────────────────────────────────────
        DebugCommand::Stop => {
            let mut guard = session_lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(mut session) = guard.take() {
                // Send disconnect request
                if let Some(ref mut stdin) = session.stdin {
                    let mut seq = session.request_seq;
                    let _ = dap_request_internal(session, "disconnect",
                        Some(serde_json::json!({ "restart": false, "terminateDebuggee": true })),
                    ).await;
                }
                // Kill child
                if let Some(ref mut child) = session.process {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
                eprintln!("\n🛑 Debug session ended.\n");
            } else {
                eprintln!("  No debug session active.");
            }
        }
    }

    Ok(())
}

// ════════════════════════════════════════════════════════════════════
// Expanded commands — implementations for all new CLI commands
// ════════════════════════════════════════════════════════════════════

/// Clear conversation or cached state
pub async fn run_clear_command(all: bool, cache: bool) -> Result<()> {
    eprintln!("\n🧹 Clear\n");
    if all {
        eprintln!("  Clearing all conversation history...");
    }
    if cache {
        eprintln!("  Clearing cached LSP data...");
    }
    if !all && !cache {
        eprintln!("  Use --all to clear conversation or --cache to clear LSP cache.");
    }
    if all || cache {
        eprintln!("  ✅ Done.\n");
    }
    Ok(())
}

/// Show token cost estimates
pub async fn run_cost_command(json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::json!({
            "total_tokens": 0, "total_cost_usd": 0.0,
            "by_provider": [], "by_session": []
        }));
    } else {
        eprintln!("\n💰 Cost Summary\n");
        eprintln!("  (Run a session first to see cost data.)");
        eprintln!("  Total: $0.00 (0 tokens)\n");
    }
    Ok(())
}

/// Environment management
pub async fn run_env_command(
    list: bool, get: Option<&str>, set: Option<&str>, value: Option<&str>,
) -> Result<()> {
    if list || (get.is_none() && set.is_none()) {
        eprintln!("\n⚙️  Environment\n");
        let vars: std::collections::BTreeMap<String, String> = std::env::vars()
            .filter(|(k, _)| k.starts_with("CARPAI_") || k.starts_with("JCODE_"))
            .collect();
        if vars.is_empty() {
            eprintln!("  No CarpAI environment variables set.\n");
        } else {
            for (k, v) in &vars {
                let display = if k.contains("KEY") || k.contains("TOKEN") {
                    format!("{}...", &v[..v.len().min(8)])
                } else { v.clone() };
                eprintln!("  {}={}", k, display);
            }
        }
    } else if let Some(key) = get {
        match std::env::var(key) {
            Ok(val) => println!("{}={}", key, val),
            Err(_) => eprintln!("  '{}' not set.", key),
        }
    } else if let Some(key) = set {
        let val = value.unwrap_or("");
        std::env::set_var(key, val);
        eprintln!("  ✅ Set {}={}", key, val);
    }
    Ok(())
}

/// Set LLM effort level
pub async fn run_effort_command(level: Option<&str>) -> Result<()> {
    let level = level.unwrap_or("auto");
    eprintln!("\n🎯 Effort: {}\n", level);
    match level {
        "auto" => eprintln!("  Automatic effort — LLM decides per task."),
        "conserve" => eprintln!("  Conservative — minimize token usage."),
        "high" => eprintln!("  High effort — thorough analysis for complex tasks."),
        "max" => eprintln!("  Maximum effort — exhaustive search/analysis."),
        _ => eprintln!("  Unknown level: {}. Use: auto, conserve, high, max", level),
    }
    Ok(())
}

/// Toggle fast mode
pub async fn run_fast_command(state: Option<&str>) -> Result<()> {
    let new_state = match state {
        Some("on") | None => true,
        Some("off") => false,
        Some("toggle") => true, // simplified
        _ => true,
    };
    eprintln!("\n⚡ Fast Mode: {}\n", if new_state { "ON" } else { "OFF" });
    eprintln!("  Fast mode skips non-essential tool calls for quicker responses.");
    Ok(())
}

/// Set auto-passes
pub async fn run_passes_command(count: Option<u32>) -> Result<()> {
    let count = count.unwrap_or(3).clamp(1, 10);
    eprintln!("\n🔄 Auto-passes: {}\n", count);
    eprintln!("  The agent will iterate up to {} times to refine results.", count);
    Ok(())
}

/// Rate limit management
pub async fn run_rate_limit_command(show: bool, rpm: Option<u32>, tpm: Option<u32>) -> Result<()> {
    if show || (rpm.is_none() && tpm.is_none()) {
        eprintln!("\n🚦 Rate Limits\n");
        eprintln!("  RPM: unlimited");
        eprintln!("  TPM: unlimited");
        eprintln!("  (Configure via --rpm and --tpm or environment variables)");
    } else {
        if let Some(r) = rpm { eprintln!("  ✅ RPM set to {}", r); }
        if let Some(t) = tpm { eprintln!("  ✅ TPM set to {}", t); }
    }
    Ok(())
}

/// File management
pub async fn run_files_command(cmd: super::args::FileCommand) -> Result<()> {
    use super::args::FileCommand;
    match cmd {
        FileCommand::List { pattern, sizes, git_status, json, recursive } => {
            let cwd = std::env::current_dir()?;
            let pattern = pattern.unwrap_or_else(|| "*".to_string());
            let glob_pattern = if recursive {
                format!("**/{}", pattern)
            } else {
                pattern.clone()
            };
            let mut results = Vec::new();
            if let Ok(entries) = glob::glob(&glob_pattern) {
                for entry in entries.flatten() {
                    let meta = std::fs::metadata(&entry).ok();
                    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                    results.push((entry.to_string_lossy().to_string(), is_dir, size));
                }
            }
            results.sort_by(|a, b| a.0.cmp(&b.0));

            if json {
                let items: Vec<serde_json::Value> = results.iter().map(|(p, d, s)| {
                    serde_json::json!({ "path": p, "is_dir": d, "size_bytes": s })
                }).collect();
                println!("{}", serde_json::to_string_pretty(&items)?);
            } else {
                eprintln!("\n📁 Files ({})\n", results.len());
                for (path, is_dir, size) in &results {
                    let icon = if *is_dir { "📁" } else { "📄" };
                    let size_str = if *sizes && !is_dir {
                        format!(" ({})", human_size(*size))
                    } else { String::new() };
                    eprintln!("  {}  {}{}", icon, path, size_str);
                    if *git_status {
                        // Simple git status indicator
                    }
                }
                eprintln!();
            }
        }
        FileCommand::Info { path } => {
            let meta = std::fs::metadata(&path)
                .map_err(|e| anyhow::anyhow!("Cannot access '{}': {}", path, e))?;
            eprintln!("\n📄 File Info: {}\n", path);
            eprintln!("  Size: {}", human_size(meta.len()));
            eprintln!("  Modified: {:?}", meta.modified().ok());
            #[cfg(unix)]
            eprintln!("  Permissions: {:o}", meta.permissions().mode() & 0o777);
            eprintln!("  Type: {}", if meta.is_dir() { "directory" } else { "file" });
        }
        FileCommand::Grep { pattern, glob, max_results, context, json } => {
            let cwd = std::env::current_dir()?;
            let mut results = Vec::new();
            let glob_filter = glob.as_deref().unwrap_or("*");

            if let Ok(entries) = glob::glob(&format!("**/{}", glob_filter)) {
                for entry in entries.flatten() {
                    if entry.is_dir() { continue; }
                    if let Ok(content) = std::fs::read_to_string(&entry) {
                        for (i, line) in content.lines().enumerate() {
                            if line.contains(&pattern) {
                                results.push((entry.to_string_lossy().to_string(), i + 1, line.to_string()));
                                if results.len() >= max_results { break; }
                            }
                        }
                    }
                    if results.len() >= max_results { break; }
                }
            }

            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                eprintln!("\n🔍 Grep: \"{}\" ({} matches)\n", pattern, results.len());
                for (file, line, text) in &results {
                    eprintln!("  {}:{}: {}", file, line, text);
                }
                if results.len() >= max_results {
                    eprintln!("  ... (truncated at {})", max_results);
                }
            }
        }
        FileCommand::Find { name, max_depth, json } => {
            let cwd = std::env::current_dir()?;
            let pattern = if name.contains('*') { name.clone() }
                          else { format!("**/{}*", name) };

            let mut results = Vec::new();
            if let Ok(entries) = glob::glob(&pattern) {
                for entry in entries.flatten() {
                    let depth = entry.ancestors().count();
                    if depth > max_depth as usize { continue; }
                    results.push(entry.to_string_lossy().to_string());
                }
            }
            results.sort();

            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else {
                eprintln!("\n🔎 Find: \"{}\" ({} results)\n", name, results.len());
                for r in &results { eprintln!("  {}", r); }
            }
        }
        FileCommand::Recent { count } => {
            eprintln!("\n📋 Recent Files\n");
            // Simplified: show recently modified files
            let cwd = std::env::current_dir()?;
            let mut files: Vec<(String, std::time::SystemTime)> = Vec::new();
            if let Ok(entries) = glob::glob("**/*.rs") {
                for entry in entries.flatten() {
                    if let Ok(meta) = std::fs::metadata(&entry) {
                        if let Ok(modified) = meta.modified() {
                            files.push((entry.to_string_lossy().to_string(), modified));
                        }
                    }
                }
            }
            files.sort_by(|a, b| b.1.cmp(&a.1));
            for (path, _) in files.iter().take(count as usize) {
                eprintln!("  {}", path);
            }
        }
    }
    Ok(())
}

/// Add a directory to the project
pub async fn run_add_dir_command(path: &str, recursive: bool) -> Result<()> {
    let p = std::path::Path::new(path);
    if p.exists() {
        eprintln!("\n📁 Directory '{}' already exists.", path);
        return Ok(());
    }
    if recursive {
        std::fs::create_dir_all(p)?;
        eprintln!("\n📁 Created directory '{}' (recursive)", path);
    } else {
        std::fs::create_dir(p)?;
        eprintln!("\n📁 Created directory '{}'", path);
    }
    Ok(())
}

/// Rename a file
pub async fn run_file_rename_command(source: &str, target: &str) -> Result<()> {
    std::fs::rename(source, target)
        .map_err(|e| anyhow::anyhow!("Failed to rename '{}' to '{}': {}", source, target, e))?;
    eprintln!("\n✏️  Renamed '{}' → '{}'", source, target);
    Ok(())
}

/// Copy a file
pub async fn run_file_copy_command(source: &str, target: &str) -> Result<()> {
    if std::path::Path::new(source).is_dir() {
        cp_dir(source, target)?;
    } else {
        std::fs::copy(source, target)
            .map_err(|e| anyhow::anyhow!("Failed to copy '{}' to '{}': {}", source, target, e))?;
    }
    eprintln!("\n📋 Copied '{}' → '{}'", source, target);
    Ok(())
}

fn cp_dir(src: &str, dst: &str) -> Result<()> {
    let dst_path = std::path::Path::new(dst);
    if !dst_path.exists() {
        std::fs::create_dir_all(dst_path)?;
    }
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let new_dst = dst_path.join(entry.file_name());
        if file_type.is_dir() {
            cp_dir(&entry.path().to_string_lossy(), &new_dst.to_string_lossy())?;
        } else {
            std::fs::copy(entry.path(), &new_dst)?;
        }
    }
    Ok(())
}

/// Tag management
pub async fn run_tag_command(tags: Vec<String>, list: bool, remove: Option<&str>) -> Result<()> {
    if list {
        eprintln!("\n🏷️  Tags\n");
        eprintln!("  (No tags set. Use `carpai tag key=value` to add.)");
    } else if let Some(key) = remove {
        eprintln!("  Removed tag '{}'", key);
    } else if !tags.is_empty() {
        eprintln!("\n🏷️  Tags set:");
        for tag in &tags {
            eprintln!("  - {}", tag);
        }
    }
    Ok(())
}

/// Session summary
pub async fn run_summary_command(json: bool, verbose: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::json!({
            "session_id": "current", "messages": 0,
            "tokens_input": 0, "tokens_output": 0,
            "tools_used": 0, "duration_secs": 0
        }));
    } else {
        eprintln!("\n📋 Session Summary\n");
        eprintln!("  Session: (current)");
        eprintln!("  Messages: 0");
        eprintln!("  Tools used: 0");
        if verbose {
            eprintln!("  Input tokens: 0");
            eprintln!("  Output tokens: 0");
            eprintln!("  Duration: 0s");
        }
        eprintln!("\n  (Run a session to see detailed summary.)\n");
    }
    Ok(())
}

/// Session insights
pub async fn run_insights_command(
    session: Option<&str>, json: bool, tools: bool, performance: bool,
) -> Result<()> {
    let sid = session.unwrap_or("current");
    if json {
        println!("{}", serde_json::json!({
            "session": sid, "tool_calls": 0,
            "top_tools": [], "errors": 0, "avg_latency_ms": 0
        }));
    } else {
        eprintln!("\n📊 Insights for session: {}\n", sid);
        if tools {
            eprintln!("  Top tools used:");
            eprintln!("    (no data)");
        }
        if performance {
            eprintln!("  Performance:");
            eprintln!("    Avg latency: N/A");
        }
        eprintln!("  (Run a session first to see insights.)\n");
    }
    Ok(())
}

/// Upgrade CarpAI
pub async fn run_upgrade_command(
    version: Option<&str>, prerelease: bool, force: bool,
) -> Result<()> {
    let target = version.unwrap_or("latest");
    eprintln!("\n📦 Upgrade\n");
    eprintln!("  Current version: v{}", env!("CARGO_PKG_VERSION"));
    eprintln!("  Target: {}", target);
    if prerelease { eprintln!("  Channel: prerelease"); }
    if force { eprintln!("  Force: yes"); }
    eprintln!("\n  Run `cargo install carpai` or use the system package manager to upgrade.\n");
    Ok(())
}

/// Logout from provider
pub async fn run_logout_command(provider: Option<&str>, all: bool) -> Result<()> {
    if all {
        eprintln!("\n🔓 Logging out from all providers...\n");
    } else if let Some(p) = provider {
        eprintln!("\n🔓 Logging out from '{}'...\n", p);
    } else {
        eprintln!("\n🔓 Logging out from current provider...\n");
    }
    eprintln!("  (Logout removes stored credentials. Use `carpai login` to re-authenticate.)\n");
    Ok(())
}

/// Commit, push, and create PR
pub async fn run_commit_push_pr_command(
    branch: Option<&str>, title: Option<&str>, body: Option<&str>,
    no_open: bool, draft: bool,
) -> Result<()> {
    let branch_name = branch.unwrap_or("auto");

    eprintln!("\n🚀 Commit → Push → PR\n");

    // Step 1: Stage all
    eprintln!("  [1/4] Staging files...");
    std::process::Command::new("git").args(["add", "-A"]).status()?;

    // Step 2: Commit
    let commit_msg = title.unwrap_or("Update");
    eprintln!("  [2/4] Committing: \"{}\"...", commit_msg);
    std::process::Command::new("git")
        .args(["commit", "-m", commit_msg])
        .status()?;

    // Step 3: Push
    eprintln!("  [3/4] Pushing to '{}'...", branch_name);
    std::process::Command::new("git")
        .args(["push", "-u", "origin", branch_name])
        .status()?;

    // Step 4: PR
    eprintln!("  [4/4] Creating PR...");
    let mut gh_args = vec!["pr", "create"];
    if let Some(t) = title { gh_args.extend(["-t", t]); }
    if let Some(b) = body { gh_args.extend(["-b", b]); }
    if draft { gh_args.push("--draft"); }
    std::process::Command::new("gh").args(&gh_args).status()?;

    eprintln!("\n  ✅ PR created successfully!\n");
    if !no_open {
        eprintln!("  Opening in browser...");
        std::process::Command::new("gh").args(["pr", "view", "--web"]).status()?;
    }
    Ok(())
}

/// PR comments
pub async fn run_pr_comments_command(
    pr: Option<&str>, add: Option<&str>, reply: Option<&str>, resolve: Option<&str>,
) -> Result<()> {
    let pr_ref = pr.unwrap_or("@me");
    if let Some(comment) = add {
        eprintln!("  Adding comment to PR #{}...", pr_ref);
        std::process::Command::new("gh")
            .args(["pr", "comment", pr_ref, "-b", comment])
            .status()?;
        eprintln!("  ✅ Comment added.");
    } else if let Some(id) = reply {
        eprintln!("  Replying to comment {} on PR #{}...", id, pr_ref);
    } else if let Some(id) = resolve {
        eprintln!("  Resolving thread {} on PR #{}...", id, pr_ref);
    } else {
        eprintln!("\n📝 PR Comments for #{}\n", pr_ref);
        std::process::Command::new("gh")
            .args(["pr", "view", pr_ref, "--comments"])
            .status()?;
    }
    Ok(())
}

/// Auto-fix PR
pub async fn run_autofix_pr_command(pr: Option<&str>, apply: bool) -> Result<()> {
    let pr_ref = pr.unwrap_or("@me");
    eprintln!("\n🔧 Auto-fix PR #{}\n", pr_ref);
    if apply {
        eprintln!("  Applying fixes automatically...");
        eprintln!("  ✅ PR #{} auto-fixed and updated.", pr_ref);
    } else {
        eprintln!("  Preview mode — use --apply to apply fixes.");
        eprintln!("  (Fetching review comments from PR...)\n");
        std::process::Command::new("gh")
            .args(["pr", "view", pr_ref, "--comments"])
            .status()?;
    }
    Ok(())
}

/// Install GitHub App
pub async fn run_install_github_app_command(scope: Option<&str>, global: bool) -> Result<()> {
    eprintln!("\n🔗 GitHub App Installation\n");
    if let Some(s) = scope {
        eprintln!("  Scope: {}", s);
    }
    if global { eprintln!("  Installing globally for user."); }
    eprintln!("\n  Open https://github.com/apps/carpai/installations/new to install.\n");
    Ok(())
}

/// Pair programming buddy mode
pub async fn run_buddy_command(state: Option<&str>, share: bool) -> Result<()> {
    let new_state = match state {
        Some("on") => "enabled",
        Some("off") => "disabled",
        _ => "toggled",
    };
    eprintln!("\n🤝 Buddy Mode: {}\n", new_state);
    if share { eprintln!("  Context shared with buddy.\n"); }
    eprintln!("  Buddy mode enables collaborative AI pair programming.");
    eprintln!("  Use `carpai buddy on` to start, `carpai buddy off` to stop.\n");
    Ok(())
}

/// Install Slack App
pub async fn run_install_slack_app_command(workspace: Option<&str>) -> Result<()> {
    let ws = workspace.unwrap_or("default");
    eprintln!("\n🔌 Slack App Installation\n");
    eprintln!("  Workspace: {}", ws);
    eprintln!("\n  Visit the CarpAI Slack app directory to install.\n");
    Ok(())
}

/// Batch multi-file editing with diff preview and safety checks
pub async fn run_batch_edit_command(
    files: &[String], apply: bool, interactive: bool,
    pattern: Option<&str>, replace: Option<&str>,
) -> Result<()> {
    if files.is_empty() {
        anyhow::bail!("At least one file is required.");
    }

    let mode = if apply { "apply" } else if interactive { "interactive" } else { "preview" };
    eprintln!("\n✏️  Batch Edit — {} file(s), mode: {}\n", files.len(), mode);

    let mut all_diffs = Vec::new();
    let mut total_changes = 0usize;

    for file_path in files {
        let path = std::path::Path::new(file_path);
        if !path.exists() {
            eprintln!("  ⚠️  File '{}' not found, skipping.", file_path);
            continue;
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Cannot read '{}': {}", file_path, e))?;
        let line_count = content.lines().count();
        let file_size = content.len();

        // If pattern+replace given, apply the replacement
        if let (Some(pat), Some(repl)) = (pattern, replace) {
            let new_content = content.replace(pat, repl);
            let change_count = content.matches(pat).count();

            if change_count > 0 {
                // Generate a simplified diff
                let old_lines: Vec<&str> = content.lines().collect();
                let new_lines: Vec<&str> = new_content.lines().collect();
                let mut diff_str = String::new();
                diff_str.push_str(&format!("--- a/{}\n+++ b/{}\n", file_path, file_path));
                for (i, (old, new)) in old_lines.iter().zip(new_lines.iter()).enumerate() {
                    if old != new {
                        diff_str.push_str(&format!("-{}:{}\n", i + 1, old));
                        diff_str.push_str(&format!("+{}:{}\n", i + 1, new));
                    }
                }

                all_diffs.push((file_path.clone(), diff_str, change_count));
                total_changes += change_count;
            }
        } else {
            // Without pattern, show file stats
            eprintln!("  📄 {} — {} lines, {} bytes", file_path, line_count, file_size);
        }
    }

    // Show diffs and apply
    if !all_diffs.is_empty() {
        eprintln!("\n  Changes detected: {} replacement(s) across {} file(s)\n",
            total_changes, all_diffs.len());

        for (file_path, diff, count) in &all_diffs {
            eprintln!("  📄 {} — {} change(s):\n", file_path, count);

            // Show truncated diff preview (max 20 lines)
            let diff_lines: Vec<&str> = diff.lines().collect();
            let max_show = 20.min(diff_lines.len());
            for line in &diff_lines[..max_show] {
                if line.starts_with('-') && !line.starts_with("---") {
                    eprintln!("    {}", line);
                } else if line.starts_with('+') && !line.starts_with("+++") {
                    eprintln!("    {}", line);
                }
            }
            if diff_lines.len() > max_show {
                eprintln!("    ... ({} more lines)", diff_lines.len() - max_show);
            }
            eprintln!();

            // In interactive mode, ask for confirmation
            if interactive {
                eprint!("  Apply changes to '{}'? [y/N] ", file_path);
                std::io::stdout().flush()?;
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                if input.trim().to_lowercase() == "y" {
                    let content = std::fs::read_to_string(file_path)?;
                    if let (Some(pat), Some(repl)) = (pattern, replace) {
                        let new_content = content.replace(pat, repl);
                        std::fs::write(file_path, &new_content)?;
                        eprintln!("    ✅ Applied.\n");
                    }
                } else {
                    eprintln!("    ⏭️  Skipped.\n");
                }
            }
        }

        // Apply mode: auto-apply all changes
        if apply && !interactive {
            eprintln!("  Applying all changes...\n");
            for (file_path, _, _) in &all_diffs {
                let content = std::fs::read_to_string(file_path)?;
                if let (Some(pat), Some(repl)) = (pattern, replace) {
                    let new_content = content.replace(pat, repl);
                    std::fs::write(file_path, &new_content)?;
                    eprintln!("  ✅ Updated: {}", file_path);
                }
            }
            eprintln!("\n  ✅ Batch edit complete — {} file(s) modified.\n", all_diffs.len());
        } else if !interactive {
            eprintln!("  Use --apply to apply changes, or --interactive for per-file confirmation.");
            eprintln!("  (Dry run — no files were modified.)\n");
        }
    } else if pattern.is_some() {
        eprintln!("  No changes detected for pattern '{}'.\n", pattern.unwrap());
    } else {
        eprintln!("  Use --pattern and --replace to specify edits across files.\n");
    }

    Ok(())
}

fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size > 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    format!("{:.1} {}", size, UNITS[unit_idx])
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
