//! Path attribution for tool calls and patch headers.

use super::*;

impl Registry {
    /// Paths a tool call names in its input, labelled by the input key they
    /// came from, resolved against the session working directory. Covers the
    /// common path keys, `paths` arrays (first 8), and `apply_patch` file
    /// headers (Codex envelope and unified diff).
    pub(super) fn touched_paths(
        input: &Value,
        ctx: &ToolContext,
    ) -> Vec<(&'static str, std::path::PathBuf)> {
        let mut touched = Vec::new();
        let Some(object) = input.as_object() else {
            return touched;
        };
        let resolve = |path: &str| ctx.resolve_path(std::path::Path::new(path));
        for key in [
            "file_path",
            "path",
            "target",
            "target_path",
            "old_path",
            "new_path",
        ] {
            if let Some(path) = object.get(key).and_then(Value::as_str) {
                touched.push((key, resolve(path)));
            }
        }
        if let Some(paths) = object.get("paths").and_then(Value::as_array) {
            for path in paths.iter().filter_map(Value::as_str).take(8) {
                touched.push(("paths", resolve(path)));
            }
        }
        if let Some(patch) = object.get("patch_text").and_then(Value::as_str) {
            for path in Self::patch_paths(patch) {
                if !touched
                    .iter()
                    .any(|(_, existing)| *existing == resolve(path))
                {
                    touched.push(("patch", resolve(path)));
                }
            }
        }
        touched
    }

    /// File paths named in patch headers. Deletions are included because the
    /// path was still touched; `/dev/null` sides of unified diffs are not.
    pub(super) fn patch_paths(patch: &str) -> Vec<&str> {
        let mut paths = Vec::new();
        for line in patch.lines() {
            let path = [
                "*** Add File: ",
                "*** Update File: ",
                "*** Delete File: ",
                "*** Move to: ",
            ]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
            .or_else(|| {
                let header = line
                    .strip_prefix("+++ ")
                    .or_else(|| line.strip_prefix("--- "))?;
                let header = header.split('\t').next().unwrap_or(header).trim();
                if header == "/dev/null" {
                    return None;
                }
                Some(
                    header
                        .strip_prefix("a/")
                        .or_else(|| header.strip_prefix("b/"))
                        .unwrap_or(header),
                )
            });
            if let Some(path) = path.map(str::trim).filter(|path| !path.is_empty())
                && !paths.contains(&path)
            {
                paths.push(path);
            }
        }
        paths
    }
}
