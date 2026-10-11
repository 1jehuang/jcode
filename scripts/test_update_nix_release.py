#!/usr/bin/env python3
"""Offline tests for the Nix binary release metadata updater."""

import base64
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import update_nix_release as updater


class UpdateReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / "release.json"
        self.tag = "v0.90.0"
        self.release = {
            "tag_name": self.tag,
            "draft": False,
            "prerelease": False,
            "published_at": "2026-10-04T00:00:00Z",
            "assets": [
                {"name": name, "state": "uploaded", "size": 100}
                for name in [*updater.ARCHIVES.values(), "SHA256SUMS"]
            ],
        }
        self.checksums = "".join(
            f"{'ab' * 32}  {name}\n" for name in updater.ARCHIVES.values()
        )
        self.existing = {
            "version": "0.89.3",
            "tag": "v0.89.3",
            "hashes": {system: "sha256-" + base64.b64encode(bytes(32)).decode()
                       for system in updater.ARCHIVES},
        }
        self.path.write_text(json.dumps(self.existing) + "\n")
        self.original = self.path.read_bytes()

    def update(self, tag=None):
        with patch.object(updater, "fetch_release", return_value=self.release), \
             patch.object(updater, "fetch_checksums", return_value=self.checksums):
            return updater.update_release(tag or self.tag, self.path)

    def assert_unchanged(self):
        self.assertEqual(self.path.read_bytes(), self.original)
        self.assertEqual(list(self.path.parent.iterdir()), [self.path])

    def test_valid_checksums_and_schema(self):
        self.assertTrue(self.update())
        self.assertEqual(json.loads(self.path.read_text()), {
            "version": "0.90.0", "tag": self.tag,
            "hashes": {system: "sha256-" + base64.b64encode(bytes.fromhex("ab" * 32)).decode()
                       for system in updater.ARCHIVES},
        })

    def test_uppercase_binary_checksum_format_and_extra_platform(self):
        self.checksums = "".join(f"{'AB' * 32} *{name}\n" for name in updater.ARCHIVES.values())
        self.checksums += f"{'cd' * 32}  jcode-windows-x86_64.exe\n\n"
        self.assertTrue(self.update())

    def test_malformed_missing_and_duplicate_checksums(self):
        valid = self.checksums
        cases = [
            valid.replace("ab" * 32, "ab" * 31, 1),
            valid.replace("ab" * 32, "ab" * 33, 1),
            valid.replace("ab" * 32, "gg" * 32, 1),
            "\n".join(valid.splitlines()[1:]),
            valid + valid.splitlines()[0] + "\n",
            valid + "not a checksum\n",
            valid.replace("  jcode", " jcode", 1),
            "",
        ]
        for text in cases:
            with self.subTest(text=text):
                self.checksums = text
                with self.assertRaises(ValueError):
                    self.update()
                self.assert_unchanged()

    def test_nonpublic_or_mismatched_release(self):
        valid = copy.deepcopy(self.release)
        for field, value in [("draft", True), ("prerelease", True),
                             ("published_at", None), ("tag_name", "v0.91.0"),
                             ("draft", None), ("prerelease", None)]:
            with self.subTest(field=field, value=value):
                self.release = dict(valid, **{field: value})
                with self.assertRaises(ValueError):
                    self.update()
                self.assert_unchanged()

    def test_missing_duplicate_and_unavailable_assets(self):
        valid = copy.deepcopy(self.release["assets"])
        for index in range(len(valid)):
            with self.subTest(missing=valid[index]["name"]):
                self.release["assets"] = valid[:index] + valid[index + 1:]
                with self.assertRaises(ValueError):
                    self.update()
                self.assert_unchanged()
        for assets in [valid + [valid[0]],
                       [dict(valid[0], state="new"), *valid[1:]],
                       [dict(valid[0], size=0), *valid[1:]]]:
            self.release["assets"] = assets
            with self.assertRaises(ValueError):
                self.update()
            self.assert_unchanged()

    def test_invalid_tags_fail_before_network(self):
        for tag in ["0.90.0", "v1.2", "v1.2.3-rc1", "v1.2.3+build", "v01.2.3",
                    "v1.2.3\n", "v1.2.3/other", "v١.2.3", "--help"]:
            with self.subTest(tag=tag), patch.object(updater, "fetch_release") as fetch:
                with self.assertRaises(ValueError):
                    updater.update_release(tag, self.path)
                fetch.assert_not_called()
                self.assert_unchanged()

    def test_downgrade_fails_before_network(self):
        with patch.object(updater, "fetch_release") as fetch:
            with self.assertRaises(ValueError):
                updater.update_release("v0.89.2", self.path)
            fetch.assert_not_called()
        self.assert_unchanged()

    def test_version_comparison_is_numeric(self):
        self.existing.update(version="0.9.0", tag="v0.9.0")
        self.path.write_text(json.dumps(self.existing))
        self.assertTrue(self.update())

    def test_invalid_existing_metadata_is_not_overwritten(self):
        for text in ['{"version":"broken"}', 'null', '[]', 'not json']:
            with self.subTest(text=text):
                self.path.write_text(text)
                self.original = self.path.read_bytes()
                with self.assertRaises(ValueError):
                    self.update()
                self.assert_unchanged()

    def test_network_failure_keeps_existing_metadata(self):
        with patch.object(updater, "fetch_release", return_value=self.release), \
             patch.object(updater, "fetch_checksums", side_effect=OSError("offline")):
            with self.assertRaises(OSError):
                updater.update_release(self.tag, self.path)
        self.assert_unchanged()

    def test_replace_failure_is_atomic_and_cleans_temporary_file(self):
        with patch.object(updater.os, "replace", side_effect=OSError("read-only")):
            with self.assertRaises(OSError):
                self.update()
        self.assert_unchanged()

    def test_flush_failure_is_atomic_and_cleans_temporary_file(self):
        with patch.object(updater.os, "fsync", side_effect=OSError("disk full")):
            with self.assertRaises(OSError):
                self.update()
        self.assert_unchanged()

    def test_release_fetch_uses_canonical_api(self):
        with patch.object(updater.urllib.request, "urlopen") as urlopen, \
             patch.dict(updater.os.environ, {}, clear=True):
            urlopen.return_value.__enter__.return_value.read.return_value = json.dumps(self.release)
            self.assertEqual(updater.fetch_release(self.tag), self.release)
        request = urlopen.call_args.args[0]
        self.assertEqual(request.full_url,
                         "https://api.github.com/repos/1jehuang/jcode/releases/tags/v0.90.0")
        self.assertFalse(request.has_header("Authorization"))
        self.assertEqual(urlopen.call_args.kwargs["timeout"], 30)

    def test_checksum_fetch_does_not_use_api_credentials(self):
        with patch.object(updater.urllib.request, "urlopen") as urlopen, \
             patch.dict(updater.os.environ, {"GH_TOKEN": "foo"}):
            urlopen.return_value.__enter__.return_value.read.return_value = self.checksums.encode()
            self.assertEqual(updater.fetch_checksums(self.tag), self.checksums)
        request = urlopen.call_args.args[0]
        self.assertEqual(request.full_url, updater.asset_url(self.tag, "SHA256SUMS"))
        self.assertFalse(request.has_header("Authorization"))

    def test_idempotent_update_does_not_replace_file(self):
        self.update()
        with patch.object(updater.os, "replace") as replace:
            self.assertFalse(self.update())
            replace.assert_not_called()

    def test_initial_creation_and_invalid_input_leave_no_partial_file(self):
        self.path.unlink()
        with self.assertRaises(ValueError):
            self.update("not-a-tag")
        self.assertEqual(list(self.path.parent.iterdir()), [])
        self.assertTrue(self.update())

    def test_canonical_archive_url(self):
        self.assertEqual(updater.asset_url(self.tag, "SHA256SUMS"),
                         "https://github.com/1jehuang/jcode/releases/download/v0.90.0/SHA256SUMS")
        with self.assertRaises(ValueError):
            updater.asset_url("v1.2.3/other", "SHA256SUMS")
        with self.assertRaises(ValueError):
            updater.asset_url(self.tag, "../other")


if __name__ == "__main__":
    unittest.main()
