"""Proof that the process-cwd fallback ratchet actually catches what it claims.

The guard shipped once already and was wrong three times in ways that only a
planted violation could have revealed: `--update` silently absorbed a new site
inside an already-grandfathered file, `DOT_RE` matched `unwrap_or(Path::new("."))`
but not `unwrap_or_else(|| Path::new("."))`, and the guard reported its own doc
comments. Each shipped as "the ratchet is on and the tree is green".

So the guard needs its own proof the way code needs tests: every forbidden shape
planted in a scratch tree and asserted to be reported, every legitimate shape
asserted left alone, and every way to launder the baseline asserted refused. A
regression that blinds the guard fails here instead of sitting there until a real
violation walks past.

Runs the real script as a subprocess rather than importing it, because the
behaviour under test includes argument handling, exit codes, and the messages a
user sees.

Baseline schema (kept in step with `load_baseline`):

    {"version": 1, "grandfathered": {<path>: {"reason": str, "sites": [{"line": int, "kind": str}]}}}

Exit codes: 0 clean, 1 violations or a refused `--update`, 2 (SystemExit) a
malformed baseline.

Run: python3 -m unittest -v scripts/test_cwd_fallback_ratchet.py
"""

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

    def test_entry_silences_only_its_own_line(self):
        self.plant('fn f() { let _ = p.parent().unwrap_or(Path::new(".")); }\n')
        self.write_baseline(
            {"src/src.rs": {"reason": "legitimate: the CLI's own cwd", "sites": [{"line": 1, "kind": "dot"}]}}
        )
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

    def test_update_cannot_add_a_site(self):
        self.plant('fn f() { let _ = p.parent().unwrap_or(Path::new(".")); }\n')
        code, output = run_guard(self.tmp, "--update")
        self.assertEqual(code, 1, output)
        self.assertEqual(self.baseline(), {}, "--update grew the baseline")

    def test_update_cannot_absorb_a_site_into_an_existing_entry(self):
        # The laundering bug: a violation planted inside an already-grandfathered
        # file inherited that file's unrelated reason and `--update` exited 0.
        self.plant("fn f() {}\n")
        self.write_baseline(
            {"src/src.rs": {"reason": "a real reason for line 1", "sites": [{"line": 1, "kind": "dot"}]}}
        )
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, output)

        self.plant(
            "fn f() { let _ = p.parent().unwrap_or(Path::new(\".\")); }\n"
            "fn g() { let _ = q.parent().unwrap_or(Path::new(\".\")); }\n"
        )
        code, output = run_guard(self.tmp, "--update")
        self.assertEqual(code, 1, "update absorbed a new site into an existing entry")
        self.assertEqual(len(self.baseline()["src/src.rs"]["sites"]), 1, "update grew the baseline")

    def test_update_only_removes(self):
        # A file that no longer holds a site is a note, and --update drops it.
        self.plant("fn f() {}\n")
        self.write_baseline(
            {"src/src.rs": {"reason": "used to be here", "sites": [{"line": 1, "kind": "dot"}]}}
        )
        code, output = run_guard(self.tmp)
        self.assertEqual(code, 0, output)
        code, output = run_guard(self.tmp, "--update")
        self.assertEqual(code, 0, output)
        self.assertEqual(self.baseline(), {})

    def test_malformed_baselines_are_refused_with_a_message(self):
        cases = {
            "no line number": {"src/src.rs": {"reason": "because", "sites": [{"kind": "dot"}]}},
            "no reason": {"src/src.rs": {"sites": [{"line": 1, "kind": "dot"}]}},
            "site list not a list": {"src/src.rs": {"reason": "because", "sites": {}}},
        }
        for label, grandfathered in cases.items():
            with self.subTest(label):
                self.write_baseline(grandfathered)
                code, output = run_guard(self.tmp)
                self.assertNotEqual(code, 0, label)
                self.assertNotIn("Traceback", output, label)
                self.assertIn("error:", output, label)

    def test_baseline_must_be_a_mapping(self):
        (self.tmp / "scripts" / BASELINE.name).write_text('{"grandfathered": []}\n', encoding="utf-8")
        code, output = run_guard(self.tmp)
        self.assertNotEqual(code, 0)
        self.assertNotIn("Traceback", output)
        self.assertIn("error:", output)

    def test_explain_prints_the_reason(self):
        self.write_baseline(
            {"src/src.rs": {"reason": "UNIQUE-REASON-TEXT", "sites": [{"line": 1, "kind": "dot"}]}}
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