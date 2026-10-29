use super::*;

pub(super) fn select_promoted_default(current: &str, catalog: &[&str]) -> Option<String> {
    if current != DEFAULT_MODEL {
        return None;
    }
    jcode_base::auth::lifecycle::preferred_model_for_provider("openrouter", catalog)
        .filter(|best| best != DEFAULT_MODEL)
}
