//! Enhanced CLI Commands Module
//!
//! Integrates all enhanced CLI functionality:
//! - Git workflow commands (commit, branch, diff, etc.)
//! - Cost tracking and budget management  
//! - Task management and project planning
//! - System diagnostics

pub mod git_commands;
pub mod cost_tracker;
pub mod task_manager;

// Re-export main types for convenience
pub use git_commands::{GitCommands, GitWorkflow, DefaultGitWorkflow, GitConfig, CommitInfo};
pub use cost_tracker::{CostCommands, CostTracker, TokenUsage, ModelPricing};
pub use task_manager::{TaskCommands, TaskManager, Task, TaskOptions};

/// Enhanced CLI application state
pub struct EnhancedCli {
    pub git: GitCommands,
    pub cost: CostCommands,
    pub tasks: TaskCommands,
}

impl EnhancedCli {
    /// Create new enhanced CLI instance with default components
    pub fn new() -> Self {
        Self {
            git: GitCommands::with_default_workflow(),
            cost: CostCommands::with_default_tracker(),
            tasks: TaskCommands::with_default_manager(),
        }
    }

    /// Create with custom components
    pub fn with_components(
        git_workflow: Box<dyn git_commands::GitWorkflow>,
        cost_tracker: cost_tracker::CostTracker,
        task_manager: std::sync::Arc<task_manager::TaskManager>,
    ) -> Self {
        Self {
            git: git_commands::GitCommands::new(git_workflow),
            cost: cost_tracker::CostCommands::new(cost_tracker),
            tasks: task_manager::TaskCommands::new(task_manager),
        }
    }

    /// Handle CLI command routing
    pub async fn handle_command(&self, command: &str, args: &[String]) -> Result<()> {
        match command {
            "git" => self.handle_git_command(args).await?,
            "cost" => self.handle_cost_command(args).await?,
            "task" => self.handle_task_command(args).await?,
            "status" => self.show_status().await?,
            _ => self.show_help()?,
        }
        
        Ok(())
    }

    async fn handle_git_command(&self, args: &[String]) -> Result<()> {
        if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
            println!("🔀 Git Workflow Commands");
            println!("═" .repeat(50));
            println!();
            println!("Usage: carpai git <command> [options]");
            println!();
            println!("Commands:");
            println!("  commit [message] [--amend]  Create a new commit");
            println!("  branch <name> [base]       Create new branch");
            println!("  branches [-r]               List branches");
            println!("  diff [--staged] [file]      Show changes");
            println!("  status                     Show working tree status");
            println!("  cherry-pick <commits>       Cherry-pick commits");
            println!("  stash [message]             Stash changes");
            println!("  stash-pop [index]           Restore stashed changes");
            println!();
            println!("Examples:");
            println!("  carpai git commit \"feat: add auth\"");
            println!("  carpai git branch feature/login main");
            println!("  carpai git diff src/main.rs");
            
            return Ok(());
        }

        match args[0].as_str() {
            "commit" => {
                let message = if args.len() > 1 { Some(&args[1]) } else { None };
                let amend = args.contains(&"--amend".to_string());
                self.git.handle_commit(message, amend).await?;
            }
            "branch" | "create-branch" => {
                if args.len() < 2 {
                    anyhow::bail!("Branch name required. Usage: carpai git branch <name> [base]");
                }
                let base = if args.len() > 2 { Some(&args[2]) } else { None };
                self.git.handle_create_branch(&args[1], base).await?;
            }
            "branches" | "branch-list" => {
                let remote = args.contains(&"-r".to_string()) || args.contains(&"--remote".to_string());
                let branches = self.git.workflow.list_branches(remote).await?;
                
                println!("📂 Branches{}", if remote { " (remote)" } else { "" });
                for branch in &branches {
                    let current = if branch.is_current { " * " } else { "   " };
                    println!("{}{} {}", current, branch.name, 
                        if branch.is_remote { "(remote)" } else { "" });
                }
            }
            "diff" => {
                let staged = args.contains(&"--staged".to_string());
                let file = args.iter().find(|a| !a.starts_with('-')).map(|s| s.as_str());
                self.git.handle_diff(staged, file).await?;
            }
            "status" => {
                self.git.handle_status().await?;
            }
            _ => {
                anyhow::bail!("Unknown git command: {}. Use 'carpai git --help' for usage", args[0]);
            }
        }

        Ok(())
    }

    async fn handle_cost_command(&self, args: &[String]) -> Result<()> {
        if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
            println!("💰 Cost Tracking Commands");
            println!("═" .repeat(50));
            println!();
            println!("Usage: carpai cost <command> [options]");
            println!();
            println!("Commands:");
            println!("  session              Show current session costs");
            println!("  daily                Show today's total costs");
            println!("  forecast             Estimate monthly spending");
            println!("  budget <amount>      Set/check daily budget limit");
            println!("  reset                Reset all counters");
            println!();
            println!("Examples:");
            println!("  carpai cost session");
            println!("  carpai cost budget 10.00");
            println!("  carpai cost reset");
            
            return Ok(());
        }

        match args[0].as_str() {
            "session" => {
                self.cost.show_session_cost().await?;
            }
            "daily" => {
                // For now, same as session - would integrate with persistent storage later
                self.cost.show_session_cost().await?;
                println!("\n💡 Tip: Use 'carpai cost reset' to start a new tracking period");
            }
            "forecast" => {
                let report = self.cost.tracker.get_session_cost();
                let monthly_estimate = report.total_cost * 30.0; // Rough estimate
                
                println!("📈 Cost Forecast");
                println!("═" .repeat(50));
                println!("Current session: ${:.4}", report.total_cost);
                println!("Estimated monthly: ${:.2}", monthly_estimate);
                println!("Estimated yearly:  ${:.2}", monthly_estimate * 12.0);
            }
            "budget" => {
                if args.len() < 2 {
                    // Show current budget status with default $10/day
                    self.cost.show_budget_status(10.0).await?;
                } else {
                    let limit: f64 = args[1].parse()
                        .map_err(|_| anyhow::anyhow!("Invalid amount: {}", args[1]))?;
                    self.cost.show_budget_status(limit).await?;
                }
            }
            "reset" => {
                self.cost.reset_counters().await?;
            }
            _ => {
                anyhow::bail!("Unknown cost command: {}. Use 'carpai cost --help' for usage", args[0]);
            }
        }

        Ok(())
    }

    async fn handle_task_command(&self, args: &[String]) -> Result<()> {
        if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
            println!("📋 Task Management Commands");
            println!("═" .repeat(50));
            println!();
            println!("Usage: carpai task <command> [options]");
            println!();
            println!("Commands:");
            println!("  create <title>         Create new task");
            println!("  list                   List all tasks");
            println!("  show <id>              Show task details");
            println!("  update <id>            Update task");
            println!("  stats                  Show statistics");
            println!();
            println!("Create Options:");
            println!("  --priority <level>     Set priority (low|medium|high|critical)");
            println!("  --assignee <name>      Assign to user");
            println!("  --tags <tag1,tag2>     Add tags");
            println!("  --due <date>           Set due date");
            println!();
            println!("Examples:");
            println!("  carpai task create \"Implement OAuth2\" --priority high");
            println!("  carpai task list");
            println!("  carpai task stats");
            
            return Ok(());
        }

        match args[0].as_str() {
            "create" | "new" => {
                if args.len() < 2 {
                    anyhow::bail!("Task title required. Usage: carpai task create <title>");
                }
                
                // Parse options
                let mut options = TaskOptions::default();
                let mut i = 2;
                while i < args.len() {
                    match args[i].as_str() {
                        "--priority" => {
                            i += 1;
                            if i < args.len() {
                                options.priority = Some(match args[i].as_str().to_lowercase().as_str() {
                                    "critical" => TaskPriority::Critical,
                                    "high" => TaskPriority::High,
                                    "medium" => TaskPriority::Medium,
                                    "low" => TaskPriority::Low,
                                    _ => return Err(anyhow::anyhow!("Invalid priority: {}", args[i])),
                                });
                            }
                        }
                        "--assignee" => {
                            i += 1;
                            if i < args.len() {
                                options.assignee = Some(args[i].clone());
                            }
                        }
                        "--tags" => {
                            i += 1;
                            if i < args.len() {
                                options.tags = Some(args[i].split(',').map(String::from).collect());
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }

                self.tasks.handle_create(&args[1], options).await?;
            }
            "list" | "ls" => {
                self.tasks.handle_list(None).await?;
            }
            "show" => {
                if args.len() < 2 {
                    anyhow::bail!("Task ID required. Usage: carpai task show <id>");
                }
                self.tasks.handle_show(&args[1]).await?;
            }
            "update" => {
                if args.len() < 2 {
                    anyhow::bail!("Task ID required. Usage: carpai task update <id>");
                }
                self.tasks.handle_update(&args[1], TaskUpdates::default()).await?;
            }
            "stats" | "statistics" => {
                self.tasks.handle_stats().await?;
            }
            _ => {
                anyhow::bail!("Unknown task command: {}. Use 'carpai task --help' for usage", args[0]);
            }
        }

        Ok(())
    }

    async fn show_status(&self) -> Result<()> {
        println!("🚀 CarpAI Status Dashboard");
        println!("═" .repeat(60));

        // Git status
        println!("\n📁 Repository:");
        if let Err(e) = self.git.handle_status().await {
            println!("   ⚠️  Not a git repository or error: {}", e);
        }

        // Cost summary
        println!("\n💰 Session Costs:");
        let report = self.cost.tracker.get_session_cost();
        println!("   Total tokens: {}", report.total_tokens);
        println!("   Total cost:   ${:.4} USD", report.total_cost);

        // Task statistics
        println!("\n📋 Tasks:");
        let stats = self.tasks.manager.get_statistics().await;
        println!("   Total: {}, Completed: {}, In Progress: {}", 
            stats.total, stats.completed, stats.in_progress);

        println!("\n✅ Status check complete!");
        
        Ok(())
    }

    fn show_help(&self) -> Result<()> {
        println!("🚀 CarpAI Enhanced CLI");
        println!("═" .repeat(60));
        println!();
        println!("Usage: carpai <category> <command> [options]");
        println!();
        println!("Categories:");
        println!("  git     Git workflow operations");
        println!("  cost    Token usage and cost tracking");
        println!("  task    Project task management");
        println!("  status  Show overall system status");
        println!();
        println!("Global Options:");
        println!("  --help, -h     Show help information");
        println!("  --version     Show version number");
        println!("  --verbose     Enable verbose output");
        println!();
        println!("For category-specific help:");
        println!("  carpai git --help");
        println!("  carpai cost --help");
        println!("  carpai task --help");
        println!();
        println!("Examples:");
        println!("  carpai git commit \"feat: add authentication\"");
        println!("  carpai task create \"Fix login bug\" --priority critical");
        println!("  carpai cost session");
        println!("  carpai status");

        Ok(())
    }
}

impl Default for EnhancedCli {
    fn default() -> Self {
        Self::new()
    }
}
