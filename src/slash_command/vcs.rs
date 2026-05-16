use super::{register, SlashResult};

fn s<F, Fut>(f: F) where F: FnOnce() -> Fut + Send + 'static, Fut: std::future::Future<Output = ()> + Send + 'static {
    if let Ok(h) = tokio::runtime::Handle::try_current() { h.spawn(f()); }
}

pub(crate) async fn register_commit() {
    register("commit", "Commit code", "/commit [msg]",
        std::sync::Arc::new(|args: &str| { let a=args.to_string(); s(move||async move{
            let cwd = std::env::current_dir().unwrap_or_default();
            let _ = tokio::process::Command::new("git").args(["add","-A"]).current_dir(&cwd).output().await;
            let msg = a.trim();
            if msg.is_empty() {
                if let Ok(o) = tokio::process::Command::new("git").args(["diff","--cached"]).current_dir(&cwd).output().await {
                    let d = String::from_utf8_lossy(&o.stdout);
                    let fc = d.lines().filter(|l|l.starts_with("diff --git")).count();
                    let ad = d.lines().filter(|l|l.starts_with('+')&&!l.starts_with("+++")).count();
                    let rm = d.lines().filter(|l|l.starts_with('-')&&!l.starts_with("---")).count();
                    if fc>0 {
                        let m = format!("Update {} files (+{}/-{})",fc,ad,rm);
                        let r = tokio::process::Command::new("git").args(["commit","-m",&m]).current_dir(&cwd).output().await;
                        if r.map(|o|o.status.success()).unwrap_or(false) { eprintln!("✅ {}\n",m); } else { eprintln!("❌\n"); }
                    } else { eprintln!("  No changes.\n"); }
                }
            } else {
                let r = tokio::process::Command::new("git").args(["commit","-m",msg]).current_dir(&cwd).output().await;
                if r.map(|o|o.status.success()).unwrap_or(false) { eprintln!("✅ Committed\n"); } else { eprintln!("❌\n"); }
            }
        }); SlashResult::Ok("Commit.".into()) }),
    ).await;
}

pub(crate) async fn register_rethink() {
    register("rethink", "Re-analyze context", "/rethink",
        std::sync::Arc::new(|_| { s(move||async move{
            let cwd = std::env::current_dir().unwrap_or_default();
            let total = std::fs::read_dir(&cwd).map(|e|e.filter_map(|e|e.ok()).count()).unwrap_or(0);
            let rs = std::fs::read_dir(&cwd).map(|e|e.filter_map(|e|e.ok()).filter(|e|e.path().extension().map(|x|x=="rs").unwrap_or(false)).count()).unwrap_or(0);
            eprintln!("\n🔄 Rethink\n  Dir: {} ({} items, {} .rs)", cwd.display(), total, rs);
            if rs > 0 { eprintln!("  -> /build to compile"); }
        }); SlashResult::Ok("Rethink.".into()) }),
    ).await;
}

pub(crate) async fn register_diff() {
    register("diff", "Show git diff", "/diff [--staged]",
        std::sync::Arc::new(|args: &str| { let a=args.to_string(); s(move||async move{
            let cwd = std::env::current_dir().unwrap_or_default();
            let git = crate::git::operations::GitOperations::new(cwd);
            let d = git.format_diff(a.contains("--staged"));
            if d.is_empty() { eprintln!("  No changes.\n"); return; }
            for l in d.lines().take(60) { eprintln!("{}",l); }
            if d.lines().count()>60 { eprintln!("... {} more\n",d.lines().count()-60); }
        }); SlashResult::Ok("Diff.".into()) }),
    ).await;
}

pub(crate) async fn register_status() {
    register("status", "Show git status", "/status",
        std::sync::Arc::new(|_| { s(move||async move{
            let cwd = std::env::current_dir().unwrap_or_default();
            let o = tokio::process::Command::new("git").args(["status","--short"]).current_dir(&cwd).output().await;
            if let Ok(o) = o { let s = String::from_utf8_lossy(&o.stdout); eprintln!("\n📋 Status\n{}\n", if s.trim().is_empty(){ "  Clean.\n".to_string() }else{ s.to_string() }); }
        }); SlashResult::Ok("Status.".into()) }),
    ).await;
}

pub(crate) async fn register_push() {
    register("push", "Push to remote", "/push [remote] [branch]",
        std::sync::Arc::new(|args: &str| { let a=args.to_string(); s(move||async move{
            let p: Vec<&str> = a.split_whitespace().collect();
            let r = p.first().copied().unwrap_or("origin"); let b = p.get(1).copied().unwrap_or("main");
            let cwd = std::env::current_dir().unwrap_or_default();
            eprintln!("\n📤 Pushing {} {}\n",r,b);
            let o = tokio::process::Command::new("git").args(["push",r,b]).current_dir(&cwd).output().await;
            if o.map(|o|o.status.success()).unwrap_or(false) { eprintln!("✅\n"); } else { eprintln!("❌\n"); }
        }); SlashResult::Ok("Push.".into()) }),
    ).await;
}

pub(crate) async fn register_pull() {
    register("pull", "Pull from remote", "/pull [remote] [branch]",
        std::sync::Arc::new(|args: &str| { let a=args.to_string(); s(move||async move{
            let p: Vec<&str> = a.split_whitespace().collect();
            let r = p.first().copied().unwrap_or("origin"); let b = p.get(1).copied().unwrap_or("main");
            let cwd = std::env::current_dir().unwrap_or_default();
            eprintln!("\n📥 Pulling {} {}\n",r,b);
            let o = tokio::process::Command::new("git").args(["pull",r,b]).current_dir(&cwd).output().await;
            if o.map(|o|o.status.success()).unwrap_or(false) { eprintln!("✅\n"); } else { eprintln!("❌\n"); }
        }); SlashResult::Ok("Pull.".into()) }),
    ).await;
}

pub(crate) async fn register_branch() {
    register("branch", "Git branch ops", "/branch [list|create <n>|delete <n>]",
        std::sync::Arc::new(|args: &str| { let a=args.to_string(); s(move||async move{
            let p: Vec<&str> = a.trim().splitn(2,' ').collect();
            let cwd = std::env::current_dir().unwrap_or_default();
            let git = crate::git::operations::GitOperations::new(cwd);
            match p.first().copied().unwrap_or("") {
                "list"|"ls"|"" => {
                        let branches = git.list_branches();
                        eprintln!("\n📋 Branches\n");
                        for b in &branches { eprintln!("  {} {}", if b.current{"*"}else{" "}, b.name); }
                    eprintln!();
                }
                "create" if p.len()>=2 => { let r = git.create_branch(p[1]); eprintln!("{}\n", r.unwrap_or_else(|e|format!("❌ {}",e))); }
                "delete" if p.len()>=2 => { let r = git.delete_branch(p[1],false); eprintln!("{}\n", r.unwrap_or_else(|e|format!("❌ {}",e))); }
                _ => eprintln!("Usage: /branch [list|create|delete]\n"),
            }
        }); SlashResult::Ok("Branch.".into()) }),
    ).await;
}

pub(crate) async fn register_merge() {
    register("merge", "Merge branch", "/merge <branch>",
        std::sync::Arc::new(|args: &str| { let a=args.to_string(); s(move||async move{
            if a.trim().is_empty() { eprintln!("Usage: /merge <branch>\n"); return; }
            let cwd = std::env::current_dir().unwrap_or_default();
            let o = tokio::process::Command::new("git").args(["merge",a.trim()]).current_dir(&cwd).output().await;
            if o.map(|o|o.status.success()).unwrap_or(false) { eprintln!("✅\n"); } else { eprintln!("❌\n"); }
        }); SlashResult::Ok("Merge.".into()) }),
    ).await;
}

pub(crate) async fn register_log() {
    register("log", "Show commit log", "/log [n]",
        std::sync::Arc::new(|args: &str| { let a=args.to_string(); s(move||async move{
            let cwd = std::env::current_dir().unwrap_or_default();
            let n = a.trim().parse::<usize>().unwrap_or(10);
            let git = crate::git::operations::GitOperations::new(cwd);
            for c in git.recent_commits(n) { eprintln!("  {}",c); }
            eprintln!();
        }); SlashResult::Ok("Log.".into()) }),
    ).await;
}

pub(crate) async fn register_undo_and_redo() {
    // UNDO and REDO are now registered in utils.rs to share the UndoManager dependency
}
