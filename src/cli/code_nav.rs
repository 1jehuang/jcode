//! Code Navigation Commands — LSP-based go-to-def, find-refs, hover
//!
//! Extracted from commands.rs for better modularity.

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
        for child in sym.children.iter().flatten() {
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
    match *kind {
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
                Box::pin(async move {
                    lsp_goto_def(client, &file, l, c).await
                })
            }).await?;

            eprintln!("\n🔍 Go to Definition\n");
            println!("{}", format_locations(&results));
        }
        CodeNavCommand::FindRefs { location } => {
            let (file, line, col) = parse_location(&location)?;
            let l = line.saturating_sub(1);
            let c = col.saturating_sub(1);
            let results = with_lsp_client(&file, move |client| {
                Box::pin(async move {
                    lsp_find_refs(client, &file, l, c).await
                })
            }).await?;

            eprintln!("\n🔎 Find References ({})\n", results.len());
            println!("{}", format_locations(&results));
        }
        CodeNavCommand::Hover { location } => {
            let (file, line, col) = parse_location(&location)?;
            let l = line.saturating_sub(1);
            let c = col.saturating_sub(1);
            let results = with_lsp_client(&file, move |client| {
                Box::pin(async move {
                    lsp_hover(client, &file, l, c).await
                })
            }).await?;

            eprintln!("\n💡 Hover Info\n");
            println!("{}", format_hover(&results));
        }
        CodeNavCommand::Symbols { file } => {
            let results = with_lsp_client(&file, move |client| {
                Box::pin(async move {
                    lsp_doc_symbols(client, &file).await
                })
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
                Box::pin(async move {
                    lsp_goto_def(client, &file, l, c).await
                })
            }).await?;

            eprintln!("\n🔌 Go to Implementation\n");
            println!("{}", format_locations(&results));
        }
        CodeNavCommand::CallHierarchy { location } => {
            let (file, line, col) = parse_location(&location)?;
            let l = line.saturating_sub(1);
            let c = col.saturating_sub(1);
            let results = with_lsp_client(&file, move |client| {
                Box::pin(async move {
                    lsp_find_refs(client, &file, l, c).await
                })
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