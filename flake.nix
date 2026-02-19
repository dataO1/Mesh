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
      # Embedded NixOS configuration (Orange Pi 5)
      # =====================================================================
      # Built natively on aarch64 by GitHub Actions ARM runner.
      # Standard NixOS packages come from cache.nixos.org (aarch64-linux).
      # Custom packages (mesh-player, essentia) are built by CI and hosted
      # on GitHub Pages as a binary cache. The SD image is uploaded to
      # GitHub Releases with hash-based deduplication.

      # Centralized version — read once, used by both per-system and embedded outputs
      meshVersion = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;

      # Native aarch64 pkgs for the vendor kernel (used by nixos-rk3588 board module)
      embeddedKernelPkgs = import nixpkgs {
        system = "aarch64-linux";
      };

      embeddedOutputs = {
        # NixOS system configuration for the Orange Pi 5
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

            # Embed U-Boot in the SD image so the board boots without SPI NOR flash.
            # Prebuilt binaries extracted from official Orange Pi Debian v1.1.8.
            # https://opensource.rock-chips.com/wiki_Boot_option
            ({ pkgs, ... }:
              let uboot = pkgs.callPackage ./nix/embedded/u-boot-orangepi5 {};
              in {
                sdImage.postBuildCommands = ''
                  dd if=${uboot}/idbloader.img of=$img seek=64 conv=notrunc
                  dd if=${uboot}/u-boot.itb of=$img seek=16384 conv=notrunc
                '';
              })

            # Build mesh-player from the NixOS module system's own pkgs
            # (native aarch64 on CI, no separate cross-compilation pkgs needed)
            ({ pkgs, ... }:
              let common = import ./nix/common.nix { inherit pkgs; };
              in {
                services.mesh-embedded.package = import ./nix/packages/mesh-player.nix {
                  inherit pkgs common;
                  version = meshVersion;
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

        # meshVersion is defined in the outer let block (line 39) and visible here.
        # All Nix packages inherit it; no manual sync needed on version bumps.

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
          version = meshVersion;
          src = ./.;
        };

        # mesh-player only (for embedded deployment, no mesh-cue/ONNX deps)
        meshPlayer = import ./nix/packages/mesh-player.nix {
          inherit pkgs common;
          version = meshVersion;
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

        # Embedded: download and flash NixOS SD image for Orange Pi 5
        embeddedFlashApp = import ./nix/apps/embedded-flash.nix {
          inherit pkgs;
        };

        # BPM accuracy report (export DB + scrape Beatport + comparison)
        bpmReportApp = import ./nix/apps/bpm-report.nix {
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

        # =================================================================
        # Runnable apps — `nix run .#<name>`
        #
        # Release workflow (version is read from Cargo.toml automatically):
        #   1. Edit [workspace.package] version in Cargo.toml
        #   2. Update CHANGELOG.md
        #   3. Commit, tag, push:
        #        git add -A && git commit -m "release: vX.Y.Z"
        #        git tag vX.Y.Z && git push && git push --tags
        #   4. CI builds all artifacts and publishes the release
        #
        # Manual builds (CI does these automatically on tag push):
        #   nix run .#build-deb          → dist/deb/mesh-{player,cue}_amd64.deb
        #   nix run .#build-deb-cuda     → dist/deb/mesh-cue-cuda_amd64.deb
        #   nix run .#build-windows      → dist/windows/mesh-{player,cue}_win.zip
        #
        # Model conversion (CI syncs to 'models' release on tag push):
        #   nix run .#convert-model      → models/htdemucs.onnx (Demucs stem separation)
        #   nix run .#convert-model -- htdemucs_ft ./models  (fine-tuned variant)
        #   nix run .#convert-ml-model   → models/genre_discogs400-*.onnx (genre head)
        #   nix run .#convert-beat-model → models/beat_this_small.onnx (beat detection)
        #
        # Embedded (Orange Pi 5):
        #   nix run .#embedded-setup     — one-time CI keypair + secrets setup
        #   nix run .#embedded-flash     — download + flash SD image to card
        #
        # Utilities:
        #   nix run .#build-nn-tilde     — build nn~ PureData external
        #   nix run .#bpm-report         — BPM accuracy report vs Beatport
        # =================================================================
        apps = {
          # Container build: Ubuntu 22.04, glibc 2.35, CPU-only
          build-deb = {
            type = "app";
            program = "${buildDebApp}/bin/build-deb";
          };
          # Container build: Ubuntu 22.04, glibc 2.35, NVIDIA CUDA 12
          build-deb-cuda = {
            type = "app";
            program = "${buildDebCudaApp}/bin/build-deb-cuda";
          };
          # Container build: MinGW-w64 cross-compilation, DirectML GPU
          build-windows = {
            type = "app";
            program = "${buildWindowsApp}/bin/build-windows";
          };
          # Demucs PyTorch → ONNX (htdemucs, htdemucs_ft, htdemucs_6s)
          convert-model = {
            type = "app";
            program = "${convertModelApp}/bin/convert-model";
          };
          # Essentia TF classification head → ONNX (genre_discogs400)
          convert-ml-model = {
            type = "app";
            program = "${convertMlModelApp}/bin/convert-ml-model";
          };
          # Beat This! PyTorch → ONNX (small or final checkpoint)
          convert-beat-model = {
            type = "app";
            program = "${convertBeatModelApp}/bin/convert-beat-model";
          };
          # nn~ PureData external (neural audio effects)
          build-nn-tilde = {
            type = "app";
            program = "${buildNnTildeApp}/bin/build-nn-tilde";
          };
          # One-time: generate cache signing keypair + configure GitHub secrets
          embedded-setup = {
            type = "app";
            program = "${embeddedSetupApp}/bin/embedded-setup";
          };
          # Download SD image from GitHub Releases + flash to SD card
          embedded-flash = {
            type = "app";
            program = "${embeddedFlashApp}/bin/embedded-flash";
          };
          # Export DB, scrape Beatport, generate BPM comparison report
          bpm-report = {
            type = "app";
            program = "${bpmReportApp}/bin/bpm-report";
          };
        };
      }
    )) embeddedOutputs;
}
