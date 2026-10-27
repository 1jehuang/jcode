//! Shared, contained component discovery for enabled plugin registries.
use std::path::{Path, PathBuf};

/// Recovered Claude `$Jo` / `qs`, embedded-216124080.js chars 66,000..69,000.
/// Metadata is declarative only: discovery never runs models, tools or shell.
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

pub(crate) fn command_declarations(
    root: &Path,
) -> anyhow::Result<Option<Vec<(String, CommandDeclaration)>>> {
    let path = root.join(".claude-plugin/plugin.json");
    if !path.exists() {
        return Ok(None);
    }
    let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let Some(value) = manifest.get("commands").filter(|v| v.is_object()) else {
        return Ok(None);
    };
    validate_command_map(root, value)?;
    let mut declarations = Vec::new();
    for (name, value) in value.as_object().unwrap() {
        let mut declaration: CommandDeclaration = serde_json::from_value(value.clone())?;
        if let Some(source) = declaration.source.as_deref().filter(|s| !s.is_empty()) {
            declaration.source = Some(command_source(root, source)?.to_string_lossy().into_owned());
        }
        declarations.push((name.clone(), declaration));
    }
    Ok(Some(declarations))
}

#[cfg(test)]
#[path = "plugin_components_tests_fixture.rs"]
pub(crate) mod fixture;

/// Skills extend their default directory; explicit command/agent paths replace
/// defaults, matching Claude discovery. Canonicalization blocks host-file escapes.
pub(crate) fn component_paths(root: &Path, component: &str) -> Vec<PathBuf> {
    let Ok(root) = root.canonicalize() else {
        return Vec::new();
    };
    let mut candidates = vec![root.join(component)];
    if let Ok(source) = std::fs::read_to_string(root.join(".claude-plugin/plugin.json"))
        && let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&source)
        && let Some(value) = manifest.get(component)
    {
        if matches!(component, "commands" | "agents") {
            candidates.clear();
        }
        match value {
            serde_json::Value::String(path) => candidates.push(root.join(path)),
            serde_json::Value::Array(paths) => candidates.extend(
                paths
                    .iter()
                    .filter_map(|p| p.as_str())
                    .map(|p| root.join(p)),
            ),
            _ => {}
        }
    }
    let mut paths = Vec::new();
    for path in candidates {
        if let Ok(path) = path.canonicalize()
            && path.starts_with(&root)
            && !paths.contains(&path)
        {
            paths.push(path);
        }
    }
    paths
}

/// Markdown components support nested directories, bounded to avoid pathological
/// trees. Relative names use colon separators, matching slash-command namespaces.
pub(crate) fn markdown_files(root: &Path) -> Vec<(String, PathBuf)> {
    fn walk(path: &Path, base: &Path, depth: usize, out: &mut Vec<(String, PathBuf)>) {
        if depth == 0 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            // Never follow directory symlinks: loops and component escapes.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                walk(&path, base, depth - 1, out);
            } else if kind.is_file() && path.extension().and_then(|s| s.to_str()) == Some("md") {
                let Ok(relative) = path.strip_prefix(base) else {
                    continue;
                };
                let name = relative
                    .with_extension("")
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(":");
                out.push((name, path));
            }
        }
    }
    if root.is_file() {
        return root
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|name| vec![(name.to_owned(), root.to_owned())])
            .unwrap_or_default();
    }
    let mut files = Vec::new();
    walk(root, root, 8, &mut files);
    files
}
