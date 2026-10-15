use super::*;

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
