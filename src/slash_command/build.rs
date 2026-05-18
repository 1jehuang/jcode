use super::{register, SlashResult};
use crate::build::ProjectType;
use crate::cli::build_cmd::BuildOptions;

fn s<F, Fut>(f: F) where F: FnOnce() -> Fut + Send + 'static, Fut: std::future::Future<Output = ()> + Send + 'static {
    if let Ok(h) = tokio::runtime::Handle::try_current() { h.spawn(f()); }
}

pub(crate) async fn register_build() {
    register("build", "Build current project", "/build [--release] [--clean] [--test]",
        std::sync::Arc::new(|args: &str| {
            let a = args.to_string();
            s(move || async move {
                let release = a.contains("--release");
                let clean = a.contains("--clean");
                let test = a.contains("--test");
                let msg = a.replace("--release","").replace("--clean","").replace("--test","").trim().to_string();
                let msg = if msg.is_empty() { None } else { Some(msg) };
                let _ = crate::cli::build_cmd::run_build_command(BuildOptions {
                    message: msg,
                    manual: false,
                    no_verify: false,
                    max_retries: 3,
                    release,
                    clean,
                    target: None,
                    all_projects: false,
                    test,
                    parallel: false,
                    jobs: None,
                }).await;
            });
            SlashResult::Ok("Starting build...".into())
        }),
    ).await;
}

pub(crate) async fn register_plan() {
    register("plan", "Analyze project and plan", "/plan [goal...]",
        std::sync::Arc::new(|args: &str| {
            let a = args.to_string();
            s(move || async move {
                let goal = if a.trim().is_empty() { "Analyze project" } else { a.trim() };
                let cwd = std::env::current_dir().unwrap_or_default();
                let pt = ProjectType::detect_from_path(&cwd);
                eprintln!("\n📋 Plan: {}\n  Project: {:?}\n  Dir:     {}\n  Build:   {}\n  Test:    {}\n", goal, pt, cwd.display(), pt.default_build_command(), pt.default_test_command());
                if let Ok(e) = std::fs::read_dir(&cwd) {
                    let files: Vec<_> = e.filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string())).collect();
                    for f in files.iter().take(15) { eprintln!("  📄 {}", f); }
                    if files.len() > 15 { eprintln!("  ... {} more", files.len()-15); }
                }
                eprintln!("\n  /build {} to execute.\n", goal);
            });
            SlashResult::Ok("Plan.".into())
        }),
    ).await;
}

pub(crate) async fn register_review() {
    register("review", "Code review via git diff + CI", "/review",
        std::sync::Arc::new(|_| {
            s(move || async move {
                let cwd = std::env::current_dir().unwrap_or_default();
                match tokio::process::Command::new("git").args(["diff","HEAD"]).current_dir(&cwd).output().await {
                    Ok(o) if !o.stdout.is_empty() => {
                        let d = String::from_utf8_lossy(&o.stdout);
                        let files = d.lines().filter(|l|l.starts_with("diff --git")).count();
                        let add = d.lines().filter(|l|l.starts_with('+')&&!l.starts_with("+++")).count();
                        let rem = d.lines().filter(|l|l.starts_with('-')&&!l.starts_with("---")).count();
                        eprintln!("\n🔍 Review  {} files  +{}/-{}\n", files, add, rem);
                        let ci = jcode_micro_ci::MicroCi::new(jcode_micro_ci::CiConfig { workspace_root: cwd.to_string_lossy().to_string(), ..Default::default() });
                        let r = ci.run().await;
                        let issues = r.issues;
                        if !issues.is_empty() { eprintln!("  Issues: {}", issues.len()); for i in issues.iter().take(10) { eprintln!("    [{}] {}:{}", i.severity, i.file.as_deref().unwrap_or("unknown"), i.line.unwrap_or(0)); } }
                        else { eprintln!("  ✅ No issues.\n"); }
                    }
                    _ => eprintln!("  No uncommitted changes.\n"),
                }
            });
            SlashResult::Ok("Review.".into())
        }),
    ).await;
}
