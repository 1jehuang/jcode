use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub(crate) enum McpCommand {
    /// Connect once, list server info/tools, and disconnect (never starts OAuth login)
    Test {
        /// Server name from the merged MCP configuration, including disabled entries
        server: String,
    },
    /// List configured servers and local authentication state without connecting
    List {
        /// Print machine-readable output, excluding credentials
        #[arg(long)]
        json: bool,
    },
    /// Explicitly sign in to a configured remote MCP server using OAuth
    Login {
        /// Server name from the merged MCP configuration
        server: String,
        /// Print the authorization URL instead of opening a local browser
        #[arg(long)]
        no_browser: bool,
    },
    /// Forget local OAuth credentials (does not revoke tokens or remove static headers)
    Logout {
        /// Server name from the merged MCP configuration
        server: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{Args, Command};
    use clap::Parser;

    #[test]
    fn mcp_cli_parses_supported_commands() {
        let args = Args::try_parse_from(["jcode", "mcp", "list", "--json"]).unwrap();
        assert!(matches!(
            args.command,
            Some(Command::Mcp {
                action: McpCommand::List { json: true }
            })
        ));
        for no_browser in [false, true] {
            let mut argv = vec!["jcode", "mcp", "login", "remote"];
            if no_browser {
                argv.push("--no-browser");
            }
            let args = Args::try_parse_from(argv).unwrap();
            assert!(
                matches!(args.command, Some(Command::Mcp { action: McpCommand::Login { server, no_browser: actual } }) if server == "remote" && actual == no_browser)
            );
        }
        let args = Args::try_parse_from(["jcode", "mcp", "logout", "remote"]).unwrap();
        assert!(
            matches!(args.command, Some(Command::Mcp { action: McpCommand::Logout { server } }) if server == "remote")
        );
        let args = Args::try_parse_from(["jcode", "mcp", "test", "remote"]).unwrap();
        assert!(
            matches!(args.command, Some(Command::Mcp { action: McpCommand::Test { server } }) if server == "remote")
        );
    }

    #[test]
    fn mcp_cli_requires_an_explicit_action_and_login_server() {
        for argv in [
            vec!["jcode", "mcp"],
            vec!["jcode", "mcp", "login"],
            vec!["jcode", "mcp", "logout"],
            vec!["jcode", "mcp", "test"],
            vec!["jcode", "mcp", "list", "--no-browser"],
        ] {
            assert!(Args::try_parse_from(argv).is_err());
        }
    }
}
