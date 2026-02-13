{
  description = "Mesh - DJ Player and Cue Software";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    nn-tilde = {
      url = "github:acids-ircam/nn_tilde";
      flake = false;
    };
    demucs-onnx = {
      url = "github:sevagh/demucs.onnx";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, nn-tilde, demucs-onnx }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };

        # Rust toolchain
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        };

        # Import shared common definitions
        common = import ./nix/common.nix { inherit pkgs; };

        # =======================================================================
        # Packages
        # =======================================================================

        # Native Rust build (Linux/NixOS)
        meshBuild = import ./nix/packages/mesh-build.nix {
          inherit pkgs common;
          src = ./.;
        };

        # nn~ Pure Data external for neural audio effects
        nnTilde = import ./nix/packages/nn-tilde.nix {
          inherit (pkgs) lib stdenv llvmPackages fetchFromGitHub cmake puredata libtorch-bin curl patchelf;
        };

        # =======================================================================
        # Apps (container-based builds for portability)
        # =======================================================================

        # Windows cross-compilation (container-based)
        # Pass Linux essentia for host builds (cross-compilation needs both)
        buildWindowsApp = import ./nix/apps/build-windows.nix {
          inherit pkgs;
          essentiaLinux = common.essentia;
        };

        # Portable .deb build (container-based, Ubuntu 22.04 for glibc 2.35)
        # CPU-only version (works everywhere)
        buildDebApp = import ./nix/apps/build-deb.nix {
          inherit pkgs;
          enableCuda = false;
        };

        # CUDA-enabled .deb build (requires NVIDIA GPU + CUDA 12 on target)
        buildDebCudaApp = import ./nix/apps/build-deb.nix {
          inherit pkgs;
          enableCuda = true;
        };

        # ONNX model conversion (Demucs PyTorch → ONNX)
        convertModelApp = import ./nix/apps/convert-model.nix {
          inherit pkgs demucs-onnx;
        };

        # ML classification head conversion (Essentia TF → ONNX)
        convertMlModelApp = import ./nix/apps/convert-ml-model.nix {
          inherit pkgs;
        };

        # Build nn~ Pure Data external for neural audio effects
        buildNnTildeApp = import ./nix/apps/build-nn-tilde.nix {
          inherit pkgs;
          nn-tilde = nnTilde;
        };

        # =======================================================================
        # Development Shell
        # =======================================================================

        devShell = import ./nix/devshell.nix {
          inherit pkgs common rustToolchain;
        };

      in
      {
        # Export packages
        packages = {
          mesh-build = meshBuild;
          nn-tilde = nnTilde;
          default = meshBuild;
        };

        devShells.default = devShell;

        # For CI/CD or quick checks
        checks = {
          format = pkgs.runCommand "check-format" {
            nativeBuildInputs = [ rustToolchain ];
          } ''
            cd ${./.}
            cargo fmt --check
            touch $out
          '';
        };

        # Runnable apps
        apps = {
          build-windows = {
            type = "app";
            program = "${buildWindowsApp}/bin/build-windows";
          };
          build-deb = {
            type = "app";
            program = "${buildDebApp}/bin/build-deb";
          };
          build-deb-cuda = {
            type = "app";
            program = "${buildDebCudaApp}/bin/build-deb-cuda";
          };
          convert-model = {
            type = "app";
            program = "${convertModelApp}/bin/convert-model";
          };
          convert-ml-model = {
            type = "app";
            program = "${convertMlModelApp}/bin/convert-ml-model";
          };
          build-nn-tilde = {
            type = "app";
            program = "${buildNnTildeApp}/bin/build-nn-tilde";
          };
        };
      }
    );
}
