//! Opt-in structural repo map: definitions plus file-reference graph plus
//! hand-rolled PageRank, rendered as token-budgeted symbol stubs.
//!
//! Boundary (per #1230): read-only provider, never core, never default-on.
//! No tree-sitter, no petgraph. Symbol extraction is regex grammar sets over
//! an explicit language list (Rust, TypeScript/JavaScript, Python); an
//! unlisted extension yields no symbols, never a failure. The tool only
//! registers when `repomap_token_budget > 0`. Cache lives under
//! `.jcode/cache/repomap.json`, keyed per file by mtime plus size; a stale
//! file rebuilds alone, never the whole map.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Token budget default. 0 disables the map entirely (tool unregistered).
pub const DEFAULT_REPOMAP_TOKEN_BUDGET: usize = 0;
/// Damping factor for PageRank power iteration.
const DAMPING: f64 = 0.85;
/// Iterations cap; the graph is small and converges far earlier.
const MAX_ITERATIONS: usize = 50;
/// Directories never walked, however deep.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "target",
    "node_modules",
    "dist",
    "build",
    "__pycache__",
    ".venv",
    "venv",
];
/// Rough chars-per-token for budget truncation (stub text is ASCII dense).
const CHARS_PER_TOKEN: usize = 4;
/// Max symbols rendered per file block (long files truncate, rank decides).
const MAX_SYMBOLS_PER_FILE: usize = 50;
/// Max bytes read per source file. Bounds the text-read path (a symlinked
/// /dev/zero or multi-GB dump must not exhaust memory); oversized files
/// contribute no symbols. Generous: real sources fit comfortably.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

struct Grammar {
    extension: &'static str,
    /// (regex, kind label). First capture group is the symbol name.
    definitions: &'static [(&'static str, &'static str)],
}

const GRAMMARS: &[Grammar] = &[
    Grammar {
        extension: "rs",
        definitions: &[
            (
                r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)",
                "fn",
            ),
            (
                r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_][A-Za-z0-9_]*)",
                "struct",
            ),
            (
                r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_][A-Za-z0-9_]*)",
                "enum",
            ),
            (
                r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?trait\s+([A-Za-z_][A-Za-z0-9_]*)",
                "trait",
            ),
            (
                r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)",
                "mod",
            ),
            (
                r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?type\s+([A-Za-z_][A-Za-z0-9_]*)",
                "type",
            ),
            (
                r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?const\s+([A-Za-z_][A-Za-z0-9_]*)",
                "const",
            ),
        ],
    },
    Grammar {
        extension: "ts",
        definitions: &[
            (
                r"(?m)^\s*(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_][A-Za-z0-9_]*)",
                "fn",
            ),
            (
                r"(?m)^\s*export\s+(?:default\s+)?class\s+([A-Za-z_][A-Za-z0-9_]*)",
                "class",
            ),
            (
                r"(?m)^\s*export\s+interface\s+([A-Za-z_][A-Za-z0-9_]*)",
                "interface",
            ),
            (r"(?m)^\s*export\s+type\s+([A-Za-z_][A-Za-z0-9_]*)", "type"),
            (r"(?m)^\s*export\s+enum\s+([A-Za-z_][A-Za-z0-9_]*)", "enum"),
        ],
    },
    Grammar {
        extension: "js",
        definitions: &[
            (
                r"(?m)^\s*(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_][A-Za-z0-9_]*)",
                "fn",
            ),
            (
                r"(?m)^\s*export\s+(?:default\s+)?class\s+([A-Za-z_][A-Za-z0-9_]*)",
                "class",
            ),
        ],
    },
    Grammar {
        extension: "py",
        definitions: &[
            (
                r"(?m)^\s*(?:async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)",
                "def",
            ),
            (r"(?m)^class\s+([A-Za-z_][A-Za-z0-9_]*)", "class"),
        ],
    },
];

struct CompiledGrammar {
    extension: &'static str,
    definitions: Vec<(regex::Regex, &'static str)>,
}

static COMPILED: LazyLock<Vec<CompiledGrammar>> = LazyLock::new(|| {
    GRAMMARS
        .iter()
        .map(|g| CompiledGrammar {
            extension: g.extension,
            definitions: g
                .definitions
                .iter()
                .filter_map(|(pattern, kind)| regex::Regex::new(pattern).ok().map(|re| (re, *kind)))
                .collect(),
        })
        .collect()
});

static WORD_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").expect("word regex"));

/// A single extracted symbol.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub line: usize,
    pub file: PathBuf,
}

/// Cached per-file parse: symbols plus the fingerprint they were built from.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CachedFile {
    mtime_ms: u128,
    size: u64,
    symbols: Vec<CachedSymbol>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CachedSymbol {
    name: String,
    kind: String,
    line: usize,
}

fn grammar_for(path: &Path) -> Option<&'static CompiledGrammar> {
    let ext = path.extension()?.to_str()?;
    COMPILED.iter().find(|g| g.extension == ext)
}

fn extract_symbols(text: &str, grammar: &CompiledGrammar, file: &Path) -> Vec<Symbol> {
    let mut out = Vec::new();
    for (re, kind) in &grammar.definitions {
        for cap in re.captures_iter(text) {
            let Some(name) = cap.get(1) else { continue };
            let line = text[..name.start()].chars().filter(|&c| c == '\n').count() + 1;
            out.push(Symbol {
                name: name.as_str().to_string(),
                kind: kind.to_string(),
                line,
                file: file.to_path_buf(),
            });
        }
    }
    out.sort_by_key(|s| s.line);
    out
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Collect listed-extension files under `root`. Never follows symlinks
/// (file or directory): a repo-controlled link pointing outside the tree
/// must not pull external source into the map or reach unbounded devices.
/// Every candidate is canonicalized and required to stay under the
/// canonicalized root.
fn walk_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(canonical_root) = root.canonicalize() else {
        return;
    };
    let mut dirs = vec![canonical_root.clone()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_symlink(&path) {
                continue;
            }
            let Ok(canonical) = path.canonicalize() else {
                continue;
            };
            if !canonical.starts_with(&canonical_root) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if canonical.is_dir() {
                if !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str()) {
                    dirs.push(canonical);
                }
            } else if grammar_for(&canonical).is_some() {
                out.push(canonical);
            }
        }
    }
}

fn file_fingerprint(path: &Path) -> Option<(u128, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((mtime, meta.len()))
}

/// Bounded text read: files over MAX_FILE_BYTES are skipped (no symbols).
fn read_source_capped(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FILE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn cache_path(root: &Path) -> PathBuf {
    root.join(".jcode").join("cache").join("repomap.json")
}

fn load_cache(root: &Path) -> HashMap<String, CachedFile> {
    let Ok(bytes) = std::fs::read(cache_path(root)) else {
        return HashMap::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save_cache(root: &Path, cache: &HashMap<String, CachedFile>) {
    let path = cache_path(root);
    // A mapped repo can symlink `.jcode/cache` (or the file itself) outside
    // the tree; following it would let the repo truncate and replace an
    // arbitrary writable file. Refuse symlinked components and destination.
    let mut cursor = root.to_path_buf();
    for component in [".jcode", "cache", "repomap.json"] {
        cursor = cursor.join(component);
        if is_symlink(&cursor) {
            return;
        }
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if is_symlink(&path) {
        return;
    }
    if let Ok(bytes) = serde_json::to_vec(cache) {
        let _ = std::fs::write(&path, bytes);
    }
}

/// Parse every listed file, reusing cache entries whose mtime plus size
/// still match. Returns symbols keyed by repo-relative path string.
fn parse_files(
    root: &Path,
    files: &[PathBuf],
    cache: &mut HashMap<String, CachedFile>,
) -> HashMap<String, Vec<Symbol>> {
    let mut out = HashMap::new();
    for file in files {
        let rel = file
            .strip_prefix(root)
            .unwrap_or(file)
            .to_string_lossy()
            .to_string();
        let print_path = PathBuf::from(&rel);
        let fingerprint = file_fingerprint(file);
        let text = read_source_capped(file);
        let grammar = grammar_for(file);
        if let (Some((mtime, size)), Some(hit)) = (fingerprint, cache.get(&rel))
            && hit.mtime_ms == mtime
            && hit.size == size
        {
            out.insert(
                rel.clone(),
                hit.symbols
                    .iter()
                    .map(|s| Symbol {
                        name: s.name.clone(),
                        kind: s.kind.clone(),
                        line: s.line,
                        file: print_path.clone(),
                    })
                    .collect(),
            );
            continue;
        }
        let symbols = match (text, grammar) {
            (Some(text), Some(grammar)) => extract_symbols(&text, grammar, &print_path),
            _ => Vec::new(),
        };
        if let Some((mtime, size)) = fingerprint {
            cache.insert(
                rel.clone(),
                CachedFile {
                    mtime_ms: mtime,
                    size,
                    symbols: symbols
                        .iter()
                        .map(|s| CachedSymbol {
                            name: s.name.clone(),
                            kind: s.kind.clone(),
                            line: s.line,
                        })
                        .collect(),
                },
            );
        }
        out.insert(rel, symbols);
    }
    out
}

/// Build the file-reference graph: file A links to file B when A mentions a
/// symbol that is defined in exactly one file (B), excluding self-links.
/// Ambiguous names (defined in several files) carry no edge. Returns
/// adjacency (outgoing edges) over file indices plus the file list order.
fn build_graph(
    files: &[String],
    symbols: &HashMap<String, Vec<Symbol>>,
    texts: &HashMap<String, String>,
) -> Vec<Vec<usize>> {
    let index: HashMap<&str, usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.as_str(), i))
        .collect();
    let mut owners: HashMap<String, String> = HashMap::new();
    let mut ambiguous: HashSet<String> = HashSet::new();
    for (file, syms) in symbols {
        for sym in syms {
            if ambiguous.contains(&sym.name) {
                continue;
            }
            match owners.get(&sym.name) {
                None => {
                    owners.insert(sym.name.clone(), file.clone());
                }
                Some(other) if other != file => {
                    owners.remove(&sym.name);
                    ambiguous.insert(sym.name.clone());
                }
                _ => {}
            }
        }
    }
    let mut edges: Vec<HashSet<usize>> = vec![HashSet::new(); files.len()];
    for (i, file) in files.iter().enumerate() {
        let Some(text) = texts.get(file) else {
            continue;
        };
        let mut seen_in_file = HashSet::new();
        for mat in WORD_RE.find_iter(text) {
            let name = mat.as_str();
            if !seen_in_file.insert(name) {
                continue;
            }
            if let Some(owner) = owners.get(name)
                && let Some(&j) = index.get(owner.as_str())
                && j != i
            {
                edges[i].insert(j);
            }
        }
    }
    edges.into_iter().map(|s| s.into_iter().collect()).collect()
}

/// Hand-rolled PageRank power iteration. `seeds` personalizes the teleport
/// vector toward the given file indices (files under discussion rank higher
/// and rank flows outward to their dependencies).
pub fn pagerank(adjacency: &[Vec<usize>], seeds: &[usize], iterations: usize) -> Vec<f64> {
    let n = adjacency.len();
    if n == 0 {
        return Vec::new();
    }
    let mut teleport = vec![0.0; n];
    if seeds.is_empty() {
        teleport.fill(1.0 / n as f64);
    } else {
        for &s in seeds {
            if s < n {
                teleport[s] += 1.0;
            }
        }
        let sum: f64 = teleport.iter().sum();
        if sum > 0.0 {
            for t in teleport.iter_mut() {
                *t /= sum;
            }
        } else {
            teleport.fill(1.0 / n as f64);
        }
    }
    let mut rank = teleport.clone();
    for _ in 0..iterations.max(1) {
        let mut next: Vec<f64> = teleport.iter().map(|t| (1.0 - DAMPING) * t).collect();
        for (i, outs) in adjacency.iter().enumerate() {
            if outs.is_empty() {
                let share = DAMPING * rank[i] / n as f64;
                for nval in next.iter_mut() {
                    *nval += share;
                }
            } else {
                let share = DAMPING * rank[i] / outs.len() as f64;
                for &j in outs {
                    next[j] += share;
                }
            }
        }
        rank = next;
    }
    rank
}

/// Render ranked stubs (`path:` then `kind name:line`), highest rank first,
/// truncated at `token_budget` estimated tokens. Budget 0 disables output.
pub fn render_map(
    files: &[String],
    symbols: &HashMap<String, Vec<Symbol>>,
    ranks: &[f64],
    token_budget: usize,
) -> Option<String> {
    if token_budget == 0 {
        return None;
    }
    let mut order: Vec<usize> = (0..files.len()).collect();
    order.sort_by(|&a, &b| ranks[b].total_cmp(&ranks[a]));
    let mut out = String::new();
    let mut used = 0usize;
    for i in order {
        let file = &files[i];
        let Some(syms) = symbols.get(file) else {
            continue;
        };
        if syms.is_empty() {
            continue;
        }
        let mut block = format!("{}:\n", file);
        for sym in syms.iter().take(MAX_SYMBOLS_PER_FILE) {
            block.push_str(&format!("  {} {}:{}\n", sym.kind, sym.name, sym.line));
        }
        let cost = block.len() / CHARS_PER_TOKEN + 1;
        if used + cost > token_budget {
            break;
        }
        used += cost;
        out.push_str(&block);
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Build the repo map for `root`. `seeds` are repo-relative path prefixes
/// (files under discussion) personalizing the rank. Returns `None` when the
/// budget is 0 or no symbols exist. Reads and refreshes the on-disk cache.
pub fn build_map(root: &Path, seeds: &[&str], token_budget: usize) -> Option<String> {
    if token_budget == 0 {
        return None;
    }
    let mut files = Vec::new();
    walk_files(root, &mut files);
    files.sort();
    let rels: Vec<String> = files
        .iter()
        .map(|f| {
            f.strip_prefix(root)
                .unwrap_or(f)
                .to_string_lossy()
                .to_string()
        })
        .collect();
    let mut cache = load_cache(root);
    let symbols = parse_files(root, &files, &mut cache);
    save_cache(root, &cache);
    let mut texts = HashMap::new();
    for (file, rel) in files.iter().zip(rels.iter()) {
        if symbols.get(rel).map(|s| s.is_empty()).unwrap_or(true) {
            continue;
        }
        if let Some(text) = read_source_capped(file) {
            texts.insert(rel.clone(), text);
        }
    }
    let adjacency = build_graph(&rels, &symbols, &texts);
    let seed_idx: Vec<usize> = rels
        .iter()
        .enumerate()
        .filter(|(_, f)| seeds.iter().any(|s| f.starts_with(s)))
        .map(|(i, _)| i)
        .collect();
    let ranks = pagerank(&adjacency, &seed_idx, MAX_ITERATIONS);
    render_map(&rels, &symbols, &ranks, token_budget)
}

/// Config knob read: token budget for the map (0 disables). Default 2000.
pub fn token_budget_from_config() -> usize {
    crate::config::config().agents.repomap_token_budget
}

#[cfg(test)]
#[path = "repomap_tests.rs"]
mod tests;
