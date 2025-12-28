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
            
            # External libraries commonly used with Pd
            fftw
            libsndfile
            libsamplerate
            
            # GUI dependencies (for Pd GUI)
            tk
            tcl
          ];

          shellHook = ''
            echo "Pure Data development environment"
            echo "Run 'pd' to start Pure Data"
            echo "Pure Data version: $(pd -version 2>&1 | head -n1 || echo 'Unknown')"
          '';
        };
      });
}