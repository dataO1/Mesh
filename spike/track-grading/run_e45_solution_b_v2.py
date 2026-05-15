#!/usr/bin/env python3
"""
Solution B-v2: Warmup distillation — LoRA teacher first, then consensus teacher.

Phase 1: distill student against LoRA scoring head (λ_fit=0) for warmup epochs.
Phase 2: continue distillation against consensus teacher for remaining epochs.

The hypothesis: the student first learns the easy audio-only geometry from the
LoRA head, then refines with the high-ceiling consensus signal.
"""
from __future__ import annotations

import sys, os, argparse, copy, time, math
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn

PROJECT = Path(__file__).resolve().parent.parent.parent
BASE = Path("/home/data01/Music/mesh-track-grading")

AUDIO_EMB  = BASE / "embeddings/corpus_muq_mulan_lora.npz"
AUDIO_KEY  = "embeddings_1024"
LORA_PREDS = BASE / "round7_7_lora_teacher_preds.npz"
CONSENSUS_PREDS = BASE / "round7_6_teacher_preds.npz"
CONSENSUS  = BASE / "round7_6_consensus.npz"
SPLIT      = BASE / "round7_6_split.npz"
OUT_DIR    = BASE


def parse_args():
    p = argparse.ArgumentParser(description="B-v2: Warmup distillation")
    p.add_argument("--hidden-dim", type=int, default=128)
    p.add_argument("--warmup-epochs", type=int, default=10)
    p.add_argument("--total-epochs", type=int, default=50)
    return p.parse_args()


def load_data():
    """Return (X, tit_warmup, tpen_warmup, tit_cons, tpen_cons, y_cons, train_idx, val_idx, A_dim, pen_dim)."""
    e = np.load(AUDIO_EMB, allow_pickle=True)
    X = e["embeddings_1024"].astype(np.float32)
    e_tids = e["track_ids"].astype(np.int64)

    # Warmup teacher (LoRA head)
    w = np.load(LORA_PREDS, allow_pickle=True)
    w_map = {int(t): i for i, t in enumerate(w["track_ids"])}
    w_int = w["teacher_intensity"].astype(np.float32)
    w_pen = w["teacher_penultimate"].astype(np.float32)

    # Consensus teacher
    c = np.load(CONSENSUS_PREDS, allow_pickle=True)
    c_map = {int(t): i for i, t in enumerate(c["track_ids"])}
    c_int = c["teacher_intensity"].astype(np.float32)
    c_pen = c["teacher_penultimate"].astype(np.float32)

    # Consensus labels + split
    cons = np.load(CONSENSUS, allow_pickle=True)
    cons_map = {int(t): cons["consensus_intensity"][i] for i, t in enumerate(cons["track_ids"])}

    s = np.load(SPLIT, allow_pickle=True)
    split_map = {int(t): str(lbl) for t, lbl in zip(s["track_ids"], s["split"])}

    # Align
    common = sorted(set(int(t) for t in e_tids) & set(w_map) & set(c_map) & set(cons_map) & set(split_map))
    e_idx = {int(t): i for i, t in enumerate(e_tids)}

    X_list, tit_w_list, tpen_w_list, tit_c_list, tpen_c_list, y_list = [], [], [], [], [], []
    for t in common:
        ei = e_idx[t]
        X_list.append(X[ei])
        tit_w_list.append(w_int[w_map[t]])
        tpen_w_list.append(w_pen[w_map[t]])
        tit_c_list.append(c_int[c_map[t]])
        tpen_c_list.append(c_pen[c_map[t]])
        y_list.append(cons_map[t])

    X = np.stack(X_list)
    tit_w = np.array(tit_w_list, dtype=np.float32)
    tpen_w = np.array(tpen_w_list, dtype=np.float32)
    tit_c = np.array(tit_c_list, dtype=np.float32)
    tpen_c = np.array(tpen_c_list, dtype=np.float32)
    y_cons = np.array(y_list, dtype=np.float32)

    splits = np.array([split_map[t] for t in common])
    train_idx = np.where(splits == "train")[0]
    val_idx   = np.where(splits == "val")[0]

    A_dim = X.shape[1]
    pen_dim = tpen_c.shape[1]
    print(f"[B-v2] {len(common)} tracks aligned, audio_dim={A_dim}, pen_dim={pen_dim}")
    return X, tit_w, tpen_w, tit_c, tpen_c, y_cons, train_idx, val_idx, A_dim, pen_dim


class Student(nn.Module):
    def __init__(self, in_dim, pen_dim, hidden):
        super().__init__()
        self.intensity = nn.Sequential(
            nn.Linear(in_dim, hidden), nn.GELU(), nn.Dropout(0.3),
            nn.Linear(hidden, 1),
        )
        self.pen_proj = nn.Linear(in_dim, pen_dim)

    def forward(self, x):
        return self.intensity(x).squeeze(-1), self.pen_proj(x)


def distill_phase(model, opt, Xt, tit, tpen, yt, train_t, val_t, epochs, device,
                  lambda_out=1.0, lambda_fit=0.5, lambda_kd=0.3, lambda_ls=0.2,
                  T=2.0, label_smooth=0.05, batch_size=512, patience=10):
    """Run distillation for `epochs`, return best model state."""
    def soft_target_kl(s_out, t_out, T):
        return nn.functional.mse_loss(s_out / T, t_out / T) * (T * T)

    best_val = float("inf"); best_state = None
    epochs_no_imp = 0

    for ep in range(epochs):
        model.train()
        perm = torch.randperm(len(train_t), device=device)
        for k in range(0, len(perm), batch_size):
            idx = train_t[perm[k:k+batch_size]]
            s_int, s_pen = model(Xt[idx])
            l_out = nn.functional.mse_loss(s_int, tit[idx])
            l_fit = (nn.functional.mse_loss(s_pen, tpen[idx])
                     if s_pen.shape[-1] == tpen.shape[-1] else torch.tensor(0.0, device=device))
            l_kd  = soft_target_kl(s_int, tit[idx], T)
            l_ls  = nn.functional.mse_loss(s_int, yt[idx] + (torch.rand_like(yt[idx])*2-1)*label_smooth)
            loss = lambda_out*l_out + lambda_fit*l_fit + lambda_kd*l_kd + lambda_ls*l_ls
            opt.zero_grad(); loss.backward(); opt.step()

        model.eval()
        with torch.no_grad():
            s_int_v, _ = model(Xt[val_t])
            v_loss = nn.functional.mse_loss(s_int_v, tit[val_t]).item()

        if v_loss < best_val:
            best_val = v_loss; best_state = copy.deepcopy(model.state_dict())
            epochs_no_imp = 0
        else:
            epochs_no_imp += 1
            if epochs_no_imp >= patience:
                print(f"  [phase] early stop at epoch {ep} (best val_S↔T_mse={best_val:.4f})")
                break

    model.load_state_dict(best_state)
    return best_val


def main():
    args = parse_args()
    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    torch.manual_seed(42)

    X, tit_w, tpen_w, tit_c, tpen_c, y_cons, train_idx, val_idx, A_dim, pen_dim = load_data()

    model = Student(A_dim, pen_dim, args.hidden_dim).to(device)
    opt = torch.optim.AdamW(model.parameters(), lr=1e-3, weight_decay=1e-4)

    Xt = torch.from_numpy(X).to(device)
    tit_w_t = torch.from_numpy(tit_w).to(device)
    tpen_w_t = torch.from_numpy(tpen_w).to(device)
    tit_c_t = torch.from_numpy(tit_c).to(device)
    tpen_c_t = torch.from_numpy(tpen_c).to(device)
    yt = torch.from_numpy(y_cons).to(device)
    train_t = torch.from_numpy(train_idx).to(device)
    val_t = torch.from_numpy(val_idx).to(device)

    # ── Phase 1: Warmup with LoRA teacher ──────────────────────────────
    print(f"\n[B-v2] Phase 1: LoRA teacher warmup ({args.warmup_epochs} epochs, λ_fit=0)")
    d1 = distill_phase(model, opt, Xt, tit_w_t, tpen_w_t, yt, train_t, val_t,
                        args.warmup_epochs, device, lambda_fit=0)
    print(f"[B-v2] Phase 1 done — val_S↔T_mse={d1:.4f}")

    # ── Phase 2: Refine with consensus teacher ─────────────────────────
    remaining = args.total_epochs - args.warmup_epochs
    print(f"[B-v2] Phase 2: Consensus teacher ({remaining} epochs, λ_fit=0.5)")
    d2 = distill_phase(model, opt, Xt, tit_c_t, tpen_c_t, yt, train_t, val_t,
                        remaining, device, lambda_fit=0.5)
    print(f"[B-v2] Phase 2 done — val_S↔T_mse={d2:.4f}")

    # ── Evaluate on test split ─────────────────────────────────────────
    model.eval()
    with torch.no_grad():
        s_all = model.intensity(Xt).squeeze(-1).cpu().numpy()

    from bt_pair_sampler import spearman_rho as srho

    s = np.load(SPLIT, allow_pickle=True)
    sp_map = {int(t): str(lbl) for t, lbl in zip(s["track_ids"], s["split"])}
    emb_map = {int(t): i for i, t in enumerate(np.load(AUDIO_EMB, allow_pickle=True)["track_ids"])}
    cons_data = np.load(CONSENSUS, allow_pickle=True)
    cons_map_all = {int(t): cons_data["consensus_intensity"][i] for i, t in enumerate(cons_data["track_ids"])}

    common_tids = sorted(set(emb_map) & set(cons_map_all) & set(sp_map))
    test_tids = [t for t in common_tids if sp_map[t] == "test"]

    scores_test = np.array([s_all[emb_map[t]] for t in test_tids], dtype=np.float32)
    cons_test   = np.array([cons_map_all[t] for t in test_tids], dtype=np.float32)

    agreement = np.mean((scores_test[1:] - scores_test[:-1]) * (cons_test[1:] - cons_test[:-1]) > 0)
    rho = srho(scores_test, cons_test)
    print(f"\n[B-v2] TEST PA (vs consensus): {agreement:.4f}  Spearman ρ={rho:+.4f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
