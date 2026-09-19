//! Tests for the opt-in repo map. Layout, extraction, ranking, budget,
//! cache, and the disabled path.

use super::*;
use std::fs;

fn write_tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("temp dir");
    for (name, content) in files {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(&path, content).expect("write file");
    }
    dir
}

#[test]
fn extracts_rust_symbols_with_kinds_and_lines() {
    let dir = write_tree(&[(
        "lib.rs",
        "pub struct Engine {\n    x: u8,\n}\n\npub fn run(engine: &Engine) {}\n\nconst LIMIT: usize = 4;\n",
    )]);
    let mut files = Vec::new();
    walk_files(dir.path(), &mut files);
    assert_eq!(files.len(), 1);
    let mut cache = HashMap::new();
    let symbols = parse_files(dir.path(), &files, &mut cache);
    let syms = &symbols["lib.rs"];
    let names: Vec<(&str, &str)> = syms
        .iter()
        .map(|s| (s.kind.as_str(), s.name.as_str()))
        .collect();
    assert!(names.contains(&("struct", "Engine")), "{names:?}");
    assert!(names.contains(&("fn", "run")), "{names:?}");
    assert!(names.contains(&("const", "LIMIT")), "{names:?}");
    let run = syms.iter().find(|s| s.name == "run").unwrap();
    assert_eq!(run.line, 5);
}

#[test]
fn extracts_python_and_typescript() {
    let dir = write_tree(&[
        (
            "a.py",
            "class Handler:\n    async def handle(self):\n        pass\n",
        ),
        (
            "b.ts",
            "export function start() {}\nexport class Server {}\n",
        ),
    ]);
    let mut files = Vec::new();
    walk_files(dir.path(), &mut files);
    assert_eq!(files.len(), 2);
    let mut cache = HashMap::new();
    let symbols = parse_files(dir.path(), &files, &mut cache);
    let py: Vec<&str> = symbols["a.py"].iter().map(|s| s.name.as_str()).collect();
    assert!(py.contains(&"Handler") && py.contains(&"handle"), "{py:?}");
    let ts: Vec<&str> = symbols["b.ts"].iter().map(|s| s.name.as_str()).collect();
    assert!(ts.contains(&"start") && ts.contains(&"Server"), "{ts:?}");
}

#[test]
fn unlisted_extensions_are_ignored() {
    let dir = write_tree(&[
        ("notes.md", "# hi\n"),
        ("data.json", "{}\n"),
        ("main.rs", "fn main() {}\n"),
    ]);
    let mut files = Vec::new();
    walk_files(dir.path(), &mut files);
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with("main.rs"));
}

#[test]
fn skip_dirs_are_never_walked() {
    let dir = write_tree(&[
        ("src/a.rs", "fn a() {}\n"),
        ("target/b.rs", "fn b() {}\n"),
        ("node_modules/c.js", "export function c() {}\n"),
        (".git/d.rs", "fn d() {}\n"),
    ]);
    let mut files = Vec::new();
    walk_files(dir.path(), &mut files);
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with("a.rs"));
}

#[test]
fn pagerank_ranks_shared_dependency_highest() {
    // b is used by both a and c; a and c are leaves.
    let adjacency = vec![vec![1], vec![], vec![1]];
    let ranks = pagerank(&adjacency, &[], 50);
    assert!(ranks[1] > ranks[0] && ranks[1] > ranks[2], "{ranks:?}");
}

#[test]
fn pagerank_seeds_personalize_toward_focus_files() {
    // Chain 0 -> 1 -> 2. Globally 2 wins; seeded on 0, rank flows outward
    // and 0 outranks its unseeded position.
    let adjacency = vec![vec![1], vec![2], vec![]];
    let global = pagerank(&adjacency, &[], 50);
    assert!(global[2] > global[0], "{global:?}");
    let seeded = pagerank(&adjacency, &[0], 50);
    assert!(seeded[0] > global[0], "{seeded:?} vs {global:?}");
}

#[test]
fn ambiguous_symbols_carry_no_edge() {
    // `helper` defined in both b and c; a mentions it. No edge either way,
    // so a/b/c keep symmetric (equal) rank.
    let dir = write_tree(&[
        ("a.rs", "fn a() {\n    helper();\n}\n"),
        ("b.rs", "fn helper() {}\n"),
        ("c.rs", "fn helper() {}\n"),
    ]);
    let mut files = Vec::new();
    walk_files(dir.path(), &mut files);
    files.sort();
    let rels: Vec<String> = files
        .iter()
        .map(|f| {
            f.strip_prefix(dir.path())
                .unwrap()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    let mut cache = HashMap::new();
    let symbols = parse_files(dir.path(), &files, &mut cache);
    let mut texts = HashMap::new();
    for (file, rel) in files.iter().zip(rels.iter()) {
        texts.insert(rel.clone(), fs::read_to_string(file).unwrap());
    }
    // Isolated root: synthetic graphs must not inherit co-change edges
    // from the checkout the suite runs in.
    let isolated = tempfile::TempDir::new().expect("temp dir");
    let graph = build_graph(isolated.path(), &rels, &symbols, &texts);
    assert!(graph.iter().all(|outs| outs.is_empty()), "{graph:?}");
}

#[test]
fn unique_reference_creates_directed_edge() {
    let dir = write_tree(&[
        ("a.rs", "fn a() {\n    run();\n}\n"),
        ("b.rs", "fn run() {}\n"),
    ]);
    let mut files = Vec::new();
    walk_files(dir.path(), &mut files);
    files.sort();
    let rels: Vec<String> = files
        .iter()
        .map(|f| {
            f.strip_prefix(dir.path())
                .unwrap()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    let mut cache = HashMap::new();
    let symbols = parse_files(dir.path(), &files, &mut cache);
    let mut texts = HashMap::new();
    for (file, rel) in files.iter().zip(rels.iter()) {
        texts.insert(rel.clone(), fs::read_to_string(file).unwrap());
    }
    // Isolated root: synthetic graphs must not inherit co-change edges
    // from the checkout the suite runs in.
    let isolated = tempfile::TempDir::new().expect("temp dir");
    let graph = build_graph(isolated.path(), &rels, &symbols, &texts);
    // rels sorted: a.rs=0, b.rs=1. Edge 0 -> 1, none back.
    assert_eq!(graph, vec![vec![1], vec![]]);
}

#[test]
fn budget_truncates_low_rank_files() {
    let mut files_vec = Vec::new();
    for i in 0..10 {
        files_vec.push((format!("f{i}.rs"), "fn f() {}\n".to_string()));
    }
    let refs: Vec<(&str, &str)> = files_vec
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let dir = write_tree(&refs);
    let small = build_map(dir.path(), &[], 30).expect("small map");
    let big = build_map(dir.path(), &[], 2000).expect("big map");
    assert!(small.len() < big.len(), "{} vs {}", small.len(), big.len());
    assert!(small.contains("f0.rs") || small.contains("f1.rs"));
}

#[test]
fn budget_zero_disables_output() {
    let dir = write_tree(&[("a.rs", "fn a() {}\n")]);
    assert!(build_map(dir.path(), &[], 0).is_none());
}

#[test]
fn cache_rebuilds_only_changed_files() {
    let dir = write_tree(&[("a.rs", "fn a() {}\n"), ("b.rs", "fn b() {}\n")]);
    let first = build_map(dir.path(), &[], 2000).expect("first");
    assert!(first.contains("fn a"));
    // Unchanged rebuild is byte-identical (cache hit).
    let second = build_map(dir.path(), &[], 2000).expect("second");
    assert_eq!(first, second);
    // Touch one file with a new symbol; the map picks it up.
    std::thread::sleep(std::time::Duration::from_millis(10));
    fs::write(dir.path().join("b.rs"), "fn b() {}\nfn brand_new() {}\n").unwrap();
    let third = build_map(dir.path(), &[], 2000).expect("third");
    assert!(third.contains("brand_new"), "{third}");
    // Cache file exists on disk.
    assert!(cache_path(dir.path()).exists());
}

#[test]
fn render_respects_personalized_seeds_end_to_end() {
    // main mentions only run; seeded on main, both files must appear.
    let dir = write_tree(&[
        ("main.rs", "fn main() {\n    run();\n}\n"),
        ("util.rs", "fn run() {}\nfn other() {}\n"),
    ]);
    let map = build_map(dir.path(), &["main.rs"], 2000).expect("map");
    assert!(map.contains("main.rs"), "{map}");
    assert!(map.contains("util.rs"), "{map}");
    assert!(map.contains("fn run:3") || map.contains("run"), "{map}");
}

#[test]
fn symlinked_sources_are_excluded() {
    #[cfg(unix)]
    {
        let outside = write_tree(&[("ext.rs", "fn external() {}\n")]);
        let dir = write_tree(&[("a.rs", "fn a() {}\n")]);
        std::os::unix::fs::symlink(outside.path().join("ext.rs"), dir.path().join("link.rs"))
            .unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("extdir")).unwrap();
        let map = build_map(dir.path(), &[], 2000).expect("map");
        assert!(!map.contains("external"), "{map}");
        assert!(map.contains("fn a"), "{map}");
    }
}

#[test]
fn symlinked_cache_path_is_not_written_through() {
    #[cfg(unix)]
    {
        let outside = tempfile::TempDir::new().expect("temp");
        let target = outside.path().join("victim.json");
        std::fs::write(&target, "original").unwrap();
        let dir = write_tree(&[("a.rs", "fn a() {}\n")]);
        std::os::unix::fs::symlink(&target, dir.path().join(".jcode")).unwrap();
        let _ = build_map(dir.path(), &[], 2000);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
    }
}

#[test]
fn oversized_files_contribute_no_symbols() {
    let dir = write_tree(&[("big.rs", "fn big() {}\n")]);
    // Inflate past the cap without changing the symbol set.
    let mut content = String::from("fn big() {}\n");
    while content.len() < 3 * 1024 * 1024 {
        content.push_str("// padding padding padding padding\n");
    }
    std::fs::write(dir.path().join("big.rs"), &content).unwrap();
    let map = build_map(dir.path(), &[], 2000);
    // Oversized file skipped: no map (only file) or map without its symbols.
    assert!(map.map(|m| !m.contains("fn big")).unwrap_or(true));
}

#[test]
fn same_size_rewrite_is_picked_up() {
    let dir = write_tree(&[("a.rs", "fn alpha() {}\n")]);
    let first = build_map(dir.path(), &[], 2000).expect("first");
    assert!(first.contains("alpha"));
    // Same byte length, different symbol: nanos+size fingerprint must miss.
    std::fs::write(dir.path().join("a.rs"), "fn omega() {}\n").unwrap();
    assert_eq!("fn alpha() {}\n".len(), "fn omega() {}\n".len());
    let second = build_map(dir.path(), &[], 2000).expect("second");
    assert!(second.contains("omega"), "{second}");
    assert!(!second.contains("alpha"), "{second}");
}

#[test]
fn first_block_over_budget_yields_no_map() {
    let dir = write_tree(&[("a.rs", "fn a() {}\n")]);
    // Smallest block costs ~2 tokens; budget 1 fits nothing, not even first.
    assert!(build_map(dir.path(), &[], 1).is_none());
}

#[test]
fn exported_functions_render_once() {
    let dir = write_tree(&[(
        "b.ts",
        "export function start() {}\nexport async function go() {}\n",
    )]);
    let map = build_map(dir.path(), &[], 2000).expect("map");
    assert_eq!(map.matches("start").count(), 1, "{map}");
    assert_eq!(map.matches("go").count(), 1, "{map}");
}

#[test]
fn symbol_name_seed_personalizes_toward_defining_file() {
    let dir = write_tree(&[
        ("aaa.rs", "fn unrelated() {}\n"),
        ("zzz.rs", "fn target_symbol() {}\n"),
    ]);
    // "aaa" sorts first; seeding the symbol must surface its file first.
    let map = build_map(dir.path(), &["target_symbol"], 2000).expect("map");
    let pos_aaa = map.find("aaa.rs").expect("aaa listed");
    let pos_zzz = map.find("zzz.rs").expect("zzz listed");
    assert!(pos_zzz < pos_aaa, "{map}");
}

#[cfg(unix)]
fn git_repo_with_history() -> Option<(tempfile::TempDir, std::path::PathBuf)> {
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        return None;
    }
    let dir = write_tree(&[
        ("a.rs", "fn a() {}\n"),
        ("b.rs", "fn b() {}\n"),
        ("c.rs", "fn c() {}\n"),
    ]);
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git")
    };
    if !run(&["init", "-q"]).status.success() {
        return None;
    }
    // a+b together twice (signal), c alone (no co-change with a).
    for msg in ["one", "two"] {
        if !run(&["add", "a.rs", "b.rs"]).status.success() {
            return None;
        }
        if !run(&["commit", "-qm", msg]).status.success() {
            return None;
        }
    }
    if !run(&["add", "c.rs"]).status.success() {
        return None;
    }
    if !run(&["commit", "-qm", "three"]).status.success() {
        return None;
    }
    let root = dir.path().to_path_buf();
    Some((dir, root))
}

#[test]
#[cfg(unix)]
fn cochange_pairs_detect_repeated_cocommits() {
    let Some((_dir, root)) = git_repo_with_history() else {
        return;
    };
    let files = vec!["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()];
    let mut pairs = cochange_pairs(&root, &files);
    pairs.sort();
    // a+b twice: edge. c committed once with nobody (bulk add was split):
    // no edge. Note c.rs was added alone, so it shares no commit at all.
    assert_eq!(pairs, vec![(0, 1)], "{pairs:?}");
}

#[test]
#[cfg(unix)]
fn cochange_boosts_base_dependents_end_to_end() {
    let Some((_dir, root)) = git_repo_with_history() else {
        return;
    };
    // Seed a: b (co-changed twice) must outrank c (unrelated).
    let map = build_map(&root, &["a.rs"], 2000).expect("map");
    let pos_b = map.find("b.rs").expect("b listed");
    let pos_c = map.find("c.rs").expect("c listed");
    assert!(pos_b < pos_c, "{map}");
}

#[test]
fn cochange_outside_repo_is_empty() {
    let dir = write_tree(&[("a.rs", "fn a() {}\n")]);
    let files = vec!["a.rs".to_string()];
    assert!(cochange_pairs(dir.path(), &files).is_empty());
}

#[test]
fn empty_tree_yields_no_map() {
    let dir = write_tree(&[("notes.md", "# nothing to parse\n")]);
    assert!(build_map(dir.path(), &[], 2000).is_none());
}
