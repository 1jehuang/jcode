//! Commit-Push-PR command - One-shot GitHub workflow
//!
//! 对标: Claude Code `commit-push-pr` command

use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct CommitPushPrCommand;

impl Command for CommitPushPrCommand {
    fn name(&self) -> &str {
        "commit-push-pr"
    }

    fn description(&self) -> &str {
        "One-command workflow: git add → commit (AI message) → push → create PR"
    }

    async fn execute(&self, args: &[String]) -> Result<CommandResult> {
        let mut title: Option<String> = None;
        let mut description: Option<String> = None;
        let mut base_branch = "main".to_string();

        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--title" => {
                    if i + 1 < args.len() {
                        title = Some(args[i + 1].clone());
                        i += 1;
                    }
                }
                "--description" => {
                    if i + 1 < args.len() {
                        description = Some(args[i + 1].clone());
                        i += 1;
                    }
                }
                "--base" => {
                    if i + 1 < args.len() {
                        base_branch = args[i + 1].clone();
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }

        println!("🚀 Starting commit-push-pr workflow...");

        // Step 1: git add
        println!("\n[1/4] Staging changes...");
        run_git(&["add", "-A"]).await?;

        // Step 2: git commit with AI-generated message
        println!("[2/4] Creating commit...");
        let commit_msg = if let Some(t) = title {
            t
        } else {
            generate_commit_message().await?
        };
        run_git(&["commit", "-m", &commit_msg]).await?;

        // Step 3: git push
        println!("[3/4] Pushing to remote...");
        run_git(&["push", "-u", "origin", "HEAD"]).await?;

        // Step 4: gh pr create
        println!("[4/4] Creating pull request...");
        create_pr(&commit_msg, description.as_deref(), &base_branch).await?;

        println!("\n✅ Workflow complete!");
        Ok(CommandResult::success("PR created successfully"))
    }
}

async fn run_git(args: &[&str]) -> Result<()> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git {} failed: {}", args.join(" "), stderr);
    }

    Ok(())
}

async fn generate_commit_message() -> Result<String> {
    tracing::info!("generate_commit_message: LLM-based smart commit message pending integration");
    // Integration: Send git diff to LLM with Conventional Commits prompt
    // Fallback: Use "chore: auto-generated commit" when LLM unavailable
    Ok("chore: auto-generated commit".to_string())
}

async fn create_pr(title: &str, description: Option<&str>, base: &str) -> Result<()> {
    let mut cmd = tokio::process::Command::new("gh");
    cmd.args(&["pr", "create", "--title", title, "--base", base]);

    if let Some(desc) = description {
        cmd.arg("--body").arg(desc);
    }

    let output = cmd.output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("gh pr create failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("   {}", stdout.trim());

    Ok(())
}
