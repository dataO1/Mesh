# nn~ Pure Data external for neural audio processing
#
# This derivation builds the nn~ external from the acids-ircam/nn_tilde repository.
# The resulting .pd_linux file can be distributed as a GitHub release asset.
#
# Usage:
#   nix build .#nn-tilde
#   # Output: result/lib/pd/extra/nn~.pd_linux
#
# For GitHub releases:
#   nix build .#nn-tilde
#   cp result/lib/pd/extra/nn~.pd_linux ./nn~.pd_linux
#   gh release upload v1.0.0 nn~.pd_linux

{ lib
, stdenv
, fetchFromGitHub
, cmake
, puredata
, libtorch-bin
, curl
}:

stdenv.mkDerivation rec {
  pname = "nn-tilde";
  version = "unstable-2024-01-01";

  src = fetchFromGitHub {
    owner = "acids-ircam";
    repo = "nn_tilde";
    # Pin to a known working commit
    rev = "main";
    sha256 = lib.fakeSha256; # Will be updated on first build
    fetchSubmodules = true;
  };

  nativeBuildInputs = [
    cmake
  ];

  buildInputs = [
    puredata
    libtorch-bin
    curl
  ];

  # nn_tilde expects libs in env/lib - we provide them via buildInputs instead
  preConfigure = ''
    mkdir -p env/lib env/include
    ln -sf ${curl.out}/lib/libcurl.so env/lib/
    ln -sf ${curl.dev}/include/curl env/include/
  '';

  cmakeDir = "../src";

  cmakeFlags = [
    "-DCMAKE_BUILD_TYPE=Release"
  ];

  installPhase = ''
    runHook preInstall

    mkdir -p $out/lib/pd/extra
    find . -name "nn~.pd_linux" -exec cp {} $out/lib/pd/extra/ \;

    # Also install the help file if available
    if [ -f ../src/help/nn~-help.pd ]; then
      cp ../src/help/nn~-help.pd $out/lib/pd/extra/
    fi

    runHook postInstall
  '';

  meta = with lib; {
    description = "Neural network external for Pure Data";
    homepage = "https://github.com/acids-ircam/nn_tilde";
    license = licenses.gpl3;
    platforms = platforms.linux;
    maintainers = [ ];
  };
}
