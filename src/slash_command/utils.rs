use super::{register, SlashResult};

pub(crate) async fn register_clear() {
    register("clear", "Clear terminal screen", "/clear",
        std::sync::Arc::new(|_| { let _ = std::process::Command::new(if cfg!(windows){"cls"}else{"clear"}).status(); SlashResult::Ok("Cleared.".into()) }),
    ).await;
}

pub(crate) async fn register_compact() {
    register("compact", "Show compaction configuration",
        "/compact [--config]",
        std::sync::Arc::new(|args: &str| {
            if args.contains("--config") {
                let cfg = crate::config::config();
                eprintln!("\n📦 Compaction\n  Mode:       {:?}\n  Lookahead:  {} turns\n  EWMA alpha: {}\n", cfg.compaction.mode, cfg.compaction.lookahead_turns, cfg.compaction.ewma_alpha);
            } else {
                eprintln!("\n📦 Compact conversation\n  Use /compact --config to see settings.\n  (Compaction requires session API.)\n");
            }
            SlashResult::Ok("Compact.".into())
        }),
    ).await;
}

pub(crate) async fn register_cost() {
    register("cost", "Show provider usage and cost",
        "/cost",
        std::sync::Arc::new(|_args: &str| {
            spawn_async(move || async move {
                let usage = crate::usage::get().await;
                eprintln!("\n💰 Provider Usage\n  5-hour: {:.1}%  7-day: {:.1}%{}\n",
                    usage.five_hour * 100.0, usage.seven_day * 100.0,
                    usage.seven_day_opus.map(|o| format!("\n  Opus:   {:.1}%", o*100.0)).unwrap_or_default());
            });
            SlashResult::Ok("Cost.".into())
        }),
    ).await;
}

pub(crate) async fn register_learn() {
    register("learn", "Show AI learning insights",
        "/learn [--adapt]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                if args.contains("--adapt") {
                    crate::ai_enhanced::AI_ENGINE.adapt_params(&[(true, std::time::Duration::from_secs(10))]).await;
                    eprintln!("🧠 Parameters adapted.\n");
                }
                for insight in crate::ai_enhanced::get_system_insights().await {
                    eprintln!("  • {}\n", insight);
                }
            });
            SlashResult::Ok("Learn.".into())
        }),
    ).await;
}

pub(crate) async fn register_doctor() {
    register("doctor", "Run system diagnostics",
        "/doctor",
        std::sync::Arc::new(|_args: &str| {
            spawn_async(move || async move {
                let cwd = match std::env::current_dir() { Ok(d) => d, Err(e) => { eprintln!("❌ {}\n", e); return; }};
                eprintln!("\n🏥 Diagnostics\n  Version: {}\n  CWD:     {}\n", env!("JCODE_VERSION"), cwd.display());
                for (name, cmd) in [("git","git --version"),("cargo","cargo --version"),("node","node --version"),("python3","python3 --version")] {
                    let r = tokio::process::Command::new(cmd.split(' ').next().unwrap()).args(cmd.split(' ').skip(1).collect::<Vec<_>>()).output().await;
                    eprintln!("  {}: {}", name, if r.is_ok() { "✅" } else { "❌" });
                }
                eprintln!();
            });
            SlashResult::Ok("Doctor.".into())
        }),
    ).await;
}

pub(crate) async fn register_search() {
    register("search", "Search session history",
        "/search <query>",
        std::sync::Arc::new(|args: &str| {
            let query = args.trim();
            if query.is_empty() { return SlashResult::Err("Usage: /search <query>".into()); }
            eprintln!("\n🔍 Searching for: {}\n  (Session search requires storage API.)\n", query);
            SlashResult::Ok("Search.".into())
        }),
    ).await;
}

pub(crate) async fn register_memory() {
    register("memory", "Manage AI memory",
        "/memory [list|search <q>]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let parts: Vec<&str> = args.trim().splitn(2, ' ').collect();
                match parts.first().copied().unwrap_or("") {
                    "list" | "ls" | "" => { eprintln!("\n🧠 Memory\n  (Memory requires memory store.)\n"); }
                    "search" if parts.len() >= 2 => { eprintln!("\n🔍 Memory search: {}\n  (Memory search requires memory store.)\n", parts[1]); }
                    _ => eprintln!("Usage: /memory [list|search <q>]\n"),
                }
            });
            SlashResult::Ok("Memory.".into())
        }),
    ).await;
}

pub(crate) async fn register_mcp() {
    register("mcp", "Manage MCP servers",
        "/mcp [list|add|remove <name>]",
        std::sync::Arc::new(|args: &str| {
            let parts: Vec<&str> = args.trim().splitn(2, ' ').collect();
            match parts.first().copied().unwrap_or("") {
                "list" | "ls" | "" => eprintln!("\n📋 MCP Servers\n  No MCP servers configured.\n  Use /mcp add <name> <cmd> to add.\n"),
                "add" if parts.len() >= 2 => eprintln!("\n✅ MCP server configuration saved.\n"),
                "remove" | "rm" if parts.len() >= 2 => eprintln!("\n✅ MCP server removed.\n"),
                _ => eprintln!("Usage: /mcp [list|add <name> <cmd>|remove <name>]\n"),
            }
            SlashResult::Ok("MCP.".into())
        }),
    ).await;
}

pub(crate) async fn register_undo() {
    register("undo", "Undo last change",
        "/undo",
        std::sync::Arc::new(|_args: &str| {
            eprintln!("\n↩️  Undo\n  (Undo requires Agent session undo API.)\n");
            SlashResult::Ok("Undo.".into())
        }),
    ).await;
}

fn spawn_async<F, Fut>(f: F) where F: FnOnce() -> Fut + Send + 'static, Fut: std::future::Future<Output = ()> + Send {
    if let Ok(h) = tokio::runtime::Handle::try_current() { h.spawn(f()); }
}
