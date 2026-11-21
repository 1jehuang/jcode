use super::*;

#[test]
fn tsubasa_models_have_bounded_context_without_metadata_leaking_to_other_profiles() {
    let profile = openai_compatible_profile_by_id("tsubasa").expect("Tsubasa profile");
    assert_eq!(
        openai_compatible_profile_static_models(profile),
        ["tsubasa-pro", "tsubasa-fast"]
    );
    for model in ["tsubasa-pro", "tsubasa-fast"] {
        assert_eq!(
            openai_compatible_profile_context_limit("tsubasa", model),
            Some(32_768)
        );
        assert_eq!(
            openai_compatible_profile_context_limit("openrouter", model),
            None
        );
    }
    assert_eq!(
        openai_compatible_profile_context_limit("tsubasa", "tsubasa-unknown"),
        None
    );
}
