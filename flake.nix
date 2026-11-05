{
  description = "FHS Rust dev + static musl release builds";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };

        # Собираем Rust-тулчейн с поддержкой musl
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          targets = [ "x86_64-unknown-linux-musl" ];
        };

        proxyUrl = "http://127.0.0.1:10808";
        fhs = pkgs.buildFHSEnv {
          name = "cray-fhs";

          targetPkgs = pkgs: with pkgs; [
            rustToolchain # <-- Вместо обычных rustc/cargo подставляем наш тулчейн
            gcc clang llvmPackages.libclang
            cmake gnumake pkg-config
            glibc glibc.dev openssl libunwind zlib
            pkgsCross.musl64.stdenv.cc
          ];

          profile = ''
            export HTTP_PROXY="${proxyUrl}"
            export HTTPS_PROXY="${proxyUrl}"
            export http_proxy="${proxyUrl}"
            export https_proxy="${proxyUrl}"
            export ALL_PROXY="${proxyUrl}"
            export all_proxy="${proxyUrl}"
            export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"
            export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="gcc"
            export CC_x86_64_unknown_linux_musl="x86_64-unknown-linux-musl-gcc"
            export CXX_x86_64_unknown_linux_musl="x86_64-unknown-linux-musl-g++"
            export AR_x86_64_unknown_linux_musl="x86_64-unknown-linux-musl-ar"
            export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="x86_64-unknown-linux-musl-gcc"
          '';
          runScript = "zsh";
        };

      in
      {
        devShells.default = fhs.env;
      }
    );
}