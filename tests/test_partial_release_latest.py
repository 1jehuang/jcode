#!/usr/bin/env python3
"""Partial-platform publish paths must never mark a release as GitHub "latest".

Clients older than v1.0.3 resolve stable updates only through
`releases/latest`. When that release lacks their platform asset they report
"Already up to date" forever, and `releases/latest/download/<asset>` 404s for
fresh installs. v1.1.1 and v1.1.2 shipped linux-x86_64 only and were marked
latest, stranding every macOS, Windows, and Linux aarch64 client on <= v1.0.2.

Only release.yml, which uploads and verifies every platform, may pass
`--latest`.
"""
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]

# Paths that publish a subset of platforms.
PARTIAL_PUBLISHERS = (
    ROOT / "scripts/publish_linux_release_direct.sh",
    ROOT / ".github/workflows/selfdev-linux.yml",
)

# Matches `--latest` or `--latest=true`, but not `--latest=false`.
PROMOTES_LATEST = re.compile(r"--latest(?:=true)?(?![=\w-])")


def promoting_lines(text):
    return [
        line.strip()
        for line in text.splitlines()
        if not line.lstrip().startswith("#") and PROMOTES_LATEST.search(line)
    ]


class PartialReleaseLatest(unittest.TestCase):
    def test_pattern_distinguishes_promotion_from_opt_out(self):
        self.assertTrue(PROMOTES_LATEST.search("gh release edit v1 --latest"))
        self.assertTrue(PROMOTES_LATEST.search("gh release edit v1 --latest --draft=false"))
        self.assertTrue(PROMOTES_LATEST.search("gh release edit v1 --latest=true"))
        self.assertFalse(PROMOTES_LATEST.search("gh release edit v1 --latest=false"))
        self.assertFalse(PROMOTES_LATEST.search("gh release edit v1 --latest-only"))

    def test_partial_publishers_never_promote_latest(self):
        for path in PARTIAL_PUBLISHERS:
            with self.subTest(path=path.relative_to(ROOT)):
                text = path.read_text(encoding="utf-8")
                self.assertIn("gh release edit", text, "publisher no longer edits releases")
                self.assertEqual(promoting_lines(text), [])

    def test_partial_publishers_explicitly_opt_out_of_latest(self):
        # GitHub defaults `make_latest` to true for a newly published
        # non-prerelease. Omitting the flag is not safe, so require the opt-out.
        for path in PARTIAL_PUBLISHERS:
            with self.subTest(path=path.relative_to(ROOT)):
                edits = [
                    line
                    for line in path.read_text(encoding="utf-8").splitlines()
                    if "gh release edit" in line and "--draft=false" in line
                ]
                self.assertTrue(edits, "no publishing `gh release edit` found")
                for line in edits:
                    self.assertIn("--latest=false", line)

    def test_full_release_workflow_still_promotes_after_verification(self):
        text = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
        verify = text.index("Verify staged assets before publication")
        publish = text.index("Publish completed release")
        self.assertLess(verify, publish)
        self.assertTrue(promoting_lines(text[publish:]))


if __name__ == "__main__":
    unittest.main()
