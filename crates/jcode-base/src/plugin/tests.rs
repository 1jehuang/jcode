use super::*;

fn git_fixture_command(root: &Path, args: &[&str]) -> String {
    let output = git_command()
        .args([
            "-c",
            "user.name=Offline Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn offline_git_branch_tag_sha_and_subdir_have_exact_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    git_fixture_command(&repo, &["init", "--initial-branch=main"]);
    fs::create_dir_all(repo.join("tools/plugin")).unwrap();
    fs::write(repo.join("tools/plugin/bytes"), "first revision").unwrap();
    git_fixture_command(&repo, &["add", "."]);
    git_fixture_command(&repo, &["commit", "-m", "first"]);
    let first = git_fixture_command(&repo, &["rev-parse", "HEAD"]);
    git_fixture_command(&repo, &["tag", "-a", "v1", "-m", "release"]);
    git_fixture_command(&repo, &["branch", "pinned-branch"]);
    fs::write(repo.join("tools/plugin/bytes"), "second revision").unwrap();
    git_fixture_command(&repo, &["commit", "-am", "second"]);
    let second = git_fixture_command(&repo, &["rev-parse", "HEAD"]);
    for (index, git_ref, sha, expected) in [
        (0, Some("pinned-branch"), None, "first revision"),
        (1, Some("v1"), None, "first revision"),
        (2, None, Some(first.as_str()), "first revision"),
        (
            3,
            Some("does-not-exist"),
            Some(first.as_str()),
            "first revision",
        ),
        (4, Some("main"), None, "second revision"),
    ] {
        let checkout = acquire_git(
            repo.to_str().unwrap(),
            &temp.path().join(format!("clone-{index}")),
            git_ref,
            sha,
        )
        .unwrap();
        let subdir = git_subdir(&checkout, "./tools/plugin").unwrap();
        assert!(subdir.starts_with(checkout.canonicalize().unwrap()));
        assert_eq!(fs::read_to_string(subdir.join("bytes")).unwrap(), expected);
        assert_eq!(
            git_output(&checkout, &["rev-parse", "HEAD"]).unwrap(),
            if index == 4 { &second } else { &first }.as_str()
        );
    }
    assert!(
        acquire_git(
            repo.to_str().unwrap(),
            &temp.path().join("bad-ref"),
            Some("missing"),
            None
        )
        .is_err()
    );
    assert!(
        acquire_git(
            repo.to_str().unwrap(),
            &temp.path().join("bad-sha"),
            None,
            Some("0000000000000000000000000000000000000000")
        )
        .is_err()
    );
}

#[test]
fn git_subdir_rejects_traversal_and_symlink_components() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("valid/child")).unwrap();
    for path in [
        "../repo",
        "valid/../../repo",
        "/tmp",
        "",
        "valid\\..\\outside",
    ] {
        assert!(git_subdir(&repo, path).is_err(), "accepted {path:?}");
    }
    fs::write(repo.join("file"), "file").unwrap();
    assert!(git_subdir(&repo, "file").is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("valid", repo.join("internal-link")).unwrap();
        std::os::unix::fs::symlink(temp.path(), repo.join("external-link")).unwrap();
        for path in ["internal-link", "internal-link/child", "external-link"] {
            assert!(git_subdir(&repo, path).is_err(), "accepted symlink {path}");
        }
    }
}

#[test]
fn source_schema_failures_precede_any_clone() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("clone");
    for source in [
        serde_json::json!({"source":"url","url":"https://example.invalid/repo","ref":true}),
        serde_json::json!({"source":"url","url":"https://example.invalid/repo","sha":"abc"}),
        serde_json::json!({"source":"git-subdir","url":"owner/repo","path":"../escape"}),
        serde_json::json!({"source":"git-subdir","url":"owner/repo"}),
        serde_json::json!({"source":"url","url":"https://example.invalid/repo","subdir":"plugin"}),
        serde_json::json!({"source":"command","command":"touch unwanted"}),
    ] {
        assert!(
            plugin_source(temp.path(), &source, &target).is_err(),
            "accepted {source}"
        );
        assert!(!target.exists());
    }
    for git_ref in [
        "-option",
        "main:other",
        "HEAD~1",
        "a..b",
        "a\nb",
        "a^{commit}",
    ] {
        assert!(validate_git_pin(Some(git_ref), None).is_err());
    }
}

#[test]
fn invalid_remote_update_preserves_installed_payload_and_registry() {
    let (_temp, manager, market) = fixture();
    manager.add_marketplace(market.to_str().unwrap()).unwrap();
    let old = manager.install("demo", PluginScope::User).unwrap();
    let before = serde_json::to_value(manager.list().unwrap()).unwrap();
    fs::write(market.join(".claude-plugin/marketplace.json"),
        r#"{"name":"fixture","plugins":[{"name":"demo","source":{"source":"git-subdir","url":"owner/repo","path":"../escape","ref":"main"}}]}"#).unwrap();
    manager.update_marketplace("fixture").unwrap();
    assert!(manager.update("demo", PluginScope::User).is_err());
    assert_eq!(
        serde_json::to_value(manager.list().unwrap()).unwrap(),
        before
    );
    assert_eq!(
        fs::read_to_string(old.root.join("skills/hello.md")).unwrap(),
        "hello"
    );
}

#[test]
fn remote_url_shaped_directory_is_never_local() {
    const MARKER: &str = "JCODE_PLUGIN_URL_SHAPE_TEST";
    let url = "https://example.invalid/repo.git";
    if std::env::var_os(MARKER).is_some() {
        assert!(Path::new(url).is_dir());
        assert!(!source_is_local(url));
        for source in [
            serde_json::json!({"source":"url","url":url}),
            serde_json::json!({"source":"git-subdir","url":url,"path":"."}),
        ] {
            assert!(plugin_source(Path::new("."), &source, Path::new("clone")).is_err());
            assert!(!Path::new("clone").exists());
        }
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join(url)).unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "plugin::tests::remote_url_shaped_directory_is_never_local",
        ])
        .current_dir(temp.path())
        .env(MARKER, "1")
        .env("GIT_ALLOW_PROTOCOL", "")
        .status()
        .unwrap();
    assert!(status.success());
}
#[test]
fn relative_local_marketplace_source_is_persisted_canonically() {
    let (_t, m, market) = fixture();
    let cwd = std::env::current_dir().unwrap();
    // A lexical relative path may contain parents, but persisted update sources must not.
    let ancestor = cwd.ancestors().last().unwrap();
    let mut relative = PathBuf::new();
    for _ in cwd.components().skip(1) {
        relative.push("..");
    }
    relative.push(market.strip_prefix(ancestor).unwrap());
    assert!(relative.is_relative());
    let entry = m.add_marketplace(relative.to_str().unwrap()).unwrap();
    assert_eq!(PathBuf::from(entry.source), market.canonicalize().unwrap());
    m.update_marketplace("fixture").unwrap();
}
#[test]
fn standalone_validation_rejects_component_escape_and_unsupported_dependency_ranges() {
    let (_t, _m, market) = fixture();
    let root = market.join("demo");
    fs::write(
        root.join(".claude-plugin/plugin.json"),
        r#"{"name":"demo","agents":["../private"]}"#,
    )
    .unwrap();
    assert!(PluginManager::validate(&root).is_err());
    fs::write(
        root.join(".claude-plugin/plugin.json"),
        r#"{"name":"demo","dependencies":["missing@market@^1"]}"#,
    )
    .unwrap();
    assert!(
        PluginManager::validate(&root)
            .unwrap_err()
            .to_string()
            .contains("dependency")
    );
}

#[test]
fn remote_object_cannot_read_local_absolute_or_parent_paths() {
    let (t, m, market) = fixture();
    let secret = t.path().join("secret");
    fs::create_dir(&secret).unwrap();
    fs::write(secret.join("private.txt"), "not plugin content").unwrap();
    for kind in ["url", "github"] {
        for remote in [secret.to_str().unwrap(), "../secret", "/etc"] {
            let key = if kind == "github" { "repo" } else { "url" };
            let source = serde_json::json!({"source":kind,key:remote});
            assert!(plugin_source(&market, &source, &t.path().join("clone")).is_err());
        }
    }
    let source = serde_json::json!({"source":"url","url":secret});
    fs::write(market.join(".claude-plugin/marketplace.json"),serde_json::to_vec(&serde_json::json!({"name":"fixture","plugins":[{"name":"demo","source":source,"strict":false}]})).unwrap()).unwrap();
    m.add_marketplace(market.to_str().unwrap()).unwrap();
    assert!(m.install("demo", PluginScope::User).is_err());
    assert!(m.list().unwrap().is_empty());
    assert!(!m.base().join("cache").exists());
}
#[test]
fn remote_source_options_do_not_silently_checkout_wrong_revision() {
    let (t, _, market) = fixture();
    for key in ["path", "directory", "subdir"] {
        let source = serde_json::json!({"source":"github","repo":"owner/repo",key:"alternate"});
        assert!(
            plugin_source(&market, &source, &t.path().join("clone"))
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
    }
    for (key, value) in [("ref", "--upload-pack=bad"), ("sha", "alternate")] {
        let source = serde_json::json!({"source":"github","repo":"owner/repo",key:value});
        assert!(plugin_source(&market, &source, &t.path().join("clone")).is_err());
        assert!(!t.path().join("clone").exists());
    }
    assert!(!valid_github_repo("../secret"));
    assert!(!valid_github_repo("-arg/repo"));
    assert!(!valid_github_repo("owner/repo;echo"));
    assert!(valid_github_repo("owner/repository-name"));
    assert!(!valid_remote_url("file:///etc"));
    assert!(!valid_remote_url("ssh://host/path\noption"));
}
#[test]
fn public_user_loader_is_read_only_without_registry() {
    // No environment mutation or real user configuration is needed to test this contract.
    let (t, m, _) = fixture();
    assert!(m.enabled_user_roots().unwrap().is_empty());
    assert!(!t.path().join("home").exists());
}

#[test]
fn scope_precedence_disabled_mask_and_user_only() {
    let (_t, m, market) = fixture();
    m.add_marketplace(market.to_str().unwrap()).unwrap();
    for scope in [PluginScope::Local, PluginScope::User, PluginScope::Project] {
        m.install("demo", scope).unwrap();
    }
    assert_eq!(m.enabled_roots().unwrap()[0].scope, PluginScope::Local);
    assert_eq!(m.enabled_user_roots().unwrap()[0].scope, PluginScope::User);
    m.disable("demo", PluginScope::Local).unwrap();
    assert!(m.enabled_roots().unwrap().is_empty());
    assert_eq!(m.enabled_user_roots().unwrap().len(), 1);
    m.uninstall("demo", PluginScope::Local).unwrap();
    assert_eq!(m.enabled_roots().unwrap()[0].scope, PluginScope::Project);
    m.disable("demo", PluginScope::Project).unwrap();
    assert!(m.enabled_roots().unwrap().is_empty());
    m.uninstall("demo", PluginScope::Project).unwrap();
    assert_eq!(m.enabled_roots().unwrap()[0].scope, PluginScope::User);
}
#[test]
fn optional_manifest_and_marketplace_components() {
    let (_t, m, market) = fixture();
    fs::remove_file(market.join("demo/.claude-plugin/plugin.json")).unwrap();
    fs::write(market.join(".claude-plugin/marketplace.json"),r#"{"name":"fixture","plugins":[{"name":"demo","source":"./demo","version":"3.0.0","skills":["./skills"],"defaultEnabled":false}]}"#).unwrap();
    m.add_marketplace(market.to_str().unwrap()).unwrap();
    let p = m.install("demo", PluginScope::User).unwrap();
    assert!(!p.enabled);
    assert_eq!(p.version.as_deref(), Some("3.0.0"));
    let manifest = PluginManager::validate(&p.root).unwrap();
    assert_eq!(manifest.extra["skills"], serde_json::json!(["./skills"]));
    assert!(m.enabled_roots().unwrap().is_empty());
    m.enable("demo", PluginScope::User).unwrap();
    assert_eq!(m.enabled_roots().unwrap().len(), 1);
}
#[test]
fn malformed_duplicate_catalog_rejected() {
    let (_t, m, market) = fixture();
    fs::write(market.join(".claude-plugin/marketplace.json"),r#"{"name":"fixture","plugins":[{"name":"demo","source":"./demo"},{"name":"demo","source":"./demo"}]}"#).unwrap();
    assert!(m.add_marketplace(market.to_str().unwrap()).is_err());
    assert!(m.list_marketplaces().unwrap().is_empty());
}
#[test]
fn failed_add_can_retry_and_remove_can_readd() {
    let (_t, m, market) = fixture();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("absent", market.join("bad-link")).unwrap();
        assert!(m.add_marketplace(market.to_str().unwrap()).is_err());
        fs::remove_file(market.join("bad-link")).unwrap();
    }
    m.add_marketplace(market.to_str().unwrap()).unwrap();
    m.remove_marketplace("fixture").unwrap();
    m.add_marketplace(market.to_str().unwrap()).unwrap();
    assert_eq!(m.list_marketplaces().unwrap().len(), 1);
}
#[test]
fn parallel_project_installs_do_not_lose_updates() {
    let (t, m, market) = fixture();
    m.add_marketplace(market.to_str().unwrap()).unwrap();
    let threads: Vec<_> = (0..8)
        .map(|n| {
            let manager =
                PluginManager::new(t.path().join("home"), t.path().join(format!("project-{n}")));
            std::thread::spawn(move || manager.install("demo", PluginScope::Project).unwrap())
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(m.read().unwrap().installed.len(), 8);
}
#[cfg(unix)]
#[test]
fn owned_cache_symlink_redirect_rejected() {
    let (t, m, market) = fixture();
    m.add_marketplace(market.to_str().unwrap()).unwrap();
    let outside = t.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, m.base().join("cache")).unwrap();
    assert!(m.install("demo", PluginScope::User).is_err());
    assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
}
#[test]
fn component_path_escape_and_dependencies_explicitly_rejected() {
    let (_t, m, market) = fixture();
    fs::write(market.join(".claude-plugin/marketplace.json"),r#"{"name":"fixture","plugins":[{"name":"demo","source":"./demo","agents":["../private"]}]}"#).unwrap();
    m.add_marketplace(market.to_str().unwrap()).unwrap();
    assert!(m.install("demo", PluginScope::User).is_err());
    fs::write(market.join(".claude-plugin/marketplace.json"),r#"{"name":"fixture","plugins":[{"name":"demo","source":"./demo","dependencies":["other@fixture"]}]}"#).unwrap();
    m.update_marketplace("fixture").unwrap();
    assert!(
        m.install("demo", PluginScope::User)
            .unwrap_err()
            .to_string()
            .contains("dependency")
    );
}
#[test]
fn uninstall_ambiguous_registry_preserves_all_records() {
    let (_t, m, market) = fixture();
    m.add_marketplace(market.to_str().unwrap()).unwrap();
    m.install("demo", PluginScope::User).unwrap();
    let mut registry = m.read().unwrap();
    let mut other = registry.installed[0].clone();
    other.marketplace = "second".into();
    registry.installed.push(other);
    m.save(&registry).unwrap();
    assert!(m.uninstall("demo", PluginScope::User).is_err());
    assert_eq!(m.list().unwrap().len(), 2);
    assert!(m.enabled_roots().unwrap().is_empty());
    m.uninstall("demo@second", PluginScope::User).unwrap();
    assert_eq!(m.list().unwrap().len(), 1);
}
fn fixture() -> (tempfile::TempDir, PluginManager, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let project = t.path().join("project");
    fs::create_dir(&project).unwrap();
    let market = t.path().join("market");
    fs::create_dir_all(market.join(".claude-plugin")).unwrap();
    fs::create_dir_all(market.join("demo/.claude-plugin")).unwrap();
    fs::write(
        market.join(".claude-plugin/marketplace.json"),
        r#"{"name":"fixture","plugins":[{"name":"demo","source":"./demo"}]}"#,
    )
    .unwrap();
    fs::write(
        market.join("demo/.claude-plugin/plugin.json"),
        r#"{"name":"demo","version":"1.0.0"}"#,
    )
    .unwrap();
    fs::create_dir(market.join("demo/skills")).unwrap();
    fs::write(market.join("demo/skills/hello.md"), "hello").unwrap();
    let manager = PluginManager::new(t.path().join("home"), project);
    (t, manager, market)
}
#[test]
fn offline_lifecycle_and_generation() {
    let (_t, m, market) = fixture();
    let before = generation();
    m.add_marketplace(market.to_str().unwrap()).unwrap();
    assert_eq!(m.discover().unwrap()[0].name, "demo");
    let p = m.install("demo@fixture", PluginScope::User).unwrap();
    assert_eq!(p.version.as_deref(), Some("1.0.0"));
    assert!(p.root.join("skills/hello.md").exists());
    assert!(generation() > before);
    assert_eq!(m.enabled_roots().unwrap().len(), 1);
    m.disable("demo", PluginScope::User).unwrap();
    assert!(m.enabled_roots().unwrap().is_empty());
    m.enable("demo", PluginScope::User).unwrap();
    assert_eq!(m.enabled_roots().unwrap().len(), 1);
    fs::write(
        market.join("demo/.claude-plugin/plugin.json"),
        r#"{"name":"demo","version":"2.0.0"}"#,
    )
    .unwrap();
    m.update_marketplace("fixture").unwrap();
    let p = m.update("demo", PluginScope::User).unwrap();
    assert_eq!(p.version.as_deref(), Some("2.0.0"));
    m.uninstall("demo", PluginScope::User).unwrap();
    assert!(m.list().unwrap().is_empty());
    m.remove_marketplace("fixture").unwrap();
    assert!(m.list_marketplaces().unwrap().is_empty());
}
#[test]
fn read_only_discovery_creates_nothing() {
    let (t, m, _) = fixture();
    assert!(m.list().unwrap().is_empty());
    assert!(m.discover().unwrap().is_empty());
    assert!(m.enabled_roots().unwrap().is_empty());
    assert!(!t.path().join("home").exists());
}
#[test]
fn scope_isolation() {
    let (t, m, market) = fixture();
    m.add_marketplace(market.to_str().unwrap()).unwrap();
    for scope in [PluginScope::User, PluginScope::Project, PluginScope::Local] {
        m.install("demo", scope).unwrap();
    }
    assert_eq!(m.list().unwrap().len(), 3);
    let other = PluginManager::new(t.path().join("home"), t.path().join("other"));
    assert_eq!(other.list().unwrap().len(), 1);
    m.disable("demo", PluginScope::Project).unwrap();
    assert_eq!(m.list().unwrap().iter().filter(|p| p.enabled).count(), 2);
}
#[test]
fn update_failure_preserves_working_plugin() {
    let (_t, m, market) = fixture();
    m.add_marketplace(market.to_str().unwrap()).unwrap();
    let p = m.install("demo", PluginScope::User).unwrap();
    fs::write(market.join("demo/.claude-plugin/plugin.json"), "invalid").unwrap();
    m.update_marketplace("fixture").unwrap();
    assert!(m.update("demo", PluginScope::User).is_err());
    assert_eq!(m.list().unwrap()[0].root, p.root);
    assert_eq!(m.enabled_roots().unwrap().len(), 1);
}
#[test]
fn traversal_rejected() {
    let (_t, m, market) = fixture();
    fs::write(
        market.join(".claude-plugin/marketplace.json"),
        r#"{"name":"fixture","plugins":[{"name":"demo","source":"../escape"}]}"#,
    )
    .unwrap();
    m.add_marketplace(market.to_str().unwrap()).unwrap();
    assert!(m.install("demo", PluginScope::User).is_err());
    assert!(m.list().unwrap().is_empty());
    assert!(safe_name("../oops").is_err());
    assert!(safe_name("--config").is_ok());
}
#[cfg(unix)]
#[test]
fn symlink_content_rejected() {
    let (t, m, market) = fixture();
    fs::write(t.path().join("secret"), "private").unwrap();
    std::os::unix::fs::symlink(t.path().join("secret"), market.join("demo/leak")).unwrap();
    assert!(m.add_marketplace(market.to_str().unwrap()).is_err());
    assert!(m.list_marketplaces().unwrap().is_empty());
}
#[test]
fn corrupt_registry_fails_closed() {
    let (_t, m, _) = fixture();
    fs::create_dir_all(m.base()).unwrap();
    fs::write(m.base().join("registry.json"), "broken").unwrap();
    assert!(m.list().is_err());
}
