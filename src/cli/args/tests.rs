use super::*;
use crate::cli::provider_init::ProviderChoice;

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
fn model_list_subcommand_parses() {
    let args = Args::try_parse_from(["jcode", "model", "list", "--json", "--verbose"]).unwrap();
    match args.command {
        Some(Command::Model(ModelCommand::List { json, verbose })) => {
            assert!(json);
            assert!(verbose);
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
fn login_no_browser_flag_parses() {
    let args = Args::try_parse_from(["jcode", "login", "--no-browser"]).unwrap();
    match args.command {
        Some(Command::Login {
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
        }) => {
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
fn quiet_global_flag_parses() {
    let args = Args::try_parse_from(["jcode", "--quiet", "model", "list"]).unwrap();
    assert!(args.quiet);
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
fn skills_scope_subcommands_parse() {
    let args = Args::try_parse_from([
        "jcode",
        "skills",
        "scope",
        "set",
        "llmwiki-memory",
        "--state",
        "discoverable",
        "--reason",
        "only on memory tasks",
        "--cwd",
        "/tmp/project",
        "--json",
    ])
    .unwrap();

    match args.command {
        Some(Command::Skills(SkillCommand::Scope {
            command:
                SkillScopeCommand::Set {
                    name,
                    state,
                    reason,
                    cwd,
                    json,
                },
        })) => {
            assert_eq!(name, "llmwiki-memory");
            assert_eq!(state, SkillScopeStateArg::Discoverable);
            assert_eq!(reason.as_deref(), Some("only on memory tasks"));
            assert_eq!(cwd.as_deref(), Some("/tmp/project"));
            assert!(json);
        }
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn skills_import_validate_match_and_bridge_parse() {
    let args = Args::try_parse_from([
        "jcode",
        "skills",
        "import",
        "--from",
        ".claude/skills",
        "--scope",
        "global",
        "--apply",
        "--force",
        "--json",
    ])
    .unwrap();
    match args.command {
        Some(Command::Skills(SkillCommand::Import {
            from,
            scope,
            apply,
            force,
            json,
            ..
        })) => {
            assert_eq!(from, vec![std::path::PathBuf::from(".claude/skills")]);
            assert_eq!(scope, SkillImportScopeArg::Global);
            assert!(apply);
            assert!(force);
            assert!(json);
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args =
        Args::try_parse_from(["jcode", "skills", "validate", "--cwd", ".", "--json"]).unwrap();
    match args.command {
        Some(Command::Skills(SkillCommand::Validate { cwd, json })) => {
            assert_eq!(cwd.as_deref(), Some("."));
            assert!(json);
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args = Args::try_parse_from([
        "jcode",
        "skills",
        "match",
        "fix this bug",
        "--skills",
        "always",
        "--skill",
        "custom",
        "--json",
    ])
    .unwrap();
    match args.command {
        Some(Command::Skills(SkillCommand::Match {
            goal,
            skills,
            skill,
            json,
            ..
        })) => {
            assert_eq!(goal, "fix this bug");
            assert_eq!(skills, SkillModeArg::Always);
            assert_eq!(skill, vec!["custom"]);
            assert!(json);
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args = Args::try_parse_from(["jcode", "skills", "llmwiki-bridge", "--json"]).unwrap();
    match args.command {
        Some(Command::Skills(SkillCommand::LlmwikiBridge { json })) => assert!(json),
        other => panic!("unexpected command: {:?}", other),
    }
}

#[test]
fn events_path_tail_and_export_parse() {
    let args = Args::try_parse_from(["jcode", "events", "list", "--json"]).unwrap();
    match args.command {
        Some(Command::Events(EventCommand::List { json })) => assert!(json),
        other => panic!("unexpected command: {:?}", other),
    }

    let args =
        Args::try_parse_from(["jcode", "events", "show", "--run", "run_123", "--json"]).unwrap();
    match args.command {
        Some(Command::Events(EventCommand::Show { run, json })) => {
            assert_eq!(run, "run_123");
            assert!(json);
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args = Args::try_parse_from([
        "jcode",
        "events",
        "replay",
        "--run",
        "run_123",
        "--json",
        "--output",
        "replay.json",
    ])
    .unwrap();
    match args.command {
        Some(Command::Events(EventCommand::Replay { run, json, output })) => {
            assert_eq!(run, "run_123");
            assert!(json);
            assert_eq!(output, Some(std::path::PathBuf::from("replay.json")));
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args =
        Args::try_parse_from(["jcode", "events", "path", "--run", "run_123", "--json"]).unwrap();
    match args.command {
        Some(Command::Events(EventCommand::Path { run, json })) => {
            assert_eq!(run, "run_123");
            assert!(json);
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args = Args::try_parse_from([
        "jcode", "events", "tail", "--run", "run_123", "--lines", "5", "--ndjson",
    ])
    .unwrap();
    match args.command {
        Some(Command::Events(EventCommand::Tail { run, lines, ndjson })) => {
            assert_eq!(run, "run_123");
            assert_eq!(lines, 5);
            assert!(ndjson);
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args = Args::try_parse_from([
        "jcode",
        "events",
        "export",
        "--run",
        "run_123",
        "--output",
        "run.ndjson",
        "--json",
    ])
    .unwrap();
    match args.command {
        Some(Command::Events(EventCommand::Export { run, output, json })) => {
            assert_eq!(run, "run_123");
            assert_eq!(output, Some(std::path::PathBuf::from("run.ndjson")));
            assert!(json);
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args = Args::try_parse_from([
        "jcode",
        "events",
        "sse",
        "--run",
        "run_123",
        "--last-event-id",
        "hevt_1",
        "--retry-ms",
        "1500",
        "--output",
        "run.sse",
    ])
    .unwrap();
    match args.command {
        Some(Command::Events(EventCommand::Sse {
            run,
            last_event_id,
            retry_ms,
            output,
        })) => {
            assert_eq!(run, "run_123");
            assert_eq!(last_event_id.as_deref(), Some("hevt_1"));
            assert_eq!(retry_ms, 1500);
            assert_eq!(output, Some(std::path::PathBuf::from("run.sse")));
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args = Args::try_parse_from([
        "jcode",
        "events",
        "prune",
        "--keep-logs",
        "3",
        "--max-total-bytes",
        "4096",
        "--apply",
        "--json",
    ])
    .unwrap();
    match args.command {
        Some(Command::Events(EventCommand::Prune {
            keep_logs,
            max_total_bytes,
            apply,
            json,
        })) => {
            assert_eq!(keep_logs, Some(3));
            assert_eq!(max_total_bytes, Some(4096));
            assert!(apply);
            assert!(json);
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args =
        Args::try_parse_from(["jcode", "events", "bench", "--events", "2500", "--json"]).unwrap();
    match args.command {
        Some(Command::Events(EventCommand::Bench { events, json })) => {
            assert_eq!(events, 2500);
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
