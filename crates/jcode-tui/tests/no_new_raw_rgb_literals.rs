//! Guard: no *new* raw `rgb(r, g, b)` literals outside the palette module.
//!
//! Requested in the #1397 migration plan (maintainer ruling, step 5): a raw
//! literal carries no role, so it can never follow `/colors <role>`. Once every
//! rendered color is a role or a role-derived shade, new literals must not creep
//! back in. The migration is incremental, so this is a ratchet: `BASELINE`
//! records the literals that still exist today and the test fails only when one
//! is *added*. Lower or delete entries as families move to roles, and this
//! becomes the hard zero-tolerance guard once `BASELINE` is empty.
//!
//! Scope: TUI-rendering crates, excluding `jcode-tui-style` (the palette module,
//! where role defaults legitimately live as raw values) and test-only files
//! (`tests/` dirs, `*_tests.rs`). Comment lines are ignored, so docs may name
//! colors freely.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Raw `rgb(r, g, b)` literal counts that exist today, per file, relative to
/// `crates/`. To regenerate after migrating a family: remove or lower the
/// matching entries. A brand-new entry means a raw literal was introduced.
const BASELINE: &[(&str, usize)] = &[
    ("jcode-tui/src/tui/ui_messages.rs", 113),
    ("jcode-tui-render/src/swarm_gallery.rs", 88),
    ("jcode-tui/src/tui/ui_input.rs", 71),
    ("jcode-tui/src/tui/info_widget.rs", 56),
    ("jcode-tui/src/tui/session_picker/render.rs", 54),
    ("jcode-tui/src/tui/info_widget_todos.rs", 47),
    ("jcode-tui/src/tui/info_widget_memory_render.rs", 44),
    ("jcode-tui/src/tui/info_widget_model.rs", 44),
    ("jcode-tui/src/tui/session_picker.rs", 38),
    ("jcode-tui/src/tui/ui_inline_interactive.rs", 30),
    ("jcode-tui-permissions/src/lib.rs", 28),
    ("jcode-tui/src/tui/info_widget_swarm_background.rs", 25),
    ("jcode-tui/src/tui/info_widget_usage.rs", 21),
    ("jcode-tui/src/tui/info_widget_git.rs", 16),
    ("jcode-tui/src/tui/ui_todo_changes.rs", 16),
    ("jcode-tui-workspace/src/workspace_map_widget.rs", 15),
    ("jcode-tui/src/tui/info_widget_timeline.rs", 15),
    ("jcode-tui-markdown/src/lib.rs", 12),
    ("jcode-tui/src/tui/ui_overlays.rs", 12),
    ("jcode-tui/src/tui/ui_tests/tools.rs", 12),
    ("jcode-tui/src/tui/ui_onboarding.rs", 10),
    ("jcode-tui-mermaid/src/mermaid_content.rs", 6),
    ("jcode-tui/src/tui/ui_prepare.rs", 6),
    ("jcode-tui/src/tui/ui_header.rs", 5),
    ("jcode-tui/src/tui/info_widget_tips.rs", 3),
    ("jcode-tui/src/tui/ui_inline.rs", 3),
    ("jcode-tui/src/tui/ui_tools.rs", 3),
    ("jcode-tui/src/tui/ui.rs", 2),
    ("jcode-tui-markdown/src/markdown_wrap.rs", 1),
    ("jcode-tui-mermaid/src/mermaid_viewport.rs", 1),
    ("jcode-tui-mermaid/src/mermaid_widget.rs", 1),
    ("jcode-tui/src/tui/app/onboarding_flow_control.rs", 1),
    ("jcode-tui/src/tui/ui/selection_highlight.rs", 1),
    ("jcode-tui/src/tui/ui_file_diff.rs", 1),
    ("jcode-tui/src/tui/ui_pinned.rs", 1),
];

// ponytail: line-based, so a literal whose digits wrap to the next line is not
// counted (there is one such site today). Consistent with how BASELINE was
// generated; revisit only if a wrapped literal sneaks past the guard.
fn parse_u8_digits(bytes: &[u8], i: &mut usize) -> bool {
    let start = *i;
    while *i < bytes.len() && bytes[*i].is_ascii_digit() {
        *i += 1;
    }
    *i > start
}

fn skip_ws(bytes: &[u8], i: &mut usize) {
    while *i < bytes.len() && (bytes[*i] == b' ' || bytes[*i] == b'\t') {
        *i += 1;
    }
}

/// Matches a bare numeric `rgb(<d>, <d>, <d>)` tail (anything after `rgb(`).
/// Expression arguments such as `rgb(MATH_FOREGROUND.0, ..)` do not count.
fn matches_numeric_triple(bytes: &[u8]) -> bool {
    let mut i = 0;
    for _ in 0..2 {
        skip_ws(bytes, &mut i);
        if !parse_u8_digits(bytes, &mut i) {
            return false;
        }
        skip_ws(bytes, &mut i);
        if bytes.get(i) != Some(&b',') {
            return false;
        }
        i += 1;
    }
    skip_ws(bytes, &mut i);
    if !parse_u8_digits(bytes, &mut i) {
        return false;
    }
    skip_ws(bytes, &mut i);
    bytes.get(i) == Some(&b')')
}

fn count_in_line(line: &str) -> usize {
    let bytes = line.as_bytes();
    let mut count = 0;
    let mut search_from = 0;
    while let Some(pos) = line[search_from..].find("rgb(") {
        let start = search_from + pos;
        // Reject `hsl_to_rgb(`, `indexed_to_rgb(` and similar identifier tails.
        let at_ident_boundary = start == 0 || {
            let prev = bytes[start - 1];
            !(prev.is_ascii_alphanumeric() || prev == b'_')
        };
        if at_ident_boundary && matches_numeric_triple(&bytes[start + 4..]) {
            count += 1;
        }
        search_from = start + 4;
    }
    count
}

fn scan_dir(dir: &Path, crates_root: &Path, out: &mut BTreeMap<String, usize>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "tests") {
                continue;
            }
            scan_dir(&path, crates_root, out);
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.ends_with("_tests.rs") || name == "tests.rs" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut count = 0;
        for line in text.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with('*') {
                continue;
            }
            count += count_in_line(line);
        }
        if count == 0 {
            continue;
        }
        let key = path
            .strip_prefix(crates_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        out.insert(key, count);
    }
}

#[test]
fn no_new_raw_rgb_literals_outside_the_palette_module() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crates_root = manifest.parent().expect("crates dir");

    let mut actual = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(crates_root) else {
        panic!("cannot read {}", crates_root.display());
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let rendered_crate = name.starts_with("jcode-tui") || name == "jcode-render-core";
        // The palette module defines role defaults as raw values; that is where
        // raw colors are allowed to live.
        if !rendered_crate || name == "jcode-tui-style" {
            continue;
        }
        scan_dir(&entry.path().join("src"), crates_root, &mut actual);
    }

    let baseline: BTreeMap<&str, usize> = BASELINE.iter().copied().collect();
    let mut violations = Vec::new();
    for (file, count) in &actual {
        match baseline.get(file.as_str()) {
            Some(&allowed) if *count <= allowed => {}
            Some(&allowed) => violations.push(format!(
                "{file}: {count} raw rgb literals, baseline {allowed} (+{})",
                count - allowed
            )),
            None => violations.push(format!(
                "{file}: {count} raw rgb literals in a file not in BASELINE"
            )),
        }
    }

    assert!(
        violations.is_empty(),
        "new raw `rgb(...)` literal(s) outside the palette module (#1397): a raw\n\
         literal carries no role, so it can never follow `/colors <role>`.\n\
         Resolve the color through a `Role` (add a role-derived shade if a plain\n\
         role cannot express it) instead of adding a literal. If the color is\n\
         genuinely untagged, raise it on #1397 rather than raising BASELINE.\n\n{}",
        violations.join("\n")
    );
}
