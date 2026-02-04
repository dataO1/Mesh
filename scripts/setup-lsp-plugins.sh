#!/usr/bin/env bash
#
# Setup LSP CLAP plugins for Mesh
#
# Downloads LSP plugins and optionally bundles dependencies for
# portable operation (required on NixOS and non-FHS systems).
#
# Usage:
#   ./setup-lsp-plugins.sh [--bundle-deps] [target-dir]
#
# Options:
#   --bundle-deps  Bundle shared library dependencies for portability
#   target-dir     Installation directory (default: ~/Music/mesh-collection/effects/clap)
#

set -euo pipefail

LSP_VERSION="1.2.26"
LSP_URL="https://github.com/lsp-plugins/lsp-plugins/releases/download/${LSP_VERSION}/lsp-plugins-clap-${LSP_VERSION}-Linux-x86_64.tar.gz"

BUNDLE_DEPS=false
TARGET_DIR="${HOME}/Music/mesh-collection/effects/clap"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        --bundle-deps)
            BUNDLE_DEPS=true
            shift
            ;;
        *)
            TARGET_DIR="$1"
            shift
            ;;
    esac
done

echo "=== LSP Plugins Setup ==="
echo "Version: $LSP_VERSION"
echo "Target:  $TARGET_DIR"
echo "Bundle:  $BUNDLE_DEPS"
echo

# Create target directory
mkdir -p "$TARGET_DIR"

# Download if not already present
if [[ ! -f "$TARGET_DIR/lsp-plugins.clap" ]]; then
    echo "Downloading LSP plugins..."
    TMP_FILE=$(mktemp)
    curl -L "$LSP_URL" -o "$TMP_FILE"

    echo "Extracting..."
    tar xzf "$TMP_FILE" -C "$TARGET_DIR" --strip-components=1
    rm "$TMP_FILE"

    echo "LSP plugins installed: $(ls "$TARGET_DIR"/*.clap 2>/dev/null | wc -l) plugin bundle(s)"
else
    echo "LSP plugins already installed"
fi

# Bundle dependencies if requested
if [[ "$BUNDLE_DEPS" == true ]]; then
    echo
    echo "=== Bundling Dependencies ==="

    LIB_DIR="$TARGET_DIR/lib"
    mkdir -p "$LIB_DIR"

    # Check for patchelf
    if ! command -v patchelf &>/dev/null; then
        echo "ERROR: patchelf is required for bundling dependencies"
        echo "Install with: nix-env -iA nixpkgs.patchelf (NixOS)"
        echo "           or: apt install patchelf (Debian/Ubuntu)"
        exit 1
    fi

    # Find missing dependencies
    echo "Checking dependencies..."
    MISSING=$(ldd "$TARGET_DIR"/lsp-plugins.clap 2>&1 | grep "not found" | awk '{print $1}' | sort -u || true)

    if [[ -z "$MISSING" ]]; then
        echo "All dependencies satisfied!"
    else
        echo "Missing libraries:"
        echo "$MISSING"
        echo

        # Try to find and copy missing libraries
        for lib in $MISSING; do
            echo -n "  $lib: "

            # Search common locations
            FOUND=""
            for searchpath in /usr/lib /usr/lib64 /usr/lib/x86_64-linux-gnu /nix/store; do
                if [[ -d "$searchpath" ]]; then
                    FOUND=$(find "$searchpath" -name "$lib" -type f 2>/dev/null | head -1 || true)
                    [[ -n "$FOUND" ]] && break
                fi
            done

            if [[ -n "$FOUND" ]]; then
                # Verify it's 64-bit
                if file "$FOUND" | grep -q "64-bit"; then
                    cp "$FOUND" "$LIB_DIR/"
                    echo "copied from $FOUND"
                else
                    echo "SKIPPED (32-bit)"
                fi
            else
                echo "NOT FOUND - install manually"
            fi
        done

        # Patch RPATH on all copied libraries
        echo
        echo "Patching RPATH..."
        for lib in "$LIB_DIR"/*.so*; do
            [[ -f "$lib" ]] || continue
            chmod u+w "$lib" 2>/dev/null || true
            patchelf --set-rpath '$ORIGIN' "$lib" 2>/dev/null || true
        done

        # Patch the main plugin
        patchelf --set-rpath '$ORIGIN/lib' "$TARGET_DIR"/lsp-plugins.clap 2>/dev/null || true

        # Verify
        echo
        echo "Verifying..."
        STILL_MISSING=$(ldd "$TARGET_DIR"/lsp-plugins.clap 2>&1 | grep "not found" || true)
        if [[ -z "$STILL_MISSING" ]]; then
            echo "SUCCESS: All dependencies bundled!"
        else
            echo "WARNING: Some dependencies still missing:"
            echo "$STILL_MISSING"
            echo
            echo "You may need to manually copy these libraries to: $LIB_DIR"
        fi
    fi
fi

echo
echo "=== Setup Complete ==="
echo "LSP plugins are ready in: $TARGET_DIR"
echo
echo "To use in Mesh, ensure your mesh-collection path points to:"
echo "  $(dirname "$TARGET_DIR")"
