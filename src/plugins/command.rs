use super::manager::PluginManager;

pub struct PluginCommand;

impl PluginCommand {
    pub fn execute(args: &[String], manager: &mut PluginManager) -> String {
        if args.is_empty() {
            return Self::usage().to_string();
        }
        match args[0].as_str() {
            "list" | "ls" => Self::list_plugins(manager),
            "add" | "install" => {
                if args.len() < 2 { return "Usage: plugin add <path>".to_string(); }
                match manager.add(std::path::Path::new(&args[1])) {
                    Ok(msg) => msg,
                    Err(e) => format!("Error: {}", e),
                }
            }
            "remove" | "rm" | "uninstall" => {
                if args.len() < 2 { return "Usage: plugin remove <name>".to_string(); }
                match manager.remove(&args[1]) {
                    Ok(_) => format!("Plugin '{}' removed", args[1]),
                    Err(e) => format!("Error: {}", e),
                }
            }
            "enable" => {
                if args.len() < 2 { return "Usage: plugin enable <name>".to_string(); }
                match manager.enable(&args[1]) {
                    Ok(_) => format!("Plugin '{}' enabled", args[1]),
                    Err(e) => format!("Error: {}", e),
                }
            }
            "disable" => {
                if args.len() < 2 { return "Usage: plugin disable <name>".to_string(); }
                match manager.disable(&args[1]) {
                    Ok(_) => format!("Plugin '{}' disabled", args[1]),
                    Err(e) => format!("Error: {}", e),
                }
            }
            _ => format!("Unknown subcommand: {}. {}", args[0], Self::usage()),
        }
    }

    fn usage() -> &'static str {
        "Usage: plugin <list|add|remove|enable|disable>"
    }

    fn list_plugins(manager: &PluginManager) -> String {
        let plugins = manager.list();
        if plugins.is_empty() {
            return "No plugins installed.".to_string();
        }
        let mut output = format!("Plugins ({} total, {} enabled):\n",
            manager.count(), manager.count());
        for p in plugins {
            let status = if p.enabled { "✅" } else { "⏸️" };
            output.push_str(&format!("  {} {} v{} — {}\n", status, p.manifest.name, p.manifest.version, p.manifest.description));
        }
        output
    }
}