//! Offline plugin CLI dispatch. No provider credentials or daemon required.
use anyhow::Result;
use serde_json::{Value, json};

use super::args::plugin::{MarketplaceCommand, PluginArgs, PluginCommand, PluginScopeArg};
use crate::plugin::{PluginManager, PluginScope};

impl From<PluginScopeArg> for PluginScope {
    fn from(scope: PluginScopeArg) -> Self {
        match scope {
            PluginScopeArg::User => Self::User,
            PluginScopeArg::Project => Self::Project,
            PluginScopeArg::Local => Self::Local,
        }
    }
}

pub(crate) fn run(args: PluginArgs) -> Result<()> {
    let result = (|| {
        // Validation works even outside a configured project.
        if let PluginCommand::Validate { path } = &args.action {
            return Ok(serde_json::to_value(PluginManager::validate(path)?)?);
        }
        let manager = PluginManager::for_dir(&std::env::current_dir()?)?;
        execute(&manager, &args.action, args.scope.into())
    })();
    match result {
        Ok(value) => {
            println!("{}", render(&value, args.json)?);
            Ok(())
        }
        Err(error) => {
            if args.json {
                println!(
                    "{}",
                    serde_json::to_string(&json!({"error": format!("{error:#}")}))?
                );
            }
            Err(error)
        }
    }
}

fn execute(manager: &PluginManager, action: &PluginCommand, scope: PluginScope) -> Result<Value> {
    Ok(match action {
        PluginCommand::List => serde_json::to_value(manager.list()?)?,
        PluginCommand::Install { plugin } => serde_json::to_value(manager.install(plugin, scope)?)?,
        PluginCommand::Uninstall { plugin } => {
            manager.uninstall(plugin, scope)?;
            json!({"ok": true, "action": "uninstall", "plugin": plugin, "scope": scope})
        }
        PluginCommand::Enable { plugin } => {
            manager.enable(plugin, scope)?;
            json!({"ok": true, "action": "enable", "plugin": plugin, "scope": scope})
        }
        PluginCommand::Disable { plugin } => {
            manager.disable(plugin, scope)?;
            json!({"ok": true, "action": "disable", "plugin": plugin, "scope": scope})
        }
        PluginCommand::Update { plugin } => serde_json::to_value(manager.update(plugin, scope)?)?,
        PluginCommand::Validate { path } => serde_json::to_value(PluginManager::validate(path)?)?,
        PluginCommand::Marketplace { action } => match action {
            MarketplaceCommand::Add { source } => {
                serde_json::to_value(manager.add_marketplace(source)?)?
            }
            MarketplaceCommand::List => serde_json::to_value(manager.list_marketplaces()?)?,
            MarketplaceCommand::Remove { name } => {
                manager.remove_marketplace(name)?;
                json!({"ok": true, "action": "remove", "marketplace": name})
            }
            MarketplaceCommand::Update { name } => {
                serde_json::to_value(manager.update_marketplace(name)?)?
            }
        },
    })
}

fn render(value: &Value, json_output: bool) -> Result<String> {
    if json_output {
        return Ok(serde_json::to_string_pretty(value)?);
    }
    match value {
        Value::Array(rows) if rows.is_empty() => Ok("No entries found.".into()),
        Value::Array(rows) => rows
            .iter()
            .map(|row| render(row, false))
            .collect::<Result<Vec<_>>>()
            .map(|rows| rows.join("\n\n")),
        Value::Object(fields) => Ok(fields
            .iter()
            .map(|(key, value)| {
                format!(
                    "{key}: {}",
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string())
                )
            })
            .collect::<Vec<_>>()
            .join("\n")),
        _ => Ok(value.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_mapping_is_exact() {
        for (arg, expected) in [
            (PluginScopeArg::User, "user"),
            (PluginScopeArg::Project, "project"),
            (PluginScopeArg::Local, "local"),
        ] {
            assert_eq!(PluginScope::from(arg).to_string(), expected);
        }
    }

    #[test]
    fn list_dispatch_is_offline_and_json_is_structured() {
        let root = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let manager = PluginManager::new(root.path().to_owned(), project.path().to_owned());
        for action in [
            PluginCommand::List,
            PluginCommand::Marketplace {
                action: MarketplaceCommand::List,
            },
        ] {
            let value = execute(&manager, &action, PluginScope::User).unwrap();
            assert!(value.is_array());
            assert_eq!(
                serde_json::from_str::<Value>(&render(&value, true).unwrap()).unwrap(),
                value
            );
            assert_eq!(render(&value, false).unwrap(), "No entries found.");
        }
    }

    #[test]
    fn validation_dispatch_rejects_missing_plugin() {
        let root = tempfile::tempdir().unwrap();
        let manager = PluginManager::new(root.path().join("user"), root.path().join("project"));
        assert!(
            execute(
                &manager,
                &PluginCommand::Validate {
                    path: root.path().join("missing")
                },
                PluginScope::User
            )
            .is_err()
        );
    }

    #[test]
    fn dispatch_marketplace_and_scoped_plugin_lifecycle() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("catalog");
        std::fs::create_dir_all(source.join(".claude-plugin")).unwrap();
        std::fs::create_dir_all(source.join("demo/.claude-plugin")).unwrap();
        std::fs::write(
            source.join(".claude-plugin/marketplace.json"),
            r#"{"name":"fixture","plugins":[{"name":"demo","source":"./demo"}]}"#,
        )
        .unwrap();
        std::fs::write(
            source.join("demo/.claude-plugin/plugin.json"),
            r#"{"name":"demo","version":"1.0.0"}"#,
        )
        .unwrap();
        let manager = PluginManager::new(root.path().join("user"), root.path().join("project"));
        let marketplace = |action| PluginCommand::Marketplace { action };
        let added = execute(
            &manager,
            &marketplace(MarketplaceCommand::Add {
                source: source.to_str().unwrap().to_owned(),
            }),
            PluginScope::User,
        )
        .unwrap();
        assert_eq!(added["name"], "fixture");
        let valid = execute(
            &manager,
            &PluginCommand::Validate {
                path: source.join("demo"),
            },
            PluginScope::User,
        )
        .unwrap();
        assert_eq!(valid["name"], "demo");
        for scope in [PluginScope::User, PluginScope::Project, PluginScope::Local] {
            let plugin = "demo@fixture".to_owned();
            let installed = execute(
                &manager,
                &PluginCommand::Install {
                    plugin: plugin.clone(),
                },
                scope,
            )
            .unwrap();
            assert_eq!(installed["scope"], scope.to_string());
            assert_eq!(installed["enabled"], true);
            execute(
                &manager,
                &PluginCommand::Disable {
                    plugin: plugin.clone(),
                },
                scope,
            )
            .unwrap();
            assert!(
                !manager
                    .list()
                    .unwrap()
                    .iter()
                    .find(|p| p.scope == scope)
                    .unwrap()
                    .enabled
            );
            execute(
                &manager,
                &PluginCommand::Enable {
                    plugin: plugin.clone(),
                },
                scope,
            )
            .unwrap();
            assert!(
                manager
                    .list()
                    .unwrap()
                    .iter()
                    .find(|p| p.scope == scope)
                    .unwrap()
                    .enabled
            );
            let updated = execute(
                &manager,
                &PluginCommand::Update {
                    plugin: plugin.clone(),
                },
                scope,
            )
            .unwrap();
            assert_eq!(updated["version"], "1.0.0");
            execute(&manager, &PluginCommand::Uninstall { plugin }, scope).unwrap();
            assert!(manager.list().unwrap().is_empty());
        }
        execute(
            &manager,
            &marketplace(MarketplaceCommand::Update {
                name: "fixture".into(),
            }),
            PluginScope::User,
        )
        .unwrap();
        execute(
            &manager,
            &marketplace(MarketplaceCommand::Remove {
                name: "fixture".into(),
            }),
            PluginScope::User,
        )
        .unwrap();
        assert!(manager.list_marketplaces().unwrap().is_empty());
    }
}
