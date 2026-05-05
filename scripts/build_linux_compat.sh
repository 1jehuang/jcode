#!/usr/bin/env bash
set -euo pipefail

# Build a Linux x86_64 release artifact against the CentOS 7 / manylinux2014
# glibc 2.17 baseline so the resulting binary runs on older distributions as
# well as newer Debian/Ubuntu containers used by Terminal-Bench tasks.

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
out_dir="${1:-$repo_root/dist}"

if [[ "$#" -gt 1 ]]; then
  echo "Usage: $0 [out-dir]" >&2
  exit 1
fi

if [[ "$out_dir" != /* ]]; then
  out_dir="$repo_root/$out_dir"
fi

artifact="${JCODE_COMPAT_ARTIFACT:-jcode-linux-x86_64}"
profile="${JCODE_COMPAT_PROFILE:-release}"
image="${JCODE_COMPAT_IMAGE:-quay.io/pypa/manylinux2014_x86_64}"
cache_root="${JCODE_COMPAT_CACHE_DIR:-$HOME/.cache/jcode-linux-compat}"
target="x86_64-unknown-linux-gnu"

mkdir -p "$out_dir" \
  "$cache_root/cargo-registry" \
  "$cache_root/cargo-git" \
  "$cache_root/rustup"

host_uid="$(id -u)"
host_gid="$(id -g)"

echo "Building portable Linux release in Docker image: $image"
echo "Output dir: $out_dir"

docker run --rm \
  -e CARGO_TERM_COLOR=always \
  -e JCODE_RELEASE_BUILD="${JCODE_RELEASE_BUILD:-1}" \
  -e JCODE_BUILD_SEMVER="${JCODE_BUILD_SEMVER:-}" \
  -e JCODE_COMPAT_PROFILE="$profile" \
  -e JCODE_COMPAT_TARGET="$target" \
  -e HOST_UID="$host_uid" \
  -e HOST_GID="$host_gid" \
  -v "$repo_root:/work" \
  -v "$out_dir:/out" \
  -v "$cache_root/cargo-registry:/root/.cargo/registry" \
  -v "$cache_root/cargo-git:/root/.cargo/git" \
  -v "$cache_root/rustup:/root/.rustup" \
  -w /work \
  "$image" \
  bash -lc '
    set -euo pipefail
    if command -v apt-get >/dev/null 2>&1; then
      export DEBIAN_FRONTEND=noninteractive
      apt-get update -qq
      apt-get install -y -qq \
        build-essential \
        ca-certificates \
        curl \
        git \
        libssl-dev \
        pkg-config
    elif command -v yum >/dev/null 2>&1; then
      yum install -y \
        ca-certificates \
        curl \
        gcc \
        gcc-c++ \
        git \
        make \
        openssl-devel \
        pkgconfig \
        tar \
        gzip
      update-ca-trust || true
    else
      echo "Unsupported build image: expected apt-get or yum" >&2
      exit 1
    fi

    if [[ ! -x /root/.cargo/bin/cargo ]]; then
      curl https://sh.rustup.rs -sSf | sh -s -- -y --profile minimal --default-toolchain stable
    fi
	    source /root/.cargo/env

	    export CARGO_TARGET_DIR=/work/target/linux-compat
	    export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
	    export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="${CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS:--C link-arg=-static-libgcc}"
	    cargo build --profile "$JCODE_COMPAT_PROFILE" --target "$JCODE_COMPAT_TARGET" -p jcode --bin jcode

	    cp "$CARGO_TARGET_DIR/$JCODE_COMPAT_TARGET/$JCODE_COMPAT_PROFILE/jcode" "/out/'"$artifact"'.bin"
	    chmod +x "/out/'"$artifact"'.bin"
	    cat > "/out/'"$artifact"'" <<WRAPPER
#!/usr/bin/env sh
set -eu
self_path=\$0
if command -v readlink >/dev/null 2>&1; then
  resolved=\$(readlink -f -- "\$0" 2>/dev/null || true)
  if [ -n "\$resolved" ]; then
    self_path=\$resolved
  fi
fi
case "\$self_path" in
  */*) self_dir=\$(CDPATH= cd -- "\$(dirname -- "\$self_path")" && pwd) ;;
  *) self_dir=\$(pwd) ;;
esac
if [ -n "\${LD_LIBRARY_PATH:-}" ]; then
  export LD_LIBRARY_PATH="\$self_dir:\$LD_LIBRARY_PATH"
else
  export LD_LIBRARY_PATH="\$self_dir"
fi
exec "\$self_dir/'"$artifact"'.bin" "\$@"
WRAPPER
	    chmod +x "/out/'"$artifact"'"

	    # Preserve the OpenSSL runtime libraries used by the build image. Some
	    # Terminal-Bench containers are older than the build host and either lack
	    # libssl entirely or expose a different SONAME. The Harbor adapter uploads
	    # these sibling libraries and sets LD_LIBRARY_PATH for the jcode process.
	    ldd "/out/'"$artifact"'.bin" \
	      | awk "/lib(ssl|crypto)[.]so/ { print \$3 }" \
	      | while read -r lib; do
	          if [[ -n "$lib" && -f "$lib" ]]; then
	            cp -L "$lib" /out/
	          fi
	        done

	    (cd /out && tar czf '"$artifact"'.tar.gz '"$artifact"' '"$artifact"'.bin libssl.so* libcrypto.so*)

	    chown "$HOST_UID:$HOST_GID" "/out/'"$artifact"'" "/out/'"$artifact"'.bin" "/out/'"$artifact"'.tar.gz" /out/libssl.so* /out/libcrypto.so* 2>/dev/null || true
	  '

echo "Built artifacts:"
ls -lh "$out_dir/$artifact" "$out_dir/$artifact.tar.gz"
