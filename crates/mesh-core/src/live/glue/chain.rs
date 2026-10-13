//! [`GlueChain`]: the fixed-topology orchestrator (see the module docs for the graph).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::engine::{MasterClipper, MasterLimiter};
use crate::live::NUM_LANES;
use crate::types::{StereoBuffer, StereoSample};

use super::db_to_lin;
use super::dynamics::Compressor;
use super::ducker::{DuckEnvelope, LowBandDucker};
use super::filter::{MonoBelow, RoleHighPass};
use super::params::GlueParams;
use super::saturation::Saturator;
use super::sends::SharedSends;
use super::Role;

/// Per-lane static setup. Changes when the lane's instrument or mapped send
/// controls change, not per block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaneSetup {
    pub role: Role,
    /// Static trim from LUFS calibration, applied first.
    pub trim_db: f32,
    /// Linear send level into the shared delay, 0..=1.
    pub delay_send: f32,
    /// Linear send level into the shared reverb, 0..=1.
    pub reverb_send: f32,
}

impl Default for LaneSetup {
    fn default() -> Self {
        Self { role: Role::Extra, trim_db: 0.0, delay_send: 0.0, reverb_send: 0.0 }
    }
}

/// Duck key for one block. Either or both sources may be used.
#[derive(Debug, Clone, Copy, Default)]
pub struct GlueKey<'a> {
    /// The drum lane's kick sub-voice bus, same length as the block.
    pub sidechain: Option<&'a [StereoSample]>,
    /// Kick onsets as ascending sample offsets into the block.
    pub triggers: &'a [u32],
}

pub struct GlueChain {
    params: GlueParams,
    lanes: [LaneSetup; NUM_LANES],
    trim: [f32; NUM_LANES],

    high_pass: [RoleHighPass; NUM_LANES],
    mono_below: [MonoBelow; NUM_LANES],
    duckers: [LowBandDucker; NUM_LANES],
    duck_env: DuckEnvelope,

    drum_comp: Compressor,
    drum_sat: Saturator,
    drive: Saturator,
    master_comp: Compressor,
    master_gain: f32,
    limiter: MasterLimiter,
    clipper: MasterClipper,
    sends: SharedSends,

    // Scratch, sized to `max_block` at construction.
    duck_amount: Vec<f32>,
    drum_bus: Vec<StereoSample>,
    low_bus: Vec<StereoSample>,
    mix: StereoBuffer,
    max_block: usize,
}

impl GlueChain {
    /// Build the chain with default parameters. `max_block` is the largest
    /// block `process` handles in one pass; longer calls are split internally.
    pub fn new(max_block: usize) -> Self {
        let max_block = max_block.max(1);
        let mut chain = Self {
            params: GlueParams::default(),
            lanes: [LaneSetup::default(); NUM_LANES],
            trim: [1.0; NUM_LANES],
            high_pass: std::array::from_fn(|_| RoleHighPass::new()),
            mono_below: std::array::from_fn(|_| MonoBelow::new()),
            duckers: std::array::from_fn(|_| LowBandDucker::new()),
            duck_env: DuckEnvelope::new(),
            drum_comp: Compressor::new(),
            drum_sat: Saturator::new(),
            drive: Saturator::new(),
            master_comp: Compressor::new(),
            master_gain: 1.0,
            limiter: MasterLimiter::new(),
            clipper: MasterClipper::new(),
            sends: SharedSends::new(max_block),
            duck_amount: vec![0.0; max_block],
            drum_bus: vec![StereoSample::default(); max_block],
            low_bus: vec![StereoSample::default(); max_block],
            mix: StereoBuffer::silence(max_block),
            max_block,
        };
        let default_roles = [Role::Drums, Role::Bass, Role::Lead, Role::Extra];
        for (i, role) in default_roles.into_iter().enumerate() {
            chain.lanes[i].role = role;
        }
        chain.set_params(&GlueParams::default());
        chain
    }

    /// Apply new parameters to every stage. Allocation-free.
    pub fn set_params(&mut self, p: &GlueParams) {
        self.params = *p;
        for i in 0..NUM_LANES {
            self.configure_lane(i);
        }
        self.duck_env.configure(&p.duck);
        self.drum_comp.configure(&p.drum_bus.compressor);
        self.drum_sat.configure(&p.drum_bus.saturation);
        self.drive.configure(&p.drive.saturation);
        self.master_comp.configure(&p.master.compressor);
        self.master_gain = db_to_lin(p.master.output_db);
        self.sends.configure(&p.delay, &p.reverb);
    }

    pub fn params(&self) -> &GlueParams {
        &self.params
    }

    /// Set a lane's role, trim and send levels. Allocation-free.
    pub fn set_lane(&mut self, lane: usize, setup: LaneSetup) {
        if lane < NUM_LANES {
            self.lanes[lane] = setup;
            self.configure_lane(lane);
        }
    }

    pub fn lane(&self, lane: usize) -> LaneSetup {
        self.lanes[lane]
    }

    fn configure_lane(&mut self, i: usize) {
        let role = self.lanes[i].role;
        self.trim[i] = db_to_lin(self.lanes[i].trim_db);
        self.high_pass[i].configure(&self.params.high_pass.for_role(role));
        self.mono_below[i].configure(&self.params.mono_below);
        self.duckers[i].set_crossover_hz(self.params.duck.crossover_hz);
    }

    /// Output latency in samples (the master limiter's lookahead).
    pub fn latency_samples(&self) -> usize {
        self.limiter.latency_samples()
    }

    /// Clip indicator of the master clipper (for the UI).
    pub fn clip_indicator(&self) -> Arc<AtomicBool> {
        self.clipper.clip_indicator()
    }

    /// Process one block.
    ///
    /// `lanes` are processed in place (afterwards they hold the post-glue lane
    /// signal, useful for scopes). `out` receives the master; its length
    /// sets the block length and every lane must be at least that long.
    pub fn process(
        &mut self,
        lanes: &mut [StereoBuffer; NUM_LANES],
        key: GlueKey<'_>,
        bpm: f64,
        out: &mut StereoBuffer,
    ) {
        self.duck_env.set_bpm(bpm);
        self.sends.set_bpm(bpm);

        let n = out.len();
        debug_assert!(lanes.iter().all(|l| l.len() >= n), "lane shorter than output block");
        let mut start = 0;
        while start < n {
            let end = (start + self.max_block).min(n);
            // Triggers relative to this chunk; ascending input keeps this a contiguous range.
            let lo = key.triggers.partition_point(|&t| (t as usize) < start);
            let hi = key.triggers.partition_point(|&t| (t as usize) < end);
            let chunk_key = GlueKey {
                sidechain: key.sidechain.map(|s| &s[start.min(s.len())..end.min(s.len())]),
                triggers: &key.triggers[lo..hi],
            };
            self.process_chunk(lanes, chunk_key, start, start as u32, end);
            out.as_mut_slice()[start..end].copy_from_slice(&self.mix.as_slice()[..end - start]);
            start = end;
        }
    }

    fn process_chunk(
        &mut self,
        lanes: &mut [StereoBuffer; NUM_LANES],
        key: GlueKey<'_>,
        start: usize,
        offset: u32,
        end: usize,
    ) {
        let m = end - start;
        let p = &self.params;

        // Duck amount, shared by all ducked lanes. Triggers are rebased to the chunk.
        let amount = &mut self.duck_amount[..m];
        if p.duck.enabled {
            render_rebased(&mut self.duck_env, key, offset, amount);
        }

        let drum_bus = &mut self.drum_bus[..m];
        let low_bus = &mut self.low_bus[..m];
        drum_bus.fill(StereoSample::default());
        low_bus.fill(StereoSample::default());
        self.mix.set_len_from_capacity(m);
        self.mix.fill_silence();
        self.sends.begin(m);

        for i in 0..NUM_LANES {
            let setup = self.lanes[i];
            let lane = &mut lanes[i].as_mut_slice()[start..end];

            // 1. Static LUFS trim.
            let t = self.trim[i];
            if t != 1.0 {
                for s in lane.iter_mut() {
                    *s *= t;
                }
            }
            // 2. Role high-pass (configured off for bass).
            self.high_pass[i].process(lane);
            // 3. Bass mono-below.
            if setup.role == Role::Bass {
                self.mono_below[i].process(lane);
            }
            // 4. Kick-keyed low-band duck.
            if p.duck.enabled {
                self.duckers[i].process(lane, amount, p.duck.depth_db(setup.role));
            }
            // 5. Post-lane send taps.
            self.sends.accumulate(lane, setup.delay_send, setup.reverb_send);
            // 6. Route to its bus.
            let dst: &mut [StereoSample] = match setup.role {
                Role::Drums => &mut *drum_bus,
                Role::Bass => &mut *low_bus,
                Role::Lead | Role::Extra => self.mix.as_mut_slice(),
            };
            for (d, s) in dst.iter_mut().zip(lane.iter()) {
                *d += *s;
            }
        }

        // Drum bus → low bus.
        if p.drum_bus.compressor_enabled {
            self.drum_comp.process(drum_bus);
        }
        if p.drum_bus.saturation_enabled {
            self.drum_sat.process(drum_bus);
        }
        for (l, d) in low_bus.iter_mut().zip(drum_bus.iter()) {
            *l += *d;
        }

        // Shared drive on drums + bass, then into the mix.
        if p.drive.enabled {
            self.drive.process(low_bus);
        }
        let mix = self.mix.as_mut_slice();
        for (o, l) in mix.iter_mut().zip(low_bus.iter()) {
            *o += *l;
        }

        // Send returns.
        self.sends.process_add(mix);

        // Master: compressor → output gain → existing limiter → clipper.
        if p.master.compressor_enabled {
            self.master_comp.process(mix);
        }
        if self.master_gain != 1.0 {
            for s in mix.iter_mut() {
                *s *= self.master_gain;
            }
        }
        self.limiter.process(&mut self.mix);
        self.clipper.process(&mut self.mix);
    }

    /// Clear all filter, envelope, dynamics and effect state.
    pub fn reset(&mut self) {
        for i in 0..NUM_LANES {
            self.high_pass[i].reset();
            self.mono_below[i].reset();
            self.duckers[i].reset();
        }
        self.duck_env.reset();
        self.drum_comp.reset();
        self.drum_sat.reset();
        self.drive.reset();
        self.master_comp.reset();
        self.sends.reset();
    }
}

/// Render the duck envelope for a chunk whose triggers are absolute block offsets.
fn render_rebased(env: &mut DuckEnvelope, key: GlueKey<'_>, offset: u32, out: &mut [f32]) {
    if offset == 0 {
        env.render(key.sidechain, key.triggers, out);
        return;
    }
    // Rare path (block > max_block): render between triggers.
    let mut pos = 0usize;
    for &t in key.triggers {
        let rel = (t - offset) as usize;
        let sc = key.sidechain.map(|s| &s[pos.min(s.len())..rel.min(s.len())]);
        env.render(sc, &[], &mut out[pos..rel]);
        env.trigger();
        pos = rel;
    }
    let sc = key.sidechain.map(|s| &s[pos.min(s.len())..]);
    env.render(sc, &[], &mut out[pos..]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::glue::filter::tests::sine;

    fn noise(seed: u32, n: usize, amp: f32) -> Vec<StereoSample> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let l = (x as f32 / u32::MAX as f32 * 2.0 - 1.0) * amp;
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let r = (x as f32 / u32::MAX as f32 * 2.0 - 1.0) * amp;
                StereoSample::new(l, r)
            })
            .collect()
    }

    fn lanes_from(srcs: &[Vec<StereoSample>; NUM_LANES], start: usize, len: usize) -> [StereoBuffer; NUM_LANES] {
        std::array::from_fn(|i| StereoBuffer::from_vec(srcs[i][start..start + len].to_vec()))
    }

    #[test]
    fn end_to_end_finite_with_sends() {
        let block = 256;
        let blocks = 400;
        let total = block * blocks;
        let srcs: [Vec<StereoSample>; NUM_LANES] = [
            noise(1, total, 0.6),
            sine(55.0, 0.7, total).into_iter().map(|v| StereoSample::new(v, v * 0.8)).collect(),
            sine(880.0, 0.5, total).into_iter().map(|v| StereoSample::new(v, -v)).collect(),
            noise(7, total, 0.3),
        ];
        let mut chain = GlueChain::new(block);
        for i in 0..NUM_LANES {
            let mut l = chain.lane(i);
            l.delay_send = 0.3;
            l.reverb_send = 0.2;
            l.trim_db = -3.0;
            chain.set_lane(i, l);
        }
        let mut out = StereoBuffer::silence(block);
        let mut peak = 0.0f32;
        let mut energy = 0.0f64;
        // Kick every beat at 174 BPM.
        let beat = (60.0 / 174.0 * 48_000.0) as usize;
        for b in 0..blocks {
            let mut lanes = lanes_from(&srcs, b * block, block);
            let s0 = b * block;
            let first = (s0 + beat - 1) / beat * beat;
            let trig: Vec<u32> = (first..s0 + block).step_by(beat).map(|t| (t - s0) as u32).collect();
            chain.process(&mut lanes, GlueKey { sidechain: None, triggers: &trig }, 174.0, &mut out);
            for s in out.iter() {
                assert!(s.left.is_finite() && s.right.is_finite());
                peak = peak.max(s.left.abs()).max(s.right.abs());
                energy += (s.left * s.left + s.right * s.right) as f64;
            }
        }
        assert!(energy > 1.0, "master must carry signal");
        assert!(peak <= 1.0, "limiter/clipper keep the master below full scale, peak {peak}");
    }

    #[test]
    fn sends_add_tail_after_input_stops() {
        let block = 512;
        let mut chain = GlueChain::new(block);
        let mut setup = chain.lane(2);
        setup.reverb_send = 1.0;
        chain.set_lane(2, setup);
        let burst = sine(1000.0, 0.5, block);
        let mut out = StereoBuffer::silence(block);
        let mut lanes: [StereoBuffer; NUM_LANES] = std::array::from_fn(|_| StereoBuffer::silence(block));
        lanes[2] = StereoBuffer::from_vec(burst.iter().map(|&v| StereoSample::new(v, v)).collect());
        chain.process(&mut lanes, GlueKey::default(), 174.0, &mut out);
        // Silent input: only the reverb tail remains.
        let mut tail = 0.0f32;
        for _ in 0..4 {
            let mut silent: [StereoBuffer; NUM_LANES] = std::array::from_fn(|_| StereoBuffer::silence(block));
            chain.process(&mut silent, GlueKey::default(), 174.0, &mut out);
            tail = tail.max(out.iter().map(|s| s.left.abs()).fold(0.0, f32::max));
        }
        assert!(tail > 1e-3, "reverb tail expected, got {tail}");
    }

    #[test]
    fn long_block_is_split_and_matches_chunked_calls() {
        // One 1000-sample call with max_block 256 equals four calls of ≤256.
        let n = 1000;
        let srcs: [Vec<StereoSample>; NUM_LANES] = std::array::from_fn(|i| noise(i as u32 + 3, n, 0.4));
        let triggers = [10u32, 300, 700];

        let mut a = GlueChain::new(256);
        let mut lanes = lanes_from(&srcs, 0, n);
        let mut out_a = StereoBuffer::silence(n);
        a.process(&mut lanes, GlueKey { sidechain: None, triggers: &triggers }, 170.0, &mut out_a);

        let mut b = GlueChain::new(256);
        let mut out_b = Vec::new();
        for (s, e) in [(0, 256), (256, 512), (512, 768), (768, 1000)] {
            let mut lanes = lanes_from(&srcs, s, e - s);
            let trig: Vec<u32> = triggers.iter().filter(|&&t| (t as usize) >= s && (t as usize) < e).map(|&t| t - s as u32).collect();
            let mut out = StereoBuffer::silence(e - s);
            b.process(&mut lanes, GlueKey { sidechain: None, triggers: &trig }, 170.0, &mut out);
            out_b.extend_from_slice(out.as_slice());
        }
        for (x, y) in out_a.iter().zip(out_b.iter()) {
            assert!((x.left - y.left).abs() < 1e-6 && (x.right - y.right).abs() < 1e-6);
        }
    }

    #[test]
    fn kick_trigger_ducks_bass_low_end() {
        let block = 4800;
        let mut params = GlueParams::default();
        // Isolate the duck: no drive, no master dynamics.
        params.drive.enabled = false;
        params.master.compressor_enabled = false;
        let mut chain = GlueChain::new(block);
        chain.set_params(&params);
        let bass: Vec<StereoSample> = sine(60.0, 0.25, block * 2).into_iter().map(|v| StereoSample::new(v, v)).collect();
        let mut out = StereoBuffer::silence(block);
        let run = |chain: &mut GlueChain, out: &mut StereoBuffer, trig: &[u32], part: usize| {
            let mut lanes: [StereoBuffer; NUM_LANES] = std::array::from_fn(|_| StereoBuffer::silence(block));
            lanes[1] = StereoBuffer::from_vec(bass[part * block..(part + 1) * block].to_vec());
            chain.process(&mut lanes, GlueKey { sidechain: None, triggers: trig }, 174.0, out);
            // Window inside the full-duck hold (attack 2 ms, hold 20 ms after block/2).
            out.as_slice()[block / 2 + 150..block / 2 + 1000].iter().map(|s| s.left.abs()).fold(0.0f32, f32::max)
        };
        let open = run(&mut chain, &mut out, &[], 0);
        let ducked = run(&mut chain, &mut out, &[block as u32 / 2], 1);
        let db = 20.0 * (ducked / open).log10();
        assert!(db < -9.0, "bass low end should duck ~12 dB, got {db}");
    }
}
