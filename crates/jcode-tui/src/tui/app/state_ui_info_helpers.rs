fn handle_skills_or_mcp_info_command(app: &mut App, trimmed: &str) -> bool {
    if trimmed == "/skills" {
        // Sync from disk first so skills added by agent-side `skill_manage
        // reload_all` (which only updates the server process registry) show up
        // without a restart (issue #431).
        app.refresh_skills_snapshot();
        app.push_display_message(
            DisplayMessage::system(build_skills_report(app)).with_title("Skills"),
        );
        app.set_status_notice("Skills");
        return true;
    }

    if trimmed != "/mcp" {
        return false;
    }
    let servers = app.mcp_servers();
    if servers.is_empty() {
        app.push_display_message(
            DisplayMessage::system("No MCP servers are currently connected.\n\nAdd MCP servers in ~/.jcode/mcp.json or the workspace .jcode/mcp.json, then use /server-reload to apply.").with_title("MCP Servers"),
        );
    } else {
        let mut report = String::from("Connected MCP servers\n\n");
        for (name, tool_count) in servers {
            report.push_str(&format!(
                "- **{}**: {} tool{}\n",
                name,
                tool_count,
                if tool_count == 1 { "" } else { "s" }
            ));
        }
        app.push_display_message(DisplayMessage::system(report).with_title("MCP Servers"));
    }
    app.set_status_notice("MCP servers");
    true
}
