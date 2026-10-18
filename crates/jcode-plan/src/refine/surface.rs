//! Editable c1..c8 harness component tree.
//!
//! Ported from harness2's `harness2/core/tree.py` (Apache-2.0, Google LLC).
//! The idea: a harness is a tree of files with different runtime costs.
//! Always-on prose competes with the agent's output budget on every run;
//! skills, scripts, subagents, and tool-loop hooks cost nothing until invoked.
//! Validators enforce the three rules that discard a whole proposal in
//! harness2's published runs: every skill needs a trigger line, no template
//! braces in always-on text, every named script must ship.
//!
//! Evidence: harness2 reports up to +11.5 pts (WorkBuddy-Bench), +4.5
//! (JobBench), +11.8pp (LAB) from this exact surface discipline; the most
//! common failure mode their prompts guard against is "no deliverable because
//! the model exhausted its output budget on reasoning".

use std::collections::{HashMap, HashSet};

/// A harness tree: relative path -> file content.
pub type Tree = HashMap<String, String>;

/// Files the runtime provides; proposals must not shadow them.
pub const RESERVED_SCRIPTS: &[&str] = &["read_doc.py", "pack.py", "unpack.py"];

/// Editable surface of one agent framework: paths, caps, per-surface rules.
/// Mirrors harness2's `SurfaceSpec` (paths, caps, frontmatter rules).
#[derive(Debug, Clone)]
pub struct SurfaceSpec {
    pub name: &'static str,
    pub always_on: &'static [&'static str],
    pub always_on_max: usize,
    pub always_on_cap_reason: &'static str,
    pub skill_dir: &'static str,
    pub plugin_dir: Option<&'static str>,
    pub hooks_file: Option<&'static str>,
    pub autotrigger_file: &'static str,
    pub trigger_line_prefix: &'static str,
    pub subagent_requires_mode: bool,
    pub reject_empty_always_on: bool,
}

impl SurfaceSpec {
    pub fn skill_prefix(&self) -> String {
        format!("{}/", self.skill_dir)
    }
}

/// OpenCode surface (c1..c8): 4 always-on files, 24k cap.
pub const OPENCODE_SURFACE: SurfaceSpec = SurfaceSpec {
    name: "opencode",
    always_on: &[
        "systemprompt.md",
        "guardrails.md",
        "LongTermMEMORY.md",
        "tool_descriptions/tool_guidance.md",
    ],
    always_on_max: 24_000,
    always_on_cap_reason: "every token here competes with the agent's own output budget",
    skill_dir: "skills",
    plugin_dir: Some("plugin"),
    hooks_file: None,
    autotrigger_file: "guardrails.md",
    trigger_line_prefix: "- When ",
    subagent_requires_mode: true,
    reject_empty_always_on: false,
};

/// Codex surface: single always-on AGENTS.md, 8k cap (CLI truncates at 32k).
pub const CODEX_SURFACE: SurfaceSpec = SurfaceSpec {
    name: "codex",
    always_on: &["AGENTS.md"],
    always_on_max: 8_000,
    always_on_cap_reason: "AGENTS.md is injected whole into EVERY turn",
    skill_dir: ".agents/skills",
    plugin_dir: None,
    hooks_file: Some(".codex/hooks.json"),
    autotrigger_file: "AGENTS.md",
    trigger_line_prefix: "- When ",
    subagent_requires_mode: false,
    reject_empty_always_on: true,
};

/// Normalise a relative path, rejecting escapes before it is joined.
pub fn normalize_rel(rel: &str) -> Result<String, String> {
    let trimmed = rel.trim().replace('\\', "/");
    // Absolute paths are rejected before any trimming: trim_matches would
    // otherwise strip the leading `/` and hide the escape.
    if trimmed.starts_with('/') {
        return Err(format!("path escapes the harness: {rel:?}"));
    }
    let mut r = trimmed;
    while r.starts_with("./") {
        r = r[2..].to_string();
    }
    let r = r.trim_matches('/').to_string();
    if r.is_empty() {
        return Err(format!("path escapes the harness: {rel:?}"));
    }
    if r.split('/').any(|part| part == "..") {
        return Err(format!("path escapes the harness: {rel:?}"));
    }
    Ok(r)
}

fn is_skill_script(surface: &SurfaceSpec, rel: &str) -> bool {
    let prefix = surface.skill_prefix();
    rel.starts_with(&prefix) && rel.ends_with("/SKILL.md") && {
        let middle = &rel[prefix.len()..rel.len() - "/SKILL.md".len()];
        valid_name(middle)
    }
}

/// A skill-local helper: `<skill_dir>/<name>/scripts/<file>`.
fn is_skill_helper_script(surface: &SurfaceSpec, rel: &str) -> bool {
    let prefix = surface.skill_prefix();
    if !rel.starts_with(&prefix) {
        return false;
    }
    let rest = &rel[prefix.len()..];
    let Some(slash) = rest.find('/') else {
        return false;
    };
    let (name, tail) = rest.split_at(slash);
    if !valid_name(name) || !tail.starts_with("/scripts/") {
        return false;
    }
    let file = &tail["/scripts/".len()..];
    // Mirror harness2's skill_script pattern: word chars, dots, dashes.
    !file.is_empty()
        && !file.contains('/')
        && file
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}

/// Scan prose for `python <path>` / `python3 <path>` invocations of
/// `scripts/*.py` (harness2's `script_ref` pattern). Returns the referenced
/// rels in order of first appearance.
fn script_refs(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while cursor < src.len() {
        let Some(rel_p) = src[cursor..].find("python") else {
            break;
        };
        let base = cursor + rel_p;
        let mut after = &src[base + "python".len()..];
        after = after.strip_prefix('3').unwrap_or(after);
        let Some(raw_tail) = after.strip_prefix(' ').or_else(|| after.strip_prefix('\t')) else {
            // Not a `python <path>` invocation; skip past this occurrence.
            cursor = base + "python".len();
            continue;
        };
        // Runs of whitespace between the interpreter and the path are fine.
        let tail: &str = raw_tail.trim_start_matches([' ', '\t']);
        let token: String = tail
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '"' && *c != '\'' && *c != '`')
            .collect();
        let token = token.trim_matches(|c| c == '(' || c == ')' || c == ',' || c == ';');
        if token.contains("scripts/") && token.ends_with(".py") {
            let t = token
                .trim_start_matches("./")
                .trim_start_matches('/')
                .to_string();
            if !out.contains(&t) {
                out.push(t);
            }
        }
        // `tail` extends to the end of `src` (only front-trimmed), so its
        // start index is `src.len() - tail.len()`; advance past the token.
        // The start is always ahead of `base`, so progress is guaranteed.
        cursor = (src.len() - tail.len() + token.len()).max(base + 1);
    }
    out
}

fn valid_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn is_subagent(rel: &str) -> bool {
    rel.starts_with("sub_agents/") && rel.ends_with("/AGENT.md") && {
        let middle = &rel["sub_agents/".len()..rel.len() - "/AGENT.md".len()];
        valid_name(middle)
    }
}

fn is_script(rel: &str) -> bool {
    if !rel.starts_with("scripts/") || !rel.ends_with(".py") {
        return false;
    }
    let stem = &rel["scripts/".len()..rel.len() - ".py".len()];
    !stem.is_empty()
        && stem
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && (stem
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit()))
}

fn is_plugin(surface: &SurfaceSpec, rel: &str) -> bool {
    match surface.plugin_dir {
        Some(dir) => {
            let prefix = format!("{dir}/");
            rel.starts_with(&prefix)
                && rel.ends_with(".ts")
                && valid_name(&rel[prefix.len()..rel.len() - ".ts".len()])
        }
        None => false,
    }
}

/// Whether this path is on the editable surface. A predicate: never raises.
pub fn is_allowed(surface: &SurfaceSpec, rel: &str) -> bool {
    let rel = match normalize_rel(rel) {
        Ok(r) => r,
        Err(_) => return false,
    };
    if surface.always_on.contains(&rel.as_str()) {
        return true;
    }
    if Some(rel.as_str()) == surface.hooks_file {
        return true;
    }
    if is_skill_script(surface, &rel) || is_subagent(&rel) || is_script(&rel) {
        return true;
    }
    // Skill-local helper scripts travel with their skill.
    if rel.contains("/scripts/") && rel.starts_with(&surface.skill_prefix()) {
        return true;
    }
    is_plugin(surface, &rel)
}

pub fn skill_names(surface: &SurfaceSpec, tree: &Tree) -> Vec<String> {
    let prefix = surface.skill_prefix();
    let mut out = HashSet::new();
    for rel in tree.keys() {
        if rel.starts_with(&prefix) && rel.ends_with("/SKILL.md") {
            let middle = &rel[prefix.len()..rel.len() - "/SKILL.md".len()];
            if valid_name(middle) {
                out.insert(middle.to_string());
            }
        }
    }
    let mut v: Vec<String> = out.into_iter().collect();
    v.sort();
    v
}

pub fn subagent_names(tree: &Tree) -> Vec<String> {
    let mut out = HashSet::new();
    for rel in tree.keys() {
        if is_subagent(rel) {
            let middle = &rel["sub_agents/".len()..rel.len() - "/AGENT.md".len()];
            out.insert(middle.to_string());
        }
    }
    let mut v: Vec<String> = out.into_iter().collect();
    v.sort();
    v
}

pub fn always_on_text(surface: &SurfaceSpec, tree: &Tree) -> String {
    surface
        .always_on
        .iter()
        .filter_map(|a| tree.get(*a).map(String::as_str))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Give every skill a trigger line in always-on text; return repaired names.
/// A line that would push the body over the cap is skipped.
pub fn autotrigger(surface: &SurfaceSpec, tree: &mut Tree) -> Vec<String> {
    let mut fixed = Vec::new();
    for name in skill_names(surface, tree) {
        let body = always_on_text(surface, tree);
        if body.contains(&name) {
            continue;
        }
        let desc = tree
            .get(&format!("{}/{name}/SKILL.md", surface.skill_dir))
            .and_then(|fm| {
                fm.lines().find_map(|l| {
                    l.strip_prefix("description:")
                        .map(|d| d.trim().trim_end_matches('.').to_string())
                })
            })
            .unwrap_or_else(|| format!("the {name} workflow applies"));
        let line = format!(
            "{}when {desc}, load the `{name}` skill.\n",
            surface.trigger_line_prefix
        );
        if body.len() + line.len() + 2 > surface.always_on_max {
            continue;
        }
        let cur = tree
            .get(surface.autotrigger_file)
            .cloned()
            .unwrap_or_default();
        let next = if cur.trim().is_empty() {
            line
        } else {
            format!("{}\n{line}", cur.trim_end())
        };
        tree.insert(surface.autotrigger_file.to_string(), next);
        fixed.push(name);
    }
    fixed
}

/// Census returned on successful validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeCensus {
    pub always_on_chars: usize,
    pub n_files: usize,
    pub skills: usize,
    pub sub_agents: usize,
    pub scripts: usize,
}

/// Validate a proposed tree. `Err` lists every problem in actionable prose.
pub fn validate(
    surface: &SurfaceSpec,
    tree: &Tree,
    allow_subagents: bool,
) -> Result<TreeCensus, String> {
    let mut errs: Vec<String> = Vec::new();
    let body = always_on_text(surface, tree);

    if body.len() > surface.always_on_max {
        errs.push(format!(
            "always-on content is {} chars, over the {} cap; {}",
            body.len(),
            surface.always_on_max,
            surface.always_on_cap_reason
        ));
    }
    if surface.reject_empty_always_on && body.trim().is_empty() {
        errs.push(format!(
            "{} is empty: it is the only always-on file, so an empty one is a harness that does not exist",
            surface.always_on[0]
        ));
    }

    // Every skill: frontmatter description + trigger line.
    let skills = skill_names(surface, tree);
    for name in &skills {
        let rel = format!("{}/{}/SKILL.md", surface.skill_dir, name);
        let fm = tree.get(&rel).map(|s| s.as_str()).unwrap_or("");
        let head: String = fm.chars().take(600).collect();
        if !head.lines().any(|l| {
            l.strip_prefix("description:")
                .is_some_and(|d| !d.trim().is_empty())
        }) {
            errs.push(format!(
                "{rel} needs a `description:` in its frontmatter, or the skill is dropped SILENTLY at load"
            ));
        }
        if !body.contains(name) {
            errs.push(format!(
                "skill '{name}' is never named in the always-on text, so the agent will not know to invoke it: add a trigger line naming `{name}`"
            ));
        }
    }

    // Subagents.
    let subs = subagent_names(tree);
    if !subs.is_empty() && !allow_subagents {
        errs.push(format!(
            "sub_agents are not available on this deployment: {subs:?}"
        ));
    }
    for name in &subs {
        let rel = format!("sub_agents/{name}/AGENT.md");
        let fm = tree.get(&rel).map(|s| s.as_str()).unwrap_or("");
        let head: String = fm.chars().take(600).collect();
        if surface.subagent_requires_mode && !head.lines().any(|l| l.trim() == "mode: subagent") {
            errs.push(format!("{rel} needs frontmatter `mode: subagent`"));
        }
        if !head.lines().any(|l| {
            l.strip_prefix("description:")
                .is_some_and(|d| !d.trim().is_empty())
        }) {
            errs.push(format!("{rel} needs a `description:`"));
        }
    }

    // Reserved script shadowing.
    for rel in tree.keys() {
        if let Some(base) = rel.rsplit('/').next()
            && RESERVED_SCRIPTS.contains(&base)
            && is_script(rel)
        {
            errs.push(format!(
                "{rel} shadows a runtime-provided script; rename it"
            ));
        }
    }

    // Every script a skill or always-on file invokes must ship in the same
    // proposal (harness2's `_check_script_refs`): a skill invoking a missing
    // file fails validation and the whole proposal is thrown away. Runtime
    // helpers (read_doc.py / pack.py / unpack.py) are exempt, as is a skill
    // calling its own carried helper at `<skill>/scripts/<file>`.
    {
        let mut have: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for rel in tree.keys() {
            if is_script(rel) || is_skill_helper_script(surface, rel) {
                have.insert(rel.as_str());
            }
        }
        let skill_prefix = surface.skill_prefix();
        let mut check_files: Vec<&String> = tree
            .keys()
            .filter(|rel| {
                is_skill_script(surface, rel)
                    || surface.always_on.contains(&rel.as_str())
                    || Some(rel.as_str()) == surface.hooks_file
            })
            .collect();
        check_files.sort();
        for rel in check_files {
            let src = &tree[rel];
            // Jinja guard (harness2's NO TEMPLATE BRACES rule): always-on
            // files render through Jinja with StrictUndefined, so a doubled
            // `{{` placeholder or `{%` / `{#` tag kills the job pre-trial.
            if surface.always_on.contains(&rel.as_str())
                && (src.contains("{{") || src.contains("{%") || src.contains("{#"))
            {
                errs.push(format!(
                    "{rel} contains a Jinja brace pair (`{{{{`, `{{%`, or `{{#`); always-on text renders with StrictUndefined, so any such braces kill the job pre-trial. Use a single brace, or words"
                ));
            }
            for reference in script_refs(src) {
                let base = reference.rsplit('/').next().unwrap_or(&reference);
                if RESERVED_SCRIPTS.contains(&base) {
                    continue;
                }
                // A skill-local ref (`skills/<name>/scripts/f.py`) resolves
                // against the skill's own helper; anything else must ship
                // at exactly the referenced rel.
                let mut resolved = reference.clone();
                if !have.contains(resolved.as_str()) && rel.starts_with(&skill_prefix) {
                    let skill_name = rel[skill_prefix.len()..].split('/').next().unwrap_or("");
                    let candidate = format!("{skill_prefix}{skill_name}/scripts/{base}");
                    if have.contains(candidate.as_str()) {
                        resolved = candidate;
                    }
                }
                if !have.contains(resolved.as_str()) {
                    errs.push(format!(
                        "{rel} invokes `{reference}` but that file is not in the harness"
                    ));
                }
            }
        }
    }

    if errs.is_empty() {
        Ok(TreeCensus {
            always_on_chars: body.len(),
            n_files: tree.len(),
            skills: skills.len(),
            sub_agents: subs.len(),
            scripts: tree.keys().filter(|r| is_script(r)).count(),
        })
    } else {
        Err(errs.join(" | "))
    }
}

/// Added/removed/modified rels plus always-on char delta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub modified: Vec<String>,
    pub always_on_char_delta: i64,
}

pub fn diff_summary(surface: &SurfaceSpec, before: &Tree, after: &Tree) -> TreeDiff {
    let mut added: Vec<String> = after
        .keys()
        .filter(|k| !before.contains_key(*k))
        .cloned()
        .collect();
    let mut removed: Vec<String> = before
        .keys()
        .filter(|k| !after.contains_key(*k))
        .cloned()
        .collect();
    let mut modified: Vec<String> = after
        .iter()
        .filter(|(k, v)| before.get(*k).is_some_and(|old| old != *v))
        .map(|(k, _)| k.clone())
        .collect();
    added.sort();
    removed.sort();
    modified.sort();
    TreeDiff {
        added,
        removed,
        modified,
        always_on_char_delta: always_on_text(surface, after).len() as i64
            - always_on_text(surface, before).len() as i64,
    }
}

/// Pairwise line-Jaccard over candidate trees, so collapse to one body is visible.
#[derive(Debug, Clone, PartialEq)]
pub struct Diversity {
    pub n: usize,
    pub n_distinct: usize,
    pub mean_line_jaccard: f64,
}

pub fn diversity(trees: &HashMap<String, Tree>) -> Diversity {
    let names: Vec<&String> = {
        let mut n: Vec<&String> = trees.keys().collect();
        n.sort();
        n
    };
    if names.len() < 2 {
        return Diversity {
            n: names.len(),
            n_distinct: names.len(),
            mean_line_jaccard: 1.0,
        };
    }
    let sets: HashMap<&str, HashSet<String>> = names
        .iter()
        .map(|n| {
            let t = &trees[*n];
            let mut s = HashSet::new();
            for (rel, content) in t {
                for ln in content.lines() {
                    let ln = ln.trim();
                    if !ln.is_empty() {
                        s.insert(format!("{rel}:{ln}"));
                    }
                }
            }
            (n.as_str(), s)
        })
        .collect();
    let mut sims = Vec::new();
    for (i, a) in names.iter().enumerate() {
        for b in names.iter().skip(i + 1) {
            let sa = &sets[a.as_str()];
            let sb = &sets[b.as_str()];
            let union = sa.union(sb).count();
            let inter = sa.intersection(sb).count();
            sims.push(if union == 0 {
                1.0
            } else {
                inter as f64 / union as f64
            });
        }
    }
    let distinct = {
        let mut sigs: HashSet<String> = HashSet::new();
        for t in trees.values() {
            let mut items: Vec<(&String, &String)> = t.iter().collect();
            items.sort();
            sigs.insert(format!("{items:?}"));
        }
        sigs.len()
    };
    Diversity {
        n: names.len(),
        n_distinct: distinct,
        mean_line_jaccard: (sims.iter().sum::<f64>() / sims.len() as f64 * 10_000.0).round()
            / 10_000.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(pairs: &[(&str, &str)]) -> Tree {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn normalize_rejects_escapes() {
        assert!(normalize_rel("../evil.md").is_err());
        assert!(normalize_rel("/abs.md").is_err());
        assert_eq!(normalize_rel("./a.md").unwrap(), "a.md");
        assert_eq!(normalize_rel("a//b.md").unwrap(), "a//b.md");
    }

    #[test]
    fn allowed_covers_always_on_skills_scripts() {
        assert!(is_allowed(&OPENCODE_SURFACE, "systemprompt.md"));
        assert!(is_allowed(&OPENCODE_SURFACE, "skills/doc_review/SKILL.md"));
        assert!(is_allowed(&OPENCODE_SURFACE, "scripts/check_totals.py"));
        assert!(is_allowed(&OPENCODE_SURFACE, "sub_agents/scout/AGENT.md"));
        assert!(is_allowed(&OPENCODE_SURFACE, "plugin/trim.ts"));
        assert!(!is_allowed(&OPENCODE_SURFACE, "AGENTS.md"));
        assert!(!is_allowed(&OPENCODE_SURFACE, "../escape.md"));
    }

    #[test]
    fn validate_rejects_untriggered_skill() {
        let t = tree(&[
            ("systemprompt.md", "do work"),
            (
                "skills/lonely/SKILL.md",
                "---\ndescription: a workflow\n---\nbody",
            ),
        ]);
        let err = validate(&OPENCODE_SURFACE, &t, false).unwrap_err();
        assert!(err.contains("never named in the always-on text"), "{err}");
    }

    #[test]
    fn validate_accepts_triggered_skill_and_census() {
        let t = tree(&[
            (
                "systemprompt.md",
                "When reviewing docs, load the `doc_review` skill.",
            ),
            (
                "skills/doc_review/SKILL.md",
                "---\ndescription: review docs\n---\nbody",
            ),
        ]);
        let census = validate(&OPENCODE_SURFACE, &t, false).unwrap();
        assert_eq!(census.skills, 1);
        assert_eq!(census.n_files, 2);
    }

    #[test]
    fn validate_enforces_always_on_cap_and_codex_nonempty() {
        let big = "x".repeat(OPENCODE_SURFACE.always_on_max + 1);
        let t = tree(&[("systemprompt.md", big.as_str())]);
        assert!(
            validate(&OPENCODE_SURFACE, &t, false)
                .unwrap_err()
                .contains("over the")
        );
        let empty: Tree = HashMap::new();
        assert!(
            validate(&CODEX_SURFACE, &empty, false)
                .unwrap_err()
                .contains("does not exist")
        );
    }

    #[test]
    fn autotrigger_repairs_missing_trigger_within_cap() {
        let mut t = tree(&[
            ("systemprompt.md", "base"),
            (
                "skills/doc_review/SKILL.md",
                "---\ndescription: review documents\n---\nbody",
            ),
        ]);
        let fixed = autotrigger(&OPENCODE_SURFACE, &mut t);
        // autotrigger_file for opencode is guardrails.md, not systemprompt.md
        assert_eq!(fixed, vec!["doc_review".to_string()]);
        assert!(t["guardrails.md"].contains("doc_review"));
    }

    #[test]
    fn diff_and_diversity_behave() {
        let a = tree(&[("systemprompt.md", "v1")]);
        let b = tree(&[("systemprompt.md", "v2"), ("scripts/a.py", "x")]);
        let d = diff_summary(&OPENCODE_SURFACE, &a, &b);
        assert_eq!(d.added, vec!["scripts/a.py".to_string()]);
        assert_eq!(d.modified, vec!["systemprompt.md".to_string()]);
        let mut m = HashMap::new();
        m.insert("one".to_string(), a.clone());
        m.insert("two".to_string(), b.clone());
        let div = diversity(&m);
        assert_eq!(div.n, 2);
        assert!(div.mean_line_jaccard < 1.0, "{div:?}");
    }

    #[test]
    fn script_refs_scan_python_invocations() {
        assert_eq!(
            script_refs("run python scripts/check_totals.py now"),
            vec!["scripts/check_totals.py".to_string()]
        );
        assert_eq!(
            script_refs("run python3  skills/x/scripts/h.py end"),
            vec!["skills/x/scripts/h.py".to_string()]
        );
        // `python` without a following path is not an invocation.
        assert!(script_refs("i love python programming").is_empty());
        // Non-script paths are ignored.
        assert!(script_refs("run python notes/readme.txt").is_empty());
    }

    #[test]
    fn validate_rejects_missing_invoked_script() {
        let t = tree(&[
            (
                "systemprompt.md",
                "When reviewing docs, load the `doc_review` skill.",
            ),
            (
                "skills/doc_review/SKILL.md",
                "---\ndescription: review docs\n---\nrun python scripts/missing.py",
            ),
        ]);
        let err = validate(&OPENCODE_SURFACE, &t, false).unwrap_err();
        assert!(err.contains("invokes `scripts/missing.py`"), "{err}");
    }

    #[test]
    fn validate_accepts_shipped_and_skill_local_scripts() {
        let t = tree(&[
            (
                "systemprompt.md",
                "When reviewing docs, load the `doc_review` skill. Run python scripts/shared.py.",
            ),
            (
                "skills/doc_review/SKILL.md",
                "---\ndescription: review docs\n---\nrun python scripts/helper.py and python scripts/shared.py",
            ),
            ("scripts/shared.py", "print(1)"),
            ("skills/doc_review/scripts/helper.py", "print(2)"),
        ]);
        validate(&OPENCODE_SURFACE, &t, false).expect("shipped scripts pass");
    }

    #[test]
    fn validate_rejects_jinja_braces_in_always_on() {
        let t = tree(&[("systemprompt.md", "use {{ instruction }} here")]);
        let err = validate(&OPENCODE_SURFACE, &t, false).unwrap_err();
        assert!(err.contains("Jinja brace pair"), "{err}");
        // Braces inside a skill body are fine: only always-on renders via Jinja.
        let ok = tree(&[
            ("systemprompt.md", "plain text"),
            (
                "skills/s/SKILL.md",
                "---\ndescription: s skill\n---\ntemplate {{ x }} lives here",
            ),
        ]);
        // Still fails on the missing trigger, not on braces: prove braces
        // in skills are not the reported problem.
        let err = validate(&OPENCODE_SURFACE, &ok, false).unwrap_err();
        assert!(!err.contains("Jinja brace pair"), "{err}");
    }
}
