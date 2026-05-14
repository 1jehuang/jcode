use std::path::PathBuf;

use super::types::{InstalledPlugin, PluginManifest};

pub struct PluginRegistry {
    plugins: std::collections::HashMap<String, InstalledPlugin>,
    plugins_dir: PathBuf,
}

impl PluginRegistry {
    pub fn new(plugins_dir: impl Into<PathBuf>) -> Self {
        let dir = plugins_dir.into();
        let mut registry = Self {
            plugins: std::collections::HashMap::new(),
            plugins_dir: dir.clone(),
        };
        if dir.exists() {
            registry.scan_directory(&dir);
        }
        registry
    }

    fn scan_directory(&mut self, dir: &PathBuf) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().map_or(false, |e| e == "json") {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Ok(manifest) = serde_json::from_str::<PluginManifest>(&content) {
                            self.plugins.insert(manifest.name.clone(), InstalledPlugin {
                                manifest,
                                install_path: path.clone(),
                                enabled: true,
                                installed_at: chrono::Utc::now(),
                            });
                        }
                    }
                }
            }
        }
    }

    pub fn list(&self) -> Vec<&InstalledPlugin> {
        self.plugins.values().collect()
    }

    pub fn get(&self, name: &str) -> Option<&InstalledPlugin> {
        self.plugins.get(name)
    }

    pub fn register(&mut self, manifest: PluginManifest, path: PathBuf) -> Result<(), String> {
        if self.plugins.contains_key(&manifest.name) {
            return Err(format!("Plugin '{}' already installed", manifest.name));
        }
        self.plugins.insert(manifest.name.clone(), InstalledPlugin {
            manifest,
            install_path: path,
            enabled: true,
            installed_at: chrono::Utc::now(),
        });
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> Result<InstalledPlugin, String> {
        self.plugins.remove(name).ok_or_else(|| format!("Plugin '{}' not found", name))
    }

    pub fn enable(&mut self, name: &str) -> Result<(), String> {
        let plugin = self.plugins.get_mut(name).ok_or_else(|| format!("Plugin '{}' not found", name))?;
        plugin.enabled = true;
        Ok(())
    }

    pub fn disable(&mut self, name: &str) -> Result<(), String> {
        let plugin = self.plugins.get_mut(name).ok_or_else(|| format!("Plugin '{}' not found", name))?;
        plugin.enabled = false;
        Ok(())
    }

    pub fn count(&self) -> usize { self.plugins.len() }
    pub fn count_enabled(&self) -> usize { self.plugins.values().filter(|p| p.enabled).count() }
    pub fn plugins_dir(&self) -> &PathBuf { &self.plugins_dir }
}