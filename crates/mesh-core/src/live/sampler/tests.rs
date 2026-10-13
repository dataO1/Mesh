//! Sampler engine tests: pitch, playback position, timing, choke, stealing,
//! tie, RT safety, and an ignored throughput benchmark.

use basedrop::Shared;

use super::*;
use crate::engine::gc::gc_handle;
use crate::live::event::{flags, EventKind, LiveEvent};
use crate::types::StereoSample;

// Debug test builds count allocator calls made inside `assert_no_alloc`.
// NOTE: a crate may only have one global allocator; if another live-mode test
// module needs the same check, move this to a shared test-support module.
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC_CHECK: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

const SR: u32 = 48_000;
const BLOCK: usize = 256;

fn shared<T: Send + 'static>(v: T) -> Shared<T> {
    Shared::new(&gc_handle(), v)
}

fn sine(freq: f32, secs: f32, root_key: u8) -> Shared<SampleData> {
    let n = (secs * SR as f32) as usize;
    let s: Vec<f32> = (0..n)
        .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / SR as f32).sin() * 0.5)
        .collect();
    shared(SampleData::from_channels(&s, &s, SR, root_key))
}

fn dc(level: f32, frames: usize) -> Shared<SampleData> {
    let s = vec![level; frames];
    shared(SampleData::from_channels(&s, &s, SR, 60))
}

/// Ramp 0 → 1 over the sample, so the output value reveals the read position.
fn ramp(frames: usize) -> Shared<SampleData> {
    let s: Vec<f32> = (0..frames).map(|i| i as f32 / frames as f32).collect();
    shared(SampleData::from_channels(&s, &s, SR, 60))
}

fn bank(slots: Vec<(u8, SlotDef)>) -> Shared<SampleBank> {
    let mut b = SampleBank::new();
    for (t, s) in slots {
        b.set(t, s);
    }
    shared(b)
}

/// Neutral lane params: no attack, linear velocity, full sustain.
fn flat_params() -> LaneSamplerParams {
    LaneSamplerParams { attack: 0.0, velocity_curve: 1.0, ..Default::default() }
}

fn sampler(slots: Vec<(u8, SlotDef)>, voices: usize) -> Sampler {
    let mut s = Sampler::new(SR, voices);
    s.set_bank(bank(slots));
    s.set_params(flat_params());
    s
}

fn trig(target: u8) -> LiveEvent {
    LiveEvent { kind: EventKind::Trigger, target, vel: 127, ..Default::default() }
}

fn note_on(key: u8, note_id: u16) -> LiveEvent {
    LiveEvent { kind: EventKind::NoteOn, key, vel: 127, note_id, ..Default::default() }
}

/// Render `frames` frames in `BLOCK`-sized blocks.
fn render(s: &mut Sampler, frames: usize, spt: f64) -> Vec<StereoSample> {
    let mut out = vec![StereoSample::default(); frames];
    for chunk in out.chunks_mut(BLOCK) {
        s.process(chunk, spt);
    }
    out
}

/// Frequency from interpolated rising zero crossings of the left channel.
fn measure_freq(x: &[StereoSample]) -> f64 {
    let mut crossings = Vec::new();
    for i in 1..x.len() {
        let (a, b) = (x[i - 1].left as f64, x[i].left as f64);
        if a < 0.0 && b >= 0.0 {
            crossings.push(i as f64 - 1.0 + a / (a - b));
        }
    }
    assert!(crossings.len() > 10, "too few crossings");
    let span = crossings.last().unwrap() - crossings[0];
    (crossings.len() - 1) as f64 / span * SR as f64
}

const ALL_INTERP: [Interpolation; 3] = [Interpolation::Linear, Interpolation::Hermite, Interpolation::Sinc8];

#[test]
fn pitch_follows_key_and_cents() {
    let f0 = 441.0;
    for interp in ALL_INTERP {
        for (key, cents, ratio) in [(72u8, 0i16, 2.0), (60, 0, 1.0), (0, 0, 1.0), (48, 0, 0.5), (60, 100, 2f64.powf(1.0 / 12.0)), (67, 0, 2f64.powf(7.0 / 12.0))] {
            let mut s = sampler(vec![(0, SlotDef::whole(sine(f0, 1.0, 60)))], 4);
            s.set_params(LaneSamplerParams { interpolation: interp, ..flat_params() });
            s.handle_event(&LiveEvent { key, cents, ..trig(0) }, 0);
            let out = render(&mut s, 12_000, 100.0);
            let f = measure_freq(&out[1000..11_000]);
            let want = f0 as f64 * ratio;
            assert!((f / want - 1.0).abs() < 0.002, "{interp:?} key {key} cents {cents}: {f} Hz, want {want}");
        }
    }
}

#[test]
fn unpitched_slot_ignores_key() {
    let mut s = sampler(vec![(0, SlotDef::whole(sine(441.0, 1.0, 60)).unpitched())], 4);
    s.handle_event(&LiveEvent { key: 72, ..trig(0) }, 0);
    let f = measure_freq(&render(&mut s, 12_000, 100.0)[1000..11_000]);
    assert!((f / 441.0 - 1.0).abs() < 0.002, "{f}");
}

#[test]
fn root_cents_corrects_tuning() {
    // Sample recorded 100 cents sharp: root_cents = +100 pulls it back down.
    let data = {
        let n = SR as usize;
        let s: Vec<f32> = (0..n).map(|i| (2.0 * std::f32::consts::PI * 441.0 * i as f32 / SR as f32).sin()).collect();
        shared(SampleData::from_channels(&s, &s, SR, 60).with_root_cents(100))
    };
    let mut s = sampler(vec![(0, SlotDef::whole(data))], 4);
    s.handle_event(&LiveEvent { key: 60, ..trig(0) }, 0);
    let f = measure_freq(&render(&mut s, 12_000, 100.0)[1000..11_000]);
    let want = 441.0 * 2f64.powf(-1.0 / 12.0);
    assert!((f / want - 1.0).abs() < 0.002, "{f} want {want}");
}

#[test]
fn start_offset_and_reverse() {
    let n = 48_000;
    let cases = [
        (0u16, false, 0.0f32, 1.0f32),
        (32768, false, 0.5, 1.0),
        (0, true, 1.0, -1.0),
        (16384, true, 0.75, -1.0),
    ];
    for (start, reverse, want, slope) in cases {
        let mut s = sampler(vec![(0, SlotDef::whole(ramp(n)))], 4);
        let fl = if reverse { flags::REVERSE } else { 0 };
        s.handle_event(&LiveEvent { start, flags: fl, ..trig(0) }, 0);
        let out = render(&mut s, 2048, 100.0);
        // Skip the filter's settling, then compare against the ramp.
        let at = 64;
        let expect = want + slope * at as f32 / n as f32;
        assert!((out[at].left - expect).abs() < 0.005, "start {start} rev {reverse}: {} vs {expect}", out[at].left);
        let later = out[1024].left;
        assert!((later - out[at].left) * slope > 0.0, "direction wrong for rev {reverse}");
    }
}

#[test]
fn slice_slot_plays_only_its_range() {
    let n = 48_000;
    let data = SampleData::from_channels(
        &(0..n).map(|i| i as f32 / n as f32).collect::<Vec<_>>(),
        &[],
        SR,
        60,
    )
    .with_slices(vec![
        Slice { start: 0, end: 12_000, kind: SliceType::Kick },
        Slice { start: 24_000, end: 25_000, kind: SliceType::Snare },
    ]);
    let data = shared(data);
    let mut b = SampleBank::new();
    assert_eq!(b.map_slices(10, &data, 1), 2);
    let mut s = Sampler::new(SR, 4);
    s.set_bank(shared(b));
    s.set_params(flat_params());
    s.handle_event(&trig(11), 0);
    let out = render(&mut s, 2048, 100.0);
    assert!((out[64].left - (24_064.0 / n as f32)).abs() < 0.005);
    // 1000-frame slice: silent and voice freed after its end.
    assert_eq!(out[1100].left, 0.0);
    assert_eq!(s.voices_active(), 0);
}

#[test]
fn onset_is_sample_accurate() {
    for offset in [0u32, 1, 77, 255] {
        let mut s = sampler(vec![(0, SlotDef::whole(dc(1.0, 10_000)))], 4);
        s.set_params(LaneSamplerParams::default()); // with the default attack
        s.handle_event(&trig(0), offset);
        let mut out = vec![StereoSample::default(); BLOCK];
        s.process(&mut out, 100.0);
        let first = out.iter().position(|x| x.left != 0.0).unwrap();
        assert_eq!(first, offset as usize);
    }
}

/// First index at or after `from` where the left channel drops below `level`.
fn first_below(out: &[StereoSample], from: usize, level: f32) -> usize {
    (from..out.len()).find(|&i| out[i].left < level).unwrap()
}

#[test]
fn gate_releases_on_exact_sample() {
    for offset in [0u32, 37] {
        let mut s = sampler(vec![(0, SlotDef::whole(dc(1.0, 96_000)))], 4);
        s.set_params(LaneSamplerParams { release: 0.01, ..flat_params() });
        s.handle_event(&LiveEvent { gate: 24, ..trig(0) }, offset);
        let out = render(&mut s, 6000, 100.0);
        // 24 ticks × 100 samples = release starts 2400 samples after onset.
        assert_eq!(first_below(&out, offset as usize + 100, 0.999), offset as usize + 2400);
        // Release to −80 dB ≈ 13.3 ms → voice gone well before 6000.
        assert_eq!(s.voices_active(), 0);
    }
}

#[test]
fn gate_follows_tempo_change_between_blocks() {
    let mut s = sampler(vec![(0, SlotDef::whole(dc(1.0, 96_000)))], 4);
    s.handle_event(&LiveEvent { gate: 24, ..trig(0) }, 0);
    let mut a = vec![StereoSample::default(); 1200];
    s.process(&mut a, 100.0); // 12 ticks elapse
    let b = render(&mut s, 2000, 50.0); // remaining 12 ticks = 600 samples
    assert!(a.iter().skip(100).all(|x| x.left > 0.999));
    assert_eq!(first_below(&b, 0, 0.999), 600);
}

#[test]
fn note_on_holds_until_matching_note_off() {
    let mut s = sampler(vec![(0, SlotDef::whole(dc(0.25, 96_000)))], 4);
    s.handle_event(&note_on(60, 1), 0);
    s.handle_event(&note_on(64, 2), 0);
    render(&mut s, 4096, 100.0);
    assert_eq!(s.voices_active(), 2);
    s.handle_event(&LiveEvent { kind: EventKind::NoteOff, note_id: 1, ..Default::default() }, 10);
    render(&mut s, 4096, 100.0);
    assert_eq!(s.voices_active(), 1);
}

#[test]
fn one_shot_plays_to_end_and_loop_holds() {
    let mut s = sampler(vec![(0, SlotDef::whole(dc(1.0, 1000)))], 4);
    s.handle_event(&trig(0), 0);
    render(&mut s, 1024, 100.0);
    assert_eq!(s.voices_active(), 0, "one-shot must end at slot end");

    s.handle_event(&LiveEvent { gate: 96, flags: flags::LOOP, ..trig(0) }, 0);
    let out = render(&mut s, 9000, 100.0);
    assert_eq!(s.voices_active(), 1, "loop holds while gated");
    // Continuous across the loop seams (every 1000 frames), all interpolators read wrapped taps.
    assert!(out[200..9000].iter().all(|x| x.left > 0.99), "loop seam dip");
    render(&mut s, 4000, 100.0); // gate ends at 9600, release follows
    assert_eq!(s.voices_active(), 0);
}

#[test]
fn decay_override_shortens_hit() {
    let mut s = sampler(vec![(0, SlotDef::whole(dc(1.0, 96_000)))], 4);
    // decay byte 1 = 5 ms to −60 dB.
    s.handle_event(&LiveEvent { decay: 1, ..trig(0) }, 0);
    let out = render(&mut s, 2048, 100.0);
    assert!(out[240].left < 0.1);
    assert_eq!(s.voices_active(), 0);
}

#[test]
fn choke_fades_within_10ms() {
    let fade = (CHOKE_FADE_SECS * SR as f32) as usize; // 480
    let mut s = sampler(vec![(0, SlotDef::whole(dc(1.0, 96_000)).with_choke(3))], 8);
    s.handle_event(&trig(0), 0);
    let choke_at = 1000;
    let mut out = Vec::new();
    let mut gone_at = None;
    for b in 0..20 {
        if b == choke_at / BLOCK {
            let ev = LiveEvent { kind: EventKind::Choke, choke: 3, ..Default::default() };
            s.handle_event(&ev, (choke_at % BLOCK) as u32);
        }
        let mut blk = vec![StereoSample::default(); BLOCK];
        s.process(&mut blk, 100.0);
        out.extend_from_slice(&blk);
        if gone_at.is_none() && s.voices_active() == 0 {
            gone_at = Some((b + 1) * BLOCK);
        }
    }
    assert!(out[choke_at - 1].left > 0.999);
    assert!(out[choke_at + 1].left > 0.99, "fade, not a hard cut");
    assert!((out[choke_at + fade / 2].left - 0.5).abs() < 0.02);
    assert_eq!(out[choke_at + fade + 2].left, 0.0);
    let gone = gone_at.unwrap();
    assert!(gone <= choke_at + fade + BLOCK, "voice freed at {gone}");
}

#[test]
fn new_hit_chokes_its_group_only() {
    let mut s = sampler(
        vec![
            (0, SlotDef::whole(dc(0.1, 96_000)).with_choke(1)),
            (1, SlotDef::whole(dc(0.2, 96_000)).with_choke(1)),
            (2, SlotDef::whole(dc(0.4, 96_000)).with_choke(2)),
        ],
        8,
    );
    s.handle_event(&trig(0), 0);
    s.handle_event(&trig(2), 0);
    s.handle_event(&trig(1), 100);
    let out = render(&mut s, 2048, 100.0);
    // Group 1: target 1 replaced target 0. Group 2 untouched.
    assert!((out[2000].left - 0.6).abs() < 0.01, "{}", out[2000].left);
    assert_eq!(s.voices_active(), 2);
    // Event choke overrides the slot default.
    s.handle_event(&LiveEvent { choke: 2, ..trig(0) }, 0);
    let out = render(&mut s, 2048, 100.0);
    assert!((out[2000].left - 0.3).abs() < 0.01, "{}", out[2000].left);
}

#[test]
fn steals_oldest_voice_when_full() {
    let mut s = sampler(
        vec![
            (0, SlotDef::whole(dc(0.1, 96_000))),
            (1, SlotDef::whole(dc(0.2, 96_000))),
            (2, SlotDef::whole(dc(0.4, 96_000))),
        ],
        2,
    );
    s.handle_event(&trig(0), 0);
    s.handle_event(&trig(1), 10);
    s.handle_event(&trig(2), 20);
    let out = render(&mut s, 1024, 100.0);
    assert_eq!(s.voices_active(), 2);
    assert!((out[1000].left - 0.6).abs() < 0.01, "oldest (0.1) must be stolen: {}", out[1000].left);
    // Next steal takes target 1, the oldest survivor.
    s.handle_event(&trig(0), 0);
    let out = render(&mut s, 1024, 100.0);
    assert!((out[1000].left - 0.5).abs() < 0.01, "{}", out[1000].left);
}

#[test]
fn tie_glides_to_target_pitch() {
    let mut s = sampler(vec![(0, SlotDef::whole(sine(441.0, 4.0, 60)))], 4);
    s.handle_event(&note_on(60, 1), 0);
    render(&mut s, 1024, 100.0);
    let glide = 128; // log scale 1 ms .. 2 s
    let glide_samples = (glide_secs(glide) * SR as f32) as usize;
    s.handle_event(&LiveEvent { flags: flags::TIE, glide, ..note_on(72, 2) }, 0);
    render(&mut s, glide_samples / 2, 100.0);
    let mid = s.newest_voice_rate().unwrap();
    assert!(mid > 1.3 && mid < 1.6, "mid-glide rate {mid}");
    render(&mut s, glide_samples, 100.0);
    assert_eq!(s.voices_active(), 1, "tie must not retrigger");
    assert!((s.newest_voice_rate().unwrap() - 2.0).abs() < 1e-9);
    let f = measure_freq(&render(&mut s, 10_000, 100.0));
    assert!((f / 882.0 - 1.0).abs() < 0.002, "{f}");
    // The tied voice now answers note_id 2.
    s.handle_event(&LiveEvent { kind: EventKind::NoteOff, note_id: 1, ..Default::default() }, 0);
    render(&mut s, 4096, 100.0);
    assert_eq!(s.voices_active(), 1);
    s.handle_event(&LiveEvent { kind: EventKind::NoteOff, note_id: 2, ..Default::default() }, 0);
    render(&mut s, 4096, 100.0);
    assert_eq!(s.voices_active(), 0);
}

#[test]
fn tie_without_held_voice_starts_a_note() {
    let mut s = sampler(vec![(0, SlotDef::whole(sine(441.0, 1.0, 60)))], 4);
    s.handle_event(&LiveEvent { flags: flags::TIE, ..note_on(72, 1) }, 0);
    render(&mut s, 512, 100.0);
    assert_eq!(s.voices_active(), 1);
    assert!((s.newest_voice_rate().unwrap() - 2.0).abs() < 1e-9);
}

#[test]
fn mono_group_plays_overlapping_notes_legato() {
    let data = sine(441.0, 2.0, 60);
    let mut b = SampleBank::new();
    b.set(0, SlotDef::whole(data).with_choke(5));
    b.set_mono(5, true);
    let mut s = Sampler::new(SR, 4);
    s.set_bank(shared(b));
    s.handle_event(&note_on(60, 1), 0);
    s.handle_event(&note_on(67, 2), 100);
    render(&mut s, 1024, 100.0);
    assert_eq!(s.voices_active(), 1);
    assert!((s.newest_voice_rate().unwrap() - 2f64.powf(7.0 / 12.0)).abs() < 1e-9);
}

#[test]
fn multisample_zones_resolve_by_key() {
    let low = sine(220.0, 1.0, 48);
    let high = sine(880.0, 1.0, 72);
    let mut b = SampleBank::new();
    b.add_zone(0, SlotDef::whole(low).with_keys(0, 59));
    b.add_zone(0, SlotDef::whole(high).with_keys(60, 127));
    let mut s = Sampler::new(SR, 4);
    s.set_bank(shared(b));
    s.set_params(flat_params());
    // Key 74 → high zone (root 72) up 2 semitones.
    s.handle_event(&LiveEvent { key: 74, ..trig(0) }, 0);
    let f = measure_freq(&render(&mut s, 12_000, 100.0)[1000..11_000]);
    let want = 880.0 * 2f64.powf(2.0 / 12.0);
    assert!((f / want - 1.0).abs() < 0.002, "{f} want {want}");
}

#[test]
fn pan_velocity_and_accent() {
    let mut s = sampler(vec![(0, SlotDef::whole(dc(0.5, 96_000)))], 4);
    s.handle_event(&LiveEvent { pan: -127, ..trig(0) }, 0);
    let out = render(&mut s, 1024, 100.0);
    assert!(out[1000].right.abs() < 1e-6);
    assert!((out[1000].left - 0.5 * std::f32::consts::SQRT_2).abs() < 1e-3);

    let level = |ev: LiveEvent, curve: f32| {
        let mut s = sampler(vec![(0, SlotDef::whole(dc(0.5, 96_000)))], 4);
        s.set_params(LaneSamplerParams { velocity_curve: curve, ..flat_params() });
        s.handle_event(&ev, 0);
        render(&mut s, 1024, 100.0)[1000].left
    };
    let full = level(trig(0), 1.5);
    let half = level(LiveEvent { vel: 64, ..trig(0) }, 1.5);
    assert!((half / full - (64f32 / 127.0).powf(1.5)).abs() < 1e-3);
    let acc = level(LiveEvent { flags: flags::ACCENT, ..trig(0) }, 1.5);
    assert!((acc / full - std::f32::consts::SQRT_2).abs() < 1e-3);
    assert_eq!(level(LiveEvent { vel: 0, ..trig(0) }, 1.5), 0.0);
}

#[test]
fn filter_cutoff_offset_attenuates_highs() {
    let rms = |cutoff: i8| {
        let mut s = sampler(vec![(0, SlotDef::whole(sine(8000.0, 1.0, 60)))], 4);
        s.set_params(LaneSamplerParams { cutoff_hz: 2000.0, ..flat_params() });
        s.handle_event(&LiveEvent { cutoff, ..trig(0) }, 0);
        let out = render(&mut s, 8192, 100.0);
        (out[2000..8000].iter().map(|x| x.left * x.left).sum::<f32>() / 6000.0).sqrt()
    };
    // 8 kHz is 2 octaves above 2 kHz (≈ −24 dB); +127 opens to 32 kHz (clamped).
    assert!(rms(0) < 0.03, "{}", rms(0));
    assert!(rms(127) > 0.3, "{}", rms(127));
}

#[test]
fn meters_report_voices_and_peak() {
    let mut s = sampler(vec![(0, SlotDef::whole(dc(0.5, 96_000)))], 4);
    let m = s.meters();
    s.handle_event(&trig(0), 0);
    s.handle_event(&trig(0), 0);
    let out = render(&mut s, 1024, 100.0);
    assert_eq!(m.voices_active(), 2);
    let max = out.iter().fold(0.0f32, |a, x| a.max(x.left.abs()));
    assert!(max >= 1.0, "two 0.5 voices sum to at least 1.0");
    assert_eq!(m.take_peak(), (max, max));
    assert_eq!(m.peak(), (0.0, 0.0));
}

#[test]
fn resample_at_load_keeps_pitch_and_slices() {
    let n = 44_100;
    let s: Vec<f32> = (0..n).map(|i| (2.0 * std::f32::consts::PI * 441.0 * i as f32 / 44_100.0).sin()).collect();
    let data = SampleData::from_channels(&s, &s, 44_100, 60)
        .with_slices(vec![Slice { start: 22_050, end: 44_100, kind: SliceType::Hat }])
        .resampled_to(SR)
        .unwrap();
    assert_eq!(data.sample_rate(), SR);
    assert_eq!(data.slices[0].start, 24_000);
    let mut s = sampler(vec![(0, SlotDef::whole(shared(data)))], 4);
    s.handle_event(&trig(0), 0);
    let f = measure_freq(&render(&mut s, 12_000, 100.0)[1000..11_000]);
    assert!((f / 441.0 - 1.0).abs() < 0.002, "{f}");
}

#[test]
fn load_file_decodes_wav() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hit.wav");
    let spec = hound::WavSpec { channels: 1, sample_rate: SR, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for i in 0..1000 {
        w.write_sample(if i < 500 { 16384i16 } else { -16384 }).unwrap();
    }
    w.finalize().unwrap();
    let data = SampleData::load_file(&path, 60, SR).unwrap();
    assert_eq!(data.len(), 1000);
    assert!((data.frame(10).left - 0.5).abs() < 1e-4);
    assert_eq!(data.frame(10).left, data.frame(10).right);
}

/// The allocation checker itself is live in debug test builds.
#[cfg(debug_assertions)]
#[test]
fn alloc_checker_detects_allocation() {
    assert_no_alloc::reset_violation_count();
    assert_no_alloc::assert_no_alloc(|| std::hint::black_box(vec![0u8; 16]).len());
    assert!(assert_no_alloc::violation_count() > 0);
    assert_no_alloc::reset_violation_count();
}

/// Every RT entry point under load: stealing, choke, tie, NoteOff, voices
/// ending (dropping their `Shared` handle) and a bank swap.
// Release builds compile assert_no_alloc to a no-op, so the check is debug-only.
#[cfg(debug_assertions)]
#[test]
fn rt_paths_do_not_allocate() {
    for interp in ALL_INTERP {
        let mut s = sampler(
            vec![
                (0, SlotDef::whole(dc(0.1, 4000)).with_choke(1)),
                (1, SlotDef::whole(sine(441.0, 1.0, 60))),
            ],
            4,
        );
        s.set_params(LaneSamplerParams { interpolation: interp, ..flat_params() });
        let new_bank = bank(vec![(0, SlotDef::whole(dc(0.2, 4000)))]);
        let mut new_bank = Some(new_bank);
        let mut out = vec![StereoSample::default(); BLOCK];

        assert_no_alloc::reset_violation_count();
        assert_no_alloc::assert_no_alloc(|| {
            for b in 0..200u32 {
                let o = b % 64;
                s.handle_event(&LiveEvent { flags: if b % 3 == 0 { flags::REVERSE } else { 0 }, ..trig(0) }, o);
                s.handle_event(&LiveEvent { key: 60 + (b % 12) as u8, gate: 48, ..note_on(0, b as u16) }, o + 1);
                s.handle_event(&LiveEvent { key: 1 + (b % 3) as u8, target: 1, ..note_on(0, b as u16) }, o + 1);
                s.handle_event(&LiveEvent { target: 1, flags: flags::TIE | flags::LOOP, glide: 40, ..note_on(67, 7) }, o + 2);
                s.handle_event(&LiveEvent { kind: EventKind::NoteOff, note_id: b.wrapping_sub(1) as u16, ..Default::default() }, o + 3);
                if b % 7 == 0 {
                    s.handle_event(&LiveEvent { kind: EventKind::Choke, choke: 1, ..Default::default() }, o + 4);
                }
                if b == 100 {
                    s.set_bank(new_bank.take().unwrap());
                }
                s.set_params(LaneSamplerParams { cutoff_hz: 500.0 + b as f32 * 10.0, interpolation: interp, ..flat_params() });
                s.process(&mut out, 25.0 + (b % 5) as f64);
            }
        });

        assert_eq!(assert_no_alloc::violation_count(), 0, "{interp:?} allocated on the RT path");
    }
}

/// Throughput per interpolator. Run with
/// `cargo test --release -p mesh-core --lib live::sampler::tests::bench -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_voice_render() {
    const VOICES: usize = 32;
    const BLOCKS: usize = 4000;
    for interp in ALL_INTERP {
        let mut s = sampler(vec![(0, SlotDef::whole(sine(441.0, 60.0, 60)))], VOICES);
        s.set_params(LaneSamplerParams { interpolation: interp, cutoff_hz: 4000.0, resonance: 0.3, ..flat_params() });
        for v in 0..VOICES {
            // Spread of pitches, mostly up, so the sinc band selection is exercised.
            s.handle_event(&LiveEvent { key: 55 + (v % 19) as u8, ..trig(0) }, 0);
        }
        let mut out = vec![StereoSample::default(); BLOCK];
        s.process(&mut out, 100.0);
        assert_eq!(s.voices_active(), VOICES);
        let t = std::time::Instant::now();
        for _ in 0..BLOCKS {
            s.process(&mut out, 100.0);
        }
        let ns = t.elapsed().as_nanos() as f64;
        assert_eq!(s.voices_active(), VOICES, "voices must still be playing");
        let per = ns / (VOICES * BLOCKS * BLOCK) as f64;
        println!("{interp:?}: {per:.2} ns per voice-sample ({VOICES} voices, {BLOCKS} × {BLOCK} frames)");
    }
}
