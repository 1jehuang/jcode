use super::*;

#[test]
fn latex_rendering_defaults_to_image_and_parses_all_modes() {
    assert_eq!(
        Config::default().display.latex_rendering,
        LatexRenderingMode::Image
    );
    for (value, expected) in [
        ("none", LatexRenderingMode::None),
        ("unicode", LatexRenderingMode::Unicode),
        ("image", LatexRenderingMode::Image),
    ] {
        let cfg: Config = toml::from_str(&format!("[display]\nlatex_rendering = \"{value}\"\n"))
            .expect("latex rendering mode should parse");
        assert_eq!(cfg.display.latex_rendering, expected);
        assert_eq!(LatexRenderingMode::parse(expected.as_str()), Some(expected));
    }
    // An unknown mode degrades to the default instead of failing the whole
    // config parse, which used to silently discard every other setting in
    // config.toml (issue #689).
    let cfg: Config = toml::from_str("[display]\ncentered = true\nlatex_rendering = \"canvas\"\n")
        .expect("an unknown latex mode must not invalidate the config");
    assert_eq!(cfg.display.latex_rendering, LatexRenderingMode::Image);
    assert!(cfg.display.centered, "unrelated settings must survive");
}

#[test]
fn latex_rendering_environment_override_accepts_aliases() {
    let _guard = crate::storage::lock_test_env();
    let previous = std::env::var_os("JCODE_LATEX_RENDERING");
    crate::env::set_var("JCODE_LATEX_RENDERING", "png");
    let mut cfg = Config::default();
    cfg.apply_env_overrides();
    assert_eq!(cfg.display.latex_rendering, LatexRenderingMode::Image);
    restore_env_var("JCODE_LATEX_RENDERING", previous);
}

/// The /latex command persists through set_latex_rendering, which is a
/// read-modify-write over the real config.toml. Pin the whole roundtrip at
/// this layer: write, then parse the file back and confirm the mode and an
/// unrelated sibling setting both survive, so a future refactor cannot make
/// the setter rewrite the file without keeping other keys.
#[test]
fn set_latex_rendering_roundtrips_through_config_toml() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    let previous_latex = std::env::var_os("JCODE_LATEX_RENDERING");
    crate::env::remove_var("JCODE_LATEX_RENDERING");

    // Seed a non-default sibling key so the test can catch a rewrite that
    // drops it.
    let seed = temp.path().join("config.toml");
    std::fs::write(
        &seed,
        "[display]\ncentered = false\nlatex_rendering = \"unicode\"\n",
    )
    .expect("seed config");

    Config::set_latex_rendering(LatexRenderingMode::None).expect("save mode");

    let raw = std::fs::read_to_string(&seed).expect("read config back");
    assert!(
        raw.contains("latex_rendering = \"none\""),
        "mode must be persisted, got: {raw}"
    );
    assert!(
        raw.contains("centered = false"),
        "unrelated display keys must survive the rewrite, got: {raw}"
    );

    let reloaded = Config::load_strict().expect("config parses after the write");
    assert_eq!(reloaded.display.latex_rendering, LatexRenderingMode::None);
    assert!(!reloaded.display.centered);

    restore_env_var("JCODE_LATEX_RENDERING", previous_latex);
    restore_env_var("JCODE_HOME", previous_home);
    Config::invalidate_cache();
}
