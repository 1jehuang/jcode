pub(super) fn runtime_activity_status_notice(message: &str) -> String {
    message
        .lines()
        .find_map(|line| {
            let line = line.trim();
            (!line.is_empty()).then_some(line)
        })
        .unwrap_or("Jcode activity")
        .trim_matches('*')
        .trim()
        .trim_end_matches('.')
        .to_string()
}
