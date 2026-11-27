use clap::{Subcommand, ValueEnum};
use std::path::PathBuf;

/// Which part of a project's configuration a command covers.
#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum TrustScopeArg {
    /// Project `[hooks]` in `.jcode/config.toml`
    Hooks,
    /// Project MCP servers that run commands
    Mcp,
    /// Policy fields (model, effort, tools) of project agent definitions
    Agents,
    /// Every scope
    All,
}

#[derive(Subcommand, Debug)]
pub(crate) enum TrustCommand {
    /// Show what a project wants to run and whether each scope is trusted
    Show {
        /// Project directory (defaults to the current directory)
        path: Option<PathBuf>,
    },
    /// Approve the exact current snapshot of a project's scopes
    Grant {
        /// Project directory (defaults to the current directory)
        path: Option<PathBuf>,
        /// Scope to approve
        #[arg(long, value_enum, default_value = "all")]
        scope: TrustScopeArg,
        /// Approve without a prompt, after reviewing `jcode trust show`
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Remove approval for a project
    Revoke {
        /// Project directory (defaults to the current directory)
        path: Option<PathBuf>,
        /// Scope to revoke
        #[arg(long, value_enum, default_value = "all")]
        scope: TrustScopeArg,
    },
    /// List every approved project and scope
    List,
}
