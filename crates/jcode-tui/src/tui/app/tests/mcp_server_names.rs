// Unit tests for `App::refresh_mcp_server_names`.
//
// NOTE: the task brief listed this function under `crates/jcode-base/src/mcp/
// manager.rs`, but `refresh_mcp_server_names` actually lives on `App` in
// `crates/jcode-tui/src/tui/app/tui_lifecycle_runtime.rs`. It reads the live
// MCP manager and caches the connected-server set with per-server tool counts.
// These tests live in `jcode-tui` because that is where the function is
// defined; `cargo test --package jcode-tui --lib -- refresh_mcp_server_names`
// covers them.

/// With no MCP servers connected (the empty state on a fresh app),
/// `refresh_mcp_server_names` must not panic and must leave the cache empty.
#[test]
fn refresh_mcp_server_names_empty_state_is_noop() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        assert!(
            app.mcp_server_names.is_empty(),
            "a fresh app should have no cached MCP server names"
        );

        // The method is async; drive it on a dedicated runtime like the other
        // jcode-tui tests that exercise async App methods.
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(app.refresh_mcp_server_names());

        assert!(
            app.mcp_server_names.is_empty(),
            "no connected servers -> mcp_server_names should stay empty"
        );
    });
}

// ---------------------------------------------------------------------------
// `/mcp` slash-command dispatch tests.
//
// These verify that the TUI command handler claims `/mcp`, produces sensible
// output for `/mcp status`, and rejects unknown servers with an error message.
// ---------------------------------------------------------------------------

/// Text of the last message the app pushed, whatever its role.
fn last_message(app: &crate::tui::app::App) -> String {
    app.display_messages
        .last()
        .map(|message| message.content.clone())
        .unwrap_or_default()
}

#[test]
fn mcp_status_with_no_servers_reports_none_configured() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        assert!(
            crate::tui::app::commands_dispatch::dispatch_local_command(&mut app, "/mcp"),
            "/mcp should be claimed by the dispatch table"
        );
        let output = last_message(&app);
        assert!(
            output.contains("none configured"),
            "with no servers configured, status should say so: {output}"
        );
    });
}

#[test]
fn mcp_status_alias_works() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        assert!(
            crate::tui::app::commands_dispatch::dispatch_local_command(&mut app, "/mcp status"),
            "/mcp status should be claimed by the dispatch table"
        );
        let output = last_message(&app);
        assert!(
            output.contains("none configured"),
            "/mcp status alias should produce the same output as /mcp: {output}"
        );
    });
}

#[test]
fn mcp_enable_unknown_server_reports_error() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        assert!(
            crate::tui::app::commands_dispatch::dispatch_local_command(
                &mut app,
                "/mcp enable nonexistent"
            ),
            "/mcp enable should be claimed by the dispatch table"
        );
        let output = last_message(&app);
        assert!(
            output.contains("not found") || output.contains("not_found"),
            "enabling a nonexistent server should report not found: {output}"
        );
    });
}

#[test]
fn mcp_disable_unknown_server_reports_error() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        assert!(
            crate::tui::app::commands_dispatch::dispatch_local_command(
                &mut app,
                "/mcp disable ghost"
            ),
            "/mcp disable should be claimed by the dispatch table"
        );
        let output = last_message(&app);
        assert!(
            output.contains("not found") || output.contains("not_found"),
            "disabling a nonexistent server should report not found: {output}"
        );
    });
}

/// A server added to `~/.jcode/mcp.json` while the TUI is open must appear in
/// `/mcp status` and be accepted by `/mcp enable` without a restart. The
/// command path reloads the config from disk; the startup snapshot alone
/// would keep the new server missing. (Greptile 4307539757)
#[test]
fn mcp_command_sees_server_added_to_config_while_tui_open() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();

        // /mcp enable spawns its connect work on the tokio runtime, so the
        // whole scenario runs inside one, like the other async App tests.
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Baseline: nothing configured yet.
            assert!(crate::tui::app::commands_dispatch::dispatch_local_command(
                &mut app,
                "/mcp"
            ));
            assert!(
                last_message(&app).contains("none configured"),
                "fresh app should report no servers: {}",
                last_message(&app)
            );

            // Simulate the user editing mcp.json while the TUI is open.
            let home = std::env::var("JCODE_HOME").expect("JCODE_HOME set by helper");
            let config = serde_json::json!({
                "servers": {
                    "added-later": {
                        "command": "jcode-mcp-test-nonexistent-server",
                        "args": [],
                        "env": {},
                        "shared": false,
                        "disabled": true,
                    }
                }
            });
            std::fs::write(
                std::path::Path::new(&home).join("mcp.json"),
                config.to_string(),
            )
            .expect("write mcp.json");

            // /mcp status must list the new server.
            assert!(crate::tui::app::commands_dispatch::dispatch_local_command(
                &mut app,
                "/mcp"
            ));
            let status = last_message(&app);
            assert!(
                status.contains("added-later"),
                "/mcp status must show a server added after startup: {status}"
            );

            // /mcp enable must find it too (past the config lookup), not report
            // "not found". The connect itself is spawned in the background; the
            // synchronous part proves the lookup used the refreshed config.
            assert!(crate::tui::app::commands_dispatch::dispatch_local_command(
                &mut app,
                "/mcp enable added-later"
            ));
            let enable_output = last_message(&app);
            assert!(
                !enable_output.contains("not found"),
                "/mcp enable must accept a server added after startup: {enable_output}"
            );
        });
    });
}
