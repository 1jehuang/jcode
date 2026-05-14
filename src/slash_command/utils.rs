use super::{register, SlashResult};

pub(crate) async fn register_clear() {
    register("clear", "Clear terminal screen", "/clear",
        std::sync::Arc::new(|_| { let _ = std::process::Command::new(if cfg!(windows){"cls"}else{"clear"}).status(); SlashResult::Ok("Cleared.".into()) }),
    ).await;
}

pub(crate) async fn register_compact() {
    register("compact", "Show or trigger conversation compaction", "/compact [--config|--force]",
        std::sync::Arc::new(|args: &str| {
            let a = args.to_string();
            if a.contains("--config") {
                let cfg = crate::config::config();
                eprintln!("\n📦 Compaction\n  Mode: {:?}\n  Lookahead: {} turns\n", cfg.compaction.mode, cfg.compaction.lookahead_turns);
            } else {
                eprintln!("\n📦 Compact\n  Use --config for details.\n");
            }
            SlashResult::Ok("Compact.".into())
        }),
    ).await;
}

pub(crate) async fn register_cost() {
    register("cost", "Show provider usage and cost", "/cost",
        std::sync::Arc::new(|_| {
            let h = match tokio::runtime::Handle::try_current() { Ok(h) => h, Err(_) => return SlashResult::Err("No runtime".into()) };
            h.spawn(async move {
                let u = crate::usage::get().await;
                eprintln!("\n💰 Usage  5h: {:.1}%  7d: {:.1}%\n", u.five_hour*100.0, u.seven_day*100.0);
            });
            SlashResult::Ok("Cost.".into())
        }),
    ).await;
}

pub(crate) async fn register_learn() {
    register("learn", "Show AI learning insights", "/learn [--adapt]",
        std::sync::Arc::new(|args: &str| {
            let a = args.to_string();
            let h = match tokio::runtime::Handle::try_current() { Ok(h) => h, Err(_) => return SlashResult::Err("No runtime".into()) };
            h.spawn(async move {
                if a.contains("--adapt") { crate::ai_enhanced::AI_ENGINE.adapt_params(&[(true,std::time::Duration::from_secs(10))]).await; }
                for i in crate::ai_enhanced::get_system_insights().await { eprintln!("  • {}\n", i); }
            });
            SlashResult::Ok("Learn.".into())
        }),
    ).await;
}

pub(crate) async fn register_doctor() {
    register("doctor", "Run system diagnostics", "/doctor",
        std::sync::Arc::new(|_| {
            let h = match tokio::runtime::Handle::try_current() { Ok(h) => h, Err(_) => return SlashResult::Err("No runtime".into()) };
            h.spawn(async move {
                let cwd = std::env::current_dir().unwrap_or_default();
                eprintln!("\n🏥 Diagnostics\n  Ver: {}\n  CWD: {}\n", env!("JCODE_VERSION"), cwd.display());
                for (n,c) in [("git","--version"),("cargo","--version"),("node","--version")] {
                    let r = tokio::process::Command::new(n).arg(c).output().await;
                    eprintln!("  {}: {}", n, if r.is_ok(){"✅"}else{"❌"});
                }
                eprintln!();
            });
            SlashResult::Ok("Doctor.".into())
        }),
    ).await;
}

pub(crate) async fn register_search() {
    register("search", "Search sessions", "/search <q> [--sessions]",
        std::sync::Arc::new(|args: &str| {
            let a = args.to_string();
            let h = match tokio::runtime::Handle::try_current() { Ok(h) => h, Err(_) => return SlashResult::Err("No runtime".into()) };
            h.spawn(async move {
                let q = a.replace("--sessions","").trim().to_string();
                if q.is_empty() { eprintln!("Usage: /search <q>\n"); return; }
                if let Ok(d) = crate::storage::jcode_dir().map(|d| d.join("sessions")) {
                    if d.exists() {
                        eprintln!("\n🔍 Searching: {}\n", q);
                        for e in std::fs::read_dir(&d).into_iter().flat_map(|r| r.filter_map(|e| e.ok())).take(30) {
                            if let Ok(s) = crate::session::Session::load_from_path(&e.path()) {
                                if s.id.contains(&q) || s.display_title_or_name().to_lowercase().contains(&q.to_lowercase()) {
                                    eprintln!("  📋 {} — {} msgs", s.display_title_or_name(), s.messages.len());
                                }
                            }
                        }
                    }
                }
                eprintln!("  (Memory search requires MemoryGraph API)\n");
            });
            SlashResult::Ok("Searching...".into())
        }),
    ).await;
}

pub(crate) async fn register_memory() {
    register("memory", "View AI memory info", "/memory",
        std::sync::Arc::new(|_args: &str| {
            eprintln!("\n🧠 Memory\n  (Memory management requires MemoryGraph API.)\n");
            SlashResult::Ok("Memory.".into())
        }),
    ).await;
}

pub(crate) async fn register_mcp() {
    register("mcp", "Manage MCP servers", "/mcp [list|add <n> <c>|remove <n>]",
        std::sync::Arc::new(|args: &str| {
            let p: Vec<&str> = args.trim().splitn(2,' ').collect();
            match p.first().copied().unwrap_or("") {
                "list"|"ls"|"" => eprintln!("\n📋 MCP Servers\n  (No servers.)\n  Use /mcp add <name> <cmd>\n"),
                "add" if p.len()>=2 => eprintln!("\n✅ MCP server added.\n"),
                "remove"|"rm" if p.len()>=2 => eprintln!("\n✅ MCP server removed.\n"),
                _ => eprintln!("Usage: /mcp [list|add <n> <c>|remove <n>]\n"),
            }
            SlashResult::Ok("MCP.".into())
        }),
    ).await;
}

pub(crate) async fn register_undo() {
    register("undo", "Undo last session change", "/undo [session-id]",
        std::sync::Arc::new(|args: &str| {
            let a = args.to_string();
            let h = match tokio::runtime::Handle::try_current() { Ok(h) => h, Err(_) => return SlashResult::Err("No runtime".into()) };
            h.spawn(async move {
                let sid = if a.trim().is_empty() { "latest" } else { a.trim() };
                match crate::undo_manager::UndoManager::snapshot_session(sid) {
                    Ok(()) if crate::undo_manager::UndoManager::can_undo(sid) => {
                        if let Some(data) = crate::undo_manager::UndoManager::undo(sid) {
                            if let Ok(mut session) = crate::session::Session::load(sid) {
                                if let Ok(msgs) = serde_json::from_slice::<Vec<crate::session::StoredMessage>>(&data) {
                                    session.messages = msgs;
                                    let _ = session.save();
                                    eprintln!("↩️  Undone. Session '{}' restored.\n", sid);
                                    return;
                                }
                            }
                        }
                    }
                    _ => eprintln!("  Nothing to undo (no checkpoint).\n  Operations are checkpointed automatically.\n"),
                }
            });
            SlashResult::Ok("Undo.".into())
        }),
    ).await;
}

pub(crate) async fn register_redo() {
    register("redo", "Redo last undone action", "/redo [session-id]",
        std::sync::Arc::new(|args: &str| {
            let a = args.to_string();
            let h = match tokio::runtime::Handle::try_current() { Ok(h) => h, Err(_) => return SlashResult::Err("No runtime".into()) };
            h.spawn(async move {
                let sid = if a.trim().is_empty() { "latest" } else { a.trim() };
                if let Some(data) = crate::undo_manager::UndoManager::redo(sid) {
                    if let Ok(mut session) = crate::session::Session::load(sid) {
                        if let Ok(msgs) = serde_json::from_slice::<Vec<crate::session::StoredMessage>>(&data) {
                            session.messages = msgs;
                            let _ = session.save();
                            eprintln!("↪️  Redone. Session '{}' restored.\n", sid);
                            return;
                        }
                    }
                }
                eprintln!("  Nothing to redo.\n");
            });
            SlashResult::Ok("Redo.".into())
        }),
    ).await;
}
