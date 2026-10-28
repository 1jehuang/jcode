#!/usr/bin/env bash
# Only called after same-run provenance and native smoke verification.
set -euo pipefail
for name in APPLE_CERTIFICATE_P12_BASE64 APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY \
            APPLE_NOTARY_KEY_P8_BASE64 APPLE_NOTARY_KEY_ID APPLE_NOTARY_ISSUER; do
    [[ -n "${!name:-}" ]] || { echo "Required signing credential missing: $name" >&2; exit 1; }
done
# A full certificate name avoids substring selection and rules out ad-hoc signing.
team_id=$(python3 - <<'PY'
import os
import re
identity = os.environ['APPLE_SIGNING_IDENTITY']
match = re.fullmatch(r'Developer ID Application: [^\r\n]+ \(([A-Z0-9]{10})\)', identity)
if not match:
    raise SystemExit('APPLE_SIGNING_IDENTITY must be an exact Developer ID Application name with a 10-character Team ID; ad-hoc signing is forbidden')
print(match.group(1))
PY
)
[[ "${GITHUB_REPOSITORY:-}" == 1jehuang/jcode && "${GITHUB_REF:-}" == refs/heads/master ]] || {
    echo 'Signing requires canonical master' >&2; exit 1;
}
[[ "$(uname -s)" == Darwin ]] || { echo 'Signing requires macOS' >&2; exit 1; }
umask 077
work=$(mktemp -d)
keychain="$work/handoff.keychain-db"
password=$(openssl rand -hex 32)
cleanup() {
    local status=$?
    if [[ -f "$work/keychains.txt" ]]; then
        # Restore the original search list before removing our temporary identity.
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
# Snapshot before create-keychain, which can itself change the search list.
security list-keychains -d user > "$work/keychains.pending"
mv "$work/keychains.pending" "$work/keychains.txt"
printf '%s' "$APPLE_CERTIFICATE_P12_BASE64" | base64 --decode > "$work/certificate.p12"
printf '%s' "$APPLE_NOTARY_KEY_P8_BASE64" | base64 --decode > "$work/AuthKey.p8"
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
security import "$work/certificate.p12" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple: -s -k "$password" "$keychain" >/dev/null
# An explicit codesign --keychain alone is not sufficient for identity discovery
# on a fresh runner. Preserve existing trust-chain keychains in the search list.
python3 - "$work/keychains.txt" "$keychain" <<'PY'
from pathlib import Path
import shlex
import subprocess
import sys
subprocess.run(['security', 'list-keychains', '-d', 'user', '-s', sys.argv[2],
                *shlex.split(Path(sys.argv[1]).read_text())], check=True)
PY
echo 'Signing keychain imported and added to user search list'
# Resolve an exact full name in this isolated keychain, then sign by SHA1 rather
# than letting codesign choose a partial-name match from another certificate.
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
    raise SystemExit('Expected exactly one matching Developer ID Application identity in the temporary keychain')
print(matches[0])
PY
)
mkdir signed-handoff
cp handoff/jcode signed-handoff/jcode
chmod 755 signed-handoff/jcode
codesign --force --sign "$certificate_sha1" --keychain "$keychain" \
    --identifier com.jcode.cli --options runtime --timestamp signed-handoff/jcode
echo 'Developer ID signature created'
# Match justrust's Apple Developer ID chain and stable-identifier requirement,
# additionally binding the leaf Team ID to the exact identity selected above.
requirement='anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and identifier "com.jcode.cli"'
requirement+=" and certificate leaf[subject.OU] = \"$team_id\""
codesign --verify --strict --verbose=2 -R "$requirement" signed-handoff/jcode
# Exercise the signed image with the same credential-free, bounded native smoke
# checks before sending anything to Apple's notary service.
python3 - <<'PY'
from pathlib import Path
import sys
sys.path.insert(0, 'scripts')
from native_handoff import smoke
smoke(Path('signed-handoff'))
PY
# CLI executables cannot carry stapled tickets. Preserve the accepted submission
# result with the signed ZIP instead of claiming stapler or offline Gatekeeper acceptance.
ditto -c -k --keepParent signed-handoff/jcode "$work/jcode.zip"
xcrun notarytool submit "$work/jcode.zip" --key "$work/AuthKey.p8" \
    --key-id "$APPLE_NOTARY_KEY_ID" --issuer "$APPLE_NOTARY_ISSUER" \
    --wait --timeout 20m --output-format json > signed-handoff/notarization.json
python3 - <<'PY'
import json
from pathlib import Path
result = json.loads(Path('signed-handoff/notarization.json').read_text())
if result.get('status') != 'Accepted':
    raise SystemExit('Notarization was not accepted')
PY
cp "$work/jcode.zip" signed-handoff/jcode.zip
# The original provenance describes the unsigned binary only. Record the
# transformation without silently replacing the original build digest.
python3 - <<'PY'
import hashlib
import json
from pathlib import Path
original = json.loads(Path('handoff/provenance.json').read_text())
record = {'unsigned_provenance': original,
          'signed_sha256': hashlib.sha256(Path('signed-handoff/jcode').read_bytes()).hexdigest(),
          'notarization': json.loads(Path('signed-handoff/notarization.json').read_text())}
Path('signed-handoff/signing-provenance.json').write_text(json.dumps(record, indent=2) + '\n')
PY
