//! Contained validation of declarative plugin component paths.
use std::path::{Path, PathBuf};

/// Declarative plugin command metadata; validation never executes commands.
#[allow(
    dead_code,
    reason = "Deserialization validates all declared metadata types."
)]
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommandDeclaration {
    pub source: Option<String>,
    pub content: Option<String>,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
    pub model: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
}

pub(crate) fn validate_command_map(root: &Path, value: &serde_json::Value) -> anyhow::Result<()> {
    let map = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("commands must be a named map"))?;
    for (name, value) in map {
        anyhow::ensure!(
            !name.is_empty() && !name.chars().any(char::is_control),
            "invalid command name"
        );
        let declaration: CommandDeclaration = serde_json::from_value(value.clone())?;
        let source = declaration.source.as_deref().filter(|s| !s.is_empty());
        let content = declaration.content.as_deref().filter(|s| !s.is_empty());
        anyhow::ensure!(
            source.is_some() ^ content.is_some(),
            "command requires source XOR content"
        );
        if let Some(source) = source {
            command_source(root, source)?;
        }
    }
    Ok(())
}

fn command_source(root: &Path, source: &str) -> anyhow::Result<PathBuf> {
    let relative = Path::new(source);
    anyhow::ensure!(
        !relative.is_absolute()
            && !relative
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
        "command source escapes plugin"
    );
    let root = root.canonicalize()?;
    let path = root.join(relative).canonicalize()?;
    anyhow::ensure!(
        path.starts_with(root) && path.is_file(),
        "command source escapes plugin or is not a file"
    );
    Ok(path)
}
