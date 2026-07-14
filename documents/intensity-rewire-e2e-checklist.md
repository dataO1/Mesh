# Intensity rewire — e2e verification checklist

Branch `intensity-rewire` wires the V18.X axis (`V18_round7_7_lora_v2`) into live
suggestion ranking via analysis-time scalars (`intensity_score` relation).
Flow: local e2e → push + CI release candidate → standalone OrangePi test.

## Already verified headless (2026-07-14)

- [x] Backfill: 909/909 tracks projected, 0 failed, ~11 s (`reanalyze_ml --only-missing`)
- [x] Stored scalars == `aggression_inspect` raw projections (min 0.196 / med 0.888 / p75 0.930; max = clamp(+1.0109))
- [x] `suggestions-smoke`: centre → same-key + matched percentile; full peak → EnergyBoost keys, percentiles at +0.15 target; full drop → EnergyCool keys, −0.15 target
- [x] Unit tests: scalar roundtrip, delete cleanup, `metadata_differs` version triggers
- [x] Workspace suite green (pre-existing unrelated `test_gain_clamping` failure only)

## 1. Local e2e (laptop)

### mesh-cue
- [x] Launch mesh-cue. Expect log: `[STARTUP] Intensity scalars up to date` (backfill
      already ran headless; a fresh library would show `backfilled: N tracks`).
      ✅ confirmed 2026-07-14.
      (Note: there is deliberately NO intensity column in the browser track table —
      the scalar feeds scoring, the graph "Intens" component column, and opener
      ranking only.)
- [ ] Graph tab: energy slider at peak/drop visibly shifts suggestion intensity
      percentiles ("Intens" column) — compare against `suggestions-smoke` output.
- [ ] `db-inspect` shows `intensity_score: 909 rows`, single axis version.

### USB export (use the PRE-EXISTING stick first — that's the backfill test)
- [ ] Export to a stick last exported BEFORE this change. Sync plan should classify
      all its tracks as metadata-only updates (`metadata_differs: intensity scalar
      differs … (Some("V18_round7_7_lora_v2") vs None)` in debug log; plan summary
      "N to update", no WAV copies).
- [ ] After export: `db-inspect /path/to/stick/mesh-collection` → `intensity_score`
      row count == stick track count, axis `V18_round7_7_lora_v2`.
- [ ] Re-export immediately → plan shows nothing to update (version match, no churn).

### mesh-player (laptop)
- [ ] Local collection: SUGGEST on, energy slider moves suggestions' intensity
      (verify against known aggressive/liquid tracks).
- [ ] With USB mounted: suggestions from both sources; log shows
      `[INTENSITY] Source 'Local': 909 stored intensity scalars` + the stick's count,
      and NO `candidate(s) have no intensity scalar` warnings.
- [ ] Opener mode (no deck playing): openers rank sanely with the intensity slider.
- [ ] While playing: judge whether the slider's throw feels right — Medium reach
      targets only seed±0.15 percentile at full slider (halved constants in
      `config.rs::intensity_reach`, tuned while the component was inert).
      Options if too timid: restore 0.15/0.30/0.50, enlarge Open only, or blend
      toward absolute intensity at slider extremes.

## 2. Push + CI release candidate

- [ ] Commit `intensity-rewire`, merge/push per usual flow, `cargo release rc`.
- [ ] CI builds .deb/.zip/SD image with the embedded V18.X axis (unchanged asset).

## 3. Standalone OrangePi (from the RC build)

- [ ] Flash/update the player to the RC.
- [ ] Insert the re-exported stick. Suggestions rank with intensity live from the
      stick's own `intensity_score` rows (device needs no 1024-d vectors, no ONNX).
- [ ] Energy slider behaviour matches the laptop (same pooled-percentile math,
      same scalars — scores should be identical for the same seed/settings).
- [ ] Stale-stick degradation check (optional): insert a stick that was NOT
      re-exported → player logs `candidate(s) have no intensity scalar → neutral 0.5`
      and suggestions still work (intensity component neutral).

## Rollback

Working tree only until commit; after RC: previous release + old sticks keep
working (relation is additive; old player binaries ignore `intensity_score`).
