# One-time CI setup for the embedded NixOS pipeline
#
# Usage:
#   nix run .#embedded-setup
#
# Idempotent — safe to run multiple times. Each step checks whether
# it has already been completed and skips if so.
#
# What it does:
#   1. Generates Ed25519 signing keypair (skips if key exists)
#   2. Uploads private key to GitHub Secrets (skips if secret exists)
#   3. Patches configuration.nix with public key (skips if already patched)
#   4. Enables GitHub Pages on gh-pages branch (skips if already enabled)
#   5. Prints remaining manual steps (commit + push tag)
{ pkgs }:

pkgs.writeShellApplication {
  name = "embedded-setup";
  runtimeInputs = with pkgs; [ gh coreutils gnused gnugrep nix ];
  text = ''
    set -euo pipefail

    KEY_DIR="''${HOME}/.config/mesh"
    PRIV_KEY="''${KEY_DIR}/cache-priv-key.pem"
    PUB_KEY="''${KEY_DIR}/cache-pub-key.pem"
    CONFIG_FILE="nix/embedded/configuration.nix"
    REPO="dataO1/Mesh"

    ok()   { echo "  ✓ $1"; }
    skip() { echo "  — $1 (already done)"; }
    fail() { echo "  ✗ $1"; exit 1; }

    echo "╔═══════════════════════════════════════════════════════════╗"
    echo "║  Mesh Embedded — First-Time CI Setup                    ║"
    echo "╚═══════════════════════════════════════════════════════════╝"
    echo ""

    # Check gh auth
    if ! gh auth status &>/dev/null; then
      fail "Not authenticated with GitHub CLI. Run: gh auth login"
    fi

    # Check we're in the repo root
    if [ ! -f "$CONFIG_FILE" ]; then
      fail "Run this from the mesh repo root (could not find $CONFIG_FILE)"
    fi

    # ── Step 1: Generate signing keypair ─────────────────────────────
    echo "[1/4] Signing keypair"
    if [ -f "$PRIV_KEY" ] && [ -f "$PUB_KEY" ]; then
      skip "Keypair exists at $KEY_DIR/"
    else
      mkdir -p "$KEY_DIR"
      nix-store --generate-binary-cache-key mesh-embedded "$PRIV_KEY" "$PUB_KEY"
      chmod 600 "$PRIV_KEY"
      ok "Generated keypair in $KEY_DIR/"
    fi

    # ── Step 2: Upload private key to GitHub Secrets ─────────────────
    echo "[2/4] GitHub Secret (NIX_CACHE_PRIV_KEY)"
    if gh secret list --repo "$REPO" 2>/dev/null | grep -q "NIX_CACHE_PRIV_KEY"; then
      skip "Secret NIX_CACHE_PRIV_KEY already exists on $REPO"
    else
      gh secret set NIX_CACHE_PRIV_KEY --repo "$REPO" < "$PRIV_KEY"
      ok "Uploaded private key to GitHub Secrets"
    fi

    # ── Step 3: Patch configuration.nix with public key ──────────────
    echo "[3/4] Public key in $CONFIG_FILE"
    if grep -q "REPLACE_WITH_PUBLIC_KEY" "$CONFIG_FILE"; then
      PUB_KEY_CONTENT=$(cat "$PUB_KEY")
      sed -i "s|mesh-embedded:REPLACE_WITH_PUBLIC_KEY|$PUB_KEY_CONTENT|" "$CONFIG_FILE"
      ok "Patched $CONFIG_FILE with public key"
    else
      skip "Public key already configured in $CONFIG_FILE"
    fi

    # ── Step 4: Enable GitHub Pages ──────────────────────────────────
    echo "[4/4] GitHub Pages"
    PAGES_STATUS=$(gh api "repos/$REPO/pages" --jq '.source.branch' 2>/dev/null || echo "none")
    if [ "$PAGES_STATUS" = "gh-pages" ]; then
      skip "GitHub Pages already enabled on gh-pages branch"
    elif [ "$PAGES_STATUS" = "none" ]; then
      gh api --method POST "repos/$REPO/pages" \
        -f source='{"branch":"gh-pages","path":"/"}' \
        --silent 2>/dev/null \
        && ok "Enabled GitHub Pages on gh-pages branch" \
        || echo "  — Could not enable Pages via API (enable manually: Settings > Pages > gh-pages)"
    else
      skip "GitHub Pages enabled on branch: $PAGES_STATUS"
    fi

    # ── Summary ──────────────────────────────────────────────────────
    echo ""
    echo "Setup complete. Remaining steps:"
    echo ""
    if grep -q "REPLACE_WITH_PUBLIC_KEY" "$CONFIG_FILE" 2>/dev/null; then
      echo "  (!) Public key was not patched — check $CONFIG_FILE manually"
    else
      HAS_CHANGES=$(git diff --name-only "$CONFIG_FILE" 2>/dev/null || true)
      if [ -n "$HAS_CHANGES" ]; then
        echo "  1. Commit the public key change:"
        echo "     git add $CONFIG_FILE && git commit -m 'chore(embedded): add cache signing public key'"
        echo ""
      fi
    fi
    echo "  Push a version tag to trigger the first CI build:"
    echo "     git tag v0.9.0 && git push origin v0.9.0"
    echo ""
    echo "  CI will build mesh-player + SD image and publish to:"
    echo "     Binary cache: https://datao1.github.io/Mesh/"
    echo "     SD image:     GitHub Releases (sdimage-<hash>)"
  '';
}
