#!/usr/bin/env bash
# Everything that must be true before `npm publish`, checked in one place.
#
# The point is that publishing should be a decision, not an investigation. This
# runs the same gates CI runs, then reports the two things only a human can
# resolve: being logged in, and owning the package name.
#
# Usage: bash scripts/sdk_publish_preflight.sh
set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
sdk_dir="$repo_root/sdk/typescript"
name="$(node -p "require('$sdk_dir/package.json').name")"
version="$(node -p "require('$sdk_dir/package.json').version")"
problems=0

note_problem() {
  echo "  BLOCKED: $1"
  problems=$((problems + 1))
}

echo "== $name@$version =="

echo
echo "-- gates --"
if npm --prefix "$sdk_dir" run check --silent >/tmp/sdk-preflight-check.log 2>&1; then
  echo "  ok: typecheck, build, unit tests"
else
  note_problem "npm run check failed; see /tmp/sdk-preflight-check.log"
fi

if bash "$repo_root/scripts/test_sdk_package.sh" >/tmp/sdk-preflight-pack.log 2>&1; then
  echo "  ok: the packed tarball installs and imports as a consumer"
else
  note_problem "the package check failed; see /tmp/sdk-preflight-pack.log"
fi

echo
echo "-- tarball --"
files="$(cd "$sdk_dir" && npm pack --dry-run 2>&1 | grep -c 'npm notice.*dist/')"
if [ "$files" -gt 0 ]; then
  echo "  ok: $files built files included"
else
  note_problem "the tarball contains no dist/ files; prepack may have failed"
fi
if (cd "$sdk_dir" && npm pack --dry-run 2>&1 | grep -qE 'npm notice.*(src/|test/)'); then
  note_problem "sources or tests are leaking into the tarball; check \`files\`"
else
  echo "  ok: no sources or tests in the tarball"
fi

echo
echo "-- things only you can resolve --"
if whoami_out="$(npm whoami 2>&1)"; then
  echo "  ok: logged in to npm as $whoami_out"
else
  note_problem "not logged in. Run \`npm login\`."
fi

# A scoped name needs its scope to exist and to be yours.
scope="${name%%/*}"
if [ "$scope" != "$name" ]; then
  if npm view "$name" version >/dev/null 2>&1; then
    echo "  ok: $name already exists on the registry (this would be an update)"
  else
    echo "  note: $name is unpublished. The $scope scope must be an org or user you own."
    echo "        Create a free org at https://www.npmjs.com/org/create, or rename the"
    echo "        package. See sdk/typescript/RELEASING.md."
  fi
fi

echo
if [ "$problems" -eq 0 ]; then
  echo "Preflight clean. To publish: cd sdk/typescript && npm publish"
else
  echo "$problems blocker(s) above. Nothing was published."
  exit 1
fi
