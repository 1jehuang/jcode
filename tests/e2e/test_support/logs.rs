//! Diagnostic log excerpt support for end-to-end failures.

pub(crate) fn latest_log_excerpt(home_dir: &std::path::Path) -> Option<String> {
    let logs_dir = home_dir.join("logs");
    let mut entries = std::fs::read_dir(logs_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entries.sort();
    let latest = entries.pop()?;
    let content = std::fs::read_to_string(latest).ok()?;
    let tail = content
        .lines()
        .rev()
        .take(120)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    Some(tail)
}
