use super::{Config, ToolConfig};

#[test]
fn parallel_tools_defaults_require_explicit_opt_in() {
    assert!(!ToolConfig::default().parallel);
    assert!(!Config::default().tools.parallel);
    for (source, expected) in [
        ("", false),
        ("[tools]\n", false),
        ("[tools]\nparallel = true\n", true),
        ("[tools]\nparallel = false\n", false),
    ] {
        let config: Config = toml::from_str(source).unwrap();
        assert_eq!(config.tools.parallel, expected, "{source:?}");
    }
    let template: Config = toml::from_str(&Config::default_config_file_contents()).unwrap();
    assert!(!template.tools.parallel);
}

#[test]
fn parallel_tools_environment_precedence_and_cache_reload() {
    const CHILD: &str = "JCODE_TEST_PARALLEL_CONFIG_CHILD";
    if let Ok(override_value) = std::env::var(CHILD) {
        let path = Config::path().unwrap();
        assert!(!path.exists());
        assert_eq!(super::config().tools.parallel, override_value == "1");
        // Exercise both transitions without changing the child's environment.
        // Explicit invalidation avoids relying on filesystem timestamp precision.
        for configured in [false, true, false] {
            let previous = super::config();
            let previous_parallel = previous.tools.parallel;
            std::fs::write(&path, format!("[tools]\nparallel = {configured}\n")).unwrap();
            Config::invalidate_cache();
            let expected = match override_value.as_str() {
                "1" => true,
                "0" => false,
                _ => configured,
            };
            assert_eq!(Config::load().tools.parallel, expected);
            let reloaded = super::config();
            assert_eq!(reloaded.tools.parallel, expected);
            assert!(!std::ptr::eq(previous, reloaded));
            assert_eq!(previous.tools.parallel, previous_parallel);
        }
        return;
    }

    // Environment is set only on fresh subprocesses, never on the test runner.
    for override_value in ["absent", "0", "1", "invalid"] {
        let home = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--nocapture",
            ])
            .env(CHILD, override_value)
            .env("JCODE_HOME", home.path())
            .env_remove("JCODE_PARALLEL_TOOLS");
        if override_value != "absent" {
            child.env("JCODE_PARALLEL_TOOLS", override_value);
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "override={override_value}\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
