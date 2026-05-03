#!/usr/bin/env bash
# Run axis_eval over every variant JSON in models/aggression-axes/ and
# emit one CSV per variant plus a stacked combined.csv.
#
# Output: /tmp/axis-eval/<variant_id>.csv  (per variant)
#         /tmp/axis-eval/combined.csv      (every variant stacked, with a
#                                           leading variant_id column so
#                                           agents can split or join easily)
#         /tmp/axis-eval/summary.txt       (intensity distribution per variant)
#
# Usage: scripts/eval-axis-variants.sh [collection-path]
#   collection-path defaults to ~/Music/mesh-collection
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COLLECTION="${1:-$HOME/Music/mesh-collection}"
VARIANT_DIR="$REPO_ROOT/models/aggression-axes"
OUT_DIR="/tmp/axis-eval"

mkdir -p "$OUT_DIR"
: > "$OUT_DIR/summary.txt"
: > "$OUT_DIR/combined.csv"

if ! ls "$VARIANT_DIR"/*.json >/dev/null 2>&1; then
    echo "[eval] no variants in $VARIANT_DIR — run \`nix run .#derive-aggression-axes\` first" >&2
    exit 1
fi

echo "[eval] building axis_eval..."
cargo build -p mesh-cue --bin axis_eval --release 2>&1 | tail -3

BIN="$REPO_ROOT/target/release/axis_eval"
if [[ ! -x "$BIN" ]]; then
    echo "[eval] expected binary at $BIN but it's missing/non-exec" >&2
    exit 1
fi

FIRST=true
for VARIANT_JSON in "$VARIANT_DIR"/*.json; do
    VARIANT_ID="$(basename "$VARIANT_JSON" .json)"
    CSV_OUT="$OUT_DIR/$VARIANT_ID.csv"

    echo
    echo "════════════════════════════════════════════════════════════════════════"
    echo "  $VARIANT_ID"
    echo "════════════════════════════════════════════════════════════════════════"
    "$BIN" \
        --variant "$VARIANT_JSON" \
        --csv "$CSV_OUT" \
        --collection "$COLLECTION" \
        --limit 15 \
        2> >(tee -a "$OUT_DIR/summary.txt" >&2) || {
            echo "[eval] $VARIANT_ID FAILED" >&2
            continue
        }

    # Stack into combined.csv. Add a leading variant_id column.
    if $FIRST; then
        # Header from this CSV with variant_id prepended.
        head -1 "$CSV_OUT" | sed 's/^/variant_id,/' >> "$OUT_DIR/combined.csv"
        FIRST=false
    fi
    tail -n +2 "$CSV_OUT" | sed "s/^/$VARIANT_ID,/" >> "$OUT_DIR/combined.csv"
done

echo
echo "[eval] done"
echo "[eval] per-variant CSVs:   $OUT_DIR/*.csv"
echo "[eval] combined CSV:       $OUT_DIR/combined.csv"
echo "[eval] distribution log:   $OUT_DIR/summary.txt"
