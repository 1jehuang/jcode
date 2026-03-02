use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

const MAX_ENTRIES: usize = 100;
const DEFAULT_IGNORE: &[&str] = &[
    "node_modules",
    "__pycache__",
    ".git",
    "dist",
    "build",
    "target",
    ".next",
    ".nuxt",
    "venv",
    ".venv",
    "coverage",
    ".cache",
];

pub struct LsTool;

impl LsTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct LsInput {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    ignore: Option<Vec<String>>,
}

struct DirEntry {
    name: String,
    is_dir: bool,
    depth: usize,
}

#[async_trait]
impl Tool for LsTool {
    fn name(&self) -> &str {
        "ls"
    }

    fn description(&self) -> &str {
        "List directory contents in a tree format. Automatically ignores common build directories \
         like node_modules, __pycache__, .git, dist, target, etc."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory to list (default: current directory)"
                },
                "ignore": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Additional patterns to ignore"
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: LsInput = serde_json::from_value(input)?;

        let base_path = params.path.clone().unwrap_or_else(|| ".".to_string());
        let base = ctx.resolve_path(Path::new(&base_path));
        let ignore_extra = params.ignore.clone();

        if !base.exists() {
            return Err(anyhow::anyhow!("Directory not found: {}", base_path));
        }

        if !base.is_dir() {
            return Err(anyhow::anyhow!("Not a directory: {}", base_path));
        }

        let entries = tokio::task::spawn_blocking(move || {
            let mut ignore_patterns: Vec<String> =
                DEFAULT_IGNORE.iter().map(|s| s.to_string()).collect();
            if let Some(extra) = ignore_extra {
                ignore_patterns.extend(extra);
            }

            let mut entries: Vec<DirEntry> = Vec::new();
            collect_entries(&base, 0, &ignore_patterns, &mut entries, MAX_ENTRIES)?;
            Ok::<_, anyhow::Error>(entries)
        })
        .await??;

        let truncated = entries.len() >= MAX_ENTRIES;

        let mut output = String::new();
        output.push_str(&format!("{}/\n", base_path));

        for entry in &entries {
            let indent = "  ".repeat(entry.depth);
            let suffix = if entry.is_dir { "/" } else { "" };
            output.push_str(&format!("{}{}{}\n", indent, entry.name, suffix));
        }

        if truncated {
            output.push_str(&format!("\n... truncated at {} entries", MAX_ENTRIES));
        }

        let file_count = entries.iter().filter(|e| !e.is_dir).count();
        let dir_count = entries.iter().filter(|e| e.is_dir).count();
        output.push_str(&format!(
            "\n{} files, {} directories",
            file_count, dir_count
        ));

        Ok(ToolOutput::new(output))
    }
}

fn collect_entries(
    dir: &Path,
    depth: usize,
    ignore: &[String],
    entries: &mut Vec<DirEntry>,
    max: usize,
) -> Result<()> {
    if entries.len() >= max {
        return Ok(());
    }

    let mut items: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();

    // Cache file_type from DirEntry (uses cached readdir data, no extra stat on most platforms)
    // Then sort using cached values instead of calling is_dir() in the comparator
    let mut typed_items: Vec<(std::fs::DirEntry, bool)> = items
        .drain(..)
        .map(|e| {
            let is_dir = e.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            (e, is_dir)
        })
        .collect();

    typed_items.sort_by(|(a, a_dir), (b, b_dir)| {
        match (*a_dir, *b_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.file_name().cmp(&b.file_name()),
        }
    });

    for (item, is_dir) in typed_items {
        if entries.len() >= max {
            break;
        }

        let name = item.file_name().to_string_lossy().to_string();

        if ignore.iter().any(|p| {
            glob::Pattern::new(p)
                .map(|pat| pat.matches(&name))
                .unwrap_or(false)
                || name == *p
        }) {
            continue;
        }

        if name.starts_with('.') && name != "." && name != ".." {
            continue;
        }

        entries.push(DirEntry {
            name: name.clone(),
            is_dir,
            depth: depth + 1,
        });

        if is_dir && depth < 5 {
            let path = item.path();
            collect_entries(&path, depth + 1, ignore, entries, max)?;
        }
    }

    Ok(())
}
