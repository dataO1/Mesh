# Build mesh-player only (for embedded deployment)
#
# This is a stripped-down version of mesh-build.nix that:
# - Only builds mesh-player (no mesh-cue)
# - Uses strictDeps = true for cross-compilation support
# - Excludes mesh-cue-specific dependencies (ort/ONNX, load-dynamic)
#
# Cross-compilation: When buildPlatform != hostPlatform (e.g. building
# aarch64 from x86_64), Nix automatically configures a cross-compiler.
# nativeBuildInputs run on the build machine, buildInputs are for the target.
{ pkgs, common, src }:

let
  rustSrc = pkgs.lib.cleanSourceWith {
    inherit src;
    filter = path: type:
      let
        baseName = baseNameOf path;
      in
      type == "directory" ||
      baseName == "Cargo.toml" ||
      baseName == "Cargo.lock" ||
      pkgs.lib.hasSuffix ".rs" baseName;
  };

  meshBuildInputs = common.runtimeInputs ++ [
    common.essentia
  ] ++ common.essentiaDeps;

in pkgs.rustPlatform.buildRustPackage {
  pname = "mesh-player";
  version = "0.1.0";
  src = rustSrc;

  cargoHash = "sha256-kzbsyTReuXC3FMswdhjmkKQ9wM3Spltn0U+O6Q54Ccc=";

  # strictDeps: nativeBuildInputs run on build machine, buildInputs link for target
  strictDeps = true;

  nativeBuildInputs = with pkgs; [
    pkg-config
    cmake
    clang
    llvmPackages.libclang
    gnumake
  ];

  buildInputs = meshBuildInputs;

  LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
  BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.glibc.dev}/include -isystem ${pkgs.llvmPackages.libclang.lib}/lib/clang/21/include";
  PKG_CONFIG_PATH = "${common.essentia}/lib/pkgconfig";
  USE_TENSORFLOW = "0";
  CPLUS_INCLUDE_PATH = "${pkgs.eigen}/include/eigen3";

  # The workspace Cargo.toml patches libpd-sys to use 32-bit floats
  # (required for PD external compatibility). The patched source lives
  # in patches/libpd-sys locally (gitignored, created by devshell hook).
  # Recreate it from the vendored crate during the Nix build.
  preBuild = ''
    if [ ! -d "patches/libpd-sys" ]; then
      echo "Creating patched libpd-sys (32-bit floats)..."
      vendor_dir=$(find /build -maxdepth 1 -name '*-vendor*' -type d 2>/dev/null | head -1)
      if [ -n "$vendor_dir" ] && [ -d "$vendor_dir/libpd-sys" ]; then
        mkdir -p patches
        cp -r "$vendor_dir/libpd-sys" patches/libpd-sys
        chmod -R u+w patches/libpd-sys
        sed -i 's/const PD_FLOATSIZE: &str = "64"/const PD_FLOATSIZE: \&str = "32"/' patches/libpd-sys/build.rs
        echo "  done (from $vendor_dir)"
      else
        echo "WARNING: libpd-sys not found (vendor_dir=$vendor_dir)"
        echo "  /build contents: $(ls /build/)"
      fi
    fi
  '';

  # Only build mesh-player (no mesh-cue)
  cargoBuildFlags = [ "-p" "mesh-player" ];

  meta = with pkgs.lib; {
    description = "Mesh DJ Player — standalone stem mixing performance application";
    license = licenses.agpl3Plus;
    platforms = platforms.linux;
  };
}
