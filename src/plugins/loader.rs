use super::types::PluginManifest;
use std::path::Path;

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

    /// Install a plugin from a URL.
    /// Supports URLs pointing to plugin.json files.
    /// The manifest and referenced files are downloaded into `target_dir/<name>/`.
    pub fn install_from_url(url: &str, target_dir: &Path) -> Result<PluginManifest, String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("Failed to create runtime: {}", e))?;

        rt.block_on(async {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

            // Fetch plugin.json
            let manifest_url = if url.ends_with(".json") { url.to_string() } else { format!("{}/plugin.json", url.trim_end_matches('/')) };
            eprintln!("  Downloading manifest from: {}", manifest_url);
            let resp = client.get(&manifest_url).send().await
                .map_err(|e| format!("Failed to download manifest: {}", e))?;
            if !resp.status().is_success() {
                return Err(format!("HTTP {} from {}", resp.status(), manifest_url));
            }
            let text = resp.text().await.map_err(|e| format!("Failed to read response: {}", e))?;
            let manifest: PluginManifest = serde_json::from_str(&text)
                .map_err(|e| format!("Invalid plugin.json: {}", e))?;

            // Validate
            let errors = Self::validate_manifest(manifest.clone());
            if !errors.is_empty() {
                return Err(format!("Validation: {}", errors.join("; ")));
            }

            // Create target directory
            let dest = target_dir.join(&manifest.name);
            std::fs::create_dir_all(&dest).map_err(|e| format!("Failed to create dir: {}", e))?;

            // Write manifest file
            std::fs::write(dest.join("plugin.json"), &text)
                .map_err(|e| format!("Failed to write manifest: {}", e))?;

            // Download entry point if it's a relative path and not yet present
            let entry = Path::new(&manifest.entry_point);
            if entry.is_relative() && !dest.join(&manifest.entry_point).exists() {
                let base_url = manifest_url.rsplit_once('/').map(|(b, _)| b.to_string()).unwrap_or_default();
                let entry_url = format!("{}/{}", base_url, manifest.entry_point);
                eprintln!("  Downloading entry: {}", entry_url);
                match client.get(&entry_url).send().await {
                    Ok(r) if r.status().is_success() => {
                        let content = r.text().await.unwrap_or_default();
                        let entry_path = dest.join(&manifest.entry_point);
                        if let Some(parent) = entry_path.parent() { let _ = std::fs::create_dir_all(parent); }
                        let _ = std::fs::write(&entry_path, &content);
                    }
                    _ => eprintln!("  ⚠️  Could not download entry point '{}'", manifest.entry_point),
                }
            }

            eprintln!("  ✅ Plugin '{}' installed from URL", manifest.name);
            Ok(manifest)
        })
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

    pub fn install_from_url(url: &str, target_dir: &std::path::Path) -> Result<PluginManifest, String> {
        eprintln!("  📥 Downloading plugin from: {}", url);

        let temp_dir = std::env::temp_dir().join("carpai-plugin-download");
        std::fs::create_dir_all(&temp_dir).map_err(|e| format!("Failed to create temp dir: {}", e))?;

        let archive_path = temp_dir.join("plugin-archive.tar.gz");

        let response = attohttpc::get(url)
            .map_err(|e| format!("Failed to download: {}", e))?;

        if !response.is_success() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        let bytes = response.bytes()
            .map_err(|e| format!("Failed to read response: {}", e))?;

        std::fs::write(&archive_path, &bytes)
            .map_err(|e| format!("Failed to write archive: {}", e))?;

        eprintln!("  📦 Extracting archive...");
        let extract_dir = temp_dir.join("extracted");
        std::fs::create_dir_all(&extract_dir).ok();

        let file = std::fs::File::open(&archive_path)
            .map_err(|e| format!("Failed to open archive: {}", e))?;
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
        archive.unpack(&extract_dir)
            .map_err(|e| format!("Failed to extract: {}", e))?;

        let manifest_path = extract_dir.join("plugin.json");
        if !manifest_path.exists() {
            let alt_manifest = extract_dir.join("manifest.json");
            if alt_manifest.exists() {
                std::fs::rename(&alt_manifest, &manifest_path).ok();
            }
        }

        let manifest = Self::install_from_local(&extract_dir, target_dir)?;

        std::fs::remove_dir_all(&temp_dir).ok();

        eprintln!("  ✅ Plugin '{}' installed from URL successfully", manifest.name);
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