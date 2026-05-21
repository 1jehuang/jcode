//! File listing command - Browse and search project files
//!
//! 对标: Claude Code `files` command

use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct FilesCommand;

impl Command for FilesCommand {
    fn name(&self) -> &str {
        "files"
    }

    fn description(&self) -> &str {
        "Browse and filter project files with smart search"
    }

    async fn execute(&self, args: &[String]) -> Result<CommandResult> {
        let mut file_type: Option<String> = None;
        let mut modified: Option<String> = None;
        let mut limit = 20;

        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--type" => {
                    if i + 1 < args.len() {
                        file_type = Some(args[i + 1].clone());
                        i += 1;
                    }
                }
                "--modified" => {
                    if i + 1 < args.len() {
                        modified = Some(args[i + 1].clone());
                        i += 1;
                    }
                }
                "--limit" => {
                    if i + 1 < args.len() {
                        limit = args[i + 1].parse().unwrap_or(20);
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }

        println!("📁 Listing files...");

        // TODO: Implement smart file browsing
        // For now, use basic glob
        
        let pattern = if let Some(ref ext) = file_type {
            format!("**/*.{}", ext.trim_start_matches('.'))
        } else {
            "**/*".to_string()
        };

        let mut count = 0;
        for entry in glob::glob(&pattern).take(limit as usize) {
            if let Ok(path) = entry {
                println!("   {}", path.display());
                count += 1;
            }
        }

        println!("\nTotal: {} files", count);
        Ok(CommandResult::success(format!("Listed {} files", count)))
    }

    fn is_read_only(&self) -> bool {
        true
    }
}
