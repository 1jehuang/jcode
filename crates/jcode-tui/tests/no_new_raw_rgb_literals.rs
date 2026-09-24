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
//! where role defaults legitimately live as raw values) and test-only code
//! (`tests`/`*_tests` directories, `*_tests.rs`/`tests.rs` files). Comment lines
//! are ignored, so docs may name colors freely.

use regex::Regex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

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

/// A bare `rgb(<digits>, <digits>, <digits>)`, with an optional trailing comma
/// (the rustfmt shape for a wrapped call). `\b` rejects `hsl_to_rgb(` and
/// friends; expression arguments (`rgb(ROLE.0, ..)`) do not match; `\s` spans
/// line breaks, so a call wrapped across lines still counts.
static LITERAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\brgb\(\s*[0-9]+\s*,\s*[0-9]+\s*,\s*[0-9]+\s*,?\s*\)").expect("valid rgb regex")
});

fn scan_dir(dir: &Path, crates_root: &Path, out: &mut BTreeMap<String, usize>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("cannot read an entry of {}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            // Test-only modules carry no rendered color; skip so a test fixture
            // never has to raise the baseline.
            let is_test_dir = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == "tests" || n.ends_with("_tests"));
            if is_test_dir {
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
        // A read failure must fail the test, not silently drop coverage.
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        // Drop whole comment lines, then scan the file as one string so a call
        // wrapped across lines is still found.
        let code: String = text
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("//") && !trimmed.starts_with('*')
            })
            .collect::<Vec<_>>()
            .join("\n");
        let count = LITERAL.find_iter(&code).count();
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
