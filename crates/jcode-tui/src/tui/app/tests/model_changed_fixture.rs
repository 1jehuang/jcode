pub(super) fn model_changed_event(
    error: Option<&str>,
    reasoning_effort: Option<&str>,
) -> crate::protocol::ServerEvent {
    crate::protocol::ServerEvent::ModelChanged {
route_selection: None,
        context_window: None,
        id: 0,
        model: "gpt-5.6-terra".to_string(),
        provider_name: Some("OpenAI".to_string()),
        error: error.map(str::to_string),
        provider_oauth_email: None,
        provider_oauth_account_label: None,
        resolved_credential: None,
        reasoning_effort: reasoning_effort.map(str::to_string),
    }
}
