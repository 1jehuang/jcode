//! Human-friendly model name rendering.
//!
//! Model ids arrive as raw provider slugs (`claude-opus-4-8`, `gpt-5.1-codex-max`,
//! `gemini-3.1-pro-preview`). Every user-facing surface (the `/model` picker,
//! header, status line, info widgets, onboarding copy) wants the same friendly
//! rendering, so the formatting rules live here rather than being reinvented per
//! call site.
//!
//! Two entry points, with deliberately different policies:
//!
//! * [`pretty_model_display_name`] always returns something readable. Prose
//!   surfaces use it because a raw slug in a sentence reads badly.
//! * [`pretty_known_model_family`] returns `None` for anything outside the
//!   curated GPT/Claude/Gemini families. List surfaces use it so third-party,
//!   open-weights, namespaced, and profile-scoped ids stay byte-exact and remain
//!   copy-pasteable.

/// Turn a raw model id into a friendlier display name.
///
/// Examples:
///   `gpt-5.5`                   -> `GPT-5.5`
///   `gpt-5.1-codex-max`         -> `GPT-5.1 Codex Max`
///   `gpt-5.6-pro[web]`          -> `GPT-5.6 Pro (web)`
///   `claude-opus-4-8`           -> `Claude Opus 4.8`
///   `claude-opus-4-6[1m]`       -> `Claude Opus 4.6 (1M)`
///   `claude-haiku-4-5-20251001` -> `Claude Haiku 4.5 (2025-10-01)`
///   `gemini-2.5-pro`            -> `Gemini 2.5 Pro`
/// Unknown shapes are returned mostly as-is so we never hide the real id.
pub(crate) fn pretty_model_display_name(model: &str) -> String {
    let model = model.trim();
    if model.is_empty() {
        return "your default model".to_string();
    }

    // Preserve bracketed route suffixes (`[1m]`, `[web]`) and re-attach them as
    // a parenthetical, since they are jcode-side route markers rather than part
    // of the upstream family/version name.
    let (core, bracket_suffix) = split_bracket_suffix(model);
    // Dated snapshots (`-20251001`) stay visible so a snapshot row is never
    // confused with its floating alias, but read as a date instead of being
    // glued onto the version number.
    let (core, snapshot_date) = split_snapshot_date(core);

    let lower = core.to_ascii_lowercase();
    let mut pretty = if lower.starts_with("gpt-") {
        prettify_versioned_family("GPT", core)
    } else if lower.starts_with("claude-") {
        // Anthropic: claude-opus-4-8 -> Claude Opus 4.8. Convert the trailing
        // `-<major>-<minor>` version into `<major>.<minor>` and title-case the
        // family/tier words.
        prettify_claude(core)
    } else {
        // Gemini and everything else: just title-case the dashed segments.
        title_case_dashed(core)
    };

    // Merge the date/route markers into one parenthetical so a dated 1M row
    // reads `Claude Sonnet 4.5 (2025-09-29, 1M)` rather than stacking two
    // separate groups.
    let markers: Vec<String> = snapshot_date.into_iter().chain(bracket_suffix).collect();
    if !markers.is_empty() {
        pretty.push_str(&format!(" ({})", markers.join(", ")));
    }
    pretty
}

/// Split a trailing bracketed route marker, normalizing `[1m]` to `1M`.
fn split_bracket_suffix(model: &str) -> (&str, Option<String>) {
    let Some(open) = model.rfind('[') else {
        return (model, None);
    };
    if !model.ends_with(']') {
        return (model, None);
    }
    let inner = &model[open + 1..model.len() - 1];
    if inner.is_empty() {
        return (model, None);
    }
    let normalized = if inner.eq_ignore_ascii_case("1m") {
        "1M".to_string()
    } else {
        inner.to_string()
    };
    (&model[..open], Some(normalized))
}

/// Split a trailing `-YYYYMMDD` snapshot date into a `YYYY-MM-DD` label.
fn split_snapshot_date(model: &str) -> (&str, Option<String>) {
    let Some((head, tail)) = model.rsplit_once('-') else {
        return (model, None);
    };
    if head.is_empty() || tail.len() != 8 || !tail.chars().all(|c| c.is_ascii_digit()) {
        return (model, None);
    }
    (
        head,
        Some(format!("{}-{}-{}", &tail[..4], &tail[4..6], &tail[6..])),
    )
}

/// Render a `<family>-<version>[-<qualifier>...]` id such as `gpt-5.1-codex-max`
/// as `GPT-5.1 Codex Max`: the family keeps its canonical casing, the version
/// stays attached to it, and the trailing qualifier words are title-cased so the
/// name does not trail off into raw lowercase slug text.
fn prettify_versioned_family(family_label: &str, core: &str) -> String {
    let rest = match core.split_once('-') {
        Some((_, rest)) => rest,
        None => return family_label.to_string(),
    };
    let mut parts = rest.split('-');
    let Some(version) = parts.next() else {
        return family_label.to_string();
    };
    // `gpt-oss-120b` and friends have no version: fall back to title-casing so
    // the caller still gets a readable label instead of `GPT-oss-120b`.
    if !version.starts_with(|c: char| c.is_ascii_digit()) {
        return title_case_dashed(core);
    }
    let mut out = format!("{family_label}-{version}");
    for part in parts {
        out.push(' ');
        out.push_str(&title_case_word(part));
    }
    out
}

/// Prettify only recognized model families (`gpt-*`, `claude-*`, `gemini-*`),
/// returning `None` for anything else so unfamiliar or namespaced ids
/// (`vendor/model`, `profile:model`) keep their exact spelling. Used by the
/// `/model` picker, where hiding the real id would break copy-paste and
/// provider-specific naming.
pub(crate) fn pretty_known_model_family(model: &str) -> Option<String> {
    let (core, _) = split_bracket_suffix(model);
    if core.contains('/') || core.contains(':') {
        return None;
    }
    let lower = core.to_ascii_lowercase();
    // `gpt-` only counts when a version number follows (`gpt-5.5`), so
    // open-weights ids like `gpt-oss-120b` are not mangled into `GPT-oss-…`.
    let versioned_gpt = lower
        .strip_prefix("gpt-")
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
    if !(versioned_gpt || lower.starts_with("claude-") || lower.starts_with("gemini-")) {
        return None;
    }
    Some(pretty_model_display_name(model))
}

/// Render `claude-opus-4-8` as `Claude Opus 4.8`.
fn prettify_claude(core: &str) -> String {
    let parts: Vec<&str> = core.split('-').collect();
    let mut words: Vec<String> = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        let part = parts[i];
        // Collapse a `<major>-<minor>` numeric pair into `<major>.<minor>`.
        if part.chars().all(|c| c.is_ascii_digit())
            && i + 1 < parts.len()
            && parts[i + 1].chars().all(|c| c.is_ascii_digit())
        {
            words.push(format!("{}.{}", part, parts[i + 1]));
            i += 2;
            continue;
        }
        words.push(title_case_word(part));
        i += 1;
    }
    words.join(" ")
}

/// Title-case a dash-separated id (`gemini-2.5-pro` -> `Gemini 2.5 Pro`).
fn title_case_dashed(core: &str) -> String {
    core.split('-')
        .map(title_case_word)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Title-case a single token, leaving anything containing a digit untouched so
/// version fragments like `4.8` or `2.5` are preserved.
fn title_case_word(word: &str) -> String {
    if word.is_empty() {
        return String::new();
    }
    if word.chars().any(|c| c.is_ascii_digit()) {
        return word.to_string();
    }
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{pretty_known_model_family, pretty_model_display_name};

    #[test]
    fn pretty_model_display_name_formats_common_models() {
        assert_eq!(pretty_model_display_name("gpt-5.5"), "GPT-5.5");
        assert_eq!(pretty_model_display_name("gpt-5.1-codex"), "GPT-5.1 Codex");
        assert_eq!(
            pretty_model_display_name("gpt-5.1-codex-max"),
            "GPT-5.1 Codex Max"
        );
        // Bracketed route markers become a parenthetical instead of leaking `[web]`.
        assert_eq!(
            pretty_model_display_name("gpt-5.6-pro[web]"),
            "GPT-5.6 Pro (web)"
        );
        // Dated snapshots read as a date, not as extra version digits.
        assert_eq!(
            pretty_model_display_name("claude-haiku-4-5-20251001"),
            "Claude Haiku 4.5 (2025-10-01)"
        );
        assert_eq!(
            pretty_model_display_name("claude-sonnet-4-20250514"),
            "Claude Sonnet 4 (2025-05-14)"
        );
        assert_eq!(
            pretty_model_display_name("claude-sonnet-4-5-20250929[1m]"),
            "Claude Sonnet 4.5 (2025-09-29, 1M)"
        );
        assert_eq!(
            pretty_model_display_name("claude-opus-4-8"),
            "Claude Opus 4.8"
        );
        assert_eq!(
            pretty_model_display_name("claude-sonnet-4-5"),
            "Claude Sonnet 4.5"
        );
        assert_eq!(
            pretty_model_display_name("claude-opus-4-8[1m]"),
            "Claude Opus 4.8 (1M)"
        );
        assert_eq!(
            pretty_model_display_name("gemini-2.5-pro"),
            "Gemini 2.5 Pro"
        );
    }

    #[test]
    fn pretty_known_model_family_gates_to_versioned_known_families() {
        // Known families with a version are prettified.
        assert_eq!(
            pretty_known_model_family("claude-opus-4-8").as_deref(),
            Some("Claude Opus 4.8")
        );
        assert_eq!(
            pretty_known_model_family("gemini-3.1-pro-preview").as_deref(),
            Some("Gemini 3.1 Pro Preview")
        );
        assert_eq!(
            pretty_known_model_family("gpt-5.6-pro[web]").as_deref(),
            Some("GPT-5.6 Pro (web)")
        );
        // Open-weights, third-party, namespaced, and profile-scoped ids stay raw so
        // they remain copy-pasteable and unambiguous in the picker.
        for raw in [
            "gpt-oss-120b-medium",
            "composer-2.5",
            "sonnet-4.6-thinking",
            "opus-4.6",
            "GLM-5.1",
            "Llama-3.3-70B-Instruct",
            "MiniMax-M2.5-highspeed",
            "qwen3-coder-plus",
            "o3-mini",
            "grok-4",
            "deepseek/deepseek-v4-pro",
            "anthropic/claude-opus-4.6",
            "google/gemini-3-pro-preview",
            "comtegra:glm-51-nvfp4",
        ] {
            assert!(
                pretty_known_model_family(raw).is_none(),
                "{raw} should stay verbatim"
            );
        }
    }

    #[test]
    fn pretty_model_display_name_handles_empty_and_unknown() {
        assert_eq!(pretty_model_display_name(""), "your default model");
        assert_eq!(pretty_model_display_name("   "), "your default model");
        // Unknown shapes fall back to a title-cased dashed rendering.
        assert_eq!(
            pretty_model_display_name("some-new-model"),
            "Some New Model"
        );
    }

    #[test]
    #[ignore = "developer review: dumps raw -> pretty model name mapping for manual audit"]
    fn dump_pretty_model_names_for_manual_audit() {
        let mut names: Vec<String> = Vec::new();
        for list in [
            jcode_provider_core::ALL_CLAUDE_MODELS,
            jcode_provider_core::ALL_OPENAI_MODELS,
            jcode_provider_core::OPENAI_API_ONLY_PRO_MODELS,
        ] {
            names.extend(list.iter().map(|m| m.to_string()));
        }
        if let Ok(extra) = std::fs::read_to_string("/tmp/audit_names.txt") {
            names.extend(
                extra
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty()),
            );
        }
        names.sort();
        names.dedup();
        for name in names {
            let pretty = pretty_known_model_family(&name);
            println!(
                "{:<42} => {}",
                name,
                pretty.unwrap_or_else(|| format!("(verbatim) {name}"))
            );
        }
    }
}
