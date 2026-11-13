//! Human-readable credential source for doctor reports.

use jcode_base::provider_catalog::ResolvedOpenAiCompatibleProfile;

pub(crate) fn loaded_detail(profile: &ResolvedOpenAiCompatibleProfile) -> String {
    if profile.id == "command-code" {
        "Loaded Command Code credential from the environment or active account store".to_string()
    } else {
        format!("Loaded credential from {}", profile.api_key_env)
    }
}
