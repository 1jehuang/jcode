//! Opt-in host editor routing. Native tools remain unchanged without an executor.
use jcode_message_types::ToolDefinition;

const HOST_EDITORS: &[&str] = &["write", "edit", "apply_patch", "patch", "replace"];

pub(crate) fn host_file_tools(tools: &[ToolDefinition], ready: bool) -> Vec<String> {
    if !ready {
        return Vec::new();
    }
    let mut names: Vec<String> = tools
        .iter()
        .filter(|tool| HOST_EDITORS.contains(&tool.name.as_str()))
        .map(|tool| tool.name.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Sparse registries stay limited rather than falling back to untracked edits.
pub(crate) fn native_denies(host: &[String]) -> Vec<String> {
    if host.is_empty() {
        return Vec::new();
    }
    ["Write", "NotebookEdit", "Edit", "MultiEdit"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Merge custom denies with mandatory replacements into one final option.
/// Repeated CLI options must never overwrite either the user's or our denies.
pub(crate) fn launch_args(args: &[String], host: &[String]) -> Vec<String> {
    let mut denied = native_denies(host);
    if denied.is_empty() {
        return args.to_vec();
    }
    let mut result = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--disallowedTools" || arg == "--disallowed-tools" {
            index += 1;
            while index < args.len() && !args[index].starts_with('-') {
                if !args[index].is_empty() {
                    denied.push(args[index].clone());
                }
                index += 1;
            }
            continue;
        }
        if let Some(value) = arg
            .strip_prefix("--disallowedTools=")
            .or_else(|| arg.strip_prefix("--disallowed-tools="))
        {
            if !value.is_empty() {
                denied.push(value.to_string());
            }
        } else {
            result.push(arg.clone());
        }
        index += 1;
    }
    result.push("--disallowedTools".into());
    result.extend(denied);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mandatory_denies_preserve_custom_rules_and_do_not_touch_standalone() {
        let args = vec![
            "--disallowedTools=Read".into(),
            "--disallowed-tools".into(),
            "Bash(rm *)".into(),
            "--permission-mode=bypassPermissions".into(),
            "--disallowedTools=".into(),
        ];
        assert_eq!(launch_args(&args, &[]), args);
        let routed = launch_args(&args, &["write".into(), "edit".into()]);
        assert_eq!(
            routed,
            vec![
                "--permission-mode=bypassPermissions",
                "--disallowedTools",
                "Write",
                "NotebookEdit",
                "Edit",
                "MultiEdit",
                "Read",
                "Bash(rm *)"
            ]
        );
        assert_eq!(
            native_denies(&["apply_patch".into()]),
            vec!["Write", "NotebookEdit", "Edit", "MultiEdit"]
        );
        assert_eq!(
            native_denies(&["write".into()]),
            native_denies(&["edit".into()])
        );
    }
}
