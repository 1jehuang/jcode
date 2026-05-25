//! Agent registry: discovery + loading of `AgentDefinition`s from disk.
//!
//! ## Lookup paths (highest priority first)
//!
//! 1. **Project-local**: `<cwd>/.jcode/agents/*.toml`
//! 2. **User-global**: `~/.jcode/agents/*.toml`
//! 3. **Builtins** registered programmatically via [`AgentRegistry::register_builtin`]
//!
//! When the same id appears in multiple sources, the higher-priority one
//! wins. The registry tracks where each agent came from so `jcode doctor`
//! can show provenance.
//!
//! ## What this module does NOT do
//!
//! - It does not validate that `tool_names` exist in the tool registry
//!   (Phase 0.4) or that `spawnable_agents` resolve to known agents
//!   (cross-reference). Both are caller responsibilities done at agent
//!   spawn time, not load time, because the tool/agent universe may be
//!   feature-gated.
//! - It does not watch for file changes. Agents are loaded once at
//!   session start. Self-dev is welcome to call `reload_from_disk()`.

use crate::definition::{AgentDefinition, DefinitionError};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where an agent definition was loaded from. Surfaced in `jcode doctor`
/// and conflict warnings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentSource {
    /// Compiled into the binary by name. Lowest priority.
    Builtin,
    /// Loaded from `~/.jcode/agents/<file>`.
    UserGlobal { path: PathBuf },
    /// Loaded from `<project>/.jcode/agents/<file>`. Highest priority.
    ProjectLocal { path: PathBuf },
}

impl AgentSource {
    fn priority(&self) -> u8 {
        match self {
            AgentSource::Builtin => 0,
            AgentSource::UserGlobal { .. } => 1,
            AgentSource::ProjectLocal { .. } => 2,
        }
    }

    /// Short human-readable label for `jcode doctor` output.
    pub fn short_label(&self) -> String {
        match self {
            AgentSource::Builtin => "builtin".to_string(),
            AgentSource::UserGlobal { path } => format!("user:{}", path.display()),
            AgentSource::ProjectLocal { path } => format!("project:{}", path.display()),
        }
    }
}

/// One loaded agent: its definition plus where it came from.
#[derive(Debug, Clone)]
pub struct LoadedAgent {
    pub definition: AgentDefinition,
    pub source: AgentSource,
}

/// Errors surfaced when loading an agent file. We distinguish I/O,
/// parse, and validation errors so the TUI can render actionable
/// messages.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("failed to read `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse `{path}`: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("invalid agent definition in `{path}`: {source}")]
    Invalid {
        path: PathBuf,
        #[source]
        source: DefinitionError,
    },

    #[error(
        "filename `{path}` does not match agent id `{id}`. Rename the file to `{id}.toml`."
    )]
    FileNameMismatch { path: PathBuf, id: String },
}

/// In-memory registry of loaded agent definitions. Wrap in `Arc` if you
/// need to share — `LoadError` contains `io::Error` so the registry itself
/// is not `Clone`.
#[derive(Debug, Default)]
pub struct AgentRegistry {
    by_id: HashMap<String, LoadedAgent>,
    /// Non-fatal load errors collected during discovery. Surfaced by
    /// `jcode doctor` so users can see why a malformed file was skipped.
    load_errors: Vec<LoadError>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Total number of registered agents.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// True if no agents are registered.
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Look up an agent by id.
    pub fn get(&self, id: &str) -> Option<&LoadedAgent> {
        self.by_id.get(id)
    }

    /// Iterate over all agents in arbitrary order.
    pub fn iter(&self) -> impl Iterator<Item = &LoadedAgent> {
        self.by_id.values()
    }

    /// Sorted (by id) iteration — handy for stable doctor output.
    pub fn iter_sorted(&self) -> Vec<&LoadedAgent> {
        let mut v: Vec<_> = self.by_id.values().collect();
        v.sort_by(|a, b| a.definition.id.cmp(&b.definition.id));
        v
    }

    /// Non-fatal errors accumulated during discovery.
    pub fn load_errors(&self) -> &[LoadError] {
        &self.load_errors
    }

    /// Insert (or replace) an agent according to source priority. Returns
    /// the previous entry if it was overridden.
    pub fn insert(&mut self, loaded: LoadedAgent) -> Option<LoadedAgent> {
        let id = loaded.definition.id.clone();
        match self.by_id.get(&id) {
            Some(existing) if existing.source.priority() > loaded.source.priority() => {
                // existing has higher priority, drop the new one
                Some(loaded)
            }
            _ => self.by_id.insert(id, loaded),
        }
    }

    /// Register a builtin agent. Builtins have the lowest priority and
    /// are overridable by both user and project files of the same id.
    pub fn register_builtin(
        &mut self,
        definition: AgentDefinition,
    ) -> Result<(), DefinitionError> {
        definition.validate()?;
        self.insert(LoadedAgent {
            definition,
            source: AgentSource::Builtin,
        });
        Ok(())
    }

    /// Discover and load all agent files from `dir`. Non-recursive.
    /// Files that don't end in `.toml` are skipped silently. Bad files
    /// are recorded in `load_errors()` and skipped.
    ///
    /// `source_kind` decides whether each loaded file is tagged as
    /// `UserGlobal` or `ProjectLocal`.
    pub fn load_directory(
        &mut self,
        dir: &Path,
        source_kind: SourceKind,
    ) -> Result<usize, std::io::Error> {
        if !dir.exists() {
            return Ok(0);
        }
        let mut loaded = 0;
        for entry in std::fs::read_dir(dir)? {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    self.load_errors.push(LoadError::Io {
                        path: dir.to_path_buf(),
                        source: err,
                    });
                    continue;
                }
            };
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            match Self::load_file(&path) {
                Ok(definition) => {
                    let expected_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    if !expected_stem.is_empty() && expected_stem != definition.id {
                        self.load_errors.push(LoadError::FileNameMismatch {
                            path: path.clone(),
                            id: definition.id.clone(),
                        });
                        continue;
                    }
                    let source = match source_kind {
                        SourceKind::UserGlobal => AgentSource::UserGlobal { path: path.clone() },
                        SourceKind::ProjectLocal => {
                            AgentSource::ProjectLocal { path: path.clone() }
                        }
                    };
                    self.insert(LoadedAgent {
                        definition,
                        source,
                    });
                    loaded += 1;
                }
                Err(err) => {
                    self.load_errors.push(err);
                }
            }
        }
        Ok(loaded)
    }

    /// Read + parse + validate a single TOML file into an `AgentDefinition`.
    pub fn load_file(path: &Path) -> Result<AgentDefinition, LoadError> {
        let raw = std::fs::read_to_string(path).map_err(|source| LoadError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let definition: AgentDefinition =
            toml::from_str(&raw).map_err(|source| LoadError::Parse {
                path: path.to_path_buf(),
                source,
            })?;
        definition.validate().map_err(|source| LoadError::Invalid {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(definition)
    }

    /// Convenience: discover both user-global and project-local agent
    /// directories using standard jcode paths. `home` defaults to
    /// `dirs::home_dir()` (omitted here to keep this crate dep-light;
    /// callers pass the resolved home to avoid pulling `dirs`).
    pub fn discover_standard_paths(
        &mut self,
        home_dir: Option<&Path>,
        project_root: Option<&Path>,
    ) {
        if let Some(home) = home_dir {
            let user_dir = home.join(".jcode").join("agents");
            if let Err(err) = self.load_directory(&user_dir, SourceKind::UserGlobal) {
                self.load_errors.push(LoadError::Io {
                    path: user_dir,
                    source: err,
                });
            }
        }
        if let Some(root) = project_root {
            let project_dir = root.join(".jcode").join("agents");
            if let Err(err) = self.load_directory(&project_dir, SourceKind::ProjectLocal) {
                self.load_errors.push(LoadError::Io {
                    path: project_dir,
                    source: err,
                });
            }
        }
    }
}

/// Tag for `load_directory` so the caller decides how loaded entries are
/// labeled. The function itself doesn't care about jcode's path convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    UserGlobal,
    ProjectLocal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::OutputMode;
    use std::fs;

    fn write_toml(dir: &Path, name: &str, body: &str) {
        let path = dir.join(name);
        fs::write(&path, body).expect("write toml");
    }

    fn temp_dir(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "jcode-agent-registry-test-{}-{}-{}",
            name,
            std::process::id(),
            // Use atomics for a per-process counter so concurrent tests don't collide.
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    #[test]
    fn missing_dir_is_zero_load_not_error() {
        let mut reg = AgentRegistry::new();
        let n = reg
            .load_directory(Path::new("/nonexistent/jcode-test-dir"), SourceKind::UserGlobal)
            .unwrap();
        assert_eq!(n, 0);
        assert!(reg.is_empty());
    }

    #[test]
    fn loads_minimal_agent() {
        let dir = temp_dir("minimal");
        write_toml(
            &dir,
            "file-picker.toml",
            r#"
                id = "file-picker"
                display_name = "Fletcher"
            "#,
        );
        let mut reg = AgentRegistry::new();
        let n = reg.load_directory(&dir, SourceKind::ProjectLocal).unwrap();
        assert_eq!(n, 1);
        let loaded = reg.get("file-picker").expect("registered");
        assert_eq!(loaded.definition.display_name, "Fletcher");
        assert!(matches!(loaded.source, AgentSource::ProjectLocal { .. }));
    }

    #[test]
    fn project_overrides_user_overrides_builtin() {
        // Builtin
        let mut reg = AgentRegistry::new();
        let mut builtin_def = AgentDefinition {
            id: "editor".to_string(),
            display_name: "Builtin Editor".to_string(),
            publisher: None,
            version: "0.1.0".to_string(),
            prefer_tier: None,
            model_override: None,
            reasoning: None,
            tool_names: vec![],
            spawnable_agents: vec![],
            system_prompt: String::new(),
            instructions_prompt: None,
            step_prompt: None,
            spawner_prompt: None,
            inherit_parent_system_prompt: false,
            include_message_history: false,
            output_mode: OutputMode::LastMessage,
            output_schema: None,
        };
        reg.register_builtin(builtin_def.clone()).unwrap();
        assert_eq!(reg.get("editor").unwrap().definition.display_name, "Builtin Editor");

        // User
        let user_dir = temp_dir("user");
        write_toml(
            &user_dir,
            "editor.toml",
            r#"
                id = "editor"
                display_name = "User Editor"
            "#,
        );
        reg.load_directory(&user_dir, SourceKind::UserGlobal).unwrap();
        assert_eq!(reg.get("editor").unwrap().definition.display_name, "User Editor");

        // Project
        let proj_dir = temp_dir("proj");
        write_toml(
            &proj_dir,
            "editor.toml",
            r#"
                id = "editor"
                display_name = "Project Editor"
            "#,
        );
        reg.load_directory(&proj_dir, SourceKind::ProjectLocal).unwrap();
        assert_eq!(
            reg.get("editor").unwrap().definition.display_name,
            "Project Editor"
        );

        // Re-register builtin should NOT override the project entry.
        // (registers via the same `insert` priority path)
        builtin_def.display_name = "Builtin Editor v2".to_string();
        reg.register_builtin(builtin_def).unwrap();
        assert_eq!(
            reg.get("editor").unwrap().definition.display_name,
            "Project Editor",
            "builtin should not override project-local"
        );
    }

    #[test]
    fn malformed_toml_collected_as_load_error() {
        let dir = temp_dir("malformed");
        write_toml(&dir, "bad.toml", "id = \"this is missing close quote\n");
        let mut reg = AgentRegistry::new();
        reg.load_directory(&dir, SourceKind::UserGlobal).unwrap();
        assert!(reg.is_empty(), "no agents registered");
        assert_eq!(reg.load_errors().len(), 1);
        assert!(matches!(
            reg.load_errors()[0],
            LoadError::Parse { .. }
        ));
    }

    #[test]
    fn invalid_id_collected_as_load_error() {
        let dir = temp_dir("invalid-id");
        write_toml(
            &dir,
            "Bad_File.toml",
            r#"
                id = "Bad_Id"
                display_name = "Bad"
            "#,
        );
        let mut reg = AgentRegistry::new();
        reg.load_directory(&dir, SourceKind::UserGlobal).unwrap();
        assert!(reg.is_empty());
        assert_eq!(reg.load_errors().len(), 1);
        assert!(matches!(
            reg.load_errors()[0],
            LoadError::Invalid { .. }
        ));
    }

    #[test]
    fn filename_must_match_agent_id() {
        let dir = temp_dir("name-mismatch");
        write_toml(
            &dir,
            "wrong-name.toml",
            r#"
                id = "right-name"
                display_name = "X"
            "#,
        );
        let mut reg = AgentRegistry::new();
        reg.load_directory(&dir, SourceKind::UserGlobal).unwrap();
        assert!(reg.is_empty());
        assert_eq!(reg.load_errors().len(), 1);
        assert!(matches!(
            reg.load_errors()[0],
            LoadError::FileNameMismatch { .. }
        ));
    }

    #[test]
    fn skips_non_toml_files() {
        let dir = temp_dir("non-toml");
        fs::write(dir.join("README.md"), "not an agent").unwrap();
        fs::write(dir.join("config.json"), "{}").unwrap();
        write_toml(
            &dir,
            "valid.toml",
            r#"
                id = "valid"
                display_name = "v"
            "#,
        );
        let mut reg = AgentRegistry::new();
        let n = reg.load_directory(&dir, SourceKind::UserGlobal).unwrap();
        assert_eq!(n, 1);
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn iter_sorted_is_deterministic() {
        let dir = temp_dir("sort");
        for id in ["zeta", "alpha", "mid"] {
            write_toml(
                &dir,
                &format!("{id}.toml"),
                &format!(r#"id = "{id}"
display_name = "{id}"
"#),
            );
        }
        let mut reg = AgentRegistry::new();
        reg.load_directory(&dir, SourceKind::UserGlobal).unwrap();
        let ids: Vec<_> = reg.iter_sorted().iter().map(|a| a.definition.id.clone()).collect();
        assert_eq!(ids, vec!["alpha", "mid", "zeta"]);
    }

    #[test]
    fn discover_standard_paths_reads_both() {
        let home = temp_dir("home");
        let proj = temp_dir("proj");
        fs::create_dir_all(home.join(".jcode/agents")).unwrap();
        fs::create_dir_all(proj.join(".jcode/agents")).unwrap();
        write_toml(
            &home.join(".jcode/agents"),
            "user-only.toml",
            r#"id = "user-only"
display_name = "U"
"#,
        );
        write_toml(
            &proj.join(".jcode/agents"),
            "project-only.toml",
            r#"id = "project-only"
display_name = "P"
"#,
        );
        let mut reg = AgentRegistry::new();
        reg.discover_standard_paths(Some(&home), Some(&proj));
        assert_eq!(reg.len(), 2);
        assert!(reg.get("user-only").is_some());
        assert!(reg.get("project-only").is_some());
    }
}
