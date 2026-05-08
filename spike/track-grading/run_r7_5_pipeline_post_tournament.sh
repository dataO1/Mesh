#!/usr/bin/env bash
# Round-7.5 post-tournament pipeline.
#
# Run this AFTER the K=4 N-way tournament has completed (i.e. after you've
# manually started serve_qwen3_omni.sh and run_nway_tournaments_r7_5.py to
# completion). It chains all the downstream steps:
#
#   1. Build BT priors from K=4 rankings (~30 s)
#   2. Mine justifications into multi-label tags (~30 s)
#   3. Train multi-task linear probe with auxiliary tag loss (~3 min on 5090)
#   4. ListMLE blend on chosen target axis (~1 min)
#   5. Interpret axes (top/bottom 20 + correlation matrix)
#   6. Cross-library projection on user library
#   7. Export V17 JSON (NOT auto-deployed — V15 stays primary)
#   8. V15 vs V16 vs V17 head-to-head on user library
#
# Total wall: ~10 min after the tournament.
#
# Usage:
#   bash spike/track-grading/run_r7_5_pipeline_post_tournament.sh
set -euo pipefail

cd "$(dirname "$0")/../.."
RUN="bash spike/track-grading/run_r7_step.sh"

echo "=== 1/8 build_bt_priors_r7_5.py ==="
$RUN build_bt_priors_r7_5.py

echo "=== 2/8 justification_mining_r7_5.py (regex-only, fast) ==="
$RUN justification_mining_r7_5.py --include-r7

echo "=== 3/8 train_axes_r7_5.py (multi-task w/ aux tag loss) ==="
$RUN train_axes_r7_5.py

echo "=== 4/8 joint_blend_r7_5.py (ListMLE on timbre_roughness target) ==="
$RUN joint_blend_r7_5.py --target-axis timbre_roughness

echo "=== 5/8 interpret_axes_r7_5.py (top/bottom + corr matrix) ==="
$RUN interpret_axes_r7_5.py

echo "=== 6/8 cross_library_r7.py (re-purposed for r7.5 inputs) ==="
$RUN cross_library_r7.py \
  --axes-file /home/data01/Music/mesh-track-grading/round7_5_axes.npz \
  --blend-file /home/data01/Music/mesh-track-grading/round7_5_blend.npz \
  --out /home/data01/Music/mesh-track-grading/round7_5_cross_library.md

echo "=== 7/8 export_axis_r7_5.py (V17 JSON, NOT auto-deployed) ==="
$RUN export_axis_r7_5.py --no-deploy

echo "=== 8/8 compare_v15_v16_v17.py (head-to-head on user library) ==="
$RUN compare_v15_v16_v17.py

echo
echo "=== round-7.5 done ==="
echo "Outputs:"
echo "  /home/data01/Music/mesh-track-grading/round7_5_priors.npz"
echo "  /home/data01/Music/mesh-track-grading/round7_5_tags.npz"
echo "  /home/data01/Music/mesh-track-grading/round7_5_axes.npz"
echo "  /home/data01/Music/mesh-track-grading/round7_5_blend.npz"
echo "  /home/data01/Music/mesh-track-grading/round7_5_predictions.csv"
echo "  /home/data01/Music/mesh-track-grading/round7_5_train_metrics.json"
echo "  /home/data01/Music/mesh-track-grading/round7_5_interpretation.md"
echo "  /home/data01/Music/mesh-track-grading/round7_5_cross_library.md"
echo "  models/aggression-axes/V17_round7_5_polar_blend.json"
echo
echo "V15 stays deployed at <collection>/muq-mulan-aggression-axis.json."
echo "If V17 wins the head-to-head meaningfully, run:"
echo "  $RUN export_axis_r7_5.py    # without --no-deploy"
