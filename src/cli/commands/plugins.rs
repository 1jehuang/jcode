use anyhow::Result;
use jcode_provider_extensions::{PluginBundle, PluginSource, PluginStore, ProviderRegistry};
use serde::Serialize;
use std::path::Path;

fn store() -> Result<PluginStore> {
    Ok(PluginStore::open(PluginStore::default_root()?))
}

#[derive(Debug, Serialize)]
struct PluginInspection<'a> {
    manifest: &'a jcode_provider_extensions::PluginManifest,
    provider: bool,
    skills: &'a [jcode_provider_extensions::SkillMetadata],
    unsupported: Vec<&'static str>,
}

pub(crate) fn run_plugin_inspect_command(source: &str, json: bool) -> Result<()> {
    let store = store()?;
    let bundle = if Path::new(source).is_dir() {
        PluginBundle::load(source)?
    } else {
        let source = PluginSource::parse(source)?;
        store.inspect(&source)?
    };
    let report = PluginInspection {
        manifest: &bundle.manifest,
        provider: bundle.provider_manifest.is_some(),
        skills: &bundle.skills,
        unsupported: bundle.unsupported_components(),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Plugin: {} v{}",
            bundle.manifest.name, bundle.manifest.version
        );
        if let Some(description) = &bundle.manifest.description {
            println!("Description: {description}");
        }
        println!(
            "Provider backend: {}",
            if report.provider {
                "external subprocess"
            } else {
                "metadata-only"
            }
        );
        if bundle.skills.is_empty() {
            println!("Skills: none");
        } else {
            println!(
                "Skills: {}",
                bundle
                    .skills
                    .iter()
                    .map(|skill| skill.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if !report.unsupported.is_empty() {
            println!("Unsupported surfaces: {}", report.unsupported.join(", "));
        }
    }
    Ok(())
}

pub(crate) fn run_plugin_add_command(source: &str, trusted: bool, json: bool) -> Result<()> {
    if Path::new(source).exists() {
        anyhow::bail!(
            "/plugin add expects a GitHub source such as owner/repository[@ref]; use `jcode provider extension add` for an existing local bundle"
        )
    }
    let source = PluginSource::parse(source)?;
    let store = store()?;
    let installed = store.install(&source)?;
    let provider_registered = match sync_plugin_provider(&store, &installed, trusted) {
        Ok(registered) => registered,
        Err(error) => {
            return Err(rollback_after_error(&store, &installed, error));
        }
    };
    if json {
        println!(
            "{}",
            serde_json::json!({
                "status": "installed",
                "name": installed.metadata.name,
                "version": installed.metadata.version,
                "repository": installed.metadata.repository,
                "reference": installed.metadata.reference,
                "commit": installed.metadata.commit,
                "path": installed.metadata.path,
                "provider_registered": provider_registered,
                "trusted": trusted,
                "runtime_tier": if provider_registered { "external" } else { "metadata_only" },
                "unsupported": installed.bundle.unsupported_components(),
            })
        );
    } else {
        println!(
            "Installed plugin '{}' v{}.",
            installed.metadata.name, installed.metadata.version
        );
        println!("  commit:  {}", installed.metadata.commit);
        println!("  path:    {}", installed.metadata.path.display());
        println!(
            "  trust:   {}",
            if trusted { "trusted" } else { "untrusted" }
        );
        println!(
            "  tier:    {}",
            if provider_registered {
                "external subprocess"
            } else {
                "metadata-only"
            }
        );
        if !installed.bundle.unsupported_components().is_empty() {
            println!("  note:    unsupported surfaces remain metadata-only");
        }
    }
    Ok(())
}

pub(crate) fn run_plugin_update_command(name: &str, trusted: bool, json: bool) -> Result<()> {
    let store = store()?;
    let installed = store.update(name)?;
    let provider_registered = match sync_plugin_provider(&store, &installed, trusted) {
        Ok(registered) => registered,
        Err(error) => {
            return Err(rollback_after_error(&store, &installed, error));
        }
    };
    if json {
        println!(
            "{}",
            serde_json::json!({
                "status": "updated",
                "name": installed.metadata.name,
                "version": installed.metadata.version,
                "commit": installed.metadata.commit,
                "path": installed.metadata.path,
                "provider_registered": provider_registered,
                "runtime_tier": if provider_registered { "external" } else { "metadata_only" },
            })
        );
    } else {
        println!(
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
        );
    }
    Ok(())
}

fn ensure_plugin_provider_owner(
    registry: &ProviderRegistry,
    provider_id: &str,
    store: &PluginStore,
    plugin_name: &str,
) -> Result<()> {
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
) -> Result<bool> {
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
) -> Result<bool> {
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

pub(crate) fn run_plugin_list_command(json: bool) -> Result<()> {
    let plugins = store()?.list()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&plugins)?);
    } else if plugins.is_empty() {
        println!("No GitHub plugins installed.");
    } else {
        for plugin in plugins {
            println!(
                "{}\t{}\t{}\t{}",
                plugin.name,
                plugin.version,
                &plugin.commit[..12.min(plugin.commit.len())],
                plugin.repository
            );
        }
    }
    Ok(())
}

pub(crate) fn run_plugin_doctor_command(json: bool) -> Result<()> {
    let plugins = store()?.list()?;
    let mut reports = Vec::new();
    for plugin in plugins {
        let loaded = PluginBundle::load(&plugin.path);
        reports.push(serde_json::json!({
            "name": plugin.name,
            "version": plugin.version,
            "commit": plugin.commit,
            "status": if loaded.is_ok() { "pass" } else { "fail" },
            "error": loaded.err().map(|error| error.to_string()),
        }));
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else if reports.is_empty() {
        println!("No GitHub plugins installed.");
    } else {
        for report in reports {
            println!(
                "{}\t{}",
                report["name"].as_str().unwrap_or("unknown"),
                report["status"].as_str().unwrap_or("unknown")
            );
        }
    }
    Ok(())
}

pub(crate) fn run_plugin_trust_command(id: &str, json: bool) -> Result<()> {
    let mut registry = ProviderRegistry::open(ProviderRegistry::default_path()?)?;
    registry.set_trusted(id, true)?;
    registry.save()?;
    if json {
        println!("{}", serde_json::json!({"status": "trusted", "id": id}));
    } else {
        println!("Plugin provider '{}' is now trusted.", id);
    }
    Ok(())
}

pub(crate) fn run_plugin_remove_command(name: &str, json: bool) -> Result<()> {
    let store = store()?;
    let plugins = store.list()?;
    let provider_id = plugins
        .iter()
        .find(|plugin| plugin.name == name)
        .and_then(|plugin| plugin.provider_id.clone());
    let mut registry = ProviderRegistry::open(ProviderRegistry::default_path()?)?;
    let owned = if let Some(provider_id) = provider_id.as_deref() {
        let owned = registry
            .get(provider_id)
            .and_then(|record| record.source.as_deref())
            .is_some_and(|source| source.starts_with(&store.root().join(name)));
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
    store.remove(name)?;
    if let Some(provider_id) = provider_id {
        if owned {
            registry.remove(&provider_id)?;
            registry.save()?;
        }
    }
    if json {
        println!("{}", serde_json::json!({"status": "removed", "name": name}));
    } else {
        println!("Removed plugin '{}'.", name);
    }
    Ok(())
}
