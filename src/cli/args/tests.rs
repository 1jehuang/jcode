use super::*;
use crate::cli::provider_init::ProviderChoice;

#[test]
fn server_start_and_internal_keepalive_parse() {
    let args = Args::try_parse_from(["jcode", "server", "start", "--json"])
        .expect("server start should parse");
    assert!(matches!(
        args.command,
        Some(Command::Server {
            action: ServerCommand::Start { json: true }
        })
    ));

    let keepalive = Args::try_parse_from(["jcode", "server", "keepalive"])
        .expect("internal server keepalive should parse");
    assert!(matches!(
        keepalive.command,
        Some(Command::Server {
            action: ServerCommand::Keepalive
        })
    ));
}

#[test]
fn test_provider_choice_aliases_parse() {
    let args = Args::try_parse_from(["jcode", "--provider", "z.ai", "run", "smoke"]).unwrap();
    assert_eq!(args.provider, ProviderChoice::Zai);

    let args =
        Args::try_parse_from(["jcode", "--provider", "kimi-for-coding", "run", "smoke"]).unwrap();
    assert_eq!(args.provider, ProviderChoice::Kimi);

    let args =
        Args::try_parse_from(["jcode", "--provider", "cerebrascode", "run", "smoke"]).unwrap();
    assert_eq!(args.provider, ProviderChoice::Cerebras);

    let args = Args::try_parse_from(["jcode", "--provider", "compat", "run", "smoke"]).unwrap();
    assert_eq!(args.provider, ProviderChoice::OpenaiCompatible);

    let args = Args::try_parse_from(["jcode", "--provider", "bailian", "run", "smoke"]).unwrap();
    assert_eq!(args.provider, ProviderChoice::AlibabaCodingPlan);

    let args = Args::try_parse_from(["jcode", "--provider", "together", "run", "smoke"]).unwrap();
    assert_eq!(args.provider, ProviderChoice::TogetherAi);

    let args = Args::try_parse_from(["jcode", "--provider", "grok", "run", "smoke"]).unwrap();
    assert_eq!(args.provider, ProviderChoice::Xai);

    let args = Args::try_parse_from(["jcode", "--provider", "cgc", "run", "smoke"]).unwrap();
    assert_eq!(args.provider, ProviderChoice::Comtegra);
}

#[test]
fn serve_server_name_option_parses() {
    let args =
        Args::try_parse_from(["jcode", "serve", "--server-name", "mount-cloud/fabian"]).unwrap();
    match args.command {
        Some(Command::Serve { server_name, .. }) => {
            assert_eq!(server_name.as_deref(), Some("mount-cloud/fabian"));
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn remote_working_dir_option_parses() {
    let args = Args::try_parse_from([
        "jcode",
        "--socket",
        "/tmp/jcode.sock",
        "--remote-working-dir",
        "/home/agent/project",
    ])
    .unwrap();

    assert_eq!(
        args.remote_working_dir.as_deref(),
        Some("/home/agent/project")
    );
}

#[test]
fn model_list_subcommand_parses() {
    let args = Args::try_parse_from(["jcode", "model", "list", "--json", "--verbose"]).unwrap();
    match args.command {
        Some(Command::Model(ModelCommand::List { json, verbose })) => {
            assert!(json);
            assert!(verbose);
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args = Args::try_parse_from([
        "jcode",
        "cloud",
        "sessions",
        "dashboard",
        "--limit",
        "10",
        "--open",
        "--with-view",
        "--user-id",
        "jeremy",
    ])
    .unwrap();

    match args.command {
        Some(Command::Cloud(CloudCommand::Sessions {
            action:
                CloudSessionsCommand::Dashboard {
                    limit,
                    output,
                    open,
                    with_view,
                    jade,
                },
        })) => {
            assert_eq!(limit, 10);
            assert!(output.is_none());
            assert!(open);
            assert!(with_view);
            assert_eq!(jade.user_id, "jeremy");
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn session_rename_subcommand_parses() {
    let args = Args::try_parse_from([
        "jcode",
        "session",
        "rename",
        "fox",
        "release planning",
        "--json",
    ])
    .unwrap();
    match args.command {
        Some(Command::Session(SessionCommand::Rename {
            session,
            name,
            clear,
            json,
        })) => {
            assert_eq!(session, "fox");
            assert_eq!(name.as_deref(), Some("release planning"));
            assert!(!clear);
            assert!(json);
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args = Args::try_parse_from(["jcode", "session", "rename", "fox", "--clear"]).unwrap();
    match args.command {
        Some(Command::Session(SessionCommand::Rename {
            session,
            name,
            clear,
            json,
        })) => {
            assert_eq!(session, "fox");
            assert!(name.is_none());
            assert!(clear);
            assert!(!json);
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn cloud_sessions_subcommands_parse() {
    let args = Args::try_parse_from([
        "jcode",
        "cloud",
        "sessions",
        "configure",
        "--api-base",
        "https://jade.example",
        "--api-token-env",
        "JADE_TOKEN",
        "--api-token-id",
        "dev-admin",
        "--user-id",
        "jeremy",
        "--helper",
        "/tmp/jade_sessions.py",
    ])
    .unwrap();

    match args.command {
        Some(Command::Cloud(CloudCommand::Sessions {
            action:
                CloudSessionsCommand::Configure {
                    api_base,
                    api_token_env,
                    api_token_id,
                    user_id,
                    helper,
                    clear,
                    ..
                },
        })) => {
            assert_eq!(api_base.as_deref(), Some("https://jade.example"));
            assert_eq!(api_token_env.as_deref(), Some("JADE_TOKEN"));
            assert_eq!(api_token_id.as_deref(), Some("dev-admin"));
            assert_eq!(user_id.as_deref(), Some("jeremy"));
            assert_eq!(helper.as_deref(), Some("/tmp/jade_sessions.py"));
            assert!(!clear);
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args = Args::try_parse_from(["jcode", "cloud", "sessions", "status", "--json"]).unwrap();
    match args.command {
        Some(Command::Cloud(CloudCommand::Sessions {
            action: CloudSessionsCommand::Status { json },
        })) => assert!(json),
        other => panic!("unexpected command: {:?}", other),
    }

    let args = Args::try_parse_from([
        "jcode",
        "cloud",
        "sessions",
        "upload-latest",
        "--sessions-dir",
        "/tmp/sessions",
        "--user-id",
        "jeremy",
        "--profile",
        "test-profile",
        "--region",
        "us-east-1",
    ])
    .unwrap();

    match args.command {
        Some(Command::Cloud(CloudCommand::Sessions {
            action:
                CloudSessionsCommand::UploadLatest {
                    sessions_dir,
                    raw,
                    jade,
                },
        })) => {
            assert_eq!(sessions_dir, "/tmp/sessions");
            assert!(!raw);
            assert_eq!(jade.user_id, "jeremy");
            assert_eq!(jade.profile.as_deref(), Some("test-profile"));
            assert_eq!(jade.region.as_deref(), Some("us-east-1"));
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args = Args::try_parse_from([
        "jcode",
        "cloud",
        "sessions",
        "view",
        "session_123",
        "--format",
        "html",
        "--open",
    ])
    .unwrap();

    match args.command {
        Some(Command::Cloud(CloudCommand::Sessions {
            action:
                CloudSessionsCommand::View {
                    session_id,
                    format,
                    open,
                    ..
                },
        })) => {
            assert_eq!(session_id, "session_123");
            assert!(matches!(format, CloudSessionViewFormat::Html));
            assert!(open);
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args = Args::try_parse_from([
        "jcode",
        "cloud",
        "sessions",
        "sync",
        "--all",
        "--max",
        "5",
        "--dry-run",
        "--json",
        "--user-id",
        "jeremy",
    ])
    .unwrap();

    match args.command {
        Some(Command::Cloud(CloudCommand::Sessions {
            action:
                CloudSessionsCommand::Sync {
                    sessions_dir,
                    since_days,
                    all,
                    max,
                    min_interval_mins,
                    raw,
                    dry_run,
                    force,
                    json,
                    jade,
                },
        })) => {
            assert!(sessions_dir.is_none());
            assert!(since_days.is_none());
            assert!(all);
            assert_eq!(max, 5);
            assert!(min_interval_mins.is_none());
            assert!(!raw);
            assert!(dry_run);
            assert!(!force);
            assert!(json);
            assert_eq!(jade.user_id, "jeremy");
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn login_no_browser_flag_parses() {
    let args = Args::try_parse_from(["jcode", "login", "--no-browser"]).unwrap();
    match args.command {
        Some(Command::Login {
            provider,
            account,
            no_browser,
            print_auth_url,
            callback_url,
            auth_code,
            json,
            complete,
            google_access_tier,
            api_base,
            api_key,
            api_key_env,
            no_validate,
        }) => {
            assert!(provider.is_none());
            assert!(account.is_none());
            assert!(no_browser);
            assert!(!print_auth_url);
            assert!(callback_url.is_none());
            assert!(auth_code.is_none());
            assert!(!json);
            assert!(!complete);
            assert!(google_access_tier.is_none());
            assert!(api_base.is_none());
            assert!(api_key.is_none());
            assert!(api_key_env.is_none());
            assert!(!no_validate);
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args = Args::try_parse_from(["jcode", "login", "--headless"]).unwrap();
    match args.command {
        Some(Command::Login { no_browser, .. }) => assert!(no_browser),
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn login_accepts_provider_positional() {
    let args = Args::try_parse_from(["jcode", "login", "google"]).unwrap();
    match args.command {
        Some(Command::Login { provider, .. }) => {
            assert_eq!(provider, Some(ProviderChoice::Google));
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn login_openai_compatible_scriptable_flags_parse() {
    let args = Args::try_parse_from([
        "jcode",
        "--provider",
        "openai-compatible",
        "--model",
        "deepseek-v4-flash",
        "login",
        "--api-base",
        "https://api.deepseek.com",
        "--api-key-env",
        "DEEPSEEK_API_KEY",
    ])
    .unwrap();
    assert_eq!(args.provider, ProviderChoice::OpenaiCompatible);
    assert_eq!(args.model.as_deref(), Some("deepseek-v4-flash"));
    match args.command {
        Some(Command::Login {
            api_base,
            api_key_env,
            ..
        }) => {
            assert_eq!(api_base.as_deref(), Some("https://api.deepseek.com"));
            assert_eq!(api_key_env.as_deref(), Some("DEEPSEEK_API_KEY"));
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn login_openai_compatible_accepts_global_provider_and_model_after_subcommand() {
    let args = Args::try_parse_from([
        "jcode",
        "login",
        "--provider",
        "openai-compatible",
        "--api-base",
        "https://api.deepseek.com",
        "--model",
        "deepseek-v4-flash",
    ])
    .unwrap();

    assert_eq!(args.provider, ProviderChoice::OpenaiCompatible);
    assert_eq!(args.model.as_deref(), Some("deepseek-v4-flash"));
    match args.command {
        Some(Command::Login { api_base, .. }) => {
            assert_eq!(api_base.as_deref(), Some("https://api.deepseek.com"));
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn login_scriptable_flags_parse() {
    let args = Args::try_parse_from(["jcode", "login", "--print-auth-url", "--json"]).unwrap();
    match args.command {
        Some(Command::Login {
            print_auth_url,
            json,
            callback_url,
            auth_code,
            complete,
            google_access_tier,
            ..
        }) => {
            assert!(print_auth_url);
            assert!(json);
            assert!(callback_url.is_none());
            assert!(auth_code.is_none());
            assert!(!complete);
            assert!(google_access_tier.is_none());
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args = Args::try_parse_from([
        "jcode",
        "login",
        "--callback-url",
        "http://localhost:1455/auth/callback?code=x&state=y",
    ])
    .unwrap();
    match args.command {
        Some(Command::Login { callback_url, .. }) => {
            assert_eq!(
                callback_url.as_deref(),
                Some("http://localhost:1455/auth/callback?code=x&state=y")
            );
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args = Args::try_parse_from(["jcode", "login", "--auth-code", "abc123"]).unwrap();
    match args.command {
        Some(Command::Login { auth_code, .. }) => {
            assert_eq!(auth_code.as_deref(), Some("abc123"));
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args = Args::try_parse_from([
        "jcode",
        "login",
        "--complete",
        "--google-access-tier",
        "readonly",
    ])
    .unwrap();
    match args.command {
        Some(Command::Login {
            complete,
            google_access_tier,
            ..
        }) => {
            assert!(complete);
            assert_eq!(google_access_tier, Some(GoogleAccessTierArg::Readonly));
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn account_subcommands_parse() {
    let login =
        Args::try_parse_from(["jcode", "account", "login", "--no-browser"]).expect("account login");
    assert!(matches!(
        login.command,
        Some(Command::Account {
            action: AccountCommand::Login { no_browser: true }
        })
    ));

    let status =
        Args::try_parse_from(["jcode", "account", "status", "--json"]).expect("account status");
    assert!(matches!(
        status.command,
        Some(Command::Account {
            action: AccountCommand::Status { json: true }
        })
    ));

    let manage = Args::try_parse_from(["jcode", "account", "manage"]).expect("account manage");
    assert!(matches!(
        manage.command,
        Some(Command::Account {
            action: AccountCommand::Manage
        })
    ));

    let logout = Args::try_parse_from(["jcode", "account", "logout"]).expect("account logout");
    assert!(matches!(
        logout.command,
        Some(Command::Account {
            action: AccountCommand::Logout
        })
    ));
}

#[test]
fn quiet_global_flag_parses() {
    let args = Args::try_parse_from(["jcode", "--quiet", "model", "list"]).unwrap();
    assert!(args.quiet);
}

#[test]
fn acp_subcommand_parses() {
    let args = Args::try_parse_from(["jcode", "acp"]).unwrap();
    match args.command {
        Some(Command::Acp) => {}
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn run_json_subcommand_parses() {
    let args = Args::try_parse_from(["jcode", "run", "--json", "hello"]).unwrap();
    match args.command {
        Some(Command::Run {
            json,
            ndjson,
            message,
        }) => {
            assert!(json);
            assert!(!ndjson);
            assert_eq!(message, "hello");
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn run_ndjson_subcommand_parses() {
    let args = Args::try_parse_from(["jcode", "run", "--ndjson", "hello"]).unwrap();
    match args.command {
        Some(Command::Run {
            json,
            ndjson,
            message,
        }) => {
            assert!(!json);
            assert!(ndjson);
            assert_eq!(message, "hello");
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn version_subcommand_parses() {
    let args = Args::try_parse_from(["jcode", "version", "--json"]).unwrap();
    match args.command {
        Some(Command::Version { json }) => assert!(json),
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn usage_subcommand_parses() {
    let args = Args::try_parse_from(["jcode", "usage", "--json"]).unwrap();
    match args.command {
        Some(Command::Usage { json }) => assert!(json),
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn auth_status_subcommand_parses() {
    let args = Args::try_parse_from(["jcode", "auth", "status", "--json"]).unwrap();
    match args.command {
        Some(Command::Auth(AuthCommand::Status { json })) => assert!(json),
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn auth_doctor_subcommand_parses() {
    let args = Args::try_parse_from(["jcode", "auth", "doctor", "openai", "--validate", "--json"])
        .unwrap();
    match args.command {
        Some(Command::Auth(AuthCommand::Doctor {
            provider,
            validate,
            json,
        })) => {
            assert_eq!(provider.as_deref(), Some("openai"));
            assert!(validate);
            assert!(json);
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn provider_list_subcommand_parses() {
    let args = Args::try_parse_from(["jcode", "provider", "list", "--json"]).unwrap();
    match args.command {
        Some(Command::Provider(ProviderCommand::List { json })) => assert!(json),
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn provider_current_subcommand_parses() {
    let args = Args::try_parse_from(["jcode", "provider", "current", "--json"]).unwrap();
    match args.command {
        Some(Command::Provider(ProviderCommand::Current { json })) => assert!(json),
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn provider_add_subcommand_parses_agent_friendly_flags() {
    let args = Args::try_parse_from([
        "jcode",
        "provider",
        "add",
        "my-api",
        "--base-url",
        "https://llm.example.com/v1",
        "--model",
        "model-a",
        "--context-window",
        "128000",
        "--api-key-stdin",
        "--auth",
        "bearer",
        "--set-default",
        "--json",
    ])
    .unwrap();

    match args.command {
        Some(Command::Provider(ProviderCommand::Add {
            name,
            base_url,
            model,
            context_window,
            api_key_stdin,
            auth,
            set_default,
            json,
            ..
        })) => {
            assert_eq!(name, "my-api");
            assert_eq!(base_url, "https://llm.example.com/v1");
            assert_eq!(model, "model-a");
            assert_eq!(context_window, Some(128000));
            assert!(api_key_stdin);
            assert_eq!(auth, Some(ProviderAuthArg::Bearer));
            assert!(set_default);
            assert!(json);
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn restart_save_subcommand_parses() {
    let args = Args::try_parse_from(["jcode", "restart", "save"]).unwrap();
    match args.command {
        Some(Command::Restart {
            action: RestartCommand::Save {
                auto_restore: false,
            },
        }) => {}
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn restart_save_auto_restore_flag_parses() {
    let args = Args::try_parse_from(["jcode", "restart", "save", "--auto-restore"]).unwrap();
    match args.command {
        Some(Command::Restart {
            action: RestartCommand::Save { auto_restore: true },
        }) => {}
        other => panic!("unexpected command: {:?}", other),
    }
}

/// Contract test for the onboarding agent-repair brief (see
/// `jcode-tui::tui::app::onboarding_repair::build_repair_brief`). The brief
/// tells a coding agent to run these exact commands to diagnose and fix a
/// failed login. If any flag here stops parsing, the brief would hand the agent
/// a broken command, so this guards the agent-facing CLI contract.
#[test]
fn onboarding_repair_brief_commands_are_valid_cli() {
    // Diagnose.
    Args::try_parse_from(["jcode", "auth-test", "--provider", "openai", "--json"])
        .expect("auth-test --provider --json must parse");
    Args::try_parse_from(["jcode", "auth-test", "--all-configured", "--json"])
        .expect("auth-test --all-configured --json must parse");
    Args::try_parse_from(["jcode", "auth", "doctor"]).expect("auth doctor must parse");

    // Fix: OAuth and API-key logins.
    Args::try_parse_from(["jcode", "login", "--provider", "openai"])
        .expect("login --provider must parse");
    Args::try_parse_from(["jcode", "login", "--provider", "openai", "--api-key", "k"])
        .expect("login --provider --api-key must parse");

    // Fix: custom OpenAI-compatible endpoint via provider add + key on stdin.
    Args::try_parse_from([
        "jcode",
        "provider",
        "add",
        "my-endpoint",
        "--base-url",
        "https://api.example.com/v1",
        "--model",
        "some-model",
        "--api-key-stdin",
    ])
    .expect("provider add --base-url --model --api-key-stdin must parse");
}
