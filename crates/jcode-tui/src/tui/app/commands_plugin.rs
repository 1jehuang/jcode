use super::{App, DisplayMessage};
use crate::bus::{Bus, BusEvent, PluginOperationCompleted};
use jcode_provider_extensions::{PluginBundle, PluginSource, PluginStore, ProviderRegistry};
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
enum PluginAction {
    Help,
    Inspect(String),
    Add { source: String, trusted: bool },
    Update { name: String, trusted: bool },
    List,
    Doctor,
    Trust(String),
    Remove(String),
}

pub(super) fn handle_plugin_command(app: &mut App, trimmed: &str) -> bool {
    let Some(parsed) = parse_plugin_command(trimmed) else {
        return false;
    };
    let action = match parsed {
        Ok(action) => action,
        Err(message) => {
            app.push_display_message(DisplayMessage::system(message));
            return true;
        }
    };
    if matches!(action, PluginAction::Help) {
        app.push_display_message(DisplayMessage::system(plugin_usage()));
        return true;
    }

    let session_id = app.session.id.clone();
    app.set_status_notice("Plugin operation running...");
    app.push_display_message(DisplayMessage::system(
        "Plugin operation started off the UI thread. Installation validates before registration and never executes plugin code.",
    ));
    std::thread::spawn(move || {
        let result = run_plugin_action(action);
        let (output, success) = match result {
            Ok(output) => (output, true),
            Err(error) => (format!("Plugin operation failed: {error}"), false),
        };
        Bus::global().publish(BusEvent::PluginOperationCompleted(
            PluginOperationCompleted {
                session_id,
                output,
                success,
            },
        ));
    });
    true
}

fn parse_plugin_command(trimmed: &str) -> Option<Result<PluginAction, String>> {
    let rest = if trimmed == "/plugin" {
        ""
    } else {
        trimmed.strip_prefix("/plugin ")?
    };
    let mut words = rest.split_whitespace();
    let command = words.next().unwrap_or("help");
    let action = match command {
        "help" | "--help" | "-h" => PluginAction::Help,
        "inspect" => match words.next() {
            Some(source) if words.next().is_none() => PluginAction::Inspect(source.to_string()),
            _ => {
                return Some(Err(
                    "Usage: /plugin inspect <owner/repository[@ref]|path>".to_string()
                ));
            }
        },
        "add" => {
            let Some(source) = words.next() else {
                return Some(Err(
                    "Usage: /plugin add <owner/repository[@ref]> [--trusted]".to_string(),
                ));
            };
            let mut trusted = false;
            for flag in words {
                if flag == "--trusted" {
                    trusted = true;
                } else {
                    return Some(Err(
                        "Usage: /plugin add <owner/repository[@ref]> [--trusted]".to_string(),
                    ));
                }
            }
            PluginAction::Add {
                source: source.to_string(),
                trusted,
            }
        }
        "update" => {
            let Some(name) = words.next() else {
                return Some(Err(
                    "Usage: /plugin update <plugin-name> [--trusted]".to_string()
                ));
            };
            let mut trusted = false;
            for flag in words {
                if flag == "--trusted" {
                    trusted = true;
                } else {
                    return Some(Err(
                        "Usage: /plugin update <plugin-name> [--trusted]".to_string()
                    ));
                }
            }
            PluginAction::Update {
                name: name.to_string(),
                trusted,
            }
        }
        "list" if words.next().is_none() => PluginAction::List,
        "doctor" if words.next().is_none() => PluginAction::Doctor,
        "trust" => match words.next() {
            Some(id) if words.next().is_none() => PluginAction::Trust(id.to_string()),
            _ => return Some(Err("Usage: /plugin trust <provider-id>".to_string())),
        },
        "remove" => match words.next() {
            Some(name) if words.next().is_none() => PluginAction::Remove(name.to_string()),
            _ => return Some(Err("Usage: /plugin remove <plugin-name>".to_string())),
        },
        _ => return Some(Err(plugin_usage())),
    };
    Some(Ok(action))
}

fn run_plugin_action(action: PluginAction) -> anyhow::Result<String> {
    let store = PluginStore::open(PluginStore::default_root()?);
    match action {
        PluginAction::Inspect(source) => {
            let bundle = if Path::new(&source).is_dir() {
                PluginBundle::load(source)?
            } else {
                store.inspect(&PluginSource::parse(&source)?)?
            };
            Ok(format_bundle(&bundle))
        }
        PluginAction::Add { source, trusted } => {
            if Path::new(&source).exists() {
                anyhow::bail!(
                    "/plugin add accepts a GitHub source. Use `jcode provider extension add` for a local bundle"
                )
            }
            let parsed_source = PluginSource::parse(&source)?;
            let installed = store.install(&parsed_source)?;
            let provider_registered = match sync_plugin_provider(&store, &installed, trusted) {
                Ok(registered) => registered,
                Err(error) => return Err(rollback_after_error(&store, &installed, error)),
            };
            Ok(format!(
                "Installed plugin '{}' v{} at {}\nPinned commit: {}\nRuntime tier: {}\nTrust: {}{}",
                installed.metadata.name,
                installed.metadata.version,
                installed.metadata.path.display(),
                installed.metadata.commit,
                if provider_registered {
                    "external subprocess"
                } else {
                    "metadata-only"
                },
                if trusted { "trusted" } else { "untrusted" },
                if installed.bundle.unsupported_components().is_empty() {
                    String::new()
                } else {
                    format!(
                        "\nUnsupported surfaces: {}",
                        installed.bundle.unsupported_components().join(", ")
                    )
                },
            ))
        }
        PluginAction::Update { name, trusted } => {
            let _previous = store
                .list()?
                .into_iter()
                .find(|plugin| plugin.name == name)
                .ok_or_else(|| anyhow::anyhow!("plugin '{name}' is not installed"))?;
            let installed = store.update(&name)?;
            let provider_registered = match sync_plugin_provider(&store, &installed, trusted) {
                Ok(registered) => registered,
                Err(error) => return Err(rollback_after_error(&store, &installed, error)),
            };
            Ok(format!(
                "Updated plugin '{}' to v{} at {}\nPinned commit: {}\nRuntime tier: {}",
                installed.metadata.name,
                installed.metadata.version,
                installed.metadata.path.display(),
                installed.metadata.commit,
                if provider_registered {
                    "external subprocess"
                } else {
                    "metadata-only"
                },
            ))
        }
        PluginAction::List => {
            let plugins = store.list()?;
            if plugins.is_empty() {
                return Ok("No GitHub plugins installed.".to_string());
            }
            Ok(plugins
                .into_iter()
                .map(|plugin| {
                    format!(
                        "{} v{} @ {} ({})",
                        plugin.name,
                        plugin.version,
                        &plugin.commit[..12.min(plugin.commit.len())],
                        plugin.repository
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        PluginAction::Doctor => {
            let plugins = store.list()?;
            if plugins.is_empty() {
                return Ok("No GitHub plugins installed.".to_string());
            }
            let mut output = Vec::new();
            for plugin in plugins {
                match PluginBundle::load(&plugin.path) {
                    Ok(bundle) => output.push(format!(
                        "{} v{}: pass ({}{})",
                        bundle.manifest.name,
                        bundle.manifest.version,
                        if bundle.provider_manifest.is_some() {
                            "external provider"
                        } else {
                            "metadata-only"
                        },
                        if bundle.unsupported_components().is_empty() {
                            String::new()
                        } else {
                            format!(
                                ", unsupported: {}",
                                bundle.unsupported_components().join(", ")
                            )
                        }
                    )),
                    Err(error) => output.push(format!("{}: fail ({error})", plugin.name)),
                }
            }
            Ok(output.join("\n"))
        }
        PluginAction::Trust(id) => {
            let mut registry = ProviderRegistry::open(ProviderRegistry::default_path()?)?;
            registry.set_trusted(&id, true)?;
            registry.save()?;
            Ok(format!("Plugin provider '{id}' is now trusted."))
        }
        PluginAction::Remove(name) => {
            let provider_id = store
                .list()?
                .into_iter()
                .find(|plugin| plugin.name == name)
                .and_then(|plugin| plugin.provider_id);
            let mut registry = ProviderRegistry::open(ProviderRegistry::default_path()?)?;
            let owned = if let Some(provider_id) = provider_id.as_deref() {
                let owned = registry
                    .get(provider_id)
                    .and_then(|record| record.source.as_deref())
                    .is_some_and(|source| source.starts_with(&store.root().join(&name)));
                if registry.get(provider_id).is_some() && !owned {
                    anyhow::bail!(
                        "provider '{}' is not owned by plugin '{}'; refusing to remove it",
                        provider_id,
                        name
                    );
                }
                owned
            } else {
                false
            };
            store.remove(&name)?;
            if let Some(provider_id) = provider_id {
                if owned {
                    registry.remove(&provider_id)?;
                    registry.save()?;
                }
            }
            Ok(format!("Removed plugin '{name}'."))
        }
        PluginAction::Help => Ok(plugin_usage()),
    }
}

fn format_bundle(bundle: &PluginBundle) -> String {
    let mut output = format!(
        "Plugin '{}' v{}\nProvider tier: {}\n",
        bundle.manifest.name,
        bundle.manifest.version,
        if bundle.provider_manifest.is_some() {
            "external subprocess"
        } else {
            "metadata-only"
        },
    );
    if bundle.skills.is_empty() {
        output.push_str("Skills: none\n");
    } else {
        output.push_str(&format!(
            "Skills: {}\n",
            bundle
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let unsupported = bundle.unsupported_components();
    if !unsupported.is_empty() {
        output.push_str(&format!(
            "Unsupported surfaces: {}\n",
            unsupported.join(", ")
        ));
    }
    output
}

fn plugin_usage() -> String {
    "Usage: /plugin <inspect|add|update|list|doctor|trust|remove>\n\n\
/plugin inspect <owner/repository[@ref]|path>\n\
/plugin add <owner/repository[@ref]> [--trusted]\n\
/plugin update <plugin-name> [--trusted]\n\
/plugin list\n\
/plugin doctor\n\
/plugin trust <provider-id>\n\
/plugin remove <plugin-name>\n\n\
Remote installs are HTTPS GitHub repositories, pinned to the resolved commit, validated in quarantine, and never executed during installation. Remote plugins use the external subprocess tier. Embedded Rust extensions remain compile-time integrations."
        .to_string()
}

fn ensure_plugin_provider_owner(
    registry: &ProviderRegistry,
    provider_id: &str,
    store: &PluginStore,
    plugin_name: &str,
) -> anyhow::Result<()> {
    let Some(existing) = registry.get(provider_id) else {
        return Ok(());
    };
    let plugin_root = store.root().join(plugin_name);
    if existing
        .source
        .as_deref()
        .is_some_and(|source| source.starts_with(&plugin_root))
    {
        return Ok(());
    }
    anyhow::bail!(
        "provider '{}' is already registered outside plugin '{}'; refusing to replace it",
        provider_id,
        plugin_name
    )
}

fn sync_plugin_provider(
    store: &PluginStore,
    installed: &jcode_provider_extensions::InstalledPlugin,
    trusted: bool,
) -> anyhow::Result<bool> {
    let provider = installed.provider_manifest()?;
    let Some(provider) = provider else {
        if let Some(previous_id) = installed.previous_provider_id() {
            let mut registry = ProviderRegistry::open(ProviderRegistry::default_path()?)?;
            if remove_owned_plugin_provider(
                &mut registry,
                previous_id,
                store,
                &installed.metadata.name,
            )? {
                registry.save()?;
            }
        }
        return Ok(false);
    };
    let mut registry = ProviderRegistry::open(ProviderRegistry::default_path()?)?;
    ensure_plugin_provider_owner(&registry, &provider.id, store, &installed.metadata.name)?;
    let new_provider_id = provider.id.clone();
    registry.register_or_update_plugin(
        provider,
        installed.bundle.root.clone(),
        &store.root().join(&installed.metadata.name),
        trusted,
    )?;
    if installed.previous_provider_id() != Some(new_provider_id.as_str()) {
        if let Some(previous_id) = installed.previous_provider_id() {
            remove_owned_plugin_provider(
                &mut registry,
                previous_id,
                store,
                &installed.metadata.name,
            )?;
        }
    }
    registry.save()?;
    Ok(true)
}

fn rollback_after_error(
    store: &PluginStore,
    installed: &jcode_provider_extensions::InstalledPlugin,
    error: anyhow::Error,
) -> anyhow::Error {
    match store.rollback(installed) {
        Ok(()) => error,
        Err(rollback) => anyhow::anyhow!("{error}; {rollback}"),
    }
}

fn remove_owned_plugin_provider(
    registry: &mut ProviderRegistry,
    provider_id: &str,
    store: &PluginStore,
    plugin_name: &str,
) -> anyhow::Result<bool> {
    let plugin_root = store.root().join(plugin_name);
    let owned = registry
        .get(provider_id)
        .and_then(|record| record.source.as_deref())
        .is_some_and(|source| source.starts_with(&plugin_root));
    if owned {
        registry.remove(provider_id)?;
    }
    Ok(owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plugin_commands_and_rejects_extra_arguments() {
        assert_eq!(
            parse_plugin_command("/plugin list"),
            Some(Ok(PluginAction::List))
        );
        assert_eq!(
            parse_plugin_command("/plugin add owner/repo@v1 --trusted"),
            Some(Ok(PluginAction::Add {
                source: "owner/repo@v1".to_string(),
                trusted: true,
            }))
        );
        assert_eq!(
            parse_plugin_command("/plugin update hello-plugin --trusted"),
            Some(Ok(PluginAction::Update {
                name: "hello-plugin".to_string(),
                trusted: true,
            }))
        );
        assert!(parse_plugin_command("/plugin list extra").is_some());
    }
}
