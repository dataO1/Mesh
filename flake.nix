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

    # Orange Pi 5 / RK3588 board support for embedded NixOS
    nixos-rk3588 = {
      url = "github:gnull/nixos-rk3588";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, nn-tilde, demucs-onnx, nixos-rk3588 }:
    let
      # =====================================================================
      # Embedded NixOS configuration (Orange Pi 5 Pro)
      # =====================================================================
      # Cross-compiled from x86_64: no binfmt, no host system changes.
      # Standard NixOS packages come from cache.nixos.org (aarch64-linux).
      # Custom packages (mesh-player, essentia) are built by CI and hosted
      # on GitHub Pages as a binary cache.
      #
      # Build SD image:  nix build .#sdImage
      # Deploy updates:  nixos-rebuild switch --fast --flake .#mesh-embedded \
      #                    --target-host mesh@orangepi --use-remote-sudo

      embeddedPkgs = import nixpkgs {
        localSystem.system = "x86_64-linux";
        crossSystem.system = "aarch64-linux";
        overlays = [ (import rust-overlay) ];
      };

      embeddedCommon = import ./nix/common.nix { pkgs = embeddedPkgs; };

      embeddedMeshPlayer = import ./nix/packages/mesh-player.nix {
        pkgs = embeddedPkgs;
        common = embeddedCommon;
        src = ./.;
      };

      embeddedOutputs = {
        # NixOS system configuration for the Orange Pi 5 Pro
        nixosConfigurations.mesh-embedded = nixpkgs.lib.nixosSystem {
          modules = [
            # Board support (kernel, bootloader, device tree)
            nixos-rk3588.nixosModules.orangepi5

            # Cross-compilation: build on x86_64, target aarch64
            {
              nixpkgs.buildPlatform.system = "x86_64-linux";
              nixpkgs.hostPlatform.system = "aarch64-linux";
              nixpkgs.overlays = [ (import rust-overlay) ];
            }

            # Mesh embedded configuration
            ./nix/embedded/configuration.nix

            # Inject the mesh-player package
            {
              services.mesh-embedded.package = embeddedMeshPlayer;
            }
          ];
        };

        # Convenience: build the SD card image from x86_64
        # Usage: nix build .#sdImage
        packages.x86_64-linux.sdImage =
          self.nixosConfigurations.mesh-embedded.config.system.build.sdImage;
      };
    in
    # Merge per-system outputs (packages, devShells, apps) with
    # top-level outputs (nixosConfigurations, sdImage)
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
    ) // embeddedOutputs;
}
