use super::*;

#[test]
fn claude_cli_login_and_explicit_oauth_flags_parse() {
    let args =
        parse_login_args(&["jcode", "login", "--provider", "claude", "--claude-code"]).unwrap();
    assert!(matches!(
        args.command,
        Some(Command::Login {
            claude_code: true,
            oauth: false,
            ..
        })
    ));
    let args = parse_login_args(&["jcode", "login", "--provider", "claude", "--oauth"]).unwrap();
    assert!(matches!(
        args.command,
        Some(Command::Login {
            claude_code: false,
            oauth: true,
            ..
        })
    ));
}
#[test]
fn claude_cli_login_rejects_conflicting_account_browser_and_oauth_flags() {
    for extra in [
        vec!["--no-browser"],
        vec!["--account", "work"],
        vec!["--oauth"],
    ] {
        let mut args = vec!["jcode", "login", "--provider", "claude", "--claude-code"];
        args.extend(extra);
        assert!(parse_login_args(&args).is_err());
    }
}

// Match the existing update-parser fixture: Clap's unoptimized command tree
// needs more stack than Rust's default test worker supplies.
#[inline(never)]
pub(super) fn parse_login_args(argv: &[&str]) -> Result<Args, clap::Error> {
    let owned = argv.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
    std::thread::Builder::new()
        .name("parse-login-cli".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || Args::try_parse_from(owned))
        .expect("spawn CLI parser fixture")
        .join()
        .expect("CLI parser fixture panicked")
}
