#!/usr/bin/env bash
# Validation only. The standalone helper, not this script, performs signing.
set -euo pipefail
[[ "${GITHUB_REPOSITORY:-}" == 1jehuang/jcode && "${GITHUB_REF:-}" == refs/heads/master &&
   "${GITHUB_EVENT_NAME:-}" == workflow_dispatch ]] || { echo 'Requires canonical master dispatch' >&2; exit 1; }
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'Requires native Apple ARM64' >&2; exit 1; }
pin=17c344f2e83f836598b82715950497c0879aa713
[[ "$(git rev-parse HEAD)" == "$GITHUB_SHA" && "$(git -C signing-helper-source rev-parse HEAD)" == "$pin" ]]
git -C signing-helper-source diff HEAD --exit-code -- scripts/sign-release.py tests/sign_release_test.py
fixture=signing-helper-validation
helper=signing-helper-source/scripts/sign-release.py
hash() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
case "${1:-}" in
prepare)
    mkdir "$fixture"
    cat > "$fixture/fixture.c" <<'C'
#include <stdio.h>
int main(void) { puts("jcode-signing-helper-native-ok"); return 0; }
C
    /usr/bin/clang -arch arm64 -O2 "$fixture/fixture.c" -o "$fixture/unsigned"
    [[ "$("./$fixture/unsigned")" == jcode-signing-helper-native-ok ]]
    echo "sha256=$(hash "$fixture/unsigned")" >> "$GITHUB_OUTPUT"
    exit 0
    ;;
sign) ;;
*) echo 'Usage: check_signing_helper_macos.sh prepare|sign' >&2; exit 1 ;;
esac
[[ "${TRUSTED_SHA256:-}" =~ ^[a-f0-9]{64}$ && "$(hash "$fixture/unsigned")" == "$TRUSTED_SHA256" ]]
for name in APPLE_CERTIFICATE_P12_BASE64 APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY; do
    [[ -n "${!name:-}" ]] || { echo "Missing credential: $name" >&2; exit 1; }
done
team_id=$(python3 - <<'PY'
import os
import re
match = re.fullmatch(r'Developer ID Application: [^\r\n]+ \(([A-Z0-9]{10})\)', os.environ['APPLE_SIGNING_IDENTITY'])
if not match:
    raise SystemExit('Expected exact Developer ID Application identity')
print(match.group(1))
PY
)
umask 077
work=$(mktemp -d)
keychain="$work/helper.keychain-db"
password=$(openssl rand -hex 32)
cleanup() {
    local status=$?
    if [[ -f "$work/keychains.txt" ]]; then
        python3 - "$work/keychains.txt" <<'PY' || status=1
from pathlib import Path
import shlex
import subprocess
import sys
subprocess.run(['security', 'list-keychains', '-d', 'user', '-s',
                *shlex.split(Path(sys.argv[1]).read_text())], check=True)
PY
    fi
    security delete-keychain "$keychain" >/dev/null 2>&1 || true
    rm -rf -- "$work"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
security list-keychains -d user > "$work/keychains.pending"
mv "$work/keychains.pending" "$work/keychains.txt"
printf '%s' "$APPLE_CERTIFICATE_P12_BASE64" | base64 --decode > "$work/certificate.p12"
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
security import "$work/certificate.p12" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple: -s -k "$password" "$keychain" >/dev/null
python3 - "$work/keychains.txt" "$keychain" <<'PY'
from pathlib import Path
import shlex
import subprocess
import sys
subprocess.run(['security', 'list-keychains', '-d', 'user', '-s', sys.argv[2],
                *shlex.split(Path(sys.argv[1]).read_text())], check=True)
PY
security find-identity -v -p codesigning "$keychain" > "$work/identities.txt"
certificate_sha1=$(python3 - "$work/identities.txt" <<'PY'
import os
from pathlib import Path
import re
import sys
matches = []
for line in Path(sys.argv[1]).read_text().splitlines():
    match = re.fullmatch(r'\s*\d+\)\s+([A-Fa-f0-9]{40})\s+"(.*)"\s*', line)
    if match and match.group(2) == os.environ['APPLE_SIGNING_IDENTITY']:
        matches.append(match.group(1))
if len(matches) != 1:
    raise SystemExit('Expected exactly one exact identity in temporary keychain')
print(matches[0])
PY
)
# Never export credentials to the helper process. Its native tools use a minimal env too.
unset APPLE_CERTIFICATE_P12_BASE64 APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY
args=(--input "$fixture/unsigned" --output "$fixture/signed" --sha256 "$TRUSTED_SHA256"
      --target aarch64-apple-darwin --identity "$certificate_sha1")
python3 "$helper" "${args[@]}" > "$work/plan.json"
[[ ! -e "$fixture/signed" && ! -L "$fixture/signed" ]]
python3 - "$work/plan.json" <<'PY'
import json
import sys
plan = json.load(open(sys.argv[1]))
assert plan['mode'] == 'plan-only'
assert len(plan['commands']) == 2
assert all(c[0] == '/usr/bin/codesign' for c in plan['commands'])
assert plan['commands'][1][-2].startswith('=anchor apple generic')
PY
python3 "$helper" "${args[@]}" --execute > "$work/execution.json"
requirement='anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and identifier "com.jcode.cli"'
requirement+=" and certificate leaf[subject.OU] = \"$team_id\""
/usr/bin/codesign --verify --strict --verbose=2 -R "=$requirement" "$fixture/signed"
[[ "$("./$fixture/signed")" == jcode-signing-helper-native-ok ]]
signed_hash=$(hash "$fixture/signed")
if python3 "$helper" "${args[@]}" --execute > "$work/refusal.txt" 2>&1; then
    echo 'Helper accepted an existing destination' >&2; exit 1
fi
[[ "$(hash "$fixture/signed")" == "$signed_hash" && "$(hash "$fixture/unsigned")" == "$TRUSTED_SHA256" ]]
python3 - "$fixture" "$pin" "$signed_hash" "$team_id" "$work/execution.json" <<'PY'
import json
import os
from pathlib import Path
import sys
assert json.loads(Path(sys.argv[5]).read_text())['mode'] == 'signed-and-native-verified'
receipt = {
    'schema': 1, 'repository': os.environ['GITHUB_REPOSITORY'],
    'source_sha': os.environ['GITHUB_SHA'], 'ref': os.environ['GITHUB_REF'],
    'run_id': os.environ['GITHUB_RUN_ID'], 'run_attempt': os.environ['GITHUB_RUN_ATTEMPT'],
    'workflow': os.environ['GITHUB_WORKFLOW'], 'helper_repository': '1jehuang/justrust',
    'helper_revision': sys.argv[2], 'unsigned_sha256': os.environ['TRUSTED_SHA256'],
    'signed_sha256': sys.argv[3], 'team_id': sys.argv[4],
    'target': 'aarch64-apple-darwin', 'identifier': 'com.jcode.cli',
    'checks': ['plan-no-output', 'real-helper-execute', 'apple-chain-identifier-team',
               'signed-native-execution', 'unsigned-unchanged', 'existing-destination-preserved'],
    'notarized': False,
}
Path(sys.argv[1], 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
PY
