//! Private instances need account model catalogs before their first live refresh.

#![cfg(unix)]

use jcode_sdk::{inherit_credentials, user_app_config_dir};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use std::process::Command;

#[test]
fn model_catalogs_are_inherited_as_private_snapshots() {
    // Re-execute with isolated platform paths rather than mutating the test
    // runner's environment or reading the developer's credentials.
    const ROOT: &str = "JCODE_SDK_CATALOG_TEST_ROOT";
    let Some(root) = std::env::var_os(ROOT) else {
        let sandbox = tempfile::tempdir().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "model_catalogs_are_inherited_as_private_snapshots",
                "--nocapture",
            ])
            .env(ROOT, sandbox.path())
            .env("HOME", sandbox.path().join("home"))
            .env("XDG_CONFIG_HOME", sandbox.path().join("config"))
            .env("APPDATA", sandbox.path().join("appdata"))
            .env_remove("JCODE_HOME")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated test failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    };
    let root = PathBuf::from(root);
    let from = root.join("user");
    let to = root.join("instance");
    let config = user_app_config_dir();
    assert!(inherit_credentials(&from, &to).unwrap().is_empty());

    fs::create_dir_all(&config).unwrap();
    let catalogs = [
        "openai_model_catalog_cache.json",
        "copilot_models_cache.json",
    ];
    let contents = r#"{"scopes":{"test-account":{"models":["account-only-model"],"observed_at_unix_secs":123}}}"#;
    for name in catalogs {
        fs::write(config.join(name), contents).unwrap();
    }
    for name in ["usage.json", "auth-validation.json", "other_cache.json"] {
        fs::write(config.join(name), "private state").unwrap();
    }
    fs::create_dir(config.join("directory_models_cache.json")).unwrap();
    symlink(
        config.join("missing"),
        config.join("missing_models_cache.json"),
    )
    .unwrap();
    fs::write(config.join("openai.env"), "TOKEN=synthetic").unwrap();

    let inherited = inherit_credentials(&from, &to).unwrap();
    assert!(inherited.contains(&PathBuf::from("config/jcode/openai.env")));
    assert!(
        fs::symlink_metadata(to.join("config/jcode/openai.env"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let instance_config = to.join("config/jcode");
    for name in catalogs {
        assert!(
            inherited.contains(&PathBuf::from("config/jcode").join(name)),
            "missing copied model catalog: {name}"
        );
        let destination = instance_config.join(name);
        let metadata = fs::symlink_metadata(&destination).unwrap();
        assert!(metadata.is_file());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::read_to_string(&destination).unwrap(), contents);
        fs::write(&destination, "instance refresh").unwrap();
        assert_eq!(fs::read_to_string(config.join(name)).unwrap(), contents);
        fs::write(config.join(name), "user refresh").unwrap();
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            "instance refresh"
        );
    }
    for name in [
        "usage.json",
        "auth-validation.json",
        "other_cache.json",
        "directory_models_cache.json",
        "missing_models_cache.json",
    ] {
        assert!(!instance_config.join(name).exists());
    }

    // Reused homes must replace file links without writing through them. Test
    // directory links with no .env files, so cache copying itself must repair them.
    fs::remove_file(config.join("openai.env")).unwrap();
    for name in catalogs {
        fs::remove_file(instance_config.join(name)).unwrap();
        symlink(config.join(name), instance_config.join(name)).unwrap();
    }
    fs::write(instance_config.join("keep.json"), "instance state").unwrap();
    inherit_credentials(&from, &to).unwrap();
    inherit_credentials(&from, &to).unwrap();
    assert_eq!(
        fs::read_to_string(instance_config.join("keep.json")).unwrap(),
        "instance state"
    );
    for name in catalogs {
        let destination = instance_config.join(name);
        assert!(fs::symlink_metadata(&destination).unwrap().is_file());
        assert_eq!(fs::read_to_string(&destination).unwrap(), "user refresh");
        fs::write(destination, "new instance refresh").unwrap();
        assert_eq!(
            fs::read_to_string(config.join(name)).unwrap(),
            "user refresh"
        );
    }
    for relative in ["config", "config/jcode"] {
        let reused = root.join(relative.replace('/', "-"));
        let link = reused.join(relative);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(config.parent().unwrap(), &link).unwrap();
        inherit_credentials(&from, &reused).unwrap();
        assert!(fs::symlink_metadata(link).unwrap().is_dir());
        assert_eq!(
            fs::read_to_string(config.join(catalogs[0])).unwrap(),
            "user refresh"
        );
        assert!(!config.parent().unwrap().join("jcode/jcode").exists());
    }
}
