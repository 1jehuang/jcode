#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_manifest_validation() {
        let valid_manifest = PluginManifest {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            description: "A test plugin".to_string(),
            author: Some("Test Author".to_string()),
            license: Some("MIT".to_string()),
            entry_point: "main.rs".to_string(),
            dependencies: vec![],
            capabilities: vec![PluginCapability::Commands],
        };

        let errors = PluginLoader::validate_manifest(valid_manifest);
        assert!(errors.is_empty(), "Valid manifest should have no errors");
    }

    #[test]
    fn test_plugin_manifest_invalid_name() {
        let invalid_manifest = PluginManifest {
            name: "".to_string(),
            version: "1.0.0".to_string(),
            description: "Test".to_string(),
            author: None,
            license: None,
            entry_point: "main.rs".to_string(),
            dependencies: vec![],
            capabilities: vec![],
        };

        let errors = PluginLoader::validate_manifest(invalid_manifest);
        assert!(!errors.is_empty(), "Empty name should produce error");
        assert!(errors.iter().any(|e| e.contains("name")));
    }

    #[test]
    fn test_plugin_registry_operations() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let mut registry = PluginRegistry::new(temp_dir.path());

        assert_eq!(registry.count(), 0, "New registry should be empty");

        let manifest = PluginManifest {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            description: "Test plugin".to_string(),
            author: None,
            license: None,
            entry_point: "main.rs".to_string(),
            dependencies: vec![],
            capabilities: vec![PluginCapability::Commands],
        };

        registry.register(manifest.clone(), PathBuf::from("/test")).unwrap();
        assert_eq!(registry.count(), 1);

        let plugin = registry.get("test-plugin");
        assert!(plugin.is_some());
        assert!(plugin.unwrap().enabled);

        registry.disable("test-plugin").unwrap();
        assert!(!registry.get("test-plugin").unwrap().enabled);
        assert_eq!(registry.count_enabled(), 0);

        registry.enable("test-plugin").unwrap();
        assert_eq!(registry.count_enabled(), 1);

        let removed = registry.remove("test-plugin");
        assert!(removed.is_ok());
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_plugin_manager_lifecycle() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let mut manager = PluginManager::new(temp_dir.path());

        assert_eq!(manager.count(), 0);

        let result = manager.remove("nonexistent");
        assert!(result.is_err());

        let list = manager.list();
        assert!(list.is_empty());
    }
}
