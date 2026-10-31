#!/usr/bin/env bash
# Exercise the real cloud replace CLI against an isolated fake SSH machine.
set -euo pipefail
if [[ ${1:-} != --isolated ]]; then
  BINARY=$(realpath -e "${JCODE_BIN:?set JCODE_BIN to the binary under test}")
  SCRIPT=$(realpath -e "$0")
  W=$(mktemp -d "${JCODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}/cloud-replace-test.XXXXXX")
  mkdir -p "$W/home" "$W/runtime" "$W/tmp"
  exec env -i PATH=/usr/local/bin:/usr/bin:/bin LANG=C.UTF-8 HOME="$W/home" \
    TMPDIR="$W/tmp" XDG_RUNTIME_DIR="$W/runtime" JCODE_RUNTIME_DIR="$W/runtime" \
    JCODE_HOME="$W/home/.jcode" JCODE_NO_AUTO_UPDATE=1 JCODE_NO_TELEMETRY=1 \
    JCODE_NON_INTERACTIVE=1 W="$W" JCODE_BIN="$BINARY" bash "$SCRIPT" --isolated
fi
trap 'echo "Acceptance artifacts: $W"' EXIT
mkdir -p "$W/local/project space" "$W/cloud/projects" "$W/cloud/home"
REPO="$W/local/project space"
git -C "$REPO" init -q
git -C "$REPO" config user.name 'Cloud Replace Test'
git -C "$REPO" config user.email 'cloud-replace@example.invalid'
printf 'base\n' > "$REPO/file.txt"
git -C "$REPO" add .
git -C "$REPO" commit -qm initial
printf 'local dirty\n' > "$REPO/file.txt"
printf 'local untracked\n' > "$REPO/new.txt"
LOCAL_INDEX=$(sha256sum "$REPO/.git/index")
{
  printf '#!/usr/bin/env bash\nset -euo pipefail\n'
  printf 'W=%q\nBINARY=%q\n' "$W" "$JCODE_BIN"
  cat <<'TRANSPORT'
exec bwrap --die-with-parent --ro-bind / / --tmpfs /tmp --unshare-pid --proc /proc --dev /dev \
  --bind "$W/cloud/projects" "$W/local" \
  --bind "$W/cloud/home" "$W/home" --ro-bind "$BINARY" /tmp/jcode-replace-test \
  --clearenv --setenv PATH /usr/local/bin:/usr/bin:/bin --setenv HOME "$W/home" \
  --setenv JCODE_HOME "$W/home/.jcode" --setenv JCODE_NO_AUTO_UPDATE 1 \
  --setenv JCODE_NO_TELEMETRY 1 --setenv JCODE_NON_INTERACTIVE 1 \
  --setenv JCODE_RUNTIME_DIR "$W/home/runtime" --setenv XDG_RUNTIME_DIR "$W/home/runtime" \
  --chdir / bash -c "$1"
TRANSPORT
} > "$W/transport"
chmod +x "$W/transport"
cd "$REPO"
run_replace() {
  "$JCODE_BIN" --no-update --no-selfdev cloud replace --host cloud-replace-test \
    --remote-binary /tmp/jcode-replace-test --transport "$W/transport" "$@"
}
# Explicit confirmation is required before touching the destination.
if run_replace --json > "$W/unconfirmed.json" 2> "$W/unconfirmed.err"; then
  echo 'FAIL: replacement without confirmation succeeded' >&2
  exit 1
fi
[[ ! -e "$W/cloud/projects/project space" ]]

# Dry-run must leave the destination absent.
run_replace --dry-run --json > "$W/dry-run.json"
[[ ! -e "$W/cloud/projects/project space" ]]

# Initial upload preserves local changes without moving a conversation.
run_replace --yes --json > "$W/initial.json"
CLOUD_REPO="$W/cloud/projects/project space"
cmp "$REPO/file.txt" "$CLOUD_REPO/file.txt"
cmp "$REPO/new.txt" "$CLOUD_REPO/new.txt"
[[ $(git -C "$REPO" rev-parse HEAD) == $(git -C "$CLOUD_REPO" rev-parse HEAD) ]]
[[ ! -d "$W/home/.jcode/session_leases" ]]

# Divergent committed, uncommitted, and ignored cloud state all survive in backup.
printf 'cloud committed\n' > "$CLOUD_REPO/file.txt"
git -C "$CLOUD_REPO" add file.txt
git -C "$CLOUD_REPO" commit -qm cloud-only
CLOUD_HEAD=$(git -C "$CLOUD_REPO" rev-parse HEAD)
printf 'cloud dirty\n' > "$CLOUD_REPO/file.txt"
printf 'cloud only\n' > "$CLOUD_REPO/cloud-only.txt"
printf 'ignored.txt\n' > "$CLOUD_REPO/.gitignore"
printf 'cloud ignored\n' > "$CLOUD_REPO/ignored.txt"
run_replace --dry-run --json > "$W/existing-dry-run.json"
[[ $(cat "$CLOUD_REPO/file.txt") == 'cloud dirty' ]]
[[ $(cat "$CLOUD_REPO/ignored.txt") == 'cloud ignored' ]]
run_replace --yes --json > "$W/replaced.json"
cmp "$REPO/file.txt" "$CLOUD_REPO/file.txt"
[[ ! -e "$CLOUD_REPO/cloud-only.txt" && ! -e "$CLOUD_REPO/ignored.txt" ]]
BACKUP=$(python3 - "$W/replaced.json" "$W/local" "$W/cloud/projects" <<'PY'
import json, sys
report = json.load(open(sys.argv[1]))
path = report['backup_path']
assert path, report
assert path.startswith(sys.argv[2] + '/'), path
print(sys.argv[3] + path[len(sys.argv[2]):])
PY
)
[[ $(git -C "$BACKUP" rev-parse HEAD) == "$CLOUD_HEAD" ]]
[[ $(cat "$BACKUP/file.txt") == 'cloud dirty' ]]
[[ $(cat "$BACKUP/cloud-only.txt") == 'cloud only' ]]
[[ $(cat "$BACKUP/ignored.txt") == 'cloud ignored' ]]

# A cloud-owned moved session blocks replacement, even if it is idle.
mkdir -p "$W/cloud/home/.jcode/session_leases"
printf '{"session_id":"active-cloud","epoch":1,"repo_root":"%s","updated_at":"2026-10-26T00:00:00Z"}\n' "$REPO" \
  > "$W/cloud/home/.jcode/session_leases/active-cloud.json"
printf 'active session work\n' > "$CLOUD_REPO/file.txt"
if run_replace --yes --json > "$W/active.json" 2> "$W/active.err"; then
  echo 'FAIL: replaced a checkout owned by a cloud session' >&2
  exit 1
fi
[[ $(cat "$CLOUD_REPO/file.txt") == 'active session work' ]]
rg -q 'active-cloud' "$W/active.err"
# Unknown lease metadata also fails closed rather than risking another session.
printf 'invalid lease JSON\n' > "$W/cloud/home/.jcode/session_leases/active-cloud.json"
if run_replace --yes --json > "$W/corrupt-lease.json" 2> "$W/corrupt-lease.err"; then
  echo 'FAIL: ignored corrupt session ownership metadata' >&2
  exit 1
fi
rg -q 'cannot inspect cloud session lease' "$W/corrupt-lease.err"
rm -- "$W/cloud/home/.jcode/session_leases/active-cloud.json"
# Never replace a symlink or modify the checkout it points to.
mv "$CLOUD_REPO" "$W/cloud/projects/symlink-target"
ln -s "$W/local/symlink-target" "$CLOUD_REPO"
if run_replace --yes --json > "$W/symlink.json" 2> "$W/symlink.err"; then
  echo 'FAIL: replaced a symlink checkout' >&2
  exit 1
fi
[[ -L "$CLOUD_REPO" ]]
rg -q 'symlink' "$W/symlink.err"
[[ $(cat "$W/cloud/projects/symlink-target/file.txt") == 'active session work' ]]
# The local working tree must be untouched by every operation.
[[ $(cat "$REPO/file.txt") == 'local dirty' ]]
[[ $(cat "$REPO/new.txt") == 'local untracked' ]]
[[ $(sha256sum "$REPO/.git/index") == "$LOCAL_INDEX" ]]
printf 'PASS: cloud replace confirmation, previews, upload, backup, ownership, metadata, symlink, and local-state checks\n'
