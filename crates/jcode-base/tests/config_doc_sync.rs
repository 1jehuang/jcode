//! Keeps `docs/CONFIGURATION.md` honest.
//!
//! The config schema lives entirely in Rust structs. Before this test existed,
//! the only user-facing description of `config.toml` was whatever a human last
//! typed into a Markdown table, which drifted from the code silently: the first
//! draft of the doc documented several defaults (`scroll_up`, `new_terminal`,
//! `theme`, `tools.profile`) that the code had not used for some time.
//!
//! These tests compare the documented tables against `Config::default()` and
//! the keybinding registry, so a default that changes in code fails here until
//! the doc is updated to match.
//!
//! Comparison is section-aware (`compaction.mode` is not `dictation.mode`), and
//! only covers keys whose default is a concrete serialized TOML value. Fields
//! that default to `None` are absent from serialized output entirely, so the
//! doc describes those in prose ("unset", "inherit", "auto") and they are
//! skipped here.

use jcode_base::config::Config;
use jcode_config_types::{KEYBINDING_DEFAULTS, KeybindingPlatform, default_binding};
use std::collections::BTreeMap;

const DOC: &str = include_str!("../../../docs/CONFIGURATION.md");

/// Split a Markdown table row into cells, honoring `\|` escapes inside cells
/// (type columns like `` `"internal"` \| `"external"` `` contain them).
fn split_row(line: &str) -> Vec<String> {
    let mut cells = vec![String::new()];
    let mut escaped = false;
    for ch in line.trim_matches('|').chars() {
        match ch {
            '\\' if !escaped => escaped = true,
            '|' if !escaped => cells.push(String::new()),
            _ => {
                if escaped {
                    escaped = false;
                    // Keep the escaped character itself, drop the backslash.
                }
                cells.last_mut().expect("row always has a cell").push(ch);
            }
        }
    }
    cells.iter().map(|c| c.trim().to_string()).collect()
}

/// Parse the doc's config tables into `section.key -> documented default cell`.
///
/// Tables vary in shape (`Key|Type|Default|Meaning`, `Key|Type|Default`, and
/// the keybinding table's `Key|Default|Action`), so the default column is
/// located by reading each table's header row rather than assumed by position.
/// The env-var and `[safety]` tables have no Default column and are skipped
/// automatically.
fn documented_defaults() -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    let mut section = String::new();
    let mut default_col: Option<usize> = None;

    for line in DOC.lines() {
        let line = line.trim();

        // Track the current section from headings like "## `[display]`" and
        // "### `[display.native_scrollbars]`".
        if let Some(rest) = line
            .strip_prefix("## ")
            .or_else(|| line.strip_prefix("### "))
        {
            if let Some(name) = rest
                .trim()
                .strip_prefix("`[")
                .and_then(|r| r.split("]`").next())
            {
                // "`[autoreview]` and `[autojudge]`" documents two sections with
                // one table; the first name resolves its keys.
                section = name.to_string();
            }
            default_col = None;
            continue;
        }

        if !line.starts_with('|') {
            // Any non-table line ends the current table.
            if !line.is_empty() {
                default_col = None;
            }
            continue;
        }

        let cells = split_row(line);

        // Header row: locate the Default column for the table that follows.
        if cells.iter().any(|c| c.eq_ignore_ascii_case("default")) {
            default_col = cells.iter().position(|c| c.eq_ignore_ascii_case("default"));
            continue;
        }
        let Some(col) = default_col else {
            continue;
        };
        if cells.len() <= col || !cells[0].starts_with('`') {
            continue;
        }

        // A row may name several sibling keys, e.g.
        // "`workspace_left` / `workspace_down` | `alt+h` / `alt+j` | ...".
        let keys: Vec<String> = cells[0]
            .split('/')
            .filter_map(|k| {
                k.trim()
                    .strip_prefix('`')
                    .and_then(|k| k.split('`').next())
                    .map(str::to_string)
            })
            .collect();
        if keys.is_empty() || section.is_empty() {
            continue;
        }
        // Cells may document both platforms, e.g.
        // "`cmd+right` / `cmd+left` (macOS), `alt+right` / `alt+left` elsewhere".
        // Only the leading (current-platform) clause lines up with the keys;
        // the cross-platform text is covered by the keybinding-registry test.
        let primary = cells[col].split(" (").next().unwrap_or(&cells[col]);
        let values: Vec<&str> = primary.split('/').map(str::trim).collect();

        for (i, key) in keys.iter().enumerate() {
            // Split the value alongside the key only when the arity matches;
            // otherwise the whole cell describes every listed key.
            let value = if keys.len() > 1 && values.len() == keys.len() {
                values[i].to_string()
            } else {
                cells[col].clone()
            };
            found.insert(format!("{section}.{key}"), value);
        }
    }
    found
}

/// Flatten the serialized default config into `section.key -> TOML value`.
fn actual_defaults() -> BTreeMap<String, String> {
    let toml = toml::to_string_pretty(&Config::default()).expect("serialize default config");
    let mut actual = BTreeMap::new();
    let mut section = String::new();

    for line in toml.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            section = name.to_string();
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        actual.insert(
            format!("{section}.{}", key.trim()),
            value.trim().to_string(),
        );
    }
    actual
}

/// Reduce a documented cell or a TOML value to a comparable core: drop
/// backticks, quotes, and the parenthetical gloss in cells like `` `""` (auto) ``.
fn normalize(value: &str) -> String {
    let mut v = value.trim();
    if let Some(idx) = v.find(" (") {
        v = &v[..idx];
    }
    v.trim()
        .trim_matches('`')
        .trim_matches('"')
        .trim()
        .to_string()
}

#[test]
fn documented_defaults_match_the_serialized_default_config() {
    let documented = documented_defaults();
    let actual = actual_defaults();

    let mut mismatches = Vec::new();
    let mut compared = 0usize;

    for (path, doc_default) in &documented {
        // Keys with no serialized default are `Option::None` fields described
        // in prose; there is no concrete value to compare against.
        let Some(actual_value) = actual.get(path) else {
            continue;
        };
        compared += 1;

        let doc_norm = normalize(doc_default);
        let actual_norm = normalize(actual_value);
        if doc_norm == actual_norm {
            continue;
        }
        // f32 fields serialize with representation noise (0.30000001192092896).
        if let (Ok(a), Ok(b)) = (doc_norm.parse::<f64>(), actual_norm.parse::<f64>())
            && (a - b).abs() < 1e-6
        {
            continue;
        }
        mismatches.push(format!(
            "  {path}: documented `{doc_norm}`, actual `{actual_norm}`"
        ));
    }

    assert!(
        compared > 50,
        "expected to compare most documented keys, only compared {compared}; \
         the doc table parser is probably broken"
    );
    assert!(
        mismatches.is_empty(),
        "docs/CONFIGURATION.md defaults disagree with Config::default():\n{}",
        mismatches.join("\n")
    );
}

#[test]
fn every_serialized_default_key_is_documented() {
    let documented = documented_defaults();
    let missing: Vec<String> = actual_defaults()
        .keys()
        .filter(|path| !documented.contains_key(*path))
        .cloned()
        .collect();

    assert!(
        missing.is_empty(),
        "config keys with a real default but no row in docs/CONFIGURATION.md: {missing:?}"
    );
}

#[test]
fn documented_keybinding_defaults_match_the_registry() {
    let mut problems = Vec::new();

    for entry in KEYBINDING_DEFAULTS {
        if !DOC.contains(&format!("`{}`", entry.id)) {
            problems.push(format!("  {}: action not documented", entry.id));
            continue;
        }
        // Every non-empty default chord must appear in the doc, so re-chording a
        // default in code cannot leave stale text behind.
        for platform in [KeybindingPlatform::MacOs, KeybindingPlatform::Other] {
            let chord = default_binding(entry.id, platform).unwrap_or("");
            if chord.is_empty() {
                continue;
            }
            if !DOC.contains(&format!("`{chord}`")) {
                problems.push(format!(
                    "  {}: {} default `{chord}` not in the doc",
                    entry.id,
                    platform.label()
                ));
            }
        }
    }

    assert!(
        problems.is_empty(),
        "docs/CONFIGURATION.md keybinding defaults are stale:\n{}",
        problems.join("\n")
    );
}

#[test]
fn every_config_section_is_documented() {
    let toml = toml::to_string_pretty(&Config::default()).expect("serialize default config");
    let mut undocumented = Vec::new();

    for line in toml.lines() {
        let Some(section) = line
            .trim()
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        else {
            continue;
        };
        // Sections may carry their own heading, or share one (autoreview and
        // autojudge have identical shapes and are documented together).
        if !DOC.contains(&format!("`[{section}]`")) {
            undocumented.push(section.to_string());
        }
    }

    assert!(
        undocumented.is_empty(),
        "config sections missing from docs/CONFIGURATION.md: {undocumented:?}"
    );
}
