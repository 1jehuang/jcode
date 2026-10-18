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
}

impl LibraryEntry {
    /// Slug: `/` becomes `__` so one task is one directory.
    pub fn slug(task_id: &str) -> String {
        task_id.replace('/', "__")
    }

    /// One directory name for a domain, never a path.
    pub fn domain_dir(domain: &str) -> String {
        let mut out = String::new();
        let mut chars = domain.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '/' || c == '\\' {
                // Collapse the separator plus surrounding whitespace.
                while out.ends_with(' ') {
                    out.pop();
                }
                while chars.peek().is_some_and(|n| *n == ' ') {
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
pub fn inventory(surface: &SurfaceSpec, tree: &Tree) -> String {
    let mut skills = 0;
    let mut sub_agents = 0;
    let mut scripts = 0;
    let mut plugins = 0;
    let skill_prefix = format!("{}/", surface.skill_dir);
    let plugin_prefix = surface.plugin_dir.map(|d| format!("{d}/"));
    let mut hooks = 0;
    for rel in tree.keys() {
        if rel.starts_with(&skill_prefix) && rel.ends_with("/SKILL.md") {
            skills += 1;
        } else if rel.starts_with("sub_agents/") && rel.ends_with("/AGENT.md") {
            sub_agents += 1;
        } else if rel.starts_with("scripts/") && rel.ends_with(".py") {
            scripts += 1;
        } else if let Some(prefix) = &plugin_prefix
            && rel.starts_with(prefix)
            && rel.ends_with(".ts")
        {
            plugins += 1;
        } else if Some(rel.as_str()) == surface.hooks_file {
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
        always_on_text(surface, tree).len()
    )
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

/// Render the domain `INDEX.md` from the entries on disk (slugs + cues).
pub fn render_index_md(domain: &str, entries: &[(&str, Option<&str>)]) -> String {
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
    let mut sorted: Vec<(&str, Option<&str>)> = entries.to_vec();
    sorted.sort_by_key(|(slug, _)| slug.to_string());
    for (slug, cue) in sorted {
        rows.push(format!("## `harnesses/{slug}/`"));
        rows.push(format!("- harness: `harnesses/{slug}/harness/`"));
        rows.push(format!("- why:     `harnesses/{slug}/EXPERIENCE.md`"));
        if let Some(c) = cue
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
    }

    #[test]
    fn inventory_counts_components() {
        let inv = inventory(&super::super::surface::OPENCODE_SURFACE, &test_tree());
        assert!(inv.contains("1 skills"), "{inv}");
        assert!(inv.contains("always-on chars"), "{inv}");
    }

    #[test]
    fn experience_md_carries_why_and_lesson() {
        let exp =
            Experience::new("reconcile figures", "join before writing", "agg1 vs base").unwrap();
        let entry = LibraryEntry {
            task_id: "t/1".to_string(),
            domain: "finance".to_string(),
            label: "agg1".to_string(),
            run_id: "r1".to_string(),
            why: "chosen for cleaner join".to_string(),
            experience: exp,
        };
        let md = render_experience_md(
            &super::super::surface::OPENCODE_SURFACE,
            &test_tree(),
            &entry,
        );
        assert!(md.contains("# t/1"), "{md}");
        assert!(md.contains("agg1"), "{md}");
        assert!(md.contains("**cue**"), "{md}");
        assert!(md.contains("chosen for cleaner join"), "{md}");
    }

    #[test]
    fn index_lists_entries_and_empty_state() {
        let empty = render_index_md("finance", &[]);
        assert!(empty.contains("first task in this domain"), "{empty}");
        let full = render_index_md("finance", &[("t__1", Some("reconcile")), ("t__2", None)]);
        assert!(full.contains("harnesses/t__1/"), "{full}");
        assert!(full.contains("cue:     reconcile"), "{full}");
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
