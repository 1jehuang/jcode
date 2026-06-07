use super::parse_diff_mode_name;
use super::parse_manual_subagent_spec;

#[test]
fn parse_diff_mode_name_maps_known_aliases() {
    use crate::config::DiffDisplayMode;
    assert_eq!(parse_diff_mode_name("off"), Some(DiffDisplayMode::Off));
    assert_eq!(parse_diff_mode_name("none"), Some(DiffDisplayMode::Off));
    assert_eq!(
        parse_diff_mode_name("inline"),
        Some(DiffDisplayMode::Inline)
    );
    assert_eq!(parse_diff_mode_name("on"), Some(DiffDisplayMode::Inline));
    assert_eq!(
        parse_diff_mode_name("full"),
        Some(DiffDisplayMode::FullInline)
    );
    assert_eq!(
        parse_diff_mode_name("pinned"),
        Some(DiffDisplayMode::Pinned)
    );
    assert_eq!(parse_diff_mode_name("file"), Some(DiffDisplayMode::File));
}

#[test]
fn parse_diff_mode_name_is_case_insensitive_and_trims() {
    use crate::config::DiffDisplayMode;
    assert_eq!(
        parse_diff_mode_name("  PINNED "),
        Some(DiffDisplayMode::Pinned)
    );
}

#[test]
fn parse_diff_mode_name_rejects_unknown() {
    assert_eq!(parse_diff_mode_name("sidebyside"), None);
    assert_eq!(parse_diff_mode_name(""), None);
}

#[test]
fn parse_manual_subagent_spec_accepts_flags_and_prompt() {
    let spec = parse_manual_subagent_spec(
        "--type research --model gpt-5.4 --continue session_123 investigate this bug",
    )
    .expect("parse manual subagent spec");

    assert_eq!(spec.subagent_type, "research");
    assert_eq!(spec.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(spec.session_id.as_deref(), Some("session_123"));
    assert_eq!(spec.prompt, "investigate this bug");
}

#[test]
fn parse_manual_subagent_spec_rejects_missing_prompt() {
    let err = parse_manual_subagent_spec("--model gpt-5.4")
        .expect_err("missing prompt should be rejected");
    assert!(err.contains("Missing prompt"));
}

#[test]
fn openrouter_402_payment_required_is_non_retryable() {
    use super::is_non_retryable_auto_poke_error;
    let err = "OpenAI-compatible chat request failed\n  endpoint: \
        https://openrouter.ai/api/v1/chat/completions\n  model: openai/gpt-5.4\n  \
        auth: OPENROUTER_API_KEY\n  status: 402 Payment Required\n  response: \
        {\"error\":{\"message\":\"This request requires more credits, or fewer max_tokens. \
        You requested up to 65536 tokens, but can only afford 34424. To increase, visit \
        https://openrouter.ai/settings/credits and add more credits\",\"code\":402}}";
    assert!(is_non_retryable_auto_poke_error(err));
}

#[test]
fn transient_server_error_remains_retryable_for_auto_poke() {
    use super::is_non_retryable_auto_poke_error;
    let err = "OpenAI-compatible chat request failed\n  status: 503 Service Unavailable";
    assert!(!is_non_retryable_auto_poke_error(err));
}
