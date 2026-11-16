use super::*;

#[test]
fn derive_session_provider_key_prefers_runtime_identity_over_transport() {
    let _lock = lock_env();
    let _runtime = EnvVarGuard::set("JCODE_RUNTIME_PROVIDER", "azure-openai");
    let _namespace = EnvVarGuard::set("JCODE_OPENROUTER_CACHE_NAMESPACE", "azure-cache");
    let _active = EnvVarGuard::set("JCODE_ACTIVE_PROVIDER", "openrouter");

    assert_eq!(
        derive_session_provider_key("openrouter").as_deref(),
        Some("azure-openai")
    );
}

#[test]
fn derive_session_provider_key_falls_back_to_openrouter_namespace() {
    let _lock = lock_env();
    let _runtime = EnvVarGuard::remove("JCODE_RUNTIME_PROVIDER");
    let _namespace = EnvVarGuard::set("JCODE_OPENROUTER_CACHE_NAMESPACE", "azure-openai");
    let _active = EnvVarGuard::set("JCODE_ACTIVE_PROVIDER", "openrouter");

    assert_eq!(
        derive_session_provider_key("openrouter").as_deref(),
        Some("azure-openai")
    );
}

#[test]
fn derive_session_provider_key_keeps_openai_compatible_profile_namespace() {
    let _lock = lock_env();
    let _runtime = EnvVarGuard::set("JCODE_RUNTIME_PROVIDER", "openai-compatible");
    let _namespace = EnvVarGuard::set("JCODE_OPENROUTER_CACHE_NAMESPACE", "zai");
    let _active = EnvVarGuard::set("JCODE_ACTIVE_PROVIDER", "openrouter");

    assert_eq!(
        derive_session_provider_key("openrouter").as_deref(),
        Some("zai")
    );
}

#[test]
fn save_label_becomes_the_session_title() {
    let mut session = Session::create_with_id(
        "session_save_label_123".to_string(),
        None,
        Some("Generated title".to_string()),
    );
    session.mark_saved(None);
    assert_eq!(session.display_title(), Some("Generated title"));

    session.mark_saved(Some("  yc mcp  ".to_string()));
    assert_eq!(session.save_label.as_deref(), Some("yc mcp"));
    assert_eq!(session.custom_title.as_deref(), Some("yc mcp"));
    assert_eq!(session.display_title(), Some("yc mcp"));

    // Legacy bookmarks saved a label without setting the title.
    session.custom_title = None;
    assert_eq!(session.display_title(), Some("yc mcp"));
    session.unmark_saved();
    assert_eq!(session.display_title(), Some("Generated title"));
}

#[test]
fn rename_title_preserves_generated_title_for_clear() {
    let mut session = Session::create_with_id(
        "session_rename_clear_123".to_string(),
        None,
        Some("Generated first prompt title".to_string()),
    );

    assert_eq!(
        session.display_title(),
        Some("Generated first prompt title")
    );
    session.rename_title(Some("Custom planning name".to_string()));
    assert_eq!(
        session.title.as_deref(),
        Some("Generated first prompt title")
    );
    assert_eq!(
        session.custom_title.as_deref(),
        Some("Custom planning name")
    );
    assert_eq!(session.display_title(), Some("Custom planning name"));

    session.rename_title(None);
    assert_eq!(
        session.title.as_deref(),
        Some("Generated first prompt title")
    );
    assert!(session.custom_title.is_none());
    assert_eq!(
        session.display_title(),
        Some("Generated first prompt title")
    );

    session.custom_title = Some("   ".to_string());
    assert_eq!(
        session.display_title(),
        Some("Generated first prompt title")
    );
}
