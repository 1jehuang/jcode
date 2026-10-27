//! `/mcp` dialog pages: the server, approval, tools and tool views, built
//! from the daemon's `inspect` JSON.

use super::*;

/// jcode's MCP guide, linked from the server list like Claude Code's docs.
pub(super) const MCP_DOCS_URL: &str =
    "https://github.com/1jehuang/jcode/blob/master/docs/MCP_REMOTE.md";

/// A server's config scope; older daemons only sent `source`.
pub(super) fn scope_of(server: &Value) -> &str {
    match server["scope"].as_str() {
        Some(scope) => scope,
        None => {
            let source = str_of(server, "source");
            if source.starts_with('~') {
                "user"
            } else if source == "added this session" {
                "dynamic"
            } else {
                "project"
            }
        }
    }
}

pub(super) fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}

pub(super) fn tool_count(server: &Value) -> usize {
    server["tools"].as_array().map_or(0, Vec::len)
}

/// List-row icon and facts, as Claude Code's server list shows them.
pub(super) fn server_status(server: &Value) -> (Tone, String) {
    match str_of(server, "status") {
        "connected" => match tool_count(server) {
            0 => (Tone::Warning, "no tools · connected".to_string()),
            n => (
                Tone::Success,
                format!("{n} tool{}", if n == 1 { "" } else { "s" }),
            ),
        },
        // Claude Code shows a bare ✘ for a failed server.
        "failed" => (Tone::Error, String::new()),
        "needs authentication" => (Tone::Warning, "needs authentication".to_string()),
        "disabled" => (Tone::Off, String::new()),
        "needs approval" => (Tone::Warning, "needs approval".to_string()),
        "not connected" => (Tone::Off, "not connected".to_string()),
        other => (Tone::Off, other.to_string()),
    }
}

/// The server page's `Status:` value (Claude Code's `qm`/`Xm`).
pub(super) fn status_text(server: &Value) -> String {
    let (tone, _) = server_status(server);
    let text = match str_of(server, "status") {
        "connected" if tool_count(server) == 0 => "connected · no tools",
        "not connected" => "connecting…",
        other => other,
    };
    format!("{} {text}", tone.glyph())
}

/// Colors a status value written as `<glyph> text`.
pub(super) fn tone_prefix(text: &str) -> Option<Tone> {
    [Tone::Success, Tone::Error, Tone::Warning, Tone::Off]
        .into_iter()
        .find(|tone| text.starts_with(tone.glyph()))
}

pub(super) fn mcp_server_view(server: &Value) -> View {
    let name = str_of(server, "name");
    if str_of(server, "status") == "needs approval" {
        return mcp_approval_view(server);
    }
    // Claude Code capitalizes the first letter of the server name.
    let mut title: String = name.chars().take(1).flat_map(char::to_uppercase).collect();
    title.extend(name.chars().skip(1));
    let mut view = View::new(
        Route::McpServer(name.to_string()),
        format!("{title} MCP Server"),
    );
    view.numbered = true;
    let status = str_of(server, "status");
    let remote = str_of(server, "transport") != "stdio" && !str_of(server, "transport").is_empty();
    view.info.push(("Status".into(), status_text(server)));
    if remote {
        if let Some(error) = server["error"].as_str()
            && matches!(status, "failed" | "needs authentication")
        {
            view.info.push(("Issue".into(), error.to_string()));
        }
        if let Some(auth) = server["auth"].as_str() {
            let glyph = if auth == "authenticated" {
                Tone::Success.glyph()
            } else {
                Tone::Error.glyph()
            };
            view.info.push(("Auth".into(), format!("{glyph} {auth}")));
        }
    }
    let target = str_of(server, "target");
    if !target.is_empty() {
        let label = if remote { "URL" } else { "Command" };
        view.info.push((label.into(), target.to_string()));
    }
    let location = match str_of(server, "config_location") {
        "" => str_of(server, "source"),
        location => location,
    };
    view.info
        .push(("Config location".into(), location.to_string()));
    let tools = tool_count(server);
    if status == "connected" {
        view.inline_info.push((
            "Capabilities".into(),
            if tools > 0 { "tools" } else { "none" }.to_string(),
        ));
        if tools > 0 {
            view.inline_info.push((
                "Tools".into(),
                format!("{tools} tool{}", if tools == 1 { "" } else { "s" }),
            ));
        }
    }
    // A stdio failure's detail goes to Claude Code's debug log; jcode has no
    // such log in the dialog, so show it under the details instead.
    if !remote
        && status == "failed"
        && let Some(error) = server["error"].as_str()
    {
        view.body.push("## Issue:".to_string());
        view.body.push(error.to_string());
    }

    // Claude Code's action list and order.
    let enabled = server["enabled"].as_bool().unwrap_or(true) && status != "disabled";
    let authenticated = server["auth"] == "authenticated";
    if !enabled {
        view.items.push(action_item(
            "Enable",
            ItemAction::Mcp(vec!["enable".into(), name.into()]),
        ));
    }
    if status == "connected" && tools > 0 {
        view.items.push(action_item(
            "View tools",
            ItemAction::Open(Route::McpTools(name.into())),
        ));
    }
    if remote && enabled {
        if authenticated {
            view.items.push(action_item(
                "Re-authenticate",
                ItemAction::Mcp(vec!["authenticate".into(), name.into()]),
            ));
            view.items.push(action_item(
                "Clear authentication",
                ItemAction::Mcp(vec!["clear-auth".into(), name.into()]),
            ));
        } else {
            view.items.push(action_item(
                "Authenticate",
                ItemAction::Mcp(vec!["authenticate".into(), name.into()]),
            ));
        }
    }
    if enabled {
        if status != "needs authentication" {
            view.items.push(action_item(
                "Reconnect",
                ItemAction::Mcp(vec!["reconnect".into(), name.into()]),
            ));
        }
        view.items.push(action_item(
            "Disable",
            ItemAction::Mcp(vec!["disable".into(), name.into()]),
        ));
    }
    // Claude Code starts on the first action that isn't an auth action.
    let auth_actions = ["Re-authenticate", "Clear authentication"];
    if view
        .items
        .first()
        .is_some_and(|item| auth_actions.contains(&item.label.as_str()))
    {
        view.selected = view
            .items
            .iter()
            .position(|item| !auth_actions.contains(&item.label.as_str()))
            .unwrap_or(0);
    }
    view
}

/// Claude Code's prompt for a server a project config file adds: it stays off
/// until approved, and the answer is remembered for this project.
pub(super) fn mcp_approval_view(server: &Value) -> View {
    let name = str_of(server, "name");
    let mut view = View::new(
        Route::McpServer(name.to_string()),
        format!("New MCP server found in this project: {name}"),
    );
    let target = str_of(server, "target");
    if !target.is_empty() {
        let label = if str_of(server, "transport") == "stdio" {
            "Command"
        } else {
            "URL"
        };
        view.info.push((label.into(), target.to_string()));
    }
    view.info.push((
        "Config location".into(),
        str_of(server, "source").to_string(),
    ));
    view.body.push(
        "MCP servers may execute code or access system resources. Only use servers you trust."
            .to_string(),
    );
    for (label, action) in [
        ("Use this MCP server", "approve"),
        (
            "Use this and all future MCP servers in this project",
            "approve-all",
        ),
        ("Continue without using this MCP server", "reject"),
    ] {
        view.items.push(action_item(
            label,
            ItemAction::Mcp(vec![action.into(), name.into()]),
        ));
    }
    // Like Claude Code, the safe answer is the default and Esc means no.
    view.selected = 2;
    view.on_escape = Some(ItemAction::Mcp(vec!["reject".into(), name.into()]));
    view
}

/// First server still waiting for approval, if any.
pub fn first_pending_approval(data: &Value) -> Option<String> {
    data["servers"]
        .as_array()?
        .iter()
        .find(|server| server["status"] == "needs approval")
        .map(|server| str_of(server, "name").to_string())
}

pub(super) fn action_item(label: &str, action: ItemAction) -> Item {
    Item {
        group: None,
        icon: None,
        label: label.to_string(),
        meta: Vec::new(),
        search_text: String::new(),
        action: Some(action),
    }
}

pub(super) fn mcp_tools_view(server: &Value) -> View {
    let name = str_of(server, "name");
    let mut view = View::new(
        Route::McpTools(name.to_string()),
        format!("Tools for {name}"),
    );
    let tools = server["tools"].as_array().map_or(&[][..], Vec::as_slice);
    view.subtitle = format!(
        "{} tool{}",
        tools.len(),
        if tools.len() == 1 { "" } else { "s" }
    );
    view.empty = "No tools available".to_string();
    for tool in tools {
        let tool_name = str_of(tool, "name");
        view.items.push(Item {
            group: None,
            icon: None,
            label: tool_name.to_string(),
            meta: tool_facts(tool),
            search_text: String::new(),
            action: Some(ItemAction::Open(Route::McpTool(
                name.to_string(),
                tool_name.to_string(),
            ))),
        });
    }
    view
}

/// Claude Code's tool facts, in its order, from the MCP annotations.
pub(super) fn tool_facts(tool: &Value) -> Vec<String> {
    [
        ("readOnlyHint", "read-only"),
        ("destructiveHint", "destructive"),
        ("openWorldHint", "open-world"),
    ]
    .into_iter()
    .filter(|(key, _)| tool["annotations"][key].as_bool() == Some(true))
    .map(|(_, label)| label.to_string())
    .collect()
}

pub(super) fn mcp_tool_view(server: &str, tool: &Value) -> View {
    let tool_name = str_of(tool, "name");
    let mut view = View::new(
        Route::McpTool(server.to_string(), tool_name.to_string()),
        tool_name.to_string(),
    );
    // Claude Code tags the title: green read-only, red destructive, dim
    // open-world.
    for fact in tool_facts(tool) {
        let style = match fact.as_str() {
            "read-only" => Style::default().fg(success_color()),
            "destructive" => Style::default().fg(error_color()),
            _ => Style::default().fg(dim_color()),
        };
        view.title_tags
            .push(Span::styled(format!("[{fact}]"), style));
    }
    view.subtitle = server.to_string();
    view.inline_info
        .push(("Tool name".into(), tool_name.to_string()));
    view.inline_info
        .push(("Full name".into(), str_of(tool, "callable").to_string()));
    let description = str_of(tool, "description");
    if !description.is_empty() {
        view.body.push("## Description:".to_string());
        view.body
            .push(truncate_chars(description, TOOL_DESCRIPTION_CHARS));
    }
    let schema = &tool["input_schema"];
    if let Some(properties) = schema["properties"].as_object()
        && !properties.is_empty()
    {
        view.body.push("## Parameters:".to_string());
        let required: Vec<&str> = schema["required"]
            .as_array()
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        for (param, spec) in properties {
            let kind = match &spec["type"] {
                Value::String(kind) => kind.clone(),
                Value::Array(kinds) => kinds
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(","),
                _ => "unknown".to_string(),
            };
            let mut rest = kind;
            if let Some(text) = spec["description"].as_str() {
                rest.push_str(&format!(
                    " - {}",
                    truncate_chars(text, PARAM_DESCRIPTION_CHARS)
                ));
            }
            // Fields are \u{1f}-separated for `param_lines`.
            let required_mark = if required.contains(&param.as_str()) {
                "required\u{1f}"
            } else {
                ""
            };
            view.body
                .push(format!("  ● {param}\u{1f}{required_mark}{rest}"));
        }
    }
    view
}

/// Claude Code caps a tool description at 1000 characters on this page and
/// each parameter description at 200.
const TOOL_DESCRIPTION_CHARS: usize = 1000;
const PARAM_DESCRIPTION_CHARS: usize = 200;
