//! Stateless counter-based RNG (design §4.4).
//!
//! Generators keep no state, so randomness is a pure hash of the musical
//! coordinates it is drawn for: `(seed, lane, bar_in_phrase, step, stream)`.
//! The same coordinates always give the same value, on every platform, which
//! is what makes seeds reproducible across captures, re-renders after a key
//! change and fill variants.
//!
//! Everything here is integer arithmetic (plus exact integer → f32
//! conversions), `const` where possible, allocation-free and RT-safe.

/// Weyl-sequence increment (2^64 / φ) used by splitmix64.
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

/// splitmix64 finalizer (Stafford "Mix13" variant). Full avalanche: every
/// input bit flips each output bit with probability ~1/2.
#[inline]
pub const fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Hash the musical coordinates of one random draw into 64 random bits.
///
/// - `seed`: the slot seed.
/// - `lane`: lane index (keeps lanes sharing a seed independent).
/// - `bar_in_phrase`: 0..4.
/// - `step`: 16th step (or any sub-position the generator uses).
/// - `stream`: independent stream per decision (velocity, pitch, chop, …).
///
/// All coordinates are packed into one distinct 64-bit word before mixing, so
/// two different coordinate tuples never share an input.
#[inline]
pub const fn hash(seed: u32, lane: u8, bar_in_phrase: u8, step: u8, stream: u32) -> u64 {
    let coords = ((lane as u64) << 56)
        | ((bar_in_phrase as u64) << 48)
        | ((step as u64) << 40)
        | (stream as u64);
    let h = mix64((seed as u64).wrapping_add(GOLDEN));
    mix64(h ^ coords.wrapping_mul(GOLDEN).wrapping_add(GOLDEN))
}

/// Derive a further independent value from an existing hash (e.g. the n-th
/// note of a motif drawn at one step).
#[inline]
pub const fn derive(h: u64, n: u32) -> u64 {
    mix64(h ^ (n as u64).wrapping_add(1).wrapping_mul(GOLDEN))
}

/// Uniform f32 in `[0, 1)` from the top 24 bits (exactly representable).
#[inline]
pub fn unit_f32(h: u64) -> f32 {
    (h >> 40) as f32 * (1.0 / (1u32 << 24) as f32)
}

/// Uniform f32 in `[lo, hi)`.
#[inline]
pub fn range_f32(h: u64, lo: f32, hi: f32) -> f32 {
    lo + (hi - lo) * unit_f32(h)
}

/// Uniform integer in `[0, n)` (Lemire multiply-shift, no modulo bias worth
/// caring about at 32 bits). Returns 0 for `n == 0`.
#[inline]
pub const fn below(h: u64, n: u32) -> u32 {
    (((h >> 32) * n as u64) >> 32) as u32
}

/// Uniform integer in `[lo, hi]` (inclusive). Returns `lo` when `hi <= lo`.
#[inline]
pub const fn range_i32(h: u64, lo: i32, hi: i32) -> i32 {
    if hi <= lo {
        return lo;
    }
    let span = (hi as i64 - lo as i64 + 1) as u64;
    if span > u32::MAX as u64 {
        // Full i32 range: take the bits directly.
        return (h >> 32) as u32 as i32;
    }
    lo + below(h, span as u32) as i32
}

/// Bernoulli trial: `true` with probability `p` (clamped to `[0, 1]`).
#[inline]
pub fn chance(h: u64, p: f32) -> bool {
    unit_f32(h) < p
}

/// Pick an index with probability proportional to `weights[i]`.
///
/// Negative and non-finite weights count as 0. Returns `None` when the slice
/// is empty or all weights are 0.
pub fn weighted_pick(h: u64, weights: &[f32]) -> Option<usize> {
    let mut total = 0.0f32;
    for &w in weights {
        total += sanitize_weight(w);
    }
    if total <= 0.0 {
        return None;
    }
    let target = unit_f32(h) * total;
    let mut acc = 0.0f32;
    let mut last = None;
    for (i, &w) in weights.iter().enumerate() {
        let w = sanitize_weight(w);
        if w <= 0.0 {
            continue;
        }
        acc += w;
        last = Some(i);
        if target < acc {
            return Some(i);
        }
    }
    // Float rounding can leave `target` == `acc` at the end: last positive weight.
    last
}

#[inline]
fn sanitize_weight(w: f32) -> f32 {
    if w.is_finite() && w > 0.0 { w } else { 0.0 }
}

/// The fixed part of a draw's coordinates, so generator code can write
/// `key.at(step, STREAM_VEL)` instead of passing five arguments every time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RngKey {
    pub seed: u32,
    pub lane: u8,
    pub bar_in_phrase: u8,
}

impl RngKey {
    pub const fn new(seed: u32, lane: u8, bar_in_phrase: u8) -> Self {
        Self { seed, lane, bar_in_phrase }
    }

    /// 64 random bits for `(step, stream)`.
    #[inline]
    pub const fn at(&self, step: u8, stream: u32) -> u64 {
        hash(self.seed, self.lane, self.bar_in_phrase, step, stream)
    }

    #[inline]
    pub fn unit(&self, step: u8, stream: u32) -> f32 {
        unit_f32(self.at(step, stream))
    }

    #[inline]
    pub fn chance(&self, step: u8, stream: u32, p: f32) -> bool {
        chance(self.at(step, stream), p)
    }

    #[inline]
    pub fn pick(&self, step: u8, stream: u32, weights: &[f32]) -> Option<usize> {
        weighted_pick(self.at(step, stream), weights)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_pinned() {
        // Golden values: changing the hash silently changes every saved seed's
        // sound. Update only with a deliberate capture-format break.
        assert_eq!(hash(0, 0, 0, 0, 0), hash(0, 0, 0, 0, 0));
        let pinned = [
            hash(0, 0, 0, 0, 0),
            hash(1, 0, 0, 0, 0),
            hash(0xDEAD_BEEF, 3, 2, 15, 7),
        ];
        assert_eq!(pinned, GOLDEN_VALUES);
    }

    // Pinned from the first implementation; see `hash_is_pinned`.
    const GOLDEN_VALUES: [u64; 3] = [258863698125685209, 15979313559291895140, 13317677550331285075];

    #[test]
    fn mix64_known_vector() {
        // splitmix64 reference: first output for state 0 is mix64(GOLDEN).
        assert_eq!(mix64(GOLDEN), 0xE220_A839_7B1D_CDAF);
    }

    #[test]
    fn every_coordinate_changes_the_hash() {
        let base = hash(42, 1, 2, 3, 4);
        assert_ne!(base, hash(43, 1, 2, 3, 4));
        assert_ne!(base, hash(42, 2, 2, 3, 4));
        assert_ne!(base, hash(42, 1, 3, 3, 4));
        assert_ne!(base, hash(42, 1, 2, 4, 4));
        assert_ne!(base, hash(42, 1, 2, 3, 5));
    }

    #[test]
    fn no_collisions_over_a_phrase_grid() {
        let mut seen = std::collections::HashSet::new();
        for lane in 0..4u8 {
            for bar in 0..4u8 {
                for step in 0..16u8 {
                    for stream in 0..8u32 {
                        assert!(seen.insert(hash(7, lane, bar, step, stream)));
                    }
                }
            }
        }
    }

    #[test]
    fn avalanche_on_seed_bit() {
        // Flipping one input bit flips roughly half the output bits.
        let mut total = 0u32;
        let n = 256u32;
        for s in 0..n {
            let a = hash(s, 0, 0, 0, 0);
            let b = hash(s ^ 1, 0, 0, 0, 0);
            total += (a ^ b).count_ones();
        }
        let mean = total as f32 / n as f32;
        assert!((mean - 32.0).abs() < 2.0, "mean flipped bits {mean}");
    }

    #[test]
    fn unit_range_and_mean() {
        let mut sum = 0.0f64;
        let n = 20_000;
        for i in 0..n {
            let u = unit_f32(hash(i, 0, 0, 0, 0));
            assert!((0.0..1.0).contains(&u));
            sum += u as f64;
        }
        let mean = sum / n as f64;
        assert!((mean - 0.5).abs() < 0.01, "mean {mean}");
    }

    #[test]
    fn unit_extremes() {
        assert_eq!(unit_f32(0), 0.0);
        assert!(unit_f32(u64::MAX) < 1.0);
    }

    #[test]
    fn integer_ranges() {
        for i in 0..5000u32 {
            let h = hash(i, 1, 1, 1, 1);
            assert!(below(h, 7) < 7);
            let r = range_i32(h, -3, 3);
            assert!((-3..=3).contains(&r));
            let f = range_f32(h, 2.0, 4.0);
            assert!((2.0..4.0).contains(&f));
        }
        assert_eq!(below(u64::MAX, 0), 0);
        assert_eq!(range_i32(123, 5, 5), 5);
        assert_eq!(range_i32(123, 5, 1), 5);
        // All values in a small range are reachable.
        let mut hit = [false; 7];
        for i in 0..1000u32 {
            hit[(range_i32(hash(i, 0, 0, 0, 9), -3, 3) + 3) as usize] = true;
        }
        assert!(hit.iter().all(|&h| h));
    }

    #[test]
    fn chance_edges_and_rate() {
        for i in 0..1000u32 {
            let h = hash(i, 0, 0, 0, 0);
            assert!(!chance(h, 0.0));
            assert!(chance(h, 1.0));
            assert!(!chance(h, -1.0));
        }
        let hits = (0..10_000u32).filter(|&i| chance(hash(i, 2, 0, 0, 0), 0.3)).count();
        assert!((2800..3200).contains(&hits), "hits {hits}");
    }

    #[test]
    fn weighted_pick_distribution() {
        let w = [1.0, 0.0, 3.0];
        let mut counts = [0usize; 3];
        for i in 0..20_000u32 {
            counts[weighted_pick(hash(i, 0, 0, 0, 0), &w).unwrap()] += 1;
        }
        assert_eq!(counts[1], 0);
        let ratio = counts[2] as f32 / counts[0] as f32;
        assert!((ratio - 3.0).abs() < 0.25, "ratio {ratio}");
    }

    #[test]
    fn weighted_pick_degenerate() {
        assert_eq!(weighted_pick(1, &[]), None);
        assert_eq!(weighted_pick(1, &[0.0, -1.0, f32::NAN]), None);
        assert_eq!(weighted_pick(u64::MAX, &[0.0, 2.0, f32::INFINITY]), Some(1));
        assert_eq!(weighted_pick(0, &[0.0, 0.0, 5.0]), Some(2));
    }

    #[test]
    fn rng_key_matches_free_functions() {
        let k = RngKey::new(99, 2, 3);
        assert_eq!(k.at(5, 6), hash(99, 2, 3, 5, 6));
        assert_eq!(k.unit(5, 6), unit_f32(hash(99, 2, 3, 5, 6)));
        assert_eq!(k.pick(5, 6, &[1.0, 1.0]), weighted_pick(hash(99, 2, 3, 5, 6), &[1.0, 1.0]));
    }

    #[test]
    fn derive_is_independent() {
        let h = hash(1, 0, 0, 0, 0);
        assert_ne!(derive(h, 0), derive(h, 1));
        assert_ne!(derive(h, 0), h);
    }
}
