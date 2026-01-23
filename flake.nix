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
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, nn-tilde }:
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

        # Native Rust build (Linux)
        meshBuild = import ./nix/packages/mesh-build.nix {
          inherit pkgs common;
          src = ./.;
        };

        # Debian packages (.deb)
        meshDeb = import ./nix/packages/mesh-deb.nix {
          inherit pkgs common meshBuild rustToolchain;
          src = ./.;
        };

        # =======================================================================
        # Apps
        # =======================================================================

        # Windows cross-compilation (container-based)
        buildWindowsApp = import ./nix/apps/build-windows.nix { inherit pkgs; };

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
          mesh-deb = meshDeb;
          default = meshDeb;
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
        };
      }
    );
}
