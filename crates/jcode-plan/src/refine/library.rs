//! Per-domain experience library: adopted harnesses + why they won.
//!
//! Ported from harness2's `harness2/core/library.py` (Apache-2.0, Google
//! LLC). After each task, the adopted harness (FULL tree, never a summary)
//! and an evidence-supported lesson are stored under
//! `<root>/<benchmark>/k<k>-<variant>/<domain>/harnesses/<task>/`, with an
//! `INDEX.md` rebuilt from disk and an `EXPERIENCE.md` per entry carrying the
//! cue/lesson/evidence triple. Tasks within a domain run in order and read
//! only earlier entries; different domains run concurrently.
//!
//! Why it matters: this is the harnessing-level recursion half. Without it,
//! every task starts from the base harness; with it, later tasks in a domain
//! read proven components as examples (copy, re-verify workspace facts, never
//! presume). This module builds the pure data (entry docs, index); the caller
//! owns the filesystem writes.

use super::selection::Experience;
use super::surface::{SurfaceSpec, Tree, always_on_text};

/// One adopted harness ready to be persisted as a library entry.
#[derive(Debug, Clone)]
pub struct LibraryEntry {
    pub task_id: String,
    pub domain: String,
    pub label: String,
    pub run_id: String,
    pub why: String,
    pub experience: Experience,
    /// Optional per-edit notes from the reflection reply (`<edits>` block).
    /// Rendered as "## What each edit did" when non-empty.
    #[allow(clippy::struct_field_names)]
    pub edits: Option<String>,
}

impl LibraryEntry {
    pub fn new(
        task_id: &str,
        domain: &str,
        label: &str,
        run_id: &str,
        why: &str,
        experience: Experience,
    ) -> Self {
        Self {
            task_id: task_id.to_string(),
            domain: domain.to_string(),
            label: label.to_string(),
            run_id: run_id.to_string(),
            why: why.to_string(),
            experience,
            edits: None,
        }
    }
}

impl LibraryEntry {
    /// Slug: `/` becomes `__` so one task is one directory.
    pub fn slug(task_id: &str) -> String {
        task_id.replace('/', "__")
    }

    /// One directory name for a domain, never a path.
    /// Mirrors the source exactly: collapse each separator plus the
    /// whitespace around it, then strip, so leading/trailing whitespace
    /// never leaves a dangling `__` edge.
    pub fn domain_dir(domain: &str) -> String {
        let mut out = String::new();
        let mut chars = domain.trim().chars().peekable();
        while let Some(c) = chars.next() {
            if c == '/' || c == '\\' {
                // Collapse the separator plus surrounding whitespace.
                while out.ends_with([' ', '\t']) {
                    out.pop();
                }
                while chars.peek().is_some_and(|n| *n == ' ' || *n == '\t') {
                    chars.next();
                }
                out.push_str("__");
            } else {
                out.push(c);
            }
        }
        let trimmed = out.trim().to_string();
        if trimmed.is_empty() {
            "unknown".to_string()
        } else {
            trimmed
        }
    }

    /// Relative entry dir under the domain dir: `harnesses/<slug>`.
    pub fn entry_rel_dir(&self) -> String {
        format!("harnesses/{}", Self::slug(&self.task_id))
    }
}

/// One-line census of the components actually in this harness.
/// Mirrors the source's `_inventory`: only valid component names count
/// (anchored like the surface regexes), the hooks file counts only when
/// the surface declares one, and the always-on char count is comma-grouped.
pub fn inventory(surface: &SurfaceSpec, tree: &Tree) -> String {
    let mut skills = 0;
    let mut sub_agents = 0;
    let mut scripts = 0;
    let skill_prefix = format!("{}/", surface.skill_dir);
    let plugin_prefix = surface.plugin_dir.map(|d| format!("{d}/"));
    let mut plugins = 0;
    let mut hooks = 0;
    for rel in tree.keys() {
        if rel.starts_with(&skill_prefix) && rel.ends_with("/SKILL.md") {
            let middle = &rel[skill_prefix.len()..rel.len() - "/SKILL.md".len()];
            if super::surface::valid_skill_name(middle) {
                skills += 1;
                continue;
            }
        }
        if rel.starts_with("sub_agents/") && rel.ends_with("/AGENT.md") {
            let middle = &rel["sub_agents/".len()..rel.len() - "/AGENT.md".len()];
            if super::surface::valid_skill_name(middle) {
                sub_agents += 1;
                continue;
            }
        }
        if super::surface::is_top_script(rel) {
            scripts += 1;
            continue;
        }
        if let Some(prefix) = &plugin_prefix
            && rel.starts_with(prefix)
            && rel.ends_with(".ts")
            && super::surface::valid_skill_name(&rel[prefix.len()..rel.len() - ".ts".len()])
        {
            plugins += 1;
            continue;
        }
        if surface.hooks_file.is_some() && Some(rel.as_str()) == surface.hooks_file {
            hooks += 1;
        }
    }
    let mut bits = Vec::new();
    if skills > 0 {
        bits.push(format!("{skills} skills"));
    }
    if sub_agents > 0 {
        bits.push(format!("{sub_agents} sub_agents"));
    }
    if scripts > 0 {
        bits.push(format!("{scripts} scripts"));
    }
    if plugins > 0 {
        bits.push(format!("{plugins} plugins"));
    }
    if hooks > 0 {
        bits.push(format!("{hooks} hooks"));
    }
    let bits = if bits.is_empty() {
        "always-on edits only".to_string()
    } else {
        bits.join(", ")
    };
    format!(
        "{bits}; {} always-on chars",
        comma_group(always_on_text(surface, tree).len())
    )
}

fn comma_group(n: usize) -> String {
    let digits: Vec<char> = n.to_string().chars().collect();
    let mut out = String::new();
    for (i, c) in digits.iter().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*c);
    }
    out
}

/// Render `EXPERIENCE.md`: why this harness was chosen + transferable lesson.
pub fn render_experience_md(surface: &SurfaceSpec, tree: &Tree, entry: &LibraryEntry) -> String {
    let mut lines = vec![
        format!("# {}", entry.task_id),
        String::new(),
        format!(
            "- adopted: `{}`{}",
            entry.label,
            if entry.run_id.is_empty() {
                String::new()
            } else {
                format!("  (run `{}`)", entry.run_id)
            }
        ),
        format!("- components: {}", inventory(surface, tree)),
        String::new(),
    ];
    if !entry.why.trim().is_empty() {
        lines.push("## Why this was chosen".to_string());
        lines.push(entry.why.trim().to_string());
        lines.push(String::new());
    }
    if let Some(edits) = entry.edits.as_deref().filter(|e| !e.trim().is_empty()) {
        lines.push("## What each edit did".to_string());
        lines.push(edits.trim().to_string());
        lines.push(String::new());
    }
    lines.push("## Transferable lesson".to_string());
    lines.push(format!("- **cue**: {}", entry.experience.cue));
    lines.push(format!("- **lesson**: {}", entry.experience.lesson));
    lines.push(format!("- **evidence**: {}", entry.experience.evidence));
    if let Some(conf) = &entry.experience.confidence
        && !conf.trim().is_empty()
    {
        lines.push(format!("- **confidence**: {}", conf.trim()));
    }
    lines.push(String::new());
    lines.join("\n")
}

/// One row of the domain index: entry slug, optional cue, and the
/// component inventory of the adopted harness at that entry.
#[derive(Debug, Clone)]
pub struct IndexRow<'a> {
    pub slug: &'a str,
    pub cue: Option<&'a str>,
    pub inventory: Option<&'a str>,
}

/// Render the domain `INDEX.md` from the entries on disk. Mirrors the
/// source's `rebuild_index`: each row names the harness dir, the why-file,
/// the component inventory, and the cue.
pub fn render_index_md(domain: &str, entries: &[IndexRow<'_>]) -> String {
    let mut rows = vec![
        format!("# Library — {domain}"),
        String::new(),
        "Each entry is an earlier task in THIS domain whose harness was chosen, stored "
            .to_string()
            + "with its FULL component tree and why it was picked. Browse this index, then "
            + "open the entries that look relevant to the matter in front of you.",
        String::new(),
    ];
    if entries.is_empty() {
        rows.push("_(empty — this is the first task in this domain)_".to_string());
    }
    let mut sorted: Vec<IndexRow<'_>> = entries.to_vec();
    sorted.sort_by_key(|row| row.slug.to_string());
    for row in sorted {
        rows.push(format!("## `harnesses/{}/`", row.slug));
        let inv = row.inventory.unwrap_or("").trim();
        if inv.is_empty() {
            rows.push(format!("- harness: `harnesses/{}/harness/`", row.slug));
        } else {
            rows.push(format!(
                "- harness: `harnesses/{}/harness/` — {}",
                row.slug, inv
            ));
        }
        rows.push(format!("- why:     `harnesses/{}/EXPERIENCE.md`", row.slug));
        if let Some(c) = row.cue
            && !c.trim().is_empty()
        {
            rows.push(format!("- cue:     {}", c.trim()));
        }
        rows.push(String::new());
    }
    rows.join("\n")
}

/// Library root for one run: `<base>/<benchmark>/k<k>-<variant>`.
/// An explicit `tag` opts into sharing one library across runs.
pub fn root_for(base: &str, benchmark: &str, k: usize, variant: &str, tag: &str) -> String {
    if !tag.is_empty() {
        return format!("{base}/{benchmark}/{tag}");
    }
    let mut suffix = format!("k{k}");
    if !variant.is_empty() {
        suffix.push('-');
        suffix.push_str(variant);
    }
    format!("{base}/{benchmark}/{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_tree() -> Tree {
        let mut t = HashMap::new();
        t.insert("systemprompt.md".to_string(), "base".to_string());
        t.insert(
            "skills/doc_review/SKILL.md".to_string(),
            "---\ndescription: x\n---\n".to_string(),
        );
        t
    }

    #[test]
    fn slugs_and_domain_dirs() {
        assert_eq!(LibraryEntry::slug("a/b/c"), "a__b__c");
        assert_eq!(
            LibraryEntry::domain_dir("antitrust / competition"),
            "antitrust__competition"
        );
        assert_eq!(LibraryEntry::domain_dir("  "), "unknown");
        assert_eq!(LibraryEntry::domain_dir("finance"), "finance");
        // Edge parity with the source regex `\\s*[/\\\\]\\s*` + strip:
        // surrounding whitespace never leaves a dangling `__` edge.
        assert_eq!(LibraryEntry::domain_dir("  pad  /  x  "), "pad__x");
        assert_eq!(LibraryEntry::domain_dir("/ a"), "__a");
        assert_eq!(LibraryEntry::domain_dir("a / "), "a__");
        assert_eq!(LibraryEntry::domain_dir("a\tb"), "a\tb");
    }

    #[test]
    fn inventory_counts_components() {
        let inv = inventory(&super::super::surface::OPENCODE_SURFACE, &test_tree());
        assert!(inv.contains("1 skills"), "{inv}");
        assert!(inv.contains("always-on chars"), "{inv}");
    }

    #[test]
    fn experience_md_carries_why_edits_and_lesson() {
        let exp =
            Experience::new("reconcile figures", "join before writing", "agg1 vs base").unwrap();
        let mut entry = LibraryEntry::new(
            "t/1",
            "finance",
            "agg1",
            "r1",
            "chosen for cleaner join",
            exp,
        );
        entry.edits = Some("added the doc_review skill".to_string());
        let md = render_experience_md(
            &super::super::surface::OPENCODE_SURFACE,
            &test_tree(),
            &entry,
        );
        assert!(md.contains("# t/1"), "{md}");
        assert!(md.contains("agg1"), "{md}");
        assert!(md.contains("**cue**"), "{md}");
        assert!(md.contains("chosen for cleaner join"), "{md}");
        assert!(md.contains("## What each edit did"), "{md}");
        assert!(md.contains("added the doc_review skill"), "{md}");
    }

    #[test]
    fn index_lists_entries_with_inventory_and_empty_state() {
        let empty = render_index_md("finance", &[]);
        assert!(empty.contains("first task in this domain"), "{empty}");
        let full = render_index_md(
            "finance",
            &[
                IndexRow {
                    slug: "t__1",
                    cue: Some("reconcile"),
                    inventory: Some("1 skills; 4 always-on chars"),
                },
                IndexRow {
                    slug: "t__2",
                    cue: None,
                    inventory: None,
                },
            ],
        );
        assert!(full.contains("harnesses/t__1/"), "{full}");
        assert!(full.contains("cue:     reconcile"), "{full}");
        assert!(full.contains("1 skills; 4 always-on chars"), "{full}");
    }

    #[test]
    fn root_for_matches_published_layout() {
        assert_eq!(root_for("runs", "lab", 3, "seq", ""), "runs/lab/k3-seq");
        assert_eq!(root_for("runs", "lab", 1, "", ""), "runs/lab/k1");
        assert_eq!(
            root_for("runs", "lab", 3, "seq", "shared"),
            "runs/lab/shared"
        );
    }
}
