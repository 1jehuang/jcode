use super::{register, SlashResult};

pub(crate) async fn register_export() {
    register("export", "Export a session to markdown file",
        "/export [session-id] [output-file]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let parts: Vec<&str> = args.trim().split_whitespace().collect();
                let sid = parts.first().map(|s| *s).unwrap_or("latest");
                let out = parts.get(1).map(|s| s.to_string()).unwrap_or_else(|| "session_export.md".to_string());
                match crate::replay::load_session(sid) {
                    Ok(s) => {
                        use std::io::Write;
                        let mut f = match std::fs::File::create(&out) {
                            Ok(f) => f, Err(e) => { eprintln!("❌ Cannot write {}: {}", out, e); return; }
                        };
                        let _ = writeln!(f, "# Session Export\n**ID:** {}\n**Messages:** {}\n", s.id, s.messages.len());
                        for msg in &s.messages {
                            let _ = writeln!(f, "## {} ({})", msg.role, msg.timestamp.format("%H:%M"));
                            let _ = writeln!(f, "ID: {}", msg.id);
                            let _ = writeln!(f);
                        }
                        eprintln!("✅ Exported {} messages to {}", s.messages.len(), out);
                    }
                    Err(e) => eprintln!("❌ Cannot load session '{}': {}", sid, e),
                }
            });
            SlashResult::Ok("Exporting...".into())
        }),
    ).await;
}

pub(crate) async fn register_resume() {
    register("resume", "List or resume a session",
        "/resume [session-id]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let trimmed = args.trim().to_string();
                if trimmed.is_empty() {
                    let jcode_dir = crate::storage::jcode_dir().ok();
                    let sessions_dir = jcode_dir.as_ref().map(|d| d.join("sessions"));
                    let entries = sessions_dir.iter().filter(|d| d.exists())
                        .flat_map(|d| std::fs::read_dir(d).ok())
                        .flat_map(|r| r.filter_map(|e| e.ok()))
                        .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
                        .collect::<Vec<_>>();
                    eprintln!("\n📋 Sessions\n");
                    if entries.is_empty() { eprintln!("  No sessions.\n"); return; }
                    for e in entries.iter().take(20).rev() {
                        let name = e.path().file_stem().map(|n| n.to_string_lossy()).unwrap_or_default();
                        if let Ok(s) = crate::session::Session::load_from_path(&e.path()) {
                            eprintln!("  [{:.8}] {} — {} msgs", name, s.display_title_or_name(), s.messages.len());
                        }
                    }
                    eprintln!("\n  Use /resume <id> to resume.\n");
                } else {
                    match crate::replay::load_session(&trimmed) {
                        Ok(s) => {
                            eprintln!("\n📋 Session: {}\n  ID: {}\n  Model: {}\n  Messages: {}\n",
                                s.display_title_or_name(), s.id,
                                s.model.as_deref().unwrap_or("default"), s.messages.len());
                            eprintln!("  Use: carpai --resume {}\n", s.id);
                        }
                        Err(e) => eprintln!("❌ {}", e),
                    }
                }
            });
            SlashResult::Ok("Listing sessions...".into())
        }),
    ).await;
}

pub(crate) async fn register_session() {
    register("session", "Show current session info",
        "/session",
        std::sync::Arc::new(|_args: &str| {
            eprintln!("\n📋 Current session:\n  (Use /resume to list sessions)\n  (Use /export to export)\n");
            SlashResult::Ok("Session info displayed.".into())
        }),
    ).await;
}

pub(crate) async fn register_fork() {
    register("fork", "Fork a session to create a branch",
        "/fork [session-id]",
        std::sync::Arc::new(|args: &str| {
            let sid = if args.trim().is_empty() { "current" } else { args.trim() };
            eprintln!("\n🔄 Forking session: {}\n  (Fork requires active session API.)\n", sid);
            SlashResult::Ok(format!("Forking: {}", sid))
        }),
    ).await;
}

fn spawn_async<F, Fut>(f: F) where F: FnOnce() -> Fut + Send + 'static, Fut: std::future::Future<Output = ()> + Send {
    if let Ok(h) = tokio::runtime::Handle::try_current() { h.spawn(f()); }
}
