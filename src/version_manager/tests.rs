#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_manager_initialization() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let vm = VersionManager::new(temp_dir.path().to_path_buf());

        assert_eq!(vm.get_version_string(), "0.1.0");
        assert!(vm.list_rollback_points().is_empty());
    }

    #[test]
    fn test_install_version_creates_rollback_point() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let mut vm = VersionManager::new(temp_dir.path().to_path_buf());

        let result = vm.install_version(
            "1.0.0",
            vec!["Initial release".to_string()]
        );

        assert!(result.is_ok());
        assert!(result.unwrap().contains("1.0.0"));
        assert_eq!(vm.get_version_string(), "1.0.0");

        let rollbacks = vm.list_rollback_points();
        assert_eq!(rollbacks.len(), 1);
    }

    #[test]
    fn test_multiple_versions_and_rollbacks() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let mut vm = VersionManager::new(temp_dir.path().to_path_buf());

        vm.install_version("1.0.0", vec!["v1".to_string()]).ok();
        vm.install_version("1.1.0", vec!["v1.1".to_string()]).ok();
        vm.install_version("2.0.0", vec!["v2".to_string()]).ok();

        assert_eq!(vm.get_version_string(), "2.0.0");
        assert_eq!(vm.list_rollback_points().len(), 3);
    }

    #[test]
    fn test_rollback_to_previous() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let mut vm = VersionManager::new(temp_dir.path().to_path_buf());

        vm.install_version("1.0.0", vec![].into()).ok();
        vm.install_version("2.0.0", vec![].into()).ok();

        let result = vm.rollback("latest");
        assert!(result.is_ok());
        assert!(result.unwrap().contains("1.0.0"));
        assert_eq!(vm.get_version_string(), "1.0.0");

        let rollbacks = vm.list_rollback_points();
        assert_eq!(rollbacks.len(), 1);
    }

    #[test]
    fn test_rollback_by_id() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let mut vm = VersionManager::new(temp_dir.path().to_path_buf());

        vm.install_version("1.0.0", vec![].into()).ok();

        let rollbacks = vm.list_rollback_points();
        if !rollbacks.is_empty() {
            let rb_id = &rollbacks[0].id;
            vm.install_version("2.0.0", vec![].into()).ok();

            let result = vm.rollback(rb_id);
            assert!(result.is_ok());
            assert!(result.unwrap().contains("1.0.0"));
        }
    }

    #[test]
    fn test_manual_rollback_point() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let mut vm = VersionManager::new(temp_dir.path().to_path_buf());

        let result = vm.create_rollback_point("Before major refactor");
        assert!(result.is_ok());
        assert!(result.unwrap().contains("created"));

        assert_eq!(vm.list_rollback_points().len(), 1);
    }

    #[test]
    fn test_changelog_display() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let mut vm = VersionManager::new(temp_dir.path().to_path_buf());

        vm.install_version(
            "1.0.0",
            vec![
                "Added plugin system".to_string(),
                "Fixed bugs".to_string(),
                "Improved performance".to_string()
            ]
        ).ok();

        let changelog = vm.get_changelog(10);
        assert!(changelog.contains("1.0.0"));
        assert!(changelog.contains("plugin system"));
        assert!(changelog.contains("bugs"));
    }

    #[test]
    fn test_rollback_nonexistent_fails() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let mut vm = VersionManager::new(temp_dir.path().to_path_buf());

        let result = vm.rollback("nonexistent-id");
        assert!(result.is_err());
    }

    #[test]
    fn test_persistence_across_instances() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

        {
            let mut vm1 = VersionManager::new(temp_dir.path().to_path_buf());
            vm1.install_version("1.0.0", vec!["Test".to_string()]).ok();
        }

        {
            let vm2 = VersionManager::new(temp_dir.path().to_path_buf());
            assert_eq!(vm2.get_version_string(), "1.0.0");
            assert_eq!(vm2.list_rollback_points().len(), 1);
        }
    }
}
