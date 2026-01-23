# Common definitions shared across all Nix expressions
# Includes: Essentia library, runtime inputs, build inputs, source filters
{ pkgs }:

let
  # Build Essentia library from source (nixpkgs only has binary extractor)
  # Required for mesh-cue's essentia-rs bindings
  # Using master branch for Python 3.12+ compatibility (WAF updates)
  essentia = pkgs.stdenv.mkDerivation rec {
    pname = "essentia";
    version = "2.1_beta6-dev";

    src = pkgs.fetchFromGitHub {
      owner = "MTG";
      repo = "essentia";
      rev = "17484ff0256169f14a959d62aa89a1463fead13f";
      hash = "sha256-q+TI03Y5Mw9W+ZNE8I1fEWvn3hjRyaxb7M6ZgntA8RA=";
    };

    nativeBuildInputs = with pkgs; [
      python3
      pkg-config
    ];

    buildInputs = with pkgs; [
      eigen
      fftwFloat
      taglib
      chromaprint
      libsamplerate
      libyaml
      ffmpeg_4-headless  # Headless: no SDL/GUI deps (~700MB smaller closure)
      zlib      # Required for linking
    ];

    configurePhase = ''
      runHook preConfigure
      python3 waf configure \
        --prefix=$out \
        --mode=release
      runHook postConfigure
    '';

    buildPhase = ''
      runHook preBuild
      python3 waf build -j $NIX_BUILD_CORES
      runHook postBuild
    '';

    installPhase = ''
      runHook preInstall
      python3 waf install
      runHook postInstall
    '';

    meta = with pkgs.lib; {
      description = "Audio analysis and audio-based music information retrieval library";
      homepage = "https://essentia.upf.edu/";
      license = licenses.agpl3Plus;
      platforms = platforms.linux;
    };
  };

  # Runtime dependencies for development and builds
  runtimeInputs = with pkgs; [
    # Core runtime (C++ stdlib needed by many deps)
    stdenv.cc.cc.lib  # libstdc++.so.6

    # Audio (libjack2 = client library only, no Python/FireWire bloat)
    libjack2
    alsa-lib
    pipewire

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

  # Build-time only dependencies
  buildOnlyInputs = with pkgs; [
    glibc.dev    # Headers for cc-rs crates
  ];

  # Combined build inputs
  buildInputs = runtimeInputs ++ buildOnlyInputs;

  # Library paths for runtime
  libraryPath = pkgs.lib.makeLibraryPath runtimeInputs;

  # Essentia dependencies (needed for both essentia build and mesh builds)
  essentiaDeps = with pkgs; [
    eigen
    fftwFloat
    taglib
    chromaprint
    libsamplerate
    libyaml
    ffmpeg_4-headless
    zlib
  ];

in {
  inherit essentia runtimeInputs buildOnlyInputs buildInputs libraryPath essentiaDeps;
}
