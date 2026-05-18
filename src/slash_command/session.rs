use super::{register, SlashResult};

pub(crate) async fn register_export() {
    register("export", "Export session to markdown", "/export [session-id] [file]",
        std::sync::Arc::new(|args: &str| {
            let a = args.to_string();
            spawn(move || async move {
                let parts: Vec<&str> = a.split_whitespace().collect();
                let sid = parts.first().copied().unwrap_or("latest");
                let out = parts.get(1).map(|s| s.to_string()).unwrap_or_else(|| "session_export.md".into());
                match crate::replay::load_session(sid) {
                    Ok(s) => {
                        use std::io::Write;
                        let mut f = match std::fs::File::create(&out) { Ok(f)=>f, Err(e)=>{eprintln!("❌ {}",e); return; }};
                        let _ = writeln!(f, "# Session Export\n**ID:** {}\n**Messages:** {}\n", s.id, s.messages.len());
                        for msg in &s.messages {
                            let ts = msg.timestamp.as_ref().map(|t| t.to_string()).unwrap_or_else(|| "N/A".to_string());
                            let _ = writeln!(f, "## {:?} ({})", msg.role, ts);
                            let _ = writeln!(f, "ID: {}", msg.id);
                            let _ = writeln!(f);
                        }
                        eprintln!("✅ Exported {} messages to {}", s.messages.len(), out);
                    }
                    Err(e) => eprintln!("❌ {}", e),
                }
            });
            SlashResult::Ok("Exporting...".into())
        }),
    ).await;
}

pub(crate) async fn register_resume() {
    register("resume", "List or resume sessions", "/resume [session-id]",
        std::sync::Arc::new(|args: &str| {
            let a = args.to_string();
            spawn(move || async move {
                let trimmed = a.trim().to_string();
                if trimmed.is_empty() {
                    if let Ok(jd) = crate::storage::jcode_dir().map(|d| d.join("sessions")) {
                        let e: Vec<_> = std::fs::read_dir(&jd).into_iter().flat_map(|r| r.filter_map(|e| e.ok())).filter(|e| e.path().extension().map(|x|x=="json").unwrap_or(false)).collect();
                        if e.is_empty() { eprintln!("  No sessions.\n"); return; }
                        eprintln!("\n📋 Sessions\n");
                        for entry in e.iter().rev().take(20) {
                            let name = entry.path().file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            if let Ok(s) = crate::session::Session::load_from_path(&entry.path()) {
                                eprintln!("  [{:.8}] {} — {} msgs", name, s.display_title_or_name(), s.messages.len());
                            }
                        }
                        eprintln!("\n  /resume <id>\n");
                    }
                } else {
                    match crate::replay::load_session(&trimmed) {
                        Ok(s) => eprintln!("\n📋 {}\n  ID: {}\n  Model: {}\n  Messages: {}\n  carpai --resume {}\n", s.display_title_or_name(), s.id, s.model.as_deref().unwrap_or("default"), s.messages.len(), s.id),
                        Err(e) => eprintln!("❌ {}\n", e),
                    }
                }
            });
            SlashResult::Ok("Resume.".into())
        }),
    ).await;
}

pub(crate) async fn register_session() {
    register("session", "Show current session info", "/session",
        std::sync::Arc::new(|_| { eprintln!("\n📋 Session\n  Use /resume to list, /export to export.\n"); SlashResult::Ok("Info.".into()) }),
    ).await;
}

pub(crate) async fn register_fork() {
    register("fork", "Fork/clone a session", "/fork [session-id]",
        std::sync::Arc::new(|args: &str| {
            let a = args.to_string();
            spawn(move || async move {
                let sid = if a.trim().is_empty() { "latest" } else { a.trim() };
                match crate::replay::load_session(sid) {
                    Ok(src) => {
                        let mut child = crate::session::Session::create(Some(src.id.clone()), Some(format!("Fork of {}", src.display_title_or_name())));
                        child.messages = src.messages.clone();
                        child.compaction = src.compaction.clone();
                        child.model = src.model.clone();
                        child.provider_key = src.provider_key.clone();
                        match child.save() {
                            Ok(_) => eprintln!("\n🔄 Forked: {}\n  New ID: {}\n  carpai --resume {}\n", child.display_title_or_name(), child.id, child.id),
                            Err(e) => eprintln!("❌ {}\n", e),
                        }
                    }
                    Err(e) => eprintln!("❌ {}\n", e),
                }
            });
            SlashResult::Ok("Forking...".into())
        }),
    ).await;
}

fn spawn<F, Fut>(f: F) where F: FnOnce() -> Fut + Send + 'static, Fut: std::future::Future<Output = ()> + Send + 'static {
    if let Ok(h) = tokio::runtime::Handle::try_current() { h.spawn(f()); }
}
