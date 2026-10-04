{
  lib,
  stdenvNoCC,
  fetchurl,
  autoPatchelfHook,
  openssl,
  stdenv,
}:
let
  release = builtins.fromJSON (builtins.readFile ./release.json);
  artifacts = {
    x86_64-linux = "jcode-linux-x86_64";
    aarch64-linux = "jcode-linux-aarch64";
    x86_64-darwin = "jcode-macos-x86_64";
    aarch64-darwin = "jcode-macos-aarch64";
  };
  system = stdenvNoCC.hostPlatform.system;
  artifact = artifacts.${system};
in
assert release.tag == "v${release.version}";
stdenvNoCC.mkDerivation {
  pname = "jcode-bin";
  inherit (release) version;

  src = fetchurl {
    url = "https://github.com/1jehuang/jcode/releases/download/${release.tag}/${artifact}.tar.gz";
    hash = release.hashes.${system};
  };

  sourceRoot = ".";
  nativeBuildInputs = lib.optionals stdenvNoCC.hostPlatform.isLinux [ autoPatchelfHook ];
  buildInputs = lib.optionals stdenvNoCC.hostPlatform.isLinux [
    stdenv.cc.cc.lib
    openssl
  ];
  dontBuild = true;
  dontStrip = true;
  dontFixup = stdenvNoCC.hostPlatform.isDarwin;

  installPhase = ''
    runHook preInstall
    binary=${artifact}
    # Portable Linux archives use a launcher beside the real ELF binary.
    if [ -f "$binary.bin" ]; then
      binary="$binary.bin"
    fi
    install -Dm755 "$binary" "$out/bin/jcode"
    for library in *.so*; do
      if [ -f "$library" ]; then
        install -Dm755 "$library" "$out/lib/$library"
      fi
    done
    runHook postInstall
  '';

  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck
    "$out/bin/jcode" --version | grep -F "v${release.version}"
    runHook postInstallCheck
  '';

  meta = {
    mainProgram = "jcode";
    homepage = "https://github.com/1jehuang/jcode";
    description = "Jcode coding agent from the official binary release";
    license = lib.licenses.mit;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
    platforms = builtins.attrNames artifacts;
  };
}
