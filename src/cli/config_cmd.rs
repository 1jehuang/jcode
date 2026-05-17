//! Config management commands
//!
//! Extracted from commands.rs for better modularity.

use anyhow::Result;

// Config management commands
// ════════════════════════════════════════════════════════════════════

pub fn run_config_command(cmd: super::args::ConfigCommand) -> Result<()> {
    match cmd {
        super::args::ConfigCommand::Get { key } => {
            match std::env::var(&key) {
                Ok(val) => println!("{}={}", key, val),
                Err(_) => eprintln!("Config key '{}' not found", key),
            }
        }
        super::args::ConfigCommand::Set { key, value } => {
            // SAFETY: set_var is called in a single-threaded CLI context
            unsafe { std::env::set_var(&key, &value); }
            eprintln!("✅ Set {}={}", key, value);
            eprintln!("  (Note: env vars are session-scoped; use config file for persistence)");
        }
        super::args::ConfigCommand::List { json } => {
            use std::env;
            let vars: std::collections::BTreeMap<String, String> = env::vars()
                .filter(|(k, _)| k.starts_with("CARPAI_") || k.starts_with("JCODE_") || k.starts_with("CLAUDE_"))
                .collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&vars)?);
            } else {
                if vars.is_empty() {
                    eprintln!("No CarpAI/JCODE config variables found.");
                } else {
                    eprintln!("\n⚙️  Config:\n");
                    for (k, v) in &vars {
                        let display = if k.contains("KEY") || k.contains("TOKEN") || k.contains("SECRET") {
                            format!("{}...", &v[..v.len().min(8)])
                        } else {
                            v.clone()
                        };
                        eprintln!("  {}={}", k, display);
                    }
                }
            }
        }
    }
    Ok(())
}

// ════════════════════════════════════════════════════════════════════