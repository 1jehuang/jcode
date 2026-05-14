use super::{register, SlashResult};
use crate::build::{ProjectType, BuildRequest};
use crate::build_module::{BuildExecutor};
use crate::workspace_manager::{WorkspaceManager, Project};
use std::sync::Arc;

pub(crate) async fn register_build() {
    register("build", "Build the current project",
        "/build [--release] [--clean] [--test] [message...]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let release = args.contains("--release");
                let clean = args.contains("--clean");
                let run_tests = args.contains("--test");
                let parallel = args.contains("--parallel");
                let all_proj = args.contains("--all") || args.contains("--workspace");
                let msg = args.replace("--release","").replace("--clean","")
                    .replace("--test","").replace("--parallel","")
                    .replace("--all","").replace("--workspace","").trim().to_string();
                let msg = if msg.is_empty() { "Build project".to_string() } else { msg };
                let _ = crate::cli::commands::run_build_command(
                    &msg, false, false, 3,
                    release, clean, None, all_proj, run_tests, parallel, None,
                ).await;
            });
            SlashResult::Ok("Starting build...".into())
        }),
    ).await;
}

pub(crate) async fn register_plan() {
    register("plan", "Generate a project analysis/plan",
        "/plan [goal...]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let goal = if args.trim().is_empty() { "Analyze project" } else { args.trim() };
                let cwd = match std::env::current_dir() { Ok(d) => d, Err(e) => { eprintln!("❌ {}", e); return; }};
                let pt = ProjectType::detect_from_path(&cwd);
                eprintln!("\n📋 Plan: {}\n", goal);
                eprintln!("  ┌─ Plan ──────────────────────────────");
                eprintln!("  │ Project:    {}", pt);
                eprintln!("  │ Dir:        {}", cwd.display());
                eprintln!("  │ Build cmd:  {}", pt.default_build_command());
                eprintln!("  │ Test cmd:   {}", pt.default_test_command());
                eprintln!("  └─────────────────────────────────────\n");
                // List key files
                if let Ok(entries) = std::fs::read_dir(&cwd) {
                    let files: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string())).collect();
                    eprintln!("  Key files:");
                    for f in files.iter().take(15) { eprintln!("    📄 {}", f); }
                    if files.len() > 15 { eprintln!("    ... {} more", files.len()-15); }
                }
                eprintln!("\n  Use `/build {}` to execute.\n", goal);
            });
            SlashResult::Ok("Generating plan...".into())
        }),
    ).await;
}

pub(crate) async fn register_review() {
    register("review", "Run code review on current changes",
        "/review [--staged] [--all]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let cwd = match std::env::current_dir() { Ok(d) => d, Err(e) => { eprintln!("❌ {}", e); return; }};
                let diff = tokio::process::Command::new("git")
                    .args(["diff", "HEAD"]).current_dir(&cwd).output().await;
                eprintln!("\n🔍 Code Review\n");
                match diff {
                    Ok(o) if !o.stdout.is_empty() => {
                        let d = String::from_utf8_lossy(&o.stdout);
                        let lines: Vec<&str> = d.lines().collect();
                        let added = lines.iter().filter(|l| l.starts_with('+') && !l.starts_with("+++")).count();
                        let removed = lines.iter().filter(|l| l.starts_with('-') && !l.starts_with("---")).count();
                        let files = lines.iter().filter(|l| l.starts_with("diff --git")).count();
                        eprintln!("  Files: {files}  +{added}/-{removed}  ({})", d.len());
                        // Run micro-ci
                        let ci = jcode_micro_ci::MicroCi::new(jcode_micro_ci::CiConfig {
                            workspace_root: cwd.to_string_lossy().to_string(), ..Default::default()
                        });
                        let r = ci.run().await;
                        if !r.issues.is_empty() {
                            eprintln!("\n  Issues found: {}", r.issues.len());
                            for issue in r.issues.iter().take(10) {
                                eprintln!("    [{}] {}:{}", issue.severity, issue.location, issue.line);
                            }
                        } else { eprintln!("\n  ✅ No issues."); }
                    }
                    _ => eprintln!("  No uncommitted changes.\n  Use /review --all to scan all files.\n"),
                }
            });
            SlashResult::Ok("Starting review...".into())
        }),
    ).await;
}

fn spawn_async<F, Fut>(f: F) where F: FnOnce() -> Fut + Send + 'static, Fut: std::future::Future<Output = ()> + Send {
    if let Ok(h) = tokio::runtime::Handle::try_current() { h.spawn(f()); }
}
