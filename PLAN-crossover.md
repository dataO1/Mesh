# Crossover Band Splitting Implementation Plan

## Problem Statement

Currently, the multiband container processes full audio through all bands - there's no actual frequency splitting. Adding an effect to a mid band still processes the full audio spectrum, not just the mid frequencies.

## Research Summary

### Option 1: LSP CLAP Crossover Plugin
**Source**: [LSP Plugins Project](https://lsp-plug.in/) | [CLAP Database](https://clapdb.tech/software/220/)

| Pros | Cons |
|------|------|
| Production-ready, high quality | External dependency (user must install) |
| Multiple filter slopes (LR12, LR24, LR48) | Complex routing (multiple outputs → bands) |
| Linear-phase mode available | CLAP multi-output handling needed |
| Already works with mesh's CLAP host | Not embedded in the app |

### Option 2: NIH-plug Crossover (Robbert van der Helm)
**Source**: [GitHub - nih-plug/crossover](https://github.com/robbert-vdh/nih-plug/tree/master/plugins/crossover)

| Pros | Cons |
|------|------|
| Written in pure Rust, ISC license | Part of larger framework |
| Linkwitz-Riley 24dB/oct IIR | Would need extraction/adaptation |
| FIR linear-phase option | Adds nih-plug dependency or manual port |
| 2-5 band splitting | |

### Option 3: Native Implementation (Extend existing SvfFilter)
**Source**: `crates/mesh-core/src/effect/native/filter.rs`

| Pros | Cons |
|------|------|
| Already have 12dB SVF filter | Need to write crossover logic |
| No external dependencies | Only 12dB slope (need cascade for 24dB) |
| Tight integration possible | Testing/validation needed |
| Full control | |

### Option 4: Rust DSP Crates
**Sources**: [fundsp](https://crates.io/crates/fundsp), [biquad](https://github.com/korken89/biquad-rs)

| Pros | Cons |
|------|------|
| Tested implementations | No dedicated crossover crate |
| fundsp has Butterworth | Would need manual filter combination |
| biquad has Q_BUTTERWORTH | |

---

## Recommended Approach: Hybrid

### Phase 1: CLAP Plugin Support (Quick Win)
Use LSP Crossover or any CLAP crossover plugin as the crossover effect.

**Changes needed:**
1. Add `crossover_effect: Option<Box<dyn Effect>>` to `MultibandHost`
2. Before processing bands, run audio through crossover
3. Capture crossover outputs and route to appropriate bands
4. UI: Add "Set Crossover" button in multiband editor

**Challenge**: CLAP plugins with multiple outputs need special handling. The crossover outputs need to be captured and routed to the correct band inputs.

### Phase 2: Native LR24 Crossover (Longer term)
Port the crossover logic from NIH-plug or build using existing SVF:

```
Linkwitz-Riley 24dB/oct = Two cascaded 12dB Butterworth filters

For N bands, need N-1 crossover points:
- Band 0 (low): LP1 → LP2 (cascaded)
- Band 1 (mid): HP1 → LP3 → HP2 → LP4
- Band N (high): HP(N-1) → HP(N) (cascaded)
```

**Rust pseudo-implementation:**
```rust
struct LR24Crossover {
    // Two SVF filters per crossover frequency
    low_filters: [SvfFilter; 2],
    high_filters: [SvfFilter; 2],
    crossover_freq: f32,
}

impl LR24Crossover {
    fn process(&mut self, input: StereoSample) -> (StereoSample, StereoSample) {
        // First 12dB stage
        let (low1, high1, _) = self.low_filters[0].process(input);
        // Second 12dB stage (cascade)
        let (low2, _, _) = self.low_filters[1].process(low1);
        let (_, high2, _) = self.high_filters[1].process(high1);
        (low2, high2) // LR24 split
    }
}
```

---

## Implementation Plan

### Step 1: Crossover Effect Slot in MultibandHost
- [ ] Add `crossover: Option<Box<dyn Effect>>` field
- [ ] Add `set_crossover_effect()` method
- [ ] Add `EngineCommand::SetMultibandCrossover`

### Step 2: Audio Routing with Crossover
- [ ] Modify `MultibandHost::process()` to:
  1. If crossover exists, process input through crossover
  2. Route crossover band outputs to corresponding band effect chains
  3. Sum processed bands back together
- [ ] Handle variable band counts (2-8 bands)

### Step 3: UI Integration
- [ ] Add "Crossover" section to multiband editor header
- [ ] Allow selecting crossover plugin (from CLAP browser)
- [ ] Display crossover frequencies
- [ ] Add frequency adjustment controls

### Step 4: (Future) Native LR24 Crossover
- [ ] Create `LinkwitzRileyCrossover` struct using existing SVF
- [ ] Support 2-8 way splitting
- [ ] Add as default crossover option

---

## References

- [Linkwitz-Riley Filter - Wikipedia](https://en.wikipedia.org/wiki/Linkwitz%E2%80%93Riley_filter)
- [Rane Note 160: Linkwitz-Riley Crossovers](https://www.ranecommercial.com/legacy/note160.html)
- [NIH-plug Crossover Source](https://github.com/robbert-vdh/nih-plug/tree/master/plugins/crossover)
- [LSP Crossover Plugin](https://lsp-plug.in/)
- [KVR: Crossover by Robbert van der Helm](https://www.kvraudio.com/product/crossover-by-robbert-van-der-helm)
