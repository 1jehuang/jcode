use super::*;

pub(crate) async fn run_login_command(
    command: super::super::args::Command,
    default_provider: ProviderChoice,
    model: Option<String>,
) -> Result<()> {
    let super::super::args::Command::Login {
        provider: login_provider,
        account,
        no_browser,
        claude_code,
        oauth,
        print_auth_url,
        callback_url,
        auth_code,
        json,
        complete,
        flow_id,
        cancel,
        no_validate,
        device_code,
        google,
        api_base,
        api_key,
        api_key_env,
        region,
    } = command
    else {
        unreachable!("login command dispatch only accepts Login")
    };

    let login_choice = login_provider.unwrap_or(default_provider);
    let mut options = LoginOptions {
        no_browser,
        claude_code,
        print_auth_url,
        callback_url,
        auth_code,
        json,
        complete,
        flow_id,
        cancel,
        no_validate,
        device_code,
        openai_compatible_api_base: api_base,
        openai_compatible_api_key: api_key,
        openai_compatible_api_key_env: api_key_env,
        openai_compatible_default_model: model,
        region,
        ..Default::default()
    };
    google.apply(&mut options)?;
    options.claude_code = options.claude_code
        || prefer_cli_login(
            &login_choice,
            account.as_deref(),
            &options,
            oauth,
            crate::external_auth::can_prompt_for_external_auth(),
            auth::browser_suppressed(options.no_browser),
        )?;
    run_login(&login_choice, account.as_deref(), options).await?;
    Ok(())
}

fn prefer_cli_login(
    choice: &ProviderChoice,
    account: Option<&str>,
    options: &LoginOptions,
    oauth: bool,
    interactive: bool,
    browserless: bool,
) -> Result<bool> {
    Ok(!oauth
        && !browserless
        && matches!(choice, ProviderChoice::Claude)
        && account.is_none()
        && !options.no_browser
        && !options.json
        && !options.uses_scriptable_flow()?
        && interactive
        && auth::claude::prefer_claude_code_cli_login())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn claude_cli_default_only_applies_to_interactive_unscoped_browser_login() {
        let _guard = crate::storage::lock_test_env();
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                match &self.0 {
                    Some(v) => crate::env::set_var("JCODE_CLAUDE_LOGIN_METHOD", v),
                    None => crate::env::remove_var("JCODE_CLAUDE_LOGIN_METHOD"),
                }
            }
        }
        let _restore = Restore(std::env::var_os("JCODE_CLAUDE_LOGIN_METHOD"));
        crate::env::set_var("JCODE_CLAUDE_LOGIN_METHOD", "cli");
        let options = LoginOptions::default();
        assert!(
            prefer_cli_login(&ProviderChoice::Claude, None, &options, false, true, false).unwrap()
        );
        assert!(
            !prefer_cli_login(&ProviderChoice::Openai, None, &options, false, true, false).unwrap()
        );
        assert!(
            !prefer_cli_login(
                &ProviderChoice::Claude,
                Some("work"),
                &options,
                false,
                true,
                false
            )
            .unwrap()
        );
        assert!(
            !prefer_cli_login(&ProviderChoice::Claude, None, &options, true, true, false).unwrap()
        );
        assert!(
            !prefer_cli_login(&ProviderChoice::Claude, None, &options, false, false, false)
                .unwrap()
        );
        for options in [
            LoginOptions {
                no_browser: true,
                ..Default::default()
            },
            LoginOptions {
                json: true,
                ..Default::default()
            },
            LoginOptions {
                print_auth_url: true,
                ..Default::default()
            },
            LoginOptions {
                complete: true,
                ..Default::default()
            },
            LoginOptions {
                callback_url: Some("callback".into()),
                ..Default::default()
            },
            LoginOptions {
                auth_code: Some("code".into()),
                ..Default::default()
            },
            LoginOptions {
                flow_id: Some("flow".into()),
                ..Default::default()
            },
            LoginOptions {
                cancel: true,
                ..Default::default()
            },
            LoginOptions {
                device_code: true,
                ..Default::default()
            },
        ] {
            assert!(
                !prefer_cli_login(&ProviderChoice::Claude, None, &options, false, true, false)
                    .unwrap()
            );
        }
        crate::env::set_var("JCODE_CLAUDE_LOGIN_METHOD", "oauth");
        assert!(
            !prefer_cli_login(
                &ProviderChoice::Claude,
                None,
                &LoginOptions::default(),
                false,
                true,
                false
            )
            .unwrap()
        );
        crate::env::set_var("JCODE_CLAUDE_LOGIN_METHOD", "cli");
        for key in ["NO_BROWSER", "JCODE_NO_BROWSER"] {
            let previous = std::env::var_os(key);
            crate::env::set_var(key, "1");
            let browserless = auth::browser_suppressed(false);
            match previous {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
            assert!(browserless);
            assert!(
                !prefer_cli_login(
                    &ProviderChoice::Claude,
                    None,
                    &options,
                    false,
                    true,
                    browserless
                )
                .unwrap()
            );
        }
        assert!(
            !prefer_cli_login(&ProviderChoice::Claude, None, &options, false, true, true).unwrap()
        );
    }
}
