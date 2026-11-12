use super::*;

#[test]
fn mcp_trust_subcommands_parse() {
    let trust = Args::try_parse_from(["jcode", "mcp", "trust", "/tmp/project", "--yes"])
        .expect("mcp trust should parse");
    assert!(matches!(
        trust.command,
        Some(Command::Mcp {
            action: McpCommand::Trust {
                path: Some(_),
                yes: true
            }
        })
    ));

    let revoke = Args::try_parse_from(["jcode", "mcp", "revoke"]).expect("mcp revoke should parse");
    assert!(matches!(
        revoke.command,
        Some(Command::Mcp {
            action: McpCommand::Revoke { path: None }
        })
    ));
}

#[test]
fn trust_subcommands_parse() {
    let grant = Args::try_parse_from([
        "jcode", "trust", "grant", "/tmp/p", "--scope", "hooks", "-y",
    ])
    .expect("trust grant should parse");
    assert!(matches!(
        grant.command,
        Some(Command::Trust {
            action: TrustCommand::Grant {
                path: Some(_),
                scope: TrustScopeArg::Hooks,
                yes: true
            }
        })
    ));
    let default_scope = Args::try_parse_from(["jcode", "trust", "grant"]).unwrap();
    assert!(matches!(
        default_scope.command,
        Some(Command::Trust {
            action: TrustCommand::Grant {
                scope: TrustScopeArg::All,
                yes: false,
                ..
            }
        })
    ));
    assert!(matches!(
        Args::try_parse_from(["jcode", "trust", "list"])
            .unwrap()
            .command,
        Some(Command::Trust {
            action: TrustCommand::List
        })
    ));
    assert!(Args::try_parse_from(["jcode", "trust", "grant", "--scope", "bogus"]).is_err());
}
