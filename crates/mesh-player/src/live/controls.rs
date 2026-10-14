//! Kontrol F1 live layout (design §7): raw controller events → live commands,
//! and engine state → LEDs / 7-segment displays.
//!
//! Two F1s. The left unit hosts lanes 0 (Drums) and 1, the right unit lanes
//! 2 and 3. Each unit is split into two halves: half A (knobs/faders 1–2,
//! pad columns 1–2, play 1–2) drives the unit's first lane, half B
//! (knobs/faders 3–4, pad columns 3–4, play 3–4) its second lane.
//!
//! | Control | Function |
//! |---|---|
//! | Knob 1 / 3 | lane control 0 (knob A) |
//! | Fader 1 / 3 | lane control 1 (fader A) |
//! | Knob 2 / 4 | lane control 2 (knob B) |
//! | Fader 2 / 4 | lane control 3 (fader B) |
//! | Pads rows 1–2 of the half | chain slots 0 1 / 2 3: press selects the bar; hold + encoder = new pattern for that bar (display shows its seed) |
//! | Pads rows 3–4 of the half | momentary: stutter fill / double void |
//! | Play 1 / 3 (button A) | lane on/off |
//! | Play 2 / 4 (button B) | unassigned (hook for a style mapping) |
//! | Encoder | seed of the selected slot of the unit's selected lane |
//! | Left Shift + fader 1 | key intent |
//! | Left Shift + fader 2 | tempo (ramp target) |
//! | Left Shift + encoder | progression |
//! | Left Shift + encoder push | commit the key intent |
//! | Quant (either unit) | quantized lane toggles on/off (default off) |
//! | Capture | capture the unit's selected lane. With that unit's Shift: everything |
//! | Reverse (either unit) | half-time toggle |
//! | Sync | recall the latest session capture for the unit's selected lane |
//! | Size / Browse (held) + encoder | browse lane A / lane B option, release loads it at the bar |
//! | Type | unassigned |
//!
//! The selected lane of a unit is the lane last touched on it. Knobs and
//! faders use soft takeover: after a layer change or an external value
//! change (recall) a control only acts once it crosses the current value.
//!
//! [`LiveControlSurface::map_into`] is the pure mapper (events → [`Act`]),
//! [`LiveControlSurface::render_feedback`] the pure feedback renderer; both
//! are tested without a running engine.

use std::time::{Duration, Instant};

use mesh_core::live::capture::CaptureScope;
use mesh_core::live::command::{LiveCommand, MoveKind};
use mesh_core::live::engine::LiveController;
use mesh_core::live::state::{ClockView, LaneView, LiveShared};
use mesh_core::live::style::{TempoSpec, CONTROLS_PER_LANE};
use mesh_core::live::NUM_LANES;
use mesh_core::music::MusicalKey;
use mesh_midi::types::{ControlAddress, ControlEvent, ControlValue, FeedbackCommand};

// ─── Layout ────────────────────────────────────────────────────────────

/// What a grid pad does inside its half.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PadRole {
    Slot(u8),
    Move(MoveKind),
}

// PLAYABILITY: tune on hardware (design §7.1 leaves the pad geometry open).
/// `grid_1..=16` (row-major, top-left first) → (half, role). Left two
/// columns = half A, right two = half B. Top two rows = chain slots, bottom
/// two rows = momentary moves.
const PADS: [(u8, PadRole); 16] = {
    use MoveKind::*;
    use PadRole::{Move, Slot};
    [
        (0, Slot(0)), (0, Slot(1)), (1, Slot(0)), (1, Slot(1)),
        (0, Slot(2)), (0, Slot(3)), (1, Slot(2)), (1, Slot(3)),
        (0, Move(Stutter)), (0, Move(Fill)), (1, Move(Stutter)), (1, Move(Fill)),
        (0, Move(Double)), (0, Move(Void)), (1, Move(Double)), (1, Move(Void)),
    ]
};

/// `knob_1..=4` → (half, lane control). Control order per lane:
/// knob A = 0, fader A = 1, knob B = 2, fader B = 3.
const KNOBS: [(u8, u8); 4] = [(0, 0), (0, 2), (1, 0), (1, 2)];
/// `fader_1..=4` → (half, lane control).
const FADERS: [(u8, u8); 4] = [(0, 1), (0, 3), (1, 1), (1, 3)];
/// `play_1..=4` → (half, is play button A). Button B is a no-op hook.
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
/// A soft-takeover control also picks up when this close to the value.
const PICKUP_WINDOW: f32 = 0.02;

// PLAYABILITY: tune on hardware. Lane colours (RGB, 0..=0x7F).
const LANE_RGB: [[u8; 3]; NUM_LANES] = [
    [0x7F, 0x28, 0x00], // Drums: orange
    [0x00, 0x7F, 0x20], // L1: green
    [0x00, 0x40, 0x7F], // L2: blue
    [0x60, 0x00, 0x7F], // L3: purple
];
const WHITE: [u8; 3] = [0x7F, 0x7F, 0x7F];
// PLAYABILITY: tune on hardware. Pad brightness relative to the lane colour.
const PAD_SELECTED: f32 = 1.0;
const PAD_IDLE: f32 = 0.18;
const PAD_MOVE_IDLE: f32 = 0.10;
const PAD_OFF_LANE: f32 = 0.04;
const PAD_OFF_LANE_SELECTED: f32 = 0.12;
// PLAYABILITY: tune on hardware. Single-colour LED levels (0..=0x7F).
const LED_ON: u8 = 0x7F;
const LED_DIM: u8 = 0x0C;
const LED_OFF: u8 = 0x00;

/// Soft-takeover targets: 0..16 are `lane * 4 + control`, then the two
/// Shift-layer faders.
const TARGET_KEY_INTENT: usize = NUM_LANES * CONTROLS_PER_LANE;
const TARGET_TEMPO: usize = TARGET_KEY_INTENT + 1;
const NUM_TARGETS: usize = TARGET_TEMPO + 1;

// ─── Mapper output ─────────────────────────────────────────────────────

/// One surface action. Mirrors the [`LiveCommand`]s the surface sends
/// (comparable in tests) plus capture / recall, which need the controller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Act {
    SelectSlot { lane: u8, slot: u8 },
    SeedDelta { lane: u8, delta: i32 },
    SetControl { lane: u8, control: u8, value: f32 },
    ToggleLane { lane: u8 },
    BrowseOption { lane: u8, delta: i32 },
    CommitOption { lane: u8 },
    MovePress { lane: u8, kind: MoveKind },
    MoveRelease { lane: u8, kind: MoveKind },
    SetKeyIntent(f32),
    CommitKey,
    ProgressionDelta(i32),
    RequestTempo { bpm: f64, bars: u32 },
    SetQuantize(bool),
    ToggleHalfTime,
    Capture(CaptureScope),
    /// Recall the newest session capture that holds `lane` (a lane capture
    /// of that lane or a full capture), restoring only that lane.
    RecallLatest { lane: u8 },
}

fn apply(act: Act, ctl: &mut LiveController) {
    let cmd = match act {
        Act::SelectSlot { lane, slot } => LiveCommand::SelectSlot { lane, slot },
        Act::SeedDelta { lane, delta } => LiveCommand::SeedDelta { lane, delta },
        Act::SetControl { lane, control, value } => LiveCommand::SetControl { lane, control, value },
        Act::ToggleLane { lane } => LiveCommand::ToggleLane { lane },
        Act::BrowseOption { lane, delta } => LiveCommand::BrowseOption { lane, delta },
        Act::CommitOption { lane } => LiveCommand::CommitOption { lane },
        Act::MovePress { lane, kind } => LiveCommand::MovePress { lane, kind },
        Act::MoveRelease { lane, kind } => LiveCommand::MoveRelease { lane, kind },
        Act::SetKeyIntent(position) => LiveCommand::SetKeyIntent(position),
        Act::CommitKey => LiveCommand::CommitKey,
        Act::ProgressionDelta(delta) => LiveCommand::ProgressionDelta(delta),
        Act::RequestTempo { bpm, bars } => LiveCommand::RequestTempo { bpm, bars },
        Act::SetQuantize(on) => LiveCommand::SetQuantize(on),
        Act::ToggleHalfTime => LiveCommand::ToggleHalfTime,
        Act::Capture(scope) => {
            // Adds to the bank and saves to the collection off-thread.
            ctl.capture(scope);
            return;
        }
        Act::RecallLatest { lane } => {
            let found = ctl
                .captures()
                .iter()
                .rev()
                .find(|c| match c.scope {
                    CaptureScope::All => true,
                    CaptureScope::Lane { lane: l } => l == lane,
                })
                .cloned();
            if let Some(mut capture) = found {
                capture.scope = CaptureScope::Lane { lane };
                ctl.recall(&capture);
            }
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

/// A physical knob or fader.
#[derive(Debug, Clone, Copy, Default)]
struct Physical {
    last: Option<f32>,
    target: Option<usize>,
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
    /// Chain slot pad held down: (half, slot). Hold + encoder edits that bar.
    slot_held: Option<(u8, u8)>,
    /// `knob_1..4` then `fader_1..4`.
    physical: [Physical; 8],
    leds: LedCache,
}

impl Unit {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            shift: false,
            half: 0,
            browse_held: [None; 2],
            slot_held: None,
            physical: [Physical::default(); 8],
            leds: LedCache::default(),
        }
    }
}

/// Current value of a soft-takeover target.
#[derive(Debug, Clone, Copy, Default)]
struct Target {
    /// Value, `0..=1` (fader position).
    value: f32,
    /// Value has been read from the engine; until then controls act directly.
    known: bool,
    /// The physical control has crossed the value and now drives it.
    picked: bool,
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
        let positions: [[f32; CONTROLS_PER_LANE]; NUM_LANES] = std::array::from_fn(|l| ctl.lane(l).positions);
        let shared = ctl.shared();
        self.sync_state(&positions, ctl.tempo_range(), shared.clock().target_bpm, shared.quantize());
    }

    /// Take over engine values. A value that changed behind the surface's
    /// back (recall, UI) re-arms soft takeover for its control.
    fn sync_state(
        &mut self,
        positions: &[[f32; CONTROLS_PER_LANE]; NUM_LANES],
        tempo: TempoSpec,
        target_bpm: f64,
        quantize: bool,
    ) {
        for (lane, controls) in positions.iter().enumerate() {
            for (control, &value) in controls.iter().enumerate() {
                let t = &mut self.targets[lane * CONTROLS_PER_LANE + control];
                if !t.known || (t.value - value).abs() > 1e-4 {
                    *t = Target { value, known: true, picked: false };
                }
            }
        }
        self.tempo = Some(tempo);
        if self.seen_target_bpm != Some(target_bpm) {
            self.seen_target_bpm = Some(target_bpm);
            let ours = self.tempo_requested.is_some_and(|r| (r - target_bpm).abs() < 0.5);
            let t = &mut self.targets[TARGET_TEMPO];
            if !ours || !t.known {
                *t = Target { value: tempo_position(tempo, target_bpm), known: true, picked: false };
            }
        }
        if !self.targets[TARGET_KEY_INTENT].known {
            // The intent fader rests at the centre (no transition).
            self.targets[TARGET_KEY_INTENT] = Target { value: 0.5, known: true, picked: false };
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
        let shift_layer = side == Side::Left && self.units[u].shift;
        let target = match phys {
            4 if shift_layer => TARGET_KEY_INTENT,
            5 if shift_layer => TARGET_TEMPO,
            _ => {
                self.touch(u, side, half);
                side.lane(half) as usize * CONTROLS_PER_LANE + control as usize
            }
        };

        let p = &mut self.units[u].physical[phys];
        if p.target != Some(target) {
            // The control left its previous target; that one needs a pickup later.
            if let Some(old) = p.target {
                self.targets[old].picked = false;
            }
            p.target = Some(target);
        }
        let last = p.last.replace(v);

        let t = &mut self.targets[target];
        if !t.known {
            t.picked = true;
        } else if !t.picked {
            let crossed = last.is_some_and(|l| (l - t.value) * (v - t.value) <= 0.0);
            t.picked = crossed || (v - t.value).abs() <= PICKUP_WINDOW;
        }
        if !t.picked {
            return;
        }
        t.value = v;

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
        } else if side == Side::Left && unit.shift {
            out.push(Act::ProgressionDelta(delta));
        } else {
            out.push(Act::SeedDelta { lane: side.lane(unit.half), delta });
        }
    }

    fn button(&mut self, u: usize, side: Side, control: Control, pressed: bool, now: Instant, out: &mut Vec<Act>) {
        let selected = side.lane(self.units[u].half);
        match control {
            Control::Shift => self.units[u].shift = pressed,
            Control::Pad(i) => {
                let (half, role) = PADS[i];
                let lane = side.lane(half);
                if pressed {
                    self.touch(u, side, half);
                }
                match role {
                    PadRole::Slot(slot) if pressed => {
                        self.units[u].slot_held = Some((half, slot));
                        out.push(Act::SelectSlot { lane, slot });
                    }
                    PadRole::Slot(slot) => {
                        if self.units[u].slot_held == Some((half, slot)) {
                            self.units[u].slot_held = None;
                        }
                    }
                    PadRole::Move(kind) if pressed => out.push(Act::MovePress { lane, kind }),
                    PadRole::Move(kind) => out.push(Act::MoveRelease { lane, kind }),
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
                if !is_a {
                    // Play button B: no-op hook, style-mappable later (§7.1).
                    return;
                }
                if pressed {
                    out.push(Act::ToggleLane { lane: side.lane(half) });
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
            Control::Sync => out.push(Act::RecallLatest { lane: selected }),
            // Unassigned (kept free for P4: master FX, page).
            Control::Type => {}
            Control::Knob(_) | Control::Fader(_) | Control::Encoder => {}
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
        let beat_on = step % 4 < 2;
        let blink_on = (step / 2) % 2 == 0;
        let left = self.left_index();
        let left_shift = left.is_some_and(|l| self.units[l].shift);

        for ui in 0..self.units.len() {
            let side = if left == Some(ui) { Side::Left } else { Side::Right };
            let unit = &mut self.units[ui];
            let id = &unit.id;

            for (pad, &(half, role)) in PADS.iter().enumerate() {
                let lane = side.lane(half) as usize;
                let rgb = pad_rgb(&input.lanes[lane], lane, role, input.clock.bar_in_phrase, beat_on);
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
                } else {
                    LED_OFF
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
                LED_DIM,
            ];
            for (i, &level) in funcs.iter().enumerate() {
                if unit.leds.funcs[i] != Some(level) {
                    unit.leds.funcs[i] = Some(level);
                    leds.push((id.clone(), FeedbackCommand::SetLed { control: FUNC_NAMES[i].to_string(), brightness: level }));
                }
            }

            // While a chain slot pad is held, show that bar's seed (last 2 digits).
            let held_seed = unit.slot_held.map(|(half, slot)| input.lanes[side.lane(half) as usize].seeds[slot as usize]);
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

fn pad_rgb(view: &LaneView, lane: usize, role: PadRole, bar_in_phrase: u8, beat_on: bool) -> [u8; 3] {
    let base = LANE_RGB[lane];
    match role {
        PadRole::Slot(slot) => {
            let selected = view.selected_slot == slot;
            if !view.on {
                scale(base, if selected { PAD_OFF_LANE_SELECTED } else { PAD_OFF_LANE })
            } else if bar_in_phrase == slot && beat_on {
                // The playing slot pulses white with the beat.
                WHITE
            } else {
                scale(base, if selected { PAD_SELECTED } else { PAD_IDLE })
            }
        }
        PadRole::Move(kind) => {
            if view.moves & kind.bit() != 0 {
                WHITE
            } else {
                scale(base, if view.on { PAD_MOVE_IDLE } else { PAD_OFF_LANE })
            }
        }
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

        fn sync(&mut self, positions: [[f32; 4]; 4], target_bpm: f64) {
            let tempo = TempoSpec { default: 170.0, min: 160.0, max: 180.0 };
            self.s.sync_state(&positions, tempo, target_bpm, false);
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
    fn slot_select_and_seed_delta_follow_the_selected_lane() {
        let mut r = Rig::new(None).both();
        assert_eq!(r.enc(A, 1), vec![Act::SeedDelta { lane: 0, delta: 1 }], "default: lane A");
        assert_eq!(r.press(A, "grid_6"), vec![Act::SelectSlot { lane: 0, slot: 3 }]);
        assert_eq!(r.release(A, "grid_6"), vec![]);
        assert_eq!(r.press(A, "grid_3"), vec![Act::SelectSlot { lane: 1, slot: 0 }]);
        assert_eq!(r.enc(A, -2), vec![Act::SeedDelta { lane: 1, delta: -2 }]);
        assert_eq!(r.press(B, "grid_7"), vec![Act::SelectSlot { lane: 3, slot: 2 }]);
        assert_eq!(r.enc(B, 3), vec![Act::SeedDelta { lane: 3, delta: 3 }]);
        assert_eq!(r.enc(A, 1), vec![Act::SeedDelta { lane: 1, delta: 1 }], "per-unit selection");
        // Touching a knob selects its lane too.
        r.abs(A, "knob_1", 0.5);
        assert_eq!(r.enc(A, 1), vec![Act::SeedDelta { lane: 0, delta: 1 }]);
    }

    #[test]
    fn hold_slot_and_turn_edits_that_bar() {
        let mut r = Rig::new(None).both();
        r.abs(A, "knob_1", 0.5); // lane 0 selected
        assert_eq!(r.press(A, "grid_8"), vec![Act::SelectSlot { lane: 1, slot: 3 }]);
        assert_eq!(r.enc(A, 2), vec![Act::SeedDelta { lane: 1, delta: 2 }], "held bar of lane 1");
        assert_eq!(r.release(A, "grid_8"), vec![]);
        assert!(r.s.units[0].slot_held.is_none());
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
        assert_eq!(r.enc(B, 1), vec![Act::SeedDelta { lane: 3, delta: 1 }], "browse ended");
        // Play button B is a no-op hook.
        assert_eq!(r.at(3000, A, "play_2", p), vec![]);
        assert_eq!(r.at(3050, A, "play_2", rel), vec![]);
    }

    #[test]
    fn momentary_moves_press_and_release() {
        let mut r = Rig::new(None).both();
        let cases = [
            (A, "grid_9", 0, MoveKind::Stutter),
            (A, "grid_10", 0, MoveKind::Fill),
            (A, "grid_13", 0, MoveKind::Double),
            (A, "grid_14", 0, MoveKind::Void),
            (A, "grid_11", 1, MoveKind::Stutter),
            (B, "grid_12", 3, MoveKind::Fill),
            (B, "grid_15", 3, MoveKind::Double),
            (B, "grid_16", 3, MoveKind::Void),
            (B, "grid_13", 2, MoveKind::Double),
        ];
        for (dev, pad, lane, kind) in cases {
            assert_eq!(r.press(dev, pad), vec![Act::MovePress { lane, kind }], "{pad}");
            assert_eq!(r.release(dev, pad), vec![Act::MoveRelease { lane, kind }], "{pad}");
        }
    }

    #[test]
    fn soft_takeover_after_sync_and_layer_change() {
        let mut r = Rig::new(None).both();
        let mut pos = [[0.5f32; 4]; 4];
        r.sync(pos, 170.0);
        // Fader A of lane 0 (control 1) at 0.5: must cross it.
        assert_eq!(r.abs(A, "fader_1", 0.2), vec![]);
        assert_eq!(r.abs(A, "fader_1", 0.4), vec![]);
        assert_eq!(r.abs(A, "fader_1", 0.6), set(0, 1, 0.6));
        assert_eq!(r.abs(A, "fader_1", 0.7), set(0, 1, 0.7));
        // Our own value echoed back by the engine keeps the pickup.
        pos[0][1] = 0.7;
        r.sync(pos, 170.0);
        assert_eq!(r.abs(A, "fader_1", 0.8), set(0, 1, 0.8));
        // A recall moves the value: pickup again.
        pos[0][1] = 0.1;
        r.sync(pos, 170.0);
        assert_eq!(r.abs(A, "fader_1", 0.65), vec![]);
        assert_eq!(r.abs(A, "fader_1", 0.11), set(0, 1, 0.11), "within the pickup window");

        // Shift layer: fader 1 becomes key intent (centre 0.5), picks up on crossing.
        pos[0][1] = 0.11;
        r.sync(pos, 170.0);
        r.press(A, "shift");
        assert_eq!(r.abs(A, "fader_1", 0.05), vec![]);
        assert_eq!(r.abs(A, "fader_1", 0.3), vec![]);
        assert_eq!(r.abs(A, "fader_1", 0.75), vec![Act::SetKeyIntent(0.5)]);
        r.release(A, "shift");
        // Back on the lane layer: the lane value (0.11) must be crossed again.
        assert_eq!(r.abs(A, "fader_1", 0.6), vec![]);
        assert_eq!(r.abs(A, "fader_1", 0.05), set(0, 1, 0.05));
        // And the key intent needs a pickup at its last value (0.75).
        r.press(A, "shift");
        assert_eq!(r.abs(A, "fader_1", 0.5), vec![]);
        assert_eq!(r.abs(A, "fader_1", 0.76), vec![Act::SetKeyIntent(0.76f32 * 2.0 - 1.0)]);
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
        // The engine adopting our target keeps the pickup ...
        r.sync([[0.5; 4]; 4], 175.0);
        assert_eq!(r.abs(A, "fader_2", 1.0), vec![Act::RequestTempo { bpm: 180.0, bars: TEMPO_RAMP_BARS }]);
        // ... an external tempo change re-arms it.
        r.sync([[0.5; 4]; 4], 165.0);
        assert_eq!(r.abs(A, "fader_2", 0.9), vec![]);
        assert_eq!(r.abs(A, "fader_2", 0.2), vec![Act::RequestTempo { bpm: 164.0, bars: TEMPO_RAMP_BARS }]);

        assert_eq!(r.enc(A, 1), vec![Act::ProgressionDelta(1)]);
        assert_eq!(r.press(A, "encoder_push"), vec![Act::CommitKey]);
        assert_eq!(r.press(A, "capture"), vec![Act::Capture(CaptureScope::All)]);
        // Shift layer faders 3/4 and knobs stay lane controls.
        assert_eq!(r.abs(A, "knob_1", 0.5), set(0, 0, 0.5));
        // Right Shift has no layer: encoder stays seed, faders stay lanes.
        r.press(B, "shift");
        assert_eq!(r.enc(B, 1), vec![Act::SeedDelta { lane: 2, delta: 1 }]);
        assert_eq!(r.abs(B, "fader_1", 0.5), set(2, 1, 0.5));
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
        assert_eq!(r.press(A, "capture"), vec![Act::Capture(CaptureScope::Lane { lane: 1 })]);
        assert_eq!(r.press(A, "sync"), vec![Act::RecallLatest { lane: 1 }]);
        assert_eq!(r.press(B, "sync"), vec![Act::RecallLatest { lane: 2 }]);
        assert_eq!(r.press(A, "reverse"), vec![Act::ToggleHalfTime]);
        for unassigned in ["type_btn"] {
            assert_eq!(r.press(A, unassigned), vec![]);
        }
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

        // Lane 1 on: its 8 pads and the play LED change; the playing slot (0) pulses white.
        leds.clear();
        inp.lanes[1].on = true;
        inp.clock.step = 0;
        r.s.render_feedback(&inp, &mut leds, &mut displays);
        assert_eq!(leds.len(), 9);
        assert!(leds.iter().all(|(d, _)| d == A));
        assert!(leds.iter().any(|(_, c)| matches!(c,
            FeedbackCommand::SetRgb { control, r: 0x7F, g: 0x7F, b: 0x7F } if control == "grid_3")));

        // Held move lights its pad; intent key and tempo ramp change the displays.
        leds.clear();
        inp.lanes[3].moves = MoveKind::Void.bit();
        inp.intent_key = Some(MusicalKey::new(11, true)); // 10A
        inp.clock.ramping = true;
        inp.clock.target_bpm = 105.4;
        r.s.render_feedback(&inp, &mut leds, &mut displays);
        assert_eq!(leds.len(), 1);
        assert!(matches!(&leds[0], (d, FeedbackCommand::SetRgb { control, .. }) if d == B && control == "grid_16"));
        assert_eq!(displays, vec![(A.to_string(), "AA".to_string()), (B.to_string(), "05".to_string())]);

        // Invalidation resends everything.
        leds.clear();
        displays.clear();
        r.s.invalidate_feedback();
        r.s.render_feedback(&inp, &mut leds, &mut displays);
        assert_eq!((leds.len(), displays.len()), (56, 2));
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
        assert!(matches!(&leds[..], [(d, FeedbackCommand::SetLed { control, brightness: LED_ON })]
            if d == A && control == "shift"));
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
