use std::path::PathBuf;
use super::operations::GitOperations;

/// Git branch command handler
pub struct GitBranchCommand;

impl GitBranchCommand {
    pub fn execute(args: &[String], repo_path: &PathBuf) -> String {
        let git = GitOperations::new(repo_path.clone());

        if args.is_empty() || (args.len() == 1 && args[0] == "list") {
            let branches = git.list_branches();
            let mut output = String::from("Branches:\n");
            for branch in &branches {
                let marker = if branch.current { "* " } else { "  " };
                output.push_str(&format!("{}{}\n", marker, branch.name));
            }
            return output;
        }

        match args[0].as_str() {
            "create" | "new" => {
                if args.len() < 2 {
                    return "Usage: git branch create <name>".to_string();
                }
                match git.create_branch(&args[1]) {
                    Ok(msg) => msg,
                    Err(e) => format!("Error: {}", e),
                }
            }
            "checkout" | "switch" => {
                if args.len() < 2 {
                    return "Usage: git branch checkout <name>".to_string();
                }
                match git.checkout_branch(&args[1]) {
                    Ok(msg) => msg,
                    Err(e) => format!("Error: {}", e),
                }
            }
            "delete" | "remove" => {
                if args.len() < 2 {
                    return "Usage: git branch delete <name> [--force]".to_string();
                }
                let force = args.contains(&"--force".to_string());
                match git.delete_branch(&args[1], force) {
                    Ok(msg) => msg,
                    Err(e) => format!("Error: {}", e),
                }
            }
            _ => format!("Unknown subcommand: {}. Use: list, create, checkout, delete", args[0]),
        }
    }
}

/// Git diff command handler
pub struct GitDiffCommand;

impl GitDiffCommand {
    pub fn execute(args: &[String], repo_path: &PathBuf) -> String {
        let git = GitOperations::new(repo_path.clone());

        let staged = args.iter().any(|a| a == "--staged" || a == "--cached");

        if args.iter().any(|a| a == "--stat" || a == "-s") {
            let changes = if staged { git.diff_staged() } else { git.diff_unstaged() };
            let mut output = String::new();
            for change in &changes {
                output.push_str(&format!("  {} (+{} -{})\n", change.path, change.additions, change.deletions));
            }
            if output.is_empty() {
                output = "No changes.".to_string();
            }
            return output;
        }

        git.format_diff(staged)
    }
}

/// Git context command handler
pub struct GitContextCommand;

impl GitContextCommand {
    pub fn execute(_args: &[String], repo_path: &PathBuf) -> String {
        let git = GitOperations::new(repo_path.clone());
        git.format_context_summary()
    }
}