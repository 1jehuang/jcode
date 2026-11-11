use super::*;

#[test]
fn selected_runtime_bridge_wins_over_stale_standalone() {
    let root = tempfile::tempdir().unwrap();
    let runtime = root.path().join("jcode");
    let standalone = root.path().join("jcode-harness-api-bridge");
    fs::write(&standalone, b"obsolete bridge").unwrap();
    // An explicit command can also be a bare name resolved through PATH.
    assert_eq!(
        shared_bridge_launch(&runtime, &standalone, true),
        (runtime.as_path(), &["api-bridge"][..])
    );
    fs::write(&runtime, b"current runtime").unwrap();
    assert_eq!(
        shared_bridge_launch(&runtime, &standalone, false),
        (runtime.as_path(), &["api-bridge"][..])
    );
}

#[test]
fn standalone_only_bundle_and_path_runtime_remain_supported() {
    let root = tempfile::tempdir().unwrap();
    let runtime = root.path().join("jcode");
    let standalone = root.path().join("jcode-harness-api-bridge");
    assert_eq!(
        shared_bridge_launch(&runtime, &standalone, false),
        (runtime.as_path(), &["api-bridge"][..])
    );
    fs::write(&standalone, b"standalone-only bundle").unwrap();
    assert_eq!(
        shared_bridge_launch(&runtime, &standalone, false),
        (standalone.as_path(), &[][..])
    );
}

#[test]
fn sibling_executable_uses_platform_suffix_in_bundle_with_spaces() {
    let root = tempfile::tempdir().unwrap();
    let bundle = root.path().join("Jcode package with spaces 雪");
    fs::create_dir(&bundle).unwrap();
    let current = bundle.join(format!("jcode-desktop{}", std::env::consts::EXE_SUFFIX));
    for name in ["jcode", "jcode-harness-api-bridge"] {
        let companion = bundle.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        fs::write(&companion, b"test executable").unwrap();
        assert_eq!(sibling_exe_next_to(Some(&current), name), companion);
    }
}

#[test]
fn sibling_executable_fallback_uses_platform_suffix() {
    let root = tempfile::tempdir().unwrap();
    let current = root.path().join("jcode-desktop");
    for name in ["jcode", "jcode-harness-api-bridge"] {
        let expected = PathBuf::from(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        assert_eq!(sibling_exe_next_to(None, name), expected);
        assert_eq!(sibling_exe_next_to(Some(&current), name), expected);
        fs::create_dir(root.path().join(&expected)).unwrap();
        assert_eq!(sibling_exe_next_to(Some(&current), name), expected);
    }
}
