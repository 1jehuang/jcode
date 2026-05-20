#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
strict=0

usage() {
  cat <<'USAGE'
Usage:
  scripts/security_preflight.sh [--strict]

Checks:
  1) Secret-pattern scan in tracked source/docs/scripts
  2) World-writable file check under scripts/
  3) Rust dependency advisory scan via cargo-audit (when available)

Options:
  --strict   Fail if cargo-audit is not installed
USAGE
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --strict)
      strict=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
  shift
done

cd "$repo_root"

echo "=== Security Preflight ==="

echo "[1/3] Scanning for likely secrets"
secret_regex='(AKIA[0-9A-Z]{16}|ASIA[0-9A-Z]{16}|gh[pousr]_[A-Za-z0-9]{36,}|xox[baprs]-[A-Za-z0-9-]{10,}|-----BEGIN (RSA|OPENSSH|EC|DSA|PGP) PRIVATE KEY-----|AIza[0-9A-Za-z_-]{35})'

set +e
mapfile -d '' tracked_files < <(git ls-files -z)
scan_status=1
if [[ "${#tracked_files[@]}" -gt 0 ]]; then
  if command -v rg >/dev/null 2>&1; then
    rg -n --color=never -e "$secret_regex" \
      --glob '!Cargo.lock' --glob '!*.snap' --glob '!*.png' --glob '!*.jpg' --glob '!*.jpeg' \
      --glob '!*.gif' --glob '!*.svg' --glob '!*.pdf' --glob '!*.woff' --glob '!*.woff2' --glob '!*.ttf' \
      "${tracked_files[@]}" > /tmp/jcode-secret-scan.txt
    scan_status=$?
  else
    scan_files=()
    for tracked_file in "${tracked_files[@]}"; do
      case "$tracked_file" in
        Cargo.lock|*.snap|*.png|*.jpg|*.jpeg|*.gif|*.svg|*.pdf|*.woff|*.woff2|*.ttf)
          ;;
        *)
          scan_files+=("$tracked_file")
          ;;
      esac
    done
    if [[ "${#scan_files[@]}" -gt 0 ]]; then
      grep -I -n -E "$secret_regex" "${scan_files[@]}" > /tmp/jcode-secret-scan.txt
      scan_status=$?
    fi
  fi
fi
set -e

if [[ "$scan_status" -gt 1 ]]; then
  rm -f /tmp/jcode-secret-scan.txt
  die "secret scan failed to execute"
fi

if [[ -s /tmp/jcode-secret-scan.txt ]]; then
  cat /tmp/jcode-secret-scan.txt
  rm -f /tmp/jcode-secret-scan.txt
  die "potential secret material detected"
fi
rm -f /tmp/jcode-secret-scan.txt

echo "[2/3] Checking script permissions"
if find scripts -type f -perm -0002 -print -quit | grep -q .; then
  find scripts -type f -perm -0002 -print
  die "world-writable files detected under scripts/"
fi

echo "[3/3] Dependency advisories (cargo-audit)"
audit_ignores=(
  # Documented in docs/SECURITY_DEPENDENCIES.md. These are transitive
  # advisories with tracked remediation paths; keep them visible in the triage
  # doc while preventing unrelated CI/release work from being blocked.
  --ignore RUSTSEC-2026-0141 # lettre via notify-email, Boring TLS backend not used by jcode
  --ignore RUSTSEC-2026-0099 # rustls-webpki via rustls stack, awaiting upstream upgrade
  --ignore RUSTSEC-2026-0104 # rustls-webpki via rustls stack, awaiting upstream upgrade
  --ignore RUSTSEC-2026-0098 # rustls-webpki via rustls stack, awaiting upstream upgrade
)
if command -v cargo-audit >/dev/null 2>&1; then
  cargo audit "${audit_ignores[@]}"
elif cargo audit --version >/dev/null 2>&1; then
  cargo audit "${audit_ignores[@]}"
else
  if [[ "$strict" -eq 1 ]]; then
    die "cargo-audit is not installed (install with: cargo install cargo-audit --locked)"
  fi
  echo "warning: cargo-audit not installed; skipping advisory check"
fi

echo "=== Security preflight passed ==="
