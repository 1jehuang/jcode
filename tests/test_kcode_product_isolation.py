"""Cross-language product boundary regression checks; no real user data used."""
import pathlib
import subprocess
import tempfile
import tomllib
import unittest
import shutil

ROOT = pathlib.Path(__file__).resolve().parents[1]


class ProductIdentityTests(unittest.TestCase):
    def test_unix_installer_product_path_guard(self):
        git_bash = pathlib.Path(r"C:\Program Files\Git\bin\bash.exe")
        bash = str(git_bash) if git_bash.is_file() else shutil.which("bash")
        if not bash:
            self.skipTest("Bash unavailable")
        rejected = subprocess.run(
            [bash, "-c", "source scripts/product_paths.sh; require_kcode_owned_path /tmp/.jcode/forbidden"],
            cwd=ROOT, capture_output=True, text=True, timeout=30,
        )
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn("cannot use jcode", rejected.stderr)
        accepted = subprocess.run(
            [bash, "-c", 'source scripts/product_paths.sh; require_kcode_owned_path "$PWD/.verification/kcode-install"'],
            cwd=ROOT, capture_output=True, text=True, timeout=30,
        )
        self.assertEqual(accepted.returncode, 0, accepted.stderr)

    def test_cargo_exposes_kcode_and_no_jcode_launcher(self):
        manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        self.assertEqual(manifest["package"]["name"], "kcode")
        self.assertIn("-k", manifest["package"]["version"])
        binaries = {target["name"] for target in manifest["bin"]}
        self.assertIn("kcode", binaries)
        self.assertNotIn("jcode", binaries)

    def test_runtime_never_reads_legacy_environment_overrides(self):
        import re
        forbidden = re.compile(r'(?:var|var_os|getenv)\s*\(\s*"JCODE_')
        for folder in ("src", "crates"):
            for source in (ROOT / folder).rglob("*.rs"):
                self.assertIsNone(forbidden.search(source.read_text(encoding="utf-8")), str(source))

    def test_all_update_entry_points_fail_before_work(self):
        source = (ROOT / "crates/jcode-app-core/src/update.rs").read_text(encoding="utf-8")
        for signature in (
            "pub fn fetch_latest_release_blocking() -> Result<GitHubRelease> {",
            "fn latest_main_sha_blocking() -> Result<String> {",
            "pub fn prepare_update_blocking() -> Result<PreparedUpdate> {",
            "    mut on_progress: impl FnMut(DownloadProgress),\n) -> Result<PathBuf> {",
        ):
            body = source.split(signature, 1)[1].lstrip()
            self.assertTrue(body.startswith("update_product::require_update_channel()?;"), signature)
        self.assertIn('const GITHUB_REPO: &str = "KooshaPari/KCode";', source)

    @unittest.skipUnless(__import__("os").name == "nt", "Windows installer")
    def test_windows_installer_rejects_remote_download_before_creating_paths(self):
        with tempfile.TemporaryDirectory(dir=ROOT) as folder:
            target = pathlib.Path(folder) / "must-not-exist"
            result = subprocess.run(
                ["pwsh", "-NoProfile", "-File", str(ROOT / "scripts/install.ps1"),
                 "-InstallDir", str(target)],
                capture_output=True, text=True, timeout=30,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("KCode release downloads are disabled", result.stdout + result.stderr)
            self.assertFalse(target.exists())

    @unittest.skipUnless(__import__("os").name == "nt", "Windows installer")
    def test_windows_installer_rejects_jcode_destination_before_artifact_access(self):
        with tempfile.TemporaryDirectory(dir=ROOT) as folder:
            target = pathlib.Path(folder) / "jcode" / "bin"
            result = subprocess.run(
                ["pwsh", "-NoProfile", "-File", str(ROOT / "scripts/install.ps1"),
                 "-InstallDir", str(target), "-ArtifactExePath", "unused.exe", "-Version", "0.93.0-k0.1.0"],
                capture_output=True, text=True, timeout=30,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("KCode cannot install into jcode", result.stdout + result.stderr)
            self.assertFalse(target.exists())

    @unittest.skipUnless(__import__("os").name == "nt", "Windows junction")
    def test_windows_installer_cannot_follow_a_junction_into_jcode(self):
        with tempfile.TemporaryDirectory(dir=ROOT) as folder:
            root = pathlib.Path(folder)
            source = root / ".jcode"
            source.mkdir()
            marker = source / "keep"
            marker.write_bytes(b"untouched jcode installation")
            before = (marker.read_bytes(), marker.stat().st_mtime_ns, source.stat().st_mtime_ns)
            alias = root / ".kcode"
            link = subprocess.run(["cmd", "/c", "mklink", "/J", str(alias), str(source)],
                                  capture_output=True, text=True, timeout=30)
            self.assertEqual(link.returncode, 0, link.stderr)
            result = subprocess.run(
                ["pwsh", "-NoProfile", "-File", str(ROOT / "scripts/install.ps1"),
                 "-InstallDir", str(alias / "bin"), "-ArtifactExePath", "unused.exe", "-Version", "0.93.0-k0.1.0"],
                capture_output=True, text=True, timeout=30,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("KCode installer does not follow reparse points", result.stdout + result.stderr)
            self.assertEqual(before, (marker.read_bytes(), marker.stat().st_mtime_ns, source.stat().st_mtime_ns))
            self.assertFalse((source / "bin").exists())


if __name__ == "__main__":
    unittest.main()
