use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const MAX_RESULTS: usize = 100;
const MAX_LINE_LEN: usize = 2000;

pub struct GrepTool;

impl GrepTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct GrepInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    include: Option<String>,
}

#[derive(Clone)]
struct GrepResult {
    file: String,
    line_num: usize,
    line: String,
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search for a regex pattern in files. Returns matching lines with file paths and line numbers. \
         Respects .gitignore and skips binary files."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: current directory)"
                },
                "include": {
                    "type": "string",
                    "description": "File pattern to include (e.g., '*.rs', '*.{ts,tsx}')"
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: GrepInput = serde_json::from_value(input)?;

        let regex_pattern = params.pattern.clone();
        let base_path_str = params.path.clone().unwrap_or_else(|| ".".to_string());
        let base = ctx.resolve_path(Path::new(&base_path_str));
        let include = params.include.clone();

        if !base.exists() {
            return Err(anyhow::anyhow!("Directory not found: {}", base_path_str));
        }

        let results = tokio::task::spawn_blocking(move || {
            grep_blocking(&base, &regex_pattern, include.as_deref())
        })
        .await??;

        let mut output = String::new();
        output.push_str(&format!(
            "Found {} matches for '{}'\n\n",
            results.len(),
            params.pattern
        ));

        let mut current_file = String::new();
        for result in &results {
            if result.file != current_file {
                if !current_file.is_empty() {
                    output.push('\n');
                }
                output.push_str(&format!("{}:\n", result.file));
                current_file = result.file.clone();
            }
            output.push_str(&format!("  {:>4}: {}\n", result.line_num, result.line));
        }

        if results.len() >= MAX_RESULTS {
            output.push_str(&format!(
                "\n... results truncated at {} matches",
                MAX_RESULTS
            ));
        }

        Ok(ToolOutput::new(output))
    }
}

fn grep_blocking(
    base: &Path,
    pattern: &str,
    include: Option<&str>,
) -> Result<Vec<GrepResult>> {
    let regex = Regex::new(pattern)?;
    let include_pattern = include.map(|p| glob::Pattern::new(p)).transpose()?;

    let hit_count = Arc::new(AtomicUsize::new(0));
    let results = Arc::new(std::sync::Mutex::new(Vec::new()));

    let walker = ignore::WalkBuilder::new(base)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .threads(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8))
        .build_parallel();

    let base_owned = base.to_path_buf();

    walker.run(|| {
        let regex = regex.clone();
        let include_pattern = include_pattern.clone();
        let hit_count = hit_count.clone();
        let results = results.clone();
        let base = base_owned.clone();

        Box::new(move |entry| {
            if hit_count.load(Ordering::Relaxed) >= MAX_RESULTS {
                return ignore::WalkState::Quit;
            }

            let entry = match entry {
                Ok(e) => e,
                Err(_) => return ignore::WalkState::Continue,
            };

            let path = entry.path();

            // Use entry.file_type() (cached from readdir, no extra stat)
            let ft = match entry.file_type() {
                Some(ft) => ft,
                None => return ignore::WalkState::Continue,
            };
            if ft.is_dir() {
                return ignore::WalkState::Continue;
            }

            if let Some(ref pattern) = include_pattern {
                let filename = path
                    .file_name()
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default();
                if !pattern.matches(&filename) {
                    return ignore::WalkState::Continue;
                }
            }

            if is_binary_extension(path) {
                return ignore::WalkState::Continue;
            }

            if let Ok(content) = std::fs::read_to_string(path) {
                let mut local_results = Vec::new();
                for (line_num, line) in content.lines().enumerate() {
                    if regex.is_match(line) {
                        let relative = path
                            .strip_prefix(&base)
                            .unwrap_or(path)
                            .display()
                            .to_string();

                        let truncated = if line.len() > MAX_LINE_LEN {
                            format!("{}...", &line[..MAX_LINE_LEN])
                        } else {
                            line.to_string()
                        };

                        local_results.push(GrepResult {
                            file: relative,
                            line_num: line_num + 1,
                            line: truncated,
                        });

                        if hit_count.load(Ordering::Relaxed) + local_results.len() >= MAX_RESULTS {
                            break;
                        }
                    }
                }

                if !local_results.is_empty() {
                    let count = local_results.len();
                    hit_count.fetch_add(count, Ordering::Relaxed);
                    results.lock().unwrap().extend(local_results);
                }
            }

            ignore::WalkState::Continue
        })
    });

    let mut final_results = match Arc::try_unwrap(results) {
        Ok(mutex) => mutex.into_inner().unwrap(),
        Err(arc) => arc.lock().unwrap().clone(),
    };

    // Sort by file then line number for deterministic output
    final_results.sort_by(|a, b| a.file.cmp(&b.file).then(a.line_num.cmp(&b.line_num)));
    final_results.truncate(MAX_RESULTS);

    Ok(final_results)
}

fn is_binary_extension(path: &Path) -> bool {
    if let Some(ext) = path.extension() {
        let ext = ext.to_string_lossy().to_lowercase();
        let binary_exts = [
            "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "pdf", "zip", "tar", "gz", "bz2",
            "xz", "7z", "rar", "exe", "dll", "so", "dylib", "o", "a", "class", "pyc", "wasm",
            "mp3", "mp4", "avi", "mov", "mkv", "flac", "ogg", "wav", "ttf", "woff", "woff2",
        ];
        return binary_exts.contains(&ext.as_str());
    }
    false
}
