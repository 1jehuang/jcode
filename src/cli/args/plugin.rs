//! Plugin management arguments, kept separate from interactive startup.
use clap::{Args, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum PluginScopeArg {
    #[default]
    User,
    Project,
    Local,
}

#[derive(Debug, Args)]
pub(crate) struct PluginArgs {
    /// Emit machine-readable JSON (also accepted after subcommands).
    #[arg(long, global = true)]
    pub json: bool,
    /// Settings scope for plugin mutations.
    #[arg(long, value_enum, default_value = "user", global = true)]
    pub scope: PluginScopeArg,
    #[command(subcommand)]
    pub action: PluginCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PluginCommand {
    /// List installed plugins.
    List,
    /// Install a plugin by name or name@marketplace.
    Install { plugin: String },
    /// Uninstall a plugin from the selected scope.
    Uninstall { plugin: String },
    /// Enable a plugin in the selected scope.
    Enable { plugin: String },
    /// Disable a plugin in the selected scope.
    Disable { plugin: String },
    /// Update an installed plugin.
    Update { plugin: String },
    /// Validate a plugin directory without installing it.
    Validate { path: PathBuf },
    /// Manage plugin marketplaces.
    Marketplace {
        #[command(subcommand)]
        action: MarketplaceCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum MarketplaceCommand {
    /// Register a local or remote marketplace.
    Add { source: String },
    /// List registered marketplaces.
    List,
    /// Remove a registered marketplace.
    Remove { name: String },
    /// Refresh a registered marketplace.
    Update { name: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{Args, Command};
    use clap::Parser;

    // The full debug CLI parser uses the same stack allowance as update tests.
    #[inline(never)]
    fn parse(argv: &[&str]) -> Result<Args, clap::Error> {
        let argv: Vec<String> = argv.iter().map(|arg| (*arg).to_owned()).collect();
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(move || Args::try_parse_from(argv))
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn plugin_commands_parse_with_global_output_and_scope_flags() {
        for command in ["install", "uninstall", "enable", "disable", "update"] {
            let args = parse(&[
                "jcode",
                "plugin",
                command,
                "sample@market",
                "--scope",
                "project",
                "--json",
            ])
            .unwrap();
            let Some(Command::Plugin(plugin)) = args.command else {
                panic!("plugin command")
            };
            assert!(plugin.json);
            assert_eq!(plugin.scope, PluginScopeArg::Project);
        }
        let args = parse(&["jcode", "plugin", "--json", "list"]).unwrap();
        let Some(Command::Plugin(plugin)) = args.command else {
            panic!("plugin command")
        };
        assert!(plugin.json);
        assert_eq!(plugin.scope, PluginScopeArg::User);
    }

    #[test]
    fn marketplace_commands_and_validation_parse() {
        for command in ["add", "remove", "update"] {
            assert!(
                parse(&[
                    "jcode",
                    "plugin",
                    "marketplace",
                    command,
                    "example",
                    "--json"
                ])
                .is_ok()
            );
        }
        assert!(parse(&["jcode", "plugin", "marketplace", "list", "--json"]).is_ok());
        assert!(parse(&["jcode", "plugin", "validate", ".", "--scope", "local"]).is_ok());
    }

    #[test]
    fn invalid_scope_and_missing_operands_are_rejected() {
        assert!(parse(&["jcode", "plugin", "list", "--scope", "workspace"]).is_err());
        for command in [
            "install",
            "uninstall",
            "enable",
            "disable",
            "update",
            "validate",
        ] {
            assert!(parse(&["jcode", "plugin", command]).is_err());
        }
    }
}
