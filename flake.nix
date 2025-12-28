{
  description = "Pure Data development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            # Core Pure Data
            puredata

            # Audio libraries and tools
            alsa-lib
            alsa-utils
            jack2
            pulseaudio
            portaudio

            # Development tools
            gcc
            gnumake
            pkg-config
            cmake

            # External libraries commonly used with Pd
            fftw
            libsndfile
            libsamplerate

            # GUI dependencies (for Pd GUI)
            tk
            tcl

            # Neural network dependencies for RAVE and nn~
            python3
            python3Packages.torch
            python3Packages.torchaudio
            python3Packages.ffmpeg-python
            python3Packages.librosa
            python3Packages.requests
            libtorch-bin

            # Download utilities
            wget
            curl
            git

            # Archive extraction
            gnutar
            gzip

            # Libraries for compatibility with prebuilt binaries
            stdenv.cc.cc.lib
            curl.out

            pipewire.jack
          ];

          shellHook = ''
            echo "Pure Data + RAVE + nn~ development environment"
            echo "Pure Data version: $(pd -version 2>&1 | head -n1 || echo 'Unknown')"
            echo ""
            echo "Audio Setup:"
            echo "  • For JACK support: pw-jack pd -jack"
            echo "  • For ALSA (default): pd"

            # Create directories for neural models and externals
            mkdir -p $PWD/pd-externals
            mkdir -p $PWD/rave-models

            # Build nn~ from source instead of using problematic prebuilt binary
            if [ ! -f "$PWD/pd-externals/nn~.pd_linux" ]; then
              echo "Building nn~ from source..."
              if [ ! -d "/tmp/nn_tilde_src" ]; then
                git clone --depth 1 https://github.com/acids-ircam/nn_tilde.git /tmp/nn_tilde_src
              fi
              cd /tmp/nn_tilde_src
              mkdir -p build && cd build
              cmake .. -DCMAKE_BUILD_TYPE=Release
              make -j$(nproc)
              cp nn~.pd_linux $OLDPWD/pd-externals/
              cp ../help/nn~-help.pd $OLDPWD/pd-externals/
              echo "nn~ built and installed to $OLDPWD/pd-externals/"
              cd $OLDPWD
            fi

            # Setup RAVE (trained models are the main requirement for nn~)
            export PATH="$HOME/.local/bin:$PATH"
            if ~/.local/share/pipx/venvs/acids-rave/bin/python -c "import rave" &>/dev/null; then
              echo "✓ RAVE available for model training/conversion"
            else
              echo "Note: RAVE not found. Install with: 'nix-shell -p python3Packages.pipx --run \"pipx install acids-rave\"'"
            fi

            # Set Pure Data externals path and library path
            export PD_EXTRA_PATH="$PWD/pd-externals:${pkgs.cyclone}/cyclone"
            export LD_LIBRARY_PATH="${pkgs.stdenv.cc.cc.lib}/lib:${pkgs.curl.out}/lib:$LD_LIBRARY_PATH"


            # Create symlinks in pd-externals for easy model access
            if [ -d "$PWD/rave-models" ]; then
              for model in $PWD/rave-models/*; do
                if [ -f "$model" ]; then
                  basename_model=$(basename "$model")
                  # Remove .ts extension if present for simpler names
                  simple_name=$(echo "$basename_model" | sed 's/\.ts$//')
                  # Create symlink in pd-externals for direct access
                  ln -sf "$model" "$PWD/pd-externals/$simple_name.ts" 2>/dev/null
                  ln -sf "$model" "$PWD/pd-externals/$basename_model" 2>/dev/null
                fi
              done
            fi

            echo ""
            echo "Setup complete:"
            echo "  • Pure Data externals: $PWD/pd-externals"
            echo "  • Cyclone externals: ${pkgs.cyclone}/cyclone"
            echo "  • RAVE models directory: $PWD/rave-models"
            echo "  • nn~ and cyclone objects available in Pure Data"
            echo ""
            echo "Available RAVE models:"

            # Check which models are downloaded and show their status
            models=("darbouka_onnx" "vintage_onnx" "percussion_onnx" "singing_onnx" "flute_onnx" "guitar_onnx")
            descriptions=("Percussion/darbouka" "Vintage synth" "General percussion" "Vocal synthesis" "Flute tones" "Electric guitar")

            for i in "''${!models[@]}"; do
              model="''${models[$i]}"
              desc="''${descriptions[$i]}"
              if [ -f "$PWD/rave-models/$model.ts" ] || [ -f "$PWD/rave-models/$model" ]; then
                echo "  ✓ $model - $desc"
                echo "    Load: [load $model.ts(  or  [load $model("
              else
                echo "  ○ $model - $desc (not downloaded)"
                echo "    Get:  wget -P rave-models/ 'https://play.forum.ircam.fr/rave-vst-api/get_model/$model'"
              fi
            done

            # Show any other .ts files in rave-models
            if [ -d "$PWD/rave-models" ] && [ "$(ls -A $PWD/rave-models/*.ts 2>/dev/null)" ]; then
              echo ""
              echo "Other downloaded models:"
              for model in $PWD/rave-models/*.ts; do
                if [ -f "$model" ]; then
                  basename_model=$(basename "$model")
                  # Check if it's not in our known list
                  known=false
                  for known_model in "''${models[@]}"; do
                    if [[ "$basename_model" == "$known_model.ts" || "$basename_model" == "$known_model" ]]; then
                      known=true
                      break
                    fi
                  done
                  if [ "$known" = false ]; then
                    simple_name=$(echo "$basename_model" | sed 's/\.ts$//')
                    echo "  ✓ $basename_model"
                    echo "    Load: [load $simple_name.ts(  or  [load $basename_model("
                  fi
                fi
              done
            fi

            echo ""
            echo "Usage in Pure Data:"
            echo "  1. Start: pw-jack pd -jack  (for low-latency audio)"
            echo "  2. Neural: [nn~ darbouka_onnx.ts] or [nn~ darbouka_onnx.ts forward]"
            echo "  3. Cyclone: [scope~], [record~], [coll], [seq], [table] (Max/MSP objects)"
            echo "  4. Process: [adc~] -> [nn~] -> [dac~]"
            echo "  5. Methods: forward, encode, decode (model-dependent)"
            echo "  6. Control: [enable 1(, [dump(, [reload("
          '';
        };
      });
}
