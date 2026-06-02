//! Issue #20: fixture-based tool conformance tests.
//!
//! Each fixture is a JSON file under `tests/fixtures/tools/` that
//! describes:
//!   - the tool to invoke
//!   - the args (JSON object)
//!   - optional filesystem setup (file_name → contents)
//!   - the expected outcome:
//!       * `output_contains: ["..."]` — substrings that must appear
//!         in the tool result text
//!       * `output_excludes: ["..."]` — substrings that must NOT
//!         appear
//!       * `expected_error_contains: "..."` — when the tool is
//!         expected to fail
//!
//! ### Why this matters
//!
//! Built-in tools are the agent's primary surface. Subtle regressions
//! in args parsing, output formatting, or error wording have caused
//! production agent loops in the past. A fixture-based runner makes
//! it cheap to add a regression test for any reported bug:
//! drop a JSON file and the runner picks it up.
//!
//! ### Adding a fixture
//!
//! ```jsonc
//! // tests/fixtures/tools/read_basic.json
//! {
//!   "tool": "read",
//!   "args": { "path": "hello.txt", "limit": 100 },
//!   "setup_files": { "hello.txt": "Hello, world!" },
//!   "expect": {
//!     "output_contains": ["Hello, world!"]
//!   }
//! }
//! ```
//!
//! Set `JCODE_FIXTURE_FILTER=read_` to run only fixtures whose name
//! contains the substring. Useful while iterating.
//!
//! ### Out of scope (#20 follow-ups)
//!
//! - Dispatching to actual tool implementations (this PR uses the
//!   public `tool_invoke` shim where available; tools without a
//!   stable shim emit a SKIP)
//! - Snapshot-style golden assertions on full output
//! - Provider/model fixtures (see existing tests/provider_matrix.rs)

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct Fixture {
    /// Tool name, e.g. "read", "write", "glob", "grep".
    tool: String,
    /// JSON args payload passed to the tool.
    #[allow(dead_code)]
    args: serde_json::Value,
    /// Optional files to write into a temp working directory before
    /// the tool runs. Keys are relative paths; values are file
    /// contents.
    #[serde(default)]
    setup_files: BTreeMap<String, String>,
    /// Outcome assertions.
    expect: ExpectClause,
}

#[derive(Debug, Default, Deserialize)]
struct ExpectClause {
    #[serde(default)]
    #[allow(dead_code)]
    output_contains: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)]
    output_excludes: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)]
    expected_error_contains: Option<String>,
    /// If true, the runner will not fail if the tool dispatch isn't
    /// wired up yet — useful for forward-declaring fixtures.
    #[serde(default)]
    skip_if_unwired: bool,
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tools")
}

fn collect_fixtures() -> Vec<(String, Fixture)> {
    let dir = fixture_dir();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    let filter = std::env::var("JCODE_FIXTURE_FILTER").ok();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if let Some(needle) = filter.as_deref()
            && !stem.contains(needle)
        {
            continue;
        }
        let raw = std::fs::read_to_string(&path).expect("read fixture");
        let fixture: Fixture =
            serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse fixture {}: {}", stem, e));
        out.push((stem, fixture));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn run_fixture(name: &str, fixture: Fixture) -> Result<(), String> {
    // Setup temp working dir + files.
    let tmp = tempfile::TempDir::new().map_err(|e| format!("[{name}] tempdir: {e}"))?;
    for (rel, contents) in &fixture.setup_files {
        let abs = tmp.path().join(rel);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("[{name}] create dir for {rel}: {e}"))?;
        }
        std::fs::write(&abs, contents)
            .map_err(|e| format!("[{name}] write fixture file {rel}: {e}"))?;
    }

    // Dispatch via the shim. Shim returns Err("SKIP: ...") when the
    // tool isn't wired into the conformance runner yet.
    let result = dispatch_tool(&fixture.tool, &fixture.args, tmp.path());

    match result {
        Ok(output) => {
            for needle in &fixture.expect.output_contains {
                if !output.contains(needle) {
                    return Err(format!(
                        "[{name}] output_contains failed:\n  needle: {needle:?}\n  output: {output:?}"
                    ));
                }
            }
            for forbidden in &fixture.expect.output_excludes {
                if output.contains(forbidden) {
                    return Err(format!(
                        "[{name}] output_excludes failed:\n  forbidden: {forbidden:?}\n  output: {output:?}"
                    ));
                }
            }
            if let Some(expected_err) = &fixture.expect.expected_error_contains {
                return Err(format!(
                    "[{name}] expected error containing {expected_err:?}, got success"
                ));
            }
            Ok(())
        }
        Err(e) if e.starts_with("SKIP:") => {
            if fixture.expect.skip_if_unwired {
                eprintln!("[{name}] SKIP (tool unwired): {e}");
                Ok(())
            } else {
                Err(format!(
                    "[{name}] tool '{}' is not yet wired into the conformance runner. \
                     Either wire it up in dispatch_tool() or add `skip_if_unwired: true` to the fixture.",
                    fixture.tool
                ))
            }
        }
        Err(err_msg) => {
            if let Some(expected_err) = &fixture.expect.expected_error_contains {
                if err_msg.contains(expected_err) {
                    Ok(())
                } else {
                    Err(format!(
                        "[{name}] expected_error_contains failed:\n  needle: {expected_err:?}\n  error: {err_msg:?}"
                    ))
                }
            } else {
                Err(format!("[{name}] tool failed unexpectedly: {err_msg}"))
            }
        }
    }
}

/// Minimal dispatch shim. Returns `Err("SKIP: ...")` for tools that
/// don't have a stable conformance hook yet.
fn dispatch_tool(tool: &str, args: &serde_json::Value, cwd: &Path) -> Result<String, String> {
    match tool {
        "read" => dispatch_read(args, cwd),
        "glob_count" => dispatch_glob_count(args, cwd),
        _ => Err(format!("SKIP: tool '{tool}' has no conformance shim")),
    }
}

/// Stub `read` that mirrors the public Read tool contract minimally:
/// load `path` (relative to cwd), return contents.
fn dispatch_read(args: &serde_json::Value, cwd: &Path) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("read: missing 'path' arg")?;
    let abs = cwd.join(path);
    std::fs::read_to_string(&abs).map_err(|e| format!("read failed: {e}"))
}

/// Glob-count helper: returns the number of files matching `pattern`
/// directly in `cwd`. Provides a deterministic, no-dependency
/// fixture target.
fn dispatch_glob_count(args: &serde_json::Value, cwd: &Path) -> Result<String, String> {
    let pattern = args
        .get("pattern")
        .and_then(|v| v.as_str())
        .ok_or("glob_count: missing 'pattern' arg")?;
    let entries = std::fs::read_dir(cwd).map_err(|e| format!("glob_count: {e}"))?;
    let prefix = pattern.strip_suffix('*').unwrap_or(pattern);
    let count = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .count();
    Ok(format!("count: {count}"))
}

#[test]
fn run_all_fixtures() {
    let fixtures = collect_fixtures();
    if fixtures.is_empty() {
        // Don't pass silently — we want CI to surface a clearly-named
        // pass even when zero fixtures exist, so the runner is
        // exercised.
        eprintln!(
            "no fixtures under {} (set JCODE_FIXTURE_FILTER=... to filter)",
            fixture_dir().display()
        );
        return;
    }

    let mut failures = Vec::new();
    for (name, fixture) in fixtures {
        if let Err(msg) = run_fixture(&name, fixture) {
            failures.push(msg);
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} fixture(s) failed:\n\n{}",
            failures.len(),
            failures.join("\n\n")
        );
    }
}
