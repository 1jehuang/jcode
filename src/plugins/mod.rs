pub mod types;
pub mod registry;
pub mod loader;
pub mod manager;
pub mod command;

pub use types::{PluginManifest, PluginCapability, InstalledPlugin};
pub use registry::PluginRegistry;
pub use loader::PluginLoader;
pub use manager::PluginManager;
pub use command::PluginCommand;