# Create portable .deb packages with patchelf
# Uses pre-built binaries from meshBuild
{ pkgs, common, meshBuild, rustToolchain, src }:

let
  # Filtered source for .deb packaging - Cargo manifests, source files, and packaging/
  # cargo-deb needs .rs files to validate manifest targets even though we use pre-built binaries
  debSrc = pkgs.lib.cleanSourceWith {
    inherit src;
    filter = path: type:
      let
        baseName = baseNameOf path;
        relPath = pkgs.lib.removePrefix (toString src + "/") path;
      in
      type == "directory" ||
      baseName == "Cargo.toml" ||
      baseName == "Cargo.lock" ||
      pkgs.lib.hasSuffix ".rs" baseName ||
      pkgs.lib.hasPrefix "packaging/" relPath;
  };

in pkgs.stdenv.mkDerivation {
  pname = "mesh-deb";
  version = "0.1.0";
  src = debSrc;

  nativeBuildInputs = with pkgs; [
    patchelf
    cargo-deb
    rustToolchain  # For cargo-deb
  ];

  # No build phase - we use pre-built binaries
  dontBuild = true;
  dontConfigure = true;

  installPhase = ''
    runHook preInstall

    # Create target directory structure that cargo-deb expects
    mkdir -p target/release/bundled

    # Copy pre-built binaries from meshBuild
    cp ${meshBuild}/bin/mesh-player target/release/
    cp ${meshBuild}/bin/mesh-cue target/release/

    echo "=== Patching binaries for portability ==="
    # Make binaries writable for patchelf
    chmod +w target/release/mesh-player target/release/mesh-cue

    # Remove Nix store paths from RUNPATH, set to standard Linux + bundled lib path
    patchelf --set-rpath '/usr/lib/x86_64-linux-gnu:/usr/lib' target/release/mesh-player
    patchelf --set-rpath '/usr/lib/mesh:/usr/lib/x86_64-linux-gnu:/usr/lib' target/release/mesh-cue

    # Set interpreter to standard Linux path
    patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 target/release/mesh-player
    patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 target/release/mesh-cue

    echo "=== Verifying patchelf results ==="
    patchelf --print-rpath target/release/mesh-player
    patchelf --print-rpath target/release/mesh-cue

    echo "=== Staging bundled libraries ==="
    # Bundle libessentia
    cp ${common.essentia}/lib/libessentia.so target/release/bundled/
    chmod +w target/release/bundled/libessentia.so

    # Bundle FFmpeg 4.x libraries (essentia depends on these, modern distros have FFmpeg 6.x)
    # Only copy the versioned .so files, not symlinks
    cp ${pkgs.ffmpeg_4-headless.lib}/lib/libavcodec.so.58.134.100 target/release/bundled/libavcodec.so.58
    cp ${pkgs.ffmpeg_4-headless.lib}/lib/libavformat.so.58.76.100 target/release/bundled/libavformat.so.58
    cp ${pkgs.ffmpeg_4-headless.lib}/lib/libavutil.so.56.70.100 target/release/bundled/libavutil.so.56
    cp ${pkgs.ffmpeg_4-headless.lib}/lib/libswresample.so.3.9.100 target/release/bundled/libswresample.so.3

    # Patch libessentia to find FFmpeg in /usr/lib/mesh/ instead of Nix store
    patchelf --set-rpath '/usr/lib/mesh:/usr/lib/x86_64-linux-gnu:/usr/lib' target/release/bundled/libessentia.so

    echo "Bundled libraries:"
    ls -lh target/release/bundled/

    echo "=== Creating .deb packages ==="
    cargo deb -p mesh-player --no-build --no-strip
    cargo deb -p mesh-cue --no-build --no-strip

    # Copy outputs
    mkdir -p $out
    cp target/debian/*.deb $out/

    echo "=== Build complete ==="
    ls -la $out/

    runHook postInstall
  '';
}
