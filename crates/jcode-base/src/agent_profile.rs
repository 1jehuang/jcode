//! Session profiles accept Markdown with optional OpenCode-style YAML frontmatter.
//! Only the instruction body is snapshotted; metadata never changes runtime policy.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProfile {
    pub name: String,
    pub instructions: String,
    /// Empty or missing means selected-profile isolation. Only explicit file
    /// snapshots join these instructions; normal auto-loading resumes on off.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instruction_sources: Vec<crate::instruction_sources::InstructionSourceSnapshot>,
}

fn validate_name(name: &str) -> Result<()> {
    let mut bytes = name.bytes();
    if !bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        || name.ends_with('.')
        || name.contains("..")
        || matches!(name, "off" | "reload")
    {
        bail!(
            "Invalid agent profile name {name:?}: start with an ASCII letter/digit, then use letters, digits, '_', '-' or internal single dots; 'off' and 'reload' are reserved"
        );
    }
    Ok(())
}

#[cfg(test)]
fn parse_instructions(source: &str, path: &Path) -> Result<String> {
    parse_profile_document(source, path).map(|(instructions, _)| instructions)
}

/// Return the Markdown body and ignored frontmatter field names without changing
/// the persisted profile schema. Name and description are informational metadata.
fn parse_profile_document(source: &str, path: &Path) -> Result<(String, Vec<String>)> {
    let mut ignored_fields = Vec::new();
    let candidate = source.trim_start_matches('\u{feff}');
    let mut lines = candidate.split_inclusive('\n');
    let mut offset = 0;
    let mut opening = None;
    for line in lines.by_ref() {
        offset += line.len();
        if line.trim().is_empty() {
            continue;
        }
        if line.trim_end() == "---" {
            opening = Some(offset);
        }
        break;
    }
    let body = if let Some(header_start) = opening {
        let mut closing = None;
        for line in lines {
            let start = offset;
            offset += line.len();
            // Leading indentation belongs to YAML block scalars, not delimiters.
            if matches!(line.trim_end(), "---" | "...") {
                closing = Some((start, offset));
                break;
            }
        }
        let (header_end, body_start) = closing.with_context(|| {
            format!(
                "Agent profile {} has unclosed YAML frontmatter",
                path.display()
            )
        })?;
        let metadata: serde_yaml::Value =
            serde_yaml::from_str(&candidate[header_start..header_end]).with_context(|| {
                format!(
                    "Invalid YAML frontmatter in agent profile {}",
                    path.display()
                )
            })?;
        if !matches!(
            metadata,
            serde_yaml::Value::Mapping(_) | serde_yaml::Value::Null
        ) {
            bail!(
                "Agent profile {} YAML frontmatter must be a mapping",
                path.display()
            );
        }
        if let serde_yaml::Value::Mapping(mapping) = &metadata {
            for key in mapping.keys() {
                let field = key.as_str().map(str::to_owned).unwrap_or_else(|| {
                    serde_yaml::to_string(key)
                        .unwrap_or_default()
                        .trim()
                        .to_owned()
                });
                if !matches!(field.as_str(), "name" | "description") {
                    ignored_fields.push(field);
                }
            }
            ignored_fields.sort();
        }
        &candidate[body_start..]
    } else {
        source
    };
    if body.trim_start_matches('\u{feff}').trim().is_empty() {
        bail!(
            "Agent profile {} has an empty instruction body; add Markdown instructions",
            path.display()
        );
    }
    Ok((body.to_owned(), ignored_fields))
}

/// Explain a successful selection without implying that ignored metadata rejected
/// the Markdown body. Call only after the profile snapshot has been persisted.
pub fn application_notice(name: &str, ignored_fields: &[String]) -> String {
    let mut notice = format!(
        "Agent profile {name} applied. Markdown instructions take effect next turn. Model, tools and permissions remain unchanged."
    );
    if !ignored_fields.is_empty() {
        notice.push_str(&format!(
            " Ignored YAML frontmatter fields: {}. The Markdown instruction body still applies.",
            ignored_fields.join(", ")
        ));
    }
    notice
}

fn directories(working_dir: &Path) -> Result<[PathBuf; 2]> {
    Ok([
        working_dir.join(".jcode/agents"),
        crate::storage::jcode_dir()?.join("agents"),
    ])
}

/// List safe Markdown profile names in deterministic order, deduplicating project overrides.
/// File contents are validated at load time, so invalid profiles can still be discovered.
pub fn list(working_dir: &Path) -> Result<Vec<String>> {
    let mut names = BTreeSet::new();
    for directory in directories(working_dir)? {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Read agent profiles in {}", directory.display()));
            }
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_file()
                || path.extension().and_then(|s| s.to_str()) != Some("md")
            {
                continue;
            }
            if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                if validate_name(name).is_ok() {
                    names.insert(name.to_owned());
                }
            }
        }
    }
    for (name, _) in plugin_profiles(working_dir) {
        names.insert(name);
    }
    Ok(names.into_iter().collect())
}

/// Load the project profile first, falling back to the global directory only if absent.
/// An invalid/unreadable project profile never silently falls back to global instructions.
pub fn load(name: &str, working_dir: &Path) -> Result<AgentProfile> {
    load_with_metadata(name, working_dir).map(|(profile, _)| profile)
}

/// Load instructions with a non-persisted list of ignored metadata fields.
pub fn load_with_metadata(name: &str, working_dir: &Path) -> Result<(AgentProfile, Vec<String>)> {
    if name.contains(':') {
        let path = plugin_profiles(working_dir)
            .into_iter()
            .find_map(|(candidate, path)| (candidate == name).then_some(path))
            .with_context(|| format!("Enabled plugin agent profile {name:?} not found"))?;
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("Read agent profile {}", path.display()))?;
        let (instructions, metadata) = parse_profile_document(&source, &path)?;
        return Ok((
            AgentProfile {
                name: name.to_owned(),
                instructions,
                instruction_sources: Vec::new(),
            },
            metadata,
        ));
    }
    validate_name(name)?;
    for directory in directories(working_dir)? {
        let path = directory.join(format!("{name}.md"));
        let instructions = match std::fs::read_to_string(&path) {
            Ok(instructions) => instructions,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Read agent profile {}", path.display()));
            }
        };
        let (instructions, metadata) = parse_profile_document(&instructions, &path)?;
        return Ok((
            AgentProfile {
                name: name.to_owned(),
                instructions,
                instruction_sources: Vec::new(),
            },
            metadata,
        ));
    }
    bail!(
        "Agent profile {name:?} not found in {} or {}",
        working_dir.join(".jcode/agents").display(),
        crate::storage::jcode_dir()?.join("agents").display()
    )
}

/// Resolve plugin agent files on every list/load, so disabled and uninstalled
/// agents cannot remain selectable through an old process-wide cache.
fn plugin_profiles(working_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut profiles = std::collections::BTreeMap::new();
    for plugin in crate::plugin::enabled_plugins_for_dir(working_dir) {
        for component in crate::plugin_components::component_paths(&plugin.root, "agents") {
            for (name, path) in crate::plugin_components::markdown_files(&component) {
                if name.split(':').all(|part| validate_name(part).is_ok()) {
                    profiles.insert(format!("{}:{name}", plugin.name), path);
                }
            }
        }
    }
    profiles.into_iter().collect()
}

#[cfg(test)]
#[path = "agent_profile_plugin_tests.rs"]
mod plugin_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_notice_only_warns_about_fields_actually_present() -> Result<()> {
        let path = Path::new("reviewer.md");
        for source in [
            "Review carefully.\n",
            "---\nname: Reviewer\ndescription: A careful reviewer\n---\nReview carefully.\n",
        ] {
            let (body, fields) = parse_profile_document(source, path)?;
            assert_eq!(body, "Review carefully.\n");
            assert!(fields.is_empty());
            let notice = application_notice("reviewer", &fields);
            assert!(notice.starts_with("Agent profile reviewer applied."));
            assert!(notice.contains("Markdown instructions take effect next turn."));
            assert!(!notice.contains("frontmatter"));
            assert!(!notice.contains("ignored"));
            assert!(!notice.contains("not applied"));
        }
        let (body, fields) = parse_profile_document(
            "---\nname: Reviewer\ndescription: Review\ntools: [Read, Bash]\nmodel: inherit\npermission:\n  edit: deny\ncustom: true\n---\nReview carefully.\n",
            path,
        )?;
        assert_eq!(body, "Review carefully.\n");
        assert_eq!(fields, ["custom", "model", "permission", "tools"]);
        let notice = application_notice("reviewer", &fields);
        assert!(notice.starts_with("Agent profile reviewer applied."));
        assert!(
            notice.contains("Ignored YAML frontmatter fields: custom, model, permission, tools.")
        );
        assert!(notice.contains("The Markdown instruction body still applies."));
        assert!(!notice.contains("description"));
        Ok(())
    }

    #[test]
    fn yaml_frontmatter_preserves_body_and_ignores_runtime_metadata() -> Result<()> {
        let body = "\n  Review carefully.\n\n---\nKeep this body rule.\n";
        let source = format!(
            "---\ndescription: Reviewer\nmode: all\nname: other-name\nmodel: vendor/model\npermission:\n  edit: deny\ntools:\n  bash: false\n---\n{body}"
        );
        assert_eq!(parse_instructions(&source, Path::new("reviewer.md"))?, body);
        assert_eq!(
            parse_instructions("plain\n---\nbody", Path::new("plain.md"))?,
            "plain\n---\nbody"
        );
        Ok(())
    }

    #[test]
    fn yaml_frontmatter_handles_crlf_bom_and_indented_block_delimiters() -> Result<()> {
        let source =
            "\u{feff}\r\n---\r\ndescription: |\r\n  ---\r\n  explanation\r\n...\r\n\r\nBody é\r\n";
        assert_eq!(
            parse_instructions(source, Path::new("agent.md"))?,
            "\r\nBody é\r\n"
        );
        assert_eq!(
            parse_instructions("---\n---\nBody", Path::new("agent.md"))?,
            "Body"
        );
        Ok(())
    }

    #[test]
    fn yaml_frontmatter_rejects_invalid_metadata_and_empty_body() {
        for source in [
            "---\nname: [broken\n---\nBody",
            "\u{feff}\r\n \t",
            "---\ndescription: valid\n---",
            "---\nname: value\nBody",
            "---\n- list\n---\nBody",
            "---\nscalar\n---\nBody",
            "---\nname: value\n---\n \n",
        ] {
            let error = parse_instructions(source, Path::new("broken.md")).unwrap_err();
            assert!(format!("{error:#}").contains("broken.md"));
        }
    }

    #[test]
    fn names_reject_paths_and_reserved_commands() {
        for name in [
            "",
            ".",
            "..",
            "../x",
            "a/b",
            "a\\b",
            "/tmp/x",
            "two words",
            "a..b",
            "a.",
            ".hidden",
            "é",
            "off",
            "reload",
            "-x",
        ] {
            assert!(validate_name(name).is_err(), "{name}");
        }
        for name in ["x", "a-B_12", "123", "kimik3.1", "review.v2", "x.md"] {
            assert!(validate_name(name).is_ok());
        }
    }

    #[test]
    fn project_precedence_snapshot_and_invalid_body() -> Result<()> {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let previous = std::env::var_os("JCODE_HOME");
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                match &self.0 {
                    Some(value) => crate::env::set_var("JCODE_HOME", value),
                    None => crate::env::remove_var("JCODE_HOME"),
                }
            }
        }
        let _restore = Restore(previous);
        crate::env::set_var("JCODE_HOME", home.path());
        let global = home.path().join("agents");
        let project = cwd.path().join(".jcode/agents");
        std::fs::create_dir_all(&global)?;
        std::fs::create_dir_all(&project)?;
        std::fs::write(global.join("writer.md"), "global")?;
        std::fs::write(global.join("alpha.md"), "alpha")?;
        std::fs::write(global.join("kimik3.1.md"), "global dotted profile")?;
        std::fs::write(project.join("kimik3.1.md"), "project dotted profile")?;
        std::fs::write(project.join("writer.md"), "  project\n")?;
        std::fs::write(project.join("off.md"), "ignored")?;
        assert_eq!(list(cwd.path())?, vec!["alpha", "kimik3.1", "writer"]);
        assert_eq!(
            load("kimik3.1", cwd.path())?.instructions,
            "project dotted profile"
        );
        let selected = load("writer", cwd.path())?;
        assert_eq!(selected.instructions, "  project\n");
        std::fs::write(
            project.join("writer.md"),
            "---\nname: Writer\ndescription: Common metadata\n---\nYAML body\n",
        )?;
        let (profile, fields) = load_with_metadata("writer", cwd.path())?;
        assert_eq!(profile.instructions, "YAML body\n");
        assert!(fields.is_empty());
        assert!(profile.instruction_sources.is_empty());
        std::fs::write(
            project.join("writer.md"),
            "---\nmodel: inherit\ntools: [Read, Bash]\n---\nPolicy body\n",
        )?;
        let (profile, fields) = load_with_metadata("writer", cwd.path())?;
        assert_eq!(profile.instructions, "Policy body\n");
        assert_eq!(fields, ["model", "tools"]);
        std::fs::write(project.join("writer.md"), "changed")?;
        assert_eq!(selected.instructions, "  project\n");
        for body in [" \n", "---\nname: [unterminated\n---\nbody"] {
            std::fs::write(project.join("writer.md"), body)?;
            assert!(load("writer", cwd.path()).is_err());
        }
        std::fs::remove_file(project.join("writer.md"))?;
        assert_eq!(load("writer", cwd.path())?.instructions, "global");
        assert!(load("missing", cwd.path()).is_err());
        Ok(())
    }
}
