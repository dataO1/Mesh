# HTDemucs ONNX Implementation Notes

This document captures findings from investigating why our ONNX-based stem separation produces lower quality results than UVR5.

## Root Cause: Missing Frequency Branch

**The HTDemucs model is a HYBRID architecture with TWO output branches:**

1. **Time Branch** (`add_67`) - Direct waveform output, good for transients
2. **Frequency Branch** (`output`) - Masked spectrogram that needs ISTFT conversion

**We were only using the time branch, completely ignoring the frequency branch.**

This explains the symptoms:
- ✅ Drum transients work well (time branch handles transients)
- ❌ Vocals poorly separated (frequency branch handles tonal content)
- ❌ Hi-hats missing from drums (frequency branch handles high frequencies)
- ❌ Content bleeding into "other" stem

## Correct Processing Pipeline

Based on [sevagh/demucs.onnx](https://github.com/sevagh/demucs.onnx) reference implementation:

```
Input Audio
    │
    ├──► STFT ──► Magnitude Spectrogram ──┐
    │                                      │
    └──► Raw Waveform ────────────────────┼──► ONNX Model
                                          │         │
                                          │         ├──► Frequency Output (masked spectrogram)
                                          │         │         │
                                          │         │         ▼
                                          │         │    ISTFT ──► Freq Waveform
                                          │         │                   │
                                          │         └──► Time Output ───┤
                                          │              (waveform)     │
                                          │                             ▼
                                          └────────────────────► SUM ──► Final Stems
```

## STFT Parameters (HTDemucs)

| Parameter | Value | Notes |
|-----------|-------|-------|
| n_fft | 4096 | FFT window size |
| hop_length | 1024 | n_fft / 4 |
| window | Hann (periodic) | `0.5 * (1 - cos(2πn/N))` |
| normalized | True | Scale by `1/√n_fft` |
| center | True | Pad by n_fft/2 on each side |
| Segment samples | 343980 | ~7.8 seconds at 44.1kHz |
| STFT frames | 336 | After cropping first 2 frames |
| Frequency bins | 2048 | n_fft/2 (excludes Nyquist) |

## Complex-as-Channels (CaC) Format

The model uses CaC format where complex spectrograms are stored as real tensors:
- Input shape: `[batch, channels*2, freq_bins, frames]` = `[1, 4, 2048, 336]`
- Output shape: `[batch, stems*channels*2, freq_bins, frames]` = `[1, 16, 2048, 336]`

Layout for output (per stem):
```
Channel 0: Left Real
Channel 1: Left Imaginary
Channel 2: Right Real
Channel 3: Right Imaginary
```

## ISTFT Implementation

From sevagh's C++ implementation:

1. **Extract complex numbers** from CaC format:
   ```cpp
   z_target(ch, freq, frame+2) = complex(real_data[idx], imag_data[idx]);
   ```

2. **Zero boundary bins** (first 2, last 2 frequency bins):
   ```cpp
   z_target(ch, 0..2, frame) = 0;
   z_target(ch, -2..-1, frame) = 0;
   ```

3. **Apply inverse FFT** with window:
   ```cpp
   ifft(complex_spec) * window * sqrt(n_fft)
   ```

4. **Overlap-add reconstruction**:
   ```cpp
   output[frame * hop + i] += windowed_frame[i]
   ```

5. **Normalize** by accumulated window weights

## Standalone Functions (from demucs-for-onnx)

These functions were extracted from HTDemucs class methods for ONNX export:

### `standalone_spec(x, nfft=4096, hop_length=1024)`
```python
le = ceil(x.shape[-1] / hop_length)
pad = hop_length // 2 * 3  # 1536
x = pad1d(x, (pad, pad + le * hop - x.shape[-1]), mode="reflect")
z = spectro(x, nfft, hop_length)[..., :-1, :]  # Remove Nyquist
z = z[..., 2: 2 + le]  # Crop first 2 frames
return z
```

### `standalone_magnitude(z, cac=True)`
```python
# Convert complex to real channels
m = torch.view_as_real(z).permute(0, 1, 4, 2, 3)
m = m.reshape(B, C * 2, Fr, T)
return m
```

### `standalone_ispec(z, length, hop_length=1024)`
```python
z = F.pad(z, (0, 0, 0, 1))  # Add back Nyquist bin
z = F.pad(z, (2, 2))        # Add back cropped frames
pad = hop_length // 2 * 3
le = hop_length * ceil(length / hop_length) + 2 * pad
x = ispectro(z, hop_length, length=le)
x = x[..., pad: pad + length]
return x
```

## Normalization

**Important:** No external z-score normalization is needed for ONNX inference.

The model normalizes internally:
- Time branch: Instance normalization on waveform
- Frequency branch: Instance normalization on spectrogram

```python
# Inside model forward():
mean = x.mean(dim=(1, 2, 3), keepdim=True)
std = x.std(dim=(1, 2, 3), keepdim=True)
x = (x - mean) / (1e-5 + std)
```

## UVR5 vs Our Implementation

| Feature | UVR5 | Our Current | Needed |
|---------|------|-------------|--------|
| Model format | PyTorch | ONNX | OK |
| Time branch | ✅ | ✅ | ✅ |
| Frequency branch | ✅ | ✅ | ✅ |
| Shift augmentation | ✅ | ❌ | Optional |
| Ensemble mode | ✅ | ❌ | Optional |

## Implementation Status

1. [x] Document findings
2. [x] Implement `combine_hybrid_outputs()` function
3. [x] Implement ISTFT for frequency branch
4. [x] Extract both outputs from ONNX model
5. [x] Sum time and frequency waveforms
6. [x] Per-stem time/freq branch weighting
7. [ ] Shift augmentation (optional)
8. [ ] Test separation quality

## Per-Stem Branch Weighting

Different stems benefit from different branch combinations:

| Stem | Time Weight | Freq Weight | Rationale |
|------|-------------|-------------|-----------|
| Drums | 1.0 | 1.0 | Transients need time branch |
| Bass | 0.0 | 1.0 | Frequency-only reduces drum bleed |
| Other | 0.3 | 1.0 | Mostly tonal content |
| Vocals | 0.0 | 1.0 | Frequency-only for cleaner extraction |

## Shift Augmentation (The Shift Trick)

Reference: [facebookresearch/demucs](https://github.com/facebookresearch/demucs) apply.py

The shift trick improves separation quality by ~0.2 SDR points:

1. **Max shift**: 0.5 seconds (22050 samples at 44.1kHz)
2. **Pad input**: Add `2 * max_shift` padding
3. **For each shift**:
   - Generate random offset in `[0, max_shift]`
   - Shift input by offset
   - Run inference
   - Shift output back by `-offset`
   - Accumulate result
4. **Average**: Divide accumulated output by number of shifts

```python
max_shift = int(0.5 * model.samplerate)  # 22050 samples
padded_mix = mix.padded(length + 2 * max_shift)
out = 0.
for _ in range(shifts):
    offset = random.randint(0, max_shift)
    shifted = padded_mix[..., offset:offset + length]
    shifted_out = apply_model(model, shifted)
    out += shifted_out[..., max_shift - offset:]
out /= shifts
```

Trade-off: Makes inference `shifts` times slower. Recommended values: 1 (none), 2, or 5.

### Implementation Notes

**Critical: Output Realignment**

When shifting the input, the output is also shifted by the same amount. Before averaging,
outputs must be realigned to a common reference point:

```rust
// Skip samples to align with center position (offset = MAX_SHIFT)
let align_skip = MAX_SHIFT - offset;
shift_accum[i] += combined_stems[align_skip + i];
```

Without this realignment, you get a "canon" effect with delayed copies stacked on top.

**Quality Assessment (as of 2024-01)**

- Shift augmentation with 2-5 shifts provides marginal improvement
- Processing time increases linearly with number of shifts
- For most use cases, shifts=1 (disabled) is recommended
- May be more beneficial for specific content types (needs further testing)

## Critical ISTFT Details

### Boundary Bin Zeroing
The frequency output from the model has bins 0-2047. Before ISTFT reconstruction,
the following bins must be zeroed to prevent artifacts:
- Bins 0, 1 (DC and near-DC)
- Bins 2046, 2047 (near-Nyquist)
- Bin 2048 (Nyquist, added for IFFT)

### Window Sum Normalization
With 75% overlap (hop = n_fft/4) and Hann window, the sum of squared windows
converges to ~1.5 in steady state. To prevent edge artifacts:
- Use a minimum threshold based on expected window sum
- At edges with partial coverage, scale down rather than amplifying noise

## Alternative Backends

### charon-audio

[charon-audio](https://crates.io/crates/charon-audio) is a pure-Rust audio separation library using ONNX Runtime
or Candle backends.

**⚠️ Status: NOT USABLE (as of v0.1.0)**

Investigation revealed that charon-audio v0.1.0 has **placeholder inference** - it returns copies
of the input instead of performing actual separation:

```rust
// From charon-audio/src/models.rs
pub fn infer(&self, input: &Array2<f32>) -> Result<Vec<Array2<f32>>> {
    // Placeholder: return copies of input as "separated" sources
    let separated = vec![input.clone(); num_sources];
    Ok(separated)
}
```

**Infrastructure is in place:**
- Dependency added with `--features charon-backend`
- `CharonBackend` implementation ready
- Backend selection UI works
- Uses patched `graph_builder` via [PR #139](https://github.com/neo4j-labs/graph/pull/139) for rayon 1.10+

**Waiting for:** charon-audio to implement actual ONNX/Candle inference.
Monitor releases at https://crates.io/crates/charon-audio

## References

- [sevagh/demucs.onnx](https://github.com/sevagh/demucs.onnx) - C++ ONNX implementation
- [Mixxx GSoC 2025](https://mixxx.org/news/2025-10-27-gsoc2025-demucs-to-onnx-dhunstack/) - Achieved <0.01 dB difference
- [facebookresearch/demucs](https://github.com/facebookresearch/demucs) - Original PyTorch model
- [UVR5](https://github.com/Anjok07/ultimatevocalremovergui) - Reference for quality comparison
- [charon-audio](https://docs.rs/charon-audio) - Pure Rust separation library (placeholder inference in v0.1.0)
