//! Enabled-plugin language servers. Each invocation owns its process, so plugin
//! disablement, workspace changes and cancelled calls cannot leak a shared server.
use super::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

const MAX_MESSAGE: usize = 8 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

pub struct LspTool;
impl LspTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerConfig {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    extension_to_language: BTreeMap<String, String>,
    #[serde(default)]
    initialization_options: Option<Value>,
    #[serde(default)]
    settings: Option<Value>,
    #[serde(default)]
    transport: Option<String>,
}

#[derive(Debug)]
struct PluginServer {
    name: String,
    root: PathBuf,
    config: ServerConfig,
    plugin: Option<jcode_base::plugin::PluginRoot>,
}

#[cfg(test)]
fn expand(value: &str, root: &Path, workspace: &Path) -> Result<String> {
    expand_string_with_data(value, root, workspace, false, None)
}

fn expand_string_with_data(
    value: &str,
    root: &Path,
    workspace: &Path,
    preserve_missing: bool,
    data: Option<&Path>,
) -> Result<String> {
    let mut result = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        result.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .context("Unclosed LSP configuration variable")?;
        let name = &tail[..end];
        let replacement = match name {
            "CLAUDE_PLUGIN_DATA" => data
                .context("Missing plugin data identity")?
                .to_string_lossy()
                .into_owned(),
            "CLAUDE_PLUGIN_ROOT" | "JCODE_PLUGIN_ROOT" => root.to_string_lossy().into_owned(),
            "CLAUDE_PROJECT_DIR" | "JCODE_PROJECT_DIR" | "workspaceFolder" => {
                workspace.to_string_lossy().into_owned()
            }
            _ => match std::env::var(name) {
                Ok(value) => value,
                Err(_) if preserve_missing => format!("${{{name}}}"),
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("Unresolved LSP configuration variable: {name}"));
                }
            },
        };
        result.push_str(&replacement);
        rest = &tail[end + 1..];
    }
    result.push_str(rest);
    Ok(result)
}

#[cfg(test)]
fn expand_json(value: &mut Value, root: &Path, workspace: &Path) -> Result<()> {
    expand_json_with_data(value, root, workspace, None)
}

fn expand_json_with_data(
    value: &mut Value,
    root: &Path,
    workspace: &Path,
    data: Option<&Path>,
) -> Result<()> {
    match value {
        Value::String(s) => *s = expand_string_with_data(s, root, workspace, true, data)?,
        Value::Array(items) => {
            for item in items {
                expand_json_with_data(item, root, workspace, data)?;
            }
        }
        Value::Object(items) => {
            for item in items.values_mut() {
                expand_json_with_data(item, root, workspace, data)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn read_config_file(root: &Path, relative: &str) -> Result<Value> {
    ensure!(
        !Path::new(relative).is_absolute(),
        "Plugin LSP config path must be relative"
    );
    let path = root
        .join(relative)
        .canonicalize()
        .with_context(|| format!("Read plugin LSP config {relative}"))?;
    ensure!(
        path.starts_with(root),
        "Plugin LSP config escapes plugin root"
    );
    let file = std::fs::File::open(&path)?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(file, (MAX_MESSAGE + 1) as u64),
        &mut bytes,
    )?;
    ensure!(bytes.len() <= MAX_MESSAGE, "Plugin LSP config too large");
    Ok(serde_json::from_slice(&bytes)?)
}

fn matching_plugin_server(
    workspace: &Path,
    extension: &str,
    selected: Option<&str>,
    plugins: impl IntoIterator<Item = jcode_base::plugin::PluginRoot>,
) -> Result<PluginServer> {
    let mut matches = Vec::new();
    let mut errors = Vec::new();
    for plugin in plugins {
        let namespace = format!("{}/{}", plugin.marketplace, plugin.name);
        if selected.is_some_and(|name| !name.starts_with(&format!("{namespace}:"))) {
            continue;
        }
        match load_servers_with_plugin(&plugin.root, &namespace, workspace, Some(&plugin)) {
            Ok(servers) => matches.extend(servers.into_iter().filter(|server| {
                server.config.extension_to_language.contains_key(extension)
                    && selected.is_none_or(|name| name == server.name)
            })),
            Err(error) => errors.push(format!("{namespace}: {error:#}")),
        }
    }
    ensure!(
        !matches.is_empty(),
        "No enabled plugin language server matches {extension}. {}",
        errors.join("; ")
    );
    ensure!(
        matches.len() == 1,
        "Multiple plugin language servers match; set server to one of: {}",
        matches
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(matches.remove(0))
}

#[cfg(test)]
fn matching_server(
    workspace: &Path,
    extension: &str,
    selected: Option<&str>,
    plugins: impl IntoIterator<Item = (PathBuf, String)>,
) -> Result<PluginServer> {
    let mut matches = Vec::new();
    let mut errors = Vec::new();
    for (root, namespace) in plugins {
        if selected.is_some_and(|name| !name.starts_with(&format!("{namespace}:"))) {
            continue;
        }
        match load_servers(&root, &namespace, workspace) {
            Ok(servers) => matches.extend(servers.into_iter().filter(|server| {
                server.config.extension_to_language.contains_key(extension)
                    && selected.is_none_or(|name| name == server.name)
            })),
            Err(error) => errors.push(format!("{namespace}: {error:#}")),
        }
    }
    ensure!(
        !matches.is_empty(),
        "No enabled plugin language server matches {extension}. {}",
        errors.join("; ")
    );
    ensure!(
        matches.len() == 1,
        "Multiple plugin language servers match; set server to one of: {}",
        matches
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(matches.remove(0))
}

#[cfg(test)]
fn load_servers(root: &Path, namespace: &str, workspace: &Path) -> Result<Vec<PluginServer>> {
    load_servers_with_plugin(root, namespace, workspace, None)
}

fn load_servers_with_plugin(
    root: &Path,
    namespace: &str,
    workspace: &Path,
    plugin: Option<&jcode_base::plugin::PluginRoot>,
) -> Result<Vec<PluginServer>> {
    let manifest = read_config_file(root, ".claude-plugin/plugin.json")?;
    let mut sources = Vec::new();
    if root.join(".lsp.json").exists() {
        sources.push(read_config_file(root, ".lsp.json")?);
    }
    if let Some(declared) = manifest.get("lspServers") {
        match declared {
            Value::String(path) => sources.push(read_config_file(root, path)?),
            Value::Array(paths) => {
                for path in paths {
                    match path {
                        Value::String(path) => sources.push(read_config_file(root, path)?),
                        Value::Object(_) => sources.push(path.clone()),
                        _ => bail!("LSP config entry must be a path or server map"),
                    }
                }
            }
            Value::Object(_) => sources.push(declared.clone()),
            _ => bail!("lspServers must be an object, path or array of paths"),
        }
    }
    let mut configs = BTreeMap::new();
    for source in sources {
        for (key, value) in source
            .as_object()
            .context("Plugin LSP configuration must be an object")?
        {
            ensure!(!key.is_empty(), "Empty plugin LSP server name");
            configs.insert(key.clone(), value.clone());
        }
    }
    configs
        .into_iter()
        .map(|(key, value)| {
            let data = plugin.map(jcode_base::plugin::data_path).transpose()?;
            let config: ServerConfig = serde_json::from_value(value)?;
            let mut config = config;
            config.command =
                expand_string_with_data(&config.command, root, workspace, false, data.as_deref())?;
            for arg in &mut config.args {
                *arg = expand_string_with_data(arg, root, workspace, false, data.as_deref())?;
            }
            for (key, value) in &mut config.env {
                // The reserved value is host-owned, including when the manifest
                // tries to supply unresolved environment syntax in its place.
                if key == "CLAUDE_PLUGIN_DATA" && data.is_some() {
                    continue;
                }
                *value = expand_string_with_data(value, root, workspace, false, data.as_deref())?;
            }
            if let Some(value) = &mut config.initialization_options {
                expand_json_with_data(value, root, workspace, data.as_deref())?;
            }
            if let Some(value) = &mut config.settings {
                expand_json_with_data(value, root, workspace, data.as_deref())?;
            }
            if let Some(data) = data {
                config.env.insert(
                    "CLAUDE_PLUGIN_DATA".into(),
                    data.to_string_lossy().into_owned(),
                );
            }
            ensure!(!config.command.trim().is_empty(), "Empty LSP command");
            ensure!(
                config.transport.as_deref().is_none_or(|t| t == "stdio"),
                "Only stdio plugin LSP transport is supported"
            );
            ensure!(
                !config.extension_to_language.is_empty(),
                "LSP extensionToLanguage is required"
            );
            Ok(PluginServer {
                name: format!("{namespace}:{key}"),
                root: root.into(),
                config,
                plugin: plugin.cloned(),
            })
        })
        .collect()
}

struct Client {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    id: u64,
    diagnostics: BTreeMap<String, Value>,
    settings: Value,
}
impl Client {
    fn spawn(server: &PluginServer, workspace: &Path) -> Result<Self> {
        let mut command = Command::new(&server.config.command);
        // Relative executable paths are rooted at the plugin, bare commands use PATH.
        if Path::new(&server.config.command).components().count() > 1
            && !Path::new(&server.config.command).is_absolute()
        {
            command = Command::new(server.root.join(&server.config.command));
        }
        let data = server
            .plugin
            .as_ref()
            .map(jcode_base::plugin::prepare_data_dir)
            .transpose()?;
        let mut child = command
            .args(&server.config.args)
            .envs(&server.config.env)
            .env("CLAUDE_PLUGIN_ROOT", &server.root)
            .env("JCODE_PLUGIN_ROOT", &server.root)
            .envs(data.as_ref().map(|data| ("CLAUDE_PLUGIN_DATA", data)))
            .current_dir(workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("Start plugin LSP {}", server.name))?;
        let input = child.stdin.take().context("Missing LSP stdin")?;
        let output = BufReader::new(child.stdout.take().context("Missing LSP stdout")?);
        Ok(Self {
            child,
            input,
            output,
            id: 0,
            diagnostics: BTreeMap::new(),
            settings: server.config.settings.clone().unwrap_or(Value::Null),
        })
    }
    async fn send(&mut self, value: Value) -> Result<()> {
        let body = serde_json::to_vec(&value)?;
        ensure!(body.len() <= MAX_MESSAGE, "LSP outgoing message too large");
        timeout(REQUEST_TIMEOUT, async {
            self.input
                .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
                .await?;
            self.input.write_all(&body).await?;
            self.input.flush().await?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("LSP write timed out")?
    }
    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(json!({"jsonrpc":"2.0", "method":method, "params":params}))
            .await
    }
    async fn read(&mut self) -> Result<Value> {
        let mut length = None;
        let mut total = 0;
        loop {
            let mut line = String::new();
            let count = (&mut self.output).take(8193).read_line(&mut line).await?;
            ensure!(count > 0, "LSP server closed stdout");
            total += count;
            ensure!(total <= 8192, "LSP headers too large");
            if line == "\r\n" || line == "\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("Content-Length") {
                    length = Some(value.trim().parse::<usize>()?);
                }
            }
        }
        let length = length.context("LSP response missing Content-Length")?;
        ensure!(length <= MAX_MESSAGE, "LSP response too large");
        let mut body = vec![0; length];
        self.output.read_exact(&mut body).await?;
        Ok(serde_json::from_slice(&body)?)
    }
    async fn handle(&mut self, message: &Value) -> Result<()> {
        if message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
        {
            if let Some(uri) = message.pointer("/params/uri").and_then(Value::as_str) {
                self.diagnostics.insert(
                    uri.into(),
                    message
                        .pointer("/params/diagnostics")
                        .cloned()
                        .unwrap_or(json!([])),
                );
            }
        }
        if let (Some(id), Some(method)) = (
            message.get("id"),
            message.get("method").and_then(Value::as_str),
        ) {
            let response = match method {
                "workspace/configuration" => {
                    let items = message.pointer("/params/items").and_then(Value::as_array);
                    let result: Vec<Value> = items
                        .into_iter()
                        .flatten()
                        .map(|item| match item.get("section").and_then(Value::as_str) {
                            Some(section) => section
                                .split('.')
                                .try_fold(&self.settings, |v, key| v.get(key))
                                .cloned()
                                .unwrap_or(Value::Null),
                            None => self.settings.clone(),
                        })
                        .collect();
                    json!({"jsonrpc":"2.0", "id":id, "result":result})
                }
                "client/registerCapability"
                | "client/unregisterCapability"
                | "window/workDoneProgress/create" => {
                    json!({"jsonrpc":"2.0", "id":id, "result":null})
                }
                _ => {
                    json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32601,"message":"Client method not supported"}})
                }
            };
            self.send(response).await?;
        }
        Ok(())
    }
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.id += 1;
        let id = self.id;
        timeout(REQUEST_TIMEOUT, async {
            self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
                .await?;
            loop {
                let message = self.read().await?;
                if message.get("id").and_then(Value::as_u64) == Some(id)
                    && message.get("method").is_none()
                {
                    if let Some(error) = message.get("error") {
                        bail!("LSP {method}: {error}");
                    }
                    return Ok(message.get("result").cloned().unwrap_or(Value::Null));
                }
                self.handle(&message).await?;
            }
        })
        .await
        .with_context(|| format!("LSP {method} timed out"))?
    }
    async fn shutdown(&mut self) {
        let _ = timeout(
            Duration::from_secs(2),
            self.request("shutdown", Value::Null),
        )
        .await;
        let _ = timeout(Duration::from_secs(1), self.notify("exit", Value::Null)).await;
        if timeout(Duration::from_secs(1), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn plugin_lsp_data_is_pure_until_spawn_and_persists_across_cache_update() {
        let _guard = crate::storage::lock_test_env();
        struct Restore(Vec<(&'static str, Option<std::ffi::OsString>)>);
        impl Drop for Restore {
            fn drop(&mut self) {
                for (key, value) in &self.0 {
                    match value {
                        Some(value) => crate::env::set_var(key, value),
                        None => crate::env::remove_var(key),
                    }
                }
            }
        }
        let _restore = Restore(
            ["JCODE_HOME", "CLAUDE_PLUGIN_DATA"]
                .into_iter()
                .map(|key| (key, std::env::var_os(key)))
                .collect(),
        );
        let home = tempfile::tempdir().unwrap();
        let data_home = home.path().join("home-${JCODE_LSP_DATA_LITERAL}");
        crate::env::set_var("JCODE_HOME", &data_home);
        crate::env::set_var("CLAUDE_PLUGIN_DATA", "/spoofed-parent-data");
        let manifest = json!({"lspServers":{"peer":{
            "command":"python3", "args":["-u","-c",include_str!("lsp_fixture.py"),"${CLAUDE_PLUGIN_DATA}"],
            "env":{"CLAUDE_PLUGIN_DATA":"${JCODE_LSP_DATA_SPOOF_UNSET_956028}", "FIXTURE_DATA_REPORT":"1"},
            "extensionToLanguage":{".rs":"rust"},
            "initializationOptions":{"data":"${CLAUDE_PLUGIN_DATA}"},
            "settings":{"data":"${CLAUDE_PLUGIN_DATA}"}
        }}});
        let old = fixture_plugin(manifest.clone());
        let new = fixture_plugin(manifest);
        let mut expected = None;
        for (dir, previous) in [(&old, false), (&new, true)] {
            let plugin = jcode_base::plugin::PluginRoot {
                name: "demo".into(),
                marketplace: "market".into(),
                root: dir.path().canonicalize().unwrap(),
                version: Some(if previous { "v2" } else { "v1" }.into()),
                scope: jcode_base::plugin::PluginScope::User,
            };
            let data = jcode_base::plugin::data_path(&plugin).unwrap();
            if let Some(expected) = &expected {
                assert_eq!(&data, expected);
            } else {
                expected = Some(data.clone());
            }
            let server = matching_plugin_server(dir.path(), ".rs", None, [plugin]).unwrap();
            assert_eq!(
                server.config.env["CLAUDE_PLUGIN_DATA"],
                data.to_string_lossy()
            );
            assert_eq!(
                server.config.settings.as_ref().unwrap()["data"],
                data.to_string_lossy().as_ref()
            );
            if !previous {
                assert!(!data_home.join("plugins/data").exists());
            }
            let mut client = Client::spawn(&server, dir.path()).unwrap();
            let result = client
                .request(
                    "initialize",
                    json!({"initializationOptions":server.config.initialization_options}),
                )
                .await
                .unwrap();
            assert_eq!(
                result["fixtureData"]["path"],
                data.to_string_lossy().as_ref()
            );
            assert_eq!(
                result["fixtureData"]["arg"],
                data.to_string_lossy().as_ref()
            );
            assert_eq!(result["fixtureData"]["exists"], true);
            assert_eq!(result["fixtureData"]["previousMarker"], previous);
            assert_eq!(
                result["fixtureData"]["params"]["initializationOptions"]["data"],
                data.to_string_lossy().as_ref()
            );
            client.shutdown().await;
            assert_eq!(
                std::env::var("CLAUDE_PLUGIN_DATA").unwrap(),
                "/spoofed-parent-data"
            );
        }
    }

    fn fixture_plugin(manifest: Value) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".claude-plugin")).unwrap();
        std::fs::write(
            dir.path().join(".claude-plugin/plugin.json"),
            manifest.to_string(),
        )
        .unwrap();
        dir
    }

    #[test]
    fn default_and_manifest_configs_merge_with_expansion_and_namespacing() {
        let dir = fixture_plugin(
            json!({"lspServers":{"rust":{"command":"${CLAUDE_PLUGIN_ROOT}/bin/rust","extensionToLanguage":{".rs":"rust"},"initializationOptions":{"workspace":"${workspaceFolder}"}}}}),
        );
        std::fs::write(dir.path().join(".lsp.json"), json!({"python":{"command":"pylsp","extensionToLanguage":{".py":"python"}},"rust":{"command":"old","extensionToLanguage":{".rs":"rust"}}}).to_string()).unwrap();
        let root = dir.path().canonicalize().unwrap();
        let servers = load_servers(&root, "market/plugin", Path::new("/workspace")).unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[1].name, "market/plugin:rust");
        assert_eq!(
            servers[1].config.command,
            format!("{}/bin/rust", root.display())
        );
        assert_eq!(
            servers[1].config.initialization_options.as_ref().unwrap()["workspace"],
            "/workspace"
        );
    }

    #[test]
    fn custom_config_paths_and_invalid_shapes() {
        let dir = fixture_plugin(json!({"lspServers":["servers.json"]}));
        std::fs::write(
            dir.path().join("servers.json"),
            json!({"test":{"command":"test-lsp","extensionToLanguage":{".test":"test"}}})
                .to_string(),
        )
        .unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert_eq!(
            load_servers(&root, "one/plugin", &root).unwrap()[0].name,
            "one/plugin:test"
        );
        std::fs::write(dir.path().join("servers.json"), json!({"test":{"command":"test-lsp","transport":"socket","extensionToLanguage":{".test":"test"}}}).to_string()).unwrap();
        assert!(load_servers(&root, "one/plugin", &root).is_err());
        std::fs::write(dir.path().join("servers.json"), "[]").unwrap();
        assert!(load_servers(&root, "one/plugin", &root).is_err());
    }

    #[test]
    fn manifest_array_accepts_mixed_paths_and_inline_overrides() {
        let dir = fixture_plugin(
            json!({"lspServers":["servers.json", {"test":{"command":"override","extensionToLanguage":{".test":"test"}}}]}),
        );
        std::fs::write(
            dir.path().join("servers.json"),
            json!({"test":{"command":"original","extensionToLanguage":{".test":"test"}}})
                .to_string(),
        )
        .unwrap();
        let root = dir.path().canonicalize().unwrap();
        let servers = load_servers(&root, "market/plugin", &root).unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].config.command, "override");
    }

    #[test]
    fn config_paths_cannot_escape_root() {
        let dir = fixture_plugin(json!({}));
        let root = dir.path().canonicalize().unwrap();
        assert!(read_config_file(&root, "/etc/passwd").is_err());
        assert!(read_config_file(&root, "../outside.json").is_err());
        #[cfg(unix)]
        {
            let outside = tempfile::NamedTempFile::new().unwrap();
            std::os::unix::fs::symlink(outside.path(), root.join("escape.json")).unwrap();
            assert!(read_config_file(&root, "escape.json").is_err());
        }
    }

    #[test]
    fn expansion_is_literal_and_missing_variables_fail_closed() {
        assert_eq!(
            expand(
                "${CLAUDE_PLUGIN_ROOT}/x; echo nope",
                Path::new("/plugin space"),
                Path::new("/work")
            )
            .unwrap(),
            "/plugin space/x; echo nope"
        );
        assert!(
            expand(
                "${JCODE_LSP_TEST_VARIABLE_THAT_DOES_NOT_EXIST_7029}",
                Path::new("/plugin"),
                Path::new("/work")
            )
            .is_err()
        );
        assert!(expand("${broken", Path::new("/plugin"), Path::new("/work")).is_err());
        let mut data = json!({"known":"${CLAUDE_PLUGIN_ROOT}", "unknown":"${JCODE_LSP_TEST_VARIABLE_THAT_DOES_NOT_EXIST_7029}"});
        expand_json(&mut data, Path::new("/plugin"), Path::new("/work")).unwrap();
        assert_eq!(data["known"], "/plugin");
        assert_eq!(
            data["unknown"],
            "${JCODE_LSP_TEST_VARIABLE_THAT_DOES_NOT_EXIST_7029}"
        );
    }

    #[test]
    fn broken_plugin_does_not_block_good_and_namespace_collisions_are_explicit() {
        let bad = fixture_plugin(json!({"lspServers":true}));
        let good = fixture_plugin(
            json!({"lspServers":{"rust":{"command":"rust-analyzer","extensionToLanguage":{".rs":"rust"}}}}),
        );
        let roots = || {
            vec![
                (bad.path().canonicalize().unwrap(), "market/broken".into()),
                (good.path().canonicalize().unwrap(), "market/good".into()),
            ]
        };
        let workspace = good.path().canonicalize().unwrap();
        assert_eq!(
            matching_server(&workspace, ".rs", None, roots())
                .unwrap()
                .name,
            "market/good:rust"
        );
        assert_eq!(
            matching_server(&workspace, ".rs", Some("market/good:rust"), roots())
                .unwrap()
                .name,
            "market/good:rust"
        );
        assert!(
            matching_server(&workspace, ".py", None, roots())
                .unwrap_err()
                .to_string()
                .contains("market/broken")
        );
        let duplicate = vec![
            (workspace.clone(), "market/good".into()),
            (workspace.clone(), "other/good".into()),
        ];
        assert!(
            matching_server(&workspace, ".rs", None, duplicate)
                .unwrap_err()
                .to_string()
                .contains("Multiple")
        );
        assert!(matching_server(&workspace, ".rs", None, Vec::new()).is_err());
    }

    #[tokio::test]
    async fn malformed_or_oversized_frames_fail_without_unbounded_allocation() {
        let dir = tempfile::tempdir().unwrap();
        for wire in [
            "Content-Length: 9000000\r\n\r\n",
            "Content-Type: application/json\r\n\r\n",
            "Content-Length: nope\r\n\r\n",
        ] {
            let code = format!("import sys; sys.stdout.write({wire:?}); sys.stdout.flush()");
            let config: ServerConfig = serde_json::from_value(json!({"command":"python3","args":["-u","-c",code],"extensionToLanguage":{".rs":"rust"}})).unwrap();
            let server = PluginServer {
                name: "fixture/bad:test".into(),
                root: dir.path().into(),
                config,
                plugin: None,
            };
            let mut client = Client::spawn(&server, dir.path()).unwrap();
            assert!(client.request("initialize", json!({})).await.is_err());
            client.shutdown().await;
        }
    }

    #[tokio::test]
    async fn stdio_initialize_queries_notifications_error_and_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let config: ServerConfig = serde_json::from_value(json!({"command":"python3","args":["-u","-c",include_str!("lsp_fixture.py")],"extensionToLanguage":{".rs":"rust"}})).unwrap();
        let server = PluginServer {
            name: "fixture/plugin:test".into(),
            root: dir.path().into(),
            config,
            plugin: None,
        };
        let mut client = Client::spawn(&server, dir.path()).unwrap();
        let result = client.request("initialize", json!({})).await.unwrap();
        assert_eq!(result["capabilities"]["hoverProvider"], true);
        client.notify("initialized", json!({})).await.unwrap();
        let uri = "file:///fixture.rs";
        client.notify("textDocument/didOpen", json!({"textDocument":{"uri":uri,"languageId":"rust","version":1,"text":"fn main() {}"}})).await.unwrap();
        for method in [
            "textDocument/hover",
            "textDocument/definition",
            "textDocument/references",
            "textDocument/documentSymbol",
        ] {
            let result = client
                .request(
                    method,
                    json!({"textDocument":{"uri":uri},"position":{"line":0,"character":2}}),
                )
                .await
                .unwrap();
            assert_eq!(result["method"], method);
            assert_eq!(result["text"], "café λ");
        }
        assert_eq!(client.diagnostics[uri][0]["message"], "fixture diagnostic");
        assert!(
            client
                .request("fixture/error", json!({}))
                .await
                .unwrap_err()
                .to_string()
                .contains("fixture error")
        );
        client.shutdown().await;
        assert!(client.child.try_wait().unwrap().unwrap().success());
    }

    #[tokio::test]
    async fn full_query_operations_use_current_file_and_one_based_positions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let file = root.join("fixture.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        let config: ServerConfig = serde_json::from_value(json!({"command":"python3","args":["-u","-c",include_str!("lsp_fixture.py")],"extensionToLanguage":{".rs":"rust"}})).unwrap();
        let server = PluginServer {
            name: "fixture/plugin:test".into(),
            root: root.clone(),
            config,
            plugin: None,
        };
        for operation in [
            "hover",
            "definition",
            "references",
            "documentSymbols",
            "diagnostics",
        ] {
            let input: Input = serde_json::from_value(
                json!({"operation":operation,"file_path":"fixture.rs","line":2,"character":3}),
            )
            .unwrap();
            let output = run_server(&server, &root, &file, ".rs", &input)
                .await
                .unwrap();
            let output: Value = serde_json::from_str(&output.output).unwrap();
            assert_eq!(output["operation"], operation);
            if operation == "diagnostics" {
                assert_eq!(
                    output["result"]["items"][0]["message"],
                    "fixture diagnostic"
                );
            } else if operation == "documentSymbols" {
                assert_eq!(output["result"]["method"], "textDocument/documentSymbol");
            } else {
                assert_eq!(
                    output["result"]["params"]["position"],
                    json!({"line":1,"character":2})
                );
                if operation == "references" {
                    assert_eq!(
                        output["result"]["params"]["context"]["includeDeclaration"],
                        true
                    );
                }
            }
        }
        let mut pull_server = server;
        pull_server
            .config
            .env
            .insert("FIXTURE_PULL_DIAGNOSTICS".into(), "1".into());
        let input: Input =
            serde_json::from_value(json!({"operation":"diagnostics","file_path":"fixture.rs"}))
                .unwrap();
        let output = run_server(&pull_server, &root, &file, ".rs", &input)
            .await
            .unwrap();
        let output: Value = serde_json::from_str(&output.output).unwrap();
        assert_eq!(output["result"]["method"], "textDocument/diagnostic");
    }
}

#[derive(Deserialize)]
struct Input {
    operation: String,
    file_path: String,
    #[serde(default)]
    line: Option<u64>,
    #[serde(default)]
    character: Option<u64>,
    #[serde(default)]
    server: Option<String>,
}

#[async_trait]
impl Tool for LspTool {
    fn name(&self) -> &str {
        "lsp"
    }
    fn description(&self) -> &str {
        "Query a language server supplied by an enabled plugin. Supports hover, definition, references, documentSymbols and diagnostics. Positions are 1-based lines and UTF-16 characters. Starts a fresh isolated stdio server per call."
    }
    fn parameters_schema(&self) -> Value {
        json!({"type":"object","properties":{
            "intent":super::intent_schema_property(),
            "operation":{"type":"string","enum":["hover","definition","references","documentSymbols","diagnostics"]},
            "file_path":{"type":"string"},"line":{"type":"integer","minimum":1},"character":{"type":"integer","minimum":1},
            "server":{"type":"string","description":"Exact marketplace/plugin:server name, required if multiple servers match."}
        },"required":["operation","file_path"]})
    }
    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let input: Input = serde_json::from_value(input)?;
        ensure!(
            [
                "hover",
                "definition",
                "references",
                "documentSymbols",
                "diagnostics"
            ]
            .contains(&input.operation.as_str()),
            "Unsupported LSP operation"
        );
        let workspace = ctx.resolve_path(Path::new(".")).canonicalize()?;
        let file = ctx
            .resolve_path(Path::new(&input.file_path))
            .canonicalize()?;
        ensure!(
            file.starts_with(&workspace),
            "LSP file must be inside the current workspace"
        );
        let extension = format!(
            ".{}",
            file.extension()
                .and_then(|s| s.to_str())
                .context("File has no extension")?
        );
        let plugins = jcode_base::plugin::enabled_plugins_for_dir(&workspace);
        let server =
            matching_plugin_server(&workspace, &extension, input.server.as_deref(), plugins)?;
        run_server(&server, &workspace, &file, &extension, &input).await
    }
}

async fn run_server(
    server: &PluginServer,
    workspace: &Path,
    file: &Path,
    extension: &str,
    input: &Input,
) -> Result<ToolOutput> {
    let mut content = String::new();
    tokio::fs::File::open(file)
        .await?
        .take((MAX_MESSAGE / 2 + 1) as u64)
        .read_to_string(&mut content)
        .await?;
    ensure!(content.len() <= MAX_MESSAGE / 2, "LSP file too large");
    let uri = url::Url::from_file_path(&file)
        .map_err(|_| anyhow::anyhow!("Invalid file URI"))?
        .to_string();
    let root_uri = url::Url::from_directory_path(&workspace)
        .map_err(|_| anyhow::anyhow!("Invalid workspace URI"))?
        .to_string();
    let mut client = Client::spawn(&server, &workspace)?;
    let result: Result<Value> = async {
            let initialized = client.request("initialize", json!({"processId":std::process::id(),"rootUri":root_uri,
                "capabilities":{"workspace":{"configuration":true},"textDocument":{"publishDiagnostics":{},"diagnostic":{},"hover":{"contentFormat":["plaintext","markdown"]}}},
                "initializationOptions":server.config.initialization_options,
                "workspaceFolders":[{"uri":root_uri,"name":workspace.file_name().and_then(|s| s.to_str()).unwrap_or("workspace")}]
            })).await?;
            client.notify("initialized", json!({})).await?;
            if let Some(settings) = &server.config.settings { client.notify("workspace/didChangeConfiguration", json!({"settings":settings})).await?; }
            client.notify("textDocument/didOpen", json!({"textDocument":{"uri":uri,"languageId":server.config.extension_to_language[extension],"version":1,"text":content}})).await?;
            let document = json!({"uri":uri});
            let position = json!({"line":input.line.unwrap_or(1).checked_sub(1).context("line must be >= 1")?,"character":input.character.unwrap_or(1).checked_sub(1).context("character must be >= 1")?});
            let result = match input.operation.as_str() {
                "documentSymbols" => client.request("textDocument/documentSymbol", json!({"textDocument":document})).await?,
                "diagnostics" => {
                    if initialized.pointer("/capabilities/diagnosticProvider").is_some_and(|v| !v.is_null() && v != &Value::Bool(false)) {
                        client.request("textDocument/diagnostic", json!({"textDocument":document})).await?
                    } else {
                        let published = timeout(Duration::from_secs(5), async {
                            loop {
                                if let Some(diagnostics) = client.diagnostics.get(&uri) { return Ok::<_,anyhow::Error>(diagnostics.clone()); }
                                let message = client.read().await?;
                                client.handle(&message).await?;
                            }
                        }).await.context("No diagnostics published within 5 seconds; this does not mean the file is error-free")??;
                        json!({"kind":"full","items":published})
                    }
                }
                operation => {
                    let mut params = json!({"textDocument":document,"position":position});
                    if operation == "references" { params["context"] = json!({"includeDeclaration":true}); }
                    client.request(&format!("textDocument/{operation}"), params).await?
                }
            };
            Ok(json!({"server":server.name,"file":uri,"operation":input.operation,"result":result}))
        }.await;
    client.shutdown().await;
    Ok(ToolOutput::new(serde_json::to_string_pretty(&result?)?))
}
