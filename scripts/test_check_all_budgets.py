#!/usr/bin/env python3
"""Tests for scripts/check_all_budgets.py.

The property under test is the one the script exists for: *every* ratchet runs
and *every* failure is reported, rather than the run stopping at the first
non-zero exit the way six sequential CI steps did.

Every test here uses a stub script, never the real ratchets. A test that ran the
real guards would depend on the tree's current pass/fail state, which is exactly
the thing that has drifted.
"""

from __future__ import annotations

import importlib.util
import io
import shutil
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

MODULE_PATH = Path(__file__).resolve().parent / "check_all_budgets.py"
_spec = importlib.util.spec_from_file_location("check_all_budgets", MODULE_PATH)
assert _spec is not None and _spec.loader is not None, "cannot load module"
mod = importlib.util.module_from_spec(_spec)
# @dataclass resolves annotations through sys.modules[cls.__module__], so the
# module must be registered before exec_module runs or it raises AttributeError.
sys.modules[_spec.name] = mod
_spec.loader.exec_module(mod)

Guard = mod.Guard
Result = mod.Result


def slug_name(filename: str) -> str:
    """`check_code_size_budget.py` -> `code_size` (the middle slug).

    Slices the *stem*, not the filename. Slicing the filename with
    `[-len("_budget"):]` silently keeps `.py`, producing a stub named
    `check_code_size.py_budget.py` that no discovery path will ever look for --
    which then shows up as "ratchet script missing" rather than as the test bug
    it is.
    """
    stem = Path(filename).stem
    assert stem.startswith("check_") and stem.endswith("_budget"), stem
    return stem[len("check_"): -len("_budget")]


def make_stub(directory: Path, name: str, exit_code: int, output: str) -> Path:
    """Write a ratchet-shaped stub that exits `exit_code` and prints `output`."""
    path = directory / f"check_{name}_budget.py"
    path.write_text(
        "#!/usr/bin/env python3\n"
        "import sys\n"
        f"sys.stdout.write({output!r})\n"
        f"sys.stderr.write('stderr from {name}')\n"
        f"sys.exit({exit_code})\n",
        encoding="utf-8",
    )
    return path


class GuardNameTests(unittest.TestCase):
    def test_derives_readable_name_from_filename(self):
        self.assertEqual(
            mod.guard_name(Path("check_swallowed_error_budget.py")),
            "swallowed error",
        )
        self.assertEqual(mod.guard_name(Path("check_panic_budget.py")), "panic")

    def test_rejects_filename_it_cannot_label(self):
        # Silently inventing a label for an unexpected name would hide a
        # renamed or new script from the report.
        for bad in ["check_thing.py", "budget_check.py", "thing_budget.py"]:
            with self.subTest(name=bad):
                with self.assertRaises(ValueError):
                    mod.guard_name(Path(bad))


class GuardsDiscoveryTests(unittest.TestCase):
    def test_all_listed_scripts_exist_in_the_repo(self):
        names = [g.name for g in mod.guards()]
        self.assertEqual(len(names), len(mod.RATCHET_SCRIPTS))
        self.assertIn("panic", names)
        self.assertIn("cwd fallback", names)

    def test_missing_script_raises_rather_than_running_a_subset(self):
        # The whole point: a guard that silently stops being checked is the
        # failure mode this script was written to prevent.
        with tempfile.TemporaryDirectory() as tmp:
            empty = Path(tmp)
            with self.assertRaises(FileNotFoundError) as ctx:
                mod.guards(empty)
        self.assertIn("budget that is not checked", str(ctx.exception))

    def test_discovery_order_matches_the_declared_order(self):
        names = [Path(s).stem for s in mod.RATCHET_SCRIPTS]
        self.assertEqual(names[0], "check_code_size_budget")
        self.assertIn("check_cwd_fallback_budget", names)


class NoEarlyExitTests(unittest.TestCase):
    """The regression tests for the actual bug."""

    def test_every_guard_runs_even_when_the_first_one_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            first = make_stub(d, "alpha", 1, "alpha failed\n")
            second = make_stub(d, "beta", 0, "beta ok\n")
            guards = [
                Guard(mod.guard_name(first), first),
                Guard(mod.guard_name(second), second),
            ]
            results = mod.run_all(guards)

        # Not just "two results came back": the *second* stub has a side effect
        # only a real execution produces.
        self.assertEqual([r.guard.name for r in results], ["alpha", "beta"])
        self.assertEqual([r.returncode for r in results], [1, 0])
        self.assertFalse(results[0].passed)
        self.assertTrue(results[1].passed)
        self.assertIn("alpha failed", results[0].output)
        self.assertIn("stderr from alpha", results[0].output)

    def test_report_names_every_failing_guard_not_just_the_first(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            a = make_stub(d, "alpha", 1, "a\n")
            b = make_stub(d, "beta", 1, "b\n")
            c = make_stub(d, "gamma", 0, "c\n")
            results = mod.run_all(
                [
                    Guard(mod.guard_name(a), a),
                    Guard(mod.guard_name(b), b),
                    Guard(mod.guard_name(c), c),
                ]
            )
            buf = io.StringIO()
            failures = mod.report(results, stream=buf)
            text = buf.getvalue()

        self.assertEqual([f.guard.name for f in failures], ["alpha", "beta"])
        self.assertIn("PASS  gamma", text)
        for label in ("FAIL  alpha", "FAIL  beta", "PASS  gamma"):
            self.assertIn(label, text)
        # A failing gate's own output must be visible, indented under it.
        self.assertIn("      a", text)

    def test_report_shows_pass_line_for_a_passing_guard(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            a = make_stub(d, "alpha", 0, "fine\n")
            buf = io.StringIO()
            failures = mod.report(mod.run_all([Guard(mod.guard_name(a), a)]),
                                  stream=buf)
        self.assertEqual(failures, [])
        self.assertIn("PASS  alpha", buf.getvalue())


class OutputTruncationTests(unittest.TestCase):
    def test_long_output_is_truncated_with_a_marker(self):
        text = "\n".join("line %d" % i for i in range(200))
        out = mod._tail(text, limit=5)
        self.assertIn("(195 earlier line(s) omitted)", out)
        self.assertIn("line 199", out)
        self.assertNotIn("line 0", out)

    def test_short_output_is_untouched(self):
        self.assertEqual(mod._tail("a\nb\n", limit=20), "a\nb")


class CommandTests(unittest.TestCase):
    def test_update_flag_is_added_only_when_requested(self):
        g = Guard("panic", Path("check_panic_budget.py"))
        self.assertNotIn("--update", g.command(update=False))
        self.assertIn("--update", g.command(update=True))

    def test_uses_the_running_interpreter(self):
        # sys.executable, not a bare "python3": the guards must run under the
        # same interpreter this script did, or a missing python3 on PATH turns
        # into a confusing launch failure.
        g = Guard("panic", Path("check_panic_budget.py"))
        self.assertEqual(g.command(False)[0], sys.executable)


class LaunchFailureTests(unittest.TestCase):
    def test_oserror_becomes_a_result_not_an_exception(self):
        # A guard that cannot start must still be reported and must still make
        # the overall run fail, rather than aborting the remaining guards.
        # Patched rather than provoked with a real bad path: sys.executable
        # exists, so a missing script yields a non-zero exit (2, "can't open
        # file"), not the OSError this branch exists to handle.
        boom = OSError(2, "No such file or directory", "python3")
        with mock.patch.object(mod.subprocess, "run", side_effect=boom):
            res = mod.run_one(Guard("panic", Path("x_budget.py")), update=False)
        self.assertFalse(res.passed)
        self.assertEqual(res.returncode, 127)
        self.assertIn("failed to launch", res.output)
        self.assertIn("No such file", res.output)

    def test_launch_failure_does_not_abort_the_remaining_guards(self):
        boom = OSError(2, "nope")
        good = mod.Guard("ok", Path("check_ok_budget.py"))
        with tempfile.TemporaryDirectory() as tmp:
            stub = make_stub(Path(tmp), "ok", 0, "ok ran\n")
            good = mod.Guard("ok", stub)
            calls = []

            def fake(cmd, **kw):
                calls.append(cmd)
                if len(calls) == 1:
                    raise boom
                return subprocess.CompletedProcess(
                    cmd, 0, stdout="ok ran\n", stderr=""
                )

            with mock.patch.object(mod.subprocess, "run", side_effect=fake):
                results = mod.run_all(
                    [mod.Guard("gone", Path("check_gone_budget.py")), good]
                )
        self.assertEqual(len(results), 2)
        self.assertFalse(results[0].passed)
        self.assertTrue(results[1].passed)

class MainExitCodeTests(unittest.TestCase):
    """End-to-end through main(), against stub ratchets.

    These call main() in-process rather than spawning `python3
    check_all_budgets.py`: a subprocess would re-import the module with the real
    SCRIPT_DIR and run the actual ratchets, so a test asserting "6 gates ran"
    would really be asserting something about the tree's current drift state.
    """

    def _stub_dir(self, exit_codes: dict[str, int]) -> Path:
        """Build a temp scripts dir containing every declared ratchet."""
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp, True)
        scripts = Path(tmp) / "scripts"
        scripts.mkdir()
        self.saved_script_dir = mod.SCRIPT_DIR
        self.addCleanup(self._restore_script_dir)
        mod.SCRIPT_DIR = scripts
        for slug in mod.RATCHET_SCRIPTS:
            name = slug_name(slug)
            make_stub(scripts, name, exit_codes.get(name, 0), f"{name} ran\n")
        return scripts

    def _restore_script_dir(self) -> None:
        mod.SCRIPT_DIR = self.saved_script_dir

    def _main(self, *argv: str) -> tuple[int, str]:
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = mod.main(list(argv))
        return code, out.getvalue() + err.getvalue()

    def test_every_gate_reports_its_own_result(self):
        self._stub_dir({})
        code, text = self._main()
        self.assertEqual(code, 0, text)
        self.assertIn("All 6 budget gates passed.", text)
        # Compare against the module's own label derivation rather than
        # re-deriving it here: a second copy of the naming rule is a second
        # thing that can be wrong, and this one already was.
        for guard in mod.guards():
            self.assertIn(f"PASS  {guard.name}", text)

    def test_all_failures_reported_not_just_the_first(self):
        # The core regression. An implementation that stopped at the first
        # non-zero exit would exit 1 but report one gate.
        self._stub_dir({"panic": 1, "swallowed_error": 1, "code_size": 1})
        code, text = self._main()
        self.assertEqual(code, 1, text)
        self.assertIn("3 of 6 budget gate(s) failed", text)
        for name in ("panic", "swallowed error", "code size"):
            self.assertIn(f"FAIL  {name}", text)
        self.assertIn("3 of 6", text)
        # And the ones that passed still say so.
        self.assertIn("PASS  test size", text)

    def test_single_failure_is_reported_with_its_output(self):
        self._stub_dir({"cwd_fallback": 1})
        code, text = self._main()
        self.assertEqual(code, 1, text)
        self.assertIn("1 of 6 budget gate(s) failed", text)
        self.assertIn("FAIL  cwd fallback", text)
        self.assertIn("cwd_fallback ran", text)

    def test_missing_script_is_a_hard_error_not_a_partial_run(self):
        self._stub_dir({})
        # Delete one declared ratchet behind guards()' back.
        (mod.SCRIPT_DIR / mod.RATCHET_SCRIPTS[2]).unlink()
        code, text = self._main()
        self.assertEqual(code, 2, text)
        self.assertIn("ratchet script missing", text)
        self.assertIn("not checked is not a budget", text)
        # No gate results printed: it refused rather than reporting a subset.
        self.assertNotIn("PASS  ", text)
        self.assertNotIn("FAIL  ", text)

    def test_list_runs_nothing(self):
        scripts = self._stub_dir({})
        for slug in mod.RATCHET_SCRIPTS:
            (scripts / slug).write_text(
                "import sys\nsys.stdout.write('EXECUTED')\nsys.exit(3)\n",
                encoding="utf-8",
            )
        code, text = self._main("--list")
        self.assertEqual(code, 0, text)
        self.assertNotIn("EXECUTED", text)
        self.assertEqual(len(text.splitlines()), len(mod.RATCHET_SCRIPTS))

    def test_update_flag_reaches_every_ratchet(self):
        scripts = self._stub_dir({})
        log = mod.SCRIPT_DIR.parent / "argv.log"
        recorder = (
            "import sys\n"
            f"open({str(log)!r}, 'a').write(sys.argv[1].split('|')[-1] + '\\n')\n"
            "sys.exit(0)\n"
        )
        for slug in mod.RATCHET_SCRIPTS:
            (scripts / slug).write_text(recorder, encoding="utf-8")
        code, text = self._main("--update")
        self.assertEqual(code, 0, text)
        recorded = log.read_text(encoding="utf-8").split()
        self.assertEqual(len(recorded), len(mod.RATCHET_SCRIPTS))
        self.assertTrue(all(t == "--update" for t in recorded), recorded)


if __name__ == "__main__":
    unittest.main()
