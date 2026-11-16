//! Patch summaries and primary-file identification.

pub(crate) fn summarize_unified_patch_input(patch_text: &str) -> String {
    let lines = patch_text.lines().count();
    let mut files: Vec<String> = Vec::new();

    for line in patch_text.lines() {
        let Some(rest) = line
            .strip_prefix("--- ")
            .or_else(|| line.strip_prefix("+++ "))
        else {
            continue;
        };

        let without_tab_suffix = rest.split('\t').next().unwrap_or(rest);
        let path_token = without_tab_suffix.split_whitespace().next().unwrap_or("");
        let path = path_token
            .strip_prefix("a/")
            .or(path_token.strip_prefix("b/"))
            .unwrap_or(path_token);

        if path.is_empty() || path == "/dev/null" {
            continue;
        }
        if !files.iter().any(|f| f == path) {
            files.push(path.to_string());
        }
    }

    if files.len() == 1 {
        format!("{} ({} lines)", files[0], lines)
    } else if !files.is_empty() {
        format!("{} files ({} lines)", files.len(), lines)
    } else {
        format!("({} lines)", lines)
    }
}

pub(crate) fn summarize_apply_patch_input(patch_text: &str) -> String {
    let lines = patch_text.lines().count();
    let mut files: Vec<String> = Vec::new();

    for line in patch_text.lines() {
        let trimmed = line.trim();
        let path = trimmed
            .strip_prefix("*** Add File: ")
            .or_else(|| trimmed.strip_prefix("*** Update File: "))
            .or_else(|| trimmed.strip_prefix("*** Delete File: "))
            .map(str::trim)
            .unwrap_or("");

        if path.is_empty() {
            continue;
        }
        if !files.iter().any(|f| f == path) {
            files.push(path.to_string());
        }
    }

    if files.len() == 1 {
        format!("{} ({} lines)", files[0], lines)
    } else if !files.is_empty() {
        format!("{} files ({} lines)", files.len(), lines)
    } else {
        format!("({} lines)", lines)
    }
}

pub(crate) fn extract_apply_patch_primary_file(patch_text: &str) -> Option<String> {
    for line in patch_text.lines() {
        let trimmed = line.trim();
        let path = trimmed
            .strip_prefix("*** Add File: ")
            .or_else(|| trimmed.strip_prefix("*** Update File: "))
            .or_else(|| trimmed.strip_prefix("*** Delete File: "))
            .map(str::trim)
            .unwrap_or("");

        if !path.is_empty() {
            return Some(path.to_string());
        }
    }

    None
}

pub(crate) fn extract_unified_patch_primary_file(patch_text: &str) -> Option<String> {
    for line in patch_text.lines() {
        let Some(rest) = line
            .strip_prefix("+++ ")
            .or_else(|| line.strip_prefix("--- "))
        else {
            continue;
        };

        let without_tab_suffix = rest.split('\t').next().unwrap_or(rest);
        let path_token = without_tab_suffix.split_whitespace().next().unwrap_or("");
        let path = path_token
            .strip_prefix("a/")
            .or(path_token.strip_prefix("b/"))
            .unwrap_or(path_token);

        if !path.is_empty() && path != "/dev/null" {
            return Some(path.to_string());
        }
    }

    None
}
