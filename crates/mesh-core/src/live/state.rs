//! UI-facing live state (design §4.2, §8).
//!
//! One [`LiveShared`] is shared by the RT engine, the bar renderer, the
//! controller and the UI. Every field is an atomic, so the UI tick reads it
//! without locks or allocation. Each field has exactly one writer:
//!
//! - **RT thread**: clock position, played harmony, per-lane on/off, option,
//!   form position and chord of the playing bar, sounding drum voices, active
//!   step, fired tag, voices, peak, moves, scope.
//! - **Controller** (UI / HID thread): master seed, form, control positions,
//!   octave, rate, drum voice flips, key intent, pending option, half-time target,
//!   quantize.
//! - **Bar renderer**: pending key and progression (set when armed, cleared
//!   when the RT thread has played the bar that applied them).

use std::sync::atomic::{AtomicBool, AtomicI8, AtomicU16, AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::music::MusicalKey;

use super::clock::BARS_PER_PHRASE;
use super::scope::ScopeRing;
use super::form::Letter;
use super::style::CONTROLS_PER_LANE;
use super::NUM_LANES;

const NO_KEY: u8 = 0xFF;
const NO_INDEX: u16 = u16::MAX;
const ORD: Ordering = Ordering::Relaxed;

fn encode_key(key: MusicalKey) -> u8 {
    key.root | ((key.minor as u8) << 4)
}

fn decode_key(v: u8) -> Option<MusicalKey> {
    (v != NO_KEY).then(|| MusicalKey::new(v & 0x0F, v & 0x10 != 0))
}

/// What produced a fired event, for the code-readout highlight (design §8).
///
/// [`LiveEvent`](super::event::LiveEvent) has no source-span field, so the
/// tag is derived on the RT thread from the event's target range (drum lane)
/// or its kind and flags (instrument lanes).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpanTag {
    #[default]
    None = 0,
    Backbone = 1,
    Break = 2,
    Hats = 3,
    Perc = 4,
    Note = 5,
    /// Tied / sliding note.
    Tie = 6,
    Accent = 7,
    Ride = 8,
    Shaker = 9,
    /// Top loop chop.
    Top = 10,
}

impl SpanTag {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Backbone,
            2 => Self::Break,
            3 => Self::Hats,
            4 => Self::Perc,
            5 => Self::Note,
            6 => Self::Tie,
            7 => Self::Accent,
            8 => Self::Ride,
            9 => Self::Shaker,
            10 => Self::Top,
            _ => Self::None,
        }
    }
}

/// Momentary-move bits in [`LaneView::moves`].
pub mod move_bits {
    pub const STUTTER: u8 = 1 << 0;
    pub const FILL: u8 = 1 << 1;
    pub const DOUBLE: u8 = 1 << 2;
    pub const VOID: u8 = 1 << 3;
}

/// Note-rate mode of a pitched lane (pads 3/4).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rate {
    #[default]
    Normal = 0,
    /// The bar's first (even bars) or second (odd bars) half at half speed.
    Half = 1,
    /// The bar compressed into each half.
    Double = 2,
}

impl Rate {
    /// Speed as a power of two: half −1, normal 0, double +1.
    pub fn exponent(self) -> i8 {
        match self {
            Rate::Normal => 0,
            Rate::Half => -1,
            Rate::Double => 1,
        }
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Half,
            2 => Self::Double,
            _ => Self::Normal,
        }
    }
}

/// Where the playing bar sits in the lane's form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FormView {
    /// Segment index in the form.
    pub segment: u8,
    /// Letter index (0 = A … 3 = D).
    pub letter: u8,
    pub prime: bool,
    /// Chord-root degree of the bar (pitched lanes).
    pub chord: u8,
}

impl FormView {
    pub fn letter(&self) -> Letter {
        Letter::ALL[(self.letter & 3) as usize]
    }

    fn pack(self) -> u32 {
        self.segment as u32 | (self.letter as u32) << 8 | (self.prime as u32) << 15 | (self.chord as u32) << 16
    }

    fn unpack(v: u32) -> Self {
        Self { segment: v as u8, letter: (v >> 8) as u8 & 0x7F, prime: v & 1 << 15 != 0, chord: (v >> 16) as u8 }
    }
}

/// Clock snapshot for the header strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockView {
    pub bar: u64,
    pub step: u8,
    pub phrase: u64,
    pub bar_in_phrase: u8,
    pub bpm: f64,
    /// Tempo the clock is heading to (equals `bpm` when not ramping).
    pub target_bpm: f64,
    pub ramping: bool,
}

/// The last event a lane fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FiredView {
    /// Events fired since start (wrapping). A changed count means a new hit.
    pub count: u32,
    pub tag: SpanTag,
    /// The event came from the fill bar.
    pub fill: bool,
}

/// Snapshot of one lane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaneView {
    /// Lane is audible (or fading in).
    pub on: bool,
    /// State the lane switches to at the next bar (equals `on` when nothing is queued).
    pub on_target: bool,
    /// Master seed the letter seeds derive from.
    pub seed: u32,
    /// Seed a Play B tap returns to.
    pub prev_seed: u32,
    /// Form index (into the style's forms).
    pub form: u16,
    /// Form queued for the next phrase boundary.
    pub form_pending: Option<u16>,
    /// Position of the playing bar in the form.
    pub pos: FormView,
    /// Octave offset of pitched notes, −2..=2.
    pub octave: i8,
    pub rate: Rate,
    /// Drum lane: pad overrides as requested
    /// ([`voice_override`](super::generator::layout::voice_override)).
    pub voice_overrides: u16,
    /// Drum lane: overrides applied in the playing bar.
    pub voice_overrides_playing: u16,
    /// Drum lane: the playing bar's arrangement (bit per
    /// [`DrumVoice`](super::generator::layout::DrumVoice)).
    pub voice_default: u8,
    /// Drum lane: voices playing in this bar (arrangement with overrides).
    pub voices_on: u8,
    pub controls: [f32; CONTROLS_PER_LANE],
    /// Shift-layer control positions.
    pub shift_controls: [f32; CONTROLS_PER_LANE],
    /// Instrument option / kit that is sounding.
    pub option: u16,
    /// Option being browsed or queued for the next bar.
    pub option_pending: Option<u16>,
    /// Step the lane is reading from its bar (differs from the clock step
    /// during stutter and double rate).
    pub active_step: u8,
    pub fired: FiredView,
    pub voices: u32,
    /// [`move_bits`] of the moves currently held.
    pub moves: u8,
}

/// Per-lane shared state.
pub struct LaneShared {
    on: AtomicBool,
    on_target: AtomicBool,
    seed: AtomicU32,
    prev_seed: AtomicU32,
    form: AtomicU16,
    form_pending: AtomicU16,
    pos: AtomicU32,
    octave: AtomicI8,
    rate: AtomicU8,
    voice_overrides: AtomicU16,
    voice_overrides_playing: AtomicU16,
    voice_default: AtomicU8,
    voices_on: AtomicU8,
    controls: [AtomicU32; CONTROLS_PER_LANE],
    shift_controls: [AtomicU32; CONTROLS_PER_LANE],
    option: AtomicU16,
    option_pending: AtomicU16,
    active_step: AtomicU8,
    /// `count << 8 | fill << 7 | tag`.
    fired: AtomicU64,
    voices: AtomicU32,
    peak: AtomicU32,
    moves: AtomicU8,
    scope: ScopeRing,
}

impl LaneShared {
    fn new() -> Self {
        Self {
            on: AtomicBool::new(true),
            on_target: AtomicBool::new(true),
            seed: AtomicU32::new(0),
            prev_seed: AtomicU32::new(0),
            form: AtomicU16::new(0),
            form_pending: AtomicU16::new(NO_INDEX),
            pos: AtomicU32::new(0),
            octave: AtomicI8::new(0),
            rate: AtomicU8::new(0),
            voice_overrides: AtomicU16::new(0),
            voice_overrides_playing: AtomicU16::new(0),
            voice_default: AtomicU8::new(0xFF),
            voices_on: AtomicU8::new(0xFF),
            controls: std::array::from_fn(|_| AtomicU32::new(0.5f32.to_bits())),
            shift_controls: std::array::from_fn(|_| AtomicU32::new(0.5f32.to_bits())),
            option: AtomicU16::new(0),
            option_pending: AtomicU16::new(NO_INDEX),
            active_step: AtomicU8::new(0),
            fired: AtomicU64::new(0),
            voices: AtomicU32::new(0),
            peak: AtomicU32::new(0),
            moves: AtomicU8::new(0),
            scope: ScopeRing::new(),
        }
    }

    pub fn view(&self) -> LaneView {
        let fired = self.fired.load(ORD);
        let pending = self.option_pending.load(ORD);
        LaneView {
            on: self.on.load(ORD),
            on_target: self.on_target.load(ORD),
            seed: self.seed.load(ORD),
            prev_seed: self.prev_seed.load(ORD),
            form: self.form.load(ORD),
            form_pending: {
                let p = self.form_pending.load(ORD);
                (p != NO_INDEX && p != self.form.load(ORD)).then_some(p)
            },
            pos: FormView::unpack(self.pos.load(ORD)),
            octave: self.octave.load(ORD),
            rate: Rate::from_u8(self.rate.load(ORD)),
            voice_overrides: self.voice_overrides.load(ORD),
            voice_overrides_playing: self.voice_overrides_playing.load(ORD),
            voice_default: self.voice_default.load(ORD),
            voices_on: self.voices_on.load(ORD),
            controls: std::array::from_fn(|i| f32::from_bits(self.controls[i].load(ORD))),
            shift_controls: std::array::from_fn(|i| f32::from_bits(self.shift_controls[i].load(ORD))),
            option: self.option.load(ORD),
            option_pending: (pending != NO_INDEX && pending != self.option.load(ORD)).then_some(pending),
            active_step: self.active_step.load(ORD),
            fired: FiredView {
                count: (fired >> 8) as u32,
                tag: SpanTag::from_u8(fired as u8 & 0x7F),
                fill: fired & 0x80 != 0,
            },
            voices: self.voices.load(ORD),
            moves: self.moves.load(ORD),
        }
    }

    /// Scope ring of the post-glue lane signal.
    pub fn scope(&self) -> &ScopeRing {
        &self.scope
    }

    /// Highest absolute sample since the last [`Self::take_peak`].
    pub fn peak(&self) -> f32 {
        f32::from_bits(self.peak.load(ORD))
    }

    /// Read and reset the held peak (meter decay is the reader's job).
    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.peak.swap(0, ORD))
    }

    // ── RT writers ──

    pub(crate) fn set_on(&self, on: bool, target: bool) {
        self.on.store(on, ORD);
        self.on_target.store(target, ORD);
    }

    pub(crate) fn set_option(&self, option: u16) {
        self.option.store(option, ORD);
    }

    pub(crate) fn set_active_step(&self, step: u8) {
        self.active_step.store(step, ORD);
    }

    pub(crate) fn note_fired(&self, tag: SpanTag, fill: bool) {
        let count = (self.fired.load(ORD) >> 8).wrapping_add(1) & 0xFFFF_FFFF;
        self.fired.store(count << 8 | (fill as u64) << 7 | tag as u64, ORD);
    }

    pub(crate) fn set_meters(&self, voices: u32, peak: f32) {
        self.voices.store(voices, ORD);
        // Non-negative f32 bit patterns order like integers.
        self.peak.fetch_max(peak.to_bits(), ORD);
    }

    pub(crate) fn set_moves(&self, moves: u8) {
        self.moves.store(moves, ORD);
    }

    /// Form position, chord and form index of the bar that started playing.
    pub(crate) fn set_bar_pos(&self, pos: FormView, form: u16) {
        self.pos.store(pos.pack(), ORD);
        self.form.store(form, ORD);
    }

    pub(crate) fn set_voices_playing(&self, overrides: u16, default: u8, on: u8) {
        self.voice_overrides_playing.store(overrides, ORD);
        self.voice_default.store(default, ORD);
        self.voices_on.store(on, ORD);
    }

    // ── Controller writers ──

    pub(crate) fn set_seed(&self, seed: u32, prev: u32) {
        self.seed.store(seed, ORD);
        self.prev_seed.store(prev, ORD);
    }

    pub(crate) fn set_form_pending(&self, form: Option<u16>) {
        self.form_pending.store(form.unwrap_or(NO_INDEX), ORD);
    }

    pub(crate) fn set_octave(&self, octave: i8) {
        self.octave.store(octave, ORD);
    }

    pub(crate) fn set_rate(&self, rate: Rate) {
        self.rate.store(rate as u8, ORD);
    }

    pub(crate) fn set_voice_overrides(&self, overrides: u16) {
        self.voice_overrides.store(overrides, ORD);
    }

    pub(crate) fn set_control(&self, control: usize, position: f32) {
        self.controls[control].store(position.to_bits(), ORD);
    }

    pub(crate) fn set_shift_control(&self, control: usize, position: f32) {
        self.shift_controls[control].store(position.to_bits(), ORD);
    }

    pub(crate) fn set_option_pending(&self, option: Option<u16>) {
        self.option_pending.store(option.unwrap_or(NO_INDEX), ORD);
    }
}

/// State shared between the RT engine, the renderer, the controller and the UI.
pub struct LiveShared {
    bar: AtomicU64,
    step: AtomicU8,
    bpm: AtomicU64,
    target_bpm: AtomicU64,
    ramping: AtomicBool,
    /// `(bar as u32) << 32 | epoch` of the bar packet swapped in at the last
    /// bar boundary. Epoch 0 = no packet had arrived (the bar was repeated).
    /// `u64::MAX` until the first bar starts.
    played: AtomicU64,
    starved_bars: AtomicU32,
    key: AtomicU8,
    key_pending: AtomicU8,
    key_intent: AtomicU8,
    chord_degree: AtomicU8,
    progression: AtomicU16,
    progression_pending: AtomicU16,
    half_time: AtomicBool,
    /// Mode of the key's family playing, and a new harmony seed armed.
    mode: AtomicU8,
    mode_pending: AtomicBool,
    half_time_target: AtomicBool,
    quantize: AtomicBool,
    lanes: [LaneShared; NUM_LANES],
}

impl LiveShared {
    pub fn new(bpm: f64, key: MusicalKey, progression: usize) -> Self {
        Self {
            bar: AtomicU64::new(0),
            step: AtomicU8::new(0),
            bpm: AtomicU64::new(bpm.to_bits()),
            target_bpm: AtomicU64::new(bpm.to_bits()),
            ramping: AtomicBool::new(false),
            played: AtomicU64::new(u64::MAX),
            starved_bars: AtomicU32::new(0),
            key: AtomicU8::new(encode_key(key)),
            key_pending: AtomicU8::new(NO_KEY),
            key_intent: AtomicU8::new(NO_KEY),
            chord_degree: AtomicU8::new(0),
            progression: AtomicU16::new(progression as u16),
            progression_pending: AtomicU16::new(NO_INDEX),
            half_time: AtomicBool::new(false),
            mode: AtomicU8::new(0),
            mode_pending: AtomicBool::new(false),
            half_time_target: AtomicBool::new(false),
            quantize: AtomicBool::new(false),
            lanes: std::array::from_fn(|_| LaneShared::new()),
        }
    }

    /// Clock position of the last processed block.
    pub fn clock(&self) -> ClockView {
        let bar = self.bar.load(ORD);
        ClockView {
            bar,
            step: self.step.load(ORD),
            phrase: bar / BARS_PER_PHRASE,
            bar_in_phrase: (bar % BARS_PER_PHRASE) as u8,
            bpm: f64::from_bits(self.bpm.load(ORD)),
            target_bpm: f64::from_bits(self.target_bpm.load(ORD)),
            ramping: self.ramping.load(ORD),
        }
    }

    /// Key of the bar that is playing.
    pub fn key(&self) -> MusicalKey {
        decode_key(self.key.load(ORD)).unwrap_or(MusicalKey::new(0, true))
    }

    /// Mode playing in the key's family (see [`crate::live::harmony::mode_name`]).
    pub fn mode(&self) -> u8 {
        self.mode.load(ORD)
    }

    /// A new harmony seed waits for the next phrase.
    pub fn mode_pending(&self) -> bool {
        self.mode_pending.load(ORD)
    }

    pub(crate) fn set_mode_pending(&self, pending: bool) {
        self.mode_pending.store(pending, ORD);
    }

    /// Key armed for the next phrase.
    pub fn pending_key(&self) -> Option<MusicalKey> {
        decode_key(self.key_pending.load(ORD))
    }

    /// Key the intent slider points at (not armed until committed).
    pub fn intent_key(&self) -> Option<MusicalKey> {
        decode_key(self.key_intent.load(ORD))
    }

    /// Chord-root degree (0-based) of the bar that is playing.
    pub fn chord_degree(&self) -> u8 {
        self.chord_degree.load(ORD)
    }

    /// Progression index of the bar that is playing.
    pub fn progression(&self) -> usize {
        self.progression.load(ORD) as usize
    }

    /// Progression armed for the next phrase.
    pub fn pending_progression(&self) -> Option<usize> {
        let p = self.progression_pending.load(ORD);
        (p != NO_INDEX).then_some(p as usize)
    }

    /// Half-time feel of the bar that is playing.
    pub fn half_time(&self) -> bool {
        self.half_time.load(ORD)
    }

    /// Half-time state requested (lands on the next bar).
    pub fn half_time_target(&self) -> bool {
        self.half_time_target.load(ORD)
    }

    /// Lane toggles land on the bar (true) or immediately (false).
    pub fn quantize(&self) -> bool {
        self.quantize.load(ORD)
    }

    /// Bars the RT thread had to repeat because the next bar had not arrived.
    pub fn starved_bars(&self) -> u32 {
        self.starved_bars.load(ORD)
    }

    pub fn lane(&self, lane: usize) -> &LaneShared {
        &self.lanes[lane]
    }

    // ── RT writers ──

    pub(crate) fn set_clock(&self, bar: u64, step: u8, bpm: f64, target_bpm: f64, ramping: bool) {
        self.bar.store(bar, ORD);
        self.step.store(step, ORD);
        self.bpm.store(bpm.to_bits(), ORD);
        self.target_bpm.store(target_bpm.to_bits(), ORD);
        self.ramping.store(ramping, ORD);
    }

    /// Record which packet (by epoch, 0 = none) the RT thread plays for `bar`.
    pub(crate) fn set_played(&self, bar: u64, epoch: u32) {
        self.played.store((bar as u32 as u64) << 32 | epoch as u64, ORD);
        if epoch == 0 {
            self.starved_bars.fetch_add(1, ORD);
        }
    }

    /// `(bar, epoch)` of the last bar swap, `None` before the first bar.
    pub(crate) fn played(&self) -> Option<(u64, u32)> {
        let v = self.played.load(ORD);
        (v != u64::MAX).then_some((v >> 32, v as u32))
    }

    pub(crate) fn set_harmony(&self, key: MusicalKey, chord_degree: u8, progression: usize, half_time: bool, mode: u8) {
        self.mode.store(mode, ORD);
        self.key.store(encode_key(key), ORD);
        self.chord_degree.store(chord_degree, ORD);
        self.progression.store(progression as u16, ORD);
        self.half_time.store(half_time, ORD);
    }

    // ── Renderer writers ──

    pub(crate) fn set_pending_key(&self, key: Option<MusicalKey>) {
        self.key_pending.store(key.map(encode_key).unwrap_or(NO_KEY), ORD);
    }

    pub(crate) fn set_pending_progression(&self, index: Option<usize>) {
        self.progression_pending.store(index.map(|i| i as u16).unwrap_or(NO_INDEX), ORD);
    }

    // ── Controller writers ──

    pub(crate) fn set_intent_key(&self, key: Option<MusicalKey>) {
        self.key_intent.store(key.map(encode_key).unwrap_or(NO_KEY), ORD);
    }

    pub(crate) fn set_half_time_target(&self, on: bool) {
        self.half_time_target.store(on, ORD);
    }

    pub(crate) fn set_quantize(&self, on: bool) {
        self.quantize.store(on, ORD);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_encoding_round_trips_all_keys() {
        for i in 0..24u8 {
            let key = MusicalKey::new(i % 12, i >= 12);
            assert_eq!(decode_key(encode_key(key)), Some(key));
        }
        assert_eq!(decode_key(NO_KEY), None);
    }

    #[test]
    fn fired_packs_count_tag_and_fill() {
        let shared = LiveShared::new(170.0, MusicalKey::new(9, true), 0);
        let lane = shared.lane(1);
        lane.note_fired(SpanTag::Note, false);
        lane.note_fired(SpanTag::Tie, true);
        let v = lane.view().fired;
        assert_eq!((v.count, v.tag, v.fill), (2, SpanTag::Tie, true));
    }

    #[test]
    fn form_view_round_trips() {
        let v = FormView { segment: 7, letter: 3, prime: true, chord: 4 };
        assert_eq!(FormView::unpack(v.pack()), v);
        assert_eq!(v.letter(), Letter::D);
    }

    #[test]
    fn played_reports_bar_epoch_and_starvation() {
        let shared = LiveShared::new(170.0, MusicalKey::new(9, true), 0);
        assert_eq!(shared.played(), None);
        shared.set_played(7, 42);
        assert_eq!(shared.played(), Some((7, 42)));
        assert_eq!(shared.starved_bars(), 0);
        shared.set_played(8, 0);
        assert_eq!(shared.starved_bars(), 1);
    }
}
