//! SSH host command routes. This module deliberately contains no local file/editor calls.
pub(super) fn command(input: &str) -> Option<(&'static str, Vec<String>)> {
    let mut words = input.split_whitespace();
    let token = words.next()?;
    let command = match token {
        "/config" => "config",
        "/swarm-prompt" => "swarm_prompt",
        "/log" => "log",
        "/open" => "open",
        "/file" => "file",
        "/reload" | "/restart" => "server_reload",
        "/rebuild" => "rebuild",
        "/update" => "update",
        "/selfdev" | "/self-dev" => "selfdev",
        "/fast" if words.next()? == "default" => "fast_default",
        _ => return None,
    };
    Some((command, words.map(str::to_string).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_commands_never_select_local_handlers() {
        for (input, expected, args) in [
            (
                "/config set display.centered true",
                "config",
                vec!["set", "display.centered", "true"],
            ),
            (
                "/swarm-prompt set project use tools",
                "swarm_prompt",
                vec!["set", "project", "use", "tools"],
            ),
            ("/fast default off", "fast_default", vec!["off"]),
            (
                "/open path with spaces",
                "open",
                vec!["path", "with", "spaces"],
            ),
            ("/file README.md", "file", vec!["README.md"]),
            ("/restart", "server_reload", vec![]),
            ("/reload", "server_reload", vec![]),
            ("/rebuild", "rebuild", vec![]),
            ("/update", "update", vec![]),
            ("/selfdev status", "selfdev", vec!["status"]),
            ("/log mark debug note", "log", vec!["mark", "debug", "note"]),
        ] {
            let (actual, actual_args) = command(input).unwrap();
            assert_eq!(actual, expected, "{input}");
            assert_eq!(actual_args, args, "{input}");
        }
        for client_command in [
            "/client-reload",
            "/fast",
            "/fast on",
            "/fast status",
            "/configuration",
        ] {
            assert!(command(client_command).is_none(), "{client_command}");
        }
    }
}
