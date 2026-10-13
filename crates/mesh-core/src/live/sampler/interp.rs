//! Resampling interpolators for sampler voices.
//!
//! Three quality modes behind [`Interpolation`] so the board benchmark can pick
//! one per lane (design R2). All read from padded [`SampleData`] frames, so no
//! tap needs clamping.
//!
//! The 8-tap sinc mode is band-limited when pitching up: a set of kernels with
//! decreasing cutoff is precomputed, and each voice picks the kernel for its
//! current playback rate once per render segment. With only 8 taps the stop
//! band of the narrow kernels is shallow, so this reduces aliasing rather than
//! removing it.
//!
//! [`SampleData`]: super::SampleData

use std::sync::OnceLock;

use crate::types::StereoSample;

/// Interpolation quality for a lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Interpolation {
    /// 2-point linear. Cheapest, audible aliasing and HF loss.
    Linear,
    /// 4-point, 3rd-order Hermite.
    Hermite,
    /// 8-tap Blackman-windowed sinc, 256 phases with linear phase blending,
    /// band-limited when pitching up.
    #[default]
    Sinc8,
}

pub(crate) const SINC_TAPS: usize = 8;
/// Taps before the read position (the read frame itself is tap `SINC_HALF`).
const SINC_HALF: usize = 3;
const SINC_PHASES: usize = 256;
const SINC_ROW: usize = SINC_TAPS;
const SINC_BAND_LEN: usize = (SINC_PHASES + 1) * SINC_ROW;
/// Upper playback-rate bound of each kernel band. Rates above the last entry
/// reuse the last (narrowest) kernel.
const SINC_BAND_RATES: [f64; 9] = [1.0, 1.19, 1.41, 1.68, 2.0, 2.38, 2.83, 3.36, 4.0];
/// Cutoff of the rate-1 kernel as a fraction of Nyquist. Below 1 to keep the
/// short kernel's transition band out of the audible top octave's images.
const SINC_BASE_CUTOFF: f64 = 0.92;

/// Precomputed sinc kernel bands. Built once, off the RT thread.
pub struct SincTables {
    coeffs: Box<[f32]>,
}

static SINC_TABLES: OnceLock<SincTables> = OnceLock::new();

/// Global kernel tables. The first call builds them (allocates), so call it
/// from a non-RT thread first; [`super::Sampler::new`] does.
pub fn sinc_tables() -> &'static SincTables {
    SINC_TABLES.get_or_init(SincTables::build)
}

impl SincTables {
    fn build() -> Self {
        let mut coeffs = vec![0.0f32; SINC_BAND_RATES.len() * SINC_BAND_LEN];
        for (b, &rate) in SINC_BAND_RATES.iter().enumerate() {
            let fc = SINC_BASE_CUTOFF / rate;
            for p in 0..=SINC_PHASES {
                let frac = p as f64 / SINC_PHASES as f64;
                let row = &mut coeffs[b * SINC_BAND_LEN + p * SINC_ROW..][..SINC_ROW];
                let mut sum = 0.0;
                let mut tmp = [0.0f64; SINC_TAPS];
                for (j, c) in tmp.iter_mut().enumerate() {
                    let x = j as f64 - SINC_HALF as f64 - frac;
                    *c = fc * sinc(fc * x) * blackman(x / (SINC_TAPS as f64 / 2.0));
                    sum += *c;
                }
                // Unity DC gain per phase.
                for (dst, c) in row.iter_mut().zip(tmp) {
                    *dst = (c / sum) as f32;
                }
            }
        }
        Self { coeffs: coeffs.into_boxed_slice() }
    }

    /// Kernel band for a playback rate (absolute value).
    #[inline]
    pub(crate) fn band(&self, rate: f64) -> &[f32] {
        let b = SINC_BAND_RATES.iter().position(|&r| rate <= r * 1.0001).unwrap_or(SINC_BAND_RATES.len() - 1);
        &self.coeffs[b * SINC_BAND_LEN..(b + 1) * SINC_BAND_LEN]
    }
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

/// Blackman window over t in [-1, 1].
fn blackman(t: f64) -> f64 {
    if t.abs() >= 1.0 {
        return 0.0;
    }
    let a = std::f64::consts::PI * t;
    0.42 + 0.5 * a.cos() + 0.08 * (2.0 * a).cos()
}

/// One interpolator. `i` is the padded index of the frame at or before the
/// read position, `frac` the fraction past it. `kernel` is the sinc band (only
/// used by [`Sinc8`]).
pub(crate) trait Interp {
    fn read(buf: &[StereoSample], i: usize, frac: f32, kernel: &[f32]) -> StereoSample;
}

pub(crate) struct Linear;
pub(crate) struct Hermite;
pub(crate) struct Sinc8;

impl Interp for Linear {
    #[inline(always)]
    fn read(buf: &[StereoSample], i: usize, frac: f32, _: &[f32]) -> StereoSample {
        let w: &[StereoSample; 2] = buf[i..i + 2].try_into().unwrap();
        StereoSample::new(
            w[0].left + (w[1].left - w[0].left) * frac,
            w[0].right + (w[1].right - w[0].right) * frac,
        )
    }
}

impl Interp for Hermite {
    #[inline(always)]
    fn read(buf: &[StereoSample], i: usize, f: f32, _: &[f32]) -> StereoSample {
        let w: &[StereoSample; 4] = buf[i - 1..i + 3].try_into().unwrap();
        #[inline(always)]
        fn h(ym1: f32, y0: f32, y1: f32, y2: f32, f: f32) -> f32 {
            let c1 = 0.5 * (y1 - ym1);
            let c2 = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
            let c3 = 0.5 * (y2 - ym1) + 1.5 * (y0 - y1);
            ((c3 * f + c2) * f + c1) * f + y0
        }
        StereoSample::new(
            h(w[0].left, w[1].left, w[2].left, w[3].left, f),
            h(w[0].right, w[1].right, w[2].right, w[3].right, f),
        )
    }
}

impl Interp for Sinc8 {
    #[inline(always)]
    fn read(buf: &[StereoSample], i: usize, frac: f32, kernel: &[f32]) -> StereoSample {
        let w: &[StereoSample; SINC_TAPS] = buf[i - SINC_HALF..i - SINC_HALF + SINC_TAPS].try_into().unwrap();
        let p = frac * SINC_PHASES as f32;
        let p0 = (p as usize).min(SINC_PHASES - 1);
        let t = p - p0 as f32;
        let rows: &[f32; 2 * SINC_ROW] = kernel[p0 * SINC_ROW..(p0 + 2) * SINC_ROW].try_into().unwrap();
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for j in 0..SINC_TAPS {
            let c = rows[j] + (rows[SINC_ROW + j] - rows[j]) * t;
            l += w[j].left * c;
            r += w[j].right * c;
        }
        StereoSample::new(l, r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sinc_rows_have_unity_dc_and_interpolate_at_phase_zero() {
        let t = sinc_tables();
        let band = t.band(1.0);
        for p in 0..=SINC_PHASES {
            let s: f32 = band[p * SINC_ROW..(p + 1) * SINC_ROW].iter().sum();
            assert!((s - 1.0).abs() < 1e-5);
        }
        // Phase 0 of the rate-1 kernel is close to an impulse at the read tap.
        assert!(band[SINC_HALF] > 0.9);
    }

    #[test]
    fn band_selection_narrows_with_rate() {
        let t = sinc_tables();
        // The read-tap weight at phase 0 falls as the cutoff drops.
        assert!(t.band(2.0)[SINC_HALF] < t.band(1.0)[SINC_HALF]);
        assert!(t.band(8.0)[SINC_HALF] <= t.band(4.0)[SINC_HALF]);
    }
}
