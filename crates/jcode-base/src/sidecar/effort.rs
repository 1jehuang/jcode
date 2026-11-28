pub(super) fn normalized_memory_effort(raw: Option<String>) -> Option<String> {
    let effort = raw?.trim().to_string();
    (!effort.is_empty()).then_some(effort)
}

pub(super) fn normalized_claude_effort(raw: Option<&str>) -> Option<String> {
    let value = raw?.trim().to_ascii_lowercase();
    let value = crate::prompt::swarm_root_reasoning_effort(&value)
        .unwrap_or(&value)
        .trim()
        .to_ascii_lowercase();
    match value.as_str() {
        "" | "default" | "auto" => None,
        "off" | "disabled" => Some("none".to_string()),
        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => Some(value),
        _ => {
            crate::logging::warn(&format!(
                "Ignoring unsupported Anthropic sidecar reasoning effort '{value}'"
            ));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::normalized_memory_effort;

    #[test]
    fn normalized_memory_effort_only_trims_and_drops_blank() {
        for effort in ["off", "auto", "HiGh", "future-provider-effort-v9"] {
            assert_eq!(
                normalized_memory_effort(Some(format!(" \t{effort}\n "))).as_deref(),
                Some(effort)
            );
        }
        for raw in [None, Some(""), Some(" \t\n")] {
            assert_eq!(normalized_memory_effort(raw.map(str::to_string)), None);
        }
    }
}
