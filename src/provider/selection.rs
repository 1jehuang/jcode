use super::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ActiveProvider {
    Claude,
    OpenAI,
    Copilot,
    Antigravity,
    Gemini,
    Cursor,
    OpenRouter,
}

impl MultiProvider {
    pub(super) fn auto_default_provider(
        openai: bool,
        claude: bool,
        copilot: bool,
        antigravity: bool,
        gemini: bool,
        cursor: bool,
        openrouter: bool,
        copilot_premium_zero: bool,
    ) -> ActiveProvider {
        if copilot_premium_zero && copilot {
            ActiveProvider::Copilot
        } else if openai {
            ActiveProvider::OpenAI
        } else if claude {
            ActiveProvider::Claude
        } else if copilot {
            ActiveProvider::Copilot
        } else if antigravity {
            ActiveProvider::Antigravity
        } else if gemini {
            ActiveProvider::Gemini
        } else if cursor {
            ActiveProvider::Cursor
        } else if openrouter {
            ActiveProvider::OpenRouter
        } else {
            ActiveProvider::Claude
        }
    }

    pub(super) fn parse_provider_hint(value: &str) -> Option<ActiveProvider> {
        match value.trim().to_ascii_lowercase().as_str() {
            "claude" | "anthropic" => Some(ActiveProvider::Claude),
            "openai" => Some(ActiveProvider::OpenAI),
            "copilot" => Some(ActiveProvider::Copilot),
            "antigravity" => Some(ActiveProvider::Antigravity),
            "gemini" => Some(ActiveProvider::Gemini),
            "cursor" => Some(ActiveProvider::Cursor),
            "openrouter" => Some(ActiveProvider::OpenRouter),
            _ => None,
        }
    }

    pub(super) fn forced_provider_from_env() -> Option<ActiveProvider> {
        let force = std::env::var("JCODE_FORCE_PROVIDER")
            .ok()
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);
        if !force {
            return None;
        }

        std::env::var("JCODE_ACTIVE_PROVIDER")
            .ok()
            .and_then(|value| Self::parse_provider_hint(&value))
    }

    pub(super) fn provider_label(provider: ActiveProvider) -> &'static str {
        match provider {
            ActiveProvider::Claude => "Anthropic",
            ActiveProvider::OpenAI => "OpenAI",
            ActiveProvider::Copilot => "GitHub Copilot",
            ActiveProvider::Antigravity => "Antigravity",
            ActiveProvider::Gemini => "Gemini",
            ActiveProvider::Cursor => "Cursor",
            ActiveProvider::OpenRouter => "OpenRouter",
        }
    }

    pub(super) fn provider_key(provider: ActiveProvider) -> &'static str {
        match provider {
            ActiveProvider::Claude => "claude",
            ActiveProvider::OpenAI => "openai",
            ActiveProvider::Copilot => "copilot",
            ActiveProvider::Antigravity => "antigravity",
            ActiveProvider::Gemini => "gemini",
            ActiveProvider::Cursor => "cursor",
            ActiveProvider::OpenRouter => "openrouter",
        }
    }

    pub(super) fn set_active_provider(&self, provider: ActiveProvider) {
        *self
            .active
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = provider;
    }
}
