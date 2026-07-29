use anyhow::Result;
use chrono::Utc;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(not(test))]
use std::sync::OnceLock;
use tokio::sync::RwLock;

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
    allowed_tools: Option<String>,
}

/// Registry of available skills
#[derive(Debug, Default, Clone)]
pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
}

/// A slash-command skill invocation, optionally followed by a prompt that
/// should be submitted immediately after activating the skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillInvocation<'a> {
    pub name: &'a str,
    pub prompt: Option<&'a str>,
}

/// Maximum directory depth scanned under a Claude Code plugin root when
/// looking for `skills/<name>/SKILL.md` entries. Plugin layouts vary across
/// Claude Code versions (`cache/<marketplace>/<plugin>/<version>/skills/...`,
/// `repos/<owner>/<repo>/skills/...`, nested `.claude/skills/...`), so we scan
/// defensively but with a bound to avoid walking arbitrarily deep trees.
const PLUGIN_SCAN_MAX_DEPTH: usize = 5;

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
            let count = Self::copy_skills_dir(&claude_skills, &jcode_skills);
            if count > 0 {
                sources.push(format!("{} from Claude Code", count));
                copied.extend(Self::list_skill_names(&jcode_skills));
            }
        }

        // Import from Codex CLI (~/.codex/skills/)
        if let Ok(codex_skills) = crate::storage::user_home_path(".codex/skills")
            && codex_skills.is_dir()
        {
            let count = Self::copy_skills_dir(&codex_skills, &jcode_skills);
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

    /// Copy skill directories from src to dst. Returns count of skills copied.
    fn copy_skills_dir(src: &Path, dst: &Path) -> usize {
        let entries = match std::fs::read_dir(src) {
            Ok(e) => e,
            Err(_) => return 0,
        };

        let mut count = 0;
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
            if let Err(e) = Self::copy_dir_recursive(&path, &dest) {
                crate::logging::error(&format!("Failed to copy skill '{}': {}", name, e));
                continue;
            }
            count += 1;
        }
        count
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

    /// Load skills from all standard locations
    pub fn load() -> Result<Self> {
        Self::load_for_working_dir(None)
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
    /// other's project skills. `working_dir = None` resolves against the
    /// process cwd (single-process CLI mode).
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

    /// Load skills from all standard locations, with project-local locations
    /// resolved against an optional active session working directory.
    pub fn load_for_working_dir(working_dir: Option<&Path>) -> Result<Self> {
        let mut registry = Self::load_global()?;
        registry.load_project_local_dirs(working_dir)?;
        Ok(registry)
    }

    fn project_local_dir(working_dir: Option<&Path>, name: &str) -> PathBuf {
        let path = Path::new(name).join("skills");
        working_dir.map(|dir| dir.join(&path)).unwrap_or(path)
    }

    fn load_project_local_dirs(&mut self, working_dir: Option<&Path>) -> Result<()> {
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
        let content = std::fs::read_to_string(path)?;

        // Parse YAML frontmatter
        let (frontmatter, body) = Self::parse_frontmatter(&content)?;

        let SkillFrontmatter {
            name,
            description,
            allowed_tools,
        } = frontmatter;

        let allowed_tools =
            allowed_tools.map(|s| s.split(',').map(|t| t.trim().to_string()).collect());
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
mod tests {
    use super::*;

    fn test_skill(name: &str, description: &str, content: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: description.to_string(),
            allowed_tools: None,
            content: content.to_string(),
            path: PathBuf::from(format!("/tmp/{name}/SKILL.md")),
            search_text: build_skill_search_text(name, description, content),
        }
    }

    fn write_test_skill(root: &Path, scope: &str, name: &str) {
        let dir = root.join(scope).join("skills").join(name);
        std::fs::create_dir_all(&dir).expect("create skill dir");
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Test skill {name}\n---\n\nUse {name}.\n"),
        )
        .expect("write skill");
    }

    #[test]
    fn parse_invocation_supports_a_trailing_prompt() {
        assert_eq!(
            SkillRegistry::parse_invocation("/frontend-design build a settings page"),
            Some(SkillInvocation {
                name: "frontend-design",
                prompt: Some("build a settings page"),
            })
        );
        assert_eq!(
            SkillRegistry::parse_invocation("  /frontend-design   \"build a settings page\"  "),
            Some(SkillInvocation {
                name: "frontend-design",
                prompt: Some("build a settings page"),
            })
        );
    }

    #[test]
    fn parse_invocation_handles_bare_and_incomplete_quoted_prompts_without_blocking() {
        assert_eq!(
            SkillRegistry::parse_invocation("/optimization"),
            Some(SkillInvocation {
                name: "optimization",
                prompt: None,
            })
        );
        assert_eq!(
            SkillRegistry::parse_invocation("/optimization \"make this faster"),
            Some(SkillInvocation {
                name: "optimization",
                prompt: Some("\"make this faster"),
            })
        );
        assert_eq!(SkillRegistry::parse_invocation("/"), None);
    }

    #[test]
    fn parse_invocation_rejects_terminal_file_drop_paths() {
        for input in [
            "/tmp/screenshot.png",
            "/Users/example/My\\ File.txt inspect this",
            "/home/example/project/file.rs",
            "/.hidden-file",
            "/network\\share\\file.txt",
        ] {
            assert_eq!(
                SkillRegistry::parse_invocation(input),
                None,
                "filesystem path must not parse as a skill invocation: {input}"
            );
        }
    }

    #[test]
    fn list_is_sorted_by_name_regardless_of_insertion_order() {
        // The "Available Skills" system-prompt section is built from `list()`.
        // The backing store is a HashMap (per-instance randomized iteration
        // order), and `current_skills_snapshot` can hand back a *different*
        // HashMap instance via its lock-contended `self.skills.clone()` fallback.
        // If `list()` did not sort, two snapshots of the same skill set could
        // serialize the section in different orders: a same-length but
        // different-bytes system prompt that silently busts the KV cache.
        let names = ["zebra", "alpha", "mango", "beta", "yak"];

        let mut reg_a = SkillRegistry::default();
        for name in names {
            reg_a
                .skills
                .insert(name.to_string(), test_skill(name, "d", "c"));
        }

        // Build a second registry with the reverse insertion order to maximize
        // the chance of a differing HashMap layout.
        let mut reg_b = SkillRegistry::default();
        for name in names.iter().rev() {
            reg_b
                .skills
                .insert(name.to_string(), test_skill(name, "d", "c"));
        }

        let order_a: Vec<&str> = reg_a.list().iter().map(|s| s.name.as_str()).collect();
        let order_b: Vec<&str> = reg_b.list().iter().map(|s| s.name.as_str()).collect();

        assert_eq!(order_a, vec!["alpha", "beta", "mango", "yak", "zebra"]);
        assert_eq!(
            order_a, order_b,
            "list() ordering must be identical across HashMap instances"
        );
    }

    #[test]
    fn skill_as_memory_entry_formats_invocation_and_prompt() {
        let skill = test_skill(
            "firefox-browser",
            "Control Firefox browser sessions and logged-in pages",
            "Use this skill when you need to open websites, click buttons, or interact with browser pages.",
        );

        let entry = skill.as_memory_entry();

        assert_eq!(entry.id, "skill:firefox-browser");
        assert!(matches!(
            entry.category,
            crate::memory::MemoryCategory::Custom(ref name) if name == "Skills"
        ));
        assert!(entry.content.contains("/firefox-browser"));
        assert!(entry.content.contains("# Skill: firefox-browser"));
        assert_eq!(entry.source.as_deref(), Some("skill_registry"));
    }

    #[test]
    fn load_for_working_dir_reads_project_local_jcode_skills() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_test_skill(temp.path(), ".jcode", "wd-only");

        let registry = SkillRegistry::load_for_working_dir(Some(temp.path())).expect("load skills");

        let skill = registry
            .get("wd-only")
            .expect("working-dir local skill should load");
        assert_eq!(skill.description, "Test skill wd-only");
        assert!(skill.path.starts_with(temp.path()));
    }

    #[test]
    fn load_for_working_dir_reads_project_local_agents_skills() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_test_skill(temp.path(), ".agents", "agents-only");

        let registry = SkillRegistry::load_for_working_dir(Some(temp.path())).expect("load skills");

        let skill = registry
            .get("agents-only")
            .expect("project-local .agents skill should load");
        assert_eq!(skill.description, "Test skill agents-only");
        assert!(skill.path.starts_with(temp.path()));
    }

    #[test]
    fn project_overlay_is_session_scoped_and_composes_over_globals() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_test_skill(temp.path(), ".jcode", "session-skill");

        // The overlay resolves against the given workspace root, not the
        // process cwd or a shared registry (issue #457).
        let overlay = SkillRegistry::load_project_overlay(Some(temp.path())).expect("load overlay");
        assert!(overlay.get("session-skill").is_some());

        // A different workspace root sees nothing from this project.
        let other = tempfile::tempdir().expect("tempdir");
        let other_overlay =
            SkillRegistry::load_project_overlay(Some(other.path())).expect("load overlay");
        assert!(other_overlay.get("session-skill").is_none());

        // Effective set = base globals + overlay, with overlay winning on name.
        let mut base = SkillRegistry::default();
        base.skills.insert(
            "session-skill".to_string(),
            test_skill("session-skill", "global variant", "global body"),
        );
        let effective = SkillRegistry::effective_for_working_dir(&base, Some(temp.path()));
        let skill = effective
            .get("session-skill")
            .expect("overlay skill should be present");
        assert!(
            skill.path.starts_with(temp.path()),
            "project-local overlay must win over a same-named global skill"
        );
    }

    #[test]
    fn reload_global_excludes_project_local_skills() {
        // chdir is process-global; serialize with other env-sensitive tests.
        let _env_guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        write_test_skill(temp.path(), ".jcode", "session-skill");

        let prev_cwd = std::env::current_dir().expect("cwd");
        // reload_global must not pick up project-local skills even when the
        // process cwd contains them (daemon startup cwd independence).
        std::env::set_current_dir(temp.path()).expect("chdir");
        let mut registry = SkillRegistry::default();
        let result = registry.reload_global();
        std::env::set_current_dir(prev_cwd).expect("restore cwd");

        result.expect("reload skills");
        assert!(
            registry.get("session-skill").is_none(),
            "shared/global reload must never absorb project-local skills"
        );
    }

    #[test]
    fn endorsed_skills_have_unique_nonempty_metadata() {
        let endorsed = endorsed_skills();
        assert!(!endorsed.is_empty(), "expected at least one endorsed skill");

        let mut seen = std::collections::HashSet::new();
        for skill in endorsed {
            assert!(!skill.name.is_empty(), "endorsed skill name must be set");
            assert!(
                !skill.description.is_empty(),
                "endorsed skill {} needs a description",
                skill.name
            );
            assert!(
                !skill.category.is_empty(),
                "endorsed skill {} needs a category",
                skill.name
            );
            assert!(
                !skill.source.is_empty(),
                "endorsed skill {} needs a source",
                skill.name
            );
            assert!(
                !skill.name.starts_with('/'),
                "endorsed skill name should not include the leading slash"
            );
            if let Some(install) = skill.install {
                assert!(
                    install.contains(skill.name),
                    "endorsed skill {} install hint should reference its name",
                    skill.name
                );
            }
            assert!(
                seen.insert(skill.name),
                "duplicate endorsed skill name: {}",
                skill.name
            );
        }
    }

    #[test]
    fn endorsed_skills_include_nvidia_cuda_x_catalog() {
        let endorsed = endorsed_skills();
        // Spot-check representative NVIDIA CUDA-X skills sourced from the
        // official NVIDIA/skills catalog.
        for expected in [
            "cuopt-numerical-optimization-api-python",
            "cupynumeric-install",
            "accelerated-computing-cudf",
            "cudaq-guide",
            "tilegym-adding-cutile-kernel",
        ] {
            let skill = endorsed
                .iter()
                .find(|s| s.name == expected)
                .unwrap_or_else(|| panic!("expected endorsed NVIDIA skill {expected}"));
            assert_eq!(skill.category, "NVIDIA CUDA-X");
            assert!(
                skill
                    .install
                    .is_some_and(|cmd| cmd.contains("nvidia/skills")),
                "NVIDIA skill {expected} should have an nvidia/skills install hint"
            );
        }
    }

    #[test]
    fn endorsed_skills_include_anthropic_frontend_design() {
        let skill = endorsed_skills()
            .iter()
            .find(|s| s.name == "frontend-design")
            .expect("expected endorsed Anthropic frontend-design skill");
        assert_eq!(skill.category, "Anthropic Design");
        assert!(
            skill.source.contains("anthropics/skills"),
            "frontend-design should be sourced from anthropics/skills"
        );
        assert!(
            skill
                .install
                .is_some_and(|cmd| cmd.contains("anthropics/skills")),
            "frontend-design should have an anthropics/skills install hint"
        );
    }

    #[test]
    fn registry_contains_reports_loaded_skills() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_test_skill(temp.path(), ".jcode", "present-skill");

        let registry = SkillRegistry::load_for_working_dir(Some(temp.path())).expect("load skills");
        assert!(registry.contains("present-skill"));
        assert!(!registry.contains("missing-skill"));
    }

    /// Write `SKILL.md` for `name` inside `<plugin_dir>/skills/<name>/`.
    fn write_plugin_skill(plugin_dir: &Path, name: &str) {
        write_plugin_skill_with_description(plugin_dir, name, &format!("Plugin skill {name}"));
    }

    fn write_plugin_skill_with_description(plugin_dir: &Path, name: &str, description: &str) {
        let dir = plugin_dir.join("skills").join(name);
        std::fs::create_dir_all(&dir).expect("create plugin skill dir");
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\nUse {name}.\n"),
        )
        .expect("write plugin skill");
    }

    fn write_installed_plugins_manifest(plugins_root: &Path, install_paths: &[&Path]) {
        let plugins: serde_json::Map<String, serde_json::Value> = install_paths
            .iter()
            .enumerate()
            .map(|(i, path)| {
                (
                    format!("plugin-{i}@test-marketplace"),
                    serde_json::json!([{ "scope": "user", "installPath": path, "version": "1.0.0" }]),
                )
            })
            .collect();
        std::fs::write(
            plugins_root.join("installed_plugins.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "version": 2,
                "plugins": plugins,
            }))
            .expect("serialize manifest"),
        )
        .expect("write manifest");
    }

    #[test]
    fn plugin_skills_load_from_installed_plugins_manifest() {
        let temp = tempfile::tempdir().expect("tempdir");
        let plugins_root = temp.path();

        // Mirror the real Claude Code layout:
        // cache/<marketplace>/<plugin>/<version>/skills/<skill>/SKILL.md
        let install = plugins_root.join("cache/test-marketplace/vercel/0.40.1");
        write_plugin_skill(&install, "ai-gateway");
        // Nested `.claude/skills` variant inside the same install.
        write_plugin_skill(&install.join(".claude"), "benchmark-agents");
        write_installed_plugins_manifest(plugins_root, &[&install]);

        let mut registry = SkillRegistry::default();
        let count = registry.load_plugin_skills_from_root(plugins_root);

        assert_eq!(count, 2);
        assert!(registry.contains("ai-gateway"));
        assert!(registry.contains("benchmark-agents"));
    }

    #[test]
    fn plugin_skills_fall_back_to_cache_scan_without_manifest() {
        let temp = tempfile::tempdir().expect("tempdir");
        let plugins_root = temp.path();

        let install = plugins_root.join("cache/test-marketplace/my-plugin/1.0.0");
        write_plugin_skill(&install, "cache-skill");

        let mut registry = SkillRegistry::default();
        let count = registry.load_plugin_skills_from_root(plugins_root);

        assert_eq!(count, 1);
        assert!(registry.contains("cache-skill"));
    }

    #[test]
    fn plugin_skills_load_from_repos_layout() {
        let temp = tempfile::tempdir().expect("tempdir");
        let plugins_root = temp.path();

        let repo = plugins_root.join("repos/owner/my-plugin");
        write_plugin_skill(&repo, "repo-skill");

        let mut registry = SkillRegistry::default();
        let count = registry.load_plugin_skills_from_root(plugins_root);

        assert_eq!(count, 1);
        assert!(registry.contains("repo-skill"));
    }

    #[test]
    fn plugin_scan_skips_marketplace_catalog_when_manifest_exists() {
        let temp = tempfile::tempdir().expect("tempdir");
        let plugins_root = temp.path();

        // Installed plugin listed in the manifest.
        let install = plugins_root.join("cache/test-marketplace/installed/1.0.0");
        write_plugin_skill(&install, "installed-skill");
        write_installed_plugins_manifest(plugins_root, &[&install]);

        // Marketplace catalog entry the user never installed.
        write_plugin_skill(
            &plugins_root.join("marketplaces/test-marketplace/plugins/uninstalled"),
            "uninstalled-skill",
        );
        // Cache entry not referenced by the manifest (stale install).
        write_plugin_skill(
            &plugins_root.join("cache/test-marketplace/stale/0.1.0"),
            "stale-skill",
        );

        let mut registry = SkillRegistry::default();
        registry.load_plugin_skills_from_root(plugins_root);

        assert!(registry.contains("installed-skill"));
        assert!(
            !registry.contains("uninstalled-skill"),
            "marketplace catalog skills must not load"
        );
        assert!(
            !registry.contains("stale-skill"),
            "cache entries outside the manifest must not load when a manifest exists"
        );
    }

    #[test]
    fn plugin_scan_respects_depth_bound() {
        let temp = tempfile::tempdir().expect("tempdir");
        let plugins_root = temp.path();

        // Deeper than PLUGIN_SCAN_MAX_DEPTH below the cache root.
        let too_deep = plugins_root.join("cache/a/b/c/d/e/f");
        write_plugin_skill(&too_deep, "too-deep-skill");

        let mut registry = SkillRegistry::default();
        let count = registry.load_plugin_skills_from_root(plugins_root);

        assert_eq!(count, 0);
        assert!(!registry.contains("too-deep-skill"));
    }

    #[test]
    fn explicit_jcode_skill_wins_over_plugin_skill_with_same_name() {
        let temp = tempfile::tempdir().expect("tempdir");
        let plugins_root = temp.path().join("plugins");

        let install = plugins_root.join("cache/test-marketplace/my-plugin/1.0.0");
        write_plugin_skill_with_description(&install, "shared-name", "plugin version");

        // Explicit jcode skill with the same name.
        write_test_skill(temp.path(), ".jcode", "shared-name");

        // Mirror load ordering: plugins first, then explicit skill dirs, so
        // the later (explicit) insert wins in the registry map.
        let mut registry = SkillRegistry::default();
        registry.load_plugin_skills_from_root(&plugins_root);
        registry
            .load_from_dir(&temp.path().join(".jcode/skills"))
            .expect("load explicit skills");

        let skill = registry.get("shared-name").expect("skill present");
        assert_eq!(
            skill.description, "Test skill shared-name",
            "explicit jcode skill must override the plugin-provided one"
        );
    }

    #[test]
    fn plugin_skill_dirs_empty_for_missing_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("does-not-exist");
        assert!(SkillRegistry::plugin_skill_dirs_under(&missing).is_empty());
    }
}
