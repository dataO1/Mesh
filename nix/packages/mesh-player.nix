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
{ pkgs, common, version, src }:

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
      pkgs.lib.hasSuffix ".rs" baseName ||
      pkgs.lib.hasSuffix ".wgsl" baseName;
  };

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
  pname = "mesh-player";
  inherit version;
  src = rustSrc;

  # Use lockfile directly — no manual hash updates needed on dep changes.
  # allowBuiltinFetchGit handles git deps (clack, baseview, graph).
  cargoLock = {
    lockFile = ./../../Cargo.lock;
    allowBuiltinFetchGit = true;
  };

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

  # Only build mesh-player (no mesh-cue)
  cargoBuildFlags = [ "-p" "mesh-player" ];

  # Skip tests — cargo test tries to compile the full workspace (including
  # mesh-cue/ort-sys which downloads ONNX binaries, blocked by the sandbox)
  doCheck = false;

  meta = with pkgs.lib; {
    description = "Mesh DJ Player — standalone stem mixing performance application";
    license = licenses.agpl3Plus;
    platforms = platforms.linux;
  };
}
