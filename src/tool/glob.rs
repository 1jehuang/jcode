use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

const MAX_RESULTS: usize = 100;

pub struct GlobTool;

impl GlobTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct GlobInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern. Supports patterns like '**/*.rs', 'src/**/*.ts', etc. \
         Returns files sorted by modification time (newest first)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern to match (e.g., '**/*.rs', 'src/**/*.ts')"
                },
                "path": {
                    "type": "string",
                    "description": "Base directory to search in (default: current directory)"
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: GlobInput = serde_json::from_value(input)?;

        let base_path = params.path.clone().unwrap_or_else(|| ".".to_string());
        let base = ctx.resolve_path(Path::new(&base_path));
        let pattern = params.pattern.clone();

        if !base.exists() {
            return Err(anyhow::anyhow!("Directory not found: {}", base_path));
        }

        let results = tokio::task::spawn_blocking(move || {
            glob_blocking(&base, &pattern)
        })
        .await??;

        let mut output = String::new();
        output.push_str(&format!(
            "Found {} files matching '{}' in {}\n\n",
            results.len(),
            params.pattern,
            base_path
        ));

        let truncated = results.len() >= MAX_RESULTS;

        for (path, _) in &results {
            output.push_str(path);
            output.push('\n');
        }

        if truncated {
            output.push_str(&format!(
                "\n... results truncated (showing {} of more)",
                MAX_RESULTS
            ));
        }

        Ok(ToolOutput::new(output))
    }
}

fn glob_blocking(
    base: &Path,
    pattern: &str,
) -> Result<Vec<(String, std::time::SystemTime)>> {
    let glob_pattern = glob::Pattern::new(pattern)?;

    let collect_limit = MAX_RESULTS * 2;
    let results = Arc::new(std::sync::Mutex::new(Vec::with_capacity(MAX_RESULTS)));
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let walker = ignore::WalkBuilder::new(base)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .threads(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8))
        .build_parallel();

    let base_owned = base.to_path_buf();

    walker.run(|| {
        let glob_pattern = glob_pattern.clone();
        let results = results.clone();
        let count = count.clone();
        let base = base_owned.clone();

        Box::new(move |entry| {
            if count.load(std::sync::atomic::Ordering::Relaxed) >= collect_limit {
                return ignore::WalkState::Quit;
            }

            let entry = match entry {
                Ok(e) => e,
                Err(_) => return ignore::WalkState::Continue,
            };

            // Use cached file_type from readdir (no extra stat)
            let ft = match entry.file_type() {
                Some(ft) => ft,
                None => return ignore::WalkState::Continue,
            };
            if ft.is_dir() {
                return ignore::WalkState::Continue;
            }

            let path = entry.path();
            let relative = path.strip_prefix(&base).unwrap_or(path);
            let path_str = relative.to_string_lossy();

            if glob_pattern.matches(&path_str) || glob_pattern.matches_path(relative) {
                // Only call metadata when we have a match (expensive on Windows)
                let mtime = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or(std::time::UNIX_EPOCH);

                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                results
                    .lock()
                    .unwrap()
                    .push((path_str.to_string(), mtime));
            }

            ignore::WalkState::Continue
        })
    });

    let mut final_results = match Arc::try_unwrap(results) {
        Ok(mutex) => mutex.into_inner().unwrap(),
        Err(arc) => arc.lock().unwrap().clone(),
    };

    // Sort by modification time (newest first)
    final_results.sort_by(|a, b| b.1.cmp(&a.1));
    final_results.truncate(MAX_RESULTS);

    Ok(final_results)
}
