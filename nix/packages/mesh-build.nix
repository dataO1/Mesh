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
      # Rust source files
      pkgs.lib.hasSuffix ".rs" baseName;
  };

  # Build inputs for mesh packages
  meshBuildInputs = common.runtimeInputs ++ [
    common.essentia
  ] ++ common.essentiaDeps;

  # libpd-sys crate source (patched for 32-bit floats in preBuild)
  # The workspace Cargo.toml uses [patch.crates-io] to override libpd-sys
  # with a local path, so it's NOT included in the cargo vendor.
  libpdSysSrc = pkgs.fetchurl {
    url = "https://crates.io/api/v1/crates/libpd-sys/0.3.4/download";
    name = "libpd-sys-0.3.4.tar.gz";
    hash = "sha256-bK5NpFB2HQsIWiRqte7px7mEBsQ/lNFtDXGqGfbUrxI=";
  };

in pkgs.rustPlatform.buildRustPackage {
  pname = "mesh";
  version = "0.8.3";
  src = rustSrc;

  # Cargo.lock hash - update this when deps change
  # Run: nix build .#mesh-build 2>&1 | grep "got:" to get new hash
  cargoHash = "sha256-Gy2ECMhZdbwHJk/Jh7nXwPMUDmfLP1Rbt+I9125UfP0=";

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

  # Recreate patched libpd-sys from crates.io source.
  # The [patch.crates-io] in Cargo.toml redirects libpd-sys to patches/libpd-sys,
  # so the crate is never vendored. We fetch it separately and apply the float patch.
  preBuild = ''
    if [ ! -d "patches/libpd-sys" ]; then
      echo "Creating patched libpd-sys (32-bit floats)..."
      mkdir -p patches
      tar xzf ${libpdSysSrc} -C patches
      mv patches/libpd-sys-* patches/libpd-sys
      chmod -R u+w patches/libpd-sys
      sed -i 's/const PD_FLOATSIZE: &str = "64"/const PD_FLOATSIZE: \&str = "32"/' patches/libpd-sys/build.rs
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
