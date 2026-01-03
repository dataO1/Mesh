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

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        };

        # Build nn~ external for Pure Data
        nn-external = pkgs.stdenv.mkDerivation {
          pname = "nn-tilde";
          version = "unstable";
          src = nn-tilde;

          nativeBuildInputs = with pkgs; [
            cmake
            pkg-config
          ];

          buildInputs = with pkgs; [
            puredata
            libtorch-bin
          ];

          cmakeFlags = [
            "-DCMAKE_BUILD_TYPE=Release"
            "-DPD_INCLUDE_DIR=${pkgs.puredata}/include/pd"
          ];

          installPhase = ''
            mkdir -p $out/lib/pd-externals
            find . -name "*.pd_linux" -exec cp {} $out/lib/pd-externals/ \;
          '';
        };

        # Common native dependencies
        nativeBuildInputs = with pkgs; [
          rustToolchain
          pkg-config
          cmake
        ];

        # Runtime and build dependencies
        buildInputs = with pkgs; [
          # Audio
          jack2
          alsa-lib
          pipewire

          # Pure Data
          puredata
          libpd

          # ML/Neural (for nn~ and RAVE)
          libtorch-bin

          # GUI (iced dependencies)
          wayland
          libxkbcommon
          xorg.libX11
          xorg.libXcursor
          xorg.libXrandr
          xorg.libXi
          vulkan-loader
          libGL

          # Misc
          openssl
        ];

        # Library paths for runtime
        libraryPath = pkgs.lib.makeLibraryPath buildInputs;

      in
      {
        packages = {
          nn-external = nn-external;

          mesh-player = pkgs.rustPlatform.buildRustPackage {
            pname = "mesh-player";
            version = "0.1.0";
            src = ./.;

            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            inherit nativeBuildInputs buildInputs;

            buildPhase = ''
              cargo build --release -p mesh-player
            '';

            installPhase = ''
              mkdir -p $out/bin
              cp target/release/mesh-player $out/bin/

              # Install PD effects and externals
              mkdir -p $out/share/mesh/effects/pd/externals
              if [ -d effects/pd ]; then
                cp -r effects/pd/* $out/share/mesh/effects/pd/
              fi
              cp ${nn-external}/lib/pd-externals/* $out/share/mesh/effects/pd/externals/

              # Install RAVE models if present
              if [ -d rave/rave-models ]; then
                mkdir -p $out/share/mesh/rave-models
                cp -r rave/rave-models/* $out/share/mesh/rave-models/
              fi
            '';

            postFixup = ''
              patchelf --set-rpath "${libraryPath}" $out/bin/mesh-player
            '';

            meta = with pkgs.lib; {
              description = "Mesh DJ Player - 4-deck stem-based mixing with neural effects";
              license = licenses.mit;
              platforms = platforms.linux;
            };
          };

          mesh-cue = pkgs.rustPlatform.buildRustPackage {
            pname = "mesh-cue";
            version = "0.1.0";
            src = ./.;

            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            inherit nativeBuildInputs buildInputs;

            buildPhase = ''
              cargo build --release -p mesh-cue
            '';

            installPhase = ''
              mkdir -p $out/bin
              cp target/release/mesh-cue $out/bin/
            '';

            postFixup = ''
              patchelf --set-rpath "${libraryPath}" $out/bin/mesh-cue
            '';

            meta = with pkgs.lib; {
              description = "Mesh Cue Software - Track preparation and playlist management";
              license = licenses.mit;
              platforms = platforms.linux;
            };
          };

          default = self.packages.${system}.mesh-player;
        };

        devShells.default = pkgs.mkShell {
          inherit buildInputs;

          nativeBuildInputs = nativeBuildInputs ++ (with pkgs; [
            # Development tools
            rust-analyzer
            cargo-watch
            cargo-edit
            cargo-expand

            # Debugging
            gdb
            lldb
          ]);

          shellHook = ''
            # Rust
            export RUST_BACKTRACE=1

            # Library paths
            export LD_LIBRARY_PATH="${libraryPath}:$LD_LIBRARY_PATH"

            # PD externals path (nn~ and others)
            export PD_EXTERNALS="${nn-external}/lib/pd-externals:./effects/pd/externals"

            # JACK settings
            export JACK_NO_AUDIO_RESERVATION=1

            # Torch library path (for nn~)
            export LIBTORCH="${pkgs.libtorch-bin}"
            export LIBTORCH_LIB="${pkgs.libtorch-bin}/lib"
            export LIBTORCH_INCLUDE="${pkgs.libtorch-bin}/include"

            # Vulkan for iced
            export VK_ICD_FILENAMES="${pkgs.vulkan-loader}/share/vulkan/icd.d/intel_icd.x86_64.json:${pkgs.vulkan-loader}/share/vulkan/icd.d/radeon_icd.x86_64.json"

            echo ""
            echo "╔══════════════════════════════════════════════════════════════╗"
            echo "║                  Mesh Development Shell                       ║"
            echo "╠══════════════════════════════════════════════════════════════╣"
            echo "║  Build:     cargo build -p mesh-player                       ║"
            echo "║  Run:       cargo run -p mesh-player                         ║"
            echo "║  Test:      cargo test                                       ║"
            echo "║  Watch:     cargo watch -x 'build -p mesh-player'            ║"
            echo "╠══════════════════════════════════════════════════════════════╣"
            echo "║  PD externals: $PD_EXTERNALS"
            echo "║  Torch lib:    $LIBTORCH_LIB"
            echo "╚══════════════════════════════════════════════════════════════╝"
            echo ""
          '';
        };

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
      }
    );
}
