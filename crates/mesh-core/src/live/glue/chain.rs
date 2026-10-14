//! [`GlueChain`]: the fixed-topology orchestrator (see the module docs for the graph).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::engine::{MasterClipper, MasterLimiter};
use crate::live::NUM_LANES;
use crate::types::{StereoBuffer, StereoSample};

use super::db_to_lin;
use super::dynamics::Compressor;
use super::ducker::{duck_full_band, DuckEnvelope};
use super::filter::{MonoBelow, RoleHighPass};
use super::params::GlueParams;
use super::saturation::Saturator;
use super::sends::SharedSends;
use super::Role;
use crate::live::lane::LaneFx;

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

/// Duck key for one block: the main drum kit's hits.
#[derive(Debug, Clone, Copy, Default)]
pub struct GlueKey<'a> {
    /// A kick bus, same length as the block (fires the kick envelopes).
    pub sidechain: Option<&'a [StereoSample]>,
    /// Main-kit kick onsets as ascending sample offsets into the block.
    pub triggers: &'a [u32],
    /// Main-kit snare onsets as ascending sample offsets into the block.
    pub snares: &'a [u32],
}

/// Duck envelopes: (key, release group).
const DUCK_KICK_BASS: usize = 0;
const DUCK_SNARE_BASS: usize = 1;
const DUCK_KICK_OTHERS: usize = 2;
const DUCK_SNARE_OTHERS: usize = 3;
const DUCK_ENVS: usize = 4;

/// Peak gain reduction per dynamics stage, in dB (for calibration and metering).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GlueMeters {
    pub drum_comp1: f32,
    pub drum_comp2: f32,
    /// Lane compressors by lane index (0 for the drum lane).
    pub lane_comp: [f32; NUM_LANES],
    pub master_comp: f32,
    pub limiter: f32,
}

pub struct GlueChain {
    params: GlueParams,
    lanes: [LaneSetup; NUM_LANES],
    trim: [f32; NUM_LANES],

    high_pass: [RoleHighPass; NUM_LANES],
    mono_below: [MonoBelow; NUM_LANES],
    duck_env: [DuckEnvelope; DUCK_ENVS],
    lane_comp: [Compressor; NUM_LANES],
    lane_sat: [Saturator; NUM_LANES],

    drum_comp: Compressor,
    drum_comp2: Compressor,
    drum_sat: Saturator,
    drive: Saturator,
    master_comp: Compressor,
    master_gain: f32,
    master_sat: Saturator,
    limiter: MasterLimiter,
    clipper: MasterClipper,
    sends: SharedSends,

    // Scratch, sized to `max_block` at construction.
    duck_amount: [Vec<f32>; DUCK_ENVS],
    drum_bus: StereoBuffer,
    /// Insert chains (native or CLAP effects in style order).
    drum_bus_fx: LaneFx,
    master_fx: LaneFx,
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
            duck_env: std::array::from_fn(|_| DuckEnvelope::new()),
            lane_comp: std::array::from_fn(|_| Compressor::new()),
            lane_sat: std::array::from_fn(|_| Saturator::new()),
            drum_comp: Compressor::new(),
            drum_comp2: Compressor::new(),
            drum_sat: Saturator::new(),
            drive: Saturator::new(),
            master_comp: Compressor::new(),
            master_gain: 1.0,
            master_sat: Saturator::new(),
            limiter: MasterLimiter::new(),
            clipper: MasterClipper::new(),
            sends: SharedSends::new(max_block),
            duck_amount: std::array::from_fn(|_| vec![0.0; max_block]),
            drum_bus: StereoBuffer::silence(max_block),
            drum_bus_fx: LaneFx::new(Vec::new(), max_block),
            master_fx: LaneFx::new(Vec::new(), max_block),
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
        let d = &p.duck;
        for (i, env) in self.duck_env.iter_mut().enumerate() {
            let group = if i < DUCK_KICK_OTHERS { &d.bass } else { &d.others };
            env.configure(d.attack_ms, 0.0, group.release_ms, d.key_threshold_db);
        }
        self.drum_comp.configure(&p.drum_bus.compressor);
        self.drum_comp2.configure(&p.drum_bus.compressor2);
        self.master_sat.configure(&p.master.saturation);
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
        if let Some(d) = self.params.lane_dynamics.for_role(role) {
            self.lane_comp[i].configure(&d.compressor);
            self.lane_sat[i].configure(&d.saturation);
        }
    }

    /// Install the drum-bus and master insert chains. Off-RT (at build).
    pub fn set_inserts(&mut self, drum_bus: LaneFx, master: LaneFx) {
        self.drum_bus_fx = drum_bus;
        self.master_fx = master;
    }

    /// Output latency in samples (the master limiter's lookahead).
    pub fn latency_samples(&self) -> usize {
        self.limiter.latency_samples()
    }

    /// Peak gain reduction of every dynamics stage since the last call.
    pub fn take_meters(&mut self) -> GlueMeters {
        GlueMeters {
            drum_comp1: self.drum_comp.take_peak_gr_db(),
            drum_comp2: self.drum_comp2.take_peak_gr_db(),
            lane_comp: std::array::from_fn(|i| self.lane_comp[i].take_peak_gr_db()),
            master_comp: self.master_comp.take_peak_gr_db(),
            limiter: self.limiter.take_peak_gr_db(),
        }
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
        self.sends.set_bpm(bpm);

        let n = out.len();
        debug_assert!(lanes.iter().all(|l| l.len() >= n), "lane shorter than output block");
        let mut start = 0;
        while start < n {
            let end = (start + self.max_block).min(n);
            // Triggers relative to this chunk; ascending input keeps this a contiguous range.
            let chunk_key = GlueKey {
                sidechain: key.sidechain.map(|s| &s[start.min(s.len())..end.min(s.len())]),
                triggers: offsets_in(key.triggers, start, end),
                snares: offsets_in(key.snares, start, end),
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

        // Duck amounts per envelope. Triggers are rebased to the chunk.
        if p.duck.enabled {
            for (i, env) in self.duck_env.iter_mut().enumerate() {
                let kick = i == DUCK_KICK_BASS || i == DUCK_KICK_OTHERS;
                let (sidechain, triggers) = if kick { (key.sidechain, key.triggers) } else { (None, key.snares) };
                render_rebased(env, sidechain, triggers, offset, &mut self.duck_amount[i][..m]);
            }
        }

        self.drum_bus.set_len_from_capacity(m);
        let drum_bus = self.drum_bus.as_mut_slice();
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
            // 4. Lane compressor then saturation (drums: on the drum bus).
            if let Some(d) = p.lane_dynamics.for_role(setup.role) {
                if d.compressor_enabled {
                    self.lane_comp[i].process(lane);
                }
                if d.saturation_enabled {
                    self.lane_sat[i].process(lane);
                }
            }
            // 5. Main-kit keyed full-band duck.
            if let Some(d) = p.duck.for_role(setup.role).filter(|_| p.duck.enabled) {
                let (k, s) = if setup.role == Role::Bass {
                    (DUCK_KICK_BASS, DUCK_SNARE_BASS)
                } else {
                    (DUCK_KICK_OTHERS, DUCK_SNARE_OTHERS)
                };
                duck_full_band(lane, &self.duck_amount[k][..m], &self.duck_amount[s][..m], d.kick_db, d.snare_db);
            }
            // 6. Post-lane send taps.
            self.sends.accumulate(lane, setup.delay_send, setup.reverb_send);
            // 7. Route to its bus.
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
        if p.drum_bus.compressor2_enabled {
            self.drum_comp2.process(drum_bus);
        }
        if p.drum_bus.saturation_enabled {
            self.drum_sat.process(drum_bus);
        }
        self.drum_bus_fx.process(&mut self.drum_bus);
        for (l, d) in low_bus.iter_mut().zip(self.drum_bus.as_slice()) {
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

        // Master: compressor → output gain → saturation → existing limiter → clipper.
        if p.master.compressor_enabled {
            self.master_comp.process(mix);
        }
        if self.master_gain != 1.0 {
            for s in mix.iter_mut() {
                *s *= self.master_gain;
            }
        }
        if p.master.saturation_enabled {
            self.master_sat.process(mix);
        }
        self.master_fx.process(&mut self.mix);
        self.limiter.process(&mut self.mix);
        self.clipper.process(&mut self.mix);
    }

    /// Clear all filter, envelope, dynamics and effect state.
    pub fn reset(&mut self) {
        for i in 0..NUM_LANES {
            self.high_pass[i].reset();
            self.mono_below[i].reset();
        }
        for env in &mut self.duck_env {
            env.reset();
        }
        for i in 0..NUM_LANES {
            self.lane_comp[i].reset();
            self.lane_sat[i].reset();
        }
        self.drum_comp.reset();
        self.drum_comp2.reset();
        self.master_sat.reset();
        self.drum_sat.reset();
        self.drive.reset();
        self.master_comp.reset();
        self.drum_bus_fx.reset();
        self.master_fx.reset();
        self.sends.reset();
    }
}

/// The part of ascending `offsets` that falls in `start..end`.
fn offsets_in(offsets: &[u32], start: usize, end: usize) -> &[u32] {
    &offsets[offsets.partition_point(|&x| (x as usize) < start)..offsets.partition_point(|&x| (x as usize) < end)]
}

/// Render the duck envelope for a chunk whose triggers are absolute block offsets.
fn render_rebased(
    env: &mut DuckEnvelope,
    sidechain: Option<&[StereoSample]>,
    triggers: &[u32],
    offset: u32,
    out: &mut [f32],
) {
    if offset == 0 {
        env.render(sidechain, triggers, out);
        return;
    }
    // Rare path (block > max_block): render between triggers.
    let mut pos = 0usize;
    for &t in triggers {
        let rel = (t - offset) as usize;
        let sc = sidechain.map(|s| &s[pos.min(s.len())..rel.min(s.len())]);
        env.render(sc, &[], &mut out[pos..rel]);
        env.trigger();
        pos = rel;
    }
    let sc = sidechain.map(|s| &s[pos.min(s.len())..]);
    env.render(sc, &[], &mut out[pos..]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::glue::DuckParams;
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
            chain.process(&mut lanes, GlueKey { sidechain: None, triggers: &trig, snares: &[] }, 174.0, &mut out);
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
        a.process(&mut lanes, GlueKey { sidechain: None, triggers: &triggers, snares: &[] }, 170.0, &mut out_a);

        let mut b = GlueChain::new(256);
        let mut out_b = Vec::new();
        for (s, e) in [(0, 256), (256, 512), (512, 768), (768, 1000)] {
            let mut lanes = lanes_from(&srcs, s, e - s);
            let trig: Vec<u32> = triggers.iter().filter(|&&t| (t as usize) >= s && (t as usize) < e).map(|&t| t - s as u32).collect();
            let mut out = StereoBuffer::silence(e - s);
            b.process(&mut lanes, GlueKey { sidechain: None, triggers: &trig, snares: &[] }, 170.0, &mut out);
            out_b.extend_from_slice(out.as_slice());
        }
        for (x, y) in out_a.iter().zip(out_b.iter()) {
            assert!((x.left - y.left).abs() < 1e-6 && (x.right - y.right).abs() < 1e-6);
        }
    }

    #[test]
    fn main_kit_kick_and_snare_duck_the_lanes() {
        let block = 9600;
        let mut params = GlueParams::default();
        params.drive.enabled = false;
        params.master.compressor_enabled = false;
        // No filters, so a constant signal passes unchanged.
        params.high_pass.lead.enabled = false;
        params.high_pass.extra.enabled = false;
        params.mono_below.enabled = false;
        // No lane dynamics, so the gain is the duck alone.
        params.lane_dynamics = crate::live::glue::RoleDynamicsParams {
            bass: crate::live::glue::LaneDynamicsParams { compressor_enabled: false, saturation_enabled: false, ..Default::default() },
            ..Default::default()
        };
        params.lane_dynamics.lead = params.lane_dynamics.bass;
        params.lane_dynamics.extra = params.lane_dynamics.bass;
        let mut chain = GlueChain::new(block);
        chain.set_params(&params);
        // Constant signal on every lane, so the lane gain reads off directly.
        let mut lanes: [StereoBuffer; NUM_LANES] =
            std::array::from_fn(|_| StereoBuffer::from_vec(vec![StereoSample::new(0.25, 0.25); block]));
        let mut out = StereoBuffer::silence(block);
        let (kick, snare) = (100u32, 6000u32);
        chain.process(&mut lanes, GlueKey { sidechain: None, triggers: &[kick], snares: &[snare] }, 174.0, &mut out);
        let db = |lane: usize, at: u32| 20.0 * (lanes[lane].as_slice()[at as usize].left / 0.25).log10();
        let d = DuckParams::default();
        let full = 30; // just past the 0.5 ms attack
        assert!((db(1, kick + full) + d.bass.kick_db).abs() < 0.5, "bass on the kick: {}", db(1, kick + full));
        assert!((db(2, kick + full) + d.others.kick_db).abs() < 0.5, "lead on the kick: {}", db(2, kick + full));
        assert!((db(1, snare + full) + d.bass.snare_db).abs() < 0.5, "bass on the snare: {}", db(1, snare + full));
        assert!((db(3, snare + full) + d.others.snare_db).abs() < 0.5, "extra on the snare: {}", db(3, snare + full));
        // Bass recovers after 50 ms, lead and extra are still releasing.
        let after_bass = kick + 24 + 2400 + 10;
        assert!(db(1, after_bass).abs() < 1e-3, "bass back: {}", db(1, after_bass));
        assert!(db(2, after_bass) < -0.3, "lead still ducked: {}", db(2, after_bass));
        // The drums are the key and never ducked.
        assert!(lanes[0].as_slice().iter().all(|s| s.left == 0.25));
    }

    #[test]
    fn meters_report_each_dynamics_stage() {
        let block = 4800;
        let mut chain = GlueChain::new(block);
        let loud = |n: usize| {
            StereoBuffer::from_vec(sine(1000.0, 0.9, n).into_iter().map(|v| StereoSample::new(v, v)).collect())
        };
        let mut lanes: [StereoBuffer; NUM_LANES] = std::array::from_fn(|_| loud(block));
        let mut out = StereoBuffer::silence(block);
        chain.process(&mut lanes, GlueKey::default(), 174.0, &mut out);
        let m = chain.take_meters();
        assert!(m.drum_comp1 > 1.0 && m.drum_comp2 > 0.5, "{m:?}");
        assert!(m.lane_comp[1] > 1.0 && m.lane_comp[2] > 1.0 && m.lane_comp[3] > 1.0, "{m:?}");
        assert_eq!(m.lane_comp[0], 0.0, "drums use the drum bus compressors");
        assert!(m.master_comp > 0.0 && m.limiter >= 0.0, "{m:?}");
        // Taking resets the peaks.
        let mut silent: [StereoBuffer; NUM_LANES] = std::array::from_fn(|_| StereoBuffer::silence(block));
        chain.reset();
        chain.process(&mut silent, GlueKey::default(), 174.0, &mut out);
        assert_eq!(chain.take_meters().drum_comp1, 0.0);
    }
}
