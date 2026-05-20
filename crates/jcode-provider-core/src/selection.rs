use crate::{ModelRoute, normalize_copilot_model_name};
use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActiveProvider {
    Claude,
    OpenAI,
    Copilot,
    Antigravity,
    Gemini,
    Cursor,
    Bedrock,
    OpenRouter,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ProviderAvailability {
    pub openai: bool,
    pub claude: bool,
    pub copilot: bool,
    pub antigravity: bool,
    pub gemini: bool,
    pub cursor: bool,
    pub bedrock: bool,
    pub openrouter: bool,
    pub copilot_premium_zero: bool,
}

impl ProviderAvailability {
    pub fn is_configured(self, provider: ActiveProvider) -> bool {
        match provider {
            ActiveProvider::Claude => self.claude,
            ActiveProvider::OpenAI => self.openai,
            ActiveProvider::Copilot => self.copilot,
            ActiveProvider::Antigravity => self.antigravity,
            ActiveProvider::Gemini => self.gemini,
            ActiveProvider::Cursor => self.cursor,
            ActiveProvider::Bedrock => self.bedrock,
            ActiveProvider::OpenRouter => self.openrouter,
        }
    }
}

pub fn auto_default_provider(availability: ProviderAvailability) -> ActiveProvider {
    if availability.copilot_premium_zero && availability.copilot {
        ActiveProvider::Copilot
    } else if availability.openai {
        ActiveProvider::OpenAI
    } else if availability.claude {
        ActiveProvider::Claude
    } else if availability.copilot {
        ActiveProvider::Copilot
    } else if availability.antigravity {
        ActiveProvider::Antigravity
    } else if availability.gemini {
        ActiveProvider::Gemini
    } else if availability.cursor {
        ActiveProvider::Cursor
    } else if availability.bedrock {
        ActiveProvider::Bedrock
    } else if availability.openrouter {
        ActiveProvider::OpenRouter
    } else {
        ActiveProvider::Claude
    }
}

pub fn parse_provider_hint(value: &str) -> Option<ActiveProvider> {
    match value.trim().to_ascii_lowercase().as_str() {
        "claude" | "anthropic" => Some(ActiveProvider::Claude),
        "openai" => Some(ActiveProvider::OpenAI),
        "copilot" => Some(ActiveProvider::Copilot),
        "antigravity" => Some(ActiveProvider::Antigravity),
        "gemini" => Some(ActiveProvider::Gemini),
        "cursor" => Some(ActiveProvider::Cursor),
        "bedrock" | "aws-bedrock" | "aws_bedrock" => Some(ActiveProvider::Bedrock),
        "openrouter" => Some(ActiveProvider::OpenRouter),
        _ => None,
    }
}

pub fn provider_label(provider: ActiveProvider) -> &'static str {
    match provider {
        ActiveProvider::Claude => "Anthropic",
        ActiveProvider::OpenAI => "OpenAI",
        ActiveProvider::Copilot => "GitHub Copilot",
        ActiveProvider::Antigravity => "Antigravity",
        ActiveProvider::Gemini => "Gemini",
        ActiveProvider::Cursor => "Cursor",
        ActiveProvider::Bedrock => "AWS Bedrock",
        ActiveProvider::OpenRouter => "OpenRouter",
    }
}

pub fn provider_key(provider: ActiveProvider) -> &'static str {
    match provider {
        ActiveProvider::Claude => "claude",
        ActiveProvider::OpenAI => "openai",
        ActiveProvider::Copilot => "copilot",
        ActiveProvider::Antigravity => "antigravity",
        ActiveProvider::Gemini => "gemini",
        ActiveProvider::Cursor => "cursor",
        ActiveProvider::Bedrock => "bedrock",
        ActiveProvider::OpenRouter => "openrouter",
    }
}

pub fn provider_from_model_key(key: &str) -> Option<ActiveProvider> {
    match key {
        "claude" => Some(ActiveProvider::Claude),
        "openai" => Some(ActiveProvider::OpenAI),
        "copilot" => Some(ActiveProvider::Copilot),
        "antigravity" => Some(ActiveProvider::Antigravity),
        "gemini" => Some(ActiveProvider::Gemini),
        "cursor" => Some(ActiveProvider::Cursor),
        "bedrock" => Some(ActiveProvider::Bedrock),
        "openrouter" => Some(ActiveProvider::OpenRouter),
        _ => None,
    }
}

pub fn explicit_model_provider_prefix(model: &str) -> Option<(ActiveProvider, &'static str, &str)> {
    if let Some(rest) = model.strip_prefix("claude:") {
        Some((ActiveProvider::Claude, "claude:", rest))
    } else if let Some(rest) = model.strip_prefix("anthropic:") {
        Some((ActiveProvider::Claude, "anthropic:", rest))
    } else if let Some(rest) = model.strip_prefix("openai:") {
        Some((ActiveProvider::OpenAI, "openai:", rest))
    } else if let Some(rest) = model.strip_prefix("copilot:") {
        Some((ActiveProvider::Copilot, "copilot:", rest))
    } else if let Some(rest) = model.strip_prefix("antigravity:") {
        Some((ActiveProvider::Antigravity, "antigravity:", rest))
    } else if let Some(rest) = model.strip_prefix("gemini:") {
        Some((ActiveProvider::Gemini, "gemini:", rest))
    } else if let Some(rest) = model.strip_prefix("cursor:") {
        Some((ActiveProvider::Cursor, "cursor:", rest))
    } else if let Some(rest) = model.strip_prefix("bedrock:") {
        Some((ActiveProvider::Bedrock, "bedrock:", rest))
    } else if let Some(rest) = model.strip_prefix("openrouter:") {
        Some((ActiveProvider::OpenRouter, "openrouter:", rest))
    } else {
        None
    }
}

pub fn model_name_for_provider(provider: ActiveProvider, model: &str) -> Cow<'_, str> {
    if matches!(provider, ActiveProvider::Claude)
        && let Some(canonical) = normalize_copilot_model_name(model)
    {
        return Cow::Borrowed(canonical);
    }
    Cow::Borrowed(model)
}

pub fn dedupe_model_routes(routes: Vec<ModelRoute>) -> Vec<ModelRoute> {
    let mut deduped: Vec<ModelRoute> = Vec::with_capacity(routes.len());

    for route in routes {
        if let Some(existing_idx) = deduped
            .iter()
            .position(|existing| duplicate_model_route(existing, &route))
        {
            if should_replace_duplicate_route(&deduped[existing_idx], &route) {
                deduped[existing_idx] = route;
            }
            continue;
        }

        deduped.push(route);
    }

    deduped
}

fn duplicate_model_route(existing: &ModelRoute, candidate: &ModelRoute) -> bool {
    existing.provider == candidate.provider
        && existing.model == candidate.model
        && duplicate_route_api_method(&existing.api_method, &candidate.api_method)
}

fn duplicate_route_api_method(existing: &str, candidate: &str) -> bool {
    existing == candidate
        || (is_generic_openai_compatible_route(existing)
            && is_profile_openai_compatible_route(candidate))
        || (is_profile_openai_compatible_route(existing)
            && is_generic_openai_compatible_route(candidate))
}

fn is_generic_openai_compatible_route(api_method: &str) -> bool {
    api_method == "openai-compatible"
}

fn is_profile_openai_compatible_route(api_method: &str) -> bool {
    api_method.starts_with("openai-compatible:")
}

fn should_replace_duplicate_route(existing: &ModelRoute, candidate: &ModelRoute) -> bool {
    // A direct OpenAI-compatible provider can briefly appear twice in merged
    // catalogs: once as the generic transport and once as the named profile
    // transport. Keep the profile-scoped route so selection writes
    // `profile:model` rather than falling back to ambiguous generic routing.
    let existing_profile_scoped = is_profile_openai_compatible_route(&existing.api_method);
    let candidate_profile_scoped = is_profile_openai_compatible_route(&candidate.api_method);
    !existing_profile_scoped && candidate_profile_scoped
}

pub fn fallback_sequence(active: ActiveProvider) -> Vec<ActiveProvider> {
    match active {
        ActiveProvider::Claude => vec![
            ActiveProvider::Claude,
            ActiveProvider::OpenAI,
            ActiveProvider::Copilot,
            ActiveProvider::Gemini,
            ActiveProvider::Cursor,
            ActiveProvider::Bedrock,
            ActiveProvider::OpenRouter,
        ],
        ActiveProvider::OpenAI => vec![
            ActiveProvider::OpenAI,
            ActiveProvider::Claude,
            ActiveProvider::Copilot,
            ActiveProvider::Gemini,
            ActiveProvider::Cursor,
            ActiveProvider::Bedrock,
            ActiveProvider::OpenRouter,
        ],
        ActiveProvider::Copilot => vec![
            ActiveProvider::Copilot,
            ActiveProvider::Claude,
            ActiveProvider::OpenAI,
            ActiveProvider::Antigravity,
            ActiveProvider::Gemini,
            ActiveProvider::Cursor,
            ActiveProvider::Bedrock,
            ActiveProvider::OpenRouter,
        ],
        ActiveProvider::Antigravity => vec![
            ActiveProvider::Antigravity,
            ActiveProvider::Claude,
            ActiveProvider::OpenAI,
            ActiveProvider::Copilot,
            ActiveProvider::Gemini,
            ActiveProvider::Cursor,
            ActiveProvider::Bedrock,
            ActiveProvider::OpenRouter,
        ],
        ActiveProvider::Gemini => vec![
            ActiveProvider::Gemini,
            ActiveProvider::Claude,
            ActiveProvider::OpenAI,
            ActiveProvider::Antigravity,
            ActiveProvider::Copilot,
            ActiveProvider::Cursor,
            ActiveProvider::Bedrock,
            ActiveProvider::OpenRouter,
        ],
        ActiveProvider::Cursor => vec![
            ActiveProvider::Cursor,
            ActiveProvider::Claude,
            ActiveProvider::OpenAI,
            ActiveProvider::Copilot,
            ActiveProvider::Antigravity,
            ActiveProvider::Gemini,
            ActiveProvider::OpenRouter,
        ],
        ActiveProvider::Bedrock => vec![
            ActiveProvider::Bedrock,
            ActiveProvider::Claude,
            ActiveProvider::OpenAI,
            ActiveProvider::Copilot,
            ActiveProvider::Antigravity,
            ActiveProvider::Gemini,
            ActiveProvider::Cursor,
            ActiveProvider::OpenRouter,
        ],
        ActiveProvider::OpenRouter => vec![
            ActiveProvider::OpenRouter,
            ActiveProvider::Claude,
            ActiveProvider::OpenAI,
            ActiveProvider::Copilot,
            ActiveProvider::Antigravity,
            ActiveProvider::Gemini,
            ActiveProvider::Cursor,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_provider_hints() {
        assert_eq!(
            parse_provider_hint("Anthropic"),
            Some(ActiveProvider::Claude)
        );
        assert_eq!(parse_provider_hint("openai"), Some(ActiveProvider::OpenAI));
        assert_eq!(parse_provider_hint("unknown"), None);
    }

    #[test]
    fn parses_model_provider_prefixes() {
        assert_eq!(
            provider_from_model_key("gemini"),
            Some(ActiveProvider::Gemini)
        );
        assert_eq!(provider_from_model_key("missing"), None);

        for (raw, expected_provider, expected_prefix, expected_model) in [
            ("claude:sonnet", ActiveProvider::Claude, "claude:", "sonnet"),
            (
                "anthropic:sonnet",
                ActiveProvider::Claude,
                "anthropic:",
                "sonnet",
            ),
            ("openai:gpt-5", ActiveProvider::OpenAI, "openai:", "gpt-5"),
            (
                "copilot:gpt-5",
                ActiveProvider::Copilot,
                "copilot:",
                "gpt-5",
            ),
            (
                "antigravity:default",
                ActiveProvider::Antigravity,
                "antigravity:",
                "default",
            ),
            (
                "gemini:gemini-2.5-pro",
                ActiveProvider::Gemini,
                "gemini:",
                "gemini-2.5-pro",
            ),
            (
                "cursor:composer-1.5",
                ActiveProvider::Cursor,
                "cursor:",
                "composer-1.5",
            ),
            (
                "bedrock:anthropic.claude",
                ActiveProvider::Bedrock,
                "bedrock:",
                "anthropic.claude",
            ),
            (
                "openrouter:meta/llama",
                ActiveProvider::OpenRouter,
                "openrouter:",
                "meta/llama",
            ),
        ] {
            let (provider, prefix, model) = explicit_model_provider_prefix(raw).unwrap();
            assert_eq!(provider, expected_provider, "{raw}");
            assert_eq!(prefix, expected_prefix, "{raw}");
            assert_eq!(model, expected_model, "{raw}");
        }
        assert_eq!(explicit_model_provider_prefix("unknown:sonnet"), None);
    }

    #[test]
    fn dedupes_model_routes_by_route_identity() {
        let routes = vec![
            ModelRoute {
                model: "m".to_string(),
                provider: "p".to_string(),
                api_method: "a".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            },
            ModelRoute {
                model: "m".to_string(),
                provider: "p".to_string(),
                api_method: "a".to_string(),
                available: false,
                detail: "duplicate".to_string(),
                cheapness: None,
            },
            ModelRoute {
                model: "m".to_string(),
                provider: "p".to_string(),
                api_method: "b".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            },
        ];

        let deduped = dedupe_model_routes(routes);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].detail, "");
    }

    #[test]
    fn dedupes_openai_compatible_generic_and_profile_aliases() {
        let routes = vec![
            ModelRoute {
                model: "qwen".to_string(),
                provider: "Cerebras".to_string(),
                api_method: "openai-compatible".to_string(),
                available: true,
                detail: "generic transport".to_string(),
                cheapness: None,
            },
            ModelRoute {
                model: "qwen".to_string(),
                provider: "Cerebras".to_string(),
                api_method: "openai-compatible:cerebras".to_string(),
                available: true,
                detail: "profile transport".to_string(),
                cheapness: None,
            },
            ModelRoute {
                model: "qwen".to_string(),
                provider: "OtherDirect".to_string(),
                api_method: "openai-compatible:other".to_string(),
                available: true,
                detail: "different provider".to_string(),
                cheapness: None,
            },
            ModelRoute {
                model: "qwen".to_string(),
                provider: "Cerebras".to_string(),
                api_method: "openai-compatible:cerebras-alt".to_string(),
                available: true,
                detail: "distinct profile route".to_string(),
                cheapness: None,
            },
        ];

        let deduped = dedupe_model_routes(routes);
        assert_eq!(deduped.len(), 3);
        let cerebras = deduped
            .iter()
            .find(|route| route.provider == "Cerebras")
            .expect("Cerebras route remains");
        assert_eq!(cerebras.api_method, "openai-compatible:cerebras");
        assert_eq!(cerebras.detail, "profile transport");
        assert!(deduped.iter().any(|route| {
            route.provider == "Cerebras" && route.api_method == "openai-compatible:cerebras-alt"
        }));
    }

    #[test]
    fn auto_default_prefers_copilot_zero_mode() {
        let provider = auto_default_provider(ProviderAvailability {
            openai: true,
            copilot: true,
            copilot_premium_zero: true,
            ..ProviderAvailability::default()
        });
        assert_eq!(provider, ActiveProvider::Copilot);
    }

    #[test]
    fn fallback_sequence_keeps_active_first() {
        let sequence = fallback_sequence(ActiveProvider::OpenRouter);
        assert_eq!(sequence.first(), Some(&ActiveProvider::OpenRouter));
        assert!(sequence.contains(&ActiveProvider::Claude));
        assert!(sequence.contains(&ActiveProvider::Cursor));
    }
}
