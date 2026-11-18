"""Regression tests for the isolated server memory profiler, no model calls."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch, MagicMock

spec = importlib.util.spec_from_file_location("profile_server_memory", Path(__file__).with_name("profile_server_memory.py"))
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)


class ProfileTests(unittest.TestCase):
    def test_environment_strips_all_inherited_jcode_routes(self):
        env = profile.isolated_env(Path("/private"), {
            "JCODE_SOCKET": "real.sock", "JCODE_HOME": "/real", "JCODE_SESSION_ID": "real",
            "JCODE_CONFIG": "/real/config", "JCODE_UNKNOWN_FUTURE_ROUTE": "bad", "PATH": "/bin"})
        self.assertEqual(env["JCODE_HOME"], "/private/home")
        self.assertEqual(env["JCODE_RUNTIME_DIR"], "/private/run")
        self.assertEqual(env["TMPDIR"], "/private/tmp")
        self.assertEqual(env["TMP"], "/private/tmp")
        self.assertEqual(env["TEMP"], "/private/tmp")
        self.assertEqual(env["JCODE_TEST_SESSION"], "1")
        self.assertEqual(env["PATH"], "/bin")
        self.assertNotIn("JCODE_SOCKET", env)
        self.assertNotIn("JCODE_SESSION_ID", env)
        self.assertNotIn("JCODE_UNKNOWN_FUTURE_ROUTE", env)
        self.assertNotIn("JCODE_CONFIG", env)

    def test_startup_heap_sampling_environment(self):
        env = profile.isolated_env(Path("/private"), {"MALLOC_CONF": "prof:false"}, True)
        self.assertEqual(env["MALLOC_CONF"], "prof:true,prof_active:true,prof_accum:true,lg_prof_sample:16")
        self.assertEqual(env["_RJEM_MALLOC_CONF"], env["MALLOC_CONF"])

    def test_auth_is_independent_private_copy_without_config(self):
        with tempfile.TemporaryDirectory() as root:
            source, dest = Path(root) / "source", Path(root) / "dest"
            source.mkdir()
            dest.mkdir()
            (source / "auth.json").write_text("secret")
            (source / "config.toml").write_text("real config")
            self.assertEqual(profile.copy_auth(source, dest), ["auth.json"])
            self.assertFalse((dest / "auth.json").is_symlink())
            self.assertEqual((dest / "auth.json").stat().st_mode & 0o777, 0o600)
            (dest / "auth.json").write_text("refreshed")
            self.assertEqual((source / "auth.json").read_text(), "secret")
            self.assertFalse((dest / "config.toml").exists())

    def test_population_strict(self):
        member = {"session_id": "one", "is_headless": True, "live_attachments": 0, "status": "idle"}
        profile.validate_population({"session_count": 0}, [], None)
        profile.validate_population({"session_count": 1}, [member], "one")
        for info, members in (({"session_count": 2}, [member]),
                              ({"session_count": 1}, []),
                              ({"session_count": 1}, [dict(member, is_headless=False)]),
                              ({"session_count": 1}, [dict(member, live_attachments=1)]),
                              ({"session_count": 1}, [dict(member, session_id="other")])):
            with self.assertRaises(RuntimeError):
                profile.validate_population(info, members, "one")

    def test_smaps_server_pid_only(self):
        sample = profile.smaps(os.getpid())
        self.assertEqual(sample["pid"], os.getpid())
        self.assertEqual(sample["scope"], "server_only")
        full = profile.full_smaps(os.getpid())
        self.assertEqual(full["pid"], os.getpid())
        self.assertIn("Size:", full["raw"])
        self.assertGreater(sample["values_kib"]["Pss"], 0)

    def test_pricing_cache_artifact_preserves_prices_and_counts(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            home, output = root / "home", root / "output"
            output.mkdir()
            self.assertEqual(profile.capture_pricing_cache(home, output), {"available": False})
            (home / "cache").mkdir(parents=True)
            cache = {"cached_at_unix_secs": 123, "providers": {
                "future-provider": {"future-model": {"input_usd_per_mtok": 1.25}},
                "other": {"one": {}, "two": {}}}}
            profile.save(home / "cache" / "models_dev_pricing.json", cache)
            self.assertEqual(profile.capture_pricing_cache(home, output), {
                "available": True, "providers": 2, "models": 3, "cached_at_unix_secs": 123})
            self.assertEqual(json.loads((output / "pricing_catalog.json").read_text()), cache)

    def test_failure_preserves_manifest_and_stops_server(self):
        with tempfile.TemporaryDirectory() as temp:
            args = argparse.Namespace(output=Path(temp) / "artifacts", binary=Path("/fake/jcode"),
                                      cwd=Path(temp), turns=1, prompt="done", model=None,
                                      settle_seconds=0, startup_settle_seconds=0, heap_profile=False,
                                      require_pricing_refresh=False,
                                      timeout=1, auth_from=Path(temp) / "noauth")
            proc = MagicMock(pid=99999999)
            with patch.object(profile.subprocess, "run", return_value=MagicMock(stdout="jcode test-hash")), \
                 patch.object(profile.subprocess, "Popen", return_value=proc), \
                 patch.object(profile, "debug_cmd", side_effect=RuntimeError("provider failed")), \
                 patch.object(profile.Path, "exists", return_value=True), \
                 patch.object(profile.os, "killpg") as kill:
                with self.assertRaisesRegex(RuntimeError, "provider failed"):
                    profile.run(args)
            manifest = json.loads((args.output / "profile.json").read_text())
            self.assertEqual(manifest["status"], "failed")
            self.assertIn("provider failed", manifest["error"])
            kill.assert_called_once()
            proc.wait.assert_called_once()

    def test_required_pricing_refresh_timeout_stops_server_before_sampling(self):
        with tempfile.TemporaryDirectory() as temp:
            args = argparse.Namespace(output=Path(temp) / "artifacts", binary=Path("/fake/jcode"),
                                      cwd=Path(temp), turns=0, prompt="done", model=None,
                                      settle_seconds=0, startup_settle_seconds=0, heap_profile=False,
                                      require_pricing_refresh=True,
                                      timeout=1, auth_from=Path(temp) / "noauth")
            proc = MagicMock(pid=99999999)
            proc.poll.return_value = None
            with patch.object(profile.subprocess, "run", return_value=MagicMock(stdout="jcode test")), \
                 patch.object(profile.subprocess, "Popen", return_value=proc), \
                 patch.object(profile, "debug_cmd") as command, \
                 patch.object(profile.Path, "exists", return_value=True), \
                 patch.object(profile.time, "monotonic", side_effect=[0, 10, 12]), \
                 patch.object(profile.os, "killpg") as kill:
                with self.assertRaisesRegex(TimeoutError, "fresh pricing catalog not published"):
                    profile.run(args)
            manifest = json.loads((args.output / "profile.json").read_text())
            self.assertEqual(manifest["status"], "failed")
            self.assertEqual(manifest["stages"], [])
            command.assert_not_called()
            kill.assert_called_once()

    def test_stages_and_explicit_purge_order(self):
        with tempfile.TemporaryDirectory() as temp:
            args = argparse.Namespace(output=Path(temp) / "artifacts", binary=Path("/fake/jcode"),
                                      cwd=Path(temp), turns=2, prompt="done", model=None,
                                      settle_seconds=0, startup_settle_seconds=0, heap_profile=True,
                                      require_pricing_refresh=False,
                                      timeout=1, auth_from=Path(temp) / "noauth")
            calls = []
            created = False

            def command(sock, cmd, sid, timeout):
                nonlocal created
                calls.append(cmd)
                if cmd.startswith("allocator:profile:dump "):
                    self.assertTrue(created, "heap dumps must not run before sid creation")
                    self.assertEqual(sid, "one")
                    Path(cmd.removeprefix("allocator:profile:dump ")).write_text("heap_v2/test")
                if cmd.startswith("create_session:"):
                    created = True
                    return json.dumps({"session_id": "one"})
                if cmd == "server:info":
                    return json.dumps({"session_count": int(created)})
                if cmd == "swarm:members":
                    return json.dumps([{"session_id": "one", "is_headless": True,
                                        "live_attachments": 0, "status": "idle"}] if created else [])
                if cmd == "server:memory":
                    return json.dumps({"process": {"allocator": {"kind": "test"}}})
                return "{}"

            proc = MagicMock(pid=99999999)
            with patch.object(profile.subprocess, "run", return_value=MagicMock(stdout="jcode test-hash")), \
                 patch.object(profile.subprocess, "Popen", return_value=proc) as popen, \
                 patch.object(profile, "full_smaps", return_value={"pid": proc.pid, "raw": "mapping"}), \
                 patch.object(profile, "debug_cmd", side_effect=command), \
                 patch.object(profile.Path, "exists", return_value=True), \
                 patch.object(profile, "smaps", return_value={"values_kib": {"Pss": 123}}), \
                 patch.object(profile.os, "killpg"):
                profile.run(args)
            result = json.loads((args.output / "profile.json").read_text())
            self.assertEqual(result["status"], "complete")
            self.assertEqual(result["binary_version"], "jcode test-hash")
            self.assertEqual(result["startup_settle_seconds"], 0)
            self.assertTrue(result["isolation"]["private_tmpdir"])
            self.assertEqual(result["stages"], ["startup", "session_idle", "turn_001", "turn_002",
                                                "settled_idle", "before_explicit_purge", "after_explicit_purge"])
            self.assertEqual(sum(c.startswith("create_session:") for c in calls), 1)
            self.assertEqual(calls.count("allocator:purge"), 1)
            self.assertEqual(sum(c.startswith("allocator:profile:dump ") for c in calls), 6)
            self.assertEqual(result["heap_malloc_conf"], profile.HEAP_MALLOC_CONF)
            self.assertEqual(popen.call_args.kwargs["env"]["MALLOC_CONF"], profile.HEAP_MALLOC_CONF)
            for stage in result["stages"]:
                sample = json.loads((args.output / f"{stage}.json").read_text())
                self.assertEqual(sample["explicit_purge_completed"], stage == "after_explicit_purge")
            self.assertEqual(popen.call_args.kwargs["env"]["JCODE_TEST_SESSION"], "1")
            self.assertEqual(proc.wait.call_count, 1)


if __name__ == "__main__":
    unittest.main()
