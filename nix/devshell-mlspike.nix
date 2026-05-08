# ML / model-grading spike devshell — `nix develop .#mlspike`
#
# Runs the vLLM-based pipelines under spike/track-grading/ (Music Flamingo
# captioning, text-LLM intensity rating, MuQ-MuLan exports, BT-priors, etc.)
# on this NixOS machine without per-script LD_LIBRARY_PATH dances.
#
# What it does on entry:
#   1. Creates ~/.cache/mesh-spike/vllm-env if missing (Python 3.11 venv).
#   2. Pip-installs pinned vllm==0.20.1 + transformers==5.7.0 + torch stack.
#   3. patchelf's bundled triton ELFs (ptxas, ptxas-blackwell, nvdisasm,
#      cuobjdump) so they run on NixOS instead of dying with "ELF interpreter
#      not found".
#   4. Applies nix/mlspike-patches/musicflamingo-rote-timestamps.patch
#      (backports vLLM PR #39011's `_build_audio_timestamps` so vLLM
#      synthesizes `rote_timestamps` instead of demanding it from HF).
#   5. Records a bootstrap stamp so re-entry is instant (~50 ms).
#   6. Exports HF_HOME, HF_HUB_CACHE, LD_LIBRARY_PATH (libcuda + zlib +
#      bundled NVIDIA libs), TRITON_LIBCUDA_PATH; prepends $VENV/bin to PATH.
#
# After bootstrap, `python`, `pip`, `vllm` and the spike serve scripts work
# without any manual env prep:
#   bash spike/track-grading/serve_music_flamingo.sh
#   TEXT_LLM_MODEL=... bash spike/track-grading/serve_text_llm.sh
{ pkgs }:

let
  # Declarative pin: every Python package in the venv is pinned in
  # `nix/mlspike-requirements.txt` (full pip freeze, transitive deps
  # included). A fresh bootstrap installs from that file → reproducible.
  # To upgrade, edit a top-level package, re-freeze, commit the diff.
  requirementsFile = "nix/mlspike-requirements.txt";

  # Project-relative path to the MF patch — re-applied on every bootstrap.
  # Located in nix/mlspike-patches/ so it lives next to this file.
  mfPatch = "nix/mlspike-patches/musicflamingo-rote-timestamps.patch";

  venvDir = "$HOME/.cache/mesh-spike/vllm-env";
  hfHome  = "$HOME/.cache/mesh-spike/hf";

in pkgs.mkShell {
  name = "mesh-mlspike-shell";

  packages = with pkgs; [
    # Python toolchain — 3.11 is the version vLLM 0.20.1's bundled wheels
    # target. Bumping to 3.12 means rebuilding several CUDA wheels.
    python311
    python311Packages.pip
    python311Packages.venvShellHook  # not used directly but pulls runtime
    patchelf

    # Native libs that pip-installed Python wheels link against at runtime.
    # We don't put these in buildInputs (mkShell doesn't propagate to runtime)
    # — instead the shellHook exposes their lib/ dirs via LD_LIBRARY_PATH.
    zlib
    stdenv.cc.cc.lib   # libstdc++.so.6 for any C++-linked extensions
    glibc

    # CLI tools the bootstrap and serve scripts use
    curl
    git
    jq
    gnupatch           # applying our vLLM patch
  ];

  # No buildInputs for the venv itself — the venv vendors its own torch/cuda
  # wheels. We just expose the few NixOS-specific lib paths it needs to find.
  shellHook = ''
    set -e

    VENV="${venvDir}"
    HF_HOME="${hfHome}"
    BOOTSTRAP_STAMP="$VENV/.mesh-mlspike-bootstrap"

    # Compute paths up-front (cheap)
    ZLIB_LIBDIR="${pkgs.zlib}/lib"
    STDCPP_LIBDIR="${pkgs.stdenv.cc.cc.lib}/lib"
    PATCH_BIN="${pkgs.gnupatch}/bin/patch"
    PATCHELF_BIN="${pkgs.patchelf}/bin/patchelf"
    PYTHON_BIN="${pkgs.python311}/bin/python3.11"

    # Pin sentinel — re-bootstrap whenever the requirements file changes.
    # We hash the file contents so any edit (top-level pin or transitive)
    # forces a clean reinstall rather than risking partial state.
    REQS_FILE="$PWD/${requirementsFile}"
    if [ ! -f "$REQS_FILE" ]; then
      echo "[mlspike] ERROR: missing $REQS_FILE — refusing to bootstrap a non-pinned venv." >&2
      exit 1
    fi
    REQS_HASH=$(sha256sum "$REQS_FILE" | cut -c1-16)
    PIN_KEY="reqs=$REQS_HASH"
    PIN_KEY_FILE="$VENV/.mesh-mlspike-pins"

    bootstrap_needed=0
    if [ ! -x "$VENV/bin/vllm" ]; then bootstrap_needed=1; fi
    if [ ! -f "$BOOTSTRAP_STAMP" ]; then bootstrap_needed=1; fi
    if [ ! -f "$PIN_KEY_FILE" ] || [ "$(cat "$PIN_KEY_FILE" 2>/dev/null)" != "$PIN_KEY" ]; then
      bootstrap_needed=1
    fi

    if [ "$bootstrap_needed" = "1" ]; then
      echo "[mlspike] bootstrapping venv at $VENV (one-time, ~5-10 min)..."
      echo "[mlspike]   pin: $REQS_FILE (sha256 $REQS_HASH)"

      # 1. venv
      if [ ! -x "$VENV/bin/python" ] || ! "$VENV/bin/python" -c 'import sys' >/dev/null 2>&1; then
        echo "[mlspike]   creating Python 3.11 venv..."
        rm -rf "$VENV"
        "$PYTHON_BIN" -m venv "$VENV"
      fi

      # 2. pip-install from pinned requirements file. Every transitive dep
      #    is locked → bootstraps are reproducible across machines and time.
      echo "[mlspike]   installing from $REQS_FILE (216 pins, ~5-10 min)..."
      LD_LIBRARY_PATH="$ZLIB_LIBDIR:$STDCPP_LIBDIR:''${LD_LIBRARY_PATH:-}" \
        "$VENV/bin/pip" install --quiet --upgrade pip
      LD_LIBRARY_PATH="$ZLIB_LIBDIR:$STDCPP_LIBDIR:''${LD_LIBRARY_PATH:-}" \
        "$VENV/bin/pip" install --quiet -r "$REQS_FILE"

      # 3. patchelf bundled triton ELFs (ptxas etc.) for NixOS
      #    Triton ships generic-Linux binaries that fail on NixOS with
      #    "ELF interpreter not found"; rewrite to the NixOS glibc loader.
      GLIBC_LD=$(ls /nix/store/*glibc-2.4*/lib/ld-linux-x86-64.so.2 2>/dev/null | sort -V | tail -1)
      GLIBC_LIB=$(dirname "$GLIBC_LD" 2>/dev/null)
      if [ -n "$GLIBC_LD" ] && [ -d "$VENV/lib/python3.11/site-packages/triton/backends/nvidia/bin" ]; then
        echo "[mlspike]   patchelf'ing triton/cuda ELFs..."
        for bin in ptxas ptxas-blackwell nvdisasm cuobjdump; do
          target="$VENV/lib/python3.11/site-packages/triton/backends/nvidia/bin/$bin"
          if [ -f "$target" ]; then
            "$PATCHELF_BIN" --set-interpreter "$GLIBC_LD" --set-rpath "$GLIBC_LIB" "$target" 2>/dev/null || true
          fi
        done
      fi

      # 4. apply the vLLM MF rote_timestamps patch (idempotent)
      MF_FILE="$VENV/lib/python3.11/site-packages/vllm/model_executor/models/musicflamingo.py"
      PATCH_FILE="$PWD/${mfPatch}"
      if [ -f "$MF_FILE" ] && [ -f "$PATCH_FILE" ]; then
        if grep -q "_build_audio_timestamps" "$MF_FILE" 2>/dev/null; then
          echo "[mlspike]   MF patch already applied, skipping"
        else
          echo "[mlspike]   applying MF rote_timestamps patch..."
          ( cd "$VENV/lib/python3.11/site-packages" && "$PATCH_BIN" -p1 < "$PATCH_FILE" ) \
            || { echo "[mlspike] ERROR: MF patch failed to apply"; exit 1; }
        fi
      fi

      mkdir -p "$VENV"
      echo "$PIN_KEY" > "$PIN_KEY_FILE"
      date -Iseconds > "$BOOTSTRAP_STAMP"
      echo "[mlspike] bootstrap complete."
    fi

    # ---- env setup (every entry) ----
    export HF_HOME="$HF_HOME"
    export HF_HUB_CACHE="$HF_HOME/hub"

    # CUDA driver discovery (NixOS) — same pattern the serve scripts used,
    # now centralised here so they don't each reimplement it.
    for cand in /run/opengl-driver/lib /run/opengl-driver-32/lib /usr/lib/x86_64-linux-gnu /usr/lib64; do
      if [ -e "$cand/libcuda.so.1" ] || [ -e "$cand/libcuda.so" ]; then
        export LD_LIBRARY_PATH="$cand:''${LD_LIBRARY_PATH:-}"
        export TRITON_LIBCUDA_PATH="$cand"
        break
      fi
    done

    # zlib + libstdc++ for Python wheel imports (numpy etc.)
    export LD_LIBRARY_PATH="$ZLIB_LIBDIR:$STDCPP_LIBDIR:''${LD_LIBRARY_PATH:-}"

    # Bundled NVIDIA wheels (cublas, cudnn, cudnn-frontend, ...) need their
    # private .so dirs on LD_LIBRARY_PATH for torch + triton + flashinfer.
    for d in "$VENV"/lib/python3.11/site-packages/nvidia/*/lib; do
      [ -d "$d" ] && export LD_LIBRARY_PATH="$d:$LD_LIBRARY_PATH"
    done

    # BLAS multithreading. torch's import hooks pin OMP/MKL/OpenBLAS to 1
    # thread when these are unset, leaving ~23 cores idle during BT refit
    # / teacher training matrix ops. 16 threads saturates the memory-bound
    # reductions while leaving headroom for concurrent vLLM-worker decode.
    export OMP_NUM_THREADS=''${OMP_NUM_THREADS:-16}
    export OPENBLAS_NUM_THREADS=''${OPENBLAS_NUM_THREADS:-16}
    export MKL_NUM_THREADS=''${MKL_NUM_THREADS:-16}
    export NUMEXPR_NUM_THREADS=''${NUMEXPR_NUM_THREADS:-16}

    # Unbuffered Python so live `tail -f` of pipeline logs shows progress.
    export PYTHONUNBUFFERED=1

    # Marker so wrapper scripts (run_r7_step.sh) know the env is already
    # set up and can skip their own LD_LIBRARY_PATH dance.
    export MESH_MLSPIKE_ENV=1

    export PATH="$VENV/bin:$PATH"

    # Disable strict mode so the user shell isn't held under -e
    set +e

    # Banner — uses a width-aware printer so unicode (·, →, —) lines align
    # to the right edge. WIDTH = number of cells between ║ and ║.
    PY_VER=$("$VENV/bin/python" --version 2>&1 | head -1 | sed 's/Python //')
    VLLM_VER=$("$VENV/bin/python" -c 'import vllm; print(vllm.__version__)' 2>/dev/null || echo "?")
    TRF_VER=$("$VENV/bin/python" -c 'import transformers; print(transformers.__version__)' 2>/dev/null || echo "?")
    BOX_W=68
    box() {
      # `wc -m` counts characters (not bytes), so multi-byte unicode
      # like · → — counts as 1 each — same as visual cells in the terminal.
      local s="$1"
      local len pad
      len=$(printf '%s' "$s" | wc -m)
      pad=$((BOX_W - len))
      if [ "$pad" -lt 0 ]; then
        # Line longer than the box — print without right border (open right).
        printf "║%s\n" "$s"
      else
        printf "║%s%*s║\n" "$s" "$pad" ""
      fi
    }
    echo ""
    echo "╔════════════════════════════════════════════════════════════════════╗"
    box "   Mesh ML-spike   vllm $VLLM_VER · transformers $TRF_VER · py $PY_VER"
    echo "╠══ Round-7.6 V18 pipeline (run from project root) ══════════════════╣"
    box ""
    box "  1. Start Music Flamingo (terminal A)"
    box "       bash spike/track-grading/serve_music_flamingo.sh   # :8001"
    box ""
    box "  2. Caption sweep (terminal B in devshell)             ~7 hr"
    box "       bash spike/track-grading/run_round7_6_pipeline.sh caption-full"
    box ""
    box "  3. Text-LLM rating — run N independent jurors (any TEXT_LLM_TAG)."
    box "     Each writes round7_6_caption_intensity_<tag>.npz; step 5 auto-"
    box "     aggregates every *_caption_intensity*.npz it finds (skips _smoke)."
    box "     Add as many as you want; Dawid-Skene weights each by reliability."
    box ""
    box "     A) remote juror (terminal C, parallel with step 2):"
    box "       export TEXT_LLM_URL=http://172.16.54.147:8000/v1/chat/completions"
    box "       export TEXT_LLM_MODEL=nvidia/NVIDIA-Nemotron-3-Nano-30B-A3B-NVFP4"
    box "       export TEXT_LLM_TAG=nemotron  TEXT_LLM_NO_THINK=0"
    box "       ./...pipeline.sh caption-rate-streaming"
    box ""
    box "     B) local juror — after step 2 finishes (frees GPU for serving):"
    box "       pkill -f 'python.*api_server'"
    box "       TEXT_LLM_MODEL=mistralai/Devstral-Small-2507 \\"
    box "         bash spike/track-grading/serve_text_llm.sh   # :8002"
    box "       TEXT_LLM_TAG=devstral \\"
    box "         bash ...pipeline.sh caption-rate"
    box ""
    box "     Add a third / fourth juror by repeating with a new TEXT_LLM_TAG."
    box ""
    box "  4. Free GPU before teacher training"
    box "       pkill -f 'python.*api_server'"
    box ""
    box "  5. Train V18 (consensus→teacher→student→eval→export) ~2 hr"
    box "       bash spike/track-grading/run_round7_6_pipeline.sh v18-train"
    box ""
    box "  6. Inspect"
    box "       less <mesh-track-grading>/round7_6_eval_report.md"
    box "       ls   models/aggression-axes/V18_round7_6_*.json"
    box ""
    echo "╠══ Helpers ═════════════════════════════════════════════════════════╣"
    box "  Smoke:   ./...pipeline.sh v18-smoke   (200 cached captions)"
    box "  Logs:    tail -F <mesh-track-grading>/logs/*.log"
    box "  GPU:     nvidia-smi   Health: curl -sf localhost:{8001,8002}/health"
    box "  Stop:    pkill -f 'python.*api_server'    (servers; orchestrator: ^C)"
    box "  Resume:  caps + rate are per-track resume-safe across days;"
    box "             re-run the same command and it skips finished work."
    box "             Train (S10/S11) re-runs from disk in <2 hr."
    box "  Pin:     nix/mlspike-requirements.txt (sha256 $REQS_HASH)"
    box "  Spec:    documents/round-7-6-pipeline-spec.md"
    echo "╚════════════════════════════════════════════════════════════════════╝"
    echo ""
  '';
}
