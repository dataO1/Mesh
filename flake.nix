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
      # Built natively on aarch64 by GitHub Actions ARM runner.
      # Standard NixOS packages come from cache.nixos.org (aarch64-linux).
      # Custom packages (mesh-player, essentia) are built by CI and hosted
      # on GitHub Pages as a binary cache. The SD image is uploaded to
      # GitHub Releases with hash-based deduplication.

      # Native aarch64 pkgs for the vendor kernel (used by nixos-rk3588 board module)
      embeddedKernelPkgs = import nixpkgs {
        system = "aarch64-linux";
      };

      embeddedOutputs = {
        # NixOS system configuration for the Orange Pi 5 Pro
        # No buildPlatform override — defaults to the evaluating machine's arch:
        #   aarch64 CI runner → native build
        #   x86_64 dev machine → needs binfmt or --builders (not recommended, use CI)
        nixosConfigurations.mesh-embedded = nixpkgs.lib.nixosSystem {
          # The rk3588 board modules require these specialArgs:
          #   pkgsKernel — nixpkgs instance for building the vendor kernel
          #   nixpkgs    — path to nixpkgs source (for sd-image module import)
          specialArgs.rk3588 = {
            pkgsKernel = embeddedKernelPkgs;
            inherit nixpkgs;
          };
          modules = [
            # Board support: kernel, bootloader, device tree (split into core + sd-image)
            nixos-rk3588.nixosModules.boards.orangepi5.core
            nixos-rk3588.nixosModules.boards.orangepi5.sd-image

            # Target platform (buildPlatform defaults to host — native build)
            {
              nixpkgs.hostPlatform.system = "aarch64-linux";
              nixpkgs.overlays = [ (import rust-overlay) ];
            }

            # Mesh embedded configuration
            ./nix/embedded/configuration.nix

            # Build mesh-player from the NixOS module system's own pkgs
            # (native aarch64 on CI, no separate cross-compilation pkgs needed)
            ({ pkgs, ... }:
              let common = import ./nix/common.nix { inherit pkgs; };
              in {
                services.mesh-embedded.package = import ./nix/packages/mesh-player.nix {
                  inherit pkgs common;
                  src = self;
                };
              })
          ];
        };

        # SD card image — built natively on aarch64 CI runner
        # Usage (CI): nix build .#packages.aarch64-linux.sdImage
        packages.aarch64-linux.sdImage =
          self.nixosConfigurations.mesh-embedded.config.system.build.sdImage;
      };
    in
    # Deep merge per-system outputs (packages, devShells, apps) with
    # top-level outputs (nixosConfigurations, sdImage).
    # recursiveUpdate needed because both sides produce packages.aarch64-linux.*
    nixpkgs.lib.recursiveUpdate (flake-utils.lib.eachDefaultSystem (system:
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

        # Native Rust build (Linux/NixOS) — full (mesh-player + mesh-cue)
        meshBuild = import ./nix/packages/mesh-build.nix {
          inherit pkgs common;
          src = ./.;
        };

        # mesh-player only (for embedded deployment, no mesh-cue/ONNX deps)
        meshPlayer = import ./nix/packages/mesh-player.nix {
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

        # Beat This! model conversion (PyTorch → ONNX)
        convertBeatModelApp = import ./nix/apps/convert-beat-model.nix {
          inherit pkgs;
        };

        # Build nn~ Pure Data external for neural audio effects
        buildNnTildeApp = import ./nix/apps/build-nn-tilde.nix {
          inherit pkgs;
          nn-tilde = nnTilde;
        };

        # Embedded: one-time CI setup (keypair, secrets, Pages)
        embeddedSetupApp = import ./nix/apps/embedded-setup.nix {
          inherit pkgs;
        };

        # Embedded: download and flash NixOS SD image for Orange Pi 5 Pro
        embeddedFlashApp = import ./nix/apps/embedded-flash.nix {
          inherit pkgs;
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
          mesh-player = meshPlayer;
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
          convert-beat-model = {
            type = "app";
            program = "${convertBeatModelApp}/bin/convert-beat-model";
          };
          build-nn-tilde = {
            type = "app";
            program = "${buildNnTildeApp}/bin/build-nn-tilde";
          };
          embedded-setup = {
            type = "app";
            program = "${embeddedSetupApp}/bin/embedded-setup";
          };
          embedded-flash = {
            type = "app";
            program = "${embeddedFlashApp}/bin/embedded-flash";
          };
        };
      }
    )) embeddedOutputs;
}
