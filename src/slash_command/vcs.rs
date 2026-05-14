use super::{register, SlashResult};

pub(crate) async fn register_commit() {
    register("commit", "Commit code with AI-generated message",
        "/commit [message]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let msg = args.trim();
                let cwd = match std::env::current_dir() { Ok(d) => d, Err(e) => { eprintln!("❌ {}", e); return; }};
                // Stage all
                let _ = tokio::process::Command::new("git").args(["add", "-A"]).current_dir(&cwd).output().await;
                if msg.is_empty() {
                    // Get diff for AI-generated message
                    let diff = tokio::process::Command::new("git").args(["diff", "--cached"]).current_dir(&cwd).output().await;
                    eprintln!("\n📝 AI Commit\n");
                    match diff {
                        Ok(o) if !o.stdout.is_empty() => {
                            let d = String::from_utf8_lossy(&o.stdout);
                            let files = d.lines().filter(|l| l.starts_with("+") && !l.starts_with("+++")).count();
                            let lines = d.lines().filter(|l| l.starts_with("-") && !l.starts_with("---")).count();
                            let auto_msg = format!("Update: {} files (+{}/-{})", d.len(), files, lines);
                            eprintln!("  Message: {}", auto_msg);
                            eprintln!("  (No message provided, use /commit <msg>)\n");
                            eprintln!("  git add -A && git commit -m \"{}\"\n", auto_msg);
                        }
                        _ => eprintln!("  No changes to commit.\n"),
                    }
                } else {
                    let r = tokio::process::Command::new("git").args(["commit", "-m", msg]).current_dir(&cwd).output().await;
                    match r {
                        Ok(o) if o.status.success() => eprintln!("✅ Committed: {}\n", msg),
                        Ok(o) => eprintln!("❌ {}\n", String::from_utf8_lossy(&o.stderr).trim()),
                        Err(e) => eprintln!("❌ Git error: {}\n", e),
                    }
                }
            });
            SlashResult::Ok("Commit command.".into())
        }),
    ).await;
}

pub(crate) async fn register_rethink() {
    register("rethink", "Re-analyze context and suggest improvements",
        "/rethink",
        std::sync::Arc::new(|_args: &str| {
            eprintln!("\n🔄 Rethinking context...\n  (Analyzing project structure...)\n");
            if let Ok(cwd) = std::env::current_dir() {
                let files = std::fs::read_dir(&cwd).ok().map(|e| e.filter_map(|e| e.ok()).count()).unwrap_or(0);
                eprintln!("  Directory: {} ({} items)", cwd.display(), files);
            }
            eprintln!("  (Full rethink requires Agent API.)\n");
            SlashResult::Ok("Rethink complete.".into())
        }),
    ).await;
}

pub(crate) async fn register_diff() {
    register("diff", "Show git diff for current changes",
        "/diff [--staged] [path]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let cwd = match std::env::current_dir() { Ok(d) => d, Err(e) => { eprintln!("❌ {}", e); return; }};
                let staged = args.contains("--staged");
                let path = args.replace("--staged", "").trim().to_string();
                let mut cmd = tokio::process::Command::new("git");
                cmd.current_dir(&cwd);
                if staged { cmd.arg("diff").arg("--cached"); }
                else { cmd.arg("diff").arg("HEAD"); }
                if !path.is_empty() { cmd.arg(&path); }
                match cmd.output().await {
                    Ok(o) if !o.stdout.is_empty() => {
                        let d = String::from_utf8_lossy(&o.stdout);
                        let lines: Vec<&str> = d.lines().collect();
                        for l in lines.iter().take(60) { eprintln!("{}", l); }
                        if lines.len() > 60 { eprintln!("... {} more lines", lines.len()-60); }
                    }
                    Ok(_) => eprintln!("  No changes found.\n"),
                    Err(e) => eprintln!("❌ Git error: {}\n", e),
                }
            });
            SlashResult::Ok("Showing diff...".into())
        }),
    ).await;
}

pub(crate) async fn register_status() {
    register("status", "Show current git status",
        "/status",
        std::sync::Arc::new(|_args: &str| {
            spawn_async(move || async move {
                let cwd = match std::env::current_dir() { Ok(d) => d, Err(e) => { eprintln!("❌ {}", e); return; }};
                match tokio::process::Command::new("git").args(["status", "--short"]).current_dir(&cwd).output().await {
                    Ok(o) => {
                        let out = String::from_utf8_lossy(&o.stdout);
                        eprintln!("\n📋 Git Status\n");
                        if out.trim().is_empty() { eprintln!("  Clean working tree.\n"); }
                        else { eprintln!("{}", out); }
                    }
                    Err(e) => eprintln!("❌ Git error: {}\n", e),
                }
            });
            SlashResult::Ok("Git status.".into())
        }),
    ).await;
}

pub(crate) async fn register_push() {
    register("push", "Push to remote git repository",
        "/push [remote] [branch]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let parts: Vec<&str> = args.trim().split_whitespace().collect();
                let remote = parts.first().copied().unwrap_or("origin");
                let branch = parts.get(1).copied().unwrap_or("main");
                let cwd = match std::env::current_dir() { Ok(d) => d, Err(e) => { eprintln!("❌ {}\n", e); return; }};
                eprintln!("\n📤 Pushing to {} {}\n", remote, branch);
                let r = tokio::process::Command::new("git")
                    .args(["push", remote, branch]).current_dir(&cwd).output().await;
                match r {
                    Ok(o) if o.status.success() => eprintln!("✅ Pushed to {}/{}\n", remote, branch),
                    Ok(o) => eprintln!("❌ {}\n", String::from_utf8_lossy(&o.stderr).trim()),
                    Err(e) => eprintln!("❌ Git error: {}\n", e),
                }
            });
            SlashResult::Ok("Pushing...".into())
        }),
    ).await;
}

pub(crate) async fn register_pull() {
    register("pull", "Pull from remote git repository",
        "/pull [remote] [branch]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let parts: Vec<&str> = args.trim().split_whitespace().collect();
                let remote = parts.first().copied().unwrap_or("origin");
                let branch = parts.get(1).copied().unwrap_or("main");
                let cwd = match std::env::current_dir() { Ok(d) => d, Err(e) => { eprintln!("❌ {}\n", e); return; }};
                eprintln!("\n📥 Pulling from {} {}\n", remote, branch);
                let r = tokio::process::Command::new("git")
                    .args(["pull", remote, branch]).current_dir(&cwd).output().await;
                match r {
                    Ok(o) if o.status.success() => eprintln!("✅ Pulled from {}/{}\n", remote, branch),
                    Ok(o) => eprintln!("❌ {}\n", String::from_utf8_lossy(&o.stderr).trim()),
                    Err(e) => eprintln!("❌ Git error: {}\n", e),
                }
            });
            SlashResult::Ok("Pulling...".into())
        }),
    ).await;
}

pub(crate) async fn register_branch() {
    register("branch", "Git branch operations",
        "/branch [list|create <n>|delete <n>]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let parts: Vec<&str> = args.trim().splitn(2, ' ').collect();
                let cwd = match std::env::current_dir() { Ok(d) => d, Err(e) => { eprintln!("❌ {}\n", e); return; }};
                match parts.first().copied().unwrap_or("") {
                    "list" | "ls" | "" => {
                        let r = tokio::process::Command::new("git").args(["branch"]).current_dir(&cwd).output().await;
                        eprintln!("\n📋 Git Branches\n");
                        if let Ok(o) = r { eprintln!("{}", String::from_utf8_lossy(&o.stdout)); }
                    }
                    "create" if parts.len() >= 2 => {
                        let r = tokio::process::Command::new("git")
                            .args(["checkout", "-b", parts[1]]).current_dir(&cwd).output().await;
                        match r { Ok(o) if o.status.success() => eprintln!("✅ Created branch: {}\n", parts[1]), _ => eprintln!("❌ Failed\n"), }
                    }
                    "delete" if parts.len() >= 2 => {
                        let r = tokio::process::Command::new("git")
                            .args(["branch", "-d", parts[1]]).current_dir(&cwd).output().await;
                        match r { Ok(o) if o.status.success() => eprintln!("✅ Deleted branch: {}\n", parts[1]), _ => eprintln!("❌ Failed\n"), }
                    }
                    _ => eprintln!("Usage: /branch [list|create <n>|delete <n>]\n"),
                }
            });
            SlashResult::Ok("Branch command.".into())
        }),
    ).await;
}

pub(crate) async fn register_merge() {
    register("merge", "Merge a branch into current",
        "/merge <branch>",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let branch = args.trim();
                if branch.is_empty() { eprintln!("Usage: /merge <branch>\n"); return; }
                let cwd = match std::env::current_dir() { Ok(d) => d, Err(e) => { eprintln!("❌ {}\n", e); return; }};
                eprintln!("\n🔀 Merging {} into current branch...\n", branch);
                let r = tokio::process::Command::new("git")
                    .args(["merge", branch]).current_dir(&cwd).output().await;
                match r {
                    Ok(o) if o.status.success() => eprintln!("✅ Merged {}\n", branch),
                    Ok(o) => eprintln!("❌ {}\n", String::from_utf8_lossy(&o.stderr).trim()),
                    Err(e) => eprintln!("❌ Git error: {}\n", e),
                }
            });
            SlashResult::Ok("Merging...".into())
        }),
    ).await;
}

pub(crate) async fn register_log() {
    register("log", "Show git commit log",
        "/log [n]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let n = args.trim().parse::<usize>().unwrap_or(10);
                let cwd = match std::env::current_dir() { Ok(d) => d, Err(e) => { eprintln!("❌ {}\n", e); return; }};
                match tokio::process::Command::new("git")
                    .args(["log", "--oneline", "-n", &n.to_string()]).current_dir(&cwd).output().await
                {
                    Ok(o) => { eprintln!("\n📋 Recent commits (last {})\n{}\n", n, String::from_utf8_lossy(&o.stdout)); }
                    Err(e) => eprintln!("❌ Git error: {}\n", e),
                }
            });
            SlashResult::Ok("Log.".into())
        }),
    ).await;
}

pub(crate) async fn register_redo() {
    register("redo", "Redo the last undone action",
        "/redo",
        std::sync::Arc::new(|_| { eprintln!("\n↪️  Redo\n  (Redo requires Agent session undo stack.)\n"); SlashResult::Ok("Redo.".into()) }),
    ).await;
}

fn spawn_async<F, Fut>(f: F) where F: FnOnce() -> Fut + Send + 'static, Fut: std::future::Future<Output = ()> + Send {
    if let Ok(h) = tokio::runtime::Handle::try_current() { h.spawn(f()); }
}
