//! Centralized session "facts" formatting.
//!
//! Several surfaces (info widgets, the overscroll status line, and the compact
//! right-side fact stack) all want to show the same handful of facts: the model,
//! reasoning effort, context usage, the working directory, the provider, and so
//! on. Historically each surface formatted these independently, which led to
//! duplication and inconsistency (raw vs pretty model ids).
//!
//! This module is the single source of truth for compact fact formatting
//! (`pretty_model`, `dir_label`, and related helpers).

/// Render `claude-opus-4-8` as `Opus 4.8`, `gpt-5.5` as `GPT-5.5`, etc. Single
/// source of truth for the human-friendly model name across every compact UI
/// surface. The redundant `Claude ` family prefix is dropped: the provider
/// fact already says Claude/Anthropic, so the model reads as `Fable 5` or
/// `Sonnet 4.5` rather than `Claude Fable 5`.
pub(crate) fn pretty_model(model: &str) -> String {
    let pretty = crate::tui::app::helpers::model_names::pretty_model_display_name(model);
    match pretty.strip_prefix("Claude ") {
        Some(rest) if !rest.trim().is_empty() => rest.to_string(),
        _ => pretty,
    }
}

/// The user's home directory as a display string.
///
/// `HOME` is the POSIX spelling and is unset on Windows, where the profile
/// lives under `USERPROFILE`. Falling back to `dirs` keeps `dir_label`
/// home-relative on every platform instead of printing a full absolute path.
fn home_dir_string() -> Option<String> {
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy().into_owned();
        if !home.trim().is_empty() {
            return Some(home);
        }
    }
    dirs::home_dir().map(|path| path.to_string_lossy().into_owned())
}

/// Whether `path` is `prefix` itself or sits underneath it.
///
/// Compares case-insensitively on Windows, where `C:\Users\Me` and
/// `c:\users\me` are the same directory, and requires a separator after the
/// prefix so `/home/mel` is not treated as being under `/home/me`.
fn path_is_at_or_under(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches(['/', '\\']);
    if prefix.is_empty() {
        return false;
    }
    if path.len() == prefix.len() {
        return path.eq_ignore_ascii_case(prefix) || path == prefix;
    }
    let Some(rest) = path.get(prefix.len()..) else {
        return false;
    };
    let same_prefix = path
        .get(..prefix.len())
        .is_some_and(|head| head == prefix || (cfg!(windows) && head.eq_ignore_ascii_case(prefix)));
    same_prefix && (rest.starts_with('/') || rest.starts_with('\\'))
}

/// The separator a path is written with, so labels echo the style the session
/// reported instead of mixing `\` paths with `/` elision.
fn path_separator(path: &str) -> char {
    if path.contains('\\') && !path.contains('/') {
        '\\'
    } else {
        '/'
    }
}

/// Last non-empty segment of a path, treating both separators as boundaries.
pub(crate) fn last_path_segment(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
}

/// Home-relative directory label, e.g. `/home/me/jcode` -> `~/jcode`. Does not
/// shorten intermediate path segments.
pub(crate) fn dir_label(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return "/".to_string();
    }
    if let Some(home) = home_dir_string()
        && path_is_at_or_under(trimmed, &home)
    {
        let home = home.trim_end_matches(['/', '\\']);
        let rest = &trimmed[home.len()..];
        return if rest.is_empty() {
            "~".to_string()
        } else {
            format!("~{rest}")
        };
    }
    trimmed.to_string()
}

/// Compact home-relative directory label that elides intermediate segments,
/// e.g. `/home/me/a/b/c` -> `…/b/c` and `~/a/b/c` -> `~/…/c`. Used where space
/// is tight (status line, overscroll, compact fact stack).
pub(crate) fn dir_label_short(path: &str) -> Option<String> {
    let trimmed = path.trim().trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return None;
    }
    let display = dir_label(trimmed);
    let sep = path_separator(&display);
    let segs: Vec<&str> = display
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .collect();
    let short = if display.starts_with('~') {
        if segs.len() <= 2 {
            display.clone()
        } else {
            format!("~{sep}…{sep}{}", segs[segs.len() - 1])
        }
    } else if segs.len() <= 2 {
        format!("{sep}{}", segs.join(&sep.to_string()))
    } else {
        format!(
            "…{sep}{}{sep}{}",
            segs[segs.len() - 2],
            segs[segs.len() - 1]
        )
    };
    Some(short)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_model_drops_redundant_claude_prefix() {
        assert_eq!(pretty_model("claude-opus-4-8"), "Opus 4.8");
        assert_eq!(pretty_model("claude-sonnet-4-5"), "Sonnet 4.5");
        assert_eq!(pretty_model("claude-fable-5"), "Fable 5");
        // Non-Claude ids are untouched.
        assert_eq!(pretty_model("gpt-5.5"), "GPT-5.5");
        assert_eq!(pretty_model("gemini-2.5-pro"), "Gemini 2.5 Pro");
    }

    #[test]
    fn dir_label_is_home_relative() {
        // Avoid depending on the real HOME by checking the non-home branch and
        // the trailing-slash normalization.
        assert_eq!(dir_label("/var/log/"), "/var/log");
        assert_eq!(dir_label("/"), "/");
        assert_eq!(dir_label("   "), "/");
    }

    #[test]
    fn dir_label_short_elides_middle_segments() {
        assert_eq!(dir_label_short("/a/b"), Some("/a/b".to_string()));
        assert_eq!(dir_label_short("/a/b/c/d"), Some("…/c/d".to_string()));
        assert_eq!(dir_label_short(""), None);
    }

    /// Windows paths use `\`, and shortening them with a `/`-only split used to
    /// leave the whole absolute path on the status line, which pushed the model
    /// and context facts off it entirely.
    #[test]
    fn dir_label_short_handles_windows_separators() {
        assert_eq!(
            dir_label_short(r"C:\work\jcode\crates\jcode-tui"),
            Some(r"…\crates\jcode-tui".to_string())
        );
        assert_eq!(
            dir_label_short(r"C:\work\jcode\"),
            Some(r"…\work\jcode".to_string()),
            "a trailing backslash must not defeat the elision"
        );
    }

    #[test]
    fn last_path_segment_treats_both_separators_as_boundaries() {
        assert_eq!(last_path_segment("/home/me/crates/jcode-tui"), "jcode-tui");
        assert_eq!(last_path_segment(r"C:\work\jcode-tui"), "jcode-tui");
        assert_eq!(last_path_segment("plain"), "plain");
        assert_eq!(last_path_segment("/home/me/"), "me");
    }

    /// `dir_label` compared against `HOME` only, which is unset on Windows, so
    /// the label was never home-relative there. The prefix check must also not
    /// treat a sibling whose name merely starts with the home name as inside it.
    #[test]
    fn path_is_at_or_under_requires_a_separator_boundary() {
        assert!(path_is_at_or_under("/home/me", "/home/me"));
        assert!(path_is_at_or_under("/home/me/jcode", "/home/me"));
        assert!(path_is_at_or_under("/home/me/jcode", "/home/me/"));
        assert!(!path_is_at_or_under("/home/mel/jcode", "/home/me"));
        assert!(path_is_at_or_under(r"C:\Users\me\jcode", r"C:\Users\me"));
        assert!(!path_is_at_or_under(r"C:\Users\mel\jcode", r"C:\Users\me"));
    }
}
