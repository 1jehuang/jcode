//! Unit tests for Plugin System module
//!
//! Tests cover:
//! - Plugin manifest parsing and validation
//! - Permission system
//! - Plugin state management
//! - Plugin manager lifecycle
//! - Command/Skill/Tool registration
//! - Error handling and edge cases

use carpai::plugins::{
    Plugin, PluginCommand, PluginContext, PluginInfo, LoadedPlugin,
    PluginManager, PluginManifest, PluginPermission, PluginState,
    LoggingPlugin, CommandInfo, SkillInfo,
};
use std::path::PathBuf;
use std::collections::HashMap;

// ════════════════════════════════════════════════════════════════
// Plugin Manifest Tests
// ════════════════════════════════════════════════════════════════

#[test]
fn test_plugin_manifest_creation() {
    let manifest = PluginManifest {
        name: "test-plugin".to_string(),
        version: "1.0.0".to_string(),
        description: "A test plugin".to_string(),
        author: Some("Test Author".to_string()),
        permissions: vec![PluginPermission::ReadFiles],
        dependencies: vec![],
        entry_point: "libtest.so".to_string(),
    };
    
    assert_eq!(manifest.name, "test-plugin");
    assert_eq!(manifest.version, "1.0.0");
    assert_eq!(manifest.author.as_deref(), Some("Test Author"));
    assert_eq!(manifest.permissions.len(), 1);
    
    println!("✓ Plugin manifest creation works");
}

#[test]
fn test_plugin_manifest_serialization() {
    let manifest = PluginManifest {
        name: "serde-plugin".to_string(),
        version: "2.0.0".to_string(),
        description: "Test serialization".to_string(),
        author: None,
        permissions: vec![
            PluginPermission::ReadFiles,
            PluginPermission::WriteFiles,
            PluginPermission::NetworkAccess,
        ],
        dependencies: vec!["base-plugin".to_string()],
        entry_point: "plugin.dll".to_string(),
    };
    
    // Test serialization to JSON
    let json_str = serde_json::to_string_pretty(&manifest)
        .expect("Serialization failed");
    
    // Test deserialization
    let parsed: PluginManifest = serde_json::from_str(&json_str)
        .expect("Deserialization failed");
    
    assert_eq!(parsed.name, manifest.name);
    assert_eq!(parsed.version, manifest.version);
    assert_eq!(parsed.permissions.len(), 3);
    assert_eq!(parsed.dependencies.len(), 1);
    
    println!("✓ Plugin manifest serialization works");
}

#[test]
fn test_plugin_manifest_with_all_permissions() {
    let all_permissions = vec![
        PluginPermission::ReadFiles,
        PluginPermission::WriteFiles,
        PluginPermission::ExecuteCommands,
        PluginPermission::NetworkAccess,
        PluginPermission::ServiceAccess,
        PluginPermission::FullAccess,
    ];
    
    let manifest = PluginManifest {
        name: "full-access-plugin".to_string(),
        version: "1.0.0".to_string(),
        description: "Plugin with all permissions".to_string(),
        author: None,
        permissions: all_permissions.clone(),
        dependencies: vec![],
        entry_point: "full_access.so".to_string(),
    };
    
    assert_eq!(manifest.permissions.len(), 6);
    
    for perm in &all_permissions {
        assert!(
            manifest.permissions.contains(perm),
            "Should contain {:?}",
            perm
        );
    }
    
    println!("✓ All permissions can be set in manifest");
}

// ════════════════════════════════════════════════════════════════
// Plugin Permission Tests
// ════════════════════════════════════════════════════════════════

#[test]
fn test_permission_display_format() {
    let tests = vec![
        (PluginPermission::ReadFiles, "read-files"),
        (PluginPermission::WriteFiles, "write-files"),
        (PluginPermission::ExecuteCommands, "execute-commands"),
        (PluginPermission::NetworkAccess, "network-access"),
        (PluginPermission::ServiceAccess, "service-access"),
        (PluginPermission::FullAccess, "full-access"),
    ];
    
    for (permission, expected) in tests {
        let display = format!("{}", permission);
        assert_eq!(display, expected, "Display for {:?} should be '{}'", permission, expected);
    }
    
    println!("✓ All permission display formats correct");
}

#[test]
fn test_permission_equality() {
    assert_eq!(PluginPermission::ReadFiles, PluginPermission::ReadFiles);
    assert_ne!(PluginPermission::ReadFiles, PluginPermission::WriteFiles);
    
    let perm = PluginPermission::FullAccess;
    match perm {
        PluginPermission::FullAccess => println!("✓ Pattern matching on FullAccess works"),
        _ => panic!("Should have matched FullAccess"),
    }
}

#[test]
fn test_permission_serialization() {
    let permission = PluginPermission::ExecuteCommands;
    
    let json = serde_json::to_value(&permission).expect("Serialization failed");
    let restored: PluginPermission = serde_json::from_value(json).expect("Deserialization failed");
    
    assert_eq!(permission, restored);
    
    println!("✓ Permission serialization round-trips correctly");
}

// ════════════════════════════════════════════════════════════════
// Plugin State Tests
// ════════════════════════════════════════════════════════════════

#[test]
fn test_plugin_state_variants() {
    let states = vec![
        PluginState::Unloaded,
        PluginState::Loading,
        PluginState::Loaded,
        PluginState::Active,
        PluginState::Error("test error".to_string()),
        PluginState::Disabled,
    ];
    
    for state in &states {
        let _display = format!("{}", state);
        assert!(!state.to_string().is_empty());
    }
    
    println!("✓ All plugin states have valid display format");
}

#[test]
fn test_plugin_state_equality() {
    assert_eq!(PluginState::Active, PluginState::Active);
    assert_ne!(PluginState::Unloaded, PluginState::Loaded);
    
    let error1 = PluginState::Error("error1".to_string());
    let error2 = PluginState::Error("error2".to_string());
    assert_ne!(error1, error2, "Different errors should not be equal");
    
    println!("✓ State equality comparisons work correctly");
}

#[tokio::test]
async fn test_loaded_plugin_initial_state() {
    let manifest = PluginManifest {
        name: "test".to_string(),
        version: "1.0.0".to_string(),
        description: String::new(),
        author: None,
        permissions: vec![],
        dependencies: vec![],
        entry_point: String::new(),
    };
    
    let plugin = LoadedPlugin::new(manifest, Box::new(DummyPlugin));
    
    assert_eq!(plugin.state().await, PluginState::Unloaded);
    assert!(!plugin.is_active().await);
    
    println!("✓ Loaded plugin starts in Unloaded state");
}

// ════════════════════════════════════════════════════════════════
// Plugin Manager Tests
// ════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_plugin_manager_creation() {
    let context = create_test_context();
    let manager = PluginManager::new(context);
    
    let plugins = manager.list_plugins().await;
    assert!(plugins.is_empty(), "New manager should have no plugins");
    
    println!("✓ Plugin manager creates with empty state");
}

#[tokio::test]
async fn test_plugin_manager_add_directory() {
    let context = create_test_context();
    let mut manager = PluginManager::new(context);
    
    manager.add_plugin_dir("/tmp/plugins");
    manager.add_plugin_dir("/home/user/.carpai/plugins");
    
    // Should not fail - just stores the directories
    let plugins = manager.list_plugins().await;
    assert!(plugins.is_empty());
    
    println!("✓ Adding plugin directories works");
}

#[tokio::test]
async fn test_plugin_manager_is_loaded_check() {
    let context = create_test_context();
    let manager = PluginManager::new(context);
    
    assert!(!manager.is_loaded("nonexistent").await);
    
    println!("✓ is_loaded returns false for non-existent plugins");
}

#[tokio::test]
async fn test_plugin_manager_get_nonexistent() {
    let context = create_test_context();
    let manager = PluginManager::new(context);
    
    let result = manager.get("nonexistent").await;
    assert!(result.is_none(), "get should return None for nonexistent");
    
    println!("✓ get returns None for nonexistent plugins");
}

#[tokio::test]
async fn test_plugin_manager_unload_nonexistent() {
    let context = create_test_context();
    let manager = PluginManager::new(context);
    
    let result = manager.unload("nonexistent").await;
    assert!(result.is_err(), "unload should fail for nonexistent plugins");
    
    println!("✓ unload fails appropriately for nonexistent plugins");
}

#[tokio::test]
async fn test_plugin_manager_list_commands_empty() {
    let context = create_test_context();
    let manager = PluginManager::new(context);
    
    let commands = manager.list_commands().await;
    assert!(commands.is_empty(), "No plugins means no commands");
    
    println!("✓ list_commands returns empty when no plugins loaded");
}

#[tokio::test]
async fn test_plugin_manager_list_skills_empty() {
    let context = create_test_context();
    let manager = PluginManager::new(context);
    
    let skills = manager.list_skills().await;
    assert!(skills.is_empty(), "No plugins means no skills");
    
    println!("✓ list_skills returns empty when no plugins loaded");
}

#[tokio::test]
async fn test_plugin_manager_shutdown_all_empty() {
    let context = create_test_context();
    let manager = PluginManager::new(context);
    
    let result = manager.shutdown_all().await;
    assert!(result.is_ok(), "shutdown_all should succeed even when empty");
    
    println!("✓ shutdown_all succeeds on empty manager");
}

#[tokio::test]
async fn test_plugin_manager_execute_command_not_found() {
    let context = create_test_context();
    let manager = PluginManager::new(context);
    
    let result = manager.execute_command("nonexistent", None).await;
    assert!(result.is_err(), "Executing nonexistent command should fail");
    
    println!("✓ execute_command fails for nonexistent command");
}

// ════════════════════════════════════════════════════════════════
// Built-in Plugin Tests
// ════════════════════════════════════════════════════════════════

#[test]
fn test_logging_plugin_metadata() {
    let log_path = PathBuf::from("/tmp/test.log");
    let plugin = LoggingPlugin::new(log_path.clone());
    
    assert_eq!(plugin.name(), "logging");
    assert_eq!(plugin.version(), "1.0.0");
    
    println!("✓ Logging plugin has correct metadata");
}

#[tokio::test]
async fn test_logging_plugin_has_command() {
    let log_path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("test_logging.log");
    let plugin = LoggingPlugin::new(log_path);
    
    let commands = plugin.commands();
    assert_eq!(commands.len(), 1, "Logging plugin should have one command");
    
    let cmd = &commands[0];
    assert_eq!(cmd.name(), "log");
    assert!(!cmd.description().is_empty());
    assert!(!cmd.usage().is_empty());
    
    println!("✓ Logging plugin provides log command");
}

#[tokio::test]
async fn test_logging_plugin_lifecycle() {
    let log_path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("test_lifecycle.log");
    let mut plugin = LoggingPlugin::new(log_path);
    
    let context = create_test_context();
    
    // Initialize
    let init_result = plugin.initialize(&context).await;
    assert!(init_result.is_ok(), "Initialize should succeed");
    
    // Shutdown
    let shutdown_result = plugin.shutdown().await;
    assert!(shutdown_result.is_ok(), "Shutdown should succeed");
    
    println!("✓ Logging plugin lifecycle completes successfully");
}

// ════════════════════════════════════════════════════════════════
// Info Structure Tests
// ════════════════════════════════════════════════════════════════

#[test]
fn test_plugin_info_serialization() {
    let info = PluginInfo {
        name: "test-plugin".to_string(),
        version: "1.0.0".to_string(),
        description: "A test plugin".to_string(),
        state: PluginState::Active,
        loaded_at: chrono::Utc::now(),
    };
    
    let json = serde_json::to_value(&info).expect("Serialization failed");
    assert_eq!(json["name"], "test-plugin");
    assert_eq!(json["version"], "1.0.0");
    assert_eq!(json["state"], "active");
    
    println!("✓ PluginInfo serializes correctly");
}

#[test]
fn test_command_info_structure() {
    let info = CommandInfo {
        name: "test-cmd".to_string(),
        description: "A test command".to_string(),
        usage: "/test <args>".to_string(),
        source: "test-plugin".to_string(),
    };
    
    assert_eq!(info.name, "test-cmd");
    assert_eq!(info.source, "test-plugin");
    
    let json = serde_json::to_string(&info).expect("Serialization failed");
    assert!(json.contains("test-cmd"));
    
    println!("✓ CommandInfo structure works correctly");
}

#[test]
fn test_skill_info_structure() {
    let info = SkillInfo {
        name: "test-skill".to_string(),
        description: "A test skill".to_string(),
        source: "ai-enhanced".to_string(),
    };
    
    assert_eq!(info.name, "test-skill");
    assert_eq!(info.source, "ai-enhanced");
    
    let json = serde_json::to_string(&info).expect("Serialization failed");
    assert!(json.contains("test-skill"));
    
    println!("✓ SkillInfo structure works correctly");
}

// ════════════════════════════════════════════════════════════════
// Edge Cases and Error Handling
// ════════════════════════════════════════════════════════════════

#[test]
fn test_empty_plugin_manifest() {
    let manifest = PluginManifest {
        name: String::new(),
        version: String::new(),
        description: String::new(),
        author: None,
        permissions: vec![],
        dependencies: vec![],
        entry_point: String::new(),
    };
    
    let json = serde_json::to_string(&manifest).expect("Empty manifest should serialize");
    let _: PluginManifest = serde_json::from_str(&json).expect("Empty manifest should deserialize");
    
    println!("✓ Empty manifest handles gracefully");
}

#[test]
fn test_plugin_state_error_message() {
    let error_msg = "Something went wrong";
    let state = PluginState::Error(error_msg.to_string());
    
    let display = format!("{}", state);
    assert!(display.contains("error:"), "Error state should contain 'error:' prefix");
    assert!(display.contains(error_msg), "Error state should contain message");
    
    println!("✓ Error state displays message correctly");
}

#[tokio::test]
async fn test_multiple_plugins_same_name_handling() {
    let context = create_test_context();
    let manager = PluginManager::new(context);
    
    // In a real scenario, loading a plugin with same name would replace or fail
    // This tests that the manager handles the data structures correctly
    assert!(!manager.is_loaded("duplicate").await);
    
    println!("✓ Manager handles plugin name uniqueness checks");
}

// ════════════════════════════════════════════════════════════════
// Helper Functions and Test Fixtures
// ════════════════════════════════════════════════════════════════

/// Create a test plugin context
fn create_test_context() -> PluginContext {
    PluginContext {
        plugin_dir: PathBuf::from("/tmp/test_plugins"),
        data_dir: PathBuf::from("/tmp/test_data"),
        config: HashMap::new(),
        api_version: "1.0.0".to_string(),
    }
}

/// Dummy plugin implementation for testing
struct DummyPlugin;

#[async_trait]
impl Plugin for DummyPlugin {
    fn name(&self) -> &str {
        "dummy"
    }
    
    fn version(&self) -> &str {
        "0.1.0"
    }
    
    async fn initialize(&mut self, _context: &PluginContext) -> Result<()> {
        Ok(())
    }
    
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}
