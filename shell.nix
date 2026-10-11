{
  pkgs ? import <nixpkgs> { },
}:

pkgs.mkShell {
  name = "jcode-dev";

  # Cargo shells out to `git` for the two git dependencies in Cargo.lock
  # (agentgrep, mermaid-rs-renderer); clippy and rustfmt mirror CI
  # (.github/workflows/ci.yml).
  #
  # clang and mold are what scripts/dev_cargo.sh looks for to enable the fast
  # linker: with both on PATH it exports the clang driver plus
  # `-C link-arg=-fuse-ld=mold`, which matters because the jcode binary relinks
  # on every build.
  nativeBuildInputs = with pkgs; [
    cargo
    rustc
    clippy
    rustfmt
    rust-analyzer
    clang
    mold
    git
  ];

  # openssl-sys is the only crate that needs a system library. The C code in
  # libsqlite3-sys, onig_sys, ring and aws-lc-sys is vendored and built by the
  # `cc` crate, so no sqlite, oniguruma, cmake or perl are needed. bindgen is
  # only pulled in by coreaudio-sys on macOS, so libclang is not needed here.
  buildInputs = with pkgs; [
    openssl
  ];

  OPENSSL_DIR = "${pkgs.openssl.dev}";
  OPENSSL_LIB_DIR = "${pkgs.openssl.out}/lib";
  OPENSSL_INCLUDE_DIR = "${pkgs.openssl.dev}/include";

  shellHook = ''
    echo "jcode dev shell: $(rustc --version), $(cargo --version)"
  '';
}
