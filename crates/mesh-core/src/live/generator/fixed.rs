//! Fixed step-pattern generator: plays a hand-written bar unchanged.
//! Used for sound-engine sign-off (design P1) and tests.

use super::{BarCtx, LaneGenerator};
use crate::live::event::{BarEvents, LiveEvent};

/// Plays the same events every bar.
#[derive(Clone, Default)]
pub struct FixedPattern {
    events: Vec<LiveEvent>,
}

impl FixedPattern {
    pub fn new(mut events: Vec<LiveEvent>) -> Self {
        events.sort_by_key(|e| e.tick);
        Self { events }
    }
}

impl LaneGenerator for FixedPattern {
    fn render_bar(&self, _ctx: &BarCtx, out: &mut BarEvents) {
        for ev in &self.events {
            out.push(*ev);
        }
    }

    fn param_names(&self) -> &'static [&'static str] {
        &[]
    }
}
