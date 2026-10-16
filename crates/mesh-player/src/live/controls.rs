//! Kontrol F1 live layout (design §7): raw controller events → live commands,
//! and engine state → LEDs / 7-segment displays.
//!
//! Two F1s. The left unit hosts lanes 0 (Drums) and 1, the right unit lanes
//! 2 and 3. Each unit is split into two halves: half A (knobs/faders 1–2,
//! pad columns 1–2, play 1–2) drives the unit's first lane, half B
//! (knobs/faders 3–4, pad columns 3–4, play 3–4) its second lane.
//!
//! Pads of one half (2 columns × 4 rows) are the lane's pads ([`pads`]):
//! drum voices on / off on the drum lane, octave / rate / form pads on the
//! pitched lanes. While Sync is held they are momentary moves:
//!
//! ```text
//!  Sync held (moves)
//!  ┌────────┬──────┐
//!  │ stutter│ fill │
//!  │ double │ void │
//!  │   –    │  –   │
//!  │   –    │  –   │
//!  └────────┴──────┘
//! ```
//!
//! | Control | Function |
//! |---|---|
//! | Knob 1 / 3 | lane control 0 (knob A) |
//! | Fader 1 / 3 | lane control 1 (fader A) |
//! | Knob 2 / 4 | lane control 2 (knob B) |
//! | Fader 2 / 4 | lane control 3 (fader B) |
//! | Shift (same unit) + knob / fader | the lane's Shift-layer control |
//! | Drum pad n | turn drum voice n on / off against the seed's arrangement (lands on the next bar; immediate when Quant is off; a new drum seed clears the changes) |
//! | Drum pads, bottom left to top right | kick, snare, break, perc, hats, ride, top loop, shaker |
//! | Shift + drum pad 7 / 8 (bottom row) | previous / next form of the drum lane |
//! | Pitched pad 1 / 2 | octave down / up (both = back to 0) |
//! | Pitched pad 3 / 4 | half / double rate, latched (next bar) |
//! | Pitched pad 5 / 6 | previous / next form (next phrase) |
//! | Sync (held) | the unit's pads become momentary moves (see above); release returns to the lane pads and releases held moves |
//! | Play 1 / 3 (button A) | lane on/off |
//! | Play 2 / 4 (button B) | press: a random new master seed for the lane (lands on the next bar; the display shows it while held) |
//! | Encoder | nothing on its own |
//! | Left Shift + fader 1 | key intent |
//! | Left Shift + fader 2 | tempo (ramp target) |
//! | Left Shift + encoder push | commit the key intent |
//! | Quant (either unit) | quantized lane and voice toggles on/off (default off) |
//! | Capture | capture the unit's selected lane. With that unit's Shift: everything |
//! | Reverse (either unit) | half-time toggle |
//! | Size / Browse (held) + encoder | browse lane A / lane B option, release loads it at the bar |
//! | Type | minor / major switch for the next phrase. With that unit's Shift: a new harmony seed (the mode) |
//!
//! The selected lane of a unit is the lane last touched on it. Knobs and
//! faders act on the first move: the value jumps to the control position,
//! also after a layer change or a recall (no soft takeover).
//!
//! [`LiveControlSurface::map_into`] is the pure mapper (events → [`Act`]),
//! [`LiveControlSurface::render_feedback`] the pure feedback renderer; both
//! are tested without a running engine.

use std::time::{Duration, Instant};

use mesh_core::live::capture::CaptureScope;
use mesh_core::live::command::{LiveCommand, MoveKind};
use mesh_core::live::engine::LiveController;
use mesh_core::live::generator::layout::DrumVoice;
use mesh_core::live::state::{ClockView, LaneView, LiveShared, Rate};
use mesh_core::live::style::{TempoSpec, CONTROLS_PER_LANE};
use mesh_core::live::NUM_LANES;

use super::pads::{self, PadFn};
use mesh_core::music::MusicalKey;
use mesh_midi::types::{ControlAddress, ControlEvent, ControlValue, FeedbackCommand};

// ─── Layout ────────────────────────────────────────────────────────────

/// What a grid pad does inside its half.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PadRole {
    /// The lane's own pad function ([`pads`]).
    Lane(PadFn),
    Move(MoveKind),
    /// Dark, no function.
    Unused,
}

// PLAYABILITY: tune on hardware.
/// Pad `0..16` (`grid_1..=16`, row-major, top-left first) → (half, role).
/// Left two columns = half A, right two = half B. The lane pad index inside
/// a half is row-major: 1 2 / 3 4 / 5 6 / 7 8. While Sync is held
/// (`moves`): row 1 = stutter fill, row 2 = double void, rows 3–4 unused.
/// `drums_half`: the half that hosts the drum lane (left unit, half A).
fn pad_role(pad: usize, moves: bool, drums_half: Option<u8>, shift: bool) -> (u8, PadRole) {
    let (row, col) = (pad / 4, pad % 4);
    let half = (col / 2) as u8;
    let c = col % 2;
    let role = if !moves {
        match pads::pad_fn(drums_half == Some(half), row * 2 + c, shift) {
            PadFn::None => PadRole::Unused,
            f => PadRole::Lane(f),
        }
    } else {
        match (row, c) {
            (0, 0) => PadRole::Move(MoveKind::Stutter),
            (0, _) => PadRole::Move(MoveKind::Fill),
            (1, 0) => PadRole::Move(MoveKind::Double),
            (1, _) => PadRole::Move(MoveKind::Void),
            _ => PadRole::Unused,
        }
    };
    (half, role)
}

/// `knob_1..=4` → (half, lane control). Control order per lane:
/// knob A = 0, fader A = 1, knob B = 2, fader B = 3.
const KNOBS: [(u8, u8); 4] = [(0, 0), (0, 2), (1, 0), (1, 2)];
/// `fader_1..=4` → (half, lane control).
const FADERS: [(u8, u8); 4] = [(0, 1), (0, 3), (1, 1), (1, 3)];
/// `play_1..=4` → (half, is play button A). Button B rerolls the seed.
const PLAYS: [(u8, bool); 4] = [(0, true), (0, false), (1, true), (1, false)];

const GRID_NAMES: [&str; 16] = [
    "grid_1", "grid_2", "grid_3", "grid_4", "grid_5", "grid_6", "grid_7", "grid_8", "grid_9", "grid_10",
    "grid_11", "grid_12", "grid_13", "grid_14", "grid_15", "grid_16",
];
const PLAY_NAMES: [&str; 4] = ["play_1", "play_2", "play_3", "play_4"];
/// Function buttons with an LED, in [`FuncLed`] order.
const FUNC_NAMES: [&str; 8] = ["browse", "size", "type_btn", "reverse", "shift", "capture", "quant", "sync"];

// PLAYABILITY: tune on hardware.
/// Bars the Shift + fader 2 tempo ramp takes.
const TEMPO_RAMP_BARS: u32 = 4;

// PLAYABILITY: tune on hardware. Lane colours (RGB, 0..=0x7F).
const LANE_RGB: [[u8; 3]; NUM_LANES] = [
    [0x7F, 0x28, 0x00], // Drums: orange
    [0x00, 0x7F, 0x20], // L1: green
    [0x00, 0x40, 0x7F], // L2: blue
    [0x60, 0x00, 0x7F], // L3: purple
];
const WHITE: [u8; 3] = [0x7F, 0x7F, 0x7F];
// PLAYABILITY: tune on hardware. Pad brightness relative to the lane colour.
const PAD_MOVE_IDLE: f32 = 0.10;
/// Lane pads of a lane that is off.
const PAD_OFF_LANE: f32 = 0.3;
// PLAYABILITY: tune on hardware. Single-colour LED levels (0..=0x7F).
const LED_ON: u8 = 0x7F;
const LED_DIM: u8 = 0x0C;
const LED_OFF: u8 = 0x00;

/// Soft-takeover targets: 0..16 are `lane * 4 + control`, 16..32 the same
/// on the Shift layer, then the two global Shift-layer faders.
const TARGET_SHIFT_BASE: usize = NUM_LANES * CONTROLS_PER_LANE;
const TARGET_KEY_INTENT: usize = 2 * NUM_LANES * CONTROLS_PER_LANE;
const TARGET_TEMPO: usize = TARGET_KEY_INTENT + 1;
const NUM_TARGETS: usize = TARGET_TEMPO + 1;

// ─── Mapper output ─────────────────────────────────────────────────────

/// One surface action. Mirrors the [`LiveCommand`]s the surface sends
/// (comparable in tests) plus capture, which needs the controller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Act {
    /// A random new master seed (Play B).
    RandomizeSeed { lane: u8 },
    ToggleDrumVoice(DrumVoice),
    OctaveDelta { lane: u8, delta: i8 },
    ResetOctave { lane: u8 },
    /// Latch `rate`, or back to normal when it is already on.
    ToggleRate { lane: u8, rate: Rate },
    FormDelta { lane: u8, delta: i32 },
    /// `control` 0..4 main layer, 4..8 Shift layer.
    SetControl { lane: u8, control: u8, value: f32 },
    ToggleLane { lane: u8 },
    BrowseOption { lane: u8, delta: i32 },
    CommitOption { lane: u8 },
    MovePress { lane: u8, kind: MoveKind },
    MoveRelease { lane: u8, kind: MoveKind },
    SetKeyIntent(f32),
    CommitKey,
    ToggleMinor,
    RandomizeHarmony,
    RequestTempo { bpm: f64, bars: u32 },
    SetQuantize(bool),
    ToggleHalfTime,
    Capture(CaptureScope),
}

fn apply(act: Act, ctl: &mut LiveController) {
    let cmd = match act {
        Act::RandomizeSeed { lane } => LiveCommand::RandomizeSeed { lane },
        Act::ToggleDrumVoice(voice) => LiveCommand::ToggleDrumVoice { voice },
        Act::OctaveDelta { lane, delta } => LiveCommand::OctaveDelta { lane, delta },
        Act::ResetOctave { lane } => LiveCommand::ResetOctave { lane },
        Act::ToggleRate { lane, rate } => {
            let current = ctl.lane(lane as usize).rate;
            LiveCommand::SetRate { lane, rate: if current == rate { Rate::Normal } else { rate } }
        }
        Act::FormDelta { lane, delta } => LiveCommand::FormDelta { lane, delta },
        Act::SetControl { lane, control, value } => LiveCommand::SetControl { lane, control, value },
        Act::ToggleLane { lane } => LiveCommand::ToggleLane { lane },
        Act::BrowseOption { lane, delta } => LiveCommand::BrowseOption { lane, delta },
        Act::CommitOption { lane } => LiveCommand::CommitOption { lane },
        Act::MovePress { lane, kind } => LiveCommand::MovePress { lane, kind },
        Act::MoveRelease { lane, kind } => LiveCommand::MoveRelease { lane, kind },
        Act::SetKeyIntent(position) => LiveCommand::SetKeyIntent(position),
        Act::CommitKey => LiveCommand::CommitKey,
        Act::RequestTempo { bpm, bars } => LiveCommand::RequestTempo { bpm, bars },
        Act::SetQuantize(on) => LiveCommand::SetQuantize(on),
        Act::ToggleHalfTime => LiveCommand::ToggleHalfTime,
        Act::ToggleMinor => LiveCommand::ToggleMinor,
        Act::RandomizeHarmony => LiveCommand::RandomizeHarmony,
        Act::Capture(scope) => {
            // Adds to the bank and saves to the collection off-thread.
            ctl.capture(scope);
            return;
        }
    };
    ctl.send(cmd);
}

// ─── Surface state ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

impl Side {
    fn lane(self, half: u8) -> u8 {
        match self {
            Side::Left => half,
            Side::Right => 2 + half,
        }
    }
}

/// A raw F1 control, parsed from its HID name without allocating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    Pad(usize),
    Play(usize),
    Knob(usize),
    Fader(usize),
    Encoder,
    EncoderPush,
    Shift,
    Capture,
    Quant,
    Sync,
    Reverse,
    Browse,
    Size,
    Type,
}

impl Control {
    fn parse(name: &str) -> Option<Self> {
        let indexed = |prefix: &str, n: usize| -> Option<usize> {
            let i: usize = name.strip_prefix(prefix)?.parse().ok()?;
            (1..=n).contains(&i).then_some(i - 1)
        };
        Some(match name {
            "encoder" => Control::Encoder,
            "encoder_push" => Control::EncoderPush,
            "shift" => Control::Shift,
            "capture" => Control::Capture,
            "quant" => Control::Quant,
            "sync" => Control::Sync,
            "reverse" => Control::Reverse,
            "browse" => Control::Browse,
            "size" => Control::Size,
            "type_btn" => Control::Type,
            _ => {
                if let Some(i) = indexed("grid_", 16) {
                    Control::Pad(i)
                } else if let Some(i) = indexed("play_", 4) {
                    Control::Play(i)
                } else if let Some(i) = indexed("knob_", 4) {
                    Control::Knob(i)
                } else if let Some(i) = indexed("fader_", 4) {
                    Control::Fader(i)
                } else {
                    return None;
                }
            }
        })
    }
}

/// Size (lane A) or Browse (lane B) held down for option browsing.
#[derive(Debug, Clone, Copy)]
struct BrowseHold {
    since: Instant,
    browsed: bool,
}

/// Change-tracked LED and display state of one unit (`None` = unknown).
#[derive(Debug, Clone, Default)]
struct LedCache {
    pads: [Option<[u8; 3]>; 16],
    plays: [Option<u8>; 4],
    funcs: [Option<u8>; 8],
    display: Option<[u8; 2]>,
}

/// One connected F1.
#[derive(Debug, Clone)]
struct Unit {
    id: String,
    shift: bool,
    /// Half last touched (its lane is the unit's selected lane).
    half: u8,
    browse_held: [Option<BrowseHold>; 2],
    /// Play button B held per half (the display shows the new seed).
    seed_held: [bool; 2],
    /// Sync held: pads are momentary moves.
    sync: bool,
    /// Per pad, what it did when pressed (half, role), so its release
    /// matches the press even if Sync or Shift changed in between.
    pads_down: [Option<(u8, PadRole)>; 16],
    leds: LedCache,
}

impl Unit {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            shift: false,
            half: 0,
            browse_held: [None; 2],
            seed_held: [false; 2],
            sync: false,
            pads_down: [None; 16],
            leds: LedCache::default(),
        }
    }
}

/// Current value of a control target.
#[derive(Debug, Clone, Copy, Default)]
struct Target {
    /// Value, `0..=1` (fader position).
    value: f32,
    /// Value has been read from the engine.
    known: bool,
}

/// What the UI should highlight (selected lane/slot, shift state).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SurfaceView {
    pub selected_lane: u8,
    pub shift: bool,
    /// Lane whose play button is held while browsing options.
    pub browse_lane: Option<u8>,
}

/// Engine state the feedback renderer reads, snapshotted once per call.
#[derive(Debug, Clone, Copy)]
pub struct FeedbackInput {
    pub clock: ClockView,
    pub key: MusicalKey,
    /// Key-intent target while the intent fader is off-centre.
    pub intent_key: Option<MusicalKey>,
    pub quantize: bool,
    pub half_time: bool,
    pub lanes: [LaneView; NUM_LANES],
}

impl FeedbackInput {
    pub fn from_shared(shared: &LiveShared) -> Self {
        Self {
            clock: shared.clock(),
            key: shared.key(),
            intent_key: shared.intent_key(),
            quantize: shared.quantize(),
            half_time: shared.half_time_target(),
            lanes: std::array::from_fn(|i| shared.lane(i).view()),
        }
    }
}

/// The two-F1 live control surface.
pub struct LiveControlSurface {
    view: SurfaceView,
    left_serial: Option<String>,
    /// Device locked as left by the first Shift press (no serial configured).
    left_locked: Option<String>,
    /// When set, events from other devices are ignored.
    allowed: Option<Vec<String>>,
    units: Vec<Unit>,
    targets: [Target; NUM_TARGETS],
    tempo: Option<TempoSpec>,
    /// Engine tempo target seen at the last sync (edge detection).
    seen_target_bpm: Option<f64>,
    /// Last tempo this surface requested.
    tempo_requested: Option<f64>,
    quantize: bool,
    /// Reused mapper output buffer.
    out: Vec<Act>,
}

impl LiveControlSurface {
    /// `left_serial`: HID serial of the left F1 (`live.f1_left_serial`).
    pub fn new(left_serial: Option<String>) -> Self {
        Self {
            view: SurfaceView::default(),
            left_serial: left_serial.filter(|s| !s.is_empty()),
            left_locked: None,
            allowed: None,
            units: Vec::with_capacity(2),
            targets: [Target::default(); NUM_TARGETS],
            tempo: None,
            seen_target_bpm: None,
            tempo_requested: None,
            quantize: false,
            out: Vec::with_capacity(4),
        }
    }

    /// Restrict the surface to these HID devices (F1 device ids). Without
    /// it, the first two devices that send events are used.
    pub fn set_devices(&mut self, ids: &[String]) {
        self.allowed = Some(ids.to_vec());
        self.units.retain(|u| ids.contains(&u.id));
        for id in ids.iter().take(2) {
            if !self.units.iter().any(|u| &u.id == id) && self.units.len() < 2 {
                self.units.push(Unit::new(id));
            }
        }
    }

    /// Forget the LED cache (e.g. after a reconnect) so the next
    /// [`feedback`](Self::feedback) resends everything.
    pub fn invalidate_feedback(&mut self) {
        for u in &mut self.units {
            u.leds = LedCache::default();
        }
    }

    /// Handle one raw controller event, sending commands through `ctl`.
    pub fn handle(&mut self, ev: &ControlEvent, ctl: &mut LiveController) {
        self.sync_from(ctl);
        let mut out = std::mem::take(&mut self.out);
        out.clear();
        self.map_into(ev, Instant::now(), &mut out);
        for act in out.drain(..) {
            apply(act, ctl);
        }
        self.out = out;
    }

    /// Called at ~30 Hz from the UI tick. Appends LED commands and
    /// 7-segment texts (device id, command / text).
    pub fn feedback(
        &mut self,
        shared: &LiveShared,
        ctl: &LiveController,
        leds: &mut Vec<(String, FeedbackCommand)>,
        displays: &mut Vec<(String, String)>,
    ) {
        self.sync_from(ctl);
        let input = FeedbackInput::from_shared(shared);
        self.render_feedback(&input, leds, displays);
    }

    pub fn view(&self) -> SurfaceView {
        self.view
    }

    // ─── Engine sync ───────────────────────────────────────────────────

    fn sync_from(&mut self, ctl: &LiveController) {
        let positions: [[f32; 2 * CONTROLS_PER_LANE]; NUM_LANES] = std::array::from_fn(|l| {
            let lane = ctl.lane(l);
            std::array::from_fn(|c| {
                if c < CONTROLS_PER_LANE { lane.positions[c] } else { lane.shift_positions[c - CONTROLS_PER_LANE] }
            })
        });
        let shared = ctl.shared();
        self.sync_state(&positions, ctl.tempo_range(), shared.clock().target_bpm, shared.quantize());
    }

    /// Take over engine values (recall, UI).
    /// `positions`: per lane, main then Shift-layer controls.
    fn sync_state(
        &mut self,
        positions: &[[f32; 2 * CONTROLS_PER_LANE]; NUM_LANES],
        tempo: TempoSpec,
        target_bpm: f64,
        quantize: bool,
    ) {
        for (lane, controls) in positions.iter().enumerate() {
            for (control, &value) in controls.iter().enumerate() {
                let index = if control < CONTROLS_PER_LANE {
                    lane * CONTROLS_PER_LANE + control
                } else {
                    TARGET_SHIFT_BASE + lane * CONTROLS_PER_LANE + control - CONTROLS_PER_LANE
                };
                let t = &mut self.targets[index];
                if !t.known || (t.value - value).abs() > 1e-4 {
                    *t = Target { value, known: true };
                }
            }
        }
        self.tempo = Some(tempo);
        if self.seen_target_bpm != Some(target_bpm) {
            self.seen_target_bpm = Some(target_bpm);
            let ours = self.tempo_requested.is_some_and(|r| (r - target_bpm).abs() < 0.5);
            let t = &mut self.targets[TARGET_TEMPO];
            if !ours || !t.known {
                *t = Target { value: tempo_position(tempo, target_bpm), known: true };
            }
        }
        if !self.targets[TARGET_KEY_INTENT].known {
            // The intent fader rests at the centre (no transition).
            self.targets[TARGET_KEY_INTENT] = Target { value: 0.5, known: true };
        }
        self.quantize = quantize;
    }

    // ─── Mapping ───────────────────────────────────────────────────────

    /// Index of the unit for `id`, registering it if there is room.
    fn unit_index(&mut self, id: &str) -> Option<usize> {
        if let Some(i) = self.units.iter().position(|u| u.id == id) {
            return Some(i);
        }
        if self.allowed.as_ref().is_some_and(|a| !a.iter().any(|x| x == id)) || self.units.len() >= 2 {
            return None;
        }
        self.units.push(Unit::new(id));
        Some(self.units.len() - 1)
    }

    /// Left unit: the configured serial if connected (a lone other unit is
    /// then right). Without a serial, or when two units are connected and
    /// neither matches it: the first unit whose Shift was pressed, else the
    /// lowest device id.
    fn left_index(&self) -> Option<usize> {
        let by_id = |id: &Option<String>| {
            id.as_ref().and_then(|id| self.units.iter().position(|u| &u.id == id))
        };
        if self.left_serial.is_some() {
            if let Some(i) = by_id(&self.left_serial) {
                return Some(i);
            }
            if self.units.len() < 2 {
                return None;
            }
        }
        by_id(&self.left_locked).or_else(|| {
            (0..self.units.len()).min_by(|&a, &b| self.units[a].id.cmp(&self.units[b].id))
        })
    }

    fn side(&self, unit: usize) -> Side {
        if self.left_index() == Some(unit) { Side::Left } else { Side::Right }
    }

    /// Map one event to actions (appended to `out`). `now` times play-button taps.
    fn map_into(&mut self, ev: &ControlEvent, now: Instant, out: &mut Vec<Act>) {
        let ControlAddress::Hid { device_id, name } = &ev.address else { return };
        let Some(control) = Control::parse(name) else { return };
        let Some(u) = self.unit_index(device_id) else { return };

        if control == Control::Shift && ev.value.is_press() && self.left_locked.is_none() {
            let serial_connected =
                self.left_serial.as_ref().is_some_and(|s| self.units.iter().any(|x| &x.id == s));
            if !serial_connected {
                self.left_locked = Some(self.units[u].id.clone());
            }
        }
        let side = self.side(u);

        match (control, &ev.value) {
            (Control::Knob(i), ControlValue::Absolute(v)) => {
                let (half, c) = KNOBS[i];
                self.analog(u, side, i, half, c, *v as f32, out);
            }
            (Control::Fader(i), ControlValue::Absolute(v)) => {
                let (half, c) = FADERS[i];
                self.analog(u, side, 4 + i, half, c, *v as f32, out);
            }
            (Control::Encoder, ControlValue::Relative(d)) => self.encoder(u, side, *d, out),
            (_, ControlValue::Button(pressed)) => self.button(u, side, control, *pressed, now, out),
            _ => {}
        }

        self.view.shift = self.units.iter().any(|x| x.shift);
        self.view.browse_lane = self.units.iter().enumerate().find_map(|(i, x)| {
            let half = x.browse_held.iter().position(|h| h.is_some())?;
            Some(self.side(i).lane(half as u8))
        });
    }

    fn touch(&mut self, u: usize, side: Side, half: u8) {
        self.units[u].half = half;
        self.view.selected_lane = side.lane(half);
    }

    #[allow(clippy::too_many_arguments)]
    fn analog(&mut self, u: usize, side: Side, phys: usize, half: u8, control: u8, v: f32, out: &mut Vec<Act>) {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { return };
        let shift = self.units[u].shift;
        let global = side == Side::Left && shift;
        let target = match phys {
            4 if global => TARGET_KEY_INTENT,
            5 if global => TARGET_TEMPO,
            _ => {
                self.touch(u, side, half);
                let base = if shift { TARGET_SHIFT_BASE } else { 0 };
                base + side.lane(half) as usize * CONTROLS_PER_LANE + control as usize
            }
        };

        let t = &mut self.targets[target];
        t.value = v;
        t.known = true;

        match target {
            TARGET_KEY_INTENT => out.push(Act::SetKeyIntent(v * 2.0 - 1.0)),
            TARGET_TEMPO => {
                let Some(range) = self.tempo else { return };
                let bpm = (range.min + v as f64 * (range.max - range.min)).round();
                if self.tempo_requested != Some(bpm) {
                    self.tempo_requested = Some(bpm);
                    out.push(Act::RequestTempo { bpm, bars: TEMPO_RAMP_BARS });
                }
            }
            t if t >= TARGET_SHIFT_BASE => out.push(Act::SetControl {
                lane: side.lane(half),
                control: CONTROLS_PER_LANE as u8 + control,
                value: v,
            }),
            _ => out.push(Act::SetControl { lane: side.lane(half), control, value: v }),
        }
    }

    fn encoder(&mut self, u: usize, side: Side, delta: i32, out: &mut Vec<Act>) {
        if delta == 0 {
            return;
        }
        let unit = &mut self.units[u];
        // The most recently pressed held Size/Browse button browses its lane.
        let held = (0..2u8)
            .filter_map(|h| unit.browse_held[h as usize].map(|p| (h, p.since)))
            .max_by_key(|&(_, since)| since);
        if let Some((half, _)) = held {
            if let Some(p) = unit.browse_held[half as usize].as_mut() {
                p.browsed = true;
            }
            out.push(Act::BrowseOption { lane: side.lane(half), delta });
        }
        // Nothing held: the encoder does nothing, so a nudge never rerolls a lane.
    }

    fn button(&mut self, u: usize, side: Side, control: Control, pressed: bool, now: Instant, out: &mut Vec<Act>) {
        let selected = side.lane(self.units[u].half);
        match control {
            Control::Shift => self.units[u].shift = pressed,
            Control::Pad(i) => self.pad(u, side, i, pressed, out),
            // Hold Sync: the unit's pads become momentary moves. Release
            // returns to the bar pads and releases every move still held.
            Control::Sync => {
                let unit = &mut self.units[u];
                unit.sync = pressed;
                if !pressed {
                    for down in &mut unit.pads_down {
                        if let Some((half, PadRole::Move(kind))) = *down {
                            *down = None;
                            out.push(Act::MoveRelease { lane: side.lane(half), kind });
                        }
                    }
                }
            }
            // Hold Size (lane A) or Browse (lane B) and turn the encoder to
            // browse kits/instruments; release loads the choice at the bar.
            Control::Size | Control::Browse => {
                let half = if control == Control::Size { 0 } else { 1 };
                let lane = side.lane(half);
                if pressed {
                    self.touch(u, side, half);
                }
                let slot = &mut self.units[u].browse_held[half as usize];
                if pressed {
                    *slot = Some(BrowseHold { since: now, browsed: false });
                } else if let Some(hold) = slot.take() {
                    if hold.browsed {
                        out.push(Act::CommitOption { lane });
                    }
                }
            }
            Control::Play(i) => {
                let (half, is_a) = PLAYS[i];
                if pressed {
                    self.touch(u, side, half);
                }
                let lane = side.lane(half);
                if !is_a {
                    // Play button B: a press rolls a random new seed.
                    self.units[u].seed_held[half as usize] = pressed;
                    if pressed {
                        out.push(Act::RandomizeSeed { lane });
                    }
                    return;
                }
                if pressed {
                    out.push(Act::ToggleLane { lane });
                }
            }
            _ if !pressed => {}
            Control::EncoderPush => {
                if side == Side::Left && self.units[u].shift {
                    out.push(Act::CommitKey);
                }
            }
            Control::Quant => {
                self.quantize = !self.quantize;
                out.push(Act::SetQuantize(self.quantize));
            }
            Control::Capture => {
                let scope = if self.units[u].shift { CaptureScope::All } else { CaptureScope::Lane { lane: selected } };
                out.push(Act::Capture(scope));
            }
            Control::Reverse => out.push(Act::ToggleHalfTime),
            Control::Type if pressed => {
                out.push(if self.units[u].shift { Act::RandomizeHarmony } else { Act::ToggleMinor });
            }
            Control::Type => {}
            Control::Knob(_) | Control::Fader(_) | Control::Encoder => {}
        }
    }

    /// Half of `side` that hosts the drum lane.
    fn drums_half(side: Side) -> Option<u8> {
        (side == Side::Left).then_some(0)
    }

    fn pad(&mut self, u: usize, side: Side, pad: usize, pressed: bool, out: &mut Vec<Act>) {
        if pressed {
            let unit = &self.units[u];
            let (half, role) = pad_role(pad, unit.sync, Self::drums_half(side), unit.shift);
            let lane = side.lane(half);
            self.touch(u, side, half);
            let unit = &mut self.units[u];
            match role {
                PadRole::Lane(f) => {
                    // The other octave pad of this half held too: back to 0.
                    let other_octave = unit.pads_down.iter().flatten().any(|&(h, r)| {
                        h == half
                            && matches!((f, r), (PadFn::OctaveDown, PadRole::Lane(PadFn::OctaveUp)) | (PadFn::OctaveUp, PadRole::Lane(PadFn::OctaveDown)))
                    });
                    out.push(match f {
                        PadFn::Voice(voice) => Act::ToggleDrumVoice(voice),
                        PadFn::OctaveDown | PadFn::OctaveUp if other_octave => Act::ResetOctave { lane },
                        PadFn::OctaveDown => Act::OctaveDelta { lane, delta: -1 },
                        PadFn::OctaveUp => Act::OctaveDelta { lane, delta: 1 },
                        PadFn::Rate(rate) => Act::ToggleRate { lane, rate },
                        PadFn::FormPrev => Act::FormDelta { lane, delta: -1 },
                        PadFn::FormNext => Act::FormDelta { lane, delta: 1 },
                        PadFn::None => return,
                    });
                }
                PadRole::Move(kind) => out.push(Act::MovePress { lane, kind }),
                PadRole::Unused => {}
            }
            unit.pads_down[pad] = Some((half, role));
        } else {
            let unit = &mut self.units[u];
            if let Some((half, PadRole::Move(kind))) = unit.pads_down[pad].take() {
                out.push(Act::MoveRelease { lane: side.lane(half), kind });
            }
        }
    }

    // ─── Feedback ──────────────────────────────────────────────────────

    /// Emit the LED / display changes since the last call.
    fn render_feedback(
        &mut self,
        input: &FeedbackInput,
        leds: &mut Vec<(String, FeedbackCommand)>,
        displays: &mut Vec<(String, String)>,
    ) {
        let step = input.clock.step;
        let blink_on = (step / 2) % 2 == 0;
        let left = self.left_index();
        let left_shift = left.is_some_and(|l| self.units[l].shift);

        for ui in 0..self.units.len() {
            let side = if left == Some(ui) { Side::Left } else { Side::Right };
            let unit = &mut self.units[ui];
            let id = &unit.id;

            for pad in 0..GRID_NAMES.len() {
                let (half, role) = pad_role(pad, unit.sync, Self::drums_half(side), unit.shift);
                let lane = side.lane(half) as usize;
                let rgb = pad_rgb(&input.lanes[lane], lane, role, blink_on);
                if unit.leds.pads[pad] != Some(rgb) {
                    unit.leds.pads[pad] = Some(rgb);
                    let [r, g, b] = rgb;
                    leds.push((id.clone(), FeedbackCommand::SetRgb { control: GRID_NAMES[pad].to_string(), r, g, b }));
                }
            }

            for (i, &(half, is_a)) in PLAYS.iter().enumerate() {
                let level = if is_a {
                    let l = &input.lanes[side.lane(half) as usize];
                    if l.on != l.on_target {
                        if blink_on { LED_ON } else { LED_OFF }
                    } else if l.on {
                        LED_ON
                    } else {
                        LED_DIM
                    }
                } else if unit.seed_held[half as usize] {
                    LED_ON
                } else {
                    LED_DIM
                };
                if unit.leds.plays[i] != Some(level) {
                    unit.leds.plays[i] = Some(level);
                    leds.push((id.clone(), FeedbackCommand::SetLed { control: PLAY_NAMES[i].to_string(), brightness: level }));
                }
            }

            let lit = |on: bool| if on { LED_ON } else { LED_DIM };
            // Order of FUNC_NAMES: browse size type reverse shift capture quant sync.
            let funcs = [
                lit(unit.browse_held[1].is_some()),
                lit(unit.browse_held[0].is_some()),
                LED_OFF,
                lit(input.half_time),
                lit(unit.shift),
                LED_DIM,
                lit(input.quantize),
                lit(unit.sync),
            ];
            for (i, &level) in funcs.iter().enumerate() {
                if unit.leds.funcs[i] != Some(level) {
                    unit.leds.funcs[i] = Some(level);
                    leds.push((id.clone(), FeedbackCommand::SetLed { control: FUNC_NAMES[i].to_string(), brightness: level }));
                }
            }

            // While Play B is held, show that lane's seed (last 2 digits).
            let held_seed =
                (0..2u8).find(|&h| unit.seed_held[h as usize]).map(|h| input.lanes[side.lane(h) as usize].seed);
            let text = match (held_seed, side) {
                (Some(seed), _) => tempo_digits((seed % 100) as f64),
                (None, Side::Left) => key_digits(input.intent_key.unwrap_or(input.key)),
                (None, Side::Right) => {
                    let bpm = match self.tempo_requested {
                        Some(requested) if left_shift => requested,
                        _ if input.clock.ramping => input.clock.target_bpm,
                        _ => input.clock.bpm,
                    };
                    tempo_digits(bpm)
                }
            };
            if unit.leds.display != Some(text) {
                unit.leds.display = Some(text);
                displays.push((id.clone(), text.iter().map(|&c| c as char).collect()));
            }
        }
    }
}

/// Fader position of `bpm` inside the style's tempo range.
fn tempo_position(range: TempoSpec, bpm: f64) -> f32 {
    let span = range.max - range.min;
    if span <= 0.0 { 0.5 } else { ((bpm - range.min) / span).clamp(0.0, 1.0) as f32 }
}

fn scale(rgb: [u8; 3], f: f32) -> [u8; 3] {
    rgb.map(|c| (c as f32 * f).round().min(127.0) as u8)
}

fn pad_rgb(view: &LaneView, lane: usize, role: PadRole, blink_on: bool) -> [u8; 3] {
    let base = LANE_RGB[lane];
    match role {
        PadRole::Lane(f) => {
            let level = pads::shown_level(pads::look(view, f), blink_on);
            let level = if view.on { level } else { level * PAD_OFF_LANE };
            scale(base, level)
        }
        PadRole::Move(kind) => {
            if view.moves & kind.bit() != 0 {
                WHITE
            } else {
                scale(base, if view.on { PAD_MOVE_IDLE } else { PAD_MOVE_IDLE * PAD_OFF_LANE })
            }
        }
        PadRole::Unused => [0, 0, 0],
    }
}

/// Camelot key on the 2-digit display. Positions 1–9 show as the digit,
/// 10–12 as hex `A`/`B`/`C`; the second digit is the letter (`A` minor,
/// `B` major, rendered `b`). So 8A → "8A", 8B → "8b", 10A → "AA",
/// 11B → "bb", 12A → "CA". A leading A–C always means 10–12.
fn key_digits(key: MusicalKey) -> [u8; 2] {
    let (n, letter) = key.camelot();
    let first = match n {
        1..=9 => b'0' + n,
        10 => b'A',
        11 => b'B',
        12 => b'C',
        _ => b'-',
    };
    [first, letter as u8]
}

/// Last two digits of the rounded tempo (172 → "72", 100 → "00").
fn tempo_digits(bpm: f64) -> [u8; 2] {
    if !bpm.is_finite() || bpm < 0.0 {
        return [b'-', b'-'];
    }
    let n = (bpm.round() as u64 % 100) as u8;
    [b'0' + n / 10, b'0' + n % 10]
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "F1-AAA";
    const B: &str = "F1-BBB";

    fn ev(dev: &str, name: &str, value: ControlValue) -> ControlEvent {
        ControlEvent { address: ControlAddress::Hid { device_id: dev.into(), name: name.into() }, value }
    }

    struct Rig {
        s: LiveControlSurface,
        t0: Instant,
    }

    impl Rig {
        fn new(left: Option<&str>) -> Self {
            Self { s: LiveControlSurface::new(left.map(String::from)), t0: Instant::now() }
        }

        fn at(&mut self, ms: u64, dev: &str, name: &str, value: ControlValue) -> Vec<Act> {
            let mut out = Vec::new();
            self.s.map_into(&ev(dev, name, value), self.t0 + Duration::from_millis(ms), &mut out);
            out
        }

        fn send(&mut self, dev: &str, name: &str, value: ControlValue) -> Vec<Act> {
            self.at(0, dev, name, value)
        }

        fn press(&mut self, dev: &str, name: &str) -> Vec<Act> {
            self.send(dev, name, ControlValue::Button(true))
        }

        fn release(&mut self, dev: &str, name: &str) -> Vec<Act> {
            self.send(dev, name, ControlValue::Button(false))
        }

        fn abs(&mut self, dev: &str, name: &str, v: f64) -> Vec<Act> {
            self.send(dev, name, ControlValue::Absolute(v))
        }

        fn enc(&mut self, dev: &str, d: i32) -> Vec<Act> {
            self.send(dev, "encoder", ControlValue::Relative(d))
        }

        /// Register both units (A is left by sorted id unless configured).
        fn both(mut self) -> Self {
            self.release(A, "browse");
            self.release(B, "browse");
            self
        }

        /// Main-layer positions; the Shift layer sits at 0.5.
        fn sync(&mut self, positions: [[f32; 4]; 4], target_bpm: f64) {
            let tempo = TempoSpec { default: 170.0, min: 160.0, max: 180.0 };
            let all: [[f32; 8]; 4] = positions.map(|l| std::array::from_fn(|c| if c < 4 { l[c] } else { 0.5 }));
            self.s.sync_state(&all, tempo, target_bpm, false);
        }
    }

    fn set(lane: u8, control: u8, value: f32) -> Vec<Act> {
        vec![Act::SetControl { lane, control, value }]
    }

    #[test]
    fn lane_and_control_mapping_on_both_units() {
        let mut r = Rig::new(None).both();
        let expect = [
            ("knob_1", 0, 0),
            ("fader_1", 0, 1),
            ("knob_2", 0, 2),
            ("fader_2", 0, 3),
            ("knob_3", 1, 0),
            ("fader_3", 1, 1),
            ("knob_4", 1, 2),
            ("fader_4", 1, 3),
        ];
        for (name, lane, control) in expect {
            assert_eq!(r.abs(A, name, 0.25), set(lane, control, 0.25), "left {name}");
            assert_eq!(r.abs(B, name, 0.75), set(lane + 2, control, 0.75), "right {name}");
        }
        assert_eq!(r.s.view().selected_lane, 3, "last touched lane");
    }

    #[test]
    fn play_b_press_rolls_a_new_seed() {
        let mut r = Rig::new(None).both();
        assert_eq!(r.enc(A, 1), vec![], "the encoder alone does nothing");
        assert_eq!(r.press(A, "play_2"), vec![Act::RandomizeSeed { lane: 0 }]);
        assert_eq!(r.enc(A, 2), vec![], "no hold + encoder any more");
        assert_eq!(r.release(A, "play_2"), vec![]);
        assert_eq!(r.press(B, "play_4"), vec![Act::RandomizeSeed { lane: 3 }]);
        assert_eq!(r.release(B, "play_4"), vec![]);
        assert_eq!(r.press(A, "play_4"), vec![Act::RandomizeSeed { lane: 1 }]);
    }

    #[test]
    fn play_toggles_and_size_browse_buttons_browse() {
        let mut r = Rig::new(None).both();
        let (p, rel) = (ControlValue::Button(true), ControlValue::Button(false));
        // Play A toggles on press.
        assert_eq!(r.at(0, A, "play_1", p.clone()), vec![Act::ToggleLane { lane: 0 }]);
        assert_eq!(r.at(900, A, "play_1", rel.clone()), vec![]);
        // Holding Size without turning: nothing.
        r.at(1000, A, "size", p.clone());
        assert_eq!(r.at(1300, A, "size", rel.clone()), vec![]);
        // Size = lane A, Browse = lane B; release commits.
        r.at(2000, A, "size", p.clone());
        assert_eq!(r.s.view().browse_lane, Some(0));
        assert_eq!(r.enc(A, 1), vec![Act::BrowseOption { lane: 0, delta: 1 }]);
        assert_eq!(r.at(2100, A, "size", rel.clone()), vec![Act::CommitOption { lane: 0 }]);
        r.at(2200, B, "browse", p.clone());
        assert_eq!(r.s.view().browse_lane, Some(3));
        assert_eq!(r.enc(B, -1), vec![Act::BrowseOption { lane: 3, delta: -1 }]);
        assert_eq!(r.at(2300, B, "browse", rel.clone()), vec![Act::CommitOption { lane: 3 }]);
        assert_eq!(r.s.view().browse_lane, None);
        assert_eq!(r.enc(B, 1), vec![], "browse ended");
        // Play button B never toggles the lane: it rolls a new seed.
        assert_eq!(r.at(3000, A, "play_2", p), vec![Act::RandomizeSeed { lane: 0 }]);
        assert_eq!(r.at(3050, A, "play_2", rel), vec![]);
    }

    #[test]
    fn drum_pads_mute_voices_and_pitched_pads_play() {
        let mut r = Rig::new(None).both();
        // Left unit, half A = drums: row-major voice order.
        let drum_pads = ["grid_1", "grid_2", "grid_5", "grid_6", "grid_9", "grid_10", "grid_13", "grid_14"];
        for (pad, voice) in drum_pads.iter().zip(pads::DRUM_PADS) {
            assert_eq!(r.press(A, pad), vec![Act::ToggleDrumVoice(voice)], "{pad}");
            assert_eq!(r.release(A, pad), vec![]);
        }
        // Every pitched half: oct− oct+ / half dbl / form◀ form▶ / unused.
        for (dev, col, lane) in [(A, 2usize, 1u8), (B, 0, 2), (B, 2, 3)] {
            let pad = |row: usize, c: usize| GRID_NAMES[row * 4 + col + c];
            assert_eq!(r.press(dev, pad(0, 0)), vec![Act::OctaveDelta { lane, delta: -1 }]);
            r.release(dev, pad(0, 0));
            assert_eq!(r.press(dev, pad(0, 1)), vec![Act::OctaveDelta { lane, delta: 1 }]);
            r.release(dev, pad(0, 1));
            assert_eq!(r.press(dev, pad(1, 0)), vec![Act::ToggleRate { lane, rate: Rate::Half }]);
            r.release(dev, pad(1, 0));
            assert_eq!(r.press(dev, pad(1, 1)), vec![Act::ToggleRate { lane, rate: Rate::Double }]);
            r.release(dev, pad(1, 1));
            assert_eq!(r.press(dev, pad(2, 0)), vec![Act::FormDelta { lane, delta: -1 }]);
            r.release(dev, pad(2, 0));
            assert_eq!(r.press(dev, pad(2, 1)), vec![Act::FormDelta { lane, delta: 1 }]);
            r.release(dev, pad(2, 1));
            assert_eq!(r.press(dev, pad(3, 0)), vec![]);
            assert_eq!(r.press(dev, pad(3, 1)), vec![]);
            r.release(dev, pad(3, 0));
            r.release(dev, pad(3, 1));
            // Both octave pads: back to 0.
            r.press(dev, pad(0, 0));
            assert_eq!(r.press(dev, pad(0, 1)), vec![Act::ResetOctave { lane }]);
            r.release(dev, pad(0, 0));
            r.release(dev, pad(0, 1));
        }
    }

    #[test]
    fn shift_and_drum_pads_7_8_step_forms() {
        let mut r = Rig::new(None).both();
        r.press(A, "shift");
        assert_eq!(r.press(A, "grid_13"), vec![Act::FormDelta { lane: 0, delta: -1 }]);
        assert_eq!(r.press(A, "grid_14"), vec![Act::FormDelta { lane: 0, delta: 1 }]);
        assert_eq!(r.press(A, "grid_1"), vec![], "Shift leaves the voice pads alone");
        // Shift is unit-local and does not change pitched pads.
        assert_eq!(r.press(B, "grid_1"), vec![Act::OctaveDelta { lane: 2, delta: -1 }]);
        r.release(A, "shift");
        assert_eq!(r.release(A, "grid_13"), vec![]);
        assert_eq!(r.press(A, "grid_13"), vec![Act::ToggleDrumVoice(DrumVoice::Kick)], "kick bottom left");
    }

    #[test]
    fn sync_held_turns_pads_into_moves() {
        let mut r = Rig::new(None).both();
        assert_eq!(r.press(A, "sync"), vec![], "Sync never recalls");
        let cases = [
            (A, "grid_1", 0, MoveKind::Stutter),
            (A, "grid_2", 0, MoveKind::Fill),
            (A, "grid_5", 0, MoveKind::Double),
            (A, "grid_6", 0, MoveKind::Void),
            (A, "grid_3", 1, MoveKind::Stutter),
            (A, "grid_8", 1, MoveKind::Void),
        ];
        for (dev, pad, lane, kind) in cases {
            assert_eq!(r.press(dev, pad), vec![Act::MovePress { lane, kind }], "{pad}");
            assert_eq!(r.release(dev, pad), vec![Act::MoveRelease { lane, kind }], "{pad}");
        }
        for pad in ["grid_9", "grid_10", "grid_12", "grid_15", "grid_16"] {
            assert_eq!(r.press(A, pad), vec![], "{pad}: rows 3-4 unused");
            assert_eq!(r.release(A, pad), vec![]);
        }
        // Only the unit whose Sync is held switches.
        assert_eq!(r.press(B, "grid_1"), vec![Act::OctaveDelta { lane: 2, delta: -1 }]);
        r.release(B, "grid_1");
        assert_eq!(r.release(A, "sync"), vec![], "nothing held, nothing released");
        assert_eq!(r.press(A, "grid_1"), vec![Act::ToggleDrumVoice(DrumVoice::Top)], "back to the lane pads");
        r.release(A, "grid_1");
        r.press(B, "sync");
        assert_eq!(r.press(B, "grid_7"), vec![Act::MovePress { lane: 3, kind: MoveKind::Double }]);
        assert_eq!(r.release(B, "grid_7"), vec![Act::MoveRelease { lane: 3, kind: MoveKind::Double }]);
    }

    #[test]
    fn sync_release_releases_held_moves() {
        let mut r = Rig::new(None).both();
        r.press(A, "sync");
        r.press(A, "grid_1"); // lane 0 stutter
        r.press(A, "grid_4"); // lane 1 fill
        r.press(A, "grid_9"); // unused
        let mut released = r.release(A, "sync");
        released.sort_by_key(|a| format!("{a:?}"));
        assert_eq!(
            released,
            vec![Act::MoveRelease { lane: 0, kind: MoveKind::Stutter }, Act::MoveRelease { lane: 1, kind: MoveKind::Fill }]
        );
        // The pads come up later on the lane pads: no second release.
        assert_eq!(r.release(A, "grid_1"), vec![]);
        assert_eq!(r.release(A, "grid_4"), vec![]);
        assert_eq!(r.release(A, "grid_9"), vec![]);
        assert_eq!(r.s.view().selected_lane, 0, "grid_9 touched lane 0 last");
        // A lane pad held while Sync goes down releases without a move.
        assert_eq!(r.press(A, "grid_2"), vec![Act::ToggleDrumVoice(DrumVoice::Shaker)]);
        r.press(A, "sync");
        assert_eq!(r.release(A, "grid_2"), vec![]);
        assert_eq!(r.release(A, "sync"), vec![]);
    }

    #[test]
    fn controls_act_on_the_first_move() {
        let mut r = Rig::new(None).both();
        let mut pos = [[0.5f32; 4]; 4];
        r.sync(pos, 170.0);
        // Fader A of lane 0 (control 1) at 0.5: no pickup, every move sets it.
        assert_eq!(r.abs(A, "fader_1", 0.2), set(0, 1, 0.2));
        assert_eq!(r.abs(A, "fader_1", 0.7), set(0, 1, 0.7));
        // A recall moves the value: the next move still acts at once.
        pos[0][1] = 0.1;
        r.sync(pos, 170.0);
        assert_eq!(r.abs(A, "fader_1", 0.65), set(0, 1, 0.65));

        // Shift layer: fader 1 is key intent, acting at once.
        r.press(A, "shift");
        assert_eq!(r.abs(A, "fader_1", 0.05), vec![Act::SetKeyIntent(0.05f32 * 2.0 - 1.0)]);
        r.release(A, "shift");
        // Back on the lane layer: acts at once.
        assert_eq!(r.abs(A, "fader_1", 0.3), set(0, 1, 0.3));
    }

    #[test]
    fn shift_layer_and_function_buttons() {
        let mut r = Rig::new(None).both();
        r.sync([[0.5; 4]; 4], 170.0);
        r.press(A, "shift");
        assert!(r.s.view().shift);

        // Tempo: 160..180, engine at 170 = fader 0.5.
        assert_eq!(r.abs(A, "fader_2", 0.5), vec![Act::RequestTempo { bpm: 170.0, bars: TEMPO_RAMP_BARS }]);
        assert_eq!(r.abs(A, "fader_2", 0.75), vec![Act::RequestTempo { bpm: 175.0, bars: TEMPO_RAMP_BARS }]);
        assert_eq!(r.abs(A, "fader_2", 0.751), vec![], "same whole bpm");
        // The engine adopting our target, then an external tempo change:
        // the fader still acts at once.
        r.sync([[0.5; 4]; 4], 175.0);
        assert_eq!(r.abs(A, "fader_2", 1.0), vec![Act::RequestTempo { bpm: 180.0, bars: TEMPO_RAMP_BARS }]);
        r.sync([[0.5; 4]; 4], 165.0);
        assert_eq!(r.abs(A, "fader_2", 0.2), vec![Act::RequestTempo { bpm: 164.0, bars: TEMPO_RAMP_BARS }]);

        assert_eq!(r.press(A, "encoder_push"), vec![Act::CommitKey]);
        assert_eq!(r.press(A, "capture"), vec![Act::Capture(CaptureScope::All)]);
        // With Shift, the other knobs and faders are the lanes' Shift-layer controls.
        assert_eq!(r.abs(A, "knob_1", 0.5), set(0, 4, 0.5));
        assert_eq!(r.abs(A, "fader_4", 0.5), set(1, 7, 0.5));
        // Right Shift: no global layer, every control is a Shift-layer lane control.
        r.press(B, "shift");
        assert_eq!(r.enc(B, 1), vec![]);
        assert_eq!(r.abs(B, "fader_1", 0.5), set(2, 5, 0.5));
        assert_eq!(r.press(B, "encoder_push"), vec![]);
        assert_eq!(r.press(B, "capture"), vec![Act::Capture(CaptureScope::All)]);
        r.release(A, "shift");
        r.release(B, "shift");
        assert!(!r.s.view().shift);

        assert_eq!(r.press(A, "encoder_push"), vec![], "push without Shift");
        assert_eq!(r.press(A, "quant"), vec![Act::SetQuantize(true)]);
        assert_eq!(r.press(B, "quant"), vec![Act::SetQuantize(false)]);
        assert_eq!(r.release(B, "quant"), vec![]);
        r.press(A, "grid_4");
        r.release(A, "grid_4");
        assert_eq!(r.press(A, "capture"), vec![Act::Capture(CaptureScope::Lane { lane: 1 })]);
        for dev in [A, B] {
            assert_eq!(r.press(dev, "sync"), vec![], "Sync does not recall");
            assert_eq!(r.release(dev, "sync"), vec![]);
        }
        assert_eq!(r.press(A, "reverse"), vec![Act::ToggleHalfTime]);
        assert_eq!(r.press(A, "type_btn"), vec![Act::ToggleMinor]);
        assert_eq!(r.release(A, "type_btn"), vec![]);
        r.press(A, "shift");
        assert_eq!(r.press(A, "type_btn"), vec![Act::RandomizeHarmony]);
        r.release(A, "shift");
    }

    #[test]
    fn assignment_by_serial() {
        let mut r = Rig::new(Some(B));
        assert_eq!(r.abs(A, "knob_1", 0.3), set(2, 0, 0.3), "A is not the configured left");
        assert_eq!(r.abs(B, "knob_1", 0.3), set(0, 0, 0.3));
        r.press(A, "shift");
        assert_eq!(r.abs(B, "knob_1", 0.4), set(0, 0, 0.4), "Shift does not override the serial");
        assert_eq!(r.abs("F1-CCC", "knob_1", 0.3), vec![], "third device ignored");
        assert_eq!(r.abs("", "knob_1", 0.3), vec![]);
    }

    #[test]
    fn assignment_by_first_shift() {
        let mut r = Rig::new(None).both();
        // Sorted ids until a Shift press: A left.
        assert_eq!(r.abs(A, "knob_1", 0.3), set(0, 0, 0.3));
        assert_eq!(r.abs(B, "knob_1", 0.3), set(2, 0, 0.3));
        r.press(B, "shift");
        r.release(B, "shift");
        assert_eq!(r.abs(B, "knob_1", 0.3), set(0, 0, 0.3));
        assert_eq!(r.abs(A, "knob_1", 0.3), set(2, 0, 0.3));
        r.press(A, "shift");
        assert_eq!(r.abs(B, "knob_1", 0.3), set(0, 0, 0.3), "locked after the first Shift");

        // A configured serial that is not connected falls back to the same rule.
        let mut r = Rig::new(Some("F1-ZZZ")).both();
        r.press(B, "shift");
        r.release(B, "shift");
        assert_eq!(r.abs(B, "knob_3", 0.3), set(1, 0, 0.3));

        // set_devices restricts the surface.
        let mut r = Rig::new(None);
        r.s.set_devices(&[B.to_string()]);
        assert_eq!(r.abs(A, "knob_1", 0.3), vec![]);
        assert_eq!(r.abs(B, "knob_1", 0.3), set(0, 0, 0.3));
    }

    fn input() -> FeedbackInput {
        let shared = LiveShared::new(172.0, MusicalKey::new(9, true), 0);
        FeedbackInput::from_shared(&shared)
    }

    #[test]
    fn feedback_is_change_tracked() {
        let mut r = Rig::new(None).both();
        let mut inp = input();
        for lane in &mut inp.lanes {
            (lane.on, lane.on_target) = (false, false);
        }
        let (mut leds, mut displays) = (Vec::new(), Vec::new());
        r.s.render_feedback(&inp, &mut leds, &mut displays);
        assert_eq!(leds.len(), 2 * (16 + 4 + 8));
        assert_eq!(displays, vec![(A.to_string(), "8A".to_string()), (B.to_string(), "72".to_string())]);

        leds.clear();
        displays.clear();
        r.s.render_feedback(&inp, &mut leds, &mut displays);
        assert!(leds.is_empty() && displays.is_empty(), "identical state emits nothing");

        // Lane 1 switches on at the next bar: only its play LED blinks.
        inp.lanes[1].on_target = true;
        inp.clock.step = 2;
        r.s.render_feedback(&inp, &mut leds, &mut displays);
        assert_eq!(leds.len(), 1);
        assert!(matches!(&leds[0], (d, FeedbackCommand::SetLed { control, brightness: 0 }) if d == A && control == "play_3"));

        // Lane 1 on: its 6 lane pads and the play LED change.
        leds.clear();
        inp.lanes[1].on = true;
        inp.clock.step = 0;
        r.s.render_feedback(&inp, &mut leds, &mut displays);
        assert_eq!(leds.len(), 7);
        assert!(leds.iter().all(|(d, _)| d == A));
        assert!(leds.iter().any(|(_, c)| matches!(c,
            FeedbackCommand::SetRgb { control, .. } if control == "grid_3")));

        // Sync held on B: only B's pads and its Sync LED change; rows 3-4 go dark.
        leds.clear();
        r.press(B, "sync");
        r.s.render_feedback(&inp, &mut leds, &mut displays);
        assert!(!leds.is_empty() && leds.iter().all(|(d, _)| d == B));
        assert!(leds.iter().any(|(_, c)| matches!(c,
            FeedbackCommand::SetLed { control, brightness: LED_ON } if control == "sync")));
        assert!(leds.iter().any(|(_, c)| matches!(c,
            FeedbackCommand::SetRgb { control, r: 0, g: 0, b: 0 } if control == "grid_9")));

        // Held move lights its pad; intent key and tempo ramp change the displays.
        leds.clear();
        inp.lanes[3].moves = MoveKind::Void.bit();
        inp.intent_key = Some(MusicalKey::new(11, true)); // 10A
        inp.clock.ramping = true;
        inp.clock.target_bpm = 105.4;
        r.s.render_feedback(&inp, &mut leds, &mut displays);
        assert_eq!(leds.len(), 1);
        assert!(matches!(&leds[0], (d, FeedbackCommand::SetRgb { control, .. }) if d == B && control == "grid_8"));
        assert_eq!(displays, vec![(A.to_string(), "AA".to_string()), (B.to_string(), "05".to_string())]);

        // Invalidation resends everything.
        leds.clear();
        displays.clear();
        r.s.invalidate_feedback();
        r.s.render_feedback(&inp, &mut leds, &mut displays);
        assert_eq!((leds.len(), displays.len()), (56, 2));
    }

    #[test]
    fn lane_pads_show_mutes_octave_rate_and_pending() {
        let mut r = Rig::new(None).both();
        let mut inp = input();
        inp.lanes[0].voice_overrides = ((DrumVoice::Hats.bit() | DrumVoice::Ride.bit()) as u16) << 8;
        inp.lanes[0].voice_overrides_playing = (DrumVoice::Hats.bit() as u16) << 8;
        inp.lanes[0].voices_on = 0xFF & !DrumVoice::Hats.bit();
        inp.lanes[1].octave = 2;
        inp.lanes[1].rate = Rate::Double;
        inp.lanes[1].form_pending = Some(3);
        inp.clock.step = 0; // blink on
        let (mut leds, mut displays) = (Vec::new(), Vec::new());
        r.s.render_feedback(&inp, &mut leds, &mut displays);
        let pad = |leds: &[(String, FeedbackCommand)], name: &str| {
            leds.iter()
                .find_map(|(d, c)| match c {
                    FeedbackCommand::SetRgb { control, r, g, b } if d == A && control == name => Some([*r, *g, *b]),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(pad(&leds, "grid_13"), scale(LANE_RGB[0], pads::PAD_ON), "kick plays");
        assert_eq!(pad(&leds, "grid_5"), scale(LANE_RGB[0], pads::PAD_MUTED), "hats off");
        assert_eq!(pad(&leds, "grid_6"), scale(LANE_RGB[0], pads::PAD_ON), "ride still on, its flip queued (blink on)");
        assert_eq!(pad(&leds, "grid_3"), scale(LANE_RGB[1], pads::PAD_IDLE), "no octave down");
        assert_eq!(pad(&leds, "grid_4"), scale(LANE_RGB[1], pads::PAD_ON), "two octaves up");
        assert_eq!(pad(&leds, "grid_8"), scale(LANE_RGB[1], pads::PAD_ON), "double rate");
        assert_eq!(pad(&leds, "grid_16"), [0, 0, 0], "unused pad");
        // Blink off: the queued ride mute and the pending form flip.
        leds.clear();
        inp.clock.step = 2;
        r.s.render_feedback(&inp, &mut leds, &mut displays);
        assert_ne!(pad(&leds, "grid_6"), scale(LANE_RGB[0], pads::PAD_ON));
        assert!(leds.iter().any(|(d, c)| d == A && matches!(c, FeedbackCommand::SetRgb { control, .. } if control == "grid_11")));
    }

    #[test]
    fn seed_hold_lights_play_b_and_shows_the_seed() {
        let mut r = Rig::new(None).both();
        let mut inp = input();
        inp.lanes[1].seed = 1234;
        let (mut leds, mut displays) = (Vec::new(), Vec::new());
        r.s.render_feedback(&inp, &mut leds, &mut displays);
        leds.clear();
        displays.clear();
        r.press(A, "play_4");
        r.s.render_feedback(&inp, &mut leds, &mut displays);
        assert_eq!(displays, vec![(A.to_string(), "34".to_string())]);
        assert!(leds.iter().any(|(d, c)| d == A && matches!(c,
            FeedbackCommand::SetLed { control, brightness: LED_ON } if control == "play_4")));
    }

    #[test]
    fn shift_led_and_requested_tempo_display() {
        let mut r = Rig::new(None).both();
        r.sync([[0.5; 4]; 4], 170.0);
        let inp = input();
        let (mut leds, mut displays) = (Vec::new(), Vec::new());
        r.s.render_feedback(&inp, &mut leds, &mut displays);
        leds.clear();
        displays.clear();
        r.press(A, "shift");
        r.abs(A, "fader_2", 0.5);
        r.abs(A, "fader_2", 0.9); // 178
        r.s.render_feedback(&inp, &mut leds, &mut displays);
        assert_eq!(displays, vec![(B.to_string(), "78".to_string())]);
        assert!(leds.iter().any(|(d, c)| d == A && matches!(c, FeedbackCommand::SetLed { control, brightness: LED_ON } if control == "shift")));
        // Shift shows the drum half's form pads; the pitched half keeps its pads.
        assert!(leds.iter().all(|(d, c)| d == A && match c {
            FeedbackCommand::SetRgb { control, .. } => ["grid_1", "grid_2", "grid_5", "grid_6", "grid_9", "grid_10", "grid_13", "grid_14"].contains(&control.as_str()),
            _ => true,
        }));
    }

    #[test]
    fn seven_segment_keys() {
        let mut seen = std::collections::HashSet::new();
        for root in 0..12 {
            for minor in [false, true] {
                let key = MusicalKey::new(root, minor);
                let (n, letter) = key.camelot();
                let d = key_digits(key);
                let text = String::from_utf8(d.to_vec()).unwrap();
                let expected_first = format!("{:X}", n);
                assert_eq!(text[..1], expected_first, "{key:?}");
                assert_eq!(d[1] as char, letter);
                assert!(text.chars().all(|c| matches!(c, '0'..='9' | 'A'..='F')), "{text}");
                assert!(seen.insert(text), "unique");
            }
        }
        assert_eq!(seen.len(), 24);
        let label = |root, minor| String::from_utf8(key_digits(MusicalKey::new(root, minor)).to_vec()).unwrap();
        assert_eq!(label(9, true), "8A"); // Am
        assert_eq!(label(0, false), "8B"); // C
        assert_eq!(label(11, true), "AA"); // Bm = 10A
        assert_eq!(label(9, false), "BB"); // A = 11B
        assert_eq!(label(1, true), "CA"); // C#m = 12A
        assert_eq!(label(8, true), "1A"); // G#m
    }

    #[test]
    fn seven_segment_tempos() {
        for bpm in 60..=200u32 {
            let d = tempo_digits(bpm as f64);
            assert_eq!(String::from_utf8(d.to_vec()).unwrap(), format!("{:02}", bpm % 100));
        }
        assert_eq!(&tempo_digits(172.0), b"72");
        assert_eq!(&tempo_digits(171.6), b"72");
        assert_eq!(&tempo_digits(100.0), b"00");
        assert_eq!(&tempo_digits(f64::NAN), b"--");
    }
}
