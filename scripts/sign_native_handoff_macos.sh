#!/usr/bin/env bash
# Only called after same-run provenance and native smoke verification.
set -euo pipefail
for name in APPLE_CERTIFICATE_P12_BASE64 APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY \
            APPLE_NOTARY_KEY_P8_BASE64 APPLE_NOTARY_KEY_ID APPLE_NOTARY_ISSUER; do
    [[ -n "${!name:-}" ]] || { echo "Required signing credential missing: $name" >&2; exit 1; }
done
[[ "${GITHUB_REPOSITORY:-}" == 1jehuang/jcode && "${GITHUB_REF:-}" == refs/heads/master ]] || {
    echo 'Signing requires canonical master' >&2; exit 1;
}
[[ "$(uname -s)" == Darwin ]] || { echo 'Signing requires macOS' >&2; exit 1; }
work=$(mktemp -d)
keychain="$work/handoff.keychain-db"
password=$(openssl rand -hex 32)
cleanup() {
    security delete-keychain "$keychain" >/dev/null 2>&1 || true
    rm -rf -- "$work"
}
trap cleanup EXIT
umask 077
printf '%s' "$APPLE_CERTIFICATE_P12_BASE64" | base64 --decode > "$work/certificate.p12"
printf '%s' "$APPLE_NOTARY_KEY_P8_BASE64" | base64 --decode > "$work/AuthKey.p8"
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
security import "$work/certificate.p12" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple: -s -k "$password" "$keychain"
mkdir signed-handoff
cp handoff/jcode signed-handoff/jcode
chmod 755 signed-handoff/jcode
codesign --force --sign "$APPLE_SIGNING_IDENTITY" --keychain "$keychain" \
    --identifier com.jcode.cli --options runtime --timestamp signed-handoff/jcode
codesign --verify --strict --verbose=2 signed-handoff/jcode
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
