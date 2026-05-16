#!/usr/bin/env bash
# Install a checkout-aware jcode-dev launcher shim.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
install_dir="${JCODE_INSTALL_DIR:-$HOME/.local/bin}"
shim="$install_dir/jcode-dev"
script="$repo_root/scripts/jcode-dev.sh"

if [[ ! -x "$script" ]]; then
  echo "jcode-dev source script is not executable: $script" >&2
  exit 1
fi

mkdir -p "$install_dir"

cat > "$shim" <<SHIM
#!/usr/bin/env bash
set -euo pipefail

repo_root="$repo_root"
script="\$repo_root/scripts/jcode-dev.sh"

if [[ ! -x "\$script" ]]; then
  echo "jcode-dev checkout is missing or not executable: \$script" >&2
  echo "Reinstall with: JCODE_INSTALL_DIR=\"${install_dir}\" <checkout>/scripts/install_jcode_dev.sh" >&2
  exit 1
fi

exec "\$script" "\$@"
SHIM

chmod +x "$shim"

echo "Installed jcode-dev shim: $shim -> $script"
if ! echo "$PATH" | tr ':' '\n' | grep -qx "$install_dir"; then
  echo "Tip: add $install_dir to PATH if needed."
fi
