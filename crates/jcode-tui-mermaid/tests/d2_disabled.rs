use jcode_tui_mermaid::{
    D2Options, RenderResult, configure_d2, d2_rendering_enabled,
    render_d2_deferred_with_registration,
};

#[test]
fn disabled_client_snapshot_stays_disabled_and_never_resolves_missing_tool() {
    let options = D2Options {
        binary: "/nonexistent/jcode-d2-disabled-test".into(),
        ..Default::default()
    };
    configure_d2(false, options.clone());
    configure_d2(true, options);
    assert!(
        !d2_rendering_enabled(),
        "later config calls changed the startup snapshot"
    );
    let Some(RenderResult::Error(error)) =
        render_d2_deferred_with_registration("a -> b", Some(80), true, None)
    else {
        panic!("disabled D2 must not enqueue work");
    };
    assert!(error.contains("disabled"));
    assert!(
        !error.contains("not found"),
        "disabled renderer resolved the executable"
    );
}
