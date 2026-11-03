{
  description = "Optional Jcode Rust development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            # mkShell supplies the platform C/C++ compiler through stdenv.
            packages = with pkgs; [
              cargo
              rustc
              rustfmt
              clippy
              rust-analyzer
              pkg-config
              cmake
              gnumake
              perl
              git
              ripgrep
            ];

            # macOS voice support generates CoreAudio bindings with libclang.
            nativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin [
              pkgs.rustPlatform.bindgenHook
            ];

            # Native TLS dependencies are used by parts of the provider stack.
            buildInputs = [ pkgs.openssl ];
          };
        });
    };
}
