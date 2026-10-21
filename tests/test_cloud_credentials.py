#!/usr/bin/env python3
"""Cloud handoff acceptance using only synthetic OAuth accounts and local pipes.

Run: python3 tests/test_cloud_credentials.py /absolute/path/to/jcode
No real account, network connection, or shared daemon is used.
"""
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tarfile
import tempfile
import unittest

BINARY = None
ACCESS = "synthetic-cloud-access-not-a-real-secret"
REFRESH = "synthetic-cloud-refresh-not-a-real-secret"
ROUTER_KEY = "synthetic-router-api-key-not-a-real-secret"


class CloudCredentials(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="jcode-cloud-auth-test-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.local = self.root / "local"
        self.remote = self.root / "remote"
        self.work = self.root / "work"
        for path in (self.local, self.remote, self.work):
            path.mkdir(mode=0o700)
        self.transport = self.root / "transport"
        self.transport.write_text(
            "#!/bin/sh\nexport HOME=" + shlex.quote(str(self.root)) + "\n"
            "export JCODE_HOME=" + shlex.quote(str(self.remote)) + "\n"
            'exec /bin/sh -c "$1"\n'
        )
        self.transport.chmod(0o700)
        self.env = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "HOME": str(self.root), "JCODE_HOME": str(self.local),
            "JCODE_RUNTIME_DIR": str(self.root),
            "JCODE_NO_TELEMETRY": "1", "JCODE_TELEMETRY": "off",
            "HTTPS_PROXY": "http://127.0.0.1:9", "HTTP_PROXY": "http://127.0.0.1:9",
        }

    def invoke(self, *args, success=True):
        result = subprocess.run(
            [BINARY, "--no-update", "--no-selfdev", "cloud", *args],
            cwd=self.work, env=self.env, capture_output=True, timeout=30,
        )
        for secret in (ACCESS, REFRESH, ROUTER_KEY, "synthetic-refreshed-access", "synthetic-refreshed-refresh"):
            self.assertNotIn(secret.encode(), result.stdout + result.stderr)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr.decode())
        else:
            self.assertNotEqual(result.returncode, 0)
        return result

    def seed(self, provider="openai", expiry=4102444800000):
        self.provider = provider
        self.label = provider + "-fox"
        self.filename = "openai-auth.json" if provider == "openai" else "auth.json"
        self.accounts = "openai_accounts" if provider == "openai" else "anthropic_accounts"
        self.active = "active_openai_account" if provider == "openai" else "active_anthropic_account"
        account = {"label": self.label, "email": "test@example.invalid"}
        if provider == "openai":
            account.update(access_token=ACCESS, refresh_token=REFRESH,
                           expires_at=expiry, account_id="synthetic-account-id")
        else:
            account.update(access=ACCESS, refresh=REFRESH, expires=expiry, scopes=["user:inference"])
        (self.local / self.filename).write_text(json.dumps({self.accounts: [account], self.active: self.label}))
        (self.local / self.filename).chmod(0o600)
        sessions = self.local / "sessions"
        sessions.mkdir()
        (sessions / "cloud-auth-test.json").write_text(json.dumps({
            "id": "cloud-auth-test", "parent_id": None, "title": "Cloud auth test",
            "created_at": "2026-10-20T00:00:00Z", "updated_at": "2026-10-20T00:00:00Z",
            "messages": [], "working_dir": str(self.work),
            "provider_key": "openai" if provider == "openai" else "anthropic",
            "model": "gpt-test" if provider == "openai" else "claude-test",
            "route_api_method": ("openai-oauth@" if provider == "openai" else "claude-oauth@") + self.label,
        }))

    def move(self, *extra, success=True):
        return self.invoke("move", "--session", "cloud-auth-test", "--host", "test-cloud",
                           "--remote-binary", BINARY, "--transport", str(self.transport), "--json",
                           *extra, success=success)

    def return_home(self, success=True):
        return self.invoke("return", "--session", "cloud-auth-test", "--host", "test-cloud",
                           "--remote-binary", BINARY, "--transport", str(self.transport), "--json",
                           success=success)

    def assert_archives_secret_free(self):
        for archive in self.remote.rglob("*.tar"):
            with tarfile.open(archive) as tar:
                for member in tar.getmembers():
                    if member.isfile():
                        contents = tar.extractfile(member).read()
                        self.assertNotIn(ACCESS.encode(), contents)
                        self.assertNotIn(REFRESH.encode(), contents)

    def roundtrip(self, provider):
        self.seed(provider)
        self.move()
        store = self.remote / self.filename
        data = json.loads(store.read_text())
        self.assertEqual(data[self.accounts][0]["label"], self.label)
        self.assertEqual(store.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.remote.stat().st_mode & 0o777, 0o700)
        self.assert_archives_secret_free()
        account = data[self.accounts][0]
        account["access_token" if provider == "openai" else "access"] = "synthetic-refreshed-access"
        account["refresh_token" if provider == "openai" else "refresh"] = "synthetic-refreshed-refresh"
        store.write_text(json.dumps(data))
        self.return_home()
        returned = json.loads((self.local / self.filename).read_text())[self.accounts][0]
        self.assertEqual(returned["refresh_token" if provider == "openai" else "refresh"], "synthetic-refreshed-refresh")

    def test_openai_roundtrip(self):
        self.roundtrip("openai")

    def test_claude_roundtrip(self):
        self.roundtrip("claude")

    def test_all_accounts_move_and_return_not_just_selected_account(self):
        self.seed()
        openai = json.loads((self.local / "openai-auth.json").read_text())
        second = dict(openai["openai_accounts"][0], label="openai-otter",
                      account_id="second-account")
        openai["openai_accounts"].append(second)
        (self.local / "openai-auth.json").write_text(json.dumps(openai))
        claude = {"anthropic_accounts": [
            {"label": label, "access": ACCESS, "refresh": REFRESH,
             "expires": 1, "email": label + "@example.invalid"}
            for label in ("claude-fox", "claude-otter")
        ], "active_anthropic_account": "claude-otter"}
        (self.local / "auth.json").write_text(json.dumps(claude))
        (self.local / "auth.json").chmod(0o600)
        self.move()
        for filename, key, refresh in [
            ("openai-auth.json", "openai_accounts", "refresh_token"),
            ("auth.json", "anthropic_accounts", "refresh"),
        ]:
            store = self.remote / filename
            data = json.loads(store.read_text())
            self.assertEqual(len(data[key]), 2)
            for account in data[key]:
                account[refresh] = "synthetic-refreshed-refresh"
            store.write_text(json.dumps(data))
        self.assert_archives_secret_free()
        self.return_home()
        for filename, key, refresh in [
            ("openai-auth.json", "openai_accounts", "refresh_token"),
            ("auth.json", "anthropic_accounts", "refresh"),
        ]:
            data = json.loads((self.local / filename).read_text())
            self.assertTrue(all(account[refresh] == "synthetic-refreshed-refresh"
                                for account in data[key]))
        # Importing the other provider must not change the local selection.
        self.assertEqual(json.loads((self.local / "auth.json").read_text())[
            "active_anthropic_account"], "claude-otter")

    def test_handoff_replaces_stale_remote_token_with_equal_expiry(self):
        self.seed()
        data = json.loads((self.local / self.filename).read_text())
        data[self.accounts][0]["refresh_token"] = "old-remote-refresh"
        store = self.remote / self.filename
        store.write_text(json.dumps(data))
        store.chmod(0o600)
        self.move()
        self.assertEqual(json.loads(store.read_text())[self.accounts][0]["refresh_token"], REFRESH)

    def test_dry_run_does_not_copy_credentials(self):
        self.seed()
        self.move("--dry-run")
        self.assertFalse((self.remote / self.filename).exists())
        self.assert_archives_secret_free()
        self.assertFalse((self.local / "session_leases" / "cloud-auth-test.json").exists())

    def test_expired_access_token_can_be_transferred_for_refresh(self):
        self.seed(expiry=1)
        self.move()
        self.assertTrue((self.remote / self.filename).exists())

    def test_identityless_refresh_roundtrip_uses_departure_fingerprint(self):
        self.seed()
        store = self.local / self.filename
        data = json.loads(store.read_text())
        data[self.accounts][0].pop("account_id")
        data[self.accounts][0].pop("email")
        store.write_text(json.dumps(data))
        self.move()
        remote_store = self.remote / self.filename
        data = json.loads(remote_store.read_text())
        data[self.accounts][0]["refresh_token"] = "synthetic-refreshed-refresh"
        remote_store.write_text(json.dumps(data))
        self.return_home()
        self.assertEqual(json.loads(store.read_text())[self.accounts][0]["refresh_token"], "synthetic-refreshed-refresh")

    def test_broken_transport_cannot_echo_secret_to_diagnostics(self):
        self.seed()
        original = self.transport.read_text()
        self.transport.write_text(original.replace(
            'exec /bin/sh -c "$1"',
            'case "$1" in *credentials*--help*) ;; *credentials*) cat >&2; exit 1 ;; esac\nexec /bin/sh -c "$1"',
        ))
        self.move(success=False)

    def test_local_refresh_is_not_silently_overwritten_on_return(self):
        self.seed()
        self.move()
        store = self.local / self.filename
        data = json.loads(store.read_text())
        data[self.accounts][0]["refresh_token"] = "newer-local-refresh"
        store.write_text(json.dumps(data))
        result = self.return_home(success=False)
        self.assertIn(b"local OAuth account changed", result.stderr)
        self.assertEqual(json.loads(store.read_text())[self.accounts][0]["refresh_token"], "newer-local-refresh")

    def test_conflicting_cloud_account_never_replaces_existing_login(self):
        self.seed()
        data = json.loads((self.local / self.filename).read_text())
        data[self.accounts][0]["account_id"] = "different-user"
        data[self.accounts][0]["access_token"] = "different-access"
        data[self.accounts][0]["refresh_token"] = "different-refresh"
        store = self.remote / self.filename
        original = json.dumps(data)
        store.write_text(original)
        store.chmod(0o600)
        self.move(success=False)
        self.assertEqual(store.read_text(), original)
        self.assertEqual(json.loads(self.invoke("where", "--session", "cloud-auth-test", "--json").stdout), [])

    def test_return_conflict_restores_cloud_ownership(self):
        self.seed()
        self.move()
        store = self.local / self.filename
        data = json.loads(store.read_text())
        data[self.accounts][0]["account_id"] = "different-local-user"
        store.write_text(json.dumps(data))
        result = self.return_home(success=False)
        self.assertIn(b"remains on the cloud host", result.stderr)
        remote_env = dict(self.env, JCODE_HOME=str(self.remote))
        where = subprocess.run([BINARY, "--no-update", "--no-selfdev", "cloud", "where", "--session",
                                "cloud-auth-test", "--json"], env=remote_env, capture_output=True, timeout=10)
        self.assertEqual(where.returncode, 0, where.stderr.decode())
        self.assertEqual(json.loads(where.stdout)[0]["location"], "local")
        remote_session = json.loads((self.remote / "sessions" / "cloud-auth-test.json").read_text())
        self.assertEqual(remote_session["migration_epoch"], json.loads(where.stdout)[0]["epoch"])
        self.assertEqual(json.loads(store.read_text())[self.accounts][0]["account_id"], "different-local-user")

    def seed_router(self):
        config = self.local / "config" / "jcode"
        config.mkdir(parents=True)
        (config / "openrouter.env").write_text("OPENROUTER_API_KEY=" + ROUTER_KEY + "\n")
        (config / "bedrock.env").write_text("AWS_BEARER_TOKEN_BEDROCK=" + ROUTER_KEY + "\n")
        (config / "unlisted.env").write_text("X=" + ROUTER_KEY + "\n")
        (config / "config.toml").write_text("")
        (self.local / "gemini_oauth.json").write_text(json.dumps(
            {"access_token": ROUTER_KEY, "refresh_token": ROUTER_KEY, "expires_at": 1}))
        (self.local / "account-pool.json").write_text(json.dumps(
            {"members": ["gemini", "openrouter"], "excluded": []}))
        remote_config = self.remote / "config" / "jcode"
        remote_config.mkdir(parents=True)
        (remote_config / "openrouter.env").write_text("OPENROUTER_API_KEY=stale\n")
        (remote_config / "groq.env").write_text("GROQ_API_KEY=cloud-only\n")
        (self.remote / "account-pool.json").write_text(json.dumps(
            {"members": ["cloud-only"], "excluded": ["xai"]}))
        return remote_config

    def test_router_credentials_transfer_one_way(self):
        self.seed()
        remote_config = self.seed_router()
        result = self.move()
        self.assertNotIn(ROUTER_KEY.encode(), result.stdout + result.stderr)
        self.assert_archives_secret_free()
        for archive in self.remote.rglob("*.tar"):
            with tarfile.open(archive) as tar:
                for member in tar.getmembers():
                    if member.isfile():
                        self.assertNotIn(ROUTER_KEY.encode(), tar.extractfile(member).read())
        for name in ("openrouter.env", "bedrock.env"):
            path = remote_config / name
            self.assertIn(ROUTER_KEY, path.read_text())
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        self.assertEqual(remote_config.stat().st_mode & 0o777, 0o700)
        self.assertEqual((remote_config / "groq.env").read_text(), "GROQ_API_KEY=cloud-only\n")
        self.assertFalse((remote_config / "unlisted.env").exists())
        self.assertFalse((remote_config / "config.toml").exists())
        gemini = self.remote / "gemini_oauth.json"
        self.assertEqual(json.loads(gemini.read_text())["refresh_token"], ROUTER_KEY)
        self.assertEqual(gemini.stat().st_mode & 0o777, 0o600)
        pool = json.loads((self.remote / "account-pool.json").read_text())
        self.assertEqual(pool["members"], ["gemini", "openrouter", "cloud-only"])
        self.assertEqual(pool["excluded"], ["xai"])
        # Keys are one-way: return brings back OAuth only and leaves local keys alone.
        (remote_config / "openrouter.env").write_text("OPENROUTER_API_KEY=cloud-changed\n")
        self.return_home()
        self.assertIn(ROUTER_KEY, (self.local / "config" / "jcode" / "openrouter.env").read_text())

    def test_router_credentials_without_oauth_account(self):
        self.seed()
        (self.local / self.filename).unlink()
        session = self.local / "sessions" / "cloud-auth-test.json"
        data = json.loads(session.read_text())
        data.update(provider_key="openrouter", model="synthetic/test", route_api_method=None)
        session.write_text(json.dumps(data))
        remote_config = self.seed_router()
        self.move()
        self.assertIn(ROUTER_KEY, (remote_config / "openrouter.env").read_text())

    def test_old_remote_without_router_command_fails_before_transfer(self):
        self.seed()
        remote_config = self.seed_router()
        original = self.transport.read_text()
        self.transport.write_text(original.replace(
            'exec /bin/sh -c "$1"',
            'case "$1" in *router-credentials*) echo "error: unrecognized subcommand router-credentials" >&2; exit 2 ;; esac\nexec /bin/sh -c "$1"',
        ))
        result = self.move(success=False)
        self.assertIn(b"does not support the complete cloud handoff protocol", result.stderr)
        self.assertFalse((self.remote / self.filename).exists())
        self.assertFalse((self.remote / "gemini_oauth.json").exists())
        self.assertEqual((remote_config / "openrouter.env").read_text(), "OPENROUTER_API_KEY=stale\n")
        self.assertFalse(list(self.remote.rglob("*.tar")))


if __name__ == "__main__":
    if len(sys.argv) < 2:
        raise SystemExit("usage: test_cloud_credentials.py /path/to/jcode")
    BINARY = str(Path(sys.argv.pop(1)).resolve())
    unittest.main()
