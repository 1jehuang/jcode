use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::io::Read;
use std::net::ToSocketAddrs;

use crate::{browser, gateway, memory, storage, tui};

use super::terminal::{cleanup_tui_runtime, init_tui_runtime};

pub enum AmbientSubcommand {
    Status,
    Log,
    Trigger,
    Stop,
    RunVisible,
}

pub async fn run_ambient_command(cmd: AmbientSubcommand) -> Result<()> {
    match cmd {
        AmbientSubcommand::RunVisible => {
            return run_ambient_visible().await;
        }
        _ => {}
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
        let Some(session_id) = crate::dictation::last_focused_session()? else {
            anyhow::bail!(
                "No last-focused live jcode client found. Focus a jcode window first, or use `jcode dictate --type`."
            );
        };
        run_transcript_command(Some(run.text), run.mode, Some(session_id)).await
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

            if scope == "all" || scope == "project" {
                if let Ok(graph) = manager.load_project_graph() {
                    all_memories.extend(graph.all_memories().cloned());
                }
            }
            if scope == "all" || scope == "global" {
                if let Ok(graph) = manager.load_global_graph() {
                    all_memories.extend(graph.all_memories().cloned());
                }
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

            if scope == "all" || scope == "project" {
                if let Ok(graph) = manager.load_project_graph() {
                    all_memories.extend(graph.all_memories().cloned());
                }
            }
            if scope == "all" || scope == "global" {
                if let Ok(graph) = manager.load_global_graph() {
                    all_memories.extend(graph.all_memories().cloned());
                }
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
                    if !overwrite {
                        if let Ok(graph) = manager.load_global_graph() {
                            if graph.get_memory(&entry.id).is_some() {
                                skipped += 1;
                                continue;
                            }
                        }
                    }
                    manager.remember_global(entry)
                } else {
                    if !overwrite {
                        if let Ok(graph) = manager.load_project_graph() {
                            if graph.get_memory(&entry.id).is_some() {
                                skipped += 1;
                                continue;
                            }
                        }
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
    if let Err(_) = qr2term::print_qr(&pair_uri) {
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

pub async fn run_model_command(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    emit_json: bool,
) -> Result<()> {
    let provider = super::provider_init::init_provider(choice, model).await?;

    if let Err(err) = provider.prefetch_models().await {
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
            ]),
            Self::Openai => Ok(vec![
                crate::storage::jcode_dir()?
                    .join("openai-auth.json")
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".codex/auth.json")?
                    .display()
                    .to_string(),
            ]),
            Self::Gemini => Ok(vec![
                crate::auth::gemini::tokens_path()?.display().to_string(),
                crate::auth::gemini::gemini_cli_oauth_path()?
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
                crate::storage::user_home_path(".config/github-copilot/hosts.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".config/github-copilot/apps.json")?
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
    success: bool,
}

impl AuthTestProviderReport {
    fn new(target: AuthTestTarget) -> Self {
        Self {
            provider: target.label().to_string(),
            credential_paths: target.credential_paths().unwrap_or_default(),
            steps: Vec::new(),
            smoke_output: None,
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

pub async fn run_auth_test_command(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    login: bool,
    all_configured: bool,
    no_smoke: bool,
    prompt: Option<&str>,
    emit_json: bool,
    output_path: Option<&str>,
) -> Result<()> {
    let targets = resolve_auth_test_targets(choice, all_configured)?;
    let smoke_prompt =
        prompt.unwrap_or("Reply with exactly AUTH_TEST_OK and nothing else. Do not call tools.");

    let mut reports = Vec::new();
    for target in targets {
        reports.push(run_auth_test_target(target, model, login, !no_smoke, smoke_prompt).await);
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
) -> Result<Vec<AuthTestTarget>> {
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

    AuthTestTarget::from_provider_choice(choice).map(|target| vec![target]).ok_or_else(|| {
        anyhow::anyhow!(
            "Provider '{}' is not yet supported by `jcode auth-test`. Supported: claude, openai, gemini, google, copilot, cursor.",
            choice.as_arg_value()
        )
    })
}

fn configured_auth_test_targets(status: &crate::auth::AuthStatus) -> Vec<AuthTestTarget> {
    let mut targets = Vec::new();
    if status.anthropic.state != crate::auth::AuthState::NotConfigured {
        targets.push(AuthTestTarget::Claude);
    }
    if status.openai != crate::auth::AuthState::NotConfigured {
        targets.push(AuthTestTarget::Openai);
    }
    if status.gemini != crate::auth::AuthState::NotConfigured {
        targets.push(AuthTestTarget::Gemini);
    }
    if status.google != crate::auth::AuthState::NotConfigured {
        targets.push(AuthTestTarget::Google);
    }
    if status.copilot != crate::auth::AuthState::NotConfigured {
        targets.push(AuthTestTarget::Copilot);
    }
    if status.cursor != crate::auth::AuthState::NotConfigured {
        targets.push(AuthTestTarget::Cursor);
    }
    targets
}

async fn run_auth_test_target(
    target: AuthTestTarget,
    model: Option<&str>,
    login: bool,
    run_smoke: bool,
    smoke_prompt: &str,
) -> AuthTestProviderReport {
    let mut report = AuthTestProviderReport::new(target);

    if login {
        match super::login::run_login(&target.provider_choice(), None).await {
            Ok(()) => report.push_step("login", true, "Login flow completed."),
            Err(err) => report.push_step("login", false, err.to_string()),
        }
    }

    match target {
        AuthTestTarget::Claude => probe_claude_auth(&mut report).await,
        AuthTestTarget::Openai => probe_openai_auth(&mut report).await,
        AuthTestTarget::Gemini => probe_gemini_auth(&mut report).await,
        AuthTestTarget::Google => probe_google_auth(&mut report).await,
        AuthTestTarget::Copilot => probe_copilot_auth(&mut report).await,
        AuthTestTarget::Cursor => probe_cursor_auth(&mut report).await,
    }

    if run_smoke && report.success && target.supports_smoke() {
        match run_provider_smoke(target, model, smoke_prompt).await {
            Ok(output) => {
                let ok = output.contains("AUTH_TEST_OK");
                report.smoke_output = Some(output.clone());
                report.push_step(
                    "provider_smoke",
                    ok,
                    if ok {
                        "Provider returned AUTH_TEST_OK.".to_string()
                    } else {
                        format!("Provider response did not contain AUTH_TEST_OK: {}", output)
                    },
                );
            }
            Err(err) => report.push_step("provider_smoke", false, err.to_string()),
        }
    } else if !target.supports_smoke() {
        report.push_step(
            "provider_smoke",
            true,
            "Skipped: provider is auth/tool-only and has no model runtime smoke step.",
        );
    } else if !run_smoke {
        report.push_step("provider_smoke", true, "Skipped by --no-smoke.");
    }

    report
}

async fn probe_claude_auth(report: &mut AuthTestProviderReport) {
    match crate::auth::claude::load_credentials() {
        Ok(creds) => {
            report.push_step(
                "credential_probe",
                true,
                format!(
                    "Loaded Claude credentials (expires_at={}).",
                    creds.expires_at
                ),
            );
            match crate::auth::oauth::refresh_claude_tokens(&creds.refresh_token).await {
                Ok(tokens) => report.push_step(
                    "refresh_probe",
                    true,
                    format!(
                        "Claude token refresh succeeded (new_expires_at={}).",
                        tokens.expires_at
                    ),
                ),
                Err(err) => report.push_step("refresh_probe", false, err.to_string()),
            }
        }
        Err(err) => report.push_step("credential_probe", false, err.to_string()),
    }
}

async fn probe_openai_auth(report: &mut AuthTestProviderReport) {
    match crate::auth::codex::load_credentials() {
        Ok(creds) => {
            let is_oauth = !creds.refresh_token.trim().is_empty();
            report.push_step(
                "credential_probe",
                true,
                if is_oauth {
                    format!(
                        "Loaded OpenAI OAuth credentials (expires_at={:?}).",
                        creds.expires_at
                    )
                } else {
                    "Loaded OpenAI API key credentials (no refresh token present).".to_string()
                },
            );
            if is_oauth {
                match crate::auth::oauth::refresh_openai_tokens(&creds.refresh_token).await {
                    Ok(tokens) => report.push_step(
                        "refresh_probe",
                        true,
                        format!(
                            "OpenAI token refresh succeeded (new_expires_at={}).",
                            tokens.expires_at
                        ),
                    ),
                    Err(err) => report.push_step("refresh_probe", false, err.to_string()),
                }
            } else {
                report.push_step(
                    "refresh_probe",
                    true,
                    "Skipped: OpenAI is using API key auth, not OAuth.".to_string(),
                );
            }
        }
        Err(err) => report.push_step("credential_probe", false, err.to_string()),
    }
}

async fn probe_gemini_auth(report: &mut AuthTestProviderReport) {
    match crate::auth::gemini::load_tokens() {
        Ok(tokens) => {
            report.push_step(
                "credential_probe",
                true,
                format!(
                    "Loaded Gemini tokens{} (expires_at={}).",
                    tokens
                        .email
                        .as_deref()
                        .map(|email| format!(" for {}", email))
                        .unwrap_or_default(),
                    tokens.expires_at
                ),
            );
            match crate::auth::gemini::load_or_refresh_tokens().await {
                Ok(tokens) => report.push_step(
                    "refresh_probe",
                    true,
                    format!(
                        "Gemini token load/refresh succeeded (expires_at={}).",
                        tokens.expires_at
                    ),
                ),
                Err(err) => report.push_step("refresh_probe", false, err.to_string()),
            }
        }
        Err(err) => report.push_step("credential_probe", false, err.to_string()),
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
                    tokens
                        .email
                        .as_deref()
                        .map(|email| format!(" for {}", email))
                        .unwrap_or_default()
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
    match crate::auth::copilot::load_github_token() {
        Ok(token) => {
            report.push_step(
                "credential_probe",
                true,
                format!(
                    "Loaded GitHub OAuth token for Copilot ({} chars).",
                    token.len()
                ),
            );
            let client = reqwest::Client::new();
            match crate::auth::copilot::exchange_github_token(&client, &token).await {
                Ok(api_token) => report.push_step(
                    "refresh_probe",
                    true,
                    format!(
                        "Exchanged GitHub token for Copilot API token (expires_at={}).",
                        api_token.expires_at
                    ),
                ),
                Err(err) => report.push_step("refresh_probe", false, err.to_string()),
            }
        }
        Err(err) => report.push_step("credential_probe", false, err.to_string()),
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

async fn run_provider_smoke(
    target: AuthTestTarget,
    model: Option<&str>,
    prompt: &str,
) -> Result<String> {
    let provider = super::provider_init::init_provider(&target.provider_choice(), model)
        .await
        .with_context(|| format!("Failed to initialize {} provider", target.label()))?;
    let output = provider
        .complete_simple(prompt, "")
        .await
        .with_context(|| format!("{} provider smoke prompt failed", target.label()))?;
    Ok(output.trim().to_string())
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
        println!("result: {}\n", if report.success { "PASS" } else { "FAIL" });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthState, AuthStatus, ProviderAuth};
    use crate::provider::ModelRoute;

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
                AuthTestTarget::Claude,
                AuthTestTarget::Gemini,
                AuthTestTarget::Google,
                AuthTestTarget::Copilot
            ]
        );
    }

    #[test]
    fn explicit_supported_provider_maps_to_single_auth_target() {
        let targets =
            resolve_auth_test_targets(&super::super::provider_init::ProviderChoice::Gemini, false)
                .expect("resolve target");
        assert_eq!(targets, vec![AuthTestTarget::Gemini]);
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
}
