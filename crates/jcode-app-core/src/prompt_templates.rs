//! Prompt-template discovery for `jcode prompts list|show` (issue #4 MVP).
//!
//! A prompt template is a Markdown file that the user can later expand into
//! the editor via `/<name>`. The interactive expansion + autocomplete +
//! front-matter / arg substitution pieces are tracked separately; this module
//! is the discovery + display half so users can drop a template into the
//! documented directories and confirm jcode sees it before any UI work.
//!
//! Discovery order (project beats global on collision):
//!
//!   1. `<cwd>/.jcode/prompts/*.md` walking up the ancestor chain (closest
//!      to cwd wins for a given name).
//!   2. `~/.jcode/prompts/*.md` (user-global).
//!
//! Filenames without the `.md` extension become the command name. Names are
//! validated as kebab-case-friendly (ASCII alphanumeric + `-`/`_`); files
//! with other characters are reported as `invalid_name` and skipped.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

// ---- Issue #4: prompt template hot reload ----
//
// `discover()` already re-reads from disk on every call (so adding a
// new .md file is picked up immediately). What was missing:
//   1. A cheap "did anything change?" probe the TUI can poll without
//      paying the full directory walk
//   2. A cache that returns the prior result when nothing changed
//
// `discover_cached()` provides both. It records the maximum mtime
// across all prompt directories on each scan; subsequent calls are
// no-ops when no mtime advanced. The TUI can call
// `prompt_templates_changed_since(token)` to test for changes
// without paying for the templates themselves.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptCacheToken(SystemTime);

impl PromptCacheToken {
    pub fn epoch() -> Self {
        Self(SystemTime::UNIX_EPOCH)
    }
}

struct PromptCache {
    last_max_mtime: SystemTime,
    cached: Vec<PromptTemplate>,
    cached_for_working_dir: Option<PathBuf>,
}

impl Default for PromptCache {
    fn default() -> Self {
        Self {
            last_max_mtime: SystemTime::UNIX_EPOCH,
            cached: Vec::new(),
            cached_for_working_dir: None,
        }
    }
}

static PROMPT_CACHE: Mutex<Option<PromptCache>> = Mutex::new(None);

/// Hot-reload aware discover: returns the cached list when no source
/// file under any prompts directory has changed since the last call.
/// Falls back to a fresh scan otherwise.
///
/// The token is the mtime watermark at the time of scan; pass it
/// back to `prompt_templates_changed_since` to check for change
/// without doing another scan.
pub fn discover_cached() -> (Vec<PromptTemplate>, PromptCacheToken) {
    discover_cached_in(std::env::current_dir().ok().as_deref())
}

pub fn discover_cached_in(working_dir: Option<&Path>) -> (Vec<PromptTemplate>, PromptCacheToken) {
    let max_mtime = max_mtime_in(working_dir);
    let working_dir_buf = working_dir.map(|p| p.to_path_buf());

    let mut guard = PROMPT_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(PromptCache::default);

    let stale =
        cache.last_max_mtime != max_mtime || cache.cached_for_working_dir != working_dir_buf;
    if stale {
        cache.cached = discover_in(working_dir);
        cache.last_max_mtime = max_mtime;
        cache.cached_for_working_dir = working_dir_buf;
    }
    (cache.cached.clone(), PromptCacheToken(cache.last_max_mtime))
}

/// Has anything changed since `token`? Cheap mtime probe — no scan.
pub fn prompt_templates_changed_since(token: &PromptCacheToken) -> bool {
    let max_mtime = max_mtime_in(std::env::current_dir().ok().as_deref());
    max_mtime != token.0
}

/// Drop the cache. Useful in tests to ensure isolation.
pub fn clear_prompt_cache_for_tests() {
    let mut guard = PROMPT_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    *guard = None;
}

fn max_mtime_in(working_dir: Option<&Path>) -> SystemTime {
    let mut max = SystemTime::UNIX_EPOCH;

    fn fold_dir_mtimes(dir: &Path, max: &mut SystemTime) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata()
                && let Ok(m) = meta.modified()
                && m > *max
            {
                *max = m;
            }
        }
        // Also include the directory's own mtime so file deletions
        // bump the watermark.
        if let Ok(meta) = std::fs::metadata(dir)
            && let Ok(m) = meta.modified()
            && m > *max
        {
            *max = m;
        }
    }

    if let Some(start) = working_dir {
        let mut current: Option<&Path> = Some(start);
        while let Some(d) = current {
            let dir = d.join(".jcode").join("prompts");
            if dir.is_dir() {
                fold_dir_mtimes(&dir, &mut max);
            }
            current = d.parent();
        }
    }
    if let Ok(home) = crate::storage::jcode_dir() {
        let global = home.join("prompts");
        if global.is_dir() {
            fold_dir_mtimes(&global, &mut max);
        }
    }
    max
}

/// One discovered prompt template.
#[derive(Debug, Clone)]
pub struct PromptTemplate {
    /// Command name (file stem). Used as `/<name>` for the future expansion
    /// flow.
    pub name: String,
    /// Source path on disk.
    pub path: PathBuf,
    /// Origin: `"project"` for `.jcode/prompts/` walked up from cwd,
    /// `"user"` for `~/.jcode/prompts/`.
    pub source: &'static str,
    /// Raw body (file contents). MVP keeps this opaque — front-matter and
    /// `{{name}}` placeholders are parsed in a follow-up PR.
    pub body: String,
}

pub fn is_valid_template_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn collect_dir_into(out: &mut BTreeMap<String, PromptTemplate>, dir: &Path, source: &'static str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase());
        if ext.as_deref() != Some("md") {
            continue;
        }
        if !is_valid_template_name(stem) {
            continue;
        }
        let body = match std::fs::read_to_string(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        // Project path (closest-to-cwd) inserted first wins; user-global is
        // walked last in `discover` and uses `entry().or_insert(...)` so it
        // does not overwrite project entries.
        out.entry(stem.to_string())
            .or_insert_with(|| PromptTemplate {
                name: stem.to_string(),
                path: path.clone(),
                source,
                body,
            });
    }
}

/// Walk cwd ancestors then home for `.jcode/prompts/`. Closest-to-cwd wins.
pub fn discover() -> Vec<PromptTemplate> {
    discover_in(std::env::current_dir().ok().as_deref())
}

/// Same as `discover` but takes an explicit working dir for tests.
pub fn discover_in(working_dir: Option<&Path>) -> Vec<PromptTemplate> {
    let mut found: BTreeMap<String, PromptTemplate> = BTreeMap::new();

    // Walk project dirs cwd-first up to root so cwd-closest wins for any name.
    if let Some(start) = working_dir {
        let mut current: Option<&Path> = Some(start);
        while let Some(d) = current {
            let dir = d.join(".jcode").join("prompts");
            if dir.is_dir() {
                collect_dir_into(&mut found, &dir, "project");
            }
            current = d.parent();
        }
    }

    // Then user-global. or_insert keeps any project entry that won.
    if let Ok(home) = crate::storage::jcode_dir() {
        let global = home.join("prompts");
        if global.is_dir() {
            collect_dir_into(&mut found, &global, "user");
        }
    }

    found.into_values().collect()
}

/// Resolve a single template by name, preserving discovery precedence.
pub fn find_by_name(name: &str) -> Option<PromptTemplate> {
    discover().into_iter().find(|t| t.name == name)
}

/// Serializable summary for `jcode prompts list --json`.
#[derive(Debug, serde::Serialize)]
pub struct PromptTemplateSummary<'a> {
    pub name: &'a str,
    pub path: String,
    pub source: &'a str,
    pub bytes: usize,
}

impl<'a> From<&'a PromptTemplate> for PromptTemplateSummary<'a> {
    fn from(t: &'a PromptTemplate) -> Self {
        Self {
            name: &t.name,
            path: t.path.display().to_string(),
            source: t.source,
            bytes: t.body.len(),
        }
    }
}

pub fn run_list(json: bool) -> Result<()> {
    let templates = discover();
    if json {
        let summaries: Vec<PromptTemplateSummary> = templates.iter().map(Into::into).collect();
        println!("{}", serde_json::to_string_pretty(&summaries)?);
        return Ok(());
    }
    if templates.is_empty() {
        println!(
            "No prompt templates found. Drop Markdown files into `.jcode/prompts/` (project) or `~/.jcode/prompts/` (user)."
        );
        return Ok(());
    }
    println!("Discovered {} prompt template(s):", templates.len());
    for t in &templates {
        println!(
            "  /{:<24} [{}]  {}  ({} bytes)",
            t.name,
            t.source,
            t.path.display(),
            t.body.len()
        );
    }
    Ok(())
}

pub fn run_show(name: &str) -> Result<()> {
    let template =
        find_by_name(name).with_context(|| format!("prompt template '{name}' not found"))?;
    eprintln!(
        "# /{name}  [{}]  {}",
        template.source,
        template.path.display()
    );
    println!("{}", template.body.trim_end());
    Ok(())
}

/// One declared template argument from the YAML frontmatter `args:` list.
///
/// Issue #4 follow-up: templates can declare arg names + defaults; expansion
/// substitutes `{{name}}` placeholders in the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgDecl {
    pub name: String,
    pub required: bool,
    pub default: Option<String>,
}

/// Split a template into (frontmatter_lines, body, declared_args).
///
/// Recognized frontmatter format (lightweight YAML subset — we don't
/// pull in serde_yaml for this):
///
/// ```text
/// ---
/// description: ...
/// args:
/// - name: focus
///   required: false
///   default: bugs
/// - name: target
///   required: true
/// ---
///
/// # body starts here
/// ```
///
/// If no frontmatter is present, returns `(None, full_body, vec![])`.
pub fn parse_frontmatter(raw: &str) -> (Option<String>, String, Vec<ArgDecl>) {
    let trimmed = raw.trim_start_matches('\u{feff}');
    if !trimmed.starts_with("---\n") && !trimmed.starts_with("---\r\n") {
        return (None, raw.to_string(), Vec::new());
    }

    // Find the closing `---` on its own line.
    let after_first = match trimmed.find('\n') {
        Some(idx) => &trimmed[idx + 1..],
        None => return (None, raw.to_string(), Vec::new()),
    };
    let close_marker = after_first
        .find("\n---\n")
        .or_else(|| after_first.find("\n---\r\n"))
        .or_else(|| after_first.strip_suffix("\n---").map(|s| s.len()));
    let Some(close_idx) = close_marker else {
        return (None, raw.to_string(), Vec::new());
    };
    let frontmatter = &after_first[..close_idx];
    // Skip the closing `---` line + optional newline.
    let body_start = close_idx + after_first[close_idx..].find('\n').unwrap_or(0);
    let body = after_first[body_start..]
        .trim_start_matches(['\n', '\r'])
        .to_string();

    let args = parse_args_block(frontmatter);
    (Some(frontmatter.to_string()), body, args)
}

/// Parse the `args:` list out of frontmatter. Tolerant of:
/// - `args:` followed by indented `- name: foo` entries
/// - per-arg `required` / `default` keys
/// - missing `args:` block (returns empty vec)
fn parse_args_block(frontmatter: &str) -> Vec<ArgDecl> {
    let mut out: Vec<ArgDecl> = Vec::new();
    let mut in_args = false;
    let mut current: Option<ArgDecl> = None;

    for raw_line in frontmatter.lines() {
        let line = raw_line.trim_end();
        let trimmed = line.trim_start();

        // A line at column 0 that doesn't start with `-` is a top-level key.
        // YAML list entries like `- name: foo` may live at column 0 too (no
        // indent under `args:`), so we don't treat those as a new top-level
        // section when we're currently inside `args:`.
        let is_top_level_key =
            !line.starts_with(' ') && !line.starts_with('\t') && !line.starts_with('-');
        if is_top_level_key {
            // Push pending arg from previous block.
            if let Some(arg) = current.take() {
                out.push(arg);
            }
            in_args = trimmed == "args:" || trimmed.starts_with("args:");
            continue;
        }

        if !in_args {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("- ") {
            // New arg entry; flush previous.
            if let Some(arg) = current.take() {
                out.push(arg);
            }
            // First field on the dash line, e.g. `- name: focus`
            let mut new_arg = ArgDecl {
                name: String::new(),
                required: false,
                default: None,
            };
            apply_arg_field(&mut new_arg, rest);
            current = Some(new_arg);
            continue;
        }

        // Continuation field on a current arg: `  required: true`
        if let Some(arg) = current.as_mut() {
            apply_arg_field(arg, trimmed);
        }
    }

    if let Some(arg) = current.take() {
        out.push(arg);
    }

    // Filter out unnamed entries (malformed frontmatter).
    out.retain(|a| !a.name.is_empty());
    out
}

fn apply_arg_field(arg: &mut ArgDecl, line: &str) {
    let Some((key, value)) = line.split_once(':') else {
        return;
    };
    let key = key.trim();
    let value = value.trim();
    let value_stripped = value
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(value);
    match key {
        "name" => arg.name = value_stripped.to_string(),
        "required" => arg.required = matches!(value_stripped, "true" | "yes" | "1"),
        "default" => arg.default = Some(value_stripped.to_string()),
        _ => {}
    }
}

/// Parse a free-form arg string into named + positional values.
///
/// Recognized shapes:
///   `focus=auth target=src/foo.rs`     fully named
///   `auth src/foo.rs`                  fully positional
///   `auth target=src/foo.rs`           mixed (positional first)
///
/// Quoting: values can be wrapped in single or double quotes to include
/// spaces.
pub fn parse_user_args(input: &str) -> (Vec<String>, std::collections::HashMap<String, String>) {
    let mut positional: Vec<String> = Vec::new();
    let mut named: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for token in tokenize_args(input) {
        if let Some((k, v)) = token.split_once('=') {
            // Only treat as named if k looks like an identifier.
            if !k.is_empty()
                && k.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                named.insert(k.to_string(), v.to_string());
                continue;
            }
        }
        positional.push(token);
    }
    (positional, named)
}

/// Split `input` on whitespace, honoring single- and double-quoted regions.
fn tokenize_args(input: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let chars = input.chars().peekable();
    let mut quote: Option<char> = None;
    for ch in chars {
        match (ch, quote) {
            (c, Some(q)) if c == q => {
                quote = None;
            }
            (c, Some(_)) => cur.push(c),
            ('"', None) | ('\'', None) => {
                quote = Some(ch);
            }
            (c, None) if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            (c, None) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Bind user-supplied args to declared args.
///
/// Resolution per declared arg:
///   1. Named match (e.g. user passed `focus=auth`) wins.
///   2. Positional match (in declaration order) for any args not yet bound.
///   3. `default` if declared.
///   4. Empty string if declared `required: false` and no value.
///   5. Bubble up an error if `required: true` and unbound.
///
/// Returns (bindings, missing_required_names).
pub fn bind_args(
    decls: &[ArgDecl],
    positional: &[String],
    named: &std::collections::HashMap<String, String>,
) -> (std::collections::HashMap<String, String>, Vec<String>) {
    let mut bindings: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut missing: Vec<String> = Vec::new();
    let mut positional_iter = positional.iter();

    for decl in decls {
        if let Some(v) = named.get(&decl.name) {
            bindings.insert(decl.name.clone(), v.clone());
            continue;
        }
        if let Some(v) = positional_iter.next() {
            bindings.insert(decl.name.clone(), v.clone());
            continue;
        }
        if let Some(d) = &decl.default {
            bindings.insert(decl.name.clone(), d.clone());
            continue;
        }
        if decl.required {
            missing.push(decl.name.clone());
        } else {
            bindings.insert(decl.name.clone(), String::new());
        }
    }
    (bindings, missing)
}

/// Substitute `{{name}}` literal placeholders in `body` with values from
/// `bindings`. Unknown placeholders are left intact so the user can see
/// what wasn't bound.
pub fn substitute_placeholders(
    body: &str,
    bindings: &std::collections::HashMap<String, String>,
) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        let Some(end) = after_open.find("}}") else {
            // No closing brace — keep the rest verbatim and stop.
            out.push_str(&rest[start..]);
            return out;
        };
        let key = after_open[..end].trim();
        match bindings.get(key) {
            Some(value) => out.push_str(value),
            None => {
                // Leave the `{{key}}` intact so the user sees the unbound
                // placeholder in their message.
                out.push_str(&rest[start..start + 2 + end + 2]);
            }
        }
        rest = &after_open[end + 2..];
    }
    out.push_str(rest);
    out
}

/// Where `prompts new` should drop a freshly-scaffolded template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewLocation {
    /// `<cwd>/.jcode/prompts/<name>.md` (default; project-local).
    Project,
    /// `~/.jcode/prompts/<name>.md` (user-global).
    User,
}

/// Scaffold a new prompt-template file with a starter body.
///
/// Returns the absolute path the file was written to. Refuses to clobber an
/// existing file unless `force` is true.
pub fn run_new(name: &str, location: NewLocation, force: bool) -> Result<PathBuf> {
    if !is_valid_template_name(name) {
        anyhow::bail!("Template name '{name}' must be ASCII alphanumeric + '-' or '_'.");
    }

    let dir = match location {
        NewLocation::Project => {
            let cwd = std::env::current_dir().context("cannot resolve cwd")?;
            cwd.join(".jcode").join("prompts")
        }
        NewLocation::User => crate::storage::jcode_dir()
            .context("cannot resolve ~/.jcode")?
            .join("prompts"),
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;

    let path = dir.join(format!("{name}.md"));
    if path.exists() && !force {
        anyhow::bail!(
            "{} already exists. Pass --force to overwrite.",
            path.display()
        );
    }

    let scaffold = format!(
        "---\n\
         description: TODO — what this prompt does\n\
         args:\n\
         - name: focus\n\
           required: false\n\
           default: bugs\n\
         ---\n\n\
         # {name}\n\n\
         Replace this body with the prompt jcode should expand when the user\n\
         types `/{name}` (or `/{name} <args>`).\n\n\
         Example placeholders supported by future expansion work:\n\
         - {{{{focus}}}} — bound to the `focus` arg above (default `bugs`).\n\n\
         Until expansion lands, the body is inserted verbatim into the editor.\n",
    );
    std::fs::write(&path, scaffold)
        .with_context(|| format!("failed to write {}", path.display()))?;

    println!("{}", path.display());
    Ok(path)
}

#[cfg(test)]
mod new_tests {
    use super::*;

    #[test]
    fn run_new_writes_starter_template_to_user_dir() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_HOME");
        let temp = tempfile::TempDir::new().expect("temp");
        crate::env::set_var("JCODE_HOME", temp.path());

        let path = run_new("review", NewLocation::User, false).expect("scaffold");
        assert_eq!(path, temp.path().join("prompts").join("review.md"));
        let body = std::fs::read_to_string(&path).expect("read back");
        assert!(body.starts_with("---\n"));
        assert!(body.contains("# review"));
        assert!(body.contains("`/review`"));

        // Refuses to clobber.
        let err = run_new("review", NewLocation::User, false).unwrap_err();
        assert!(err.to_string().contains("already exists"));

        // --force overrides.
        run_new("review", NewLocation::User, true).expect("force overwrite");

        if let Some(prev) = prev {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn run_new_rejects_invalid_names() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_HOME");
        let temp = tempfile::TempDir::new().expect("temp");
        crate::env::set_var("JCODE_HOME", temp.path());

        for bad in ["bad name", "with$char", "", "../escape"] {
            let err = run_new(bad, NewLocation::User, false).unwrap_err();
            assert!(
                err.to_string().contains("must be ASCII alphanumeric"),
                "bad name {bad:?} not rejected: {err}"
            );
        }

        if let Some(prev) = prev {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_dir_overrides_user_global_on_name_collision() {
        let temp = tempfile::TempDir::new().expect("temp");
        let proj = temp.path().join(".jcode/prompts");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("review.md"), "PROJECT_REVIEW_BODY").unwrap();

        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_HOME");
        let user_temp = tempfile::TempDir::new().expect("user");
        crate::env::set_var("JCODE_HOME", user_temp.path());
        let user_dir = user_temp.path().join("prompts");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::write(user_dir.join("review.md"), "USER_REVIEW_BODY").unwrap();

        let templates = discover_in(Some(temp.path()));

        if let Some(prev) = prev {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }

        let review = templates
            .iter()
            .find(|t| t.name == "review")
            .expect("found");
        assert_eq!(review.body, "PROJECT_REVIEW_BODY");
        assert_eq!(review.source, "project");
    }

    #[test]
    fn invalid_names_are_skipped() {
        let temp = tempfile::TempDir::new().expect("temp");
        let proj = temp.path().join(".jcode/prompts");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("ok-name.md"), "ok").unwrap();
        std::fs::write(proj.join("bad name with spaces.md"), "bad").unwrap();
        std::fs::write(proj.join("with$char.md"), "bad").unwrap();
        std::fs::write(proj.join("not-markdown.txt"), "ignored").unwrap();

        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_HOME");
        let user_temp = tempfile::TempDir::new().expect("user");
        crate::env::set_var("JCODE_HOME", user_temp.path());

        let templates = discover_in(Some(temp.path()));

        if let Some(prev) = prev {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }

        let names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["ok-name"]);
    }

    #[test]
    fn ancestor_walk_finds_template_in_parent_jcode_dir() {
        let temp = tempfile::TempDir::new().expect("temp");
        let parent = temp.path();
        let proj = parent.join(".jcode/prompts");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("rev.md"), "BODY_FROM_PARENT").unwrap();
        let nested = parent.join("nested/deep");
        std::fs::create_dir_all(&nested).unwrap();

        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_HOME");
        let user_temp = tempfile::TempDir::new().expect("user");
        crate::env::set_var("JCODE_HOME", user_temp.path());

        let templates = discover_in(Some(&nested));

        if let Some(prev) = prev {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }

        let rev = templates.iter().find(|t| t.name == "rev").expect("found");
        assert_eq!(rev.body, "BODY_FROM_PARENT");
    }
}

#[cfg(test)]
mod arg_substitution_tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn parse_frontmatter_extracts_args_block() {
        let body = "---\n\
                    description: review\n\
                    args:\n\
                    - name: focus\n  required: false\n  default: bugs\n\
                    - name: target\n  required: true\n\
                    ---\n\n# review\n\nFocus on {{focus}} in {{target}}.\n";
        let (fm, body, args) = parse_frontmatter(body);
        assert!(fm.is_some());
        assert!(body.contains("Focus on {{focus}} in {{target}}."));
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].name, "focus");
        assert!(!args[0].required);
        assert_eq!(args[0].default.as_deref(), Some("bugs"));
        assert_eq!(args[1].name, "target");
        assert!(args[1].required);
        assert!(args[1].default.is_none());
    }

    #[test]
    fn parse_frontmatter_handles_no_frontmatter() {
        let body = "# Plain body\n\nNo frontmatter here.\n";
        let (fm, parsed_body, args) = parse_frontmatter(body);
        assert!(fm.is_none());
        assert_eq!(parsed_body, body);
        assert!(args.is_empty());
    }

    #[test]
    fn parse_user_args_handles_named_and_positional() {
        let (pos, named) = parse_user_args("auth focus=bugs target=src/foo.rs");
        assert_eq!(pos, vec!["auth"]);
        assert_eq!(named.get("focus").map(String::as_str), Some("bugs"));
        assert_eq!(named.get("target").map(String::as_str), Some("src/foo.rs"));
    }

    #[test]
    fn parse_user_args_honors_quotes() {
        let (pos, named) = parse_user_args("\"hello world\" key=\"val with space\"");
        assert_eq!(pos, vec!["hello world"]);
        assert_eq!(named.get("key").map(String::as_str), Some("val with space"));
    }

    #[test]
    fn bind_args_named_wins_over_positional() {
        let decls = vec![
            ArgDecl {
                name: "focus".into(),
                required: false,
                default: None,
            },
            ArgDecl {
                name: "target".into(),
                required: false,
                default: None,
            },
        ];
        let mut named = HashMap::new();
        named.insert("focus".into(), "auth".into());
        let positional = vec!["src/foo.rs".to_string()];
        let (bindings, missing) = bind_args(&decls, &positional, &named);
        assert_eq!(bindings.get("focus").map(String::as_str), Some("auth"));
        // Positional fills target since focus was named.
        assert_eq!(
            bindings.get("target").map(String::as_str),
            Some("src/foo.rs")
        );
        assert!(missing.is_empty());
    }

    #[test]
    fn bind_args_uses_default_when_no_value() {
        let decls = vec![ArgDecl {
            name: "focus".into(),
            required: false,
            default: Some("bugs".into()),
        }];
        let (bindings, missing) = bind_args(&decls, &[], &HashMap::new());
        assert_eq!(bindings.get("focus").map(String::as_str), Some("bugs"));
        assert!(missing.is_empty());
    }

    #[test]
    fn bind_args_reports_missing_required() {
        let decls = vec![ArgDecl {
            name: "target".into(),
            required: true,
            default: None,
        }];
        let (bindings, missing) = bind_args(&decls, &[], &HashMap::new());
        assert!(bindings.is_empty());
        assert_eq!(missing, vec!["target"]);
    }

    #[test]
    fn substitute_placeholders_replaces_known_keys() {
        let mut bindings = HashMap::new();
        bindings.insert("focus".into(), "auth".into());
        bindings.insert("target".into(), "src/foo.rs".into());
        let out = substitute_placeholders("Focus on {{focus}} in {{target}}.", &bindings);
        assert_eq!(out, "Focus on auth in src/foo.rs.");
    }

    #[test]
    fn substitute_placeholders_leaves_unknown_intact() {
        let bindings = HashMap::new();
        let out = substitute_placeholders("Use {{missing}} please.", &bindings);
        // Unknown stays as `{{missing}}` so user sees what's unbound.
        assert_eq!(out, "Use {{missing}} please.");
    }

    #[test]
    fn substitute_placeholders_handles_unmatched_open_brace() {
        let bindings = HashMap::new();
        let out = substitute_placeholders("Plain {{ but no close.", &bindings);
        assert_eq!(out, "Plain {{ but no close.");
    }

    #[test]
    fn substitute_placeholders_trims_whitespace_inside_braces() {
        let mut bindings = HashMap::new();
        bindings.insert("focus".into(), "auth".into());
        let out = substitute_placeholders("Look at {{ focus }}.", &bindings);
        assert_eq!(out, "Look at auth.");
    }
}

#[cfg(test)]
mod hot_reload_tests {
    use super::*;
    use std::time::Duration;

    fn write_template(dir: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(format!("{name}.md")), body).unwrap();
    }

    #[test]
    fn cached_returns_cached_when_unchanged() {
        let _lock = crate::storage::lock_test_env();
        clear_prompt_cache_for_tests();
        let temp = tempfile::TempDir::new().unwrap();
        let prev = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let project = temp.path().join("project");
        let prompts = project.join(".jcode/prompts");
        write_template(&prompts, "review", "Review this.");

        let (first, token1) = discover_cached_in(Some(&project));
        let (second, token2) = discover_cached_in(Some(&project));

        assert_eq!(first.len(), 1);
        assert_eq!(first.len(), second.len());
        assert_eq!(token1, token2);

        if let Some(p) = prev {
            crate::env::set_var("JCODE_HOME", p);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn cached_invalidates_when_new_template_added() {
        let _lock = crate::storage::lock_test_env();
        clear_prompt_cache_for_tests();
        let temp = tempfile::TempDir::new().unwrap();
        let prev = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let project = temp.path().join("proj2");
        let prompts = project.join(".jcode/prompts");
        write_template(&prompts, "first", "First.");

        let (first, token1) = discover_cached_in(Some(&project));
        assert_eq!(first.len(), 1);

        // Sleep enough for mtime to advance (filesystem resolution).
        std::thread::sleep(Duration::from_millis(50));
        write_template(&prompts, "second", "Second.");

        let (second, token2) = discover_cached_in(Some(&project));
        assert_eq!(second.len(), 2);
        assert_ne!(token1, token2);

        if let Some(p) = prev {
            crate::env::set_var("JCODE_HOME", p);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn cached_invalidates_when_template_deleted() {
        let _lock = crate::storage::lock_test_env();
        clear_prompt_cache_for_tests();
        let temp = tempfile::TempDir::new().unwrap();
        let prev = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let project = temp.path().join("proj3");
        let prompts = project.join(".jcode/prompts");
        write_template(&prompts, "doomed", "About to be deleted.");

        let (first, token1) = discover_cached_in(Some(&project));
        assert_eq!(first.len(), 1);

        std::thread::sleep(Duration::from_millis(50));
        std::fs::remove_file(prompts.join("doomed.md")).unwrap();

        let (second, token2) = discover_cached_in(Some(&project));
        assert_eq!(second.len(), 0);
        assert_ne!(token1, token2);

        if let Some(p) = prev {
            crate::env::set_var("JCODE_HOME", p);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn changed_since_detects_new_file_without_full_rescan() {
        let _lock = crate::storage::lock_test_env();
        clear_prompt_cache_for_tests();
        let temp = tempfile::TempDir::new().unwrap();
        let prev_home = std::env::var_os("JCODE_HOME");
        let prev_cwd = std::env::current_dir().ok();
        crate::env::set_var("JCODE_HOME", temp.path());
        std::env::set_current_dir(temp.path()).unwrap();

        let prompts = temp.path().join(".jcode/prompts");
        write_template(&prompts, "initial", "x");

        let (_, token) = discover_cached();
        assert!(!prompt_templates_changed_since(&token));

        std::thread::sleep(Duration::from_millis(50));
        write_template(&prompts, "added", "y");

        assert!(prompt_templates_changed_since(&token));

        if let Some(c) = prev_cwd {
            std::env::set_current_dir(c).ok();
        }
        if let Some(p) = prev_home {
            crate::env::set_var("JCODE_HOME", p);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn epoch_token_always_signals_change_with_existing_files() {
        let _lock = crate::storage::lock_test_env();
        clear_prompt_cache_for_tests();
        let temp = tempfile::TempDir::new().unwrap();
        let prev_home = std::env::var_os("JCODE_HOME");
        let prev_cwd = std::env::current_dir().ok();
        crate::env::set_var("JCODE_HOME", temp.path());
        std::env::set_current_dir(temp.path()).unwrap();

        let prompts = temp.path().join(".jcode/prompts");
        write_template(&prompts, "any", "any");

        // Epoch token should always differ from current state when files exist.
        assert!(prompt_templates_changed_since(&PromptCacheToken::epoch()));

        if let Some(c) = prev_cwd {
            std::env::set_current_dir(c).ok();
        }
        if let Some(p) = prev_home {
            crate::env::set_var("JCODE_HOME", p);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}
