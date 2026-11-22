use super::*;

/// Parse an external spawn-hook command line into argv parts.
///
/// Supports basic POSIX-style word splitting: whitespace separates arguments,
/// single and double quotes group words, and backslash escapes the next
/// character (outside single quotes). Errors on empty input, unterminated
/// quotes, and trailing escapes.
pub fn parse_hook_command(raw: &str) -> Result<Vec<String>> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut token_started = false;

    for ch in raw.chars() {
        if escaped {
            current.push(ch);
            token_started = true;
            escaped = false;
            continue;
        }

        if let Some(quote_ch) = quote {
            if ch == quote_ch {
                quote = None;
            } else if ch == '\\' && quote_ch == '"' {
                escaped = true;
            } else {
                current.push(ch);
                token_started = true;
            }
            continue;
        }

        match ch {
            '\\' => {
                escaped = true;
                token_started = true;
            }
            '\'' | '"' => {
                quote = Some(ch);
                token_started = true;
            }
            ch if ch.is_whitespace() => {
                if token_started {
                    parts.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            ch => {
                current.push(ch);
                token_started = true;
            }
        }
    }

    if escaped {
        anyhow::bail!("spawn hook command ends with an escape character");
    }
    if quote.is_some() {
        anyhow::bail!("spawn hook command has an unterminated quote");
    }
    if token_started {
        parts.push(current);
    }
    if parts.is_empty() {
        anyhow::bail!("spawn hook command is empty");
    }

    Ok(parts)
}

/// Expand a leading `~/` in a hook program path to the user's home directory,
/// since the hook is executed directly (no shell) and would otherwise fail.
pub fn expand_home(program: &str) -> PathBuf {
    if let Some(rest) = program.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(program)
}
