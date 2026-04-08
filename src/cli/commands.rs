use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::io::{IsTerminal, Read, Write};
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::time::Duration;

use crate::{browser, gateway, memory, storage, tui};

use super::terminal::{cleanup_tui_runtime, init_tui_runtime};

const DEFAULT_AUTH_TEST_PROVIDER_PROMPT: &str =
    "Reply with exactly AUTH_TEST_OK and nothing else. Do not call tools.";
const DEFAULT_AUTH_TEST_TOOL_PROMPT: &str = "If tools are available, use exactly one trivial tool call and then reply with exactly AUTH_TEST_OK and nothing else.";

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
            if browser::is_setup_complete() {
                println!("Browser bridge: installed and ready");
            } else {
                println!("Browser bridge: not set up");
                println!("Run `jcode browser setup` to install");
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolvedAuthTestTarget {
    Detailed(AuthTestTarget),
    Generic {
        provider: crate::provider_catalog::LoginProviderDescriptor,
        choice: super::provider_init::ProviderChoice,
    },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthTestTarget {
    Claude,
    Openai,
    Gemini,
    Antigravity,
    Google,
    Copilot,
    Cursor,
}

impl AuthTestTarget {
    fn provider_choice(self) -> super::provider_init::ProviderChoice {
        match self {
            Self::Claude => super::provider_init::ProviderChoice::Claude,
            Self::Openai => super::provider_init::ProviderChoice::Openai,
            Self::Gemini => super::provider_init::ProviderChoice::Gemini,
            Self::Antigravity => super::provider_init::ProviderChoice::Antigravity,
            Self::Google => super::provider_init::ProviderChoice::Google,
            Self::Copilot => super::provider_init::ProviderChoice::Copilot,
            Self::Cursor => super::provider_init::ProviderChoice::Cursor,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Openai => "openai",
            Self::Gemini => "gemini",
            Self::Antigravity => "antigravity",
            Self::Google => "google",
            Self::Copilot => "copilot",
            Self::Cursor => "cursor",
        }
    }

    fn supports_smoke(self) -> bool {
        !matches!(self, Self::Google)
    }

    fn from_provider_choice(choice: &super::provider_init::ProviderChoice) -> Option<Self> {
        match choice {
            super::provider_init::ProviderChoice::Claude
            | super::provider_init::ProviderChoice::ClaudeSubprocess => Some(Self::Claude),
            super::provider_init::ProviderChoice::Openai => Some(Self::Openai),
            super::provider_init::ProviderChoice::Gemini => Some(Self::Gemini),
            super::provider_init::ProviderChoice::Antigravity => Some(Self::Antigravity),
            super::provider_init::ProviderChoice::Google => Some(Self::Google),
            super::provider_init::ProviderChoice::Copilot => Some(Self::Copilot),
            super::provider_init::ProviderChoice::Cursor => Some(Self::Cursor),
            _ => None,
        }
    }

    fn credential_paths(self) -> Result<Vec<String>> {
        match self {
            Self::Claude => Ok(vec![
                crate::auth::claude::jcode_path()?.display().to_string(),
                crate::storage::user_home_path(".claude/.credentials.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".local/share/opencode/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".pi/agent/auth.json")?
                    .display()
                    .to_string(),
            ]),
            Self::Openai => Ok(vec![
                crate::storage::jcode_dir()?
                    .join("openai-auth.json")
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".codex/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".local/share/opencode/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".pi/agent/auth.json")?
                    .display()
                    .to_string(),
            ]),
            Self::Gemini => Ok(vec![
                crate::auth::gemini::tokens_path()?.display().to_string(),
                crate::auth::gemini::gemini_cli_oauth_path()?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".local/share/opencode/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".pi/agent/auth.json")?
                    .display()
                    .to_string(),
            ]),
            Self::Antigravity => Ok(vec![
                crate::auth::antigravity::tokens_path()?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".local/share/opencode/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".pi/agent/auth.json")?
                    .display()
                    .to_string(),
            ]),
            Self::Google => Ok(vec![
                crate::auth::google::credentials_path()?
                    .display()
                    .to_string(),
                crate::auth::google::tokens_path()?.display().to_string(),
            ]),
            Self::Copilot => Ok(vec![
                crate::storage::user_home_path(".copilot/config.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".config/github-copilot/hosts.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".config/github-copilot/apps.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".local/share/opencode/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".pi/agent/auth.json")?
                    .display()
                    .to_string(),
            ]),
            Self::Cursor => Ok(vec![
                dirs::config_dir()
                    .ok_or_else(|| anyhow::anyhow!("No config directory found"))?
                    .join("jcode")
                    .join("cursor.env")
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".config/Cursor/User/globalStorage/state.vscdb")?
                    .display()
                    .to_string(),
            ]),
        }
    }
}

#[derive(Debug, Serialize)]
struct AuthTestStepReport {
    name: String,
    ok: bool,
    detail: String,
}

#[derive(Debug, Serialize)]
struct AuthTestProviderReport {
    provider: String,
    credential_paths: Vec<String>,
    steps: Vec<AuthTestStepReport>,
    smoke_output: Option<String>,
    tool_smoke_output: Option<String>,
    success: bool,
}

impl AuthTestProviderReport {
    fn new(target: AuthTestTarget) -> Self {
        Self {
            provider: target.label().to_string(),
            credential_paths: target.credential_paths().unwrap_or_default(),
            steps: Vec::new(),
            smoke_output: None,
            tool_smoke_output: None,
            success: true,
        }
    }

    fn new_generic(provider_id: String, credential_paths: Vec<String>) -> Self {
        Self {
            provider: provider_id,
            credential_paths,
            steps: Vec::new(),
            smoke_output: None,
            tool_smoke_output: None,
            success: true,
        }
    }

    fn push_step(&mut self, name: impl Into<String>, ok: bool, detail: impl Into<String>) {
        if !ok {
            self.success = false;
        }
        self.steps.push(AuthTestStepReport {
            name: name.into(),
            ok,
            detail: detail.into(),
        });
    }
}

impl ResolvedAuthTestTarget {
    fn from_choice(choice: &super::provider_init::ProviderChoice) -> Option<Self> {
        let provider = super::provider_init::login_provider_for_choice(choice)?;
        Some(match AuthTestTarget::from_provider_choice(choice) {
            Some(target) => Self::Detailed(target),
            None => Self::Generic {
                provider,
                choice: choice.clone(),
            },
        })
    }

    fn from_provider(provider: crate::provider_catalog::LoginProviderDescriptor) -> Option<Self> {
        let choice = super::provider_init::choice_for_login_provider(provider)?;
        Some(match AuthTestTarget::from_provider_choice(&choice) {
            Some(target) => Self::Detailed(target),
            None => Self::Generic { provider, choice },
        })
    }
}

#[derive(Clone, Copy)]
enum AuthTestSmokeKind {
    Provider,
    Tool,
}

impl AuthTestSmokeKind {
    fn step_name(self) -> &'static str {
        match self {
            Self::Provider => "provider_smoke",
            Self::Tool => "tool_smoke",
        }
    }

    fn skipped_by_flag_detail(self) -> &'static str {
        match self {
            Self::Provider => "Skipped by --no-smoke.",
            Self::Tool => "Skipped by --no-tool-smoke.",
        }
    }

    fn unsupported_detail(self) -> &'static str {
        "Skipped: provider is auth/tool-only and has no model runtime smoke step."
    }

    fn success_detail(self) -> &'static str {
        match self {
            Self::Provider => "Provider returned AUTH_TEST_OK.",
            Self::Tool => "Tool-enabled provider request returned AUTH_TEST_OK.",
        }
    }

    fn failure_detail(self, output: &str) -> String {
        match self {
            Self::Provider => {
                format!("Provider response did not contain AUTH_TEST_OK: {}", output)
            }
            Self::Tool => format!(
                "Tool-enabled provider response did not contain AUTH_TEST_OK: {}",
                output
            ),
        }
    }

    async fn run(
        self,
        target: AuthTestTarget,
        model: Option<&str>,
        prompt: &str,
    ) -> Result<String> {
        self.run_for_choice(&target.provider_choice(), model, prompt)
            .await
    }

    async fn run_for_choice(
        self,
        choice: &super::provider_init::ProviderChoice,
        model: Option<&str>,
        prompt: &str,
    ) -> Result<String> {
        match self {
            Self::Provider => run_provider_smoke_for_choice(choice, model, prompt).await,
            Self::Tool => run_provider_tool_smoke_for_choice(choice, model, prompt).await,
        }
    }

    fn set_output(self, report: &mut AuthTestProviderReport, output: String) {
        match self {
            Self::Provider => report.smoke_output = Some(output),
            Self::Tool => report.tool_smoke_output = Some(output),
        }
    }
}

fn push_result_step<T, E, F>(
    report: &mut AuthTestProviderReport,
    name: &'static str,
    result: std::result::Result<T, E>,
    detail: F,
) -> Option<T>
where
    E: std::fmt::Display,
    F: FnOnce(&T) -> String,
{
    match result {
        Ok(value) => {
            report.push_step(name, true, detail(&value));
            Some(value)
        }
        Err(err) => {
            report.push_step(name, false, err.to_string());
            None
        }
    }
}

fn auth_email_suffix(email: Option<&str>) -> String {
    email
        .map(|email| format!(" for {}", email))
        .unwrap_or_default()
}

async fn maybe_run_auth_test_smoke(
    report: &mut AuthTestProviderReport,
    kind: AuthTestSmokeKind,
    target: AuthTestTarget,
    model: Option<&str>,
    enabled: bool,
    prompt: &str,
) {
    if enabled && report.success && target.supports_smoke() {
        match kind.run(target, model, prompt).await {
            Ok(output) => {
                let ok = output.contains("AUTH_TEST_OK");
                kind.set_output(report, output.clone());
                report.push_step(
                    kind.step_name(),
                    ok,
                    if ok {
                        kind.success_detail().to_string()
                    } else {
                        kind.failure_detail(&output)
                    },
                );
            }
            Err(err) => report.push_step(kind.step_name(), false, format!("{err:#}")),
        }
    } else if !target.supports_smoke() {
        report.push_step(kind.step_name(), true, kind.unsupported_detail());
    } else if !enabled {
        report.push_step(kind.step_name(), true, kind.skipped_by_flag_detail());
    }
}

async fn maybe_run_auth_test_smoke_for_choice(
    report: &mut AuthTestProviderReport,
    kind: AuthTestSmokeKind,
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    enabled: bool,
    prompt: &str,
) {
    if enabled && report.success {
        match auth_test_choice_plan(choice, model).await {
            Ok(AuthTestChoicePlan::Run { model }) => {
                match kind.run_for_choice(choice, model.as_deref(), prompt).await {
                    Ok(output) => {
                        let ok = output.contains("AUTH_TEST_OK");
                        kind.set_output(report, output.clone());
                        report.push_step(
                            kind.step_name(),
                            ok,
                            if ok {
                                kind.success_detail().to_string()
                            } else {
                                kind.failure_detail(&output)
                            },
                        );
                    }
                    Err(err) => report.push_step(kind.step_name(), false, format!("{err:#}")),
                }
            }
            Ok(AuthTestChoicePlan::Skip(detail)) => {
                report.push_step(kind.step_name(), true, detail);
            }
            Err(err) => report.push_step(kind.step_name(), false, format!("{err:#}")),
        }
    } else if !enabled {
        report.push_step(kind.step_name(), true, kind.skipped_by_flag_detail());
    }
}

pub(crate) async fn run_post_login_validation(
    provider: crate::provider_catalog::LoginProviderDescriptor,
) -> Result<()> {
    let Some(choice) = super::provider_init::choice_for_login_provider(provider) else {
        eprintln!(
            "\nSkipping automatic runtime validation for {}. Auto Import can add multiple providers; run `jcode auth-test --all-configured` to validate them.",
            provider.display_name
        );
        return Ok(());
    };

    super::provider_init::apply_login_provider_profile_env(provider);

    eprintln!(
        "\nValidating {} login with live auth/runtime checks...",
        provider.display_name
    );

    let report = if let Some(target) = AuthTestTarget::from_provider_choice(&choice) {
        populate_auth_test_target_report(
            target,
            None,
            true,
            true,
            DEFAULT_AUTH_TEST_PROVIDER_PROMPT,
            DEFAULT_AUTH_TEST_TOOL_PROMPT,
            AuthTestProviderReport::new(target),
        )
        .await
    } else {
        populate_generic_auth_test_report(
            provider,
            choice.clone(),
            None,
            true,
            true,
            DEFAULT_AUTH_TEST_PROVIDER_PROMPT,
            DEFAULT_AUTH_TEST_TOOL_PROMPT,
            AuthTestProviderReport::new_generic(
                choice.as_arg_value().to_string(),
                generic_credential_paths_for_provider(provider),
            ),
        )
        .await
    };

    persist_auth_test_report(&report);
    print_auth_test_reports(std::slice::from_ref(&report));

    if report.success {
        Ok(())
    } else if AuthTestTarget::from_provider_choice(&choice).is_some() {
        anyhow::bail!(
            "Post-login validation failed for {}. Credentials were saved, but jcode could not verify runtime readiness. Re-run `jcode auth-test --provider {}` for details.",
            provider.display_name,
            choice.as_arg_value()
        )
    } else {
        anyhow::bail!(
            "Post-login validation failed for {}. Credentials were saved, but jcode could not verify runtime readiness. Re-test with `jcode --provider {} run \"Reply with exactly AUTH_TEST_OK and nothing else.\"` after fixing the provider/runtime.",
            provider.display_name,
            choice.as_arg_value()
        )
    }
}

pub async fn run_auth_test_command(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    login: bool,
    all_configured: bool,
    no_smoke: bool,
    no_tool_smoke: bool,
    prompt: Option<&str>,
    emit_json: bool,
    output_path: Option<&str>,
) -> Result<()> {
    let targets = resolve_auth_test_targets(choice, all_configured)?;
    let provider_smoke_prompt = prompt.unwrap_or(DEFAULT_AUTH_TEST_PROVIDER_PROMPT);
    let tool_smoke_prompt = prompt.unwrap_or(DEFAULT_AUTH_TEST_TOOL_PROMPT);

    let mut reports = Vec::new();
    for target in targets {
        let report = match target {
            ResolvedAuthTestTarget::Detailed(target) => {
                run_auth_test_target(
                    target,
                    model,
                    login,
                    !no_smoke,
                    !no_tool_smoke,
                    provider_smoke_prompt,
                    tool_smoke_prompt,
                )
                .await
            }
            ResolvedAuthTestTarget::Generic { provider, choice } => {
                run_generic_auth_test_target(
                    provider,
                    choice,
                    model,
                    login,
                    !no_smoke,
                    !no_tool_smoke,
                    provider_smoke_prompt,
                    tool_smoke_prompt,
                )
                .await
            }
        };
        persist_auth_test_report(&report);
        reports.push(report);
    }

    let report_json = if emit_json || output_path.is_some() {
        Some(serde_json::to_string_pretty(&reports)?)
    } else {
        None
    };

    if let Some(path) = output_path {
        std::fs::write(path, report_json.as_deref().unwrap_or("[]"))
            .with_context(|| format!("failed to write auth-test report to {}", path))?;
    }

    if emit_json {
        println!("{}", report_json.as_deref().unwrap_or("[]"));
    } else {
        print_auth_test_reports(&reports);
    }

    if reports.iter().all(|report| report.success) {
        Ok(())
    } else {
        anyhow::bail!("One or more auth tests failed")
    }
}

fn resolve_auth_test_targets(
    choice: &super::provider_init::ProviderChoice,
    all_configured: bool,
) -> Result<Vec<ResolvedAuthTestTarget>> {
    if all_configured || matches!(choice, super::provider_init::ProviderChoice::Auto) {
        let status = crate::auth::AuthStatus::check();
        let targets = configured_auth_test_targets(&status);
        if targets.is_empty() {
            anyhow::bail!(
                "No configured supported auth providers found. Run `jcode login --provider <provider>` first, or choose an explicit --provider."
            );
        }
        return Ok(targets);
    }

    ResolvedAuthTestTarget::from_choice(choice)
        .map(|target| vec![target])
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Provider '{}' is not yet supported by `jcode auth-test`.",
                choice.as_arg_value()
            )
        })
}

fn configured_auth_test_targets(status: &crate::auth::AuthStatus) -> Vec<ResolvedAuthTestTarget> {
    crate::provider_catalog::auth_status_login_providers()
        .into_iter()
        .filter(|provider| {
            status.state_for_provider(*provider) != crate::auth::AuthState::NotConfigured
        })
        .filter_map(ResolvedAuthTestTarget::from_provider)
        .collect()
}

async fn run_auth_test_target(
    target: AuthTestTarget,
    model: Option<&str>,
    login: bool,
    run_smoke: bool,
    run_tool_smoke: bool,
    provider_smoke_prompt: &str,
    tool_smoke_prompt: &str,
) -> AuthTestProviderReport {
    let mut report = AuthTestProviderReport::new(target);

    if login {
        match super::login::run_login(&target.provider_choice(), None).await {
            Ok(()) => report.push_step("login", true, "Login flow completed."),
            Err(err) => report.push_step("login", false, err.to_string()),
        }
    }

    populate_auth_test_target_report(
        target,
        model,
        run_smoke,
        run_tool_smoke,
        provider_smoke_prompt,
        tool_smoke_prompt,
        report,
    )
    .await
}

async fn populate_auth_test_target_report(
    target: AuthTestTarget,
    model: Option<&str>,
    run_smoke: bool,
    run_tool_smoke: bool,
    provider_smoke_prompt: &str,
    tool_smoke_prompt: &str,
    mut report: AuthTestProviderReport,
) -> AuthTestProviderReport {
    match target {
        AuthTestTarget::Claude => probe_claude_auth(&mut report).await,
        AuthTestTarget::Openai => probe_openai_auth(&mut report).await,
        AuthTestTarget::Gemini => probe_gemini_auth(&mut report).await,
        AuthTestTarget::Antigravity => probe_antigravity_auth(&mut report).await,
        AuthTestTarget::Google => probe_google_auth(&mut report).await,
        AuthTestTarget::Copilot => probe_copilot_auth(&mut report).await,
        AuthTestTarget::Cursor => probe_cursor_auth(&mut report).await,
    }

    maybe_run_auth_test_smoke(
        &mut report,
        AuthTestSmokeKind::Provider,
        target,
        model,
        run_smoke,
        provider_smoke_prompt,
    )
    .await;

    maybe_run_auth_test_smoke(
        &mut report,
        AuthTestSmokeKind::Tool,
        target,
        model,
        run_tool_smoke,
        tool_smoke_prompt,
    )
    .await;

    report
}

async fn run_generic_auth_test_target(
    provider: crate::provider_catalog::LoginProviderDescriptor,
    choice: super::provider_init::ProviderChoice,
    model: Option<&str>,
    login: bool,
    run_smoke: bool,
    run_tool_smoke: bool,
    provider_smoke_prompt: &str,
    tool_smoke_prompt: &str,
) -> AuthTestProviderReport {
    let mut report = AuthTestProviderReport::new_generic(
        choice.as_arg_value().to_string(),
        generic_credential_paths_for_provider(provider),
    );

    if login {
        match super::login::run_login(&choice, None).await {
            Ok(()) => report.push_step("login", true, "Login flow completed."),
            Err(err) => report.push_step("login", false, err.to_string()),
        }
    }

    populate_generic_auth_test_report(
        provider,
        choice,
        model,
        run_smoke,
        run_tool_smoke,
        provider_smoke_prompt,
        tool_smoke_prompt,
        report,
    )
    .await
}

async fn populate_generic_auth_test_report(
    provider: crate::provider_catalog::LoginProviderDescriptor,
    choice: super::provider_init::ProviderChoice,
    model: Option<&str>,
    run_smoke: bool,
    run_tool_smoke: bool,
    provider_smoke_prompt: &str,
    tool_smoke_prompt: &str,
    mut report: AuthTestProviderReport,
) -> AuthTestProviderReport {
    super::provider_init::apply_login_provider_profile_env(provider);
    probe_generic_provider_auth(provider, &mut report);

    maybe_run_auth_test_smoke_for_choice(
        &mut report,
        AuthTestSmokeKind::Provider,
        &choice,
        model,
        run_smoke,
        provider_smoke_prompt,
    )
    .await;

    maybe_run_auth_test_smoke_for_choice(
        &mut report,
        AuthTestSmokeKind::Tool,
        &choice,
        model,
        run_tool_smoke,
        tool_smoke_prompt,
    )
    .await;

    report
}

fn persist_auth_test_report(report: &AuthTestProviderReport) {
    let step_map = report
        .steps
        .iter()
        .map(|step| (step.name.as_str(), step.ok))
        .collect::<HashMap<_, _>>();
    let summary = report
        .steps
        .iter()
        .find(|step| !step.ok)
        .map(|step| format!("{}: {}", step.name, step.detail))
        .or_else(|| {
            report
                .steps
                .last()
                .map(|step| format!("{}: {}", step.name, step.detail))
        })
        .unwrap_or_else(|| "No validation steps recorded.".to_string());

    let record = crate::auth::validation::ProviderValidationRecord {
        checked_at_ms: chrono::Utc::now().timestamp_millis(),
        success: report.success,
        provider_smoke_ok: step_map.get("provider_smoke").copied(),
        tool_smoke_ok: step_map.get("tool_smoke").copied(),
        summary,
    };

    if let Err(err) = crate::auth::validation::save(&report.provider, record) {
        crate::logging::warn(&format!(
            "failed to persist auth validation result for {}: {}",
            report.provider, err
        ));
    }
}

fn generic_credential_paths_for_provider(
    provider: crate::provider_catalog::LoginProviderDescriptor,
) -> Vec<String> {
    let Ok(config_dir) = crate::storage::app_config_dir() else {
        return Vec::new();
    };

    match provider.target {
        crate::provider_catalog::LoginProviderTarget::Jcode => {
            vec![config_dir.join(crate::subscription_catalog::JCODE_ENV_FILE)]
        }
        crate::provider_catalog::LoginProviderTarget::OpenRouter => {
            vec![config_dir.join("openrouter.env")]
        }
        crate::provider_catalog::LoginProviderTarget::Azure => {
            vec![config_dir.join(crate::auth::azure::ENV_FILE)]
        }
        crate::provider_catalog::LoginProviderTarget::OpenAiCompatible(profile) => {
            let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
            vec![config_dir.join(resolved.env_file)]
        }
        _ => Vec::new(),
    }
    .into_iter()
    .map(|path| path.display().to_string())
    .collect()
}

fn probe_generic_provider_auth(
    provider: crate::provider_catalog::LoginProviderDescriptor,
    report: &mut AuthTestProviderReport,
) {
    let status = crate::auth::AuthStatus::check();
    let state = status.state_for_provider(provider);
    let detail = status.method_detail_for_provider(provider);
    report.push_step(
        "credential_probe",
        state == crate::auth::AuthState::Available,
        format!(
            "{} auth status is {} ({detail}).",
            provider.display_name,
            auth_state_label(state),
        ),
    );
    report.push_step(
        "refresh_probe",
        true,
        "Skipped: provider does not expose a dedicated refresh probe in jcode today.".to_string(),
    );
}

async fn probe_claude_auth(report: &mut AuthTestProviderReport) {
    if let Some(creds) = push_result_step(
        report,
        "credential_probe",
        crate::auth::claude::load_credentials(),
        |creds| {
            format!(
                "Loaded Claude credentials (expires_at={}).",
                creds.expires_at
            )
        },
    ) {
        push_result_step(
            report,
            "refresh_probe",
            crate::auth::oauth::refresh_claude_tokens(&creds.refresh_token).await,
            |tokens| {
                format!(
                    "Claude token refresh succeeded (new_expires_at={}).",
                    tokens.expires_at
                )
            },
        );
    }
}

async fn probe_openai_auth(report: &mut AuthTestProviderReport) {
    if let Some(creds) = push_result_step(
        report,
        "credential_probe",
        crate::auth::codex::load_credentials(),
        |creds| {
            if creds.refresh_token.trim().is_empty() {
                "Loaded OpenAI API key credentials (no refresh token present).".to_string()
            } else {
                format!(
                    "Loaded OpenAI OAuth credentials (expires_at={:?}).",
                    creds.expires_at
                )
            }
        },
    ) {
        if creds.refresh_token.trim().is_empty() {
            report.push_step(
                "refresh_probe",
                true,
                "Skipped: OpenAI is using API key auth, not OAuth.",
            );
        } else {
            push_result_step(
                report,
                "refresh_probe",
                crate::auth::oauth::refresh_openai_tokens(&creds.refresh_token).await,
                |tokens| {
                    format!(
                        "OpenAI token refresh succeeded (new_expires_at={}).",
                        tokens.expires_at
                    )
                },
            );
        }
    }
}

async fn probe_gemini_auth(report: &mut AuthTestProviderReport) {
    if push_result_step(
        report,
        "credential_probe",
        crate::auth::gemini::load_tokens(),
        |tokens| {
            format!(
                "Loaded Gemini tokens{} (expires_at={}).",
                auth_email_suffix(tokens.email.as_deref()),
                tokens.expires_at
            )
        },
    )
    .is_some()
    {
        push_result_step(
            report,
            "refresh_probe",
            crate::auth::gemini::load_or_refresh_tokens().await,
            |tokens| {
                format!(
                    "Gemini token load/refresh succeeded (expires_at={}).",
                    tokens.expires_at
                )
            },
        );
    }
}

async fn probe_antigravity_auth(report: &mut AuthTestProviderReport) {
    if push_result_step(
        report,
        "credential_probe",
        crate::auth::antigravity::load_tokens(),
        |tokens| {
            format!(
                "Loaded Antigravity OAuth tokens{} (expires_at={}).",
                auth_email_suffix(tokens.email.as_deref()),
                tokens.expires_at
            )
        },
    )
    .is_some()
    {
        push_result_step(
            report,
            "refresh_probe",
            crate::auth::antigravity::load_or_refresh_tokens().await,
            |tokens| {
                format!(
                    "Antigravity token load/refresh succeeded (expires_at={}).",
                    tokens.expires_at
                )
            },
        );
    }
}

async fn probe_google_auth(report: &mut AuthTestProviderReport) {
    let creds_result = crate::auth::google::load_credentials();
    let tokens_result = crate::auth::google::load_tokens();
    match (creds_result, tokens_result) {
        (Ok(creds), Ok(tokens)) => {
            report.push_step(
                "credential_probe",
                true,
                format!(
                    "Loaded Google credentials (client_id={}...) and Gmail tokens{}.",
                    &creds.client_id[..20.min(creds.client_id.len())],
                    auth_email_suffix(tokens.email.as_deref())
                ),
            );
            match crate::auth::google::get_valid_token().await {
                Ok(_) => report.push_step(
                    "refresh_probe",
                    true,
                    "Google/Gmail token load/refresh succeeded.".to_string(),
                ),
                Err(err) => report.push_step("refresh_probe", false, err.to_string()),
            }
        }
        (Err(err), _) => report.push_step("credential_probe", false, err.to_string()),
        (_, Err(err)) => report.push_step("credential_probe", false, err.to_string()),
    }
}

async fn probe_copilot_auth(report: &mut AuthTestProviderReport) {
    if let Some(token) = push_result_step(
        report,
        "credential_probe",
        crate::auth::copilot::load_github_token(),
        |token| {
            format!(
                "Loaded GitHub OAuth token for Copilot ({} chars).",
                token.len()
            )
        },
    ) {
        let client = reqwest::Client::new();
        push_result_step(
            report,
            "refresh_probe",
            crate::auth::copilot::exchange_github_token(&client, &token).await,
            |api_token| {
                format!(
                    "Exchanged GitHub token for Copilot API token (expires_at={}).",
                    api_token.expires_at
                )
            },
        );
    }
}

async fn probe_cursor_auth(report: &mut AuthTestProviderReport) {
    let has_agent_auth = crate::auth::cursor::has_cursor_agent_auth();
    let has_api_key = crate::auth::cursor::has_cursor_api_key();
    let has_vscdb = crate::auth::cursor::has_cursor_vscdb_token();
    let ok = has_agent_auth || has_api_key || has_vscdb;
    report.push_step(
        "credential_probe",
        ok,
        format!(
            "Cursor auth sources: agent_session={}, api_key={}, vscdb_token={}",
            has_agent_auth, has_api_key, has_vscdb
        ),
    );
    report.push_step(
        "refresh_probe",
        true,
        "Skipped: Cursor provider does not expose a native refresh-token probe in jcode today."
            .to_string(),
    );
}

#[derive(Debug)]
enum AuthTestChoicePlan {
    Run { model: Option<String> },
    Skip(String),
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatibleModelsResponse {
    #[serde(default)]
    data: Vec<OpenAiCompatibleModelInfo>,
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatibleModelInfo {
    id: String,
}

async fn auth_test_choice_plan(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
) -> Result<AuthTestChoicePlan> {
    if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
        return Ok(AuthTestChoicePlan::Run {
            model: Some(model.to_string()),
        });
    }

    let Some(profile) = super::provider_init::profile_for_choice(choice) else {
        return Ok(AuthTestChoicePlan::Run { model: None });
    };
    let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
    if resolved.requires_api_key || resolved.default_model.is_some() {
        return Ok(AuthTestChoicePlan::Run { model: None });
    }

    crate::provider_catalog::apply_openai_compatible_profile_env(Some(profile));
    let discovered_model = discover_openai_compatible_validation_model(&resolved).await?;
    if let Some(model) = discovered_model {
        return Ok(AuthTestChoicePlan::Run { model: Some(model) });
    }

    Ok(AuthTestChoicePlan::Skip(format!(
        "Skipped: {} local endpoint reported no models. Re-run `jcode auth-test --provider {} --model <local-model>` or set a default model first.",
        resolved.display_name,
        choice.as_arg_value()
    )))
}

async fn discover_openai_compatible_validation_model(
    profile: &crate::provider_catalog::ResolvedOpenAiCompatibleProfile,
) -> Result<Option<String>> {
    let url = format!("{}/models", profile.api_base.trim_end_matches('/'));
    let mut request = crate::provider::shared_http_client().get(&url);
    if let Some(api_key) = crate::provider_catalog::load_api_key_from_env_or_config(
        &profile.api_key_env,
        &profile.env_file,
    ) {
        request = request.bearer_auth(api_key);
    }

    let response = request.send().await.with_context(|| {
        format!(
            "Failed to query {} models from {} during auth-test validation",
            profile.display_name, url
        )
    })?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!(
            "{} model discovery failed (HTTP {}): {}",
            profile.display_name,
            status,
            body.trim()
        );
    }

    let parsed: OpenAiCompatibleModelsResponse =
        serde_json::from_str(&body).with_context(|| {
            format!(
                "Failed to parse {} model discovery response from {}",
                profile.display_name, url
            )
        })?;
    Ok(parsed
        .data
        .into_iter()
        .map(|model| model.id.trim().to_string())
        .find(|model| !model.is_empty()))
}

async fn run_provider_smoke_for_choice(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    prompt: &str,
) -> Result<String> {
    run_auth_test_with_retry(async || {
        let provider = super::provider_init::init_provider_for_validation(choice, model)
            .await
            .with_context(|| format!("Failed to initialize {} provider", choice.as_arg_value()))?;
        let output = provider
            .complete_simple(prompt, "")
            .await
            .with_context(|| format!("{} provider smoke prompt failed", choice.as_arg_value()))?;
        Ok(output.trim().to_string())
    })
    .await
}

async fn run_provider_tool_smoke_for_choice(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    prompt: &str,
) -> Result<String> {
    use futures::StreamExt;

    run_auth_test_with_retry(async || {
        let (provider, registry) =
            super::provider_init::init_provider_and_registry_for_validation(choice, model)
                .await
                .with_context(|| {
                    format!("Failed to initialize {} provider", choice.as_arg_value())
                })?;
        registry
            .register_mcp_tools(None, None, Some("auth-test".to_string()))
            .await;
        let tools = registry.definitions(None).await;

        let messages = vec![crate::message::Message {
            role: crate::message::Role::User,
            content: vec![crate::message::ContentBlock::Text {
                text: prompt.to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        }];

        let response = provider
            .complete(&messages, &tools, "", None)
            .await
            .with_context(|| {
                format!(
                    "{} tool-enabled smoke prompt failed with {} attached tools",
                    choice.as_arg_value(),
                    tools.len()
                )
            })?;

        tokio::pin!(response);
        let mut output = String::new();
        while let Some(event) = response.next().await {
            match event {
                Ok(crate::message::StreamEvent::TextDelta(text)) => output.push_str(&text),
                Ok(_) => {}
                Err(err) => return Err(err),
            }
        }

        Ok(output.trim().to_string())
    })
    .await
}

async fn run_auth_test_with_retry<F, Fut>(mut f: F) -> Result<String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<String>>,
{
    const RETRY_DELAYS: &[Duration] = &[Duration::from_secs(3), Duration::from_secs(8)];

    let mut last_err = None;
    for (attempt, delay) in RETRY_DELAYS.iter().enumerate() {
        match f().await {
            Ok(output) => return Ok(output),
            Err(err) if auth_test_error_is_retryable(&err) => {
                last_err = Some(err);
                crate::logging::warn(&format!(
                    "auth-test transient failure on attempt {} - retrying in {}s",
                    attempt + 1,
                    delay.as_secs()
                ));
                tokio::time::sleep(*delay).await;
            }
            Err(err) => return Err(err),
        }
    }

    match f().await {
        Ok(output) => Ok(output),
        Err(err) if last_err.is_some() => Err(err),
        Err(err) => Err(err),
    }
}

fn auth_test_error_is_retryable(err: &anyhow::Error) -> bool {
    let text = format!("{err:#}").to_ascii_lowercase();
    [
        "http 429",
        "too many requests",
        "resource_exhausted",
        "rate_limit_exceeded",
        "rate limit",
        "temporarily unavailable",
        "timeout",
        "connection reset",
        "service unavailable",
        "http 500",
        "http 502",
        "http 503",
        "http 504",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn print_auth_test_reports(reports: &[AuthTestProviderReport]) {
    for report in reports {
        println!("=== auth-test: {} ===", report.provider);
        if !report.credential_paths.is_empty() {
            println!("credential paths:");
            for path in &report.credential_paths {
                println!("  - {}", path);
            }
        }
        for step in &report.steps {
            let marker = if step.ok { "✓" } else { "✗" };
            println!("{} {} — {}", marker, step.name, step.detail);
        }
        if let Some(output) = report.smoke_output.as_deref() {
            println!("smoke output: {}", output);
        }
        if let Some(output) = report.tool_smoke_output.as_deref() {
            println!("tool smoke output: {}", output);
        }
        println!("result: {}\n", if report.success { "PASS" } else { "FAIL" });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthState, AuthStatus, ProviderAuth};
    use crate::provider::ModelRoute;
    use std::io::{Read, Write};

    struct SavedEnv {
        vars: Vec<(String, Option<String>)>,
    }

    impl SavedEnv {
        fn capture(keys: &[&str]) -> Self {
            Self {
                vars: keys
                    .iter()
                    .map(|key| (key.to_string(), std::env::var(key).ok()))
                    .collect(),
            }
        }
    }

    impl Drop for SavedEnv {
        fn drop(&mut self) {
            for (key, value) in &self.vars {
                if let Some(value) = value {
                    crate::env::set_var(key, value);
                } else {
                    crate::env::remove_var(key);
                }
            }
        }
    }

    fn spawn_single_response_http_server(status: u16, body: &str) -> String {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let body = body.to_string();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let status_text = match status {
                200 => "OK",
                400 => "Bad Request",
                404 => "Not Found",
                500 => "Internal Server Error",
                _ => "OK",
            };
            let response = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status,
                status_text,
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });
        format!("http://{}/v1", addr)
    }

    #[test]
    fn test_parse_tailscale_dns_name_trims_trailing_dot() {
        let payload = br#"{"Self":{"DNSName":"yashmacbook.tailabc.ts.net."}}"#;
        let parsed = parse_tailscale_dns_name(payload);
        assert_eq!(parsed.as_deref(), Some("yashmacbook.tailabc.ts.net"));
    }

    #[test]
    fn test_parse_tailscale_dns_name_handles_missing_or_empty() {
        let missing = br#"{"Self":{}}"#;
        assert!(parse_tailscale_dns_name(missing).is_none());

        let empty = br#"{"Self":{"DNSName":"   "}}"#;
        assert!(parse_tailscale_dns_name(empty).is_none());
    }

    #[test]
    fn test_parse_tailscale_dns_name_invalid_json() {
        assert!(parse_tailscale_dns_name(b"not-json").is_none());
    }

    #[test]
    fn configured_auth_test_targets_only_include_configured_supported_providers() {
        let status = AuthStatus {
            anthropic: ProviderAuth {
                state: AuthState::Available,
                has_oauth: true,
                has_api_key: false,
            },
            openai: AuthState::NotConfigured,
            gemini: AuthState::Available,
            google: AuthState::Expired,
            copilot: AuthState::Available,
            cursor: AuthState::NotConfigured,
            ..AuthStatus::default()
        };

        let targets = configured_auth_test_targets(&status);
        assert_eq!(
            targets,
            vec![
                ResolvedAuthTestTarget::Detailed(AuthTestTarget::Claude),
                ResolvedAuthTestTarget::Generic {
                    provider: crate::provider_catalog::OPENROUTER_LOGIN_PROVIDER,
                    choice: super::super::provider_init::ProviderChoice::Openrouter,
                },
                ResolvedAuthTestTarget::Detailed(AuthTestTarget::Copilot),
                ResolvedAuthTestTarget::Detailed(AuthTestTarget::Gemini)
            ]
        );
    }

    #[test]
    fn explicit_supported_provider_maps_to_single_auth_target() {
        let targets =
            resolve_auth_test_targets(&super::super::provider_init::ProviderChoice::Gemini, false)
                .expect("resolve target");
        assert_eq!(
            targets,
            vec![ResolvedAuthTestTarget::Detailed(AuthTestTarget::Gemini)]
        );
    }

    #[test]
    fn explicit_generic_provider_maps_to_generic_auth_target() {
        let targets = resolve_auth_test_targets(
            &super::super::provider_init::ProviderChoice::Openrouter,
            false,
        )
        .expect("resolve target");
        assert_eq!(
            targets,
            vec![ResolvedAuthTestTarget::Generic {
                provider: crate::provider_catalog::OPENROUTER_LOGIN_PROVIDER,
                choice: super::super::provider_init::ProviderChoice::Openrouter,
            }]
        );
    }

    #[test]
    fn collect_cli_model_names_prefers_available_routes_and_dedupes() {
        let routes = vec![
            ModelRoute {
                model: "gpt-5.4".to_string(),
                provider: "OpenAI".to_string(),
                api_method: "openai-oauth".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            },
            ModelRoute {
                model: "gpt-5.4".to_string(),
                provider: "auto".to_string(),
                api_method: "openrouter".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            },
            ModelRoute {
                model: "openrouter models".to_string(),
                provider: "—".to_string(),
                api_method: "openrouter".to_string(),
                available: false,
                detail: "OPENROUTER_API_KEY not set".to_string(),
                cheapness: None,
            },
        ];

        let models = collect_cli_model_names(
            &routes,
            vec!["gpt-5.4".to_string(), "claude-sonnet-4".to_string()],
        );

        assert_eq!(models, vec!["gpt-5.4", "claude-sonnet-4"]);
    }

    #[test]
    fn auth_test_retryable_error_detection_handles_rate_limits() {
        let err = anyhow::anyhow!(
            "Gemini request generateContent failed (HTTP 429 Too Many Requests): RESOURCE_EXHAUSTED"
        );
        assert!(auth_test_error_is_retryable(&err));
    }

    #[test]
    fn auth_test_retryable_error_detection_rejects_schema_errors() {
        let err = anyhow::anyhow!(
            "Gemini request generateContent failed (HTTP 400 Bad Request): invalid argument"
        );
        assert!(!auth_test_error_is_retryable(&err));
    }

    #[tokio::test]
    async fn auth_test_choice_plan_preserves_explicit_model_for_local_provider() {
        let plan = auth_test_choice_plan(
            &super::super::provider_init::ProviderChoice::Ollama,
            Some("llama3.2"),
        )
        .await
        .expect("choice plan");

        match plan {
            AuthTestChoicePlan::Run { model } => assert_eq!(model.as_deref(), Some("llama3.2")),
            AuthTestChoicePlan::Skip(detail) => panic!("unexpected skip: {detail}"),
        }
    }

    #[tokio::test]
    async fn auth_test_choice_plan_leaves_non_compat_provider_unchanged() {
        let plan = auth_test_choice_plan(
            &super::super::provider_init::ProviderChoice::Openrouter,
            None,
        )
        .await
        .expect("choice plan");

        match plan {
            AuthTestChoicePlan::Run { model } => assert!(model.is_none()),
            AuthTestChoicePlan::Skip(detail) => panic!("unexpected skip: {detail}"),
        }
    }

    #[tokio::test]
    async fn auth_test_choice_plan_discovers_model_for_local_custom_compat_endpoint() {
        let _env_guard = crate::storage::lock_test_env();
        let _saved = SavedEnv::capture(&[
            "JCODE_OPENAI_COMPAT_API_BASE",
            "JCODE_OPENAI_COMPAT_API_KEY_NAME",
            "JCODE_OPENAI_COMPAT_ENV_FILE",
            "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
            "JCODE_OPENAI_COMPAT_LOCAL_ENABLED",
            "JCODE_OPENROUTER_API_BASE",
            "JCODE_OPENROUTER_API_KEY_NAME",
            "JCODE_OPENROUTER_ENV_FILE",
            "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        ]);
        let api_base = spawn_single_response_http_server(200, r#"{"data":[{"id":"llama3.2"}]}"#);
        crate::env::set_var("JCODE_OPENAI_COMPAT_API_BASE", &api_base);
        crate::env::remove_var("JCODE_OPENAI_COMPAT_DEFAULT_MODEL");
        crate::env::remove_var("JCODE_OPENAI_COMPAT_LOCAL_ENABLED");
        crate::provider_catalog::apply_openai_compatible_profile_env(None);

        let plan = auth_test_choice_plan(
            &super::super::provider_init::ProviderChoice::OpenaiCompatible,
            None,
        )
        .await
        .expect("choice plan");

        match plan {
            AuthTestChoicePlan::Run { model } => assert_eq!(model.as_deref(), Some("llama3.2")),
            AuthTestChoicePlan::Skip(detail) => panic!("unexpected skip: {detail}"),
        }
    }

    #[tokio::test]
    async fn auth_test_choice_plan_skips_local_custom_compat_endpoint_without_models() {
        let _env_guard = crate::storage::lock_test_env();
        let _saved = SavedEnv::capture(&[
            "JCODE_OPENAI_COMPAT_API_BASE",
            "JCODE_OPENAI_COMPAT_API_KEY_NAME",
            "JCODE_OPENAI_COMPAT_ENV_FILE",
            "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
            "JCODE_OPENAI_COMPAT_LOCAL_ENABLED",
            "JCODE_OPENROUTER_API_BASE",
            "JCODE_OPENROUTER_API_KEY_NAME",
            "JCODE_OPENROUTER_ENV_FILE",
            "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        ]);
        let api_base = spawn_single_response_http_server(200, r#"{"data":[]}"#);
        crate::env::set_var("JCODE_OPENAI_COMPAT_API_BASE", &api_base);
        crate::env::remove_var("JCODE_OPENAI_COMPAT_DEFAULT_MODEL");
        crate::env::remove_var("JCODE_OPENAI_COMPAT_LOCAL_ENABLED");
        crate::provider_catalog::apply_openai_compatible_profile_env(None);

        let plan = auth_test_choice_plan(
            &super::super::provider_init::ProviderChoice::OpenaiCompatible,
            None,
        )
        .await
        .expect("choice plan");

        match plan {
            AuthTestChoicePlan::Run { model } => panic!("unexpected run plan: {model:?}"),
            AuthTestChoicePlan::Skip(detail) => {
                assert!(detail.contains("reported no models"));
                assert!(detail.contains("openai-compatible"));
            }
        }
    }

    #[test]
    fn collect_cli_model_names_falls_back_when_no_routes_are_available() {
        let routes = vec![ModelRoute {
            model: "claude-opus-4-6".to_string(),
            provider: "Anthropic".to_string(),
            api_method: "claude-oauth".to_string(),
            available: false,
            detail: "no credentials".to_string(),
            cheapness: None,
        }];

        let models = collect_cli_model_names(&routes, vec!["gpt-5.4".to_string()]);

        assert_eq!(models, vec!["claude-opus-4-6", "gpt-5.4"]);
    }

    #[test]
    fn list_cli_providers_includes_auto_and_openai() {
        let providers = list_cli_providers();
        assert!(providers.iter().any(|provider| provider.id == "auto"));
        assert!(providers.iter().any(|provider| {
            provider.id == "openai"
                && provider.display_name == "OpenAI"
                && provider.auth_kind.as_deref() == Some("OAuth")
        }));
        assert!(providers.iter().any(|provider| provider.id == "groq"));
        assert!(providers.iter().any(|provider| provider.id == "xai"));
    }

    #[test]
    fn version_command_plain_output_includes_core_fields() {
        let report = VersionReport {
            version: "v1.2.3 (abc1234)".to_string(),
            semver: "1.2.3".to_string(),
            base_semver: "1.2.0".to_string(),
            update_semver: "1.2.0".to_string(),
            git_hash: "abc1234".to_string(),
            git_tag: "v1.2.3".to_string(),
            build_time: "2026-03-18 18:00:00 +0000".to_string(),
            git_date: "2026-03-18 17:59:00 +0000".to_string(),
            release_build: false,
        };
        let text = format!(
            "version\t{}\nsemver\t{}\nbase_semver\t{}\nupdate_semver\t{}\ngit_hash\t{}\ngit_tag\t{}\nbuild_time\t{}\ngit_date\t{}\nrelease_build\t{}\n",
            report.version,
            report.semver,
            report.base_semver,
            report.update_semver,
            report.git_hash,
            report.git_tag,
            report.build_time,
            report.git_date,
            report.release_build
        );

        assert!(text.contains("version\tv1.2.3 (abc1234)"));
        assert!(text.contains("semver\t1.2.3"));
        assert!(text.contains("git_hash\tabc1234"));
        assert!(text.contains("release_build\tfalse"));
    }
}
