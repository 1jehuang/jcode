"""Proof that the process-cwd fallback ratchet actually catches what it claims.

The guard shipped once already and was wrong three times in ways that only a
planted violation could have revealed: `--update` silently absorbed a new site
inside an already-grandfathered file, `DOT_RE` matched `unwrap_or(Path::new("."))`
but not `unwrap_or_else(|| Path::new("."))`, and the guard reported its own doc
comments. Each shipped as "the ratchet is on and the tree is green".

It was then wrong a fourth time, in the opposite direction, which none of these
cases could see because none of them moved a site: grandfathered sites were
matched by LINE NUMBER, so adding or removing any line above one reported a
violation that did not exist, and `--update` -- which may only remove -- had no
way to re-record a site that had merely moved. The suite was also
order-dependent, because the guard read its scan roots from a module-level global
in an imported sibling that Python caches under a bare module name, so every
test after the first scanned the first test's already-deleted directory and
passed for the wrong reason.

So the guard needs its own proof the way code needs tests: every forbidden shape
planted in a scratch tree and asserted to be reported, every legitimate shape
asserted left alone, every way to launder the baseline asserted refused, and
every way to move or reformat an already-recorded site asserted NOT to change
the verdict. A regression that blinds the guard fails here instead of sitting
there until a real violation walks past.

Runs the real script as a subprocess rather than importing it, because the
behaviour under test includes argument handling, exit codes, and the messages a
user sees.

Baseline schema (kept in step with `load_baseline`):

    {"version": 1, "grandfathered": {<path>: {"reason": str,
        "sites": [{"line": int, "kind": str, "text": str}]}}}

`line` is a hint for humans, not the identity: sites are matched on
`kind` plus the normalized statement `text`, so moving one is not a new
violation. A site with no `text` is rejected by name rather than silently
reported as an unrecorded site.

Exit codes: 0 clean, 1 violations or a refused `--update`, 2 (SystemExit) a
malformed baseline.

Run: python3 -m unittest -v scripts/test_cwd_fallback_ratchet.py
"""

import importlib.util
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
GUARD = REPO_ROOT / "scripts" / "check_cwd_fallback_budget.py"
BASELINE = REPO_ROOT / "scripts" / "cwd_fallback.json"
# The guard imports its file scanner from here, so the scratch copy needs it too.
PANIC_BUDGET = REPO_ROOT / "scripts" / "check_panic_budget.py"

SCRATCH_SOURCE = "pub fn placeholder() {}\n"


def run_guard(cwd, *args):
    """Run the guard in `cwd` and return (exit_code, combined_output).

    The script under test must be the copy *inside* `cwd`: it derives its repo
    root from its own `__file__`, so running the real one would scan the real
    repository and quietly pass every planted violation.
    """
    proc = subprocess.run(
        [sys.executable, str(cwd / "scripts" / GUARD.name), *args],
        cwd=str(cwd),
        capture_output=True,
        text=True,
    )
    return proc.returncode, proc.stdout + proc.stderr


def load_guard(tmp):
    """Import the guard copy inside `tmp`, with its scanner bound to `tmp` too.

    A plain import is not fresh, for two reasons that only show up once the suite
    runs more than one scratch tree:

    - `sys.modules["check_panic_budget"]` caches under the BARE module name, so
      the first tree imported stays bound for the whole process.
    - the guard does `sys.path.insert(0, ...)` at import time, so each import adds
      another entry and the path list grows without bound.

    Between them, a test that imports the guard normally scans some earlier test's
    directory, which `shutil.rmtree` has already removed -- so it sees no files and
    passes for the wrong reason. Bind the scanner to this tree explicitly first,
    then load the guard by file location under a unique name.
    """
    scripts = tmp / "scripts"
    scanner_spec = importlib.util.spec_from_file_location(
        "check_panic_budget", str(scripts / PANIC_BUDGET.name))
    scanner = importlib.util.module_from_spec(scanner_spec)
    previous = sys.modules.get("check_panic_budget")
    sys.modules["check_panic_budget"] = scanner
    try:
        scanner_spec.loader.exec_module(scanner)
        spec = importlib.util.spec_from_file_location(
            "cwd_guard_under_test", str(scripts / GUARD.name))
        guard = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(guard)
    finally:
        if previous is not None:
            sys.modules["check_panic_budget"] = previous
    return guard


def planted_sites(tmp):
    """Every site the guard currently sees in `src/src.rs`, as baseline entries.

    Asks the guard itself rather than restating the statement, so a test can never
    grandfather text that has drifted from what the guard will compare against.
    """
    hits = load_guard(tmp).current_sites().get("src/src.rs", [])
    return [{"line": h["line"], "kind": h["kind"], "text": h["text"]} for h in hits]


def planted_statement(tmp, kind=None):
    """The single site the guard currently sees in `src/src.rs`, or assert."""
    hits = planted_sites(tmp)
    if kind is not None:
        hits = [h for h in hits if h["kind"] == kind]
    if len(hits) != 1:
        raise AssertionError(
            "expected exactly one %s site in src/src.rs, found %d" % (kind, len(hits)))
    return hits[0]["text"]


class CwdFallbackRatchetTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="cwd-ratchet-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        (self.tmp / "scripts").mkdir(parents=True, exist_ok=True)
        shutil.copy2(GUARD, self.tmp / "scripts" / GUARD.name)
        # The guard imports its scanner from check_panic_budget; without it the
        # copy would fail to import and every case would "pass" for the wrong
        # reason. Copy the real one rather than stubbing it, so the scan scope
        # under test is the real scope.
        shutil.copy2(PANIC_BUDGET, self.tmp / "scripts" / PANIC_BUDGET.name)
        self.plant(SCRATCH_SOURCE)
        # A well-formed but empty baseline, so a test that plants a violation
        # isolates the *matching* logic rather than tripping validation first.
        self.write_baseline({})

    def write_baseline(self, grandfathered):
        (self.tmp / "scripts" / BASELINE.name).write_text(
            json.dumps({"version": 1, "grandfathered": grandfathered}, indent=2) + "\n",
            encoding="utf-8",
        )

    def baseline(self):
        data = json.loads((self.tmp / "scripts" / BASELINE.name).read_text(encoding="utf-8"))
        return data["grandfathered"]

    def plant(self, code, name="src.rs"):
        # Must live under a directory the guard actually scans (`src/` and
        # `crates/`) and must not look like a test file, or the planted violation
        # is skipped and the case passes for the wrong reason.
        (self.tmp / "src").mkdir(parents=True, exist_ok=True)
        (self.tmp / "src" / name).write_text(code, encoding="utf-8")

    def site(self, kind="dot", text=None, line=1):
        """One baseline entry in the shape the guard now stores and matches on.

        Sites are keyed by their normalized statement text, not by their line, so
        a test that grandfathers a site has to record the text too. `text=None`
        reads it off the planted file via the guard itself, so a test never has to
        restate the statement it is grandfathering. Passing `text` explicitly is
        for the cases that need a site that is NOT in the file, which is how a
        stale entry is set up.
        """
        if text is None:
            text = planted_statement(self.tmp, kind)
        return {"line": line, "kind": kind, "text": text}

    def grandfather(self, reason, sites):
        """Grandfather `sites` in `src/src.rs` under one reason."""
        self.write_baseline({"src/src.rs": {"reason": reason, "sites": sites}})

    # --- the shapes AGENTS.md forbids -------------------------------------

    def test_untouched_tree_passes(self):
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, output)

    def test_rejects_unwrap_or_path_new_dot(self):
        self.plant('fn f() -> PathBuf { p.parent().unwrap_or(Path::new(".")) }\n')
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 1, output)
        self.assertIn("src/src.rs:1", output)

    def test_rejects_unwrap_or_else_closure(self):
        # The shape AGENTS.md names explicitly. It was invisible to the first
        # shipped pattern set, which is why this case exists separately.
        self.plant('fn f() -> PathBuf { p.parent().unwrap_or_else(|| Path::new(".")) }\n')
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 1, output)
        self.assertIn("src/src.rs:1", output)

    def test_rejects_pathbuf_from_closure(self):
        self.plant('fn f() -> PathBuf { d.clone().unwrap_or_else(|| PathBuf::from(".")) }\n')
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 1, output)

    def test_known_blind_spot_named_closure(self):
        """A fallback behind a function name is invisible, and that is pinned.

        Not a wish list. A text ratchet cannot see through `rooted_else` to the
        `Path::new(".")` inside it, and matching every `unwrap_or_else(` would flag
        hundreds of legitimate combinators. The guard's own header records this as
        deliberate; this test fails if someone widens the pattern to close the
        hole, so the change has to be a conscious one.
        """
        self.plant("fn f() -> PathBuf { d.clone().unwrap_or_else(rooted_else) }\n")
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, output)

    def test_rejects_fully_qualified_path(self):
        self.plant('fn f() -> PathBuf { p.parent().unwrap_or(std::path::Path::new(".")) }\n')
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 1, output)

    def test_rejects_cwd_as_fallback_value(self):
        self.plant("fn f() -> PathBuf { d.clone().unwrap_or(std::env::current_dir().unwrap()) }\n")
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 1, output)

    def test_rejects_swallowed_cwd_error(self):
        self.plant(
            "fn f() -> PathBuf {\n"
            '    d.clone().unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))\n'
            "}\n"
        )
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 1, output)

    # --- the shapes that must NOT be reported -----------------------------

    def test_allows_propagated_cwd_error(self):
        self.plant("fn f() -> Result<PathBuf> { Ok(std::env::current_dir()?) }\n")
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, output)

    def test_allows_display_string_fallback(self):
        self.plant('fn f() -> String { d.map(|p| p.display().to_string()).unwrap_or_default() }\n')
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, output)

    def test_allows_generic_unwrap_or_default(self):
        # `serde_json::to_string(..).unwrap_or_default()` is everywhere and is not
        # a cwd fallback. A bare `unwrap_or_default()` pattern once flagged all of
        # them, which is why this case pins the narrow behaviour.
        self.plant("fn f() -> String { serde_json::to_string(v).unwrap_or_default() }\n")
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, output)

    def test_allows_int_unwrap_or_else(self):
        self.plant("fn f() -> u32 { n.clone().unwrap_or_else(|_| 7) }\n")
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, output)

    def test_allows_option_unwrap_or(self):
        self.plant("fn f() -> u8 { o.clone().unwrap_or(0) }\n")
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, output)

    def test_allows_cfg_test_block(self):
        self.plant(
            "#[cfg(test)]\nmod tests {\n"
            '    fn f() { let _ = p.parent().unwrap_or(Path::new(".")); }\n}\n'
        )
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, output)

    def test_allows_empty_path_literal(self):
        self.plant('fn f() -> PathBuf { PathBuf::from("") }\n')
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, output)

    def test_ignores_doc_comment_quoting_the_shape(self):
        # The guard reported its own documentation. Prose is not a violation, and
        # over-clearing here costs an allowlist entry while under-clearing lets a
        # real violation through.
        self.plant('/// use `unwrap_or_else(|| Path::new("."))` only in CLI code\nfn f() {}\n')
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, output)

    def test_still_reports_code_before_a_trailing_comment(self):
        # Proves comment stripping did not blind the guard: the violation is on the
        # same line as the comment, ahead of it.
        self.plant('fn f() { let _ = p.parent().unwrap_or(Path::new(".")); } // cwd\n')
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 1, output)

    def test_string_literal_slashes_do_not_truncate(self):
        # `//` inside a string must not be mistaken for the start of a comment.
        self.plant('fn f() { let _ = "http://x"; let _ = p.parent().unwrap_or(Path::new(".")); }\n')
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 1, output)

    # --- laundering the baseline ------------------------------------------

    def test_entry_silences_only_its_own_site(self):
        self.plant('fn f() { let _ = p.parent().unwrap_or(Path::new(".")); }\n')
        self.grandfather("legitimate: the CLI's own cwd", [self.site(kind="dot")])
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, output)

        # A second violation in the same file must still be reported.
        self.plant(
            'fn f() { let _ = p.parent().unwrap_or(Path::new(".")); }\n'
            'fn g() { let _ = q.parent().unwrap_or(Path::new(".")); }\n'
        )
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 1, output)
        self.assertIn("unrecorded site", output)
        self.assertIn("src/src.rs:2", output)
        self.assertEqual(code, 1, output)
        self.assertIn("unrecorded site", output)
        self.assertIn("src/src.rs:2", output)

    def test_update_cannot_add_a_site(self):
        self.plant('fn f() { let _ = p.parent().unwrap_or(Path::new(".")); }\n')
        code, output = run_guard(self.tmp, "--update")
        self.assertEqual(code, 1, output)
        self.assertEqual(self.baseline(), {}, "--update grew the baseline")

    def test_update_cannot_absorb_a_site_into_an_existing_entry(self):
        # The laundering bug: a violation planted inside an already-grandfathered
        # file inherited that file's unrelated reason and `--update` exited 0.
        self.plant("fn f() {}\n")
        # A site recorded for a line that holds nothing: the "used to be here"
        # shape, which is exactly what a shifted or deleted site leaves behind.
        self.grandfather("a real reason for line 1",
                         [self.site(kind="dot", text="fn f() { let _ = gone(); }")])
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, output)

        self.plant(
            "fn f() { let _ = p.parent().unwrap_or(Path::new(\".\")); }\n"
            "fn g() { let _ = q.parent().unwrap_or(Path::new(\".\")); }\n"
        )
        # Grandfather only the first, then check --update refuses to absorb the
        # second into that entry.
        self.grandfather("a real reason for line 1", planted_sites(self.tmp)[:1])
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 1, "setup: the unrecorded second site must be reported")
        code, output = run_guard(self.tmp, "--update")
        self.assertEqual(code, 1, "update absorbed a new site into an existing entry")
        self.assertEqual(len(self.baseline()["src/src.rs"]["sites"]), 1, "update grew the baseline")

    def test_update_only_removes(self):
        # A file that no longer holds a site is a note, and --update drops it.
        self.plant("fn f() {}\n")
        self.grandfather("used to be here",
                         [self.site(kind="dot", text="fn f() { let _ = gone(); }")])
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, output)
        code, output = run_guard(self.tmp, "--update")
        self.assertEqual(code, 0, output)
        self.assertEqual(self.baseline(), {})

    def test_malformed_baselines_are_refused_with_a_message(self):
        cases = {
            "no line number": {"src/src.rs": {"reason": "because", "sites": [{"kind": "dot"}]}},
            "no reason": {"src/src.rs": {"sites": [{"line": 1, "kind": "dot", "text": "t"}]}},
            "site list not a list": {"src/src.rs": {"reason": "because", "sites": {}}},
            "no kind": {"src/src.rs": {"reason": "because", "sites": [{"line": 1, "text": "t"}]}},
            # A site with no text can never match, so without this check every
            # such site would read as a brand new violation instead of naming the
            # real problem, which is a baseline from before content keying.
            "no text": {"src/src.rs": {"reason": "because", "sites": [{"line": 1, "kind": "dot"}]}},
        }
        for label, grandfathered in cases.items():
            with self.subTest(label):
                self.write_baseline(grandfathered)
                code, output = run_guard(self.tmp)
                self.assertNotEqual(code, 0, label)
                self.assertNotIn("Traceback", output, label)
                self.assertIn("error:", output, label)

    def test_a_site_that_moved_is_still_grandfathered(self):
        # The bug this file's schema now guards against: sites used to be matched
        # by LINE NUMBER, so inserting an unrelated line above a recorded site made
        # the guard report a violation that did not exist -- and `--update`, which
        # may only remove, had no way to recover one that had merely moved.
        self.plant('fn f() { let _ = p.parent().unwrap_or(Path::new(".")); }\n')
        self.grandfather("legitimate: the CLI's own cwd", planted_sites(self.tmp))
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, "setup: the recorded site must be clean")

        # Nothing about the fallback changes; only its line number moves.
        self.plant(
            "// an unrelated line added above the site\n"
            "// and another\n"
            'fn f() { let _ = p.parent().unwrap_or(Path::new(".")); }\n'
        )
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, "a site that merely moved was reported: " + output)

    def test_removing_a_line_above_a_site_keeps_it_grandfathered(self):
        # The other direction of the same bug, which a line-number match fails
        # just as badly.
        self.plant(
            "// filler that will go away\n"
            "// another\n"
            'fn f() { let _ = p.parent().unwrap_or(Path::new(".")); }\n'
        )
        self.grandfather("legitimate: the CLI's own cwd", planted_sites(self.tmp))
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, "setup: the recorded site must be clean")

        self.plant('fn f() { let _ = p.parent().unwrap_or(Path::new(".")); }\n')
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, "a site whose line fell was reported: " + output)

    def test_reformatting_a_site_in_place_keeps_it_grandfathered(self):
        # rustfmt splits these statements across lines routinely, and the guard
        # joins a statement before matching, so the recorded text is the joined
        # form. Reformatting must not turn into a phantom violation.
        self.plant(
            "fn f() {\n"
            "    let _ = p\n"
            "        .parent()\n"
            '        .unwrap_or(Path::new("."));\n'
            "}\n"
        )
        self.grandfather("legitimate: the CLI's own cwd", planted_sites(self.tmp))
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, "setup: the recorded site must be clean")

        # Same statement, different layout, and extra blank lines above it.
        self.plant(
            "\n\n"
            "fn f() {\n"
            "    let _ = p.parent()\n"
            '        .unwrap_or(Path::new("."));\n'
            "}\n"
        )
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, "a reformatted site was reported: " + output)

    def test_a_different_site_beside_a_shifted_one_is_still_reported(self):
        # Content keying must not become a blanket amnesty for the file. A NEW
        # fallback next to a grandfathered one that moved is still a violation.
        self.plant('fn f() { let _ = p.parent().unwrap_or(Path::new(".")); }\n')
        self.grandfather("legitimate: the CLI's own cwd", planted_sites(self.tmp))
        self.plant(
            "// shifted down\n"
            'fn f() { let _ = p.parent().unwrap_or(Path::new(".")); }\n'
            'fn g() { let _ = q.parent().unwrap_or(Path::new(".")); }\n'
        )
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 1, "the new site was absorbed: " + output)
        self.assertIn("unrecorded site", output)
        self.assertIn("src/src.rs:3", output)

    def test_a_site_whose_code_changes_is_reported_again(self):
        # Content keying pins the statement, so editing the grandfathered fallback
        # itself invalidates the record rather than riding along on the old text.
        # The edit has to stay a forbidden shape: changing `Path::new(".")` to
        # `Path::new("..")` is NOT a violation, so the guard would report nothing
        # and the test would pass for the wrong reason. Changing the receiver while
        # keeping the bare-cwd shape is the edit under test.
        self.plant('fn f() { let _ = p.parent().unwrap_or(Path::new(".")); }\n')
        self.grandfather("legitimate: the CLI's own cwd", planted_sites(self.tmp))
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, output)

        self.plant('fn f() { let _ = q.other().unwrap_or(Path::new(".")); }\n')
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 1, "an edited site stayed grandfathered: " + output)
        self.assertIn("unrecorded site", output)

    def test_the_guard_scans_the_tree_it_was_pointed_at(self):
        # The scan roots are a parameter, not a read of the imported scanner's
        # module-level `SCAN_ROOTS`. Python caches `check_panic_budget` under a
        # bare module name, so an implicit read binds whichever copy was imported
        # first -- which made this suite order-dependent and made a cross-tree
        # scan crash in `is_test_rust_file`, not report.
        #
        # A second scratch tree holding one file, so "which tree answered" is
        # observable rather than a count. It needs no scripts/ directory: nothing
        # imports from it, it is only handed to the scanner as a path.
        other = Path(tempfile.mkdtemp(prefix="cwd-ratchet-other-"))
        self.addCleanup(shutil.rmtree, other, ignore_errors=True)
        (other / "src").mkdir(parents=True, exist_ok=True)
        (other / "src" / "elsewhere.rs").write_text(
            'fn g() { let _ = q.parent().unwrap_or(Path::new(".")); }\n',
            encoding="utf-8")

        guard = load_guard(self.tmp)
        self.plant('fn f() { let _ = p.parent().unwrap_or(Path::new(".")); }\n')

        # `current_sites` keys by repo-relative posix path, so compare on that.
        self.assertIn(
            "src/src.rs", guard.current_sites(),
            "the guard must scan its own tree")

        # Point the scanner at the other tree: its file must now be the answer.
        from_tree = guard.production_rust_files((other / "src",))
        self.assertEqual(
            [p.name for p in from_tree], ["elsewhere.rs"],
            "production_rust_files ignored the roots it was given")

    def test_whitespace_inside_a_string_is_still_significant(self):
        # The normalizer drops whitespace outside string literals so a reformat
        # cannot fake a new site. It must NOT drop it inside one, or two different
        # paths would collapse onto one identity and a record would cover code it
        # was never written for.
        #
        # Tested at the key, not end to end, because `Path::new(" ")` is not a
        # forbidden shape at all (`classify` returns None for it, as it does for
        # `""` and `"./"`), so an end-to-end version would have nothing to report
        # and would pass without ever reaching the normalizer.
        guard = load_guard(self.tmp)
        self.assertNotEqual(
            guard._key("dot", 'let _ = p.parent().unwrap_or(Path::new(" "));'),
            guard._key("dot", 'let _ = p.parent().unwrap_or(Path::new(""));'),
            "whitespace inside a string literal was collapsed away")
        # A backslash-escaped quote must not end the string early. If it did, the
        # text after it would be read as code and the space there would be
        # stripped, collapsing `f("a\" b")` onto `f("a\"b")`. So the escape being
        # honoured shows up as the two keys staying DIFFERENT.
        self.assertNotEqual(
            guard._key("dot", 'let _ = f("a\\" b");'),
            guard._key("dot", 'let _ = f("a\\"b");'),
            "an escaped quote ended the string early, so string text was stripped")

    def test_baseline_must_be_a_mapping(self):
        (self.tmp / "scripts" / BASELINE.name).write_text('{"grandfathered": []}\n', encoding="utf-8")
        code, output = run_guard(self.tmp)
        self.assertNotEqual(code, 0)
        self.assertNotIn("Traceback", output)
        self.assertIn("error:", output)

    def test_explain_prints_the_reason(self):
        self.grandfather(
            "UNIQUE-REASON-TEXT",
            [{"line": 1, "kind": "dot", "text": "fn f() { let _ = p.parent().unwrap_or(Path::new(\".\")); }"}],
        )
        code, output = run_guard(self.tmp, "--explain", "src/src.rs")
        self.assertEqual(code, 0, output)
        self.assertIn("UNIQUE-REASON-TEXT", output)

    def test_explain_on_an_untracked_file_says_so(self):
        code, output = run_guard(self.tmp, "--explain", "other.rs")
        self.assertEqual(code, 0, output)
        self.assertIn("not grandfathered", output)


class RealBaselineTest(unittest.TestCase):
    """The committed baseline and guard must agree, without a scratch tree."""

    def test_guard_passes_on_the_real_repository(self):
        code, output = run_guard(REPO_ROOT)
        self.assertEqual(code, 0, output)

    def test_every_committed_entry_carries_a_reason(self):
        data = json.loads(BASELINE.read_text(encoding="utf-8"))
        self.assertIsInstance(data["grandfathered"], dict)
        for path, entry in data["grandfathered"].items():
            with self.subTest(path):
                self.assertTrue(entry.get("reason", "").strip(), "an empty reason records nothing")
                for site in entry.get("sites", []):
                    self.assertIsInstance(site.get("line"), int)


if __name__ == "__main__":
    unittest.main()