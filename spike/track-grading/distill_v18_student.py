"""Stage S11 — Distill the linear-probe student from the teacher.

Per spec § 17. Student input: MuQ-MuLan(512) only. Output: 1-d intensity.
This is V18's deployed shape.

Loss = λ_out · MSE(student_out, teacher_intensity)
     + λ_fit · MSE(penult_proj(audio), teacher_penultimate)        [FitNets]
     + λ_kd  · KL(softmax(student/T) || softmax(teacher/T)) · T²    [Hinton]
     + λ_ls  · LabelSmoothing(student_out, consensus)                [direct]

The penultimate-projection layer (Linear 512 → 128) is a training-time
adapter that maps audio_emb into the teacher's penultimate space; it is
DISCARDED at export. The deployed student is the single 512→1 linear map.

Outputs:
  - <out-dir>/round7_6_student.pt
  - <out-dir>/round7_6_student_metrics.json
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
    p.add_argument("--audio-emb", type=Path, required=True)
    p.add_argument("--teacher-preds", type=Path, required=True,
                   help="from train_v18_teacher.py")
    p.add_argument("--consensus", type=Path, required=True)
    p.add_argument("--out-dir", type=Path,
                   default=Path("/home/data01/Music/mesh-track-grading"))
    p.add_argument("--lambda-out", type=float, default=1.0)
    p.add_argument("--lambda-fit", type=float, default=0.5)
    p.add_argument("--lambda-kd",  type=float, default=0.3)
    p.add_argument("--lambda-ls",  type=float, default=0.2)
    p.add_argument("--temperature", type=float, default=2.0)
    p.add_argument("--label-smooth", type=float, default=0.05)
    p.add_argument("--lr", type=float, default=1e-3)
    p.add_argument("--weight-decay", type=float, default=1e-4)
    p.add_argument("--batch-size", type=int, default=512)
    p.add_argument("--epochs", type=int, default=50)
    p.add_argument("--patience", type=int, default=10)
    p.add_argument("--seed", type=int, default=42)
    # Distinct output stems let us train multiple students in parallel
    # (e.g. linear V18 baseline + V18.1 MLP experiment) without overwriting
    # each other's checkpoints.
    p.add_argument("--out-stem", default="round7_6_student",
                   help="basename for student.pt + student_metrics.json (default: round7_6_student)")
    # Architecture knobs — added 2026-05-08 to escalate per spec §765-768.
    # Linear is the original spec G1; mlp is the spec-anticipated escalation
    # for closing the G6 distillation gap when linear-probe ceiling falls
    # short. CPU latency is ~0.05 ms/track at hidden=128 → still 2000× under
    # the G9 budget of 100 ms/1000 tracks.
    p.add_argument("--student-arch", choices=["linear", "mlp"], default="linear",
                   help="linear: V18 spec G1 (Linear(512→1)); "
                        "mlp: 2-layer (Linear(512→hidden)→GELU→Dropout→Linear(hidden→1))")
    p.add_argument("--hidden-dim", type=int, default=128,
                   help="hidden width for --student-arch mlp")
    p.add_argument("--dropout", type=float, default=0.3,
                   help="dropout in mlp hidden layer")
    return p.parse_args()


def main(args) -> int:
    import torch
    import torch.nn as nn

    torch.manual_seed(args.seed)
    np.random.seed(args.seed)
    torch.backends.cudnn.deterministic = True
    torch.backends.cudnn.benchmark = False
    device = "cuda" if torch.cuda.is_available() else "cpu"
    print(f"[student] device: {device}  (cudnn.deterministic=True)")

    # ── Load aligned arrays ────────────────────────────────────────────
    e = np.load(args.audio_emb, allow_pickle=True)
    audio_tids = e["track_ids"].astype(np.int64)
    audio_arr = e["embeddings"].astype(np.float32)
    audio_tid_to_i = {int(t): i for i, t in enumerate(audio_tids)}

    tp = np.load(args.teacher_preds, allow_pickle=True)
    teacher_tids = tp["track_ids"].astype(np.int64)
    teacher_int = tp["teacher_intensity"].astype(np.float32)
    teacher_pen = tp["teacher_penultimate"].astype(np.float32)
    teacher_split = tp["split"]
    teacher_tid_to_i = {int(t): i for i, t in enumerate(teacher_tids)}

    cs = np.load(args.consensus, allow_pickle=True)
    cs_tids = cs["track_ids"].astype(np.int64)
    cs_arr = cs["consensus_intensity"].astype(np.float32)
    cs_tid_to_i = {int(t): i for i, t in enumerate(cs_tids)}

    # Track IDs that have all three; should be the same as the teacher run
    common = (set(int(t) for t in audio_tids)
              & set(int(t) for t in teacher_tids)
              & set(int(t) for t in cs_tids))
    track_ids = sorted(common)
    N = len(track_ids)
    print(f"[student] aligned {N} tracks")

    A = audio_arr.shape[1]
    pen_dim = teacher_pen.shape[1]
    print(f"[student] audio_dim={A}  teacher_penultimate_dim={pen_dim}")

    X = np.zeros((N, A), dtype=np.float32)
    t_int = np.zeros(N, dtype=np.float32)
    t_pen = np.zeros((N, pen_dim), dtype=np.float32)
    y_cons = np.zeros(N, dtype=np.float32)
    splits = np.empty(N, dtype=object)
    for i, tid in enumerate(track_ids):
        X[i] = audio_arr[audio_tid_to_i[tid]]
        ti = teacher_tid_to_i[tid]
        t_int[i] = teacher_int[ti]
        t_pen[i] = teacher_pen[ti]
        y_cons[i] = cs_arr[cs_tid_to_i[tid]]
        splits[i] = str(teacher_split[ti])

    train_idx = np.where(splits == "train")[0]
    val_idx   = np.where(splits == "val")[0]
    test_idx  = np.where(splits == "test")[0]
    print(f"[student] splits: train={len(train_idx)} val={len(val_idx)} "
          f"test={len(test_idx)}")

    # ── Build student + FitNets adapter ────────────────────────────────
    class Student(nn.Module):
        """Student over the audio embedding only.

        Two architectures:

          - 'linear' (V18 spec G1): a single Linear(in_dim → 1) for the
            intensity head. Deployable as `audio_emb @ vec + bias`.

          - 'mlp' (spec §765-768 escalation, 2026-05-08 V18.1): a 2-layer
            MLP `Linear(in_dim → hidden) → GELU → Dropout → Linear(hidden → 1)`.
            CPU cost still <0.05 ms/track at hidden=128 (well under the G9
            100 ms/1000-track budget). Closes part of the G6 distillation gap
            by giving the student capacity to learn nonlinear interactions
            in the audio embedding (e.g. AND-gates between
            high-frequency-energy and low-pitch features).

        In both cases we also have a training-time `pen_proj` that maps the
        audio embedding into the teacher's penultimate space for FitNets
        feature matching. It is DISCARDED at export.
        """
        def __init__(self, in_dim, pen_dim, arch, hidden, dropout):
            super().__init__()
            self.arch = arch
            if arch == "linear":
                self.intensity = nn.Linear(in_dim, 1)
            elif arch == "mlp":
                self.intensity = nn.Sequential(
                    nn.Linear(in_dim, hidden),
                    nn.GELU(),
                    nn.Dropout(dropout),
                    nn.Linear(hidden, 1),
                )
            else:
                raise ValueError(f"unknown arch {arch!r}")
            self.pen_proj  = nn.Linear(in_dim, pen_dim)   # training-only

        def forward(self, x):
            return self.intensity(x).squeeze(-1), self.pen_proj(x)

    model = Student(A, pen_dim, args.student_arch, args.hidden_dim, args.dropout).to(device)
    n_params = sum(p.numel() for p in model.intensity.parameters())
    print(f"[student] arch={args.student_arch}  intensity-head params={n_params:_}")
    if args.student_arch == "mlp":
        print(f"[student]   hidden_dim={args.hidden_dim}  dropout={args.dropout}")
    opt = torch.optim.AdamW(model.parameters(), lr=args.lr, weight_decay=args.weight_decay)

    Xt = torch.from_numpy(X).to(device)
    tit = torch.from_numpy(t_int).to(device)
    tpen = torch.from_numpy(t_pen).to(device)
    yt = torch.from_numpy(y_cons).to(device)
    train_t = torch.from_numpy(train_idx).to(device)
    val_t = torch.from_numpy(val_idx).to(device)

    T = args.temperature

    def label_smooth_mse(pred, target, eps):
        # Tiny noise injection; "label smoothing" for a regression target
        # = additive uniform noise over a small interval.
        noise = (torch.rand_like(target) * 2 - 1) * eps
        return nn.functional.mse_loss(pred, target + noise)

    def soft_target_kl(student_out, teacher_out, T):
        """KL divergence over a 1d "score → distribution" view.

        For scalar targets, the standard Hinton KD trick is to treat the
        scalar as the location of a 1-d Gaussian or, more practically, to
        compare *two soft scalars* via a temperature-scaled MSE. We
        implement the latter: T-softened scalars compared via MSE × T².
        """
        s_soft = student_out / T
        t_soft = teacher_out / T
        return nn.functional.mse_loss(s_soft, t_soft) * (T * T)

    best_val = float("inf"); best_epoch = -1; best_state = None
    history = []
    t0 = time.time()
    for epoch in range(args.epochs):
        model.train()
        perm = torch.randperm(len(train_idx), device=device)
        train_loss = 0.0; nb = 0
        for k in range(0, len(perm), args.batch_size):
            idx = train_t[perm[k:k+args.batch_size]]
            s_int, s_pen = model(Xt[idx])
            l_out = nn.functional.mse_loss(s_int, tit[idx])
            l_fit = nn.functional.mse_loss(s_pen, tpen[idx])
            l_kd  = soft_target_kl(s_int, tit[idx], T)
            l_ls  = label_smooth_mse(s_int, yt[idx], args.label_smooth)
            loss = (args.lambda_out * l_out
                    + args.lambda_fit * l_fit
                    + args.lambda_kd  * l_kd
                    + args.lambda_ls  * l_ls)
            opt.zero_grad(); loss.backward(); opt.step()
            train_loss += loss.item(); nb += 1
        train_loss /= max(nb, 1)

        model.eval()
        with torch.no_grad():
            s_int_v, _ = model(Xt[val_t])
            v_loss = nn.functional.mse_loss(s_int_v, tit[val_t]).item()
        history.append({"epoch": epoch, "train_loss": train_loss,
                        "val_student_vs_teacher_mse": v_loss})
        if v_loss < best_val:
            best_val = v_loss; best_epoch = epoch
            best_state = {k: v.detach().clone() for k, v in model.state_dict().items()}
        if epoch - best_epoch >= args.patience:
            print(f"[student] early stop at epoch {epoch} (best @ {best_epoch})")
            break
        if epoch % 5 == 0 or epoch == args.epochs - 1:
            print(f"  ep {epoch:>3d}  train={train_loss:.4f}  "
                  f"val_S↔T_mse={v_loss:.4f}  best@{best_epoch}")

    wall = time.time() - t0
    print(f"[student] trained in {wall:.1f}s; best val_S↔T_mse={best_val:.4f}")
    model.load_state_dict(best_state)
    model.eval()

    # ── Test PA on consensus ──────────────────────────────────────────
    def spearman(a, b):
        a, b = np.asarray(a), np.asarray(b); n = len(a)
        ra = np.argsort(np.argsort(a)); rb = np.argsort(np.argsort(b))
        return 1 - 6 * float(np.sum((ra - rb) ** 2)) / (n * (n*n - 1))

    def pa(s, y):
        n = len(s); ds = s[:, None] - s[None, :]; dy = y[:, None] - y[None, :]
        tri = np.triu(np.ones((n, n), dtype=bool), k=1)
        valid = tri & (ds != 0) & (dy != 0)
        return float((valid & ((ds > 0) == (dy > 0))).sum() / max(valid.sum(), 1))

    with torch.no_grad():
        s_int_all, _ = model(Xt)
    s_int_np = s_int_all.cpu().numpy()
    test_pa = pa(s_int_np[test_idx], y_cons[test_idx])
    test_rho = spearman(s_int_np[test_idx], y_cons[test_idx])
    teacher_test_pa = pa(t_int[test_idx], y_cons[test_idx])
    print(f"[student] TEST PA (vs consensus): student={test_pa:.4f} "
          f"teacher={teacher_test_pa:.4f}  gap={teacher_test_pa - test_pa:+.4f}")
    print(f"[student] TEST Spearman: {test_rho:.4f}")

    # ── Persist (keep both heads in checkpoint; export drops pen_proj) ─
    args.out_dir.mkdir(parents=True, exist_ok=True)
    import torch
    torch.save(model.state_dict(), args.out_dir / f"{args.out_stem}.pt")
    (args.out_dir / f"{args.out_stem}_metrics.json").write_text(json.dumps({
        "history": history,
        "best_epoch": best_epoch,
        "best_val_student_teacher_mse": best_val,
        "test_pa_consensus": test_pa,
        "test_spearman_consensus": test_rho,
        "test_pa_teacher_consensus": teacher_test_pa,
        "distillation_gap_pp": teacher_test_pa - test_pa,
        "arch": args.student_arch,
        "hidden_dim": args.hidden_dim if args.student_arch == "mlp" else None,
        "dropout": args.dropout if args.student_arch == "mlp" else None,
        "intensity_head_params": int(sum(p.numel() for p in model.intensity.parameters())),
        "loss_weights": {
            "out": args.lambda_out, "fit": args.lambda_fit,
            "kd": args.lambda_kd,   "ls": args.lambda_ls,
        },
        "temperature": T,
        "label_smoothing": args.label_smooth,
        "wall_seconds": wall,
    }, indent=2))
    print(f"[student] wrote {args.out_dir}/{args.out_stem}.pt + metrics")
    return 0


if __name__ == "__main__":
    sys.exit(main(parse_args()))
