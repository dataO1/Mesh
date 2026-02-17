# Build the Rust workspace using rustPlatform
# Outputs: mesh-player and mesh-cue binaries
#
# Default features include jack-backend for Linux (JACK audio with port routing)
# The jack dependency is Linux-only, so this works correctly on all platforms
{ pkgs, common, src }:

let
  # Filtered source - only includes files needed for Rust compilation
  # Prevents rebuilds when unrelated files change (distrobox.ini, packaging/, etc.)
  rustSrc = pkgs.lib.cleanSourceWith {
    inherit src;
    filter = path: type:
      let
        baseName = baseNameOf path;
      in
      # Always include directories (filter will recurse into them)
      type == "directory" ||
      # Cargo files
      baseName == "Cargo.toml" ||
      baseName == "Cargo.lock" ||
      # Rust source files + compile-time includes (WGSL shaders)
      pkgs.lib.hasSuffix ".rs" baseName ||
      pkgs.lib.hasSuffix ".wgsl" baseName;
  };

  # Build inputs for mesh packages
  meshBuildInputs = common.runtimeInputs ++ [
    common.essentia
  ] ++ common.essentiaDeps;

  # Patched crate sources fetched from crates.io (not vendored due to [patch.crates-io])
  libpdSysSrc = pkgs.fetchurl {
    url = "https://crates.io/api/v1/crates/libpd-sys/0.3.4/download";
    name = "libpd-sys-0.3.4.tar.gz";
    hash = "sha256-bK5NpFB2HQsIWiRqte7px7mEBsQ/lNFtDXGqGfbUrxI=";
  };
  libpdRsSrc = pkgs.fetchurl {
    url = "https://crates.io/api/v1/crates/libpd-rs/0.2.0/download";
    name = "libpd-rs-0.2.0.tar.gz";
    hash = "sha256-KyEm7K9x1d1N1dALHif6hsQJdceaJHfqjFThN7JRi0Q=";
  };

in pkgs.rustPlatform.buildRustPackage {
  pname = "mesh";
  version = "0.8.3";
  src = rustSrc;

  # Cargo.lock hash - update this when deps change
  # Run: nix build .#mesh-build 2>&1 | grep "got:" to get new hash
  cargoHash = "sha256-Au58ZpzGMZ/LsaO009qofHGaEnYnP5sTs5eEb8RRrbM=";

  nativeBuildInputs = with pkgs; [
    pkg-config
    cmake
    clang
    llvmPackages.libclang
    gnumake
  ];

  buildInputs = meshBuildInputs;

  # Build environment
  LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
  BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.glibc.dev}/include -isystem ${pkgs.llvmPackages.libclang.lib}/lib/clang/21/include";
  PKG_CONFIG_PATH = "${common.essentia}/lib/pkgconfig";
  USE_TENSORFLOW = "0";
  CPLUS_INCLUDE_PATH = "${pkgs.eigen}/include/eigen3";

  # Recreate patched crates from crates.io sources.
  # [patch.crates-io] in Cargo.toml redirects these to patches/, so they're
  # never vendored. We fetch them separately and apply patches here.
  preBuild = ''
    mkdir -p patches

    if [ ! -d "patches/libpd-sys" ]; then
      echo "Creating patched libpd-sys (32-bit floats)..."
      tar xzf ${libpdSysSrc} -C patches
      mv patches/libpd-sys-* patches/libpd-sys
      chmod -R u+w patches/libpd-sys
      sed -i 's/const PD_FLOATSIZE: &str = "64"/const PD_FLOATSIZE: \&str = "32"/' patches/libpd-sys/build.rs
      echo "  done"
    fi

    if [ ! -d "patches/libpd-rs" ]; then
      echo "Creating patched libpd-rs (c_char portability)..."
      tar xzf ${libpdRsSrc} -C patches
      mv patches/libpd-rs-* patches/libpd-rs
      chmod -R u+w patches/libpd-rs
      sed -i 's/\*const i8/\*const os::raw::c_char/g' patches/libpd-rs/src/functions/receive.rs
      echo "  done"
    fi
  '';

  # Build specific packages (default features include jack-backend for Linux)
  cargoBuildFlags = [ "-p" "mesh-player" "-p" "mesh-cue" ];

  meta = with pkgs.lib; {
    description = "DJ Player and Cue Software";
    license = licenses.agpl3Plus;
  };
}
