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
6. [ ] Test separation quality

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

## References

- [sevagh/demucs.onnx](https://github.com/sevagh/demucs.onnx) - C++ ONNX implementation
- [Mixxx GSoC 2025](https://mixxx.org/news/2025-10-27-gsoc2025-demucs-to-onnx-dhunstack/) - Achieved <0.01 dB difference
- [facebookresearch/demucs](https://github.com/facebookresearch/demucs) - Original PyTorch model
- [UVR5](https://github.com/Anjok07/ultimatevocalremovergui) - Reference for quality comparison
