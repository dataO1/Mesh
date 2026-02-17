# BPM accuracy report — export DB, scrape Beatport, compare backends
#
# Usage:
#   nix run .#bpm-report                    # Export + scrape all + report
#   nix run .#bpm-report -- --limit 20      # Only scrape 20 new tracks
#   nix run .#bpm-report -- --no-scrape     # Report from existing ground truth only
#   nix run .#bpm-report -- --workers 4     # Limit parallel Beatport requests
#
# Outputs (in project root):
#   bpm-comparison.json   — Full comparison data with per-track errors
#   ground-truth.json     — Cached Beatport BPMs (edit to fix wrong values)
#
# The ground truth file is cumulative — re-running only scrapes tracks
# not already present. Edit it to correct wrong Beatport values.
{ pkgs }:

pkgs.writeShellApplication {
  name = "bpm-report";
  runtimeInputs = with pkgs; [ python311 coreutils ];
  text = ''
    set -euo pipefail

    # Find the mesh project root (where Cargo.toml lives)
    # When run via `nix run`, CWD is the user's terminal CWD
    PROJECT_ROOT="''${MESH_PROJECT_ROOT:-$(pwd)}"
    COMPARISON_SCRIPT="$PROJECT_ROOT/scripts/bpm_comparison.py"

    if [ ! -f "$COMPARISON_SCRIPT" ]; then
      echo "Error: Cannot find scripts/bpm_comparison.py"
      echo "Run this from the mesh project root, or set MESH_PROJECT_ROOT"
      exit 1
    fi

    EXPORT_FILE="$(mktemp --suffix=.json)"
    trap 'rm -f "$EXPORT_FILE"' EXIT

    # Parse our args vs pass-through args
    SCRAPE="--scrape"
    PASS_ARGS=()
    for arg in "$@"; do
      case "$arg" in
        --no-scrape) SCRAPE="" ;;
        *) PASS_ARGS+=("$arg") ;;
      esac
    done

    echo "╔═══════════════════════════════════════════════════════════╗"
    echo "║  BPM Accuracy Report                                    ║"
    echo "╚═══════════════════════════════════════════════════════════╝"
    echo ""

    # Step 1: Export analysis data from the mesh database
    echo "[1/3] Exporting analysis data from mesh database..."
    cargo run -p mesh-core --bin export-analysis --release -- "$EXPORT_FILE" 2>&1
    echo ""

    # Step 2+3: Scrape Beatport (parallel) + generate report
    echo "[2/3] Scraping Beatport for ground truth BPMs..."
    echo "[3/3] Generating comparison report..."
    echo ""

    python3 "$COMPARISON_SCRIPT" \
      "$EXPORT_FILE" \
      -g "$PROJECT_ROOT/ground-truth.json" \
      -o "$PROJECT_ROOT/bpm-comparison.json" \
      $SCRAPE \
      "''${PASS_ARGS[@]+"''${PASS_ARGS[@]}"}"
  '';
}
