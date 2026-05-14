use super::registry::PluginRegistry;
use super::loader::PluginLoader;

pub struct PluginManager {
    registry: PluginRegistry,
}

impl PluginManager {
    pub fn new(plugins_dir: impl Into<std::path::PathBuf>) -> Self {
        PluginManager {
            registry: PluginRegistry::new(plugins_dir.into()),
        }
    }

    pub fn add(&mut self, source: &std::path::Path) -> Result<String, String> {
        let manifest = PluginLoader::install_from_local(source, self.registry.plugins_dir())?;
        let name = manifest.name.clone();
        self.registry.register(manifest, source.to_path_buf())?;
        Ok(format!("Plugin '{}' installed successfully", name))
    }

    pub fn remove(&mut self, name: &str) -> Result<(), String> {
        PluginLoader::uninstall(name, self.registry.plugins_dir())?;
        self.registry.remove(name).map(drop)
    }

    pub fn list(&self) -> Vec<&super::types::InstalledPlugin> {
        self.registry.list()
    }

    pub fn enable(&mut self, name: &str) -> Result<(), String> { self.registry.enable(name) }
    pub fn disable(&mut self, name: &str) -> Result<(), String> { self.registry.disable(name) }
    pub fn count(&self) -> usize { self.registry.count() }

    pub fn plugins_dir(&self) -> &std::path::Path { self.registry.plugins_dir() }
}
