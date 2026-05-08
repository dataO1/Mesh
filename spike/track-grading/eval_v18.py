"""Stage S12 — Held-out evaluation + caption-emb K-means cluster diagnostic.

Per spec § 18. Reports on the held-out test set:

  1. Primary: PA(student, consensus) — the deployment metric (G3, ≥ 0.75)
  2. Spearman ρ, R²
  3. Per-cluster PA via caption-emb K-means (NOT source_category, per G7)
  4. Per-cluster intensity histogram + cluster theme via top-3 nearest captions
  5. V15 / V17b / student PA on the same test set
  6. Distillation gap teacher → student

Output:
  - <out-dir>/round7_6_eval_report.md
  - <out-dir>/round7_6_eval.json
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--audio-emb", type=Path, required=True)
    p.add_argument("--caption-emb", type=Path, required=True)
    p.add_argument("--captions-root", type=Path, required=True,
                   help="for cluster-theme strings (top-3 nearest captions)")
    p.add_argument("--teacher-preds", type=Path, required=True)
    p.add_argument("--student-pt", type=Path, required=True)
    p.add_argument("--consensus", type=Path, required=True)
    p.add_argument("--split", type=Path, required=True)
    p.add_argument("--v15", type=Path,
                   default=Path("models/aggression-axes/V15_linear_probe_r6.json"))
    p.add_argument("--v17b", type=Path,
                   default=Path("models/aggression-axes/V17_round7_5_polar_blend.json"))
    p.add_argument("--out-dir", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading"))
    p.add_argument("--n-clusters", type=int, default=20)
    p.add_argument("--seed", type=int, default=42)
    return p.parse_args()


def spearman(a, b):
    a, b = np.asarray(a, dtype=np.float64), np.asarray(b, dtype=np.float64)
    n = len(a)
    if n < 3: return float("nan")
    ra = np.argsort(np.argsort(a)); rb = np.argsort(np.argsort(b))
    return 1 - 6 * float(np.sum((ra - rb) ** 2)) / (n * (n*n - 1))


def pa(s, y):
    s, y = np.asarray(s), np.asarray(y); n = len(s)
    if n < 2: return float("nan")
    ds = s[:, None] - s[None, :]; dy = y[:, None] - y[None, :]
    tri = np.triu(np.ones((n, n), dtype=bool), k=1)
    valid = tri & (ds != 0) & (dy != 0)
    return float((valid & ((ds > 0) == (dy > 0))).sum() / max(valid.sum(), 1))


def r2(pred, true):
    ss_res = float(np.sum((true - pred) ** 2))
    ss_tot = float(np.sum((true - true.mean()) ** 2))
    return 1 - ss_res / max(ss_tot, 1e-12)


def main(args) -> int:
    import torch

    # ── Load data ──────────────────────────────────────────────────────
    e = np.load(args.audio_emb, allow_pickle=True)
    audio_tids = e["track_ids"].astype(np.int64)
    audio_arr = e["embeddings"].astype(np.float32)
    audio_tid_to_i = {int(t): i for i, t in enumerate(audio_tids)}

    c = np.load(args.caption_emb, allow_pickle=True)
    cap_tids = c["track_ids"].astype(np.int64)
    cap_arr = c["caption_emb"].astype(np.float32)
    cap_tid_to_i = {int(t): i for i, t in enumerate(cap_tids)}

    tp = np.load(args.teacher_preds, allow_pickle=True)
    tp_tids = tp["track_ids"].astype(np.int64)
    teacher_int = tp["teacher_intensity"].astype(np.float32)
    tp_tid_to_i = {int(t): i for i, t in enumerate(tp_tids)}

    cs = np.load(args.consensus, allow_pickle=True)
    cs_tids = cs["track_ids"].astype(np.int64)
    cs_arr = cs["consensus_intensity"].astype(np.float32)
    cs_tid_to_i = {int(t): i for i, t in enumerate(cs_tids)}

    sp = np.load(args.split, allow_pickle=True)
    sp_tids = sp["track_ids"].astype(np.int64)
    sp_split = sp["split"]
    sp_tid_to_split = {int(t): str(sp_split[i]) for i, t in enumerate(sp_tids)}

    # ── Load student weights ───────────────────────────────────────────
    # Auto-detect arch (linear vs mlp) so we can eval either V18 baseline
    # or V18.1 MLP from the same code path. Matches export_v18.py.
    state = torch.load(args.student_pt, map_location="cpu", weights_only=True)
    if "intensity.weight" in state:
        student_arch = "linear"
        W = state["intensity.weight"].numpy().squeeze()
        b = float(state["intensity.bias"].numpy().squeeze())
        print(f"[eval] student arch=linear, vec dim={W.shape}, bias={b:+.4f}")

        def student_score_fn(audio: np.ndarray) -> np.ndarray:
            return audio @ W + b
    elif "intensity.0.weight" in state and "intensity.3.weight" in state:
        student_arch = "mlp"
        W1 = state["intensity.0.weight"].numpy().astype(np.float32)
        b1 = state["intensity.0.bias"].numpy().astype(np.float32)
        W2 = state["intensity.3.weight"].numpy().astype(np.float32)
        b2 = float(state["intensity.3.bias"].numpy().squeeze())
        from math import sqrt
        from scipy.special import erf as _erf
        SQRT2 = sqrt(2.0)
        print(f"[eval] student arch=mlp, hidden={W1.shape[0]}, bias={b2:+.4f}")

        def student_score_fn(audio: np.ndarray) -> np.ndarray:
            h = audio @ W1.T + b1
            h = 0.5 * h * (1.0 + _erf(h / SQRT2))   # GELU (exact)
            return (h @ W2.T + b2).squeeze(-1)
    else:
        sys.exit(f"unrecognized student state_dict keys: {list(state.keys())[:8]}")

    # ── V15 / V17b reference vectors ──────────────────────────────────
    v15 = json.loads(args.v15.read_text())
    v15_vec = np.asarray(v15["intensity_axis_vec"], dtype=np.float32)
    v17b = json.loads(args.v17b.read_text())
    v17b_vec = np.asarray(v17b["intensity_axis_vec"], dtype=np.float32)

    # ── Tracks with all data ──────────────────────────────────────────
    common = (set(int(t) for t in audio_tids)
              & set(int(t) for t in cap_tids)
              & set(int(t) for t in tp_tids)
              & set(int(t) for t in cs_tids)
              & set(sp_tid_to_split.keys()))
    track_ids = sorted(common)
    N = len(track_ids)
    print(f"[eval] {N} tracks aligned")

    A = audio_arr.shape[1]
    aud = np.zeros((N, A), dtype=np.float32)
    cap = np.zeros((N, cap_arr.shape[1]), dtype=np.float32)
    teach_int = np.zeros(N, dtype=np.float32)
    cons = np.zeros(N, dtype=np.float32)
    splits = np.empty(N, dtype=object)
    for i, tid in enumerate(track_ids):
        aud[i] = audio_arr[audio_tid_to_i[tid]]
        cap[i] = cap_arr[cap_tid_to_i[tid]]
        teach_int[i] = teacher_int[tp_tid_to_i[tid]]
        cons[i] = cs_arr[cs_tid_to_i[tid]]
        splits[i] = sp_tid_to_split[tid]
    test_idx = np.where(splits == "test")[0]
    print(f"[eval] test={len(test_idx)}")

    # ── Score with each model ─────────────────────────────────────────
    student_score = student_score_fn(aud)
    v15_score = aud @ v15_vec
    v17b_score = aud @ v17b_vec

    # ── G9 CPU-latency microbench (1000 random tracks) ────────────────
    import time as _time
    rng_bench = np.random.default_rng(args.seed)
    bench_idx = rng_bench.choice(len(track_ids), size=min(1000, len(track_ids)),
                                 replace=False)
    aud_bench = np.ascontiguousarray(aud[bench_idx], dtype=np.float32)
    # Warm + bench through the same scoring fn used for real eval, so the
    # numbers reflect the actual deployed cost (linear or mlp).
    _ = student_score_fn(aud_bench)
    t0 = _time.perf_counter()
    for _ in range(10):
        _ = student_score_fn(aud_bench)
    t1 = _time.perf_counter()
    g9_total_ms = ((t1 - t0) / 10) * 1000
    g9_per_track_us = (g9_total_ms / len(bench_idx)) * 1000
    print(f"[g9-bench] {len(bench_idx)} tracks via numpy dot: "
          f"total={g9_total_ms:.3f} ms ({g9_per_track_us:.2f} µs/track)")

    # ── Primary metrics on test ───────────────────────────────────────
    metrics = {}
    metrics["test_pa_student"] = pa(student_score[test_idx], cons[test_idx])
    metrics["test_pa_teacher"] = pa(teach_int[test_idx], cons[test_idx])
    metrics["test_pa_v15"]     = pa(v15_score[test_idx], cons[test_idx])
    metrics["test_pa_v17b"]    = pa(v17b_score[test_idx], cons[test_idx])
    metrics["test_spearman_student"] = spearman(student_score[test_idx], cons[test_idx])
    metrics["test_r2_student"] = r2(student_score[test_idx], cons[test_idx])
    metrics["distillation_gap_pp"] = metrics["test_pa_teacher"] - metrics["test_pa_student"]
    metrics["g9_bench_total_ms"] = g9_total_ms
    metrics["g9_bench_per_track_us"] = g9_per_track_us
    metrics["g9_bench_n_tracks"] = int(len(bench_idx))

    print()
    print(f"=== Held-out test PA (vs consensus, N_test={len(test_idx)}) ===")
    print(f"  student (V18):    {metrics['test_pa_student']:.4f}")
    print(f"  teacher:          {metrics['test_pa_teacher']:.4f}")
    print(f"  V15 (deployed):   {metrics['test_pa_v15']:.4f}")
    print(f"  V17b polar blend: {metrics['test_pa_v17b']:.4f}")
    print(f"  distill gap:      {metrics['distillation_gap_pp']:+.4f} pp")
    print(f"  student Spearman: {metrics['test_spearman_student']:.4f}")
    print(f"  student R²:       {metrics['test_r2_student']:.4f}")

    # ── Caption-emb K-means clustering for the audible-genre diagnostic ─
    print(f"\n[eval] K-means clustering caption_emb into {args.n_clusters} clusters ...")
    from sklearn.cluster import KMeans
    rng = np.random.default_rng(args.seed)
    km = KMeans(n_clusters=args.n_clusters, random_state=args.seed, n_init=10)
    cluster = km.fit_predict(cap)        # [N]

    # Top-3 nearest captions per cluster (for theme labelling)
    print(f"[eval] reading captions for cluster themes ...")
    captions_text: dict[int, str] = {}
    for tid in track_ids:
        f = args.captions_root / f"{tid}.json"
        if f.exists():
            try:
                rec = json.loads(f.read_text())
                captions_text[tid] = (rec.get("caption") or "").strip()
            except Exception:
                pass

    cluster_themes: dict[int, list[str]] = {}
    for k in range(args.n_clusters):
        members = np.where(cluster == k)[0]
        if len(members) == 0:
            cluster_themes[k] = []
            continue
        # Distance to centroid
        d = np.linalg.norm(cap[members] - km.cluster_centers_[k], axis=1)
        order = np.argsort(d)[:3]
        themes = []
        for ord_idx in order:
            tid = track_ids[members[ord_idx]]
            t = captions_text.get(tid, "")
            # First sentence of caption
            t = t.split(".", 1)[0].strip() if "." in t else t.strip()
            themes.append(t[:200])
        cluster_themes[k] = themes

    # Per-cluster PA on test
    per_cluster = []
    for k in range(args.n_clusters):
        idx_k = test_idx[cluster[test_idx] == k]
        if len(idx_k) < 5:
            per_cluster.append({"k": k, "n_test": int(len(idx_k)),
                                "pa_student": float("nan"),
                                "mean_intensity": float("nan"),
                                "themes": cluster_themes[k]})
            continue
        per_cluster.append({
            "k": k,
            "n_test": int(len(idx_k)),
            "pa_student": pa(student_score[idx_k], cons[idx_k]),
            "mean_intensity": float(student_score[idx_k].mean()),
            "themes": cluster_themes[k],
        })

    # Sort by mean intensity for the histogram-style output
    per_cluster.sort(key=lambda r: r["mean_intensity"] if not np.isnan(r["mean_intensity"]) else 0)

    # ── Write report ──────────────────────────────────────────────────
    args.out_dir.mkdir(parents=True, exist_ok=True)
    md = ["# Round-7.6 V18 evaluation report",
          "",
          f"Held-out test set size: **{len(test_idx)} tracks** "
          f"(out of {N} aligned).",
          "",
          "## Headline metrics (vs consensus on held-out test)",
          "",
          f"| Model | PA | Spearman | R² | Notes |",
          f"|---|---:|---:|---:|---|",
          f"| **V18 student (deployed)** | **{metrics['test_pa_student']:.4f}** | "
          f"{metrics['test_spearman_student']:+.4f} | "
          f"{metrics['test_r2_student']:+.4f} | linear probe over MuQ-MuLan |",
          f"| V18 teacher | {metrics['test_pa_teacher']:.4f} | — | — | "
          f"privileged: audio + caption + tags |",
          f"| V15 (deployed) | {metrics['test_pa_v15']:.4f} | — | — | round-6 reference |",
          f"| V17b polar blend | {metrics['test_pa_v17b']:.4f} | — | — | round-7.5 reference |",
          "",
          f"Teacher → student distillation gap: "
          f"**{metrics['distillation_gap_pp']:+.4f} pp** "
          f"(spec G6 target ≤ 5 pp).",
          "",
          "## Per-cluster diagnostic (caption-emb K-means)",
          "",
          "Audible-genre breakdown via K-means on the 768d bge-base caption "
          "embeddings. Replaces per-genre breakdown — uses what the model "
          "actually heard, not the unreliable everynoise tags (per G7).",
          "",
          "Sorted by **mean predicted intensity** (low → high):",
          "",
          f"| k | n_test | mean_int | PA(student) | top-3 nearest captions |",
          f"|---|---:|---:|---:|---|",
    ]
    for r in per_cluster:
        themes_md = "; ".join(t.replace("|", "\\|") for t in r["themes"])
        mi = r["mean_intensity"]; pa_s = r["pa_student"]
        md.append(f"| {r['k']} | {r['n_test']} | "
                  f"{(f'{mi:+.3f}' if not np.isnan(mi) else '—')} | "
                  f"{(f'{pa_s:.3f}' if not np.isnan(pa_s) else '—')} | "
                  f"{themes_md[:240]} |")

    md.append("")
    md.append("## Spec compliance (Appendix D rubric)")
    md.append("")
    md.append(f"- **G3** test PA ≥ 0.75: "
              f"{'PASS' if metrics['test_pa_student'] >= 0.75 else 'FAIL'} "
              f"({metrics['test_pa_student']:.4f})")
    md.append(f"- **G6** distill gap ≤ 5 pp: "
              f"{'PASS' if metrics['distillation_gap_pp'] <= 0.05 else 'FAIL'} "
              f"({metrics['distillation_gap_pp']:+.4f})")
    g9_pass = metrics['g9_bench_total_ms'] < 100
    md.append(f"- **G9** CPU latency (1000-track dot): "
              f"{'PASS' if g9_pass else 'FAIL'} "
              f"({metrics['g9_bench_total_ms']:.2f} ms total, "
              f"{metrics['g9_bench_per_track_us']:.2f} µs/track)")
    md.append("")

    (args.out_dir / "round7_6_eval_report.md").write_text("\n".join(md))
    test_track_ids = [int(track_ids[i]) for i in test_idx]
    (args.out_dir / "round7_6_eval.json").write_text(json.dumps({
        "metrics": metrics,
        "per_cluster": per_cluster,
        "n_test": int(len(test_idx)),
        # Persist the exact test track IDs used so export_v18 can
        # reproduce the same PA without re-deriving the intersection
        # (which would differ if the export-side schema is narrower).
        "test_track_ids": test_track_ids,
    }, indent=2, default=float))
    print(f"\n[eval] wrote {args.out_dir}/round7_6_eval_report.md")
    print(f"[eval] wrote {args.out_dir}/round7_6_eval.json")
    return 0


if __name__ == "__main__":
    sys.exit(main(parse_args()))
