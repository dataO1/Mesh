# Download and flash the NixOS SD image for Orange Pi 5 Pro
#
# Usage:
#   nix run .#embedded-flash              # interactive: pick release + device
#   nix run .#embedded-flash /dev/sdX     # flash to specific device
#
# The SD image is built by CI and uploaded to GitHub Releases.
# This script downloads it and flashes it to a microSD card.
{ pkgs }:

pkgs.writeShellApplication {
  name = "embedded-flash";
  runtimeInputs = with pkgs; [ gh coreutils zstd gnugrep gawk ];
  text = ''
    set -euo pipefail

    DEVICE="''${1:-}"

    echo "╔═══════════════════════════════════════════════════════════╗"
    echo "║  Mesh Embedded — Flash SD Image (Orange Pi 5 Pro)       ║"
    echo "╚═══════════════════════════════════════════════════════════╝"
    echo ""

    # Check gh auth
    if ! gh auth status &>/dev/null; then
      echo "Error: not authenticated with GitHub CLI."
      echo "Run: gh auth login"
      exit 1
    fi

    # Find available SD image releases
    echo "Fetching SD image releases..."
    RELEASES=$(gh release list --repo dataO1/Mesh --limit 20 | grep -E "^SD Image" || true)

    if [ -z "$RELEASES" ]; then
      echo "No SD image releases found."
      echo "Push a version tag to trigger CI: git tag v0.9.0 && git push origin v0.9.0"
      exit 1
    fi

    echo ""
    echo "Available SD images:"
    echo "---"
    echo "$RELEASES" | awk -F'\t' '{ printf "  %s  (%s)\n", $1, $3 }'
    echo ""

    # Get the latest release tag
    LATEST_TAG=$(gh release list --repo dataO1/Mesh --limit 20 --json tagName,name \
      | gh api --input - /dev/stdin --jq 'null' 2>/dev/null || true)

    # Simpler: just get the first sdimage tag
    LATEST_TAG=$(gh release list --repo dataO1/Mesh --limit 20 --json tagName,name -q '.[] | select(.tagName | startswith("sdimage-")) | .tagName' | head -1)

    if [ -z "$LATEST_TAG" ]; then
      echo "Error: could not determine latest SD image release tag."
      exit 1
    fi

    echo "Latest: $LATEST_TAG"
    echo ""

    # Download to temp directory
    TMPDIR=$(mktemp -d)
    trap 'rm -rf "$TMPDIR"' EXIT

    echo "Downloading SD image..."
    gh release download "$LATEST_TAG" --repo dataO1/Mesh --dir "$TMPDIR"

    IMAGE=$(find "$TMPDIR" -name '*.img.zst' -o -name '*.img' | head -1)
    if [ -z "$IMAGE" ]; then
      echo "Error: no .img or .img.zst file found in release."
      ls -la "$TMPDIR"
      exit 1
    fi

    FILENAME=$(basename "$IMAGE")
    echo "Downloaded: $FILENAME ($(du -h "$IMAGE" | cut -f1))"
    echo ""

    # Determine target device
    if [ -z "$DEVICE" ]; then
      echo "Available block devices:"
      lsblk -d -o NAME,SIZE,MODEL,TRAN | grep -E "sd|mmcblk" || echo "  (none found — is the microSD inserted?)"
      echo ""
      read -rp "Target device (e.g., /dev/sdb): " DEVICE
    fi

    if [ ! -b "$DEVICE" ]; then
      echo "Error: $DEVICE is not a block device."
      exit 1
    fi

    # Safety check
    echo ""
    echo "┌─────────────────────────────────────────────────┐"
    echo "│  WARNING: ALL DATA ON $DEVICE WILL BE ERASED"
    echo "│  Image: $FILENAME"
    echo "└─────────────────────────────────────────────────┘"
    echo ""
    read -rp "Type 'yes' to continue: " CONFIRM
    if [ "$CONFIRM" != "yes" ]; then
      echo "Aborted."
      exit 1
    fi

    echo ""
    echo "Flashing..."

    if [[ "$FILENAME" == *.zst ]]; then
      zstdcat "$IMAGE" | sudo dd of="$DEVICE" bs=4M status=progress conv=fsync
    else
      sudo dd if="$IMAGE" of="$DEVICE" bs=4M status=progress conv=fsync
    fi

    echo ""
    echo "Done! Insert the microSD into the Orange Pi 5 Pro and power on."
    echo "Default SSH: ssh mesh@<board-ip> (password: mesh)"
  '';
}
