#![cfg_attr(test, allow(clippy::await_holding_lock))]

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::{IsTerminal, Read, Write};
use std::net::ToSocketAddrs;
use std::path::PathBuf;

use crate::{browser, gateway, memory, storage, tui};

use super::terminal::{cleanup_tui_runtime, init_tui_runtime};

pub use super::auth_test::run_auth_test_command;
pub(crate) use super::auth_test::run_post_login_validation;
#[cfg(test)]
pub(crate) use super::auth_test::{
    AuthTestChoicePlan, AuthTestTarget, ResolvedAuthTestTarget, auth_test_choice_plan,
    auth_test_error_is_retryable, configured_auth_test_targets, resolve_auth_test_targets,
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
    if qr2term::print_qr(&pair_uri).is_err() {
        eprintln!("  \x1b[33m(QR code generation failed)\x1b[0m\n");
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

pub async fn run_restart_save_command(auto_restore: bool) -> Result<()> {
    let mut snapshot = if let Some(snapshot) = capture_connected_restart_snapshot().await? {
        snapshot
    } else {
        crate::restart_snapshot::save_current_snapshot()?
    };
    snapshot.auto_restore_on_next_start = auto_restore;
    crate::restart_snapshot::write_snapshot(&snapshot)?;
    let path = crate::restart_snapshot::snapshot_path()?;

    if snapshot.sessions.is_empty() {
        println!("Saved empty reboot snapshot to {}", path.display());
        if auto_restore {
            println!("Automatic restore is armed for the next plain `jcode` launch.");
        }
        println!("\nNo active jcode windows were detected.");
        return Ok(());
    }

    println!(
        "Saved reboot snapshot with {} session(s) to {}\n",
        snapshot.sessions.len(),
        path.display()
    );
    for session in &snapshot.sessions {
        let suffix = if session.is_selfdev {
            " [self-dev]"
        } else {
            ""
        };
        println!(
            "- {} ({}){}",
            session.display_name, session.session_id, suffix
        );
    }
    if auto_restore {
        println!("\nAutomatic restore is armed for the next plain `jcode` launch.");
    }
    println!("\nAfter reboot, restore them with:\n  jcode restart restore");

    Ok(())
}

pub fn run_restart_status_command() -> Result<()> {
    let path = crate::restart_snapshot::snapshot_path()?;
    let snapshot = match crate::restart_snapshot::load_snapshot() {
        Ok(snapshot) => snapshot,
        Err(_) => {
            println!("No reboot snapshot saved.\n\nCreate one with:\n  jcode restart save");
            return Ok(());
        }
    };

    println!(
        "Reboot snapshot: {}\nCreated: {}\nSessions: {}\nAuto-restore on next plain startup: {}\n",
        path.display(),
        snapshot.created_at,
        snapshot.sessions.len(),
        if snapshot.auto_restore_on_next_start {
            "armed"
        } else {
            "off"
        }
    );
    for session in &snapshot.sessions {
        let suffix = if session.is_selfdev {
            " [self-dev]"
        } else {
            ""
        };
        println!(
            "- {} ({}){}",
            session.display_name, session.session_id, suffix
        );
    }

    Ok(())
}

pub async fn maybe_run_pending_restart_restore_on_startup() -> Result<bool> {
    let snapshot = match crate::restart_snapshot::load_snapshot() {
        Ok(snapshot) => snapshot,
        Err(_) => return Ok(false),
    };

    if snapshot.auto_restore_on_next_start {
        let _ = crate::restart_snapshot::set_auto_restore_on_next_start(false);
        println!(
            "Found a reboot snapshot with auto-restore enabled. Restoring {} jcode window(s)...\n",
            snapshot.sessions.len()
        );
        run_restart_restore_command()?;
        return Ok(true);
    }

    if std::io::stdin().is_terminal() || std::io::stderr().is_terminal() {
        println!("Saved reboot snapshot detected. Restore it with:\n  jcode restart restore\n");
    }

    Ok(false)
}

pub fn run_restart_clear_command() -> Result<()> {
    if crate::restart_snapshot::clear_snapshot()? {
        println!("Cleared reboot snapshot.");
    } else {
        println!("No reboot snapshot was saved.");
    }
    Ok(())
}

pub fn run_restart_restore_command() -> Result<()> {
    let exe = current_restart_restore_exe()?;
    let result = match crate::restart_snapshot::restore_snapshot(&exe) {
        Ok(result) => result,
        Err(error) => {
            let path = crate::restart_snapshot::snapshot_path()?;
            return Err(anyhow::anyhow!(
                "Failed to restore reboot snapshot at {}: {}",
                path.display(),
                error
            ));
        }
    };

    if result.snapshot.sessions.is_empty() {
        println!("Saved reboot snapshot is empty. Nothing to restore.");
        let _ = crate::restart_snapshot::clear_snapshot();
        return Ok(());
    }

    let launched = result
        .outcomes
        .iter()
        .filter(|outcome| outcome.launched)
        .count();
    let fallback = result.outcomes.len().saturating_sub(launched);

    if launched > 0 {
        println!("Restored {} jcode window(s).", launched);
    }

    if fallback > 0 {
        println!(
            "\n{} session(s) could not be opened automatically. Run these commands manually:\n",
            fallback
        );
        for outcome in result.outcomes.iter().filter(|outcome| !outcome.launched) {
            println!("# {}", outcome.session.display_name);
            println!("{}", outcome.command);
        }
        println!(
            "\nThe reboot snapshot was kept so you can try `jcode restart restore` again later."
        );
        return Ok(());
    }

    let _ = crate::restart_snapshot::clear_snapshot();
    println!("Cleared reboot snapshot after successful restore.");
    Ok(())
}

fn current_restart_restore_exe() -> Result<PathBuf> {
    crate::build::client_update_candidate(false)
        .map(|(path, _)| path)
        .or_else(|| std::env::current_exe().ok())
        .ok_or_else(|| anyhow::anyhow!("Could not determine jcode executable for restore"))
}

#[derive(Debug, Deserialize)]
struct ConnectedRestartSessionRow {
    session_id: String,
    #[serde(default)]
    working_dir: Option<String>,
}

async fn capture_connected_restart_snapshot()
-> Result<Option<crate::restart_snapshot::RestartSnapshot>> {
    let mut client = match crate::server::Client::connect_debug().await {
        Ok(client) => client,
        Err(_) => return Ok(None),
    };

    let request_id = client.debug_command("sessions", None).await?;
    let response = loop {
        match client.read_event().await? {
            crate::protocol::ServerEvent::DebugResponse { id, ok, output } if id == request_id => {
                if !ok {
                    anyhow::bail!(output);
                }
                break output;
            }
            crate::protocol::ServerEvent::Ack { id } if id == request_id => {}
            crate::protocol::ServerEvent::Done { id } if id == request_id => {}
            crate::protocol::ServerEvent::Error { id, message, .. } if id == request_id => {
                anyhow::bail!(message);
            }
            _ => {}
        }
    };

    let rows: Vec<ConnectedRestartSessionRow> = serde_json::from_str(&response)?;
    if rows.is_empty() {
        return Ok(Some(crate::restart_snapshot::RestartSnapshot {
            version: 1,
            created_at: Utc::now(),
            auto_restore_on_next_start: false,
            sessions: Vec::new(),
        }));
    }

    let mut seen = std::collections::HashSet::new();
    let mut sessions = Vec::new();
    for row in rows {
        if !seen.insert(row.session_id.clone()) {
            continue;
        }
        let Ok(mut session) = crate::session::Session::load(&row.session_id) else {
            continue;
        };
        if session.detect_crash() {
            let _ = session.save();
            continue;
        }
        sessions.push(crate::restart_snapshot::RestartSnapshotSession {
            session_id: session.id.clone(),
            display_name: session.display_name().to_string(),
            working_dir: session.working_dir.clone().or(row.working_dir),
            is_selfdev: session.is_canary,
        });
    }

    sessions.sort_by(|a, b| {
        a.display_name
            .cmp(&b.display_name)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });

    Ok(Some(crate::restart_snapshot::RestartSnapshot {
        version: 1,
        created_at: Utc::now(),
        auto_restore_on_next_start: false,
        sessions,
    }))
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

#[derive(Debug, Serialize)]
struct AuthStatusProviderReport {
    id: String,
    display_name: String,
    status: String,
    method: String,
    health: String,
    credential_source: String,
    expiry_confidence: String,
    refresh_support: String,
    validation_method: String,
    last_refresh: Option<String>,
    validation: Option<String>,
    auth_kind: String,
    recommended: bool,
}

#[derive(Debug, Serialize)]
struct AuthStatusReport {
    any_available: bool,
    providers: Vec<AuthStatusProviderReport>,
}

#[derive(Debug, Serialize)]
struct ProviderListEntry {
    id: String,
    display_name: String,
    auth_kind: Option<String>,
    recommended: bool,
    aliases: Vec<String>,
    detail: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProviderListReport {
    providers: Vec<ProviderListEntry>,
}

#[derive(Debug, Serialize)]
struct ProviderCurrentReport {
    requested_provider: String,
    requested_model: Option<String>,
    resolved_provider: String,
    selected_model: String,
}

#[derive(Debug, Serialize)]
struct VersionReport {
    version: String,
    semver: String,
    base_semver: String,
    update_semver: String,
    git_hash: String,
    git_tag: String,
    build_time: String,
    git_date: String,
    release_build: bool,
}

#[derive(Debug, Serialize)]
struct UsageLimitReport {
    name: String,
    usage_percent: f32,
    resets_at: Option<String>,
    reset_in: Option<String>,
}

#[derive(Debug, Serialize)]
struct UsageProviderReport {
    provider_name: String,
    limits: Vec<UsageLimitReport>,
    extra_info: Vec<(String, String)>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct UsageReport {
    providers: Vec<UsageProviderReport>,
}

pub fn run_auth_status_command(emit_json: bool) -> Result<()> {
    let status = crate::auth::AuthStatus::check();
    let validation = crate::auth::validation::load_all();
    let providers = crate::provider_catalog::auth_status_login_providers();
    let reports = providers
        .into_iter()
        .map(|provider| {
            let assessment = status.assessment_for_provider(provider);
            AuthStatusProviderReport {
                id: provider.id.to_string(),
                display_name: provider.display_name.to_string(),
                status: auth_state_label(assessment.state).to_string(),
                method: assessment.method_detail.clone(),
                health: assessment.health_summary(),
                credential_source: assessment.credential_source.label().to_string(),
                expiry_confidence: assessment.expiry_confidence.label().to_string(),
                refresh_support: assessment.refresh_support.label().to_string(),
                validation_method: assessment.validation_method.label().to_string(),
                last_refresh: assessment
                    .last_refresh
                    .as_ref()
                    .map(crate::auth::refresh_state::format_record_label),
                validation: validation
                    .get(provider.id)
                    .map(crate::auth::validation::format_record_label),
                auth_kind: provider.auth_kind.label().to_string(),
                recommended: provider.recommended,
            }
        })
        .collect::<Vec<_>>();

    if emit_json {
        let report = AuthStatusReport {
            any_available: status.has_any_available(),
            providers: reports,
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for provider in reports {
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                provider.id,
                provider.status,
                provider.auth_kind,
                provider.method,
                provider.health,
                provider.validation.as_deref().unwrap_or("not validated")
            );
        }
    }

    Ok(())
}

pub fn run_provider_list_command(emit_json: bool) -> Result<()> {
    let providers = list_cli_providers();

    if emit_json {
        let report = ProviderListReport { providers };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for provider in providers {
            if let Some(detail) = provider.detail.as_deref() {
                println!("{}\t{}\t{}", provider.id, provider.display_name, detail);
            } else {
                println!("{}\t{}", provider.id, provider.display_name);
            }
        }
    }

    Ok(())
}

pub async fn run_provider_current_command(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    emit_json: bool,
) -> Result<()> {
    let provider = super::provider_init::init_provider_quiet(choice, model).await?;
    let report = ProviderCurrentReport {
        requested_provider: choice.as_arg_value().to_string(),
        requested_model: model.map(str::to_string),
        resolved_provider: provider.name().to_string(),
        selected_model: provider.model(),
    };

    if emit_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("requested_provider\t{}", report.requested_provider);
        if let Some(requested_model) = report.requested_model.as_deref() {
            println!("requested_model\t{}", requested_model);
        }
        println!("resolved_provider\t{}", report.resolved_provider);
        println!("selected_model\t{}", report.selected_model);
    }

    Ok(())
}

pub fn run_version_command(emit_json: bool) -> Result<()> {
    let report = VersionReport {
        version: env!("JCODE_VERSION").to_string(),
        semver: env!("JCODE_SEMVER").to_string(),
        base_semver: env!("JCODE_BASE_SEMVER").to_string(),
        update_semver: env!("JCODE_UPDATE_SEMVER").to_string(),
        git_hash: env!("JCODE_GIT_HASH").to_string(),
        git_tag: env!("JCODE_GIT_TAG").to_string(),
        build_time: crate::build::current_binary_build_time_string()
            .unwrap_or_else(|| "unknown".to_string()),
        git_date: env!("JCODE_GIT_DATE").to_string(),
        release_build: option_env!("JCODE_RELEASE_BUILD").is_some(),
    };

    if emit_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("version\t{}", report.version);
        println!("semver\t{}", report.semver);
        println!("base_semver\t{}", report.base_semver);
        println!("update_semver\t{}", report.update_semver);
        println!("git_hash\t{}", report.git_hash);
        println!("git_tag\t{}", report.git_tag);
        println!("build_time\t{}", report.build_time);
        println!("git_date\t{}", report.git_date);
        println!("release_build\t{}", report.release_build);
    }

    Ok(())
}

pub async fn run_usage_command(emit_json: bool) -> Result<()> {
    let providers = crate::usage::fetch_all_provider_usage().await;

    let report = UsageReport {
        providers: providers.iter().map(usage_provider_report).collect(),
    };

    if emit_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    if report.providers.is_empty() {
        println!("No connected providers");
        println!();
        println!("Next steps:");
        println!("- Use `jcode login --provider claude` to connect Claude OAuth.");
        println!("- Use `jcode login --provider openai` to connect ChatGPT / Codex OAuth.");
        return Ok(());
    }

    for (idx, provider) in report.providers.iter().enumerate() {
        if idx > 0 {
            println!();
        }

        println!("{}", provider.provider_name);
        println!("{}", "-".repeat(provider.provider_name.chars().count()));

        if let Some(error) = &provider.error {
            println!("error: {}", error);
            continue;
        }

        if provider.limits.is_empty() && provider.extra_info.is_empty() {
            println!("No usage data available.");
            continue;
        }

        for limit in &provider.limits {
            match limit.reset_in.as_deref() {
                Some(reset_in) => println!(
                    "{}: {} (resets in {})",
                    limit.name,
                    crate::usage::format_usage_bar(limit.usage_percent, 15),
                    reset_in
                ),
                None => println!(
                    "{}: {}",
                    limit.name,
                    crate::usage::format_usage_bar(limit.usage_percent, 15)
                ),
            }
        }

        if !provider.extra_info.is_empty() {
            if !provider.limits.is_empty() {
                println!();
            }
            for (key, value) in &provider.extra_info {
                println!("{}: {}", key, value);
            }
        }
    }

    Ok(())
}

fn usage_provider_report(provider: &crate::usage::ProviderUsage) -> UsageProviderReport {
    UsageProviderReport {
        provider_name: provider.provider_name.clone(),
        limits: provider
            .limits
            .iter()
            .map(|limit| UsageLimitReport {
                name: limit.name.clone(),
                usage_percent: limit.usage_percent,
                resets_at: limit.resets_at.clone(),
                reset_in: limit
                    .resets_at
                    .as_deref()
                    .map(crate::usage::format_reset_time),
            })
            .collect(),
        extra_info: provider.extra_info.clone(),
        error: provider.error.clone(),
    }
}

fn list_cli_providers() -> Vec<ProviderListEntry> {
    use super::provider_init::ProviderChoice;

    let choices = [
        ProviderChoice::Jcode,
        ProviderChoice::Claude,
        ProviderChoice::Openai,
        ProviderChoice::Openrouter,
        ProviderChoice::Azure,
        ProviderChoice::Opencode,
        ProviderChoice::OpencodeGo,
        ProviderChoice::Zai,
        ProviderChoice::Kimi,
        ProviderChoice::Groq,
        ProviderChoice::Mistral,
        ProviderChoice::Perplexity,
        ProviderChoice::TogetherAi,
        ProviderChoice::Deepinfra,
        ProviderChoice::Xai,
        ProviderChoice::Chutes,
        ProviderChoice::Cerebras,
        ProviderChoice::AlibabaCodingPlan,
        ProviderChoice::OpenaiCompatible,
        ProviderChoice::Cursor,
        ProviderChoice::Copilot,
        ProviderChoice::Gemini,
        ProviderChoice::Antigravity,
        ProviderChoice::Google,
        ProviderChoice::Auto,
    ];

    choices
        .into_iter()
        .map(|choice| {
            if let Some(provider) = super::provider_init::login_provider_for_choice(&choice) {
                ProviderListEntry {
                    id: choice.as_arg_value().to_string(),
                    display_name: provider.display_name.to_string(),
                    auth_kind: Some(provider.auth_kind.label().to_string()),
                    recommended: provider.recommended,
                    aliases: provider
                        .aliases
                        .iter()
                        .map(|alias| (*alias).to_string())
                        .collect(),
                    detail: Some(provider.menu_detail.to_string()),
                }
            } else {
                ProviderListEntry {
                    id: choice.as_arg_value().to_string(),
                    display_name: "Auto-detect".to_string(),
                    auth_kind: None,
                    recommended: false,
                    aliases: Vec::new(),
                    detail: Some("Use the best configured provider automatically".to_string()),
                }
            }
        })
        .collect()
}

fn auth_state_label(state: crate::auth::AuthState) -> &'static str {
    match state {
        crate::auth::AuthState::Available => "available",
        crate::auth::AuthState::Expired => "expired",
        crate::auth::AuthState::NotConfigured => "not_configured",
    }
}

pub async fn run_single_message_command(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
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

    if emit_json {
        let text = agent.run_once_capture(message).await?;
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
        agent.run_once(message).await?;
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
    let mut run_future =
        std::pin::pin!(agent.run_once_streaming_mpsc(message, Vec::new(), None, event_tx,));
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
    }

    let result = run_result.unwrap_or(Ok(()));
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
    let models = collect_cli_model_names(&routes, provider.available_models_display());

    if models.is_empty() {
        anyhow::bail!(
            "No models found for provider '{}'. Check credentials or try a different --provider.",
            provider.name()
        );
    }

    if emit_json {
        let report = ModelListReport {
            provider: provider.name().to_string(),
            selected_model: provider.model(),
            models,
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        if verbose {
            println!("Provider: {}", provider.name());
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

fn collect_cli_model_names(
    routes: &[crate::provider::ModelRoute],
    display_models: Vec<String>,
) -> Vec<String> {
    let mut deduped = Vec::new();
    let mut seen = BTreeSet::new();

    fn push_model(deduped: &mut Vec<String>, seen: &mut BTreeSet<String>, model: &str) {
        let trimmed = model.trim();
        if !is_listable_model_name(trimmed) {
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

fn is_listable_model_name(model: &str) -> bool {
    !model.is_empty() && !matches!(model, "copilot models" | "openrouter models")
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
