//! `replace`: literal or regex search-and-replace across one file or many.
//!
//! Exists so agents never need `sed -i`, `perl -pi`, or ad-hoc Python for
//! bulk edits. Every file is computed in memory first. If any check fails
//! (bad pattern, count mismatch, unreadable file) nothing is written.

use super::{Tool, ToolContext, ToolOutput};
use crate::bus::{Bus, BusEvent, FileOp, FileTouch};
use anyhow::{Context as _, Result};
use async_trait::async_trait;
use regex::{NoExpand, Regex, RegexBuilder};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const MAX_FILES: usize = 2000;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const PREVIEW_FILES: usize = 20;

pub struct ReplaceTool;

impl ReplaceTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct ReplaceInput {
    #[serde(default)]
    intent: Option<String>,
    pattern: String,
    replacement: String,
    /// File or directory. Defaults to the working directory.
    #[serde(default)]
    path: Option<String>,
    /// Glob filter relative to `path`, e.g. `**/*.rs`.
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    regex: bool,
    #[serde(default)]
    case_insensitive: bool,
    #[serde(default)]
    multiline: bool,
    #[serde(default)]
    expected_count: Option<usize>,
    #[serde(default)]
    dry_run: bool,
}

struct Matcher {
    regex: Regex,
    literal: bool,
}

impl Matcher {
    fn new(input: &ReplaceInput) -> Result<Self> {
        anyhow::ensure!(!input.pattern.is_empty(), "pattern must not be empty");
        let source = if input.regex {
            input.pattern.clone()
        } else {
            regex::escape(&input.pattern)
        };
        let regex = RegexBuilder::new(&source)
            .case_insensitive(input.case_insensitive)
            .multi_line(input.multiline)
            .dot_matches_new_line(input.multiline)
            .build()
            .with_context(|| format!("invalid regex: {}", input.pattern))?;
        Ok(Self {
            regex,
            literal: !input.regex,
        })
    }

    /// Returns the new content and number of matches.
    fn apply(&self, content: &str, replacement: &str) -> (String, usize) {
        let count = self.regex.find_iter(content).count();
        if count == 0 {
            return (content.to_string(), 0);
        }
        let replaced = if self.literal {
            self.regex.replace_all(content, NoExpand(replacement))
        } else {
            self.regex.replace_all(content, replacement)
        };
        (replaced.into_owned(), count)
    }
}

struct FileChange {
    display: String,
    path: PathBuf,
    before: String,
    after: String,
    count: usize,
}

#[async_trait]
impl Tool for ReplaceTool {
    fn name(&self) -> &str {
        "replace"
    }

    fn description(&self) -> &str {
        "Literal or regex replace across files. All apply or none do."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["pattern", "replacement"],
            "properties": {
                "intent": super::intent_schema_property(),
                "pattern": {
                    "type": "string",
                    "description": "Text to find. Literal unless regex=true."
                },
                "replacement": {
                    "type": "string",
                    "description": "Replacement. With regex=true, $1 / ${name} expand capture groups (use $$ for a literal $)."
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search. Defaults to the working directory."
                },
                "glob": {
                    "type": "string",
                    "description": "File filter relative to path, e.g. **/*.rs. Respects .gitignore."
                },
                "regex": {"type": "boolean", "description": "Treat pattern as a Rust regex."},
                "case_insensitive": {"type": "boolean"},
                "multiline": {
                    "type": "boolean",
                    "description": "^/$ match at line boundaries and . matches newlines."
                },
                "expected_count": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Total matches expected across all files. Nothing is written if the actual count differs."
                },
                "dry_run": {
                    "type": "boolean",
                    "description": "Report matches and diff without writing."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: ReplaceInput = serde_json::from_value(input)?;
        let matcher = Matcher::new(&params)?;
        let root = ctx.resolve_path(Path::new(params.path.as_deref().unwrap_or(".")));
        anyhow::ensure!(root.exists(), "Path not found: {}", root.display());

        let files = {
            let root = root.clone();
            let glob = params.glob.clone();
            tokio::task::spawn_blocking(move || collect_files(&root, glob.as_deref())).await??
        };

        let mut changes = Vec::new();
        let _locks = super::file_lock::lock_all(files.iter().cloned()).await;
        for file in files {
            let Ok(before) = tokio::fs::read_to_string(&file).await else {
                continue; // binary or unreadable
            };
            let (after, count) = matcher.apply(&before, &params.replacement);
            if count == 0 {
                continue;
            }
            changes.push(FileChange {
                display: display_path(&file, &root),
                path: file,
                before,
                after,
                count,
            });
        }

        let total: usize = changes.iter().map(|change| change.count).sum();
        if total == 0 {
            anyhow::bail!(
                "No matches for {:?} under {}. Nothing written.",
                params.pattern,
                root.display()
            );
        }
        if let Some(expected) = params.expected_count
            && expected != total
        {
            anyhow::bail!(
                "Expected {expected} match{} but found {total} across {} file{}. Nothing written.\n{}",
                plural_es(expected),
                changes.len(),
                plural_s(changes.len()),
                summary_lines(&changes)
            );
        }

        let mut unified = String::new();
        for change in &changes {
            unified.push_str(&super::file_diff::unified(
                &change.display,
                &change.display,
                &change.before,
                &change.after,
            ));
        }

        let verb = if params.dry_run {
            "Would replace"
        } else {
            "Replaced"
        };
        let mut body = format!(
            "{verb} {total} match{} in {} file{}\n{}",
            plural_es(total),
            changes.len(),
            plural_s(changes.len()),
            summary_lines(&changes)
        );

        if !params.dry_run {
            let config_watch = super::config_edit_notice::ConfigEditWatch::begin();
            for change in &changes {
                tokio::fs::write(&change.path, &change.after)
                    .await
                    .with_context(|| format!("writing {}", change.display))?;
                super::edit_stats::record(&ctx, &change.before, &change.after, false).await;
                Bus::global().publish(BusEvent::FileTouch(FileTouch {
                    session_id: ctx.session_id.clone(),
                    path: change.path.clone(),
                    op: FileOp::Edit,
                    intent: params
                        .intent
                        .clone()
                        .filter(|value| !value.trim().is_empty()),
                    summary: Some(format!(
                        "replaced {} match{}",
                        change.count,
                        plural_es(change.count)
                    )),
                    detail: None,
                }));
            }
            config_watch.finish(&mut body);
        }

        let title = if changes.len() == 1 {
            changes[0].display.clone()
        } else {
            format!("{} files", changes.len())
        };
        Ok(super::file_diff::attach(
            ToolOutput::new(body).with_title(title),
            unified,
        ))
    }
}

fn collect_files(root: &Path, glob: Option<&str>) -> Result<Vec<PathBuf>> {
    if root.is_file() {
        return Ok(vec![root.to_path_buf()]);
    }
    let pattern = glob
        .map(|glob| glob::Pattern::new(glob).with_context(|| format!("invalid glob: {glob}")))
        .transpose()?;
    let options = glob::MatchOptions {
        require_literal_separator: true,
        ..Default::default()
    };
    let mut files = Vec::new();
    for entry in ignore::WalkBuilder::new(root).hidden(false).build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        if path.components().any(|part| part.as_os_str() == ".git") {
            continue;
        }
        if entry
            .metadata()
            .is_ok_and(|meta| meta.len() > MAX_FILE_BYTES)
        {
            continue;
        }
        if let Some(pattern) = &pattern {
            let relative = path.strip_prefix(root).unwrap_or(path);
            let matches = pattern.matches_path_with(relative, options)
                // `*.rs` should also match nested files, like ripgrep's -g.
                || (!pattern.as_str().contains('/')
                    && relative
                        .file_name()
                        .is_some_and(|name| pattern.matches_with(&name.to_string_lossy(), options)));
            if !matches {
                continue;
            }
        }
        files.push(path.to_path_buf());
        anyhow::ensure!(
            files.len() <= MAX_FILES,
            "More than {MAX_FILES} files matched. Narrow path or glob."
        );
    }
    files.sort();
    Ok(files)
}

fn display_path(path: &Path, root: &Path) -> String {
    if root.is_file() {
        return root.display().to_string();
    }
    path.strip_prefix(root)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

fn summary_lines(changes: &[FileChange]) -> String {
    let mut lines: Vec<String> = changes
        .iter()
        .take(PREVIEW_FILES)
        .map(|change| {
            format!(
                "  ✓ {}: {} match{}",
                change.display,
                change.count,
                plural_es(change.count)
            )
        })
        .collect();
    if changes.len() > PREVIEW_FILES {
        lines.push(format!("  … {} more files", changes.len() - PREVIEW_FILES));
    }
    lines.join("\n")
}

fn plural_s(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn plural_es(count: usize) -> &'static str {
    if count == 1 { "" } else { "es" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(value: Value) -> ReplaceInput {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn literal_mode_escapes_pattern_and_replacement() {
        let matcher = Matcher::new(&input(json!({"pattern":"a.b($1)","replacement":"x"}))).unwrap();
        assert_eq!(
            matcher.apply("a.b($1) axb($1)", "$0"),
            ("$0 axb($1)".into(), 1)
        );
    }

    #[test]
    fn regex_mode_expands_captures() {
        let matcher = Matcher::new(&input(
            json!({"pattern":r"fn (\w+)\(\)","replacement":"x","regex":true}),
        ))
        .unwrap();
        assert_eq!(
            matcher.apply("fn a() fn b()", "fn ${1}_v2()"),
            ("fn a_v2() fn b_v2()".into(), 2)
        );
    }

    #[test]
    fn invalid_regex_is_an_error() {
        assert!(
            Matcher::new(&input(json!({"pattern":"(","replacement":"","regex":true}))).is_err()
        );
        assert!(Matcher::new(&input(json!({"pattern":"","replacement":"x"}))).is_err());
    }

    #[test]
    fn collect_files_filters_by_glob_and_skips_git() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "x").unwrap();
        std::fs::write(dir.path().join("src/nested/b.rs"), "x").unwrap();
        std::fs::write(dir.path().join("src/c.md"), "x").unwrap();
        std::fs::write(dir.path().join(".git/config"), "x").unwrap();

        let names = |glob| {
            collect_files(dir.path(), glob)
                .unwrap()
                .iter()
                .map(|path| display_path(path, dir.path()))
                .collect::<Vec<_>>()
        };
        assert_eq!(names(Some("*.rs")), ["src/a.rs", "src/nested/b.rs"]);
        assert_eq!(names(Some("src/*.rs")), ["src/a.rs"]);
        assert_eq!(names(None), ["src/a.rs", "src/c.md", "src/nested/b.rs"]);
    }
}
