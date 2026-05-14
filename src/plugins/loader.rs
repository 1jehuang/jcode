use super::types::PluginManifest;

pub struct PluginLoader;

impl PluginLoader {
    pub fn load_from_manifest(path: &std::path::Path) -> Result<PluginManifest, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read manifest: {}", e))?;
        let manifest: PluginManifest = serde_json::from_str(&content)
            .map_err(|e| format!("Invalid manifest JSON: {}", e))?;
        Ok(manifest)
    }

    pub fn validate_manifest(manifest: PluginManifest) -> Vec<String> {
        let mut errors = vec![];
        if manifest.name.is_empty() || !manifest.name.chars().all(|c: char| c.is_alphanumeric() || c == '-' || c == '_') {
            errors.push("Invalid plugin name (must be alphanumeric + -_)".into());
        }
        if manifest.version.is_empty() { errors.push("Version is required".into()); }
        if manifest.entry_point.is_empty() { errors.push("Entry point is required".into()); }
        errors
    }

    pub fn install_from_url(_url: &str, _target_dir: &std::path::Path) -> Result<PluginManifest, String> {
        Err("URL installation not yet implemented (requires network)".to_string())
    }

    pub fn install_from_local(source: &std::path::Path, target_dir: &std::path::Path) -> Result<PluginManifest, String> {
        let manifest_path = source.join("plugin.json");
        let manifest = Self::load_from_manifest(&manifest_path)?;
        let errors = Self::validate_manifest(manifest.clone());
        if !errors.is_empty() {
            return Err(format!("Validation failed: {}", errors.join("; ")));
        }
        let dest = target_dir.join(&manifest.name);
        std::fs::create_dir_all(&dest).map_err(|e| format!("Failed to create directory: {}", e))?;
        // Copy source to target
        for entry in std::fs::read_dir(source).unwrap().flatten() {
            let from = entry.path();
            let to = dest.join(entry.file_name());
            if from.is_dir() {
                copy_dir_recursive(&from, &to)?;
            } else {
                std::fs::copy(&from, &to).ok();
            }
        }
        Ok(manifest)
    }

    pub fn uninstall(plugin_name: &str, plugins_dir: &std::path::Path) -> Result<(), String> {
        let path = plugins_dir.join(plugin_name);
        if path.exists() {
            std::fs::remove_dir_all(&path).map_err(|e| format!("Failed to remove: {}", e))?;
        }
        Ok(())
    }
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("{}", e))?;
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).ok();
        }
    }
    Ok(())
}