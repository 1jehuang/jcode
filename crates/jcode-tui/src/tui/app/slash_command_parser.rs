//! Boundary-aware slash-command parsing shared by submission and completion.
//!
//! A slash is only command syntax when it starts a known command token at the
//! beginning of input or after whitespace. This deliberately avoids treating
//! URLs, filesystem paths, prose, quoted text, and code as commands.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SlashCommandMatch {
    pub start: usize,
    pub name_end: usize,
}

pub(super) fn find_known_commands(input: &str, names: &[&str]) -> Vec<SlashCommandMatch> {
    let mut matches = Vec::new();
    scan_slash_tokens(input, |start, end| {
        let token = &input[start..end];
        if names.contains(&token) {
            matches.push(SlashCommandMatch {
                start,
                name_end: end,
            });
        }
    });
    matches
}

/// Return the slash-token span currently being edited, if any.
pub(super) fn active_token_before_cursor(input: &str, cursor: usize) -> Option<(usize, usize)> {
    let cursor = cursor.min(input.len());
    if !input.is_char_boundary(cursor) {
        return None;
    }

    let mut active = None;
    scan_slash_tokens(&input[..cursor], |start, end| {
        if end == cursor {
            active = Some((start, end));
        }
    });
    active
}

fn scan_slash_tokens(input: &str, mut on_token: impl FnMut(usize, usize)) {
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_backticks = false;
    let mut in_fenced_code = false;
    let mut escaped = false;
    let mut line_start = true;

    let mut iter = input.char_indices().peekable();
    while let Some((index, ch)) = iter.next() {
        if in_fenced_code {
            if line_start && ch == '`' && input[index..].starts_with("```") {
                iter.next();
                iter.next();
                in_fenced_code = false;
                line_start = false;
                continue;
            }
            line_start = ch == '\n';
            continue;
        }

        if escaped {
            escaped = false;
            line_start = ch == '\n';
            continue;
        }
        if ch == '\\' {
            escaped = true;
            line_start = false;
            continue;
        }

        if !in_single_quote && !in_double_quote && ch == '`' {
            if line_start && input[index..].starts_with("```") {
                iter.next();
                iter.next();
                in_fenced_code = true;
                line_start = false;
                continue;
            }
            in_backticks = !in_backticks;
            line_start = false;
            continue;
        }
        if in_backticks {
            line_start = ch == '\n';
            continue;
        }
        if !in_double_quote && ch == '\'' {
            in_single_quote = !in_single_quote;
            line_start = false;
            continue;
        }
        if !in_single_quote && ch == '"' {
            in_double_quote = !in_double_quote;
            line_start = false;
            continue;
        }

        if !in_single_quote
            && !in_double_quote
            && ch == '/'
            && (index == 0
                || input[..index]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace))
        {
            let end = input[index..]
                .char_indices()
                .find_map(|(offset, value)| value.is_whitespace().then_some(index + offset))
                .unwrap_or(input.len());
            on_token(index, end);
            line_start = false;
            continue;
        }

        line_start = ch == '\n';
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMANDS: &[&str] = &["/plan", "/review", "/help"];

    #[test]
    fn finds_known_commands_at_start_and_after_whitespace() {
        assert_eq!(
            find_known_commands("test /plan now /review", COMMANDS),
            vec![
                SlashCommandMatch {
                    start: 5,
                    name_end: 10
                },
                SlashCommandMatch {
                    start: 15,
                    name_end: 22
                },
            ]
        );
    }

    #[test]
    fn ignores_urls_paths_unknown_commands_and_code() {
        assert!(find_known_commands("https://example.com /tmp /unknown", COMMANDS).is_empty());
        assert!(find_known_commands("`/plan` ```\n/review\n```", COMMANDS).is_empty());
    }

    #[test]
    fn finds_active_partial_token_for_completion() {
        assert_eq!(active_token_before_cursor("text /pla", 9), Some((5, 9)));
        assert_eq!(active_token_before_cursor("https://pla", 11), None);
    }
}
