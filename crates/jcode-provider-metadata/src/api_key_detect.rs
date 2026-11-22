use crate::{LoginProviderDescriptor, LoginProviderTarget, resolve_login_provider_loose};

/// Result of inspecting a pasted API key to determine its provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiKeyDetection {
    /// Unambiguous vendor prefix matched a single API-key provider.
    Matched(LoginProviderDescriptor),
    /// Shared prefix (such as legacy `sk-`) could belong to multiple providers.
    Ambiguous {
        prefix: &'static str,
        candidates: &'static [&'static str],
    },
    /// Key does not match a known vendor prefix.
    Unknown,
}

/// Ordered prefix rules: more specific prefixes must appear before shorter ones.
const UNAMBIGUOUS_KEY_PREFIXES: &[(&str, &str)] = &[
    ("sk-ant-", "anthropic-api"),
    ("sk-or-", "openrouter"),
    ("sk-proj-", "openai-api"),
    ("sk-svcacct-", "openai-api"),
    ("sk-admin-", "openai-api"),
    ("AIza", "gemini-api"),
    ("xai-", "xai"),
    ("gsk_", "groq"),
    ("hf_", "huggingface"),
    ("fw_", "fireworks"),
    ("pplx-", "perplexity"),
    ("csk-", "cerebras"),
    ("tgp_v1_", "togetherai"),
    ("cpk_", "chutes"),
    ("nvapi-", "nvidia-nim"),
    ("ABSK", "bedrock"),
];

/// Providers that commonly issue generic `sk-` keys without a vendor sub-prefix.
pub const AMBIGUOUS_SK_CANDIDATES: &[&str] =
    &["openai-api", "deepseek", "moonshotai", "kimi", "opencode"];

/// Canonical aliases that open the "Paste an API key" flow directly.
pub fn is_auto_api_key_login_alias(input: &str) -> bool {
    matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "key" | "api-key" | "apikey" | "paste-key" | "paste-api-key" | "auto-key"
    )
}

/// Normalize a pasted API key by trimming whitespace, stripping an optional
/// leading `Bearer ` header prefix, and removing matching surrounding quotes.
pub fn normalize_pasted_api_key(raw: &str) -> String {
    let mut key = raw.trim();
    if let Some(stripped) = key
        .strip_prefix("Bearer ")
        .or_else(|| key.strip_prefix("bearer "))
        .or_else(|| key.strip_prefix("BEARER "))
    {
        key = stripped.trim();
    }
    if key.len() >= 2
        && ((key.starts_with('"') && key.ends_with('"'))
            || (key.starts_with('\'') && key.ends_with('\'')))
    {
        key = key[1..key.len() - 1].trim();
    }
    key.to_string()
}

/// Whether `provider` can be authenticated by saving an API key directly.
pub fn is_api_key_login_provider(provider: LoginProviderDescriptor) -> bool {
    match provider.target {
        LoginProviderTarget::ClaudeApiKey
        | LoginProviderTarget::OpenAiApiKey
        | LoginProviderTarget::OpenRouter
        | LoginProviderTarget::Bedrock
        | LoginProviderTarget::Cursor => true,
        LoginProviderTarget::OpenAiCompatible(profile) => profile.id != "muse-subscription",
        LoginProviderTarget::AutoImport
        | LoginProviderTarget::Jcode
        | LoginProviderTarget::Claude
        | LoginProviderTarget::OpenAi
        | LoginProviderTarget::Azure
        | LoginProviderTarget::GrokBuild
        | LoginProviderTarget::Copilot
        | LoginProviderTarget::Gemini
        | LoginProviderTarget::Antigravity
        | LoginProviderTarget::Google => false,
    }
}

/// Detect the provider for a pasted API key using well-known vendor prefixes.
///
/// Only unmistakable vendor prefixes return [`ApiKeyDetection::Matched`].
/// Shared prefixes like generic `sk-` return [`ApiKeyDetection::Ambiguous`]
/// so the user can confirm in one step rather than saving under the wrong provider.
pub fn detect_api_key_provider(raw_key: &str) -> ApiKeyDetection {
    let key = normalize_pasted_api_key(raw_key);
    if key.is_empty() {
        return ApiKeyDetection::Unknown;
    }

    for (prefix, provider_id) in UNAMBIGUOUS_KEY_PREFIXES {
        if key.starts_with(prefix)
            && key.len() > prefix.len()
            && let Some(provider) = crate::resolve_login_provider(provider_id)
        {
            return ApiKeyDetection::Matched(provider);
        }
    }

    if key.starts_with("sk-") && key.len() > 3 {
        return ApiKeyDetection::Ambiguous {
            prefix: "sk-",
            candidates: AMBIGUOUS_SK_CANDIDATES,
        };
    }

    ApiKeyDetection::Unknown
}

/// Resolve a user's answer to "which provider is this API key for?".
///
/// Accepts either a 1-based number from `candidates` or any provider id, alias,
/// or display name. OAuth-first aliases (`claude`, `openai`, `gemini`) map to
/// their API-key counterparts (`anthropic-api`, `openai-api`, `gemini-api`)
/// since the user is explicitly providing an API key.
pub fn resolve_api_key_provider_input(
    input: &str,
    candidates: &[&str],
) -> Option<LoginProviderDescriptor> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(index) = trimmed.parse::<usize>()
        && let Some(candidate_id) = index.checked_sub(1).and_then(|idx| candidates.get(idx))
        && let Some(provider) = crate::resolve_login_provider(candidate_id)
        && is_api_key_login_provider(provider)
    {
        return Some(provider);
    }

    let mapped = match trimmed.to_ascii_lowercase().as_str() {
        "claude" | "anthropic" => "anthropic-api",
        "openai" | "chatgpt" => "openai-api",
        "gemini" | "google" => "gemini-api",
        _ => trimmed,
    };

    resolve_login_provider_loose(mapped).filter(|provider| is_api_key_login_provider(*provider))
}

/// Format numbered candidate choices for an ambiguous API key prompt.
pub fn format_api_key_candidates(candidates: &[&str]) -> String {
    candidates
        .iter()
        .enumerate()
        .filter_map(|(idx, id)| {
            crate::resolve_login_provider(id)
                .map(|provider| format!("{}. {}", idx + 1, provider.display_name))
        })
        .collect::<Vec<_>>()
        .join("  ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_unambiguous_vendor_prefixes() {
        let cases = [
            ("sk-ant-api03-examplekey123456", "anthropic-api"),
            ("sk-or-v1-examplekey123456", "openrouter"),
            ("sk-proj-examplekey123456", "openai-api"),
            ("sk-svcacct-examplekey123456", "openai-api"),
            ("sk-admin-examplekey123456", "openai-api"),
            ("AIzaSyExampleGeminiKey123456", "gemini-api"),
            ("xai-examplekey123456", "xai"),
            ("gsk_examplekey123456", "groq"),
            ("hf_examplekey123456", "huggingface"),
            ("fw_examplekey123456", "fireworks"),
            ("pplx-examplekey123456", "perplexity"),
            ("csk-examplekey123456", "cerebras"),
            ("tgp_v1_examplekey123456", "togetherai"),
            ("cpk_examplekey123456", "chutes"),
            ("nvapi-examplekey123456", "nvidia-nim"),
            ("ABSKQmV0YVRlc3RLZXk=", "bedrock"),
        ];

        for (key, expected_id) in cases {
            match detect_api_key_provider(key) {
                ApiKeyDetection::Matched(provider) => {
                    assert_eq!(provider.id, expected_id, "key prefix mismatch for {key}");
                    assert!(is_api_key_login_provider(provider));
                }
                other => panic!("expected Matched({expected_id}) for {key}, got {other:?}"),
            }
        }
    }

    #[test]
    fn normalizes_bearer_prefix_and_quotes() {
        assert_eq!(
            normalize_pasted_api_key("  Bearer \"sk-ant-api03-secret\"  "),
            "sk-ant-api03-secret"
        );
        match detect_api_key_provider("Bearer 'sk-or-v1-secret'") {
            ApiKeyDetection::Matched(provider) => assert_eq!(provider.id, "openrouter"),
            other => panic!("expected openrouter match, got {other:?}"),
        }
    }

    #[test]
    fn generic_sk_key_is_ambiguous_with_valid_candidates() {
        match detect_api_key_provider("sk-legacy1234567890") {
            ApiKeyDetection::Ambiguous { prefix, candidates } => {
                assert_eq!(prefix, "sk-");
                assert!(candidates.contains(&"openai-api"));
                assert!(candidates.contains(&"deepseek"));
                for id in candidates {
                    let provider = crate::resolve_login_provider(id)
                        .unwrap_or_else(|| panic!("missing candidate provider {id}"));
                    assert!(is_api_key_login_provider(provider));
                }
            }
            other => panic!("expected Ambiguous for legacy sk-, got {other:?}"),
        }
    }

    #[test]
    fn unknown_key_returns_unknown() {
        assert_eq!(
            detect_api_key_provider("arbitrary-unprefixed-secret-token"),
            ApiKeyDetection::Unknown
        );
        assert_eq!(detect_api_key_provider("   "), ApiKeyDetection::Unknown);
    }

    #[test]
    fn resolves_candidate_number_and_oauth_aliases_to_api_key_providers() {
        assert_eq!(
            resolve_api_key_provider_input("2", AMBIGUOUS_SK_CANDIDATES).map(|p| p.id),
            Some("deepseek")
        );
        assert_eq!(
            resolve_api_key_provider_input("claude", &[]).map(|p| p.id),
            Some("anthropic-api")
        );
        assert_eq!(
            resolve_api_key_provider_input("openai", &[]).map(|p| p.id),
            Some("openai-api")
        );
        assert_eq!(
            resolve_api_key_provider_input("gemini", &[]).map(|p| p.id),
            Some("gemini-api")
        );
        assert_eq!(
            resolve_api_key_provider_input("mistral", &[]).map(|p| p.id),
            Some("mistral")
        );
        assert!(resolve_api_key_provider_input("copilot", &[]).is_none());
    }
}
