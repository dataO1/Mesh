#!/usr/bin/env bash
# Activate one of the variants in models/aggression-axes/ as the runtime
# intensity axis Mesh consumes.
#
# Usage:
#   scripts/select-active-axis.sh                 # show available variants
#   scripts/select-active-axis.sh V5_aggression_led
#
# Effect: copies the chosen variant JSON to models/muq-mulan-aggression-axis.json
# (the canonical filename MlModelManager looks for). Triggering "Build Similarity
# Index" in mesh-cue afterward refreshes pca_aggression_axis with the new vector.
#
# We use copy not symlink so the artifact is git-tracked as a real file —
# important for releases where the GitHub `models` release uploads the JSON
# verbatim.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VARIANT_DIR="$REPO_ROOT/models/aggression-axes"
ACTIVE="$REPO_ROOT/models/muq-mulan-aggression-axis.json"

if [[ $# -eq 0 ]]; then
    echo "Available variants in $VARIANT_DIR:"
    if ls "$VARIANT_DIR"/*.json >/dev/null 2>&1; then
        for v in "$VARIANT_DIR"/*.json; do
            id="$(basename "$v" .json)"
            name="$(grep -m1 '"name"' "$v" | sed 's/.*"name": "\([^"]*\)".*/\1/')"
            printf "  %-30s %s\n" "$id" "$name"
        done
    else
        echo "  (none — run \`nix run .#derive-aggression-axes\` first)"
    fi
    echo
    if [[ -f "$ACTIVE" ]]; then
        active_id="$(grep -m1 '"variant_id"' "$ACTIVE" | sed 's/.*"variant_id": "\([^"]*\)".*/\1/')"
        echo "Currently active: $active_id  ($ACTIVE)"
    else
        echo "Currently active: <none>"
    fi
    echo
    echo "To activate: $0 <variant_id>"
    exit 0
fi

VARIANT_ID="$1"
SOURCE="$VARIANT_DIR/$VARIANT_ID.json"

if [[ ! -f "$SOURCE" ]]; then
    echo "[!] Variant '$VARIANT_ID' not found at $SOURCE" >&2
    echo "    Available: $(ls "$VARIANT_DIR"/*.json 2>/dev/null | xargs -n1 basename | sed 's/\.json$//' | tr '\n' ' ')" >&2
    exit 1
fi

cp "$SOURCE" "$ACTIVE"
echo "[active] $VARIANT_ID → $ACTIVE"

# Best-effort: also drop into the user-cache so the next mesh-cue launch
# picks it up without a fresh "Build Similarity Index" cycle.
USER_CACHE="$HOME/.cache/mesh-cue/ml-models/muq-mulan-aggression-axis.json"
if [[ -d "$(dirname "$USER_CACHE")" ]]; then
    cp "$SOURCE" "$USER_CACHE"
    echo "[active] also copied to user cache: $USER_CACHE"
fi

echo
echo "Next: trigger 'Build Similarity Index' in mesh-cue to refresh"
echo "      pca_aggression_axis with the new vector."
