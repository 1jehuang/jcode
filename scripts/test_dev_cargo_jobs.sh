#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/home" "$tmp/work"

cat > "$tmp/bin/uname" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
  -s) printf '%s\n' "${TEST_UNAME_S:-Darwin}" ;;
  -m) printf '%s\n' "${TEST_UNAME_M:-arm64}" ;;
  *) printf '%s\n' "${TEST_UNAME_S:-Darwin}" ;;
esac
EOF

cat > "$tmp/bin/nproc" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "${TEST_CPU_COUNT:-8}"
EOF

cat > "$tmp/bin/vm_stat" <<'EOF'
#!/usr/bin/env bash
if [[ "${TEST_VM_STAT_MODE:-valid}" == "failed" ]]; then
  exit 1
fi
if [[ "${TEST_VM_STAT_MODE:-valid}" == "malformed" ]]; then
  cat <<'STATS'
Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                               unknown.
STATS
  exit 0
fi
cat <<STATS
Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                               32768.
Pages active:                            99999.
Pages inactive:                         393216.
Pages speculative:                       32768.
Pages throttled:                             0.
Pages wired down:                        99999.
Pages purgeable:                         65536.
STATS
EOF

chmod +x "$tmp/bin/uname" "$tmp/bin/nproc" "$tmp/bin/vm_stat"

run_setup() {
  (
    unset CARGO_BUILD_JOBS JCODE_BUILD_JOBS
    export PATH="$tmp/bin:$PATH"
    export HOME="$tmp/home"
    export TMPDIR="$tmp/work"
    export JCODE_BUILD_GIT_HASH=test
    export JCODE_PARALLEL_FRONTEND=0
    export SCCACHE_DISABLE=1
    for assignment in "$@"; do
      export "$assignment"
    done
    "$repo_root/scripts/dev_cargo.sh" --print-setup
  )
}

assert_line() {
  local output="$1" expected="$2"
  if ! grep -Fqx "$expected" <<<"$output"; then
    printf 'expected setup output to contain %q\noutput:\n%s\n' "$expected" "$output" >&2
    exit 1
  fi
}

# 32,768 free + 393,216 inactive + 32,768 speculative 16 KiB pages =
# 7,168 MiB available. At 1,792 MiB/job, memory limits an 8-CPU Mac to 4 jobs.
output=$(run_setup)
assert_line "$output" 'os=Darwin'
assert_line "$output" 'build_jobs_status=adaptive:4 (cpus=8, mem_avail=7168MiB, budget=1792MiB/job)'
assert_line "$output" 'cargo_build_jobs=4'

# Both documented overrides bypass memory probing, with JCODE_BUILD_JOBS taking
# precedence when both are present.
output=$(run_setup TEST_VM_STAT_MODE=failed CARGO_BUILD_JOBS=7)
assert_line "$output" 'build_jobs_status=override:7'
assert_line "$output" 'cargo_build_jobs=7'
output=$(run_setup TEST_VM_STAT_MODE=failed CARGO_BUILD_JOBS=7 JCODE_BUILD_JOBS=3)
assert_line "$output" 'build_jobs_status=override:3'
assert_line "$output" 'cargo_build_jobs=3'

# If vm_stat cannot be read, leave CARGO_BUILD_JOBS unset so Cargo can apply its
# own config/default rather than guessing from invalid memory data.
output=$(run_setup TEST_VM_STAT_MODE=failed)
assert_line "$output" 'build_jobs_status=cargo-default'
assert_line "$output" 'cargo_build_jobs=<unset>'
output=$(run_setup TEST_VM_STAT_MODE=malformed)
assert_line "$output" 'build_jobs_status=cargo-default'
assert_line "$output" 'cargo_build_jobs=<unset>'
output=$(run_setup TEST_UNAME_S=FreeBSD)
assert_line "$output" 'build_jobs_status=cargo-default'
assert_line "$output" 'cargo_build_jobs=<unset>'

echo 'dev_cargo job sizing tests passed'
