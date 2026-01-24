# Build the Rust workspace using rustPlatform
# Outputs: mesh-player and mesh-cue binaries
#
# Feature flags:
#   jackBackend (default: true) - Enable native JACK backend for port-level routing
#                                 Set to false for CPAL-only builds (cross-platform)
{ pkgs, common, src, jackBackend ? true }:

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

in pkgs.rustPlatform.buildRustPackage {
  pname = "mesh";
  version = "0.1.0";
  src = rustSrc;

  # Cargo.lock hash - update this when deps change
  # Run: nix build .#mesh-build 2>&1 | grep "got:" to get new hash
  cargoHash = "sha256-jMe47CJzn8W/fzTe0BNAUy+QsSMyeDSzp3866s67aeM=";

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

  # Build specific packages with optional JACK backend
  cargoBuildFlags = [ "-p" "mesh-player" "-p" "mesh-cue" ]
    ++ pkgs.lib.optionals jackBackend [ "--features" "jack-backend" ];

  meta = with pkgs.lib; {
    description = "DJ Player and Cue Software";
    license = licenses.agpl3Plus;
  };
}
