//! Configured server identity name resolution: env/CLI precedence and label
//! normalization.

const SERVER_NAME_ENV: &str = "JCODE_SERVER_NAME";
const SERVER_DISPLAY_NAME_ENV: &str = "JCODE_SERVER_DISPLAY_NAME";
const MAX_CONFIGURED_SERVER_NAME_LEN: usize = 64;

pub(super) fn configured_server_name(cli_name: Option<String>) -> Option<String> {
    cli_name
        .as_deref()
        .and_then(normalize_configured_server_name)
        .or_else(configured_server_name_from_env)
}

fn configured_server_name_from_env() -> Option<String> {
    [SERVER_NAME_ENV, SERVER_DISPLAY_NAME_ENV]
        .into_iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .and_then(|value| normalize_configured_server_name(&value))
        })
}

pub(super) fn normalize_configured_server_name(raw: &str) -> Option<String> {
    let mut normalized = String::new();
    let mut previous_dash = false;

    for ch in raw.trim().chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else if ch == '.' || ch == '-' {
            ch
        } else {
            '-'
        };

        if mapped == '-' {
            if previous_dash {
                continue;
            }
            previous_dash = true;
        } else {
            previous_dash = false;
        }
        normalized.push(mapped);
        if normalized.len() >= MAX_CONFIGURED_SERVER_NAME_LEN {
            break;
        }
    }

    let trimmed = normalized.trim_matches(|ch| matches!(ch, '-' | '.'));
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}
