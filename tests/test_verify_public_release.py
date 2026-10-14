#!/usr/bin/env python3
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("verifier", ROOT / "scripts/verify_public_release.py")
verifier = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(verifier)
TAG = "v0.98.1"
SHA = "a" * 40


def metadata():
    return {"tag_name": TAG, "draft": False, "prerelease": False,
            "assets": [{"name": name, "size": 1, "state": "uploaded"}
                       for name in sorted(verifier.REQUIRED)]}


class PublicReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=os.environ.get("JCODE_SCRATCH_DIR"))
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.names = verifier.verify_metadata(metadata(), TAG)
        for name in self.names - {"SHA256SUMS"}:
            (self.directory / name).write_bytes(b"fixture")
        (self.directory / "release-provenance.json").write_text(json.dumps({
            "tag": TAG, "tag_commit": SHA, "repair": "none",
            "eligibility_policy": "exact-sha-main-ci"}))
        self.write_checksums()

    def write_checksums(self):
        (self.directory / "SHA256SUMS").write_text("".join(
            f"{hashlib.sha256((self.directory / name).read_bytes()).hexdigest()}  {name}\n"
            for name in sorted(self.names - {"SHA256SUMS"})))

    def verify(self):
        verifier.verify_downloads(self.directory, self.names, TAG, SHA)

    def test_complete_release_passes(self):
        self.verify()

    def test_workflow_verifies_before_announcements_and_package_updates(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        publish = workflow.index("      - name: Publish completed release")
        verify = workflow.index("      - name: Verify public assets and exact source provenance")
        announce = workflow.index("      - name: Queue Discord release announcement")
        packages = workflow.index("      - name: Update Homebrew formula")
        self.assertLess(publish, verify)
        self.assertLess(verify, announce)
        self.assertLess(verify, packages)
        step = workflow[verify:announce]
        self.assertNotIn("continue-on-error", step)
        self.assertIn('--sha "$TAG_SHA"', step)
        self.assertIn('git show "$WORKFLOW_SHA:scripts/verify_public_release.py"', step)

    def test_incomplete_metadata_rejected(self):
        for field, value in (("draft", True), ("prerelease", True), ("tag_name", "v0.98.0")):
            with self.subTest(field=field):
                data = metadata()
                data[field] = value
                with self.assertRaises(ValueError):
                    verifier.verify_metadata(data, TAG)
        data = metadata()
        data["assets"].pop()
        with self.assertRaises(ValueError):
            verifier.verify_metadata(data, TAG)

    def test_invalid_asset_metadata_rejected(self):
        for field, value in (("name", "../unsafe"), ("name", "..\\unsafe"),
                             ("size", 0), ("size", True), ("state", "new")):
            with self.subTest(field=field, value=value):
                data = metadata()
                data["assets"][0][field] = value
                with self.assertRaises(ValueError):
                    verifier.verify_metadata(data, TAG)
        data = metadata()
        data["assets"].append(copy.deepcopy(data["assets"][0]))
        with self.assertRaises(ValueError):
            verifier.verify_metadata(data, TAG)

    def test_corrupted_asset_rejected(self):
        (self.directory / "jcode-linux-x86_64.tar.gz").write_bytes(b"corrupted")
        with self.assertRaisesRegex(ValueError, "Checksum mismatch"):
            self.verify()

    def test_checksum_coverage_and_format_rejected(self):
        path = self.directory / "SHA256SUMS"
        original = path.read_text()
        for content in ("", original.splitlines()[0] + "\n", original + original.splitlines()[0] + "\n",
                        original + "a" * 64 + "  ../unsafe\n", original + "invalid\n"):
            with self.subTest(content=content[:80]):
                path.write_text(content)
                with self.assertRaises(ValueError):
                    self.verify()

    def test_missing_and_symlinked_download_rejected(self):
        path = self.directory / "jcode-linux-x86_64.tar.gz"
        path.unlink()
        with self.assertRaisesRegex(ValueError, "Missing or unsafe"):
            self.verify()
        path.symlink_to(self.directory / "jcode-linux-aarch64.tar.gz")
        with self.assertRaisesRegex(ValueError, "Missing or unsafe"):
            self.verify()

    def test_wrong_commit_or_repaired_provenance_rejected(self):
        path = self.directory / "release-provenance.json"
        original = json.loads(path.read_text())
        for field, value in (("tag", "v0.98.0"), ("tag_commit", "b" * 40),
                             ("repair", "windows-listener"), ("eligibility_policy", "bypassed")):
            with self.subTest(field=field):
                data = dict(original, **{field: value})
                path.write_text(json.dumps(data))
                self.write_checksums()
                with self.assertRaisesRegex(ValueError, "provenance"):
                    self.verify()


if __name__ == "__main__":
    unittest.main()
