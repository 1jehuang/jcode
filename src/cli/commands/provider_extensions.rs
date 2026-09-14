use anyhow::Result;
use jcode_provider_extensions::{ProviderRecord, ProviderRegistry, load_manifest_file};
use serde::Serialize;
use std::path::{Path, PathBuf};

fn registry() -> Result<ProviderRegistry> {
    Ok(ProviderRegistry::open(ProviderRegistry::default_path()?)?)
}

#[derive(Debug, Serialize)]
struct ProviderExtensionReport<'a> {
    status: &'static str,
    provider: &'a ProviderRecord,
}

pub(crate) fn run_provider_extension_list_command(json: bool) -> Result<()> {
    let registry = registry()?;
    let providers: Vec<&ProviderRecord> = registry.list().collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&providers)?);
    } else if providers.is_empty() {
        println!("No external provider extensions registered.");
    } else {
        for provider in providers {
            println!(
                "{}\t{}\t{}\t{}",
                provider.manifest.id,
                provider.manifest.version,
                if provider.enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                if provider.trusted {
                    "trusted"
                } else {
                    "untrusted"
                },
            );
        }
    }
    Ok(())
}

pub(crate) fn run_provider_extension_add_command(
    manifest_path: &str,
    trusted: bool,
    json: bool,
) -> Result<()> {
    let path = PathBuf::from(manifest_path);
    let manifest = load_manifest_file(&path)?;
    let mut registry = registry()?;
    registry.register(
        manifest.clone(),
        Some(canonical_source_path(&path)?),
        trusted,
    )?;
    registry.save()?;
    let record = registry
        .get(&manifest.id)
        .expect("registered provider must be present");
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&ProviderExtensionReport {
                status: "registered",
                provider: record,
            })?
        );
    } else {
        println!("Registered external provider '{}'.", manifest.id);
        println!("  manifest: {}", path.display());
        println!(
            "  trust:    {}",
            if trusted { "trusted" } else { "untrusted" }
        );
    }
    Ok(())
}

pub(crate) fn run_provider_extension_remove_command(id: &str, json: bool) -> Result<()> {
    let mut registry = registry()?;
    let removed = registry.remove(id)?;
    registry.save()?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "status": "removed",
                "id": removed.manifest.id,
            })
        );
    } else {
        println!("Removed external provider '{}'.", removed.manifest.id);
    }
    Ok(())
}

pub(crate) fn run_provider_extension_set_enabled_command(
    id: &str,
    enabled: bool,
    json: bool,
) -> Result<()> {
    let mut registry = registry()?;
    registry.set_enabled(id, enabled)?;
    registry.save()?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "status": if enabled { "enabled" } else { "disabled" },
                "id": id,
                "enabled": enabled,
            })
        );
    } else {
        println!(
            "External provider '{}' {}.",
            id,
            if enabled { "enabled" } else { "disabled" }
        );
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    id: String,
    status: &'static str,
    enabled: bool,
    trusted: bool,
    executable: String,
    executable_available: bool,
    protocol_version: String,
}

pub(crate) fn run_provider_extension_doctor_command(id: Option<&str>, json: bool) -> Result<()> {
    let registry = registry()?;
    let records: Vec<&ProviderRecord> = match id {
        Some(id) => vec![
            registry
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("external provider '{id}' is not registered"))?,
        ],
        None => registry.list().collect(),
    };
    let reports: Vec<DoctorReport> = records
        .into_iter()
        .map(|record| {
            let available = executable_available(&record.manifest.executable);
            DoctorReport {
                id: record.manifest.id.clone(),
                status: if available { "pass" } else { "fail" },
                enabled: record.enabled,
                trusted: record.trusted,
                executable: record.manifest.executable.display().to_string(),
                executable_available: available,
                protocol_version: record.manifest.protocol_version.clone(),
            }
        })
        .collect();
    let failed = reports.iter().any(|report| report.status == "fail");
    if json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else if reports.is_empty() {
        println!("No external provider extensions registered.");
    } else {
        for report in &reports {
            println!("{}\t{}\t{}", report.id, report.status, report.executable);
        }
    }
    if failed {
        anyhow::bail!("one or more external provider checks failed")
    }
    Ok(())
}

fn canonical_source_path(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .map_err(|error| anyhow::anyhow!("cannot resolve manifest {}: {error}", path.display()))
}

fn executable_available(path: &Path) -> bool {
    if path.components().count() > 1 || path.is_absolute() {
        return path.is_file();
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|directory| directory.join(path).is_file())
}
