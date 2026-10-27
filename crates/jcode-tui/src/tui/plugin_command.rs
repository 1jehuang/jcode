//! `/plugin` command grammar, shared by the interactive dialog and slash arguments.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PluginCommand {
    Browse(String),
    Run(Vec<String>),
}

pub(crate) const USAGE: &str = "Usage: /plugin [discover|installed|errors|list|install <name@marketplace>|enable <plugin>|disable <plugin>|uninstall <plugin>|update <plugin>] [--scope user|project|local]\n/plugin marketplace [list|add <source>|remove <name>|update <name>]";

pub(crate) fn parse(input: &str) -> Option<Result<PluginCommand, String>> {
    let mut words = input.split_whitespace();
    if words.next()? != "/plugin" {
        return None;
    }
    let args: Vec<String> = words.map(str::to_owned).collect();
    Some(parse_args(args))
}

fn parse_args(mut args: Vec<String>) -> Result<PluginCommand, String> {
    if args.is_empty() {
        return Ok(PluginCommand::Browse("discover".into()));
    }
    if args.len() == 1 {
        match args[0].as_str() {
            "discover" | "installed" | "errors" => {
                return Ok(PluginCommand::Browse(args.remove(0)));
            }
            "list" => return Ok(PluginCommand::Browse("installed".into())),
            "marketplaces" | "marketplace" => {
                return Ok(PluginCommand::Browse("marketplaces".into()));
            }
            _ => {}
        }
    }
    if args.first().map(String::as_str) == Some("marketplace") {
        let valid = match args.get(1).map(String::as_str) {
            Some("list") => args.len() == 2,
            Some("add" | "remove" | "update") => args.len() == 3,
            _ => false,
        };
        return if valid {
            Ok(PluginCommand::Run(args))
        } else {
            Err(USAGE.into())
        };
    }
    if !matches!(
        args[0].as_str(),
        "install" | "enable" | "disable" | "uninstall" | "update"
    ) {
        return Err(USAGE.into());
    }
    if args.len() == 4
        && args[2] == "--scope"
        && matches!(args[3].as_str(), "user" | "project" | "local")
    {
        return Ok(PluginCommand::Run(args));
    }
    if args.len() == 2 && !args[1].starts_with('-') {
        return Ok(PluginCommand::Run(args));
    }
    Err(USAGE.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_command_boundary() {
        for input in ["/pluginatic", "/plugins", "hello /plugin"] {
            assert!(parse(input).is_none());
        }
        assert_eq!(
            parse("/plugin\tlist"),
            Some(Ok(PluginCommand::Browse("installed".into())))
        );
    }

    #[test]
    fn accepts_real_operations_and_scope() {
        for operation in ["install", "enable", "disable", "uninstall", "update"] {
            assert!(
                parse(&format!("/plugin {operation} lint@tools --scope project"))
                    .unwrap()
                    .is_ok()
            );
        }
        for args in [
            "marketplace list",
            "marketplace add owner/repo",
            "marketplace remove tools",
            "marketplace update tools",
        ] {
            assert!(parse(&format!("/plugin {args}")).unwrap().is_ok());
        }
    }

    #[test]
    fn rejects_unsupported_or_ambiguous_arguments() {
        for args in [
            "install",
            "enable a b",
            "install x --scope system",
            "marketplace update",
            "marketplace invent x",
            "update --all",
            "list unexpected",
        ] {
            assert!(
                parse(&format!("/plugin {args}")).unwrap().is_err(),
                "{args}"
            );
        }
    }
}
