pub(super) fn is_test_harness_path(path: &str) -> bool {
    let path = path.replace('\\', "/");
    // Preserve detection for the default target tree, including custom runners.
    if path.contains("/target/") && path.contains("/deps/") {
        return true;
    }
    let mut components = path.rsplit('/');
    let Some(filename) = components.next() else {
        return false;
    };
    if components.next() != Some("deps") {
        return false;
    }
    let stem = filename.strip_suffix(".exe").unwrap_or(filename);
    let Some((name, hash)) = stem.rsplit_once('-') else {
        return false;
    };
    !name.is_empty() && hash.len() == 16 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}
