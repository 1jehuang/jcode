use super::*;

#[test]
fn test_remote_debug_frame_commands_request_a_fresh_draw() {
    for command in [
        "frame",
        "frame-normalized",
        "screen",
        "screen-json",
        "screen-json-normalized",
        "layout",
        "margins",
        "widgets",
        "info-widgets",
        "render-stats",
        "render-order",
        "anomalies",
        "theme",
    ] {
        assert!(
            remote::debug_command_needs_current_frame(command),
            "{command} should capture the current frame"
        );
    }

    for command in ["state", "picker", "memory", "draw-stats", "overlay:on"] {
        assert!(
            !remote::debug_command_needs_current_frame(command),
            "{command} should not force a draw"
        );
    }
}
