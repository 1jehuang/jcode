use super::{ExtensionError, PROVIDER_MANIFEST_FILE, ProviderManifest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub const PLUGIN_MANIFEST_VERSION: u32 = 1;
const MAX_PLUGIN_MANIFEST_BYTES: u64 = 256 * 1024;
const MAX_SKILL_METADATA_BYTES: u64 = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("plugin bundle root is not a directory: {0}")]
    InvalidRoot(PathBuf),
    #[error(
        "plugin bundle has no plugin manifest; expected plugin.json, .codex-plugin/plugin.json, or .claude-plugin/plugin.json"
    )]
    MissingManifest,
    #[error("plugin bundle has multiple plugin manifests: {0:?}")]
    AmbiguousManifest(Vec<PathBuf>),
    #[error("failed to read {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("plugin manifest {path} exceeds the {limit} byte limit")]
    ManifestTooLarge { path: PathBuf, limit: u64 },
    #[error("skill metadata {path} exceeds the {limit} byte limit")]
    SkillTooLarge { path: PathBuf, limit: u64 },
    #[error("failed to parse plugin manifest {path}: {source}")]
    ParseManifest {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("invalid plugin manifest: {0}")]
    InvalidManifest(String),
    #[error("invalid skill metadata {path}: {message}")]
    InvalidSkill { path: PathBuf, message: String },
    #[error("provider manifest error: {0}")]
    Provider(#[from] ExtensionError),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginManifest {
    #[serde(default = "default_plugin_manifest_version")]
    pub manifest_version: u32,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

fn default_plugin_manifest_version() -> u32 {
    PLUGIN_MANIFEST_VERSION
}

impl PluginManifest {
    fn validate(&self) -> Result<(), BundleError> {
        if self.manifest_version != PLUGIN_MANIFEST_VERSION {
            return Err(BundleError::InvalidManifest(format!(
                "unsupported manifest_version {}, expected {}",
                self.manifest_version, PLUGIN_MANIFEST_VERSION
            )));
        }
        validate_identifier("name", &self.name)?;
        validate_text("version", self.version.as_str())?;
        for (field, value) in [
            ("description", self.description.as_deref()),
            ("author", self.author.as_deref()),
            ("homepage", self.homepage.as_deref()),
            ("repository", self.repository.as_deref()),
            ("license", self.license.as_deref()),
        ] {
            if let Some(value) = value {
                validate_text(field, value)?;
            }
        }
        for keyword in &self.keywords {
            validate_text("keyword", keyword)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillMetadata {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleComponents {
    pub provider: bool,
    pub skills: bool,
    pub agents: bool,
    pub hooks: bool,
    pub mcp: bool,
    pub lsp: bool,
    pub monitors: bool,
    pub commands: bool,
    pub assets: bool,
    pub scripts: bool,
    pub apps: bool,
}

impl BundleComponents {
    pub fn unsupported(&self) -> Vec<&'static str> {
        let mut components = Vec::new();
        if self.agents {
            components.push("agents");
        }
        if self.hooks {
            components.push("hooks");
        }
        if self.mcp {
            components.push("mcp");
        }
        if self.lsp {
            components.push("lsp");
        }
        if self.monitors {
            components.push("monitors");
        }
        if self.commands {
            components.push("commands");
        }
        if self.assets {
            components.push("assets");
        }
        if self.scripts {
            components.push("scripts");
        }
        if self.apps {
            components.push("apps");
        }
        components
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginBundle {
    pub root: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: PluginManifest,
    #[serde(default)]
    pub provider_manifest: Option<ProviderManifest>,
    #[serde(default)]
    pub skills: Vec<SkillMetadata>,
    pub components: BundleComponents,
}

impl PluginBundle {
    /// Inspect a Claude/Codex-style bundle without executing any component.
    ///
    /// Only bounded metadata is read. Provider manifests are parsed for safety
    /// validation, but their executables are never started by this function.
    pub fn load(root: impl AsRef<Path>) -> Result<Self, BundleError> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|source| BundleError::Read {
                path: root.as_ref().to_path_buf(),
                source,
            })?;
        if !root.is_dir() {
            return Err(BundleError::InvalidRoot(root));
        }

        let manifest_candidates = [
            root.join("plugin.json"),
            root.join(".codex-plugin").join("plugin.json"),
            root.join(".claude-plugin").join("plugin.json"),
        ]
        .into_iter()
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
        let manifest_path = match manifest_candidates.as_slice() {
            [] => return Err(BundleError::MissingManifest),
            [path] => path.clone(),
            paths => return Err(BundleError::AmbiguousManifest(paths.to_vec())),
        };

        let manifest_bytes = read_bounded(&manifest_path, MAX_PLUGIN_MANIFEST_BYTES).map_err(
            |error| match error {
                BoundedReadError::TooLarge => BundleError::ManifestTooLarge {
                    path: manifest_path.clone(),
                    limit: MAX_PLUGIN_MANIFEST_BYTES,
                },
                BoundedReadError::Io(source) => BundleError::Read {
                    path: manifest_path.clone(),
                    source,
                },
            },
        )?;
        let manifest: PluginManifest =
            serde_json::from_slice(&manifest_bytes).map_err(|source| {
                BundleError::ParseManifest {
                    path: manifest_path.clone(),
                    source,
                }
            })?;
        manifest.validate()?;

        let provider_path = root.join(PROVIDER_MANIFEST_FILE);
        let provider_manifest = if provider_path.is_file() {
            Some(super::load_manifest_file(&provider_path)?)
        } else {
            None
        };

        let skills = discover_skills(&root)?;
        let components = detect_components(&root, provider_manifest.is_some(), !skills.is_empty());

        Ok(Self {
            root,
            manifest_path,
            manifest,
            provider_manifest,
            skills,
            components,
        })
    }

    pub fn unsupported_components(&self) -> Vec<&'static str> {
        self.components.unsupported()
    }
}

fn discover_skills(root: &Path) -> Result<Vec<SkillMetadata>, BundleError> {
    let skills_root = root.join("skills");
    if !skills_root.is_dir() {
        return Ok(Vec::new());
    }

    let mut skills = Vec::new();
    let mut seen = BTreeSet::new();
    let entries = fs::read_dir(&skills_root).map_err(|source| BundleError::Read {
        path: skills_root.clone(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| BundleError::Read {
            path: skills_root.clone(),
            source,
        })?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let directory_name = entry.file_name().to_string_lossy().into_owned();
        validate_identifier("skill directory", &directory_name).map_err(|error| {
            BundleError::InvalidSkill {
                path: path.clone(),
                message: error.to_string(),
            }
        })?;
        let skill_path = path.join("SKILL.md");
        if !skill_path.is_file() {
            continue;
        }
        let bytes =
            read_bounded(&skill_path, MAX_SKILL_METADATA_BYTES).map_err(|error| match error {
                BoundedReadError::TooLarge => BundleError::SkillTooLarge {
                    path: skill_path.clone(),
                    limit: MAX_SKILL_METADATA_BYTES,
                },
                BoundedReadError::Io(source) => BundleError::Read {
                    path: skill_path.clone(),
                    source,
                },
            })?;
        let (name, description) = parse_skill_frontmatter(&bytes, &skill_path)?;
        if !seen.insert(name.clone()) {
            return Err(BundleError::InvalidSkill {
                path: skill_path,
                message: format!("duplicate skill name '{name}'"),
            });
        }
        skills.push(SkillMetadata {
            name,
            description,
            path: skill_path,
        });
    }
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(skills)
}

fn parse_skill_frontmatter(
    bytes: &[u8],
    path: &Path,
) -> Result<(String, Option<String>), BundleError> {
    let text = std::str::from_utf8(bytes).map_err(|_| BundleError::InvalidSkill {
        path: path.to_path_buf(),
        message: "SKILL.md must be UTF-8".to_string(),
    })?;
    let directory_name = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("skill")
        .to_string();
    if !text.lines().next().is_some_and(|line| line.trim() == "---") {
        return Ok((directory_name, None));
    }

    let mut name = None;
    let mut description = None;
    let mut closed = false;
    for line in text.lines().skip(1) {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches(['"', '\'']);
        match key.trim() {
            "name" => name = Some(value.to_string()),
            "description" => description = Some(value.to_string()),
            _ => {}
        }
    }
    if !closed {
        return Err(BundleError::InvalidSkill {
            path: path.to_path_buf(),
            message: "unterminated frontmatter".to_string(),
        });
    }
    let name = name
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(directory_name);
    validate_identifier("skill name", &name).map_err(|error| BundleError::InvalidSkill {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    if let Some(description) = &description {
        validate_text("skill description", description)?;
    }
    Ok((name, description))
}

fn detect_components(root: &Path, provider: bool, skills: bool) -> BundleComponents {
    BundleComponents {
        provider,
        skills,
        agents: root.join("agents").is_dir(),
        hooks: root.join("hooks.json").is_file() || root.join("hooks").join("hooks.json").is_file(),
        mcp: root.join(".mcp.json").is_file(),
        lsp: root.join(".lsp.json").is_file(),
        monitors: root.join("monitors").join("monitors.json").is_file(),
        commands: root.join("commands").is_dir(),
        assets: root.join("assets").is_dir(),
        scripts: root.join("scripts").is_dir() || root.join("bin").is_dir(),
        apps: root.join(".app.json").is_file(),
    }
}

fn validate_identifier(field: &str, value: &str) -> Result<(), BundleError> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value.chars().enumerate().all(|(index, ch)| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || (index > 0 && matches!(ch, '-' | '_'))
        });
    if valid {
        Ok(())
    } else {
        Err(BundleError::InvalidManifest(format!(
            "{field} must match [a-z0-9][a-z0-9_-]{{0,63}}"
        )))
    }
}

fn validate_text(field: &str, value: &str) -> Result<(), BundleError> {
    if value.trim().is_empty() || value.contains(['\0', '\n', '\r']) {
        return Err(BundleError::InvalidManifest(format!(
            "{field} must be non-empty and contain no control characters"
        )));
    }
    Ok(())
}

enum BoundedReadError {
    TooLarge,
    Io(io::Error),
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, BoundedReadError> {
    let metadata = fs::metadata(path).map_err(BoundedReadError::Io)?;
    if metadata.len() > limit {
        return Err(BoundedReadError::TooLarge);
    }
    let mut file = File::open(path).map_err(BoundedReadError::Io)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes).map_err(BoundedReadError::Io)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn loads_codex_manifest_and_discovers_skills_without_execution() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".codex-plugin")).unwrap();
        fs::create_dir_all(dir.path().join("skills").join("hello")).unwrap();
        fs::write(
            dir.path().join(".codex-plugin/plugin.json"),
            r#"{"name":"hello-plugin","version":"1.0.0","description":"Hello"}"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("skills/hello/SKILL.md"),
            "---\nname: hello\ndescription: Say hello\n---\n# Hello\n",
        )
        .unwrap();

        let bundle = PluginBundle::load(dir.path()).unwrap();
        assert_eq!(bundle.manifest.name, "hello-plugin");
        assert_eq!(bundle.skills[0].name, "hello");
        assert!(bundle.unsupported_components().is_empty());
    }

    #[test]
    fn detects_unsupported_surfaces_without_loading_them() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("plugin.json"),
            r#"{"name":"rich-plugin","version":"1.0.0"}"#,
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("agents")).unwrap();
        fs::write(dir.path().join(".mcp.json"), "{}").unwrap();
        fs::write(dir.path().join("hooks.json"), "{}").unwrap();

        let bundle = PluginBundle::load(dir.path()).unwrap();
        assert_eq!(
            bundle.unsupported_components(),
            vec!["agents", "hooks", "mcp"]
        );
    }

    #[test]
    fn rejects_ambiguous_manifests() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("plugin.json"),
            r#"{"name":"one","version":"1.0.0"}"#,
        )
        .unwrap();
        fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
        fs::write(
            dir.path().join(".claude-plugin/plugin.json"),
            r#"{"name":"two","version":"1.0.0"}"#,
        )
        .unwrap();

        assert!(matches!(
            PluginBundle::load(dir.path()),
            Err(BundleError::AmbiguousManifest(_))
        ));
    }
}
