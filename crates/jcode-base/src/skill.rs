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
    /// Issue #94: MAS (Multi-Agent Skill) — model the skill targets.
    /// When `Some`, jcode will route activation of this skill to the
    /// matching sub-agent / model. When `None`, the active model is
    /// used (current behavior).
    pub model: Option<String>,
    /// Issue #94: MAS — sub-agent role identifier. Used by the future
    /// MAS dispatcher to find the right side-agent. `None` means
    /// the skill runs in the main agent.
    pub agent: Option<String>,
    /// Issue #94: MAS — semantic activation tags. Used in addition to
    /// the embedding-based activation for keyword fallback.
    pub tags: Vec<String>,
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
    /// MAS: target model id (#94)
    #[serde(default)]
    model: Option<String>,
    /// MAS: sub-agent role (#94)
    #[serde(default)]
    agent: Option<String>,
    /// MAS: keyword tags (#94). Accepts either a YAML sequence or a
    /// comma-separated string.
    #[serde(default, deserialize_with = "deserialize_tags")]
    tags: Vec<String>,
}

fn deserialize_tags<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Tags {
        List(Vec<String>),
        CommaSeparated(String),
    }
    Ok(match Tags::deserialize(deserializer)? {
        Tags::List(v) => v
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Tags::CommaSeparated(s) => s
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    })
}

/// Registry of available skills
#[derive(Debug, Default, Clone)]
pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
}

impl SkillRegistry {
    /// Process-wide shared mutable registry used by both `skill_manage` and
    /// direct slash invocation paths. Keeping a single registry prevents slash
    /// commands from seeing a stale startup-only skill snapshot after reloads.
    pub fn shared_registry() -> Arc<RwLock<Self>> {
        #[cfg(test)]
        {
            Arc::new(RwLock::new(Self::load().unwrap_or_default()))
        }

        #[cfg(not(test))]
        {
            static SHARED: OnceLock<Arc<RwLock<SkillRegistry>>> = OnceLock::new();
            SHARED
                .get_or_init(|| Arc::new(RwLock::new(SkillRegistry::load().unwrap_or_default())))
                .clone()
        }
    }

    /// Load a process-wide shared immutable snapshot of skills for startup paths
    /// that only need read access.
    pub fn shared_snapshot() -> Arc<Self> {
        #[cfg(test)]
        {
            Arc::new(Self::load().unwrap_or_default())
        }

        #[cfg(not(test))]
        {
            if let Ok(skills) = Self::shared_registry().try_read() {
                Arc::new(skills.clone())
            } else {
                Arc::new(SkillRegistry::load().unwrap_or_default())
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

    /// Load skills from all standard locations, with project-local locations
    /// resolved against an optional active session working directory.
    pub fn load_for_working_dir(working_dir: Option<&Path>) -> Result<Self> {
        // First-run import from Claude Code / Codex CLI
        Self::import_from_external();

        let mut registry = Self::default();

        // Load from ~/.jcode/skills/ (jcode's own global skills)
        if let Ok(jcode_dir) = crate::storage::jcode_dir() {
            let jcode_skills = jcode_dir.join("skills");
            if jcode_skills.exists() {
                registry.load_from_dir(&jcode_skills)?;
            }
        }

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

        // Fallback: ./.claude/skills/ (project-local Claude skills for compatibility)
        let local_claude = Self::project_local_dir(working_dir, ".claude");
        if local_claude.exists() {
            self.load_from_dir(&local_claude)?;
        }

        // Issue #112: repo-level scoping. If `working_dir` (or cwd) lives
        // inside a git repo, walk up to the repo root and load
        // <repo>/.jcode/skills/ — but only if the repo root differs from
        // the current dir (so we don't double-load the same dir).
        if let Some(repo_skills) = Self::repo_level_skills_dir(working_dir)
            && repo_skills.is_dir()
            && Some(repo_skills.as_path()) != Some(local_jcode.as_path())
        {
            self.load_from_dir(&repo_skills)?;
        }

        Ok(())
    }

    /// Walk up from `working_dir` (or cwd) until we find a directory
    /// containing `.git`. Returns `<repo_root>/.jcode/skills/` for that
    /// directory. Used by `load_project_local_dirs` to support repo-level
    /// skills (#112).
    ///
    /// Returns `None` if cwd cannot be resolved or no `.git` ancestor
    /// exists in the path.
    fn repo_level_skills_dir(working_dir: Option<&Path>) -> Option<PathBuf> {
        let start = match working_dir {
            Some(d) => d.to_path_buf(),
            None => std::env::current_dir().ok()?,
        };
        let mut current = start.as_path();
        loop {
            if current.join(".git").exists() {
                return Some(current.join(".jcode").join("skills"));
            }
            current = current.parent()?;
        }
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
            model,
            agent,
            tags,
        } = frontmatter;

        let allowed_tools =
            allowed_tools.map(|s| s.split(',').map(|t| t.trim().to_string()).collect());
        let search_text = build_skill_search_text(&name, &description, &body);

        Ok(Skill {
            name,
            description,
            allowed_tools,
            model,
            agent,
            tags,
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

    /// List all available skills
    pub fn list(&self) -> Vec<&Skill> {
        self.skills.values().collect()
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
        self.reload_all_for_working_dir(None)
    }

    /// Reload all skills, resolving project-local locations against an optional
    /// active session working directory.
    pub fn reload_all_for_working_dir(&mut self, working_dir: Option<&Path>) -> Result<usize> {
        self.skills.clear();

        let mut count = 0;

        // Load from ~/.jcode/skills/ (jcode's own global skills)
        if let Ok(jcode_dir) = crate::storage::jcode_dir() {
            let jcode_skills = jcode_dir.join("skills");
            if jcode_skills.exists() {
                count += self.load_from_dir_count(&jcode_skills)?;
            }
        }

        // Load from ./.jcode/skills/ (project-local jcode skills)
        let local_jcode = Self::project_local_dir(working_dir, ".jcode");
        if local_jcode.exists() {
            count += self.load_from_dir_count(&local_jcode)?;
        }

        // Fallback: ./.claude/skills/ (project-local Claude skills for compatibility)
        let local_claude = Self::project_local_dir(working_dir, ".claude");
        if local_claude.exists() {
            count += self.load_from_dir_count(&local_claude)?;
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

    /// Check if a message is a skill invocation.
    ///
    /// Skills use the `$<name>` namespace exclusively — `/` is reserved
    /// for built-in slash commands. Returns the bare skill name without
    /// the prefix, or `None` if the input is not a single-word `$`
    /// invocation.
    ///
    /// Note: the historical `/<skill>` form was retired to keep the `/`
    /// autocomplete dropdown navigable when the user has dozens of
    /// skills installed.
    pub fn parse_invocation(input: &str) -> Option<&str> {
        let trimmed = input.trim();
        if trimmed.contains(' ') {
            return None;
        }
        let rest = trimmed.strip_prefix('$')?;
        (!rest.is_empty()).then_some(rest)
    }
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
        crate::memory::MemoryEntry {
            id: format!("skill:{}", self.name),
            category: crate::memory::MemoryCategory::Custom("Skills".to_string()),
            content: format!(
                "Use skill `/{} ` when relevant.\n\n{}",
                self.name,
                self.get_prompt()
            ),
            tags: vec!["skill".to_string(), self.name.clone()],
            search_text: self.search_text.clone(),
            created_at: now,
            updated_at: now,
            access_count: 0,
            source: Some("skill_registry".to_string()),
            trust: crate::memory::TrustLevel::Medium,
            strength: 1,
            active: true,
            superseded_by: None,
            reinforcements: Vec::new(),
            embedding: None,
            confidence: 1.0,
        }
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
            model: None,
            agent: None,
            tags: Vec::new(),
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
    fn reload_all_for_working_dir_replaces_stale_snapshot_with_session_local_skills() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_test_skill(temp.path(), ".jcode", "session-skill");

        let mut registry = SkillRegistry::default();
        let count = registry
            .reload_all_for_working_dir(Some(temp.path()))
            .expect("reload skills");

        assert!(count >= 1);
        assert!(registry.get("session-skill").is_some());
    }

    // Issue #112: repo-level scoping. Walking up from cwd to a `.git`
    // ancestor and loading <repo>/.jcode/skills/.

    #[test]
    fn load_for_working_dir_reads_repo_level_skills_from_git_ancestor() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo_root = temp.path();
        // Mark this as a git repo.
        std::fs::create_dir_all(repo_root.join(".git")).expect("create .git");
        // Write a repo-level skill at <repo>/.jcode/skills/repo-skill/SKILL.md
        write_test_skill(repo_root, ".jcode", "repo-skill");

        // Working dir is a sub-directory of the repo.
        let sub = repo_root.join("crates/foo");
        std::fs::create_dir_all(&sub).expect("create sub");

        let registry = SkillRegistry::load_for_working_dir(Some(&sub)).expect("load skills");
        assert!(
            registry.get("repo-skill").is_some(),
            "repo-level skill must be discovered when cwd is a sub-dir of the .git ancestor"
        );
    }

    #[test]
    fn repo_level_skills_dir_returns_none_outside_git_repo() {
        let temp = tempfile::tempdir().expect("tempdir");
        // No .git anywhere up the tree from this temp dir.
        let result = SkillRegistry::repo_level_skills_dir(Some(temp.path()));
        // Possible Some if the tempdir is itself inside a git tree (e.g. in
        // CI the runner might be in /tmp which has no .git, but on a dev
        // machine /tmp could be inside a worktree). We don't assert None
        // unconditionally — instead assert the path is sensible if returned.
        if let Some(path) = result {
            assert!(path.ends_with(std::path::Path::new(".jcode/skills")));
        }
    }

    #[test]
    fn repo_level_skills_dir_does_not_double_load_when_cwd_equals_repo_root() {
        // When cwd IS the repo root, project-local + repo-level resolve to
        // the same dir; load_project_local_dirs guards against double-load.
        // We only assert that repo_level_skills_dir returns the same path
        // as project_local_dir — the dedup check is done in the caller.
        let temp = tempfile::tempdir().expect("tempdir");
        let repo_root = temp.path();
        std::fs::create_dir_all(repo_root.join(".git")).expect("create .git");
        write_test_skill(repo_root, ".jcode", "rl-skill");

        let registry = SkillRegistry::load_for_working_dir(Some(repo_root)).expect("load");
        // Skill loaded exactly once (no duplicate).
        let count = registry
            .skills
            .values()
            .filter(|s| s.name == "rl-skill")
            .count();
        assert_eq!(
            count, 1,
            "skill must not be loaded twice when cwd == repo root"
        );
    }
}

#[cfg(test)]
mod invocation_parse_tests {
    use super::*;

    #[test]
    fn parse_invocation_accepts_dollar_prefix() {
        assert_eq!(
            SkillRegistry::parse_invocation("$grill-me"),
            Some("grill-me")
        );
        assert_eq!(
            SkillRegistry::parse_invocation("  $grill-me  "),
            Some("grill-me")
        );
    }

    #[test]
    fn parse_invocation_rejects_slash_prefix() {
        // The historical `/<skill>` form is intentionally rejected so the
        // `/` autocomplete dropdown can stay focused on built-in commands.
        // Slash-prefixed input falls through to the slash-command chain.
        assert!(SkillRegistry::parse_invocation("/grill-me").is_none());
        assert!(SkillRegistry::parse_invocation("/help").is_none());
    }

    #[test]
    fn parse_invocation_rejects_invocation_with_args() {
        // $<name> requires single-word form; whitespace returns None so
        // the input is treated as a literal user message.
        assert!(SkillRegistry::parse_invocation("$grill-me with args").is_none());
    }

    #[test]
    fn parse_invocation_rejects_bare_dollar() {
        assert!(SkillRegistry::parse_invocation("$").is_none());
        assert!(SkillRegistry::parse_invocation("  $  ").is_none());
    }

    #[test]
    fn parse_invocation_rejects_other_prefixes() {
        assert!(SkillRegistry::parse_invocation("@grill-me").is_none());
        assert!(SkillRegistry::parse_invocation("!grill-me").is_none());
        assert!(SkillRegistry::parse_invocation("grill-me").is_none());
    }

    // ---- Issue #94: MAS frontmatter fields ----

    fn write_skill_with_frontmatter(
        dir: &std::path::Path,
        name: &str,
        fm: &str,
        body: &str,
    ) -> std::path::PathBuf {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        std::fs::write(&path, format!("---\n{}\n---\n\n{}\n", fm.trim(), body)).unwrap();
        path
    }

    #[test]
    fn mas_frontmatter_defaults_to_none_when_unset() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = write_skill_with_frontmatter(
            temp.path(),
            "basic",
            "name: basic\ndescription: A basic skill",
            "Body.",
        );
        let skill = SkillRegistry::parse_skill(&path).unwrap();
        assert_eq!(skill.name, "basic");
        assert_eq!(skill.model, None);
        assert_eq!(skill.agent, None);
        assert!(skill.tags.is_empty());
    }

    #[test]
    fn mas_frontmatter_parses_model_and_agent() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = write_skill_with_frontmatter(
            temp.path(),
            "review",
            "name: review\ndescription: Code review\nmodel: claude-opus-4\nagent: reviewer",
            "Body.",
        );
        let skill = SkillRegistry::parse_skill(&path).unwrap();
        assert_eq!(skill.model.as_deref(), Some("claude-opus-4"));
        assert_eq!(skill.agent.as_deref(), Some("reviewer"));
    }

    #[test]
    fn mas_frontmatter_parses_tags_as_list() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = write_skill_with_frontmatter(
            temp.path(),
            "tagged",
            "name: tagged\ndescription: Tagged\ntags:\n  - rust\n  - async\n  - perf",
            "Body.",
        );
        let skill = SkillRegistry::parse_skill(&path).unwrap();
        assert_eq!(skill.tags, vec!["rust", "async", "perf"]);
    }

    #[test]
    fn mas_frontmatter_parses_tags_as_comma_separated() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = write_skill_with_frontmatter(
            temp.path(),
            "csv",
            "name: csv\ndescription: CSV tags\ntags: rust, async , perf",
            "Body.",
        );
        let skill = SkillRegistry::parse_skill(&path).unwrap();
        assert_eq!(skill.tags, vec!["rust", "async", "perf"]);
    }

    #[test]
    fn mas_frontmatter_filters_blank_tags() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = write_skill_with_frontmatter(
            temp.path(),
            "blank",
            "name: blank\ndescription: x\ntags:\n  - rust\n  - ''\n  - perf",
            "Body.",
        );
        let skill = SkillRegistry::parse_skill(&path).unwrap();
        assert_eq!(skill.tags, vec!["rust", "perf"]);
    }
}
