use super::*;
use ratatui::backend::TestBackend;
use serde_json::json;

fn mcp_data() -> Value {
    json!({"servers": [
        {"name": "github", "status": "connected", "enabled": true, "transport": "stdio",
         "target": "npx -y gh-mcp", "source": "~/.jcode/mcp.json", "scope": "user",
         "config_location": "/home/u/.jcode/mcp.json",
         "tools": [{"name": "create_issue", "callable": "mcp__github__create_issue",
                    "description": "Create an issue",
                    "annotations": {"destructiveHint": true, "openWorldHint": true, "readOnlyHint": false},
                    "input_schema": {"type": "object", "required": ["title"],
                        "properties": {"title": {"type": "string", "description": "Title"}}}}]},
        {"name": "broken", "status": "failed", "enabled": true, "transport": "http",
         "target": "https://example.com/mcp", "source": ".mcp.json", "scope": "project",
         "config_location": "/repo/.mcp.json", "auth": "not authenticated",
         "error": "500 Internal Server Error", "tools": []}
    ]})
}

fn press(dialog: &mut Dialog, code: KeyCode) -> DialogKey {
    dialog.handle_key(code, KeyModifiers::empty())
}

/// Render into a terminal of `width` x `height` and return trimmed rows.
fn screen(dialog: &Dialog, width: u16, height: u16) -> Vec<String> {
    let mut terminal = ratatui::Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| dialog.render(frame, frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

#[test]
fn server_list_matches_claude_code_layout() {
    let mut dialog = Dialog::mcp_loading();
    dialog.set_mcp(mcp_data());
    let rows = screen(&dialog, 80, dialog.desired_height(80));
    assert!(rows[0].chars().all(|c| c == '▔'), "accent rule first");
    assert_eq!(
        rows[1..].to_vec(),
        [
            "   Manage MCP servers",
            "   2 servers",
            "",
            "     Project MCPs (/repo/.mcp.json)",
            "   ❯ ✘ broken",
            "",
            "     User MCPs (/home/u/.jcode/mcp.json)",
            "     ✔ github   1 tool",
            "",
            "   ※ Run jcode --trace to see error logs",
            "   https://github.com/1jehuang/jcode/blob/master/docs/MCP_REMOTE.md for help",
            "",
            "   ↑/↓ to navigate · Enter to confirm · Esc to cancel",
        ]
    );
}

#[test]
fn stdio_server_page_matches_claude_code() {
    let mut dialog = Dialog::mcp_loading();
    dialog.set_mcp(mcp_data());
    press(&mut dialog, KeyCode::Down);
    press(&mut dialog, KeyCode::Enter);
    let rows = screen(&dialog, 80, dialog.desired_height(80));
    assert_eq!(
        rows[1..].to_vec(),
        [
            "   Github MCP Server",
            "",
            "   Status:           ✔ connected",
            "   Command:          npx -y gh-mcp",
            "   Config location:  /home/u/.jcode/mcp.json",
            "   Capabilities: tools",
            "   Tools: 1 tool",
            "",
            "   ❯ 1. View tools",
            "     2. Reconnect",
            "     3. Disable",
            "",
            "   ↑/↓ to navigate · Enter to select · Esc to back",
        ]
    );
}

#[test]
fn remote_failed_server_offers_authenticate_and_issue_row() {
    let mut dialog = Dialog::mcp_loading();
    dialog.set_mcp(mcp_data());
    press(&mut dialog, KeyCode::Enter);
    let rows = screen(&dialog, 80, dialog.desired_height(80));
    assert_eq!(
        rows[1..9].to_vec(),
        [
            "   Broken MCP Server",
            "",
            "   Status:           ✘ failed",
            "   Issue:            500 Internal Server Error",
            "   Auth:             ✘ not authenticated",
            "   URL:              https://example.com/mcp",
            "   Config location:  /repo/.mcp.json",
            "",
        ]
    );
    assert_eq!(
        dialog.item_summaries(),
        ["Authenticate", "Reconnect", "Disable"]
    );
    assert_eq!(
        press(&mut dialog, KeyCode::Enter),
        DialogKey::Mcp(vec!["authenticate".into(), "broken".into()])
    );
    // A second press while the first is in flight is ignored.
    assert_eq!(press(&mut dialog, KeyCode::Enter), DialogKey::Handled);
    // The open server vanished on refresh, so only the list remains.
    let mut data = mcp_data();
    data["servers"].as_array_mut().unwrap().remove(1);
    dialog.set_mcp(data);
    assert_eq!(dialog.stack.len(), 1);
}

#[test]
fn needs_auth_and_disabled_servers_get_claude_code_actions() {
    let data = json!({"servers": [
        {"name": "drive", "status": "needs authentication", "enabled": true,
         "transport": "http", "target": "https://drive.example", "scope": "user",
         "config_location": "/home/u/.claude.json", "auth": "not authenticated", "tools": []},
        {"name": "off", "status": "disabled", "enabled": false, "transport": "stdio",
         "target": "srv", "scope": "user", "config_location": "/home/u/.claude.json", "tools": []}
    ]});
    let mut dialog = Dialog::mcp_loading();
    dialog.set_mcp(data);
    assert_eq!(
        dialog.item_summaries(),
        ["drive · needs authentication", "off"]
    );
    press(&mut dialog, KeyCode::Enter);
    assert_eq!(dialog.item_summaries(), ["Authenticate", "Disable"]);
    press(&mut dialog, KeyCode::Esc);
    press(&mut dialog, KeyCode::Down);
    press(&mut dialog, KeyCode::Enter);
    assert_eq!(dialog.item_summaries(), ["Enable"]);
    let rows = screen(&dialog, 60, dialog.desired_height(60));
    assert_eq!(rows[3], "   Status:           ◯ disabled");
}

#[test]
fn tools_list_and_tool_page_match_claude_code() {
    let mut dialog = Dialog::mcp_loading();
    dialog.set_mcp(mcp_data());
    press(&mut dialog, KeyCode::Down);
    press(&mut dialog, KeyCode::Enter);
    press(&mut dialog, KeyCode::Enter);
    let rows = screen(&dialog, 80, dialog.desired_height(80));
    assert_eq!(
        rows[1..].to_vec(),
        [
            "   Tools for github",
            "   1 tool",
            "",
            "   ❯ create_issue   destructive · open-world",
            "",
            "   ↑/↓ to navigate · Enter to select · Esc to back",
        ]
    );
    press(&mut dialog, KeyCode::Enter);
    let rows = screen(&dialog, 80, dialog.desired_height(80));
    assert_eq!(
        rows[1..].to_vec(),
        [
            "   create_issue [destructive] [open-world]",
            "   github",
            "",
            "   Tool name: create_issue",
            "   Full name: mcp__github__create_issue",
            "",
            "   Description:",
            "   Create an issue",
            "",
            "   Parameters:",
            "     ● title (required): string - Title",
            "",
            "   Esc to go back",
        ]
    );
    for _ in 0..3 {
        assert_eq!(press(&mut dialog, KeyCode::Esc), DialogKey::Handled);
    }
    assert_eq!(press(&mut dialog, KeyCode::Esc), DialogKey::Close);
}

#[test]
fn long_list_scrolls_with_more_markers() {
    let tools: Vec<Value> = (0..40)
        .map(|i| json!({"name": format!("tool_{i:02}"), "callable": "x", "input_schema": {}}))
        .collect();
    let data = json!({"servers": [{"name": "big", "status": "connected", "enabled": true,
        "transport": "stdio", "target": "big", "scope": "user",
        "config_location": "/c", "tools": tools}]});
    let mut dialog = Dialog::mcp_loading();
    dialog.set_mcp(data);
    press(&mut dialog, KeyCode::Enter);
    press(&mut dialog, KeyCode::Enter);
    let rows = screen(&dialog, 60, 16);
    assert!(rows.iter().any(|r| r.trim() == "↓ 31 more"), "{rows:#?}");
    for _ in 0..20 {
        press(&mut dialog, KeyCode::Down);
    }
    let rows = screen(&dialog, 60, 16);
    assert!(rows.iter().any(|r| r.contains("❯ tool_20")), "{rows:#?}");
    assert!(rows.iter().any(|r| r.trim().starts_with("↑ ")), "{rows:#?}");
    assert!(rows.iter().any(|r| r.trim().starts_with("↓ ")), "{rows:#?}");
    // Up from the top wraps to the bottom, like Claude Code.
    press(&mut dialog, KeyCode::Home);
    press(&mut dialog, KeyCode::Up);
    let rows = screen(&dialog, 60, 16);
    assert!(rows.iter().any(|r| r.contains("❯ tool_39")), "{rows:#?}");
}

#[test]
fn escape_back_to_list_resets_cursor() {
    let mut dialog = Dialog::mcp_loading();
    dialog.set_mcp(mcp_data());
    press(&mut dialog, KeyCode::Down);
    press(&mut dialog, KeyCode::Enter);
    press(&mut dialog, KeyCode::Esc);
    assert_eq!(dialog.stack.last().unwrap().selected, 0);
}

#[test]
fn project_server_approval_prompt_matches_claude_code() {
    let data = json!({"servers": [{
        "name": "repo-tool", "status": "needs approval", "enabled": false,
        "transport": "stdio", "target": "node server.js", "source": ".mcp.json", "tools": []
    }]});
    assert_eq!(first_pending_approval(&data).as_deref(), Some("repo-tool"));
    let mut dialog = Dialog::mcp_at_server(data, "repo-tool");
    let prompt = dialog.stack.last().unwrap();
    assert_eq!(
        prompt.title,
        "New MCP server found in this project: repo-tool"
    );
    let labels: Vec<_> = prompt.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Use this MCP server",
            "Use this and all future MCP servers in this project",
            "Continue without using this MCP server",
        ]
    );
    assert_eq!(prompt.selected, 2, "the safe answer is the default");
    assert!(!prompt.numbered);
    assert_eq!(
        press(&mut dialog, KeyCode::Esc),
        DialogKey::Mcp(vec!["reject".into(), "repo-tool".into()])
    );
    dialog.pending_mcp = None;
    press(&mut dialog, KeyCode::Up);
    press(&mut dialog, KeyCode::Up);
    assert_eq!(
        press(&mut dialog, KeyCode::Enter),
        DialogKey::Mcp(vec!["approve".into(), "repo-tool".into()])
    );
    assert_eq!(dialog.stack[0].items[0].meta, vec!["needs approval"]);
}

#[test]
fn digit_runs_numbered_server_action() {
    let mut dialog = Dialog::mcp_loading();
    dialog.set_mcp(mcp_data());
    press(&mut dialog, KeyCode::Enter);
    // "broken" offers 1. Authenticate, 2. Reconnect, 3. Disable.
    assert_eq!(
        press(&mut dialog, KeyCode::Char('3')),
        DialogKey::Mcp(vec!["disable".into(), "broken".into()])
    );
}

#[test]
fn skills_filter_cycle_and_sort() {
    let rows = vec![
        SkillRow {
            name: "deploy".into(),
            description: "Ship it".into(),
            source: "project".into(),
            author_user_only: false,
        },
        SkillRow {
            name: "review".into(),
            description: "A much longer description that costs more listing tokens than deploy does, by a wide margin".into(),
            source: "user".into(),
            author_user_only: false,
        },
        SkillRow {
            name: "secret".into(),
            description: "Author-locked".into(),
            source: "user".into(),
            author_user_only: true,
        },
        SkillRow {
            name: "tools:lint".into(),
            description: "From a plugin".into(),
            source: "plugin".into(),
            author_user_only: false,
        },
    ];
    let mut dialog = Dialog::skills(rows, SkillOverrides::new());
    for ch in "rev".chars() {
        press(&mut dialog, KeyCode::Char(ch));
    }
    let view = dialog.stack.last().unwrap();
    assert_eq!(view.visible().len(), 1);
    press(&mut dialog, KeyCode::Char(' '));
    press(&mut dialog, KeyCode::Enter);
    let current = &dialog.skills.as_ref().unwrap().current;
    assert_eq!(current.get("review"), Some(&SkillMode::UserInvocableOnly));

    assert_eq!(press(&mut dialog, KeyCode::Esc), DialogKey::Handled);
    press(&mut dialog, KeyCode::Char('t'));
    let view = dialog.stack.last().unwrap();
    assert_eq!(view.items[0].label, "review");
    let secret = view.items.iter().find(|i| i.label == "secret").unwrap();
    assert!(secret.action.is_none());
    assert_eq!(secret.meta[0], "locked by author");
    let plugin = view.items.iter().find(|i| i.label == "tools:lint").unwrap();
    assert!(
        plugin.action.is_none(),
        "plugin skills are managed via /plugin"
    );
    assert_eq!(plugin.meta[0], "locked by plugin");
}

#[test]
fn skills_dialog_matches_claude_code_layout_and_searches_descriptions() {
    let rows = vec![
        SkillRow {
            name: "amq-cli".into(),
            description: "Coordinate agents over a message queue".into(),
            source: "user".into(),
            author_user_only: false,
        },
        SkillRow {
            name: "ponytail:ponytail".into(),
            description: "Lazy senior dev mode".into(),
            source: "plugin".into(),
            author_user_only: false,
        },
    ];
    let mut dialog = Dialog::skills(rows, SkillOverrides::new());
    let screen_rows = screen(&dialog, 90, dialog.desired_height(90));
    assert_eq!(
        screen_rows[1..].to_vec(),
        [
            "   Skills",
            "   2 skills · enter/space to cycle, / to search, t to sort, Esc to close",
            "   ╭────────────────────────────────────────────────────────────────────────────────────╮",
            "   │ ⌕ Search skills…                                                                   │",
            "   ╰────────────────────────────────────────────────────────────────────────────────────╯",
            "   ❯ ✔ ponytail:ponytail   locked by plugin · plugin · < 20 tok",
            "     ✔ amq-cli             user · < 20 tok",
            "",
            "   Plugin skills are managed via Claude Code's /plugin",
        ]
    );
    // Claude Code's search also matches descriptions.
    for ch in "queue".chars() {
        press(&mut dialog, KeyCode::Char(ch));
    }
    assert_eq!(dialog.stack.last().unwrap().visible().len(), 1);
    let screen_rows = screen(&dialog, 90, dialog.desired_height(90));
    assert!(
        screen_rows[2].starts_with("   1/2 skills"),
        "{screen_rows:#?}"
    );
    assert!(screen_rows[6].contains("amq-cli"), "{screen_rows:#?}");
}

#[test]
fn wrap_respects_width_and_indent() {
    assert_eq!(wrap("aa bb cc", 5), ["aa bb", "cc"]);
    assert_eq!(wrap("  • x yy", 6), ["  • x", "  yy"]);
    assert_eq!(wrap("abcdef", 3), ["abc", "def"]);
    assert_eq!(format_tokens(7), "< 20 tok");
    assert_eq!(format_tokens(134), "~130 tok");
}
