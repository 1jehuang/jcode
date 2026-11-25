#[test]
fn login_no_browser_flag_parses() {
    let args =
        super::claude_login_tests::parse_login_args(&["jcode", "login", "--no-browser"]).unwrap();
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
            google,
            api_base,
            api_key,
            api_key_env,
            no_validate,
            flow_id,
            cancel,
            device_code,
            region,
            claude_code,
            oauth,
        }) => {
            assert!(region.is_none());
            assert!(provider.is_none());
            assert!(account.is_none());
            assert!(no_browser);
            assert!(!claude_code);
            assert!(!oauth);
            assert!(!device_code);
            assert!(!print_auth_url);
            assert!(callback_url.is_none());
            assert!(auth_code.is_none());
            assert!(!json);
            assert!(!complete);
            assert!(google.google_access_tier.is_none());
            assert!(google.google_services.is_none());
            assert!(!google.setup);
            assert!(google.google_client_json.is_none());
            assert!(api_base.is_none());
            assert!(api_key.is_none());
            assert!(api_key_env.is_none());
            assert!(!no_validate);
            assert!(flow_id.is_none());
            assert!(!cancel);
        }
        other => panic!("unexpected command: {:?}", other),
    }

    let args =
        super::claude_login_tests::parse_login_args(&["jcode", "login", "--headless"]).unwrap();
    match args.command {
        Some(Command::Login { no_browser, .. }) => assert!(no_browser),
        other => panic!("unexpected command: {:?}", other),
    }
}
