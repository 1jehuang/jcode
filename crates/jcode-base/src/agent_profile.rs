//! Session agent profiles are plain Markdown, not skills or system-prompt overrides.
//! YAML frontmatter is rejected. Selection snapshots the exact UTF-8 file contents.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProfile {
    pub name: String,
    pub instructions: String,
}

fn validate_name(name: &str) -> Result<()> {
    let mut bytes = name.bytes();
    if !bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || matches!(name, "off" | "reload")
    {
        bail!(
            "Invalid agent profile name {name:?}: use ASCII letters/digits followed by letters, digits, '_' or '-'; 'off' and 'reload' are reserved"
        );
    }
    Ok(())
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
    Ok(names.into_iter().collect())
}

/// Load the project profile first, falling back to the global directory only if absent.
/// An invalid/unreadable project profile never silently falls back to global instructions.
pub fn load(name: &str, working_dir: &Path) -> Result<AgentProfile> {
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
        if instructions.trim().is_empty() {
            bail!(
                "Agent profile {} is empty; add Markdown instructions before selecting it",
                path.display()
            );
        }
        if instructions
            .trim_start_matches('\u{feff}')
            .trim_start()
            .lines()
            .next()
            .is_some_and(|line| line.trim() == "---")
        {
            bail!(
                "Agent profile {} uses YAML frontmatter, which is unsupported; use plain Markdown instructions only",
                path.display()
            );
        }
        return Ok(AgentProfile {
            name: name.to_owned(),
            instructions,
        });
    }
    bail!(
        "Agent profile {name:?} not found in {} or {}",
        working_dir.join(".jcode/agents").display(),
        crate::storage::jcode_dir()?.join("agents").display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "x.md",
            "é",
            "off",
            "reload",
            "-x",
        ] {
            assert!(validate_name(name).is_err(), "{name}");
        }
        for name in ["x", "a-B_12", "123"] {
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
        std::fs::write(project.join("writer.md"), "  project\n")?;
        std::fs::write(project.join("off.md"), "ignored")?;
        assert_eq!(list(cwd.path())?, vec!["alpha", "writer"]);
        let selected = load("writer", cwd.path())?;
        assert_eq!(selected.instructions, "  project\n");
        std::fs::write(project.join("writer.md"), "changed")?;
        assert_eq!(selected.instructions, "  project\n");
        for body in [" \n", "---\nname: injected\n---\nbody"] {
            std::fs::write(project.join("writer.md"), body)?;
            assert!(load("writer", cwd.path()).is_err());
        }
        std::fs::remove_file(project.join("writer.md"))?;
        assert_eq!(load("writer", cwd.path())?.instructions, "global");
        assert!(load("missing", cwd.path()).is_err());
        Ok(())
    }
}
