//! Fixed-size, RT-safe event schema (design §4.5).

use super::MAX_EVENTS_PER_BAR;

/// What an event does to its voice target.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EventKind {
    /// Sampler one-shot / slice trigger.
    #[default]
    Trigger = 0,
    /// Pitched note start (sampler instrument or CLAP synth).
    NoteOn = 1,
    /// Pitched note end (matched by `note_id`).
    NoteOff = 2,
    /// Choke voices in the event's choke group.
    Choke = 3,
}

/// Event flag bits.
pub mod flags {
    /// Play the sample backwards from `start`.
    pub const REVERSE: u8 = 1 << 0;
    /// Tie to the previous note (legato / glide over `glide`).
    pub const TIE: u8 = 1 << 1;
    /// Accented hit.
    pub const ACCENT: u8 = 1 << 2;
    /// Loop the slot while the gate is held.
    pub const LOOP: u8 = 1 << 3;
}

/// One musical event inside a bar. `Copy`, fixed size, no heap.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LiveEvent {
    /// Position within the bar, 0..TICKS_PER_BAR.
    pub tick: u16,
    pub kind: EventKind,
    /// Bitset of [`flags`].
    pub flags: u8,
    /// Sampler slot / slice / drum sub-voice index.
    pub target: u8,
    /// 0 = none, else lane-local choke group.
    pub choke: u8,
    /// MIDI key, resolved from scale degree at render time.
    pub key: u8,
    /// Velocity 0..=127.
    pub vel: u8,
    /// Fine pitch in cents (also sample-root correction).
    pub cents: i16,
    /// Start offset within slot/slice, 0..=65535 maps to 0.0..1.0.
    pub start: u16,
    /// Gate length in ticks, 0 = one-shot (play to end).
    pub gate: u16,
    /// Voice identity for NoteOff / CLAP note_id.
    pub note_id: u16,
    /// 0 = lane default, else amp-decay override (log scale).
    pub decay: u8,
    /// Bipolar offset to the lane filter cutoff.
    pub cutoff: i8,
    /// Glide time (log scale), used with `flags::TIE`.
    pub glide: u8,
    /// Bipolar offset to lane pan.
    pub pan: i8,
}

impl LiveEvent {
    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }
}

/// One lane's rendered bar. Events are kept sorted by `tick`.
#[derive(Clone)]
pub struct BarEvents {
    len: u16,
    events: [LiveEvent; MAX_EVENTS_PER_BAR],
}

impl Default for BarEvents {
    fn default() -> Self {
        Self { len: 0, events: [LiveEvent::default(); MAX_EVENTS_PER_BAR] }
    }
}

impl BarEvents {
    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Append an event. Returns `false` (and drops it) when full.
    pub fn push(&mut self, ev: LiveEvent) -> bool {
        if (self.len as usize) >= MAX_EVENTS_PER_BAR {
            return false;
        }
        self.events[self.len as usize] = ev;
        self.len += 1;
        true
    }

    /// Stable sort by tick (insertion sort: allocation-free, small N).
    pub fn sort(&mut self) {
        let evs = &mut self.events[..self.len as usize];
        for i in 1..evs.len() {
            let mut j = i;
            while j > 0 && evs[j - 1].tick > evs[j].tick {
                evs.swap(j - 1, j);
                j -= 1;
            }
        }
    }

    pub fn as_slice(&self) -> &[LiveEvent] {
        &self.events[..self.len as usize]
    }

    /// Edit events in place. Callers keep them sorted by tick.
    pub fn as_mut_slice(&mut self) -> &mut [LiveEvent] {
        &mut self.events[..self.len as usize]
    }
}
