#!/usr/bin/env bash
# Cherry-pick script — manually reviewed, do not auto-run
# Generated: 2026-10-17
# Base: 748eeaa1e (KCode fork master tip)
# Upstream: 6f2926e0c (1jehuang/jcode master)
#
# These 6 commits were verified to cherry-pick cleanly onto 748eeaa1e.
# Only d0091d078 (code fix) was verified with cargo check --workspace (PASSED).
# The batch also adds a Rust integration test, run explicitly below.
set -euo pipefail

cd /Users/kooshapari/CodeProjects/Phenotype/repos/jcode

echo "=== Creating cherry-pick batch branch from 748eeaa1e ==="
git checkout -b cherry-pick-batch-2026-10-17 748eeaa1e

echo ""
echo "=== Cherry-pick 1/6: d0091d078 fix(fuzzy): remove redundant slice conversion ==="
echo "    Category: Bug fix | Files: crates/jcode-fuzzy/src/lib.rs | Cargo check: PASSED"
git cherry-pick d0091d078

echo ""
echo "=== Cherry-pick 2/6: 5cbb82c81 test: guard CLI parse exit statuses (refs #1823) ==="
echo "    Category: Test | Files: tests/cli_exit_status.rs (new)"
git cherry-pick 5cbb82c81

echo ""
echo "=== Cherry-pick 3/6: 0d240f1d4 ios: support all orientations ==="
echo "    Category: Platform fix | Files: ios/Sources/JCodeMobile/Info.plist"
git cherry-pick 0d240f1d4

echo ""
echo "=== Cherry-pick 4/6: 4121a46e6 Add optional pinned Nix development shell ==="
echo "    Category: DevEx | Files: flake.lock, flake.nix, CONTRIBUTING.md"
git cherry-pick 4121a46e6

echo ""
echo "=== Cherry-pick 5/6: 458384094 Add .gitattributes ==="
echo "    Category: DevEx | Files: .gitattributes (new)"
git cherry-pick 458384094

echo ""
echo "=== Cherry-pick 6/6: 7e696d778 docs: plan guided Google OAuth setup ==="
echo "    Category: Docs | Files: docs/GOOGLE_GUIDED_SETUP.md (new)"
git cherry-pick 7e696d778

echo ""
echo "=== Running cargo check --workspace ==="
cargo check --workspace

echo ""
echo "=== Running CLI exit-status integration tests ==="
cargo test --test cli_exit_status

echo ""
echo "=== All cherry-picks applied, cargo check and CLI exit-status tests passed ==="
echo "=== Branch: cherry-pick-batch-2026-10-17 ==="
echo "=== Review with: git log --oneline 748eeaa1e..HEAD ==="
echo ""
echo "To merge into master:"
echo "  git checkout master"
echo "  git merge cherry-pick-batch-2026-10-17"
echo ""
echo "To discard:"
echo "  git checkout master"
echo "  git branch -D cherry-pick-batch-2026-10-17"