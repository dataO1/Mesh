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

  # libpd-sys crate source (patched for 32-bit floats in preBuild)
  # The workspace Cargo.toml uses [patch.crates-io] to override libpd-sys
  # with a local path, so it's NOT included in the cargo vendor.
  libpdSysSrc = pkgs.fetchurl {
    url = "https://crates.io/api/v1/crates/libpd-sys/0.3.4/download";
    name = "libpd-sys-0.3.4.tar.gz";
    hash = "sha256-TRzL2qaGOSo8Rx73VmCjc/FrYVr10ycqfrDAq//rYL8=";
  };

in pkgs.rustPlatform.buildRustPackage {
  pname = "mesh-player";
  version = "0.8.3";
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

  # Only build mesh-player (no mesh-cue)
  cargoBuildFlags = [ "-p" "mesh-player" ];

  meta = with pkgs.lib; {
    description = "Mesh DJ Player — standalone stem mixing performance application";
    license = licenses.agpl3Plus;
    platforms = platforms.linux;
  };
}
