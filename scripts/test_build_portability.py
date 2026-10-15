#!/usr/bin/env python3
"""Build only a dependency-free throwaway crate, never the Jcode workspace.

DEV_CARGO_TEST_BASH selects the shell under test, e.g. /bin/bash on macOS.
"""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parent.parent
BASH = os.environ.get("DEV_CARGO_TEST_BASH", "/bin/bash")


class BuildPortabilityTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=os.environ.get("JCODE_SCRATCH_DIR"))
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        for directory in ["scripts", ".cargo", "src", "bin"]:
            (self.root / directory).mkdir()
        for name in ["dev_cargo.sh", "remote_config.sh", "rustc-parallel-frontend"]:
            shutil.copy2(REPO / "scripts" / name, self.root / "scripts" / name)
        shutil.copy2(REPO / ".cargo/config.toml", self.root / ".cargo/config.toml")
        (self.root / "Cargo.toml").write_text(
            '[package]\nname="portability_probe"\nversion="0.1.0"\nedition="2021"\n'
        )
        (self.root / "src/lib.rs").write_text("pub fn probe() -> u8 { 42 }\n")
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(
            ("JCODE_", "CARGO_", "RUSTC_", "SCCACHE_", "BASH_FUNC_"))}
        self.env.update(
            PATH=str(self.root / "bin") + os.pathsep + os.environ["PATH"],
            JCODE_REMOTE_CONFIG=str(self.root / "absent"), JCODE_REMOTE_CARGO="0",
            JCODE_CARGO_NICED="1", JCODE_BUILD_GIT_HASH="fixture",
            JCODE_SCCACHE="off", JCODE_BUILD_JOBS="2", JCODE_FAST_LINKER="off",
            JCODE_RUST_ACTION_LOG_PATH=str(self.root / "actions.jsonl"),
            JCODE_CARGO_GATE_PATH=str(self.root / "gate.lock"),
        )
        self.script("cargo", 'printf "%s\\n" "${RUSTC_WRAPPER-UNSET}" > "$PROBE_WRAPPER"\n'
                    'printf "%s\\n" "${CARGO_BUILD_JOBS-UNSET}" > "$PROBE_JOBS"\n'
                    'echo cargo-ran\nexit "${PROBE_EXIT:-0}"\n')
        self.script("flock", 'echo "$*" >> "$PROBE_FLOCK"\n'
                    '[ "${PROBE_CONTENDED:-0}" = 1 ] && [ "$1" = -n ] && exit 1\n'
                    'exit 0\n')
        self.env.update(PROBE_WRAPPER=str(self.root / "wrapper"),
                        PROBE_JOBS=str(self.root / "jobs"),
                        PROBE_FLOCK=str(self.root / "flock.log"))

    def script(self, name, body):
        path = self.root / "bin" / name
        path.write_text("#!/bin/sh\n" + body)
        path.chmod(0o755)

    def dev(self, *args, **env):
        return subprocess.run([BASH, str(self.root / "scripts/dev_cargo.sh"), *args],
                              cwd=self.root, env=dict(self.env, **env),
                              capture_output=True, text=True, timeout=30)

    def test_gate_reaches_cargo(self):
        result = self.dev("check")
        self.assertEqual((result.returncode, "cargo-ran" in result.stdout), (0, True), result.stderr)
        self.assertEqual((self.root / "flock.log").read_text().strip(), "-n 9")

    def test_contended_gate_waits_then_runs(self):
        result = self.dev("check", PROBE_CONTENDED="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("-w 30 9", (self.root / "flock.log").read_text())

    def test_cargo_failure_is_logged_and_propagated(self):
        result = self.dev("check", PROBE_EXIT="42")
        self.assertEqual(result.returncode, 42, result.stderr)
        self.assertEqual(json.loads((self.root / "actions.jsonl").read_text())["exit_code"], 42)

    def test_gate_open_failure_does_not_report_success(self):
        result = self.dev("check", JCODE_CARGO_GATE_PATH=str(self.root))
        self.assertNotEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.root / "wrapper").exists())

    def test_explicit_wrapper_is_preserved(self):
        for wrapper in ["/custom/sccache", "/custom/wrapper", ""]:
            with self.subTest(wrapper=wrapper):
                result = self.dev("check", RUSTC_WRAPPER=wrapper, JCODE_SCCACHE="auto")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual((self.root / "wrapper").read_text().strip(), wrapper)

    def test_config_env_wrapper_is_preserved(self):
        for wrapper in ["/custom/wrapper", ""]:
            with self.subTest(wrapper=wrapper):
                result = self.dev("check", CARGO_BUILD_RUSTC_WRAPPER=wrapper)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual((self.root / "wrapper").read_text().strip(), "UNSET")

    def test_dev_path_retains_parallel_frontend(self):
        result = self.dev("check")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / "wrapper").read_text().strip(),
                         str(self.root / "scripts/rustc-parallel-frontend"))
        self.assertEqual((self.root / "jobs").read_text().strip(), "2")

    def test_filtered_zero_tests_fail(self):
        self.script("cargo", 'echo "running 0 tests"\n')
        self.assertEqual(self.dev("test", "missing_filter").returncode, 97)

    def test_filtered_test_failure_is_preserved(self):
        self.script("cargo", 'echo "running 1 tests"\nexit 43\n')
        self.assertEqual(self.dev("test", "some_filter").returncode, 43)

    def test_non_build_action_skips_gate(self):
        result = self.dev("metadata")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.root / "flock.log").exists())

    def test_real_gate_serializes_competing_wrappers(self):
        if shutil.which("flock") is None:
            self.skipTest("real flock is not installed")
        (self.root / "bin/flock").unlink()
        self.script("cargo", 'mkdir "$PROBE_WRAPPER.active" || exit 44\n'
                    'sleep 1\nrmdir "$PROBE_WRAPPER.active"\n')
        args = [BASH, str(self.root / "scripts/dev_cargo.sh"), "check"]
        first = subprocess.Popen(args, cwd=self.root, env=self.env,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        second = subprocess.Popen(args, cwd=self.root, env=self.env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            first_output = first.communicate(timeout=20)
            second_output = second.communicate(timeout=20)
        finally:
            for process in [first, second]:
                if process.poll() is None:
                    process.kill()
                    process.communicate()
        self.assertEqual((first.returncode, second.returncode), (0, 0),
                         (first_output, second_output))

    def test_dev_path_runs_real_tiny_crate(self):
        (self.root / "bin/cargo").unlink()
        result = self.dev("check", "--offline", JCODE_FRONTEND_THREADS="2")
        self.assertEqual(result.returncode, 0, result.stderr)
        (self.root / "src/lib.rs").write_text("this is not valid Rust\n")
        result = self.dev("check", "--offline", JCODE_FRONTEND_THREADS="2")
        self.assertEqual(result.returncode, 101, result.stderr)

    def test_bsd_date_fallback_keeps_failure_status(self):
        self.script("date", 'case "$1" in\n'
                    '  +%s%N) echo 1700000000N ;;\n'
                    '  +%s) echo 1700000000 ;;\n'
                    '  *) echo 2023-11-14T22:13:20Z ;;\nesac\n')
        result = self.dev("check", PROBE_EXIT="42", PROBE_CONTENDED="1")
        self.assertEqual(result.returncode, 42, result.stderr)
        record = json.loads((self.root / "actions.jsonl").read_text())
        self.assertEqual((record["duration_ms"], record["gate_wait_ms"]), (0, 0))

    def test_disabled_and_inherited_gates_skip_lock(self):
        for env in [{"JCODE_CARGO_GATE": "off"}, {"JCODE_CARGO_GATE_HELD": "1"}]:
            with self.subTest(env=env):
                result = self.dev("check", **env)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertFalse((self.root / "flock.log").exists())

    def test_plain_cargo_has_no_shell_wrapper_and_opt_in_works(self):
        cargo = shutil.which("cargo")
        self.assertIsNotNone(cargo)
        for opt_in in [False, True]:
            with self.subTest(opt_in=opt_in):
                env = dict(self.env, CARGO_TARGET_DIR=str(self.root / ("threaded" if opt_in else "plain")))
                if opt_in:
                    env["RUSTC_WRAPPER"] = str(self.root / "scripts/rustc-parallel-frontend")
                    env["JCODE_FRONTEND_THREADS"] = "2"
                result = subprocess.run([cargo, "check", "--offline", "-vv"], cwd=self.root,
                                        env=env, capture_output=True, text=True, timeout=60)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual("rustc-parallel-frontend" in result.stderr, opt_in, result.stderr)

    def test_posix_wrapper_threads_and_passthrough(self):
        self.script("rustc-probe", 'printf "%s\\n" "${RUSTC_BOOTSTRAP-UNSET}" "$@"\n')
        for mode, optimized, expected in [("auto", False, True), ("0", False, False), ("auto", True, False)]:
            with self.subTest(mode=mode, optimized=optimized):
                result = subprocess.run(
                    ["/bin/sh", str(self.root / "scripts/rustc-parallel-frontend"),
                     str(self.root / "bin/rustc-probe"), "--crate-name", "probe",
                     "-C", "opt-level=" + ("3" if optimized else "0")],
                    env=dict(self.env, CARGO_CRATE_NAME="probe", CARGO_MANIFEST_DIR=str(self.root),
                             JCODE_PARALLEL_FRONTEND=mode, JCODE_FRONTEND_THREADS="2"),
                    capture_output=True, text=True, check=True)
                self.assertEqual("-Zthreads=2" in result.stdout, expected)
                self.assertEqual(result.stdout.splitlines()[0], "1" if expected else "UNSET")


if __name__ == "__main__":
    unittest.main()
