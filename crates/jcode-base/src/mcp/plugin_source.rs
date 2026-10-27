//! Read-only MCP discovery from enabled plugins. Never starts plugin processes.

use super::protocol::{McpConfig, McpServerConfig};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) const PROJECT_SOURCE_PREFIX: &str = "plugin:project:";
pub(super) const USER_SOURCE_PREFIX: &str = "plugin:user:";
pub(super) const LOCAL_SOURCE_PREFIX: &str = "plugin:local:";

/// The plugin manager resolves enablement and scope; this adapter only reads files.
pub(super) struct PluginMcpSource {
    pub id: String,
    pub root: PathBuf,
    pub scope: crate::plugin::PluginScope,
    pub plugin: Option<crate::plugin::PluginRoot>,
}

fn namespace_component(value: &str) -> String {
    // Escape separators as well as the escape character, so distinct identities
    // cannot collapse onto the same dispatch name.
    value.replace('%', "%25").replace(':', "%3A")
}

fn server_name(plugin: &str, server: &str) -> String {
    format!(
        "plugin:{}:{}",
        namespace_component(plugin),
        namespace_component(server)
    )
}

fn read_json(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&raw) {
        Ok(value) => Some(value),
        Err(_) => {
            crate::logging::warn(&format!("MCP: Invalid plugin JSON in {}", path.display()));
            None
        }
    }
}

fn confined_file(root: &Path, relative: &str) -> Option<PathBuf> {
    // Paths are relative to the installed plugin, not the daemon's cwd. Reject
    // escapes (including symlinks) instead of reading arbitrary local files.
    let relative = relative.replace("${CLAUDE_PLUGIN_ROOT}", &root.to_string_lossy());
    let path = Path::new(&relative);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let path = path.canonicalize().ok()?;
    path.starts_with(root).then_some(path)
}

fn merge_map(
    merged: &mut McpConfig,
    source: &PluginMcpSource,
    root: &Path,
    value: &Value,
    path: &Path,
) {
    let map = value.get("mcpServers").unwrap_or(value);
    let Some(map) = map.as_object() else { return };
    for (name, value) in map {
        let value = value.clone();
        let data = match source
            .plugin
            .as_ref()
            .map(crate::plugin::data_path)
            .transpose()
        {
            Ok(data) => data,
            Err(error) => {
                crate::logging::warn(&format!("MCP: Invalid plugin data identity: {error:#}"));
                continue;
            }
        };
        let Ok(mut config) = serde_json::from_value::<McpServerConfig>(value) else {
            crate::logging::warn(&format!(
                "MCP: Invalid plugin server entry in {}",
                path.display()
            ));
            continue;
        };
        if !config.is_runnable() {
            continue;
        }
        // The root is an intrinsic plugin value, never a caller's environment
        // variable or an entry supplied by the plugin manifest itself.
        // Plugin configs may differ between projects under the same namespace.
        // The shared pool keys only by name, so keep these clients session-owned.
        config.shared = false;
        if let Some(data) = data {
            config.env.insert(
                "CLAUDE_PLUGIN_DATA".into(),
                data.to_string_lossy().into_owned(),
            );
        }
        config.env.insert(
            "CLAUDE_PLUGIN_ROOT".into(),
            root.to_string_lossy().into_owned(),
        );
        let name = server_name(&source.id, name);
        let prefix = match source.scope {
            crate::plugin::PluginScope::User => USER_SOURCE_PREFIX,
            crate::plugin::PluginScope::Project => PROJECT_SOURCE_PREFIX,
            crate::plugin::PluginScope::Local => LOCAL_SOURCE_PREFIX,
        };
        merged.sources.insert(
            name.clone(),
            format!("{prefix}{} [{}]", source.id, path.display()),
        );
        merged.servers.insert(name, config);
    }
}

fn merge_manifest_value(
    merged: &mut McpConfig,
    source: &PluginMcpSource,
    root: &Path,
    value: &Value,
    manifest: &Path,
) {
    match value {
        Value::String(relative) => {
            if let Some(path) = confined_file(root, relative)
                && let Some(value) = read_json(&path)
            {
                merge_map(merged, source, root, &value, &path);
            }
        }
        Value::Array(values) => {
            for value in values {
                merge_manifest_value(merged, source, root, value, manifest);
            }
        }
        Value::Object(_) => merge_map(merged, source, root, value, manifest),
        _ => {}
    }
}

pub(super) fn load_for_dir(project_dir: Option<&Path>) -> McpConfig {
    let roots = match project_dir {
        Some(dir) => crate::plugin::enabled_plugins_for_dir(dir),
        None => crate::plugin::enabled_user_plugins(),
    };
    load_sources(roots.into_iter().map(|plugin| PluginMcpSource {
        id: format!("{}@{}", plugin.name, plugin.marketplace),
        root: plugin.root.clone(),
        scope: plugin.scope,
        plugin: Some(plugin),
    }))
}

/// Resolve current enabled identity rather than trusting a config environment
/// variable. Discovery remains read-only; this is called only by child startup.
pub(super) fn prepare_for_execution(
    name: &str,
    config: &McpServerConfig,
    project_dir: Option<&Path>,
) -> anyhow::Result<()> {
    let effective = McpConfig::load_for_dir(project_dir);
    let expected = effective.servers.get(name).ok_or_else(|| {
        anyhow::anyhow!("Plugin MCP source is no longer enabled; reload plugin sources")
    })?;
    anyhow::ensure!(
        effective
            .sources
            .get(name)
            .is_some_and(|source| super::is_plugin_mcp_source(source)),
        "MCP source is no longer a plugin"
    );
    anyhow::ensure!(
        !effective.pending_approval.contains(name),
        "Plugin MCP source requires project approval"
    );
    anyhow::ensure!(
        serde_json::to_value(expected)? == serde_json::to_value(config)?,
        "Plugin MCP configuration changed; reload plugin sources before connecting"
    );
    let roots = match project_dir {
        Some(dir) => crate::plugin::enabled_plugins_for_dir(dir),
        None => crate::plugin::enabled_user_plugins(),
    };
    for plugin in roots {
        let source = PluginMcpSource {
            id: format!("{}@{}", plugin.name, plugin.marketplace),
            root: plugin.root.clone(),
            scope: plugin.scope,
            plugin: Some(plugin.clone()),
        };
        let discovered = load_sources([source]);
        if discovered.servers.contains_key(name)
            && discovered.sources.get(name) == effective.sources.get(name)
        {
            if !config.is_remote() {
                crate::plugin::prepare_data_dir(&plugin)?;
            }
            return Ok(());
        }
    }
    anyhow::bail!("Plugin MCP source is no longer enabled; reload plugin sources")
}

pub(super) fn load_sources(sources: impl IntoIterator<Item = PluginMcpSource>) -> McpConfig {
    let mut merged = McpConfig::default();
    for source in sources {
        let Ok(root) = source.root.canonicalize() else {
            continue;
        };
        if let Some(path) = confined_file(&root, ".mcp.json")
            && let Some(value) = read_json(&path)
        {
            merge_map(&mut merged, &source, &root, &value, &path);
        }
        if let Some(manifest) = confined_file(&root, ".claude-plugin/plugin.json")
            && let Some(value) = read_json(&manifest)
            && let Some(value) = value.get("mcpServers")
        {
            merge_manifest_value(&mut merged, &source, &root, value, &manifest);
        }
    }
    merged
}

#[cfg(test)]
#[path = "plugin_source_tests.rs"]
mod tests;
