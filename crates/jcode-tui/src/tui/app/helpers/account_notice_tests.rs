use super::*;

#[cfg(test)]
mod switched_model_account_suffix_tests {
    use super::switched_model_account_suffix;

    #[test]
    fn email_is_masked() {
        assert_eq!(
            switched_model_account_suffix(Some("OAuth"), Some("jeremy@gmail.com")),
            " (via OAuth · j***y@gmail.com)"
        );
        assert_eq!(
            switched_model_account_suffix(Some("OAuth"), Some("me@example.com")),
            " (via OAuth · m*@example.com)"
        );
    }

    #[test]
    fn no_email_keeps_existing_suffix() {
        assert_eq!(
            switched_model_account_suffix(Some("API key"), None),
            " (via API key)"
        );
        assert_eq!(switched_model_account_suffix(None, Some("  ")), "");
    }
}

#[cfg(test)]
mod account_switch_notice_tests {
    #[test]
    fn notice_states_that_other_running_sessions_are_unchanged() {
        let notice = super::account_switch_notice("OpenAI", "openai-fox");
        assert!(notice.contains("`openai-fox`"));
        assert!(notice.contains("this session and new sessions"));
        assert!(notice.contains("Other running sessions keep their current account"));
    }
}
