use super::*;

/// An external image (LaTeX formula PNG, `read` of an image file) must be
/// recoverable after the bounded render cache evicts it. Before this, the
/// hash -> path mapping lived only in that LRU, so a math-heavy transcript
/// silently lost its rendered formulas: the placeholder rows stayed
/// reserved and nothing could ever repaint them.
#[test]
fn evicted_external_image_is_rediscovered_from_its_remembered_path() {
    use image::{ImageBuffer, Rgba};

    let dir = std::env::temp_dir().join(format!(
        "jcode-external-image-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create fixture dir");
    let path = dir.join("formula.png");
    ImageBuffer::from_pixel(9, 4, Rgba([120u8, 160, 255, 255]))
        .save(&path)
        .expect("write fixture png");

    let hash = register_external_image(&path, 9, 4);
    assert!(
        get_cached_png(hash).is_some(),
        "registration should populate the render cache"
    );

    // Simulate LRU eviction of the render-cache entry.
    {
        let mut cache = RENDER_CACHE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache
            .entries
            .retain(|(entry_hash, _), _| *entry_hash != hash);
        cache.order.retain(|(entry_hash, _)| *entry_hash != hash);
    }
    assert!(
        !inline_image_is_materialized(hash),
        "eviction should leave the image unmaterialized"
    );

    assert_eq!(rediscover_external_image(hash), Some((hash, 9, 4)));
    assert!(
        inline_image_is_materialized(hash),
        "rediscovery must restore the render-cache entry"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A deleted cache file must fail cleanly instead of resurrecting a
/// dangling entry that would draw nothing.
#[test]
fn rediscovery_fails_when_the_external_image_file_is_gone() {
    let path = std::env::temp_dir().join(format!(
        "jcode-missing-external-image-{}-{:?}.png",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_file(&path);
    let hash = register_external_image(&path, 5, 5);
    assert_eq!(rediscover_external_image(hash), None);
}

#[test]
fn infer_protocol_detects_kitty_family() {
    assert_eq!(
        infer_protocol_from_env(Some("xterm-kitty"), None, None, None),
        Some(ProtocolType::Kitty)
    );
    assert_eq!(
        infer_protocol_from_env(None, Some("ghostty"), None, None),
        Some(ProtocolType::Kitty)
    );
    assert_eq!(
        infer_protocol_from_env(None, Some("HandTerm"), None, None),
        Some(ProtocolType::Kitty)
    );
    // KITTY_WINDOW_ID present is sufficient.
    assert_eq!(
        infer_protocol_from_env(Some("xterm-256color"), None, None, Some("3")),
        Some(ProtocolType::Kitty)
    );
}

#[test]
fn infer_protocol_detects_iterm_and_sixel() {
    // Real iTerm2 breaks on inline images, so it reports no protocol.
    assert_eq!(
        infer_protocol_from_env(None, Some("iTerm.app"), None, None),
        None
    );
    assert_eq!(
        infer_protocol_from_env(None, Some("WezTerm"), None, None),
        Some(ProtocolType::Iterm2)
    );
    assert_eq!(
        infer_protocol_from_env(Some("xterm-sixel"), None, None, None),
        Some(ProtocolType::Sixel)
    );
    assert_eq!(
        infer_protocol_from_env(Some("foot"), Some("foot"), None, None),
        Some(ProtocolType::Sixel)
    );
    // foot sets TERM=foot but no TERM_PROGRAM.
    assert_eq!(
        infer_protocol_from_env(Some("foot"), None, None, None),
        Some(ProtocolType::Sixel)
    );
}

#[test]
fn explicit_wezterm_identity_avoids_incomplete_kitty_placeholders() {
    assert_eq!(
        infer_protocol_from_env(
            Some("xterm-kitty"),
            Some("WezTerm"),
            None,
            Some("stale-kitty-window")
        ),
        Some(ProtocolType::Iterm2)
    );
    assert_eq!(
        infer_protocol_from_env(Some("xterm-256color"), Some("konsole"), None, None),
        None
    );
}

#[test]
fn infer_protocol_misses_inside_masking_multiplexer() {
    // Herdr/tmux advertise a bland TERM with no graphics hints.
    assert_eq!(
        infer_protocol_from_env(Some("xterm-256color"), None, None, None),
        None
    );
}

#[test]
fn detect_multiplexer_identifies_each() {
    assert_eq!(
        detect_multiplexer(Some("xterm-256color"), None, None, None, Some("1")),
        Multiplexer::Herdr
    );
    assert_eq!(
        detect_multiplexer(Some("xterm-256color"), None, None, Some("0.40.1"), None),
        Multiplexer::Zellij
    );
    assert_eq!(
        detect_multiplexer(
            Some("tmux-256color"),
            Some("/tmp/tmux-1000/default,1,0"),
            None,
            None,
            None
        ),
        Multiplexer::Tmux
    );
    assert_eq!(
        detect_multiplexer(
            Some("screen.xterm-256color"),
            None,
            Some("1234.pts-0.host"),
            None,
            None
        ),
        Multiplexer::Screen
    );
    // TERM prefix alone is enough for screen/tmux even without TMUX/STY.
    assert_eq!(
        detect_multiplexer(Some("screen-256color"), None, None, None, None),
        Multiplexer::Screen
    );
    assert_eq!(
        detect_multiplexer(Some("tmux-256color"), None, None, None, None),
        Multiplexer::Tmux
    );
    assert_eq!(
        detect_multiplexer(Some("xterm-kitty"), None, None, None, None),
        Multiplexer::None
    );
}

#[test]
fn detect_multiplexer_herdr_wins_over_others() {
    // Herdr is the most specific signal even if a stale TMUX leaks through.
    assert_eq!(
        detect_multiplexer(
            Some("xterm-256color"),
            Some("/tmp/tmux"),
            None,
            None,
            Some("1")
        ),
        Multiplexer::Herdr
    );
}

#[test]
fn native_tmux_sixel_requires_server_and_client_support() {
    assert!(tmux_reports_native_sixel(
        "1\n",
        "256,clipboard,mouse,sixel,title"
    ));
    assert!(tmux_reports_native_sixel("1", "RGB sixel hyperlinks"));
    assert!(!tmux_reports_native_sixel("0", "sixel"));
    assert!(!tmux_reports_native_sixel("1", "256,clipboard,mouse"));
    assert!(!tmux_reports_native_sixel("1", "sixel-overwrite"));
}

#[test]
fn zellij_preserves_classic_kitty_unless_explicitly_disabled() {
    assert_eq!(
        adjust_protocol_for_multiplexer(ProtocolType::Kitty, Multiplexer::Zellij, false),
        ProtocolType::Halfblocks
    );
    assert_eq!(
        adjust_protocol_for_multiplexer(ProtocolType::Kitty, Multiplexer::Zellij, true),
        ProtocolType::Kitty
    );
    // Zellij's Sixel passthrough and other multiplexers are untouched.
    assert_eq!(
        adjust_protocol_for_multiplexer(ProtocolType::Sixel, Multiplexer::Zellij, false),
        ProtocolType::Sixel
    );
    for mux in [Multiplexer::None, Multiplexer::Tmux, Multiplexer::Herdr] {
        assert_eq!(
            adjust_protocol_for_multiplexer(ProtocolType::Kitty, mux, false),
            ProtocolType::Kitty
        );
    }
}

#[test]
fn decide_mode_respects_explicit_override() {
    // Force-on always probes, even when env already detected a protocol.
    assert_eq!(
        decide_picker_init_mode(
            Some(true),
            Some(ProtocolType::Kitty),
            Multiplexer::None,
            None
        ),
        PickerInitMode::Probe
    );
    // Force-off never probes, even on an env miss inside a multiplexer.
    assert_eq!(
        decide_picker_init_mode(Some(false), None, Multiplexer::Herdr, None),
        PickerInitMode::Fast
    );
    for term_program in [None, Some("Orca"), Some("kitty"), Some("xterm")] {
        assert_eq!(
            decide_picker_init_mode(Some(true), None, Multiplexer::None, term_program),
            PickerInitMode::Probe,
            "force-on for {term_program:?}"
        );
        assert_eq!(
            decide_picker_init_mode(Some(false), None, Multiplexer::None, term_program),
            PickerInitMode::Fast,
            "force-off for {term_program:?}"
        );
    }
}

#[test]
fn decide_mode_probes_orca_case_insensitively() {
    for term_program in ["Orca", "orca", "ORCA", "oRcA"] {
        assert_eq!(
            decide_picker_init_mode(None, None, Multiplexer::None, Some(term_program)),
            PickerInitMode::Probe,
            "{term_program}"
        );
    }
}

#[test]
fn decide_mode_defaults_to_fast_path() {
    // Env already identified a graphics terminal: stay fast.
    assert_eq!(
        decide_picker_init_mode(None, Some(ProtocolType::Kitty), Multiplexer::None, None),
        PickerInitMode::Fast
    );
    // An env miss must not turn an optional capability query into a
    // two-second startup stall. Multiplexer users can opt in explicitly.
    assert_eq!(
        decide_picker_init_mode(None, None, Multiplexer::Herdr, None),
        PickerInitMode::Fast
    );
    assert_eq!(
        decide_picker_init_mode(None, None, Multiplexer::None, None),
        PickerInitMode::Fast
    );
    for term_program in [
        None,
        Some(""),
        Some("kitty"),
        Some("ghostty"),
        Some("WezTerm"),
        Some("iTerm.app"),
        Some("foot"),
        Some("xterm"),
        Some("Orca Nightly"),
        Some("not-orca"),
        Some(" Orca "),
    ] {
        assert_eq!(
            decide_picker_init_mode(None, None, Multiplexer::None, term_program),
            PickerInitMode::Fast,
            "{term_program:?}"
        );
    }
}

#[test]
fn decide_mode_keeps_orca_multiplexers_fast_unless_forced() {
    for multiplexer in [
        Multiplexer::Tmux,
        Multiplexer::Screen,
        Multiplexer::Herdr,
        Multiplexer::Zellij,
    ] {
        assert_eq!(
            decide_picker_init_mode(None, None, multiplexer, Some("Orca")),
            PickerInitMode::Fast
        );
        assert_eq!(
            decide_picker_init_mode(Some(true), None, multiplexer, Some("Orca")),
            PickerInitMode::Probe
        );
    }
}

#[test]
fn probe_env_helper_back_compat() {
    // Unset / disabled keeps the historical fast default.
    assert_eq!(picker_init_mode_from_probe_env(None), PickerInitMode::Fast);
    assert_eq!(
        picker_init_mode_from_probe_env(Some("0")),
        PickerInitMode::Fast
    );
    // Explicit enable still probes.
    assert_eq!(
        picker_init_mode_from_probe_env(Some("1")),
        PickerInitMode::Probe
    );
}

#[test]
fn only_halfblocks_uses_the_user_facing_text_image_fallback() {
    assert!(protocol_uses_text_image_fallback(
        Some(ProtocolType::Halfblocks),
        false
    ));
    assert!(!protocol_uses_text_image_fallback(
        Some(ProtocolType::Kitty),
        false
    ));
    assert!(!protocol_uses_text_image_fallback(
        Some(ProtocolType::Iterm2),
        false
    ));
    assert!(!protocol_uses_text_image_fallback(
        Some(ProtocolType::Sixel),
        false
    ));
    assert!(!protocol_uses_text_image_fallback(None, false));
    assert!(!protocol_uses_text_image_fallback(
        Some(ProtocolType::Halfblocks),
        true
    ));
}

#[test]
fn native_image_capability_excludes_halfblocks_but_allows_video_export() {
    assert!(protocol_supports_native_images(
        Some(ProtocolType::Kitty),
        false
    ));
    assert!(protocol_supports_native_images(
        Some(ProtocolType::Iterm2),
        false
    ));
    assert!(protocol_supports_native_images(
        Some(ProtocolType::Sixel),
        false
    ));
    assert!(!protocol_supports_native_images(
        Some(ProtocolType::Halfblocks),
        false
    ));
    assert!(!protocol_supports_native_images(None, false));
    assert!(protocol_supports_native_images(None, true));
}

#[test]
fn native_image_capability_respects_scoped_test_override() {
    with_image_protocol_override(Some(false), || {
        assert!(!native_image_protocol_available());
    });
    with_image_protocol_override(Some(true), || {
        assert!(native_image_protocol_available());
    });
}
