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
pub fn pretty_model_display_name(model: &str) -> String {
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
    // Compact form: `-20251001`.
    if let Some((head, tail)) = model.rsplit_once('-')
        && !head.is_empty()
        && tail.len() == 8
        && tail.chars().all(|c| c.is_ascii_digit())
    {
        return (
            head,
            Some(format!("{}-{}-{}", &tail[..4], &tail[4..6], &tail[6..])),
        );
    }

    // Dashed form: `-2025-04-14`, as used by OpenAI snapshot ids. Without this
    // the date leaks into the name as three separate title-cased words
    // (`GPT-4.1 Mini 2025 04 14`).
    let mut segments = model.rsplitn(4, '-');
    let day = segments.next();
    let month = segments.next();
    let year = segments.next();
    let head = segments.next();
    if let (Some(head), Some(year), Some(month), Some(day)) = (head, year, month, day)
        && !head.is_empty()
        && is_ascii_digits(year, 4)
        && is_ascii_digits(month, 2)
        && is_ascii_digits(day, 2)
    {
        return (&model[..head.len()], Some(format!("{year}-{month}-{day}")));
    }

    (model, None)
}

/// True when `value` is exactly `len` ASCII digits.
fn is_ascii_digits(value: &str, len: usize) -> bool {
    value.len() == len && value.chars().all(|c| c.is_ascii_digit())
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

/// AWS Bedrock region routing prefixes on cross-region inference profile ids.
const BEDROCK_REGION_PREFIXES: [&str; 4] = ["us.", "eu.", "apac.", "global."];

/// Bedrock vendor namespaces jcode knows how to render.
///
/// `anthropic.` reuses the Claude formatter. `amazon.` covers the first-party
/// Nova family, which users see most on Bedrock and which title-cases cleanly.
/// `meta.`, `qwen.`, `cohere.`, `ai21.`, `writer.`, `stability.` and friends are
/// deliberately absent: their slugs carry parameter counts and quantization
/// detail (`llama3-1-405b-instruct`, `qwen3-coder-480b-a35b`) that must stay
/// byte-exact to remain meaningful.
const BEDROCK_PRETTY_VENDORS: [&str; 2] = ["anthropic.", "amazon."];

/// Split an AWS Bedrock model id into its region prefix, vendor namespace, and
/// model portion.
///
/// Bedrock ids are structured rather than opaque:
/// `us.anthropic.claude-opus-4-20250514-v1:0` is a region-routed cross-region
/// inference profile for `anthropic`'s `claude-opus-4` snapshot at API revision
/// `v1:0`. Rendering that raw makes Bedrock rows the least readable in the
/// picker, so the parts are separated and reassembled by the caller.
fn split_bedrock_model_id(model: &str) -> Option<(Option<&str>, &str, &str, Option<String>)> {
    // Full ARNs carry account/region routing detail that must not be hidden.
    let trimmed = model.trim();
    if trimmed.starts_with("arn:aws:bedrock:") {
        return None;
    }

    let mut rest = trimmed;
    let mut region = None;
    for prefix in BEDROCK_REGION_PREFIXES {
        if let Some(stripped) = rest.strip_prefix(prefix) {
            region = Some(&prefix[..prefix.len() - 1]);
            rest = stripped;
            break;
        }
    }

    let vendor = BEDROCK_PRETTY_VENDORS
        .iter()
        .find(|vendor| rest.starts_with(**vendor))?;
    let model_part = &rest[vendor.len()..];
    if model_part.is_empty() {
        return None;
    }

    // A trailing `-v1`, `-v1:0`, or `-v1:0:200k` is a Bedrock API revision plus
    // an optional context/modality variant, not part of the model version. Keep
    // it as a marker so distinct revisions and context variants stay
    // distinguishable, but stop it from corrupting the version number.
    let (model_part, revision) = match split_bedrock_revision(model_part) {
        Some((head, revision)) => (head, Some(revision)),
        None => (model_part, None),
    };

    Some((region, &vendor[..vendor.len() - 1], model_part, revision))
}

/// Split a trailing Bedrock revision segment (`-v1`, `-v1:0`, `-v1:0:200k`,
/// `-v1:0:mm`) off a model id, returning the head plus the revision label.
fn split_bedrock_revision(model_part: &str) -> Option<(&str, String)> {
    let (head, tail) = model_part.rsplit_once("-v")?;
    if head.is_empty() || tail.is_empty() {
        return None;
    }
    let mut segments = tail.split(':');
    // The revision major must be numeric (`v1`), which is what distinguishes it
    // from a family word such as the `-vl` in `qwen3-vl-235b`.
    let major = segments.next()?;
    if major.is_empty() || !major.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // Remaining segments are the revision minor and an optional context or
    // modality variant (`200k`, `mm`); accept alphanumerics so new variants do
    // not silently fall back to a raw id.
    for segment in segments {
        if segment.is_empty() || !segment.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
    }
    Some((head, format!("v{tail}")))
}

/// Render `us.anthropic.claude-opus-4-20250514-v1:0` as
/// `Claude Opus 4 (2025-05-14, us, v1:0)`.
fn prettify_bedrock(model: &str) -> Option<String> {
    let (region, vendor, model_part, revision) = split_bedrock_model_id(model)?;
    // Only render when the model portion is a family we know how to format;
    // otherwise the raw id is more informative than a half-prettified one.
    let base = match vendor {
        // `Amazon Nova Pro` is the official product name, and unlike `Claude`
        // the family slug alone would not identify the vendor.
        "amazon" if model_part.to_ascii_lowercase().starts_with("nova") => {
            format!("Amazon {}", title_case_dashed(model_part))
        }
        _ => pretty_known_model_family(model_part)?,
    };
    let mut markers: Vec<String> = Vec::new();
    if let Some(region) = region {
        markers.push(region.to_string());
    }
    if let Some(revision) = revision {
        markers.push(revision);
    }
    if markers.is_empty() {
        return Some(base);
    }
    // The base may already carry its own parenthetical (snapshot date, 1M);
    // merge into a single group rather than stacking them.
    Some(match base.strip_suffix(')') {
        Some(head) => format!("{head}, {})", markers.join(", ")),
        None => format!("{base} ({})", markers.join(", ")),
    })
}

/// Prettify only recognized model families (`gpt-*`, `claude-*`, `gemini-*`,
/// plus AWS Bedrock ids wrapping those families), returning `None` for anything
/// else so unfamiliar or namespaced ids (`vendor/model`, `profile:model`) keep
/// their exact spelling. Used by the `/model` picker, where hiding the real id
/// would break copy-paste and provider-specific naming.
pub fn pretty_known_model_family(model: &str) -> Option<String> {
    let (core, _) = split_bracket_suffix(model);
    if core.contains('.') && core.contains('-') {
        // Possibly a Bedrock id; `gpt-5.5` and `gemini-2.5-pro` also contain a
        // dot, so only take this path when a vendor namespace actually matches.
        if let Some(pretty) = prettify_bedrock(core) {
            return Some(pretty);
        }
    }
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

/// Acronyms that read wrong when naively title-cased (`Tts`, `Api`, `Vl`).
const UPPERCASE_TOKENS: [&str; 6] = ["tts", "stt", "api", "vl", "ocr", "id"];

/// Title-case a single token, leaving anything containing a digit untouched so
/// version fragments like `4.8` or `2.5` are preserved.
fn title_case_word(word: &str) -> String {
    if word.is_empty() {
        return String::new();
    }
    if word.chars().any(|c| c.is_ascii_digit()) {
        return word.to_string();
    }
    let lower = word.to_ascii_lowercase();
    if UPPERCASE_TOKENS.contains(&lower.as_str()) {
        return lower.to_ascii_uppercase();
    }
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// Brand spellings that plain title-casing gets wrong (`Deepseek`, `Glm`).
const BRAND_TOKENS: [(&str, &str); 24] = [
    ("gpt", "GPT"),
    ("oss", "OSS"),
    ("glm", "GLM"),
    ("deepseek", "DeepSeek"),
    ("minimax", "MiniMax"),
    ("openai", "OpenAI"),
    ("xai", "xAI"),
    ("tts", "TTS"),
    ("stt", "STT"),
    ("api", "API"),
    ("vl", "VL"),
    ("ocr", "OCR"),
    ("id", "ID"),
    ("it", "IT"),
    ("moe", "MoE"),
    ("fp8", "FP8"),
    ("fp4", "FP4"),
    ("nvfp4", "NVFP4"),
    ("awq", "AWQ"),
    ("gguf", "GGUF"),
    ("hd", "HD"),
    ("r1", "R1"),
    ("ai", "AI"),
    ("lfm", "LFM"),
];

/// OpenRouter-style `:variant` suffixes rendered as a parenthetical.
const PICKER_VARIANT_TAGS: [&str; 9] = [
    "free", "batch", "thinking", "beta", "nitro", "floor", "online", "exacto", "extended",
];

/// Families whose official names hyphenate the family to its version
/// (`GPT-5.5`, `GLM-5.1`, `GPT-OSS`).
const HYPHENATED_FAMILIES: [&str; 2] = ["GPT", "GLM"];

/// Render any model id as a readable picker title.
///
/// Unlike [`pretty_known_model_family`], this never returns the raw id for
/// unfamiliar shapes. It is meant for surfaces that also show the exact id
/// nearby (such as the `/model` picker's detail line), so a friendlier title
/// cannot hide which route will actually run.
///
/// Examples:
///   `openai-api:gpt-5.5`               -> `GPT-5.5`
///   `anthropic/claude-opus-4.6`        -> `Claude Opus 4.6`
///   `deepseek/deepseek-v4-pro`         -> `DeepSeek V4 Pro`
///   `Llama-3.3-70B-Instruct`           -> `Llama 3.3 70B Instruct`
///   `moonshotai/kimi-k2.5:free`        -> `Kimi K2.5 (free)`
///   `gpt-oss-120b`                     -> `GPT-OSS 120B`
///   `o3-mini`                          -> `o3 Mini`
pub fn pretty_picker_model_name(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Routing prefixes (`openai-api:`) and vendor namespaces (`anthropic/`)
    // are shown by the picker's group header, not the title.
    let bare = crate::selection::explicit_model_provider_prefix(trimmed)
        .map_or(trimmed, |(_, _, bare)| bare);
    if bare.starts_with("arn:") {
        return bare.to_string();
    }
    if let Some(pretty) = pretty_known_model_family(bare) {
        return pretty;
    }
    let mut bare = bare.rsplit('/').next().unwrap_or(bare);
    // Profile or provider namespaces (`comtegra:glm-51`, `google:gemini-x`)
    // are purely alphabetic heads. The picker's group header names them.
    while let Some((head, rest)) = bare.split_once(':') {
        if head.is_empty() || rest.is_empty() || !head.chars().all(|c| c.is_ascii_alphabetic()) {
            break;
        }
        if PICKER_VARIANT_TAGS.contains(&rest.to_ascii_lowercase().as_str()) {
            break;
        }
        bare = rest;
    }
    // A known `:tag` (`:free`, `:thinking`) is a variant marker. Anything else
    // with a colon (Bedrock `-v1:0`) stays exact.
    let (bare, tag) = match bare.split_once(':') {
        Some((head, tag))
            if !head.is_empty()
                && PICKER_VARIANT_TAGS.contains(&tag.to_ascii_lowercase().as_str()) =>
        {
            (head, Some(tag.to_ascii_lowercase()))
        }
        Some(_) => return bare.to_string(),
        None => (bare, None),
    };
    if let Some(pretty) = pretty_known_model_family(bare) {
        return append_markers(pretty, tag.into_iter().collect());
    }
    // Bedrock-style `vendor.model` namespaces: drop a purely alphabetic vendor
    // segment that ends before the first dash (`google.gemma-3-27b-it`).
    let bare = match bare.split_once('.') {
        Some((vendor, rest))
            if !rest.is_empty()
                && vendor.chars().all(|c| c.is_ascii_alphabetic())
                && !bare[..vendor.len()].contains('-')
                && !rest.starts_with(|c: char| c.is_ascii_digit()) =>
        {
            rest
        }
        _ => bare,
    };
    if let Some(pretty) = pretty_known_model_family(bare) {
        return append_markers(pretty, tag.into_iter().collect());
    }
    let (core, bracket) = split_bracket_suffix(bare);
    let (core, date) = split_snapshot_date(core);
    let parts: Vec<&str> = core
        .split(['-', '_', ' '])
        .filter(|p| !p.is_empty())
        .collect();
    let mut words: Vec<String> = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        let part = parts[i];
        // `3-1` reads as version `3.1`, as Claude and Llama slugs intend.
        if part.len() <= 2
            && part.chars().all(|c| c.is_ascii_digit())
            && i + 1 < parts.len()
            && parts[i + 1].len() <= 2
            && parts[i + 1].chars().all(|c| c.is_ascii_digit())
        {
            words.push(format!("{part}.{}", parts[i + 1]));
            i += 2;
            continue;
        }
        let word = pretty_picker_token(part);
        match words.last_mut() {
            Some(previous)
                if HYPHENATED_FAMILIES.contains(&previous.as_str())
                    && (word.starts_with(|c: char| c.is_ascii_digit()) || word == "OSS") =>
            {
                previous.push('-');
                previous.push_str(&word);
            }
            _ => words.push(word),
        }
        i += 1;
    }
    if words.is_empty() {
        return bare.to_string();
    }
    let markers = date.into_iter().chain(bracket).chain(tag).collect();
    append_markers(words.join(" "), markers)
}

fn append_markers(base: String, markers: Vec<String>) -> String {
    if markers.is_empty() {
        return base;
    }
    match base.strip_suffix(')') {
        Some(head) if head.contains(" (") => format!("{head}, {})", markers.join(", ")),
        _ => format!("{base} ({})", markers.join(", ")),
    }
}

/// Case one model-id token for a picker title.
fn pretty_picker_token(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    if let Some((_, brand)) = BRAND_TOKENS.iter().find(|(raw, _)| *raw == lower) {
        return (*brand).to_string();
    }
    let bytes = lower.as_bytes();
    let digits_then = |suffix: &[u8]| {
        bytes.len() > 1
            && suffix.contains(&bytes[bytes.len() - 1])
            && lower[..lower.len() - 1]
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == 'x')
            && lower.starts_with(|c: char| c.is_ascii_digit())
    };
    // Parameter and context sizes: `70b` -> `70B`, `8x7b` -> `8x7B`, `200k`.
    if digits_then(b"bmkt") {
        let (head, unit) = lower.split_at(lower.len() - 1);
        return format!("{head}{}", unit.to_ascii_uppercase());
    }
    // Active-parameter counts: `a35b` -> `A35B`.
    if bytes.len() > 2
        && bytes[0] == b'a'
        && bytes[bytes.len() - 1] == b'b'
        && lower[1..lower.len() - 1]
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.')
    {
        return lower.to_ascii_uppercase();
    }
    // OpenAI reasoning families stay lowercase: `o3`, `o4`.
    if bytes[0] == b'o' && lower[1..].chars().all(|c| c.is_ascii_digit()) && bytes.len() > 1 {
        return lower;
    }
    // Single-letter generations: `k2.5` -> `K2.5`, `v4` -> `V4`, `m2` -> `M2`.
    if bytes.len() > 1
        && bytes[0].is_ascii_alphabetic()
        && lower[1..].chars().all(|c| c.is_ascii_digit() || c == '.')
    {
        return lower.to_ascii_uppercase();
    }
    // Mixed tokens: keep digits, capitalize a leading word (`qwen3` -> `Qwen3`).
    if token.chars().any(|c| c.is_ascii_digit()) {
        if token.starts_with(|c: char| c.is_ascii_lowercase()) {
            let letters: String = token
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            if letters.len() > 1 {
                let mut chars = token.chars();
                let first = chars.next().unwrap().to_ascii_uppercase();
                return format!("{first}{}", chars.as_str());
            }
        }
        return token.to_string();
    }
    // Preserve deliberate mixed case (`MiniMax`), title-case plain words.
    if token.chars().any(|c| c.is_ascii_uppercase()) {
        return token.to_string();
    }
    title_case_word(token)
}

#[cfg(test)]
mod tests {
    use super::{pretty_known_model_family, pretty_model_display_name, pretty_picker_model_name};

    #[test]
    fn picker_names_are_readable_for_every_catalog_shape() {
        for (raw, pretty) in [
            ("openai-api:gpt-5.5", "GPT-5.5"),
            ("claude-oauth:claude-opus-4-8", "Claude Opus 4.8"),
            ("anthropic/claude-opus-4.6", "Claude Opus 4.6"),
            ("google/gemini-3-pro-preview", "Gemini 3 Pro Preview"),
            ("deepseek/deepseek-v4-pro", "DeepSeek V4 Pro"),
            ("Llama-3.3-70B-Instruct", "Llama 3.3 70B Instruct"),
            ("MiniMax-M2.5-highspeed", "MiniMax M2.5 Highspeed"),
            ("moonshotai/kimi-k2.5:free", "Kimi K2.5 (free)"),
            ("gpt-oss-120b-medium", "GPT-OSS 120B Medium"),
            ("GLM-5.1", "GLM-5.1"),
            ("qwen3-coder-plus", "Qwen3 Coder Plus"),
            ("qwen3-coder-480b-a35b", "Qwen3 Coder 480B A35B"),
            ("o3-mini", "o3 Mini"),
            ("grok-4", "Grok 4"),
            ("composer-2.5", "Composer 2.5"),
            ("sonnet-4.6-thinking", "Sonnet 4.6 Thinking"),
            ("opus-4-6", "Opus 4.6"),
            ("google.gemma-3-27b-it", "Gemma 3 27B IT"),
            ("mixtral-8x7b", "Mixtral 8x7B"),
            (
                "anthropic/claude-fable-5.1:batch",
                "Claude Fable 5.1 (batch)",
            ),
            ("atlas-04", "Atlas 04"),
            ("mistral-large-2407", "Mistral Large 2407"),
        ] {
            assert_eq!(pretty_picker_model_name(raw), pretty, "{raw}");
        }
    }

    #[test]
    fn picker_names_keep_opaque_route_ids_exact() {
        for raw in [
            "arn:aws:bedrock:us-east-1::foundation-model/anthropic.claude-opus-4-20250514-v1:0",
            "meta.llama3-1-405b-instruct-v1:0",
        ] {
            assert_eq!(pretty_picker_model_name(raw), raw);
        }
        assert_eq!(
            pretty_picker_model_name("comtegra:glm-51-nvfp4"),
            "GLM-51 NVFP4"
        );
        assert_eq!(
            pretty_picker_model_name("google:gemini-review"),
            "Gemini Review"
        );
        assert_eq!(pretty_picker_model_name("openai:atlas-04"), "Atlas 04");
        assert_eq!(pretty_picker_model_name("  "), "");
    }

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
    fn pretty_model_display_name_collapses_dashed_snapshot_dates() {
        // OpenAI snapshot ids use a dashed date, which previously leaked into
        // the name as three separate words (`GPT-4.1 Mini 2025 04 14`).
        assert_eq!(
            pretty_model_display_name("gpt-4.1-mini-2025-04-14"),
            "GPT-4.1 Mini (2025-04-14)"
        );
        assert_eq!(
            pretty_model_display_name("gpt-4-turbo-2024-04-09"),
            "GPT-4 Turbo (2024-04-09)"
        );
        assert_eq!(
            pretty_model_display_name("gpt-5-pro-2025-10-06"),
            "GPT-5 Pro (2025-10-06)"
        );
        // A bare version segment must not be mistaken for a date.
        assert_eq!(pretty_model_display_name("gpt-5.4"), "GPT-5.4");
        assert_eq!(
            pretty_model_display_name("gemini-2.5-flash-lite"),
            "Gemini 2.5 Flash Lite"
        );
    }

    #[test]
    fn pretty_model_display_name_uppercases_known_acronyms() {
        assert_eq!(
            pretty_model_display_name("gpt-4o-mini-tts"),
            "GPT-4o Mini TTS"
        );
        assert_eq!(
            pretty_model_display_name("gpt-5-search-api"),
            "GPT-5 Search API"
        );
    }

    #[test]
    fn pretty_known_model_family_renders_aws_bedrock_ids() {
        // Region-routed cross-region inference profiles keep the region and the
        // Bedrock API revision visible, since both distinguish real routes.
        assert_eq!(
            pretty_known_model_family("us.anthropic.claude-opus-4-20250514-v1:0").as_deref(),
            Some("Claude Opus 4 (2025-05-14, us, v1:0)")
        );
        assert_eq!(
            pretty_known_model_family("eu.anthropic.claude-3-5-sonnet-20241022-v2:0").as_deref(),
            Some("Claude 3.5 Sonnet (2024-10-22, eu, v2:0)")
        );
        // Plain foundation-model ids have no region segment.
        assert_eq!(
            pretty_known_model_family("anthropic.claude-3-haiku-20240307-v1:0").as_deref(),
            Some("Claude 3 Haiku (2024-03-07, v1:0)")
        );
        // Undated ids stay clean rather than gaining an empty group.
        assert_eq!(
            pretty_known_model_family("anthropic.claude-sonnet-4-6").as_deref(),
            Some("Claude Sonnet 4.6")
        );
        assert_eq!(
            pretty_known_model_family("us.anthropic.claude-sonnet-4-6").as_deref(),
            Some("Claude Sonnet 4.6 (us)")
        );
        // Bare `-v1` revisions (no minor) and `-v1:0:200k` context variants are
        // both real Bedrock shapes seen in a live catalog.
        assert_eq!(
            pretty_known_model_family("us.anthropic.claude-opus-4-6-v1").as_deref(),
            Some("Claude Opus 4.6 (us, v1)")
        );
        assert_eq!(
            pretty_known_model_family("anthropic.claude-3-haiku-20240307-v1:0:200k").as_deref(),
            Some("Claude 3 Haiku (2024-03-07, v1:0:200k)")
        );
        assert_eq!(
            pretty_known_model_family("amazon.nova-premier-v1:0:mm").as_deref(),
            Some("Amazon Nova Premier (v1:0:mm)")
        );
        // Amazon's first-party Nova family keeps its vendor word.
        assert_eq!(
            pretty_known_model_family("amazon.nova-pro-v1:0").as_deref(),
            Some("Amazon Nova Pro (v1:0)")
        );
        assert_eq!(
            pretty_known_model_family("global.amazon.nova-2-lite-v1:0").as_deref(),
            Some("Amazon Nova 2 Lite (global, v1:0)")
        );
    }

    #[test]
    fn pretty_known_model_family_keeps_opaque_bedrock_ids_verbatim() {
        for raw in [
            // Full ARNs carry account/region routing that must not be hidden.
            "arn:aws:bedrock:us-east-1::foundation-model/anthropic.claude-opus-4-20250514-v1:0",
            // Third-party Bedrock vendors encode parameter counts and
            // quantization detail that only reads correctly byte-exact.
            "meta.llama3-1-405b-instruct-v1:0",
            "qwen.qwen3-coder-480b-a35b-v1:0",
            // `-vl` is a family word, not a `-v<major>` revision, so the
            // revision split must not fire here.
            "qwen.qwen3-vl-235b-a22b",
            "meta.llama3-1-70b-instruct-v1:0:128k",
            "google.gemma-3-27b-it",
            "zai.glm-4.7-flash",
            "mistral.devstral-2-123b",
            "mistral.mistral-large-2407-v1:0",
            "cohere.command-r-plus-v1:0",
            "ai21.jamba-1-5-large-v1:0",
            "writer.palmyra-x5-v1:0",
            "stability.sd3-5-large-v1:0",
            "deepseek.r1-v1:0",
            "us.deepseek.r1-v1:0",
            "openai.gpt-oss-120b-1:0",
            "moonshotai.kimi-k2-instruct-v1:0",
            "minimax.m2-v1:0",
            "zai.glm-4-6-v1:0",
            "nvidia.nemotron-super-v1:0",
            "google.gemini-2-5-pro-v1:0",
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
            crate::ALL_CLAUDE_MODELS,
            crate::ALL_OPENAI_MODELS,
            crate::OPENAI_API_ONLY_PRO_MODELS,
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
