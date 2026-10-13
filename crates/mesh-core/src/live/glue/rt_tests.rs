//! RT-safety: the glue chain's per-block calls must not allocate.
//!
//! `assert_no_alloc` needs its allocator installed as the test binary's
//! global allocator. Only one `#[global_allocator]` may exist per binary; if
//! another live-mode module installs `AllocDisabler` too, keep one of them.

use assert_no_alloc::{assert_no_alloc, AllocDisabler};

use crate::live::NUM_LANES;
use crate::types::{StereoBuffer, StereoSample};

use super::{GlueChain, GlueKey, GlueParams, LaneSetup, Role};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

#[test]
fn glue_chain_process_does_not_allocate() {
    let block = 512;
    let mut chain = GlueChain::new(block);
    for i in 0..NUM_LANES {
        chain.set_lane(
            i,
            LaneSetup { role: Role::ALL[i], trim_db: -2.0, delay_send: 0.3, reverb_send: 0.3 },
        );
    }
    let mut lanes: [StereoBuffer; NUM_LANES] = std::array::from_fn(|i| {
        StereoBuffer::from_vec(
            (0..block).map(|n| {
                let v = ((n * (i + 1)) as f32 * 0.05).sin() * 0.5;
                StereoSample::new(v, -v)
            }).collect(),
        )
    });
    let key: Vec<StereoSample> = (0..block).map(|n| {
        let v = if n % 128 < 8 { 0.9 } else { 0.0 };
        StereoSample::new(v, v)
    }).collect();
    let triggers = [0u32, 200, 400];
    let mut out = StereoBuffer::silence(block);
    // Longer-than-max output exercises the internal split path too.
    let mut long_lanes: [StereoBuffer; NUM_LANES] = std::array::from_fn(|_| StereoBuffer::silence(block * 3));
    let mut long_out = StereoBuffer::silence(block * 3);

    let mut params = GlueParams::default();
    params.drum_bus.saturation.asymmetry = 0.3;

    assert_no_alloc(|| {
        for b in 0..20 {
            // Parameter and tempo changes on the audio thread are allocation-free too.
            if b == 10 {
                chain.set_params(&params);
                chain.set_lane(2, LaneSetup { role: Role::Lead, trim_db: 0.0, delay_send: 0.0, reverb_send: 0.5 });
            }
            let bpm = if b < 15 { 174.0 } else { 87.0 };
            chain.process(&mut lanes, GlueKey { sidechain: Some(&key), triggers: &triggers }, bpm, &mut out);
        }
        chain.process(&mut long_lanes, GlueKey { sidechain: None, triggers: &[5, 700, 1300] }, 174.0, &mut long_out);
    });
    assert!(out.iter().all(|s| s.left.is_finite() && s.right.is_finite()));
}
