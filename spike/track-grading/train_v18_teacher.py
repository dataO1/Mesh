"""Stage S10 — Train the privileged-information teacher.

Per spec § 16. 2-layer MLP over [audio_emb, caption_emb, struct_tags,
r7.5_tags]. Outputs: 16 axis heads (predicting r7.5 BT priors) + 1
intensity head (predicting consensus). Multi-task MSE loss.

NOTE per G7: NO genre_OH / source_category flows in. Only
  (a) audio_emb (MuQ-MuLan, dim depends on --audio-emb-key:
                 embeddings_1024 = 1024-d Conformer hidden, the
                   round-7.7 default; also the student's only input
                 embeddings      = 512-d L2-normalized joint-space, the
                   v18.1-era substrate, kept selectable for ablation)
  (b) caption_emb (bge-base, 768d) — privileged at training, distilled away
  (c) struct_tags (~50d boolean → float32) — derived from caption text
  (d) r7.5_tags (13d) — mined evidence

Outputs:
  - <out-dir>/round7_6_teacher.pt        — model weights
  - <out-dir>/round7_6_teacher_metrics.json
  - <out-dir>/round7_6_teacher_preds.npz — full-corpus teacher outputs
                                           (intensity + 16 axes + penultimate
                                           features), used for student
                                           distillation in S11
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

import numpy as np


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--audio-emb", type=Path, required=True)        # corpus_muq_mulan.npz
    p.add_argument("--audio-emb-key", default="embeddings_1024",
                   choices=["embeddings_1024", "embeddings"],
                   help="which audio head to use as teacher input. "
                        "embeddings_1024 = 1024-d Conformer hidden "
                        "(round-7.7 default per the MuQ paper's probe "
                        "recipe). embeddings = 512-d joint-space latent "
                        "(v18.1-era substrate, kept for ablation).")
    p.add_argument("--caption-emb", type=Path, required=True)
    p.add_argument("--struct-tags", type=Path, required=True)
    # Round-7.5 inputs are now optional. When omitted, the teacher trains
    # without the 16-axis auxiliary head (β=0 effectively) and without the
    # 13d r7.5 tag feature. Decided 2026-05-08 because r7.5 only covers
    # 38% of the expanded 40k corpus, and forcing the alignment shrinks
    # the teacher's training set to that subset. See pipeline-spec note.
    p.add_argument("--r75-priors", type=Path, default=None,
                   help="(optional) round-7.5 BT priors NPZ; enables 16-axis aux head")
    p.add_argument("--r75-tags", type=Path, default=None,
                   help="(optional) round-7.5 mined tags NPZ; adds 13d to teacher input")
    p.add_argument("--consensus", type=Path, required=True)        # primary target
    p.add_argument("--split", type=Path, required=True)
    p.add_argument("--out-dir", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading"))
    p.add_argument("--hidden1", type=int, default=256)
    p.add_argument("--hidden2", type=int, default=128)
    p.add_argument("--dropout", type=float, default=0.2)
    p.add_argument("--lr", type=float, default=3e-4)
    p.add_argument("--weight-decay", type=float, default=1e-4)
    p.add_argument("--batch-size", type=int, default=256)
    p.add_argument("--epochs", type=int, default=100)
    p.add_argument("--patience", type=int, default=10)
    p.add_argument("--alpha-intensity", type=float, default=1.0)
    p.add_argument("--beta-axes", type=float, default=0.3)
    p.add_argument("--seed", type=int, default=42)
    return p.parse_args()


def main(args) -> int:
    import torch
    import torch.nn as nn

    torch.manual_seed(args.seed)
    np.random.seed(args.seed)
    # Pin cudnn determinism so reruns produce bit-identical V18 weights (G10).
    torch.backends.cudnn.deterministic = True
    torch.backends.cudnn.benchmark = False
    device = "cuda" if torch.cuda.is_available() else "cpu"
    print(f"[teacher] device: {device}  (cudnn.deterministic=True)")

    # ── Load all features & labels, align by track_id ─────────────────
    e = np.load(args.audio_emb, allow_pickle=True)
    audio_tids = e["track_ids"].astype(np.int64)
    if args.audio_emb_key not in e.files:
        sys.exit(f"[teacher] audio_emb NPZ at {args.audio_emb} has no "
                 f"'{args.audio_emb_key}' field (available: {list(e.files)}). "
                 f"Re-run embed_corpus_mulan.py with the round-7.7 dual-head "
                 f"version, or pass --audio-emb-key embeddings to use the "
                 f"v18.1-era 512-d substrate.")
    audio_arr  = e[args.audio_emb_key].astype(np.float32)
    audio_tid_to_i = {int(t): i for i, t in enumerate(audio_tids)}
    print(f"[teacher] audio head: {args.audio_emb_key} (dim={audio_arr.shape[1]})")

    c = np.load(args.caption_emb, allow_pickle=True)
    cap_tids = c["track_ids"].astype(np.int64)
    cap_arr  = c["caption_emb"].astype(np.float32)
    cap_tid_to_i = {int(t): i for i, t in enumerate(cap_tids)}

    s = np.load(args.struct_tags, allow_pickle=True)
    st_tids = s["track_ids"].astype(np.int64)
    st_arr  = s["tag_present"].astype(np.float32)        # boolean → 0/1
    st_tid_to_i = {int(t): i for i, t in enumerate(st_tids)}

    use_r75_tags = args.r75_tags is not None
    use_r75_priors = args.r75_priors is not None
    if use_r75_tags:
        rt = np.load(args.r75_tags, allow_pickle=True)
        rt_tids = rt["track_ids"].astype(np.int64)
        rt_arr  = rt["tag_evidence"].astype(np.float32)
        # z-score r7.5 tags so feature scale is comparable to the others
        rt_arr = (rt_arr - rt_arr.mean(axis=0)) / (rt_arr.std(axis=0) + 1e-6)
        rt_tid_to_i = {int(t): i for i, t in enumerate(rt_tids)}
    else:
        rt_tids = None; rt_arr = None; rt_tid_to_i = None

    if use_r75_priors:
        rp = np.load(args.r75_priors, allow_pickle=True)
        rp_tids = rp["track_ids"].astype(np.int64)
        axis_names = list(rp["axes"])
        rp_scores = rp["scores"]                              # [16, N]
        # z-score per axis
        rp_z = (rp_scores - rp_scores.mean(axis=1, keepdims=True)) / \
               (rp_scores.std(axis=1, keepdims=True) + 1e-6)
        rp_tid_to_i = {int(t): i for i, t in enumerate(rp_tids)}
    else:
        rp_tids = None; axis_names = []; rp_z = None; rp_tid_to_i = None

    cs = np.load(args.consensus, allow_pickle=True)
    cs_tids = cs["track_ids"].astype(np.int64)
    cs_arr  = cs["consensus_intensity"].astype(np.float32)
    cs_tid_to_i = {int(t): i for i, t in enumerate(cs_tids)}

    sp = np.load(args.split, allow_pickle=True)
    sp_tids = sp["track_ids"].astype(np.int64)
    sp_split = sp["split"]
    sp_tid_to_split = {int(t): str(sp_split[i]) for i, t in enumerate(sp_tids)}

    # Tracks must have all features + labels + a split tag
    common = (set(int(t) for t in audio_tids)
              & set(int(t) for t in cap_tids)
              & set(int(t) for t in st_tids)
              & set(int(t) for t in cs_tids)
              & set(sp_tid_to_split.keys()))
    if use_r75_tags:
        common &= set(int(t) for t in rt_tids)
    if use_r75_priors:
        common &= set(int(t) for t in rp_tids)
    track_ids = sorted(common)
    print(f"[teacher] aligned: {len(track_ids)} tracks across all features")

    # Build feature matrix
    N = len(track_ids)
    A = audio_arr.shape[1]
    C = cap_arr.shape[1]
    S = st_arr.shape[1]
    T = rt_arr.shape[1] if use_r75_tags else 0
    AX = rp_z.shape[0] if use_r75_priors else 0
    print(f"[teacher] feature dims: audio={A} caption={C} struct={S} r75tags={T}; "
          f"axis heads={AX}")

    X = np.zeros((N, A + C + S + T), dtype=np.float32)
    y_int = np.zeros(N, dtype=np.float32)
    y_axes = np.zeros((N, AX), dtype=np.float32) if AX > 0 else None
    splits = np.empty(N, dtype=object)
    for i, tid in enumerate(track_ids):
        X[i, :A]                = audio_arr[audio_tid_to_i[tid]]
        X[i, A:A+C]             = cap_arr[cap_tid_to_i[tid]]
        X[i, A+C:A+C+S]         = st_arr[st_tid_to_i[tid]]
        if use_r75_tags:
            X[i, A+C+S:A+C+S+T] = rt_arr[rt_tid_to_i[tid]]
        y_int[i]                = cs_arr[cs_tid_to_i[tid]]
        if use_r75_priors:
            y_axes[i]           = rp_z[:, rp_tid_to_i[tid]]
        splits[i]               = sp_tid_to_split[tid]
    print(f"[teacher] X shape={X.shape}  y_int range=[{y_int.min():.3f},{y_int.max():.3f}]")

    train_idx = np.where(splits == "train")[0]
    val_idx   = np.where(splits == "val")[0]
    test_idx  = np.where(splits == "test")[0]
    print(f"[teacher] train={len(train_idx)} val={len(val_idx)} test={len(test_idx)}")

    # ── Build model ────────────────────────────────────────────────────
    class Teacher(nn.Module):
        def __init__(self, in_dim, h1, h2, n_axes, p):
            super().__init__()
            self.fc1 = nn.Linear(in_dim, h1)
            self.fc2 = nn.Linear(h1, h2)
            self.dropout = nn.Dropout(p)
            self.act = nn.GELU()
            self.head_int = nn.Linear(h2, 1)
            self.has_axes = n_axes > 0
            self.head_axes = nn.Linear(h2, n_axes) if self.has_axes else None

        def forward(self, x, return_penultimate=False):
            h = self.act(self.fc1(x))
            h = self.dropout(h)
            penult = self.act(self.fc2(h))
            ax = self.head_axes(penult) if self.has_axes else None
            if return_penultimate:
                return self.head_int(penult).squeeze(-1), ax, penult
            return self.head_int(penult).squeeze(-1), ax

    in_dim = X.shape[1]
    model = Teacher(in_dim, args.hidden1, args.hidden2, AX, args.dropout).to(device)
    opt = torch.optim.AdamW(model.parameters(), lr=args.lr, weight_decay=args.weight_decay)
    print(f"[teacher] model params: {sum(p.numel() for p in model.parameters()):_}")

    Xt = torch.from_numpy(X).to(device)
    yi = torch.from_numpy(y_int).to(device)
    ya = torch.from_numpy(y_axes).to(device) if y_axes is not None else None
    train_t = torch.from_numpy(train_idx).to(device)
    val_t = torch.from_numpy(val_idx).to(device)

    best_val = float("inf"); best_epoch = -1; best_state = None
    history = []
    t0 = time.time()
    for epoch in range(args.epochs):
        model.train()
        perm = torch.randperm(len(train_idx), device=device)
        train_loss = 0.0; n_batches = 0
        for k in range(0, len(perm), args.batch_size):
            idx = train_t[perm[k:k+args.batch_size]]
            pred_int, pred_ax = model(Xt[idx])
            loss_int = nn.functional.mse_loss(pred_int, yi[idx])
            if model.has_axes:
                loss_ax = nn.functional.mse_loss(pred_ax, ya[idx])
                loss = args.alpha_intensity * loss_int + args.beta_axes * loss_ax
            else:
                loss = args.alpha_intensity * loss_int
            opt.zero_grad()
            loss.backward()
            opt.step()
            train_loss += loss.item(); n_batches += 1
        train_loss /= max(n_batches, 1)

        model.eval()
        with torch.no_grad():
            pred_int_v, pred_ax_v = model(Xt[val_t])
            v_int = nn.functional.mse_loss(pred_int_v, yi[val_t]).item()
            v_ax = (nn.functional.mse_loss(pred_ax_v, ya[val_t]).item()
                    if model.has_axes else float("nan"))
        history.append({"epoch": epoch, "train_loss": train_loss,
                        "val_int_mse": v_int, "val_ax_mse": v_ax})

        if v_int < best_val:
            best_val = v_int; best_epoch = epoch
            best_state = {k: v.detach().clone() for k, v in model.state_dict().items()}

        if epoch - best_epoch >= args.patience:
            print(f"[teacher] early stop at epoch {epoch} (best @ {best_epoch})")
            break

        if epoch % 5 == 0 or epoch == args.epochs - 1:
            ax_str = f"val_ax={v_ax:.4f}  " if model.has_axes else ""
            print(f"  ep {epoch:>3d}  train={train_loss:.4f}  "
                  f"val_int={v_int:.4f}  {ax_str}best@{best_epoch}")

    wall = time.time() - t0
    print(f"[teacher] trained in {wall:.1f}s; best val_int_mse={best_val:.4f} "
          f"@ ep {best_epoch}")

    # Restore best
    model.load_state_dict(best_state)
    model.eval()

    # ── Test PA + cache full-corpus teacher predictions for student S11 ─
    def spearman(a, b):
        a, b = np.asarray(a), np.asarray(b)
        n = len(a)
        ra = np.argsort(np.argsort(a)); rb = np.argsort(np.argsort(b))
        return 1 - 6 * float(np.sum((ra - rb) ** 2)) / (n * (n*n - 1))

    def pa(s, y):
        n = len(s)
        ds = s[:, None] - s[None, :]; dy = y[:, None] - y[None, :]
        tri = np.triu(np.ones((n, n), dtype=bool), k=1)
        valid = tri & (ds != 0) & (dy != 0)
        return float((valid & ((ds > 0) == (dy > 0))).sum() / max(valid.sum(), 1))

    with torch.no_grad():
        pred_int_all, pred_ax_all, penult_all = model(Xt, return_penultimate=True)
    pred_int_np = pred_int_all.cpu().numpy()
    pred_ax_np  = pred_ax_all.cpu().numpy() if pred_ax_all is not None else np.zeros((Xt.shape[0], 0), dtype=np.float32)
    penult_np   = penult_all.cpu().numpy()

    test_int_pred = pred_int_np[test_idx]
    test_int_true = y_int[test_idx]
    test_pa = pa(test_int_pred, test_int_true)
    test_rho = spearman(test_int_pred, test_int_true)
    print(f"[teacher] TEST PA={test_pa:.4f}  Spearman={test_rho:.4f}")

    # ── Persist ───────────────────────────────────────────────────────
    args.out_dir.mkdir(parents=True, exist_ok=True)
    torch.save(model.state_dict(), args.out_dir / "round7_6_teacher.pt")
    (args.out_dir / "round7_6_teacher_metrics.json").write_text(json.dumps({
        "history": history,
        "best_epoch": best_epoch,
        "best_val_int_mse": best_val,
        "test_pa": test_pa,
        "test_spearman": test_rho,
        "feature_dims": {"audio": A, "caption": C, "struct": S, "r75_tags": T,
                         "axis_heads": AX},
        "in_dim": in_dim,
        "hidden1": args.hidden1, "hidden2": args.hidden2,
        "wall_seconds": wall,
    }, indent=2))
    np.savez(
        args.out_dir / "round7_6_teacher_preds.npz",
        track_ids=np.array(track_ids, dtype=np.int64),
        teacher_intensity=pred_int_np,
        teacher_axes=pred_ax_np,
        teacher_penultimate=penult_np,
        split=splits,
        consensus_intensity=y_int,
    )
    print(f"[teacher] wrote {args.out_dir}/round7_6_teacher.pt + metrics + preds")
    return 0


if __name__ == "__main__":
    sys.exit(main(parse_args()))
