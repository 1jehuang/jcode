use anyhow::Result;
use chrono::Utc;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(not(test))]
use std::sync::OnceLock;
use tokio::sync::RwLock;

mod invocation;
pub use invocation::SkillInvocation;

/// A skill definition from SKILL.md
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub allowed_tools: Option<Vec<String>>,
    pub content: String,
    pub path: PathBuf,
    search_text: String,
}

#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
    #[serde(rename = "allowed-tools")]
    allowed_tools: Option<AllowedTools>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AllowedTools {
    CommaDelimited(String),
    Sequence(Vec<String>),
}

/// Registry of available skills
#[derive(Debug, Default, Clone)]
pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
}

/// Maximum directory depth scanned under a Claude Code plugin root when
/// looking for `skills/<name>/SKILL.md` entries. Plugin layouts vary across
/// Claude Code versions (`cache/<marketplace>/<plugin>/<version>/skills/...`,
/// `repos/<owner>/<repo>/skills/...`, nested `.claude/skills/...`), so we scan
/// defensively but with a bound to avoid walking arbitrarily deep trees.
const PLUGIN_SCAN_MAX_DEPTH: usize = 5;

/// Outcome of importing one external skills directory.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct SkillCopyStats {
    copied: usize,
    skipped: usize,
    failed: usize,
}

impl SkillRegistry {
    /// Process-wide shared mutable registry used by both `skill_manage` and
    /// direct slash invocation paths. Keeping a single registry prevents slash
    /// commands from seeing a stale startup-only skill snapshot after reloads.
    ///
    /// Holds GLOBAL skills only (plugins, `~/.jcode/skills/`,
    /// `~/.agents/skills/`). Project-local skills are a per-session overlay
    /// composed at read time from the session's workspace root (issue #457);
    /// they must never enter this shared registry, and the daemon's startup
    /// cwd must never influence its contents.
    pub fn shared_registry() -> Arc<RwLock<Self>> {
        #[cfg(test)]
        {
            Arc::new(RwLock::new(Self::load_global().unwrap_or_default()))
        }

        #[cfg(not(test))]
        {
            static SHARED: OnceLock<Arc<RwLock<SkillRegistry>>> = OnceLock::new();
            SHARED
                .get_or_init(|| {
                    Arc::new(RwLock::new(
                        SkillRegistry::load_global().unwrap_or_default(),
                    ))
                })
                .clone()
        }
    }

    /// Load a process-wide shared immutable snapshot of global skills for
    /// startup paths that only need read access.
    pub fn shared_snapshot() -> Arc<Self> {
        #[cfg(test)]
        {
            Arc::new(Self::load_global().unwrap_or_default())
        }

        #[cfg(not(test))]
        {
            if let Ok(skills) = Self::shared_registry().try_read() {
                Arc::new(skills.clone())
            } else {
                Arc::new(SkillRegistry::load_global().unwrap_or_default())
            }
        }
    }

    /// Import skills from Claude Code and Codex CLI on first run.
    /// Only runs if ~/.jcode/skills/ doesn't exist yet.
    fn import_from_external() {
        let jcode_skills = match crate::storage::jcode_dir() {
            Ok(dir) => dir.join("skills"),
            Err(_) => return,
        };

        if jcode_skills.exists() {
            return; // Not first run
        }

        let mut sources = Vec::new();
        let mut copied = Vec::new();

        // Import from Claude Code (~/.claude/skills/)
        if let Ok(claude_skills) = crate::storage::user_home_path(".claude/skills")
            && claude_skills.is_dir()
        {
            let count = Self::copy_skills_dir(&claude_skills, &jcode_skills).copied;
            if count > 0 {
                sources.push(format!("{} from Claude Code", count));
                copied.extend(Self::list_skill_names(&jcode_skills));
            }
        }

        // Import from Codex CLI (~/.codex/skills/)
        if let Ok(codex_skills) = crate::storage::user_home_path(".codex/skills")
            && codex_skills.is_dir()
        {
            let count = Self::copy_skills_dir(&codex_skills, &jcode_skills).copied;
            if count > 0 {
                sources.push(format!("{} from Codex CLI", count));
                copied.extend(Self::list_skill_names(&jcode_skills));
            }
        }

        if !sources.is_empty() {
            // Deduplicate names
            copied.sort();
            copied.dedup();
            crate::logging::info(&format!(
                "Skills: Imported {} ({}) from {}",
                copied.len(),
                copied.join(", "),
                sources.join(" + "),
            ));
        }
    }

    /// Copy skill directories from src to dst.
    fn copy_skills_dir(src: &Path, dst: &Path) -> SkillCopyStats {
        let mut stats = SkillCopyStats::default();
        let entries = match std::fs::read_dir(src) {
            Ok(e) => e,
            Err(_) => return stats,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            // Skip Codex system skills
            if name.starts_with('.') {
                continue;
            }

            // Only copy if SKILL.md exists
            if !path.join("SKILL.md").exists() {
                continue;
            }

            let dest = dst.join(&name);
            // An earlier source (e.g. ~/.claude/skills) may already have imported
            // this skill, often because ~/.codex/skills links to the same
            // directory. Copying over it fails on read-only files such as
            // `.git/objects/**` (issue #1404), and the first copy wins anyway.
            if dest.exists() {
                stats.skipped += 1;
                continue;
            }
            if let Err(e) = Self::copy_dir_recursive(&path, &dest) {
                crate::logging::error(&format!("Failed to copy skill '{}': {}", name, e));
                stats.failed += 1;
                continue;
            }
            stats.copied += 1;
        }
        stats
    }

    /// Recursively copy a directory
    fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());

            if src_path.is_dir() {
                Self::copy_dir_recursive(&src_path, &dst_path)?;
            } else if src_path.is_symlink() {
                // Resolve symlink and copy the target
                let target = std::fs::read_link(&src_path)?;
                // Try to create symlink, fall back to copying the file
                if crate::platform::symlink_or_copy(&target, &dst_path).is_err()
                    && let Ok(resolved) = std::fs::canonicalize(&src_path)
                {
                    std::fs::copy(&resolved, &dst_path)?;
                }
            } else {
                std::fs::copy(&src_path, &dst_path)?;
            }
        }
        Ok(())
    }

    /// List skill directory names
    fn list_skill_names(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .ok()
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| e.file_name().to_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Load only the shared global skill sources: Claude Code plugin installs,
    /// `~/.jcode/skills/`, and `~/.agents/skills/`.
    ///
    /// This is what the process-wide shared registry holds. Project-local
    /// skills are intentionally excluded: they are a per-session overlay
    /// resolved from the active session's workspace root (issue #457), never
    /// from the daemon's startup cwd, and never shared across sessions.
    pub fn load_global() -> Result<Self> {
        // First-run import from Claude Code / Codex CLI
        Self::import_from_external();

        let mut registry = Self::default();

        // Load skills provided by Claude Code plugins/marketplace installs
        // first, so explicit jcode/agents skills with the same name win below.
        if let Some(plugins_root) = Self::claude_plugins_root() {
            registry.load_plugin_skills_from_root(&plugins_root);
        }

        // Load from ~/.jcode/skills/ (jcode's own global skills)
        if let Ok(jcode_dir) = crate::storage::jcode_dir() {
            let jcode_skills = jcode_dir.join("skills");
            if jcode_skills.exists() {
                registry.load_from_dir(&jcode_skills)?;
            }
        }

        // Load from ~/.agents/skills/ (shared cross-tool `.agents` convention)
        if let Ok(agents_skills) = crate::storage::user_home_path(".agents/skills")
            && agents_skills.exists()
        {
            registry.load_from_dir(&agents_skills)?;
        }

        Ok(registry)
    }

    /// Load only the project-local skill overlay for a workspace root:
    /// `./.jcode/skills/`, `./.agents/skills/`, and `./.claude/skills/`.
    ///
    /// Loaded fresh from disk on access so edits are visible without daemon
    /// restarts and two sessions in different repositories never see each
    /// other's project skills. `working_dir = None` yields an empty overlay: a
    /// session with no workspace root has no project.
    pub fn load_project_overlay(working_dir: Option<&Path>) -> Result<Self> {
        let mut overlay = Self::default();
        overlay.load_project_local_dirs(working_dir)?;
        Ok(overlay)
    }

    /// Merge a project-local overlay into this registry. Overlay skills win
    /// over same-named global skills, mirroring the historical load order
    /// (project dirs loaded last).
    pub fn merge_overlay(&mut self, overlay: Self) {
        self.skills.extend(overlay.skills);
    }

    /// Effective skills for a session: shared global skills plus the
    /// project-local overlay for the session's workspace root.
    pub fn effective_for_working_dir(base: &Self, working_dir: Option<&Path>) -> Self {
        let mut effective = base.clone();
        if let Ok(overlay) = Self::load_project_overlay(working_dir) {
            effective.merge_overlay(overlay);
        }
        effective
    }

    /// Load global skills plus, when `working_dir` is `Some`, that project's
    /// local skills. `None` yields global skills only; it never falls back to
    /// the process cwd (see [`Self::load_project_local_dirs`]).
    pub fn load_for_working_dir(working_dir: Option<&Path>) -> Result<Self> {
        let mut registry = Self::load_global()?;
        registry.load_project_local_dirs(working_dir)?;
        Ok(registry)
    }

    /// Project-local skill directory for a workspace root.
    ///
    /// Takes a non-optional directory on purpose. The daemon is long-lived and
    /// serves sessions for many projects, so "no workspace root" must never
    /// degrade into a lookup relative to the process cwd, which would load
    /// whichever repository happened to start it. A caller that genuinely
    /// wants the process cwd has to pass it in, visibly.
    fn project_local_dir(working_dir: &Path, name: &str) -> PathBuf {
        working_dir.join(name).join("skills")
    }

    /// Merge this project's `.jcode/skills/`, `.agents/skills/`, and
    /// `.claude/skills/` into this registry.
    ///
    /// `working_dir = None` loads **no** project skills. Falling back to the
    /// process cwd would hand a session with no workspace root another
    /// project's skills, because the daemon is long-lived and its startup
    /// directory is arbitrary (P2.1). Such a caller gets global skills only.
    fn load_project_local_dirs(&mut self, working_dir: Option<&Path>) -> Result<()> {
        let Some(working_dir) = working_dir else {
            return Ok(());
        };

        // Load from ./.jcode/skills/ (project-local jcode skills)
        let local_jcode = Self::project_local_dir(working_dir, ".jcode");
        if local_jcode.exists() {
            self.load_from_dir(&local_jcode)?;
        }

        // Load from ./.agents/skills/ (shared cross-tool `.agents` convention)
        let local_agents = Self::project_local_dir(working_dir, ".agents");
        if local_agents.exists() {
            self.load_from_dir(&local_agents)?;
        }

        // Fallback: ./.claude/skills/ (project-local Claude skills for compatibility)
        let local_claude = Self::project_local_dir(working_dir, ".claude");
        if local_claude.exists() {
            self.load_from_dir(&local_claude)?;
        }

        Ok(())
    }

    /// Root of the Claude Code plugin store (`~/.claude/plugins`), if present.
    fn claude_plugins_root() -> Option<PathBuf> {
        crate::storage::user_home_path(".claude/plugins")
            .ok()
            .filter(|p| p.is_dir())
    }

    /// Load skills provided by Claude Code plugins under `plugins_root`.
    /// Returns the number of skills loaded. Errors are skipped so a broken
    /// plugin never prevents jcode's own skills from loading.
    fn load_plugin_skills_from_root(&mut self, plugins_root: &Path) -> usize {
        let mut count = 0;
        for dir in Self::plugin_skill_dirs_under(plugins_root) {
            count += self.load_from_dir_count(&dir).unwrap_or(0);
        }
        count
    }

    /// Discover `skills/` directories provided by Claude Code plugins under
    /// the given plugins root (normally `~/.claude/plugins`).
    ///
    /// Sources, in order of trust:
    /// - `installed_plugins.json` install paths (current Claude Code layout,
    ///   pointing into `cache/<marketplace>/<plugin>/<version>/`).
    /// - `repos/` checkouts (legacy plugin layout).
    /// - `cache/` as a fallback only when the manifest is missing/unparsable,
    ///   since the cache holds installed plugins.
    ///
    /// `marketplaces/` is intentionally not scanned: it mirrors the full
    /// marketplace catalog, including plugins the user never installed.
    fn plugin_skill_dirs_under(plugins_root: &Path) -> Vec<PathBuf> {
        if !plugins_root.is_dir() {
            return Vec::new();
        }

        let mut roots: Vec<PathBuf> =
            Self::installed_plugin_paths(&plugins_root.join("installed_plugins.json"));
        if roots.is_empty() {
            let cache = plugins_root.join("cache");
            if cache.is_dir() {
                roots.push(cache);
            }
        }
        let repos = plugins_root.join("repos");
        if repos.is_dir() {
            roots.push(repos);
        }

        let mut dirs = std::collections::BTreeSet::new();
        for root in roots {
            Self::collect_plugin_skills_dirs(&root, PLUGIN_SCAN_MAX_DEPTH, &mut dirs);
        }
        dirs.into_iter().collect()
    }

    /// Parse install paths from a Claude Code `installed_plugins.json`
    /// manifest. Tolerates both a list of installs per plugin (version 2) and
    /// a single install object, and skips paths that no longer exist.
    fn installed_plugin_paths(manifest: &Path) -> Vec<PathBuf> {
        let Ok(raw) = std::fs::read_to_string(manifest) else {
            return Vec::new();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return Vec::new();
        };
        let Some(plugins) = value.get("plugins").and_then(|p| p.as_object()) else {
            return Vec::new();
        };

        let mut paths = Vec::new();
        for installs in plugins.values() {
            let installs: Vec<&serde_json::Value> = match installs {
                serde_json::Value::Array(list) => list.iter().collect(),
                other => vec![other],
            };
            for install in installs {
                if let Some(path) = install.get("installPath").and_then(|p| p.as_str()) {
                    let path = PathBuf::from(path);
                    if path.is_dir() {
                        paths.push(path);
                    }
                }
            }
        }
        paths
    }

    /// Recursively collect directories named `skills` that contain at least
    /// one `<name>/SKILL.md`, up to `depth` levels below `root`.
    fn collect_plugin_skills_dirs(
        root: &Path,
        depth: usize,
        out: &mut std::collections::BTreeSet<PathBuf>,
    ) {
        if depth == 0 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || path.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == ".git" || name == "node_modules" {
                continue;
            }
            if name == "skills" && Self::dir_contains_skill(&path) {
                out.insert(path);
                continue;
            }
            Self::collect_plugin_skills_dirs(&path, depth - 1, out);
        }
    }

    /// True if `dir` has at least one immediate subdirectory with a SKILL.md.
    fn dir_contains_skill(dir: &Path) -> bool {
        std::fs::read_dir(dir).ok().is_some_and(|entries| {
            entries
                .flatten()
                .any(|e| e.path().join("SKILL.md").is_file())
        })
    }

    /// Load skills from a directory
    fn load_from_dir(&mut self, dir: &Path) -> Result<()> {
        if !dir.is_dir() {
            return Ok(());
        }

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let skill_file = path.join("SKILL.md");
                if skill_file.exists()
                    && let Ok(skill) = Self::parse_skill(&skill_file)
                {
                    self.skills.insert(skill.name.clone(), skill);
                }
            }
        }

        Ok(())
    }

    /// Parse a SKILL.md file
    fn parse_skill(path: &Path) -> Result<Skill> {
        Self::parse_skill_inner(path).map_err(|error| {
            crate::logging::warn(&format!(
                "Failed to parse skill file '{}': {}",
                path.display(),
                error
            ));
            error
        })
    }

    fn parse_skill_inner(path: &Path) -> Result<Skill> {
        let content = std::fs::read_to_string(path)?;

        // Parse YAML frontmatter
        let (frontmatter, body) = Self::parse_frontmatter(&content)?;

        let SkillFrontmatter {
            name,
            description,
            allowed_tools,
        } = frontmatter;

        let allowed_tools = allowed_tools.map(|tools| match tools {
            AllowedTools::CommaDelimited(tools) => tools
                .split(',')
                .map(|tool| tool.trim().to_string())
                .collect(),
            AllowedTools::Sequence(tools) => tools,
        });
        let search_text = build_skill_search_text(&name, &description, &body);

        Ok(Skill {
            name,
            description,
            allowed_tools,
            content: body,
            path: path.to_path_buf(),
            search_text,
        })
    }

    /// Parse YAML frontmatter from markdown
    fn parse_frontmatter(content: &str) -> Result<(SkillFrontmatter, String)> {
        let content = content.trim();

        if !content.starts_with("---") {
            anyhow::bail!("Missing YAML frontmatter");
        }

        let rest = &content[3..];
        let end = rest
            .find("---")
            .ok_or_else(|| anyhow::anyhow!("Unclosed frontmatter"))?;

        let yaml = &rest[..end];
        let body = rest[end + 3..].trim().to_string();

        let frontmatter: SkillFrontmatter = serde_yaml::from_str(yaml)?;

        Ok((frontmatter, body))
    }

    /// Get a skill by name
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    /// List all available skills.
    ///
    /// Sorted by skill name so the ordering is deterministic. The backing store
    /// is a `HashMap`, whose iteration order is randomized per instance; without
    /// this sort, two snapshots of the same skill set (e.g. the lock-contended
    /// `self.skills.clone()` fallback in `current_skills_snapshot`) could emit
    /// the "Available Skills" prompt section in different orders. That produces a
    /// system prompt with identical length but different bytes, silently busting
    /// the Anthropic strict-prefix KV cache mid-conversation.
    pub fn list(&self) -> Vec<&Skill> {
        let mut skills: Vec<&Skill> = self.skills.values().collect();
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        skills
    }

    /// Reload a specific skill by name
    pub fn reload(&mut self, name: &str) -> Result<bool> {
        // Find the skill's path first
        let path = self.skills.get(name).map(|s| s.path.clone());

        if let Some(path) = path {
            if path.exists() {
                let skill = Self::parse_skill(&path)?;
                self.skills.insert(skill.name.clone(), skill);
                Ok(true)
            } else {
                // Skill file was deleted
                self.skills.remove(name);
                Ok(false)
            }
        } else {
            Ok(false)
        }
    }

    /// Reload all skills from all locations
    pub fn reload_all(&mut self) -> Result<usize> {
        self.reload_global()
    }

    /// Reload the shared global skill sources (plugins, `~/.jcode/skills/`,
    /// `~/.agents/skills/`) into this registry.
    ///
    /// Project-local skills are intentionally NOT loaded here: they are a
    /// per-session overlay composed at read time via
    /// [`Self::effective_for_working_dir`], so one session's `reload_all`
    /// can never leak its project skills into the shared registry that other
    /// sessions see (issue #457).
    pub fn reload_global(&mut self) -> Result<usize> {
        // The available-skills list is embedded in the static system prompt,
        // so a reload that changes it legitimately invalidates warm KV cache
        // prefixes. Document it so the miss is attributed instead of alarmed.
        crate::cache_invalidation::record(
            "skill reload",
            "reloaded all skills; the skills list in the system prompt may have changed",
        );
        self.skills.clear();

        let mut count = 0;

        // Load skills provided by Claude Code plugins/marketplace installs
        // first, so explicit jcode/agents skills with the same name win below.
        if let Some(plugins_root) = Self::claude_plugins_root() {
            count += self.load_plugin_skills_from_root(&plugins_root);
        }

        // Load from ~/.jcode/skills/ (jcode's own global skills)
        if let Ok(jcode_dir) = crate::storage::jcode_dir() {
            let jcode_skills = jcode_dir.join("skills");
            if jcode_skills.exists() {
                count += self.load_from_dir_count(&jcode_skills)?;
            }
        }

        // Load from ~/.agents/skills/ (shared cross-tool `.agents` convention)
        if let Ok(agents_skills) = crate::storage::user_home_path(".agents/skills")
            && agents_skills.exists()
        {
            count += self.load_from_dir_count(&agents_skills)?;
        }

        Ok(count)
    }

    /// Load skills from a directory and return count
    fn load_from_dir_count(&mut self, dir: &Path) -> Result<usize> {
        if !dir.is_dir() {
            return Ok(0);
        }

        let mut count = 0;
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let skill_file = path.join("SKILL.md");
                if skill_file.exists()
                    && let Ok(skill) = Self::parse_skill(&skill_file)
                {
                    self.skills.insert(skill.name.clone(), skill);
                    count += 1;
                }
            }
        }

        Ok(count)
    }

    /// Parse `/skill-name` and `/skill-name prompt...` invocations.
    ///
    /// The trailing prompt is kept verbatim apart from surrounding whitespace.
    /// Quotes are intentionally not interpreted as shell syntax, so incomplete
    /// or literal quotes can never put the input path into a continuation state.
    /// Skill command tokens are limited to identifier-like names. In particular,
    /// path separators and filename punctuation are rejected so a terminal file
    /// drop such as `/tmp/screenshot.png` remains ordinary user input.
    pub fn parse_invocation(input: &str) -> Option<SkillInvocation<'_>> {
        let trimmed = input.trim();
        let invocation = trimmed.strip_prefix('/')?;
        let name_end = invocation
            .find(char::is_whitespace)
            .unwrap_or(invocation.len());
        let name = &invocation[..name_end];
        if name.is_empty()
            || !name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        {
            return None;
        }

        let prompt = invocation[name_end..].trim();
        let prompt = match prompt.as_bytes() {
            [b'"', .., b'"'] | [b'\'', .., b'\''] if prompt.len() >= 2 => {
                &prompt[1..prompt.len() - 1]
            }
            _ => prompt,
        };
        Some(SkillInvocation {
            name,
            prompt: (!prompt.is_empty()).then_some(prompt),
        })
    }

    /// Return true if a skill with the given name is currently loaded.
    pub fn contains(&self, name: &str) -> bool {
        self.skills.contains_key(name)
    }
}

/// A skill recommended/curated by jcode that the user may want to install.
#[derive(Debug, Clone, Copy)]
pub struct EndorsedSkill {
    /// Skill name (matches the `name` field in SKILL.md and the slash command).
    pub name: &'static str,
    /// One-line description of what the skill does.
    pub description: &'static str,
    /// Grouping label used to organize the endorsed list (e.g. "jcode",
    /// "NVIDIA CUDA-X").
    pub category: &'static str,
    /// Where users can get the skill (repo path, URL, or short note).
    pub source: &'static str,
    /// Optional install command/hint shown when the skill is not installed.
    pub install: Option<&'static str>,
}

/// Curated list of skills endorsed by jcode. Used by the `/skills` command to
/// show users which recommended skills they have installed and which they are
/// missing. This is the single source of truth for endorsed skills.
///
/// The NVIDIA CUDA-X entries mirror the official NVIDIA-verified catalog at
/// <https://github.com/NVIDIA/skills>; install them with
/// `npx skills add nvidia/skills --skill <name> --yes`.
pub const ENDORSED_SKILLS: &[EndorsedSkill] = &[
    EndorsedSkill {
        name: "optimization",
        description: "Improve performance, latency, throughput, memory usage, or general efficiency by defining metrics, measuring, attributing bottlenecks, and prioritizing macro-optimizations.",
        category: "jcode",
        source: "bundled in jcode repo (.jcode/skills/optimization)",
        install: None,
    },
    EndorsedSkill {
        name: "todo-planning-skill",
        description: "Create thorough, well-structured todo lists for long tasks, including reflection, static analysis, verification, and next-step updates.",
        category: "jcode",
        source: "bundled with jcode / Claude Code skills",
        install: None,
    },
    EndorsedSkill {
        name: "firefox-browser",
        description: "Control the user's Firefox browser with their logins and cookies intact to browse, fill forms, click, screenshot, and read authenticated pages.",
        category: "jcode",
        source: "bundled with jcode / Claude Code skills",
        install: None,
    },
    // Anthropic official skills (github.com/anthropics/skills, Apache-2.0).
    EndorsedSkill {
        name: "frontend-design",
        description: "Create distinctive, production-grade frontend interfaces with high design quality (web components, pages, apps). Generates creative, polished code that avoids generic AI aesthetics.",
        category: "Anthropic Design",
        source: "anthropics/skills (official Anthropic catalog)",
        install: Some(
            "npx skills add anthropics/skills --skill frontend-design --yes (or Claude Code: /plugin marketplace add anthropics/skills)",
        ),
    },
    // NVIDIA CUDA-X / GPU accelerated-computing skills from the official
    // NVIDIA-verified catalog (github.com/NVIDIA/skills).
    EndorsedSkill {
        name: "cuopt-developer",
        description: "Modify, build, test, debug, and contribute to NVIDIA cuOpt (C++/CUDA, Python, server, CI) — solver internals, PRs, DCO, and code conventions.",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some("npx skills add nvidia/skills --skill cuopt-developer --yes"),
    },
    EndorsedSkill {
        name: "cuopt-install",
        description: "Install NVIDIA cuOpt for Python, C, or server via pip, conda, or Docker, and verify the install.",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some("npx skills add nvidia/skills --skill cuopt-install --yes"),
    },
    EndorsedSkill {
        name: "cuopt-numerical-optimization-api-c",
        description: "Solve LP, MILP, and QP (beta) with the cuOpt C API for embedding optimization in C/C++.",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some(
            "npx skills add nvidia/skills --skill cuopt-numerical-optimization-api-c --yes",
        ),
    },
    EndorsedSkill {
        name: "cuopt-numerical-optimization-api-cli",
        description: "Solve LP, MILP, and QP (beta) with cuOpt from MPS files via the cuopt_cli command line.",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some(
            "npx skills add nvidia/skills --skill cuopt-numerical-optimization-api-cli --yes",
        ),
    },
    EndorsedSkill {
        name: "cuopt-numerical-optimization-api-python",
        description: "Solve LP, MILP, and QP (beta) with the cuOpt Python API — linear/quadratic objectives, integer variables, scheduling, portfolio, and least squares.",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some(
            "npx skills add nvidia/skills --skill cuopt-numerical-optimization-api-python --yes",
        ),
    },
    EndorsedSkill {
        name: "cuopt-numerical-optimization-formulation",
        description: "LP, MILP, and QP concepts and formulation patterns (parameters, constraints, decisions, objective). Concepts only; no API.",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some(
            "npx skills add nvidia/skills --skill cuopt-numerical-optimization-formulation --yes",
        ),
    },
    EndorsedSkill {
        name: "cuopt-routing-api-python",
        description: "Solve vehicle routing (VRP, TSP, PDP) with the cuOpt Python API.",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some("npx skills add nvidia/skills --skill cuopt-routing-api-python --yes"),
    },
    EndorsedSkill {
        name: "cuopt-routing-formulation",
        description: "Vehicle routing (VRP, TSP, PDP) problem types and data requirements. Domain concepts; no API or interface.",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some("npx skills add nvidia/skills --skill cuopt-routing-formulation --yes"),
    },
    EndorsedSkill {
        name: "cuopt-server-api-python",
        description: "Run the cuOpt REST server — start it, call endpoints, and use Python/curl client examples.",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some("npx skills add nvidia/skills --skill cuopt-server-api-python --yes"),
    },
    EndorsedSkill {
        name: "cuopt-server-common",
        description: "Understand what the cuOpt REST server does and how requests flow. Concepts only; no deploy or client code.",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some("npx skills add nvidia/skills --skill cuopt-server-common --yes"),
    },
    EndorsedSkill {
        name: "cuopt-user-rules",
        description: "Base rules for end users calling NVIDIA cuOpt (routing/LP/MILP/QP/install/server).",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some("npx skills add nvidia/skills --skill cuopt-user-rules --yes"),
    },
    EndorsedSkill {
        name: "cupynumeric-install",
        description: "Install and verify NVIDIA cuPyNumeric (NumPy/SciPy on multi-node multi-GPU) for Python — requirements, commands, and verification.",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some("npx skills add nvidia/skills --skill cupynumeric-install --yes"),
    },
    EndorsedSkill {
        name: "cupynumeric-migration-readiness",
        description: "Assess NumPy code before porting to cuPyNumeric — which patterns scale on GPU, what must be refactored, and a READY/REFACTOR/NOT-RECOMMENDED verdict.",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some("npx skills add nvidia/skills --skill cupynumeric-migration-readiness --yes"),
    },
    EndorsedSkill {
        name: "cupynumeric-hdf5",
        description: "Read and write large cuPyNumeric arrays to HDF5 with Legate's parallel, distributed HDF5 I/O (legate.io.hdf5), including GPUDirect Storage.",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some("npx skills add nvidia/skills --skill cupynumeric-hdf5 --yes"),
    },
    EndorsedSkill {
        name: "cupynumeric-parallel-data-load",
        description: "Load sharded on-disk datasets (.npy, Parquet/Arrow, raw binary, sharded HDF5) into a distributed cuPyNumeric ndarray via manual partition + leaf task launch.",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some("npx skills add nvidia/skills --skill cupynumeric-parallel-data-load --yes"),
    },
    EndorsedSkill {
        name: "accelerated-computing-cudf",
        description: "Official NVIDIA guidance for cuDF GPU DataFrames, pandas acceleration, dask-cuDF, ETL, joins, groupby, CSV/Parquet I/O, and multi-GPU DataFrame workloads.",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some("npx skills add nvidia/skills --skill accelerated-computing-cudf --yes"),
    },
    EndorsedSkill {
        name: "cudaq-guide",
        description: "NVIDIA CUDA-Q (CUDA Quantum) onboarding guide for installation, test programs, GPU simulation, QPU hardware, and quantum applications.",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some("npx skills add nvidia/skills --skill cudaq-guide --yes"),
    },
    EndorsedSkill {
        name: "tilegym-adding-cutile-kernel",
        description: "Add a new cuTile GPU kernel operator to NVIDIA TileGym — dispatch registration, cuTile backend implementation, exports, tests, and benchmarks.",
        category: "NVIDIA CUDA-X",
        source: "NVIDIA/skills (official NVIDIA-verified catalog)",
        install: Some("npx skills add nvidia/skills --skill tilegym-adding-cutile-kernel --yes"),
    },
];

/// Return the curated list of skills endorsed by jcode.
pub fn endorsed_skills() -> &'static [EndorsedSkill] {
    ENDORSED_SKILLS
}

impl Skill {
    /// Get the full prompt content for this skill
    pub fn get_prompt(&self) -> String {
        format!(
            "# Skill: {}\n\n{}\n\n{}",
            self.name, self.description, self.content
        )
    }

    /// Load additional files from the skill directory
    pub fn load_file(&self, filename: &str) -> Result<String> {
        let skill_dir = self
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("No parent dir"))?;
        let file_path = skill_dir.join(filename);
        Ok(std::fs::read_to_string(file_path)?)
    }

    pub fn as_memory_entry(&self) -> crate::memory::MemoryEntry {
        let now = Utc::now() - chrono::Duration::days(365);
        let mut entry = crate::memory::MemoryEntry::new(
            crate::memory::MemoryCategory::Custom("Skills".to_string()),
            format!(
                "Use skill `/{} ` when relevant.\n\n{}",
                self.name,
                self.get_prompt()
            ),
        )
        .with_id(format!("skill:{}", self.name))
        .with_tags(vec!["skill".to_string(), self.name.clone()])
        .with_source("skill_registry")
        .with_trust(crate::memory::TrustLevel::Medium)
        .with_timestamps(now, now);
        // Use the precomputed skill search text rather than the tag-derived one.
        entry.search_text = self.search_text.clone();
        entry
    }
}

fn build_skill_search_text(name: &str, description: &str, content: &str) -> String {
    normalize_skill_search_text(&format!("{}\n{}\n{}", name, description, content))
}

fn normalize_skill_search_text(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
