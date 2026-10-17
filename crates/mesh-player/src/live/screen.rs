//! The terminal screen of live mode: a dark, monochrome, analog look with
//! only the performance information (key, tempo, phrase, seeds, forms,
//! knobs, the drum steps) and a scrolling waveline per lane.
//!
//! The wavelines are centred on the sample playing now and span
//! [`VIEW_BEATS`] beats, so every lane scrolls with the tempo; the right
//! half is the span a bar earlier (what the loop is about to repeat).
//!
//! Each lane's waveline carries its own VFX ([`mesh_widgets::live_vfx`]):
//! its knobs drive them by what they do ([`fx_of`]: density = line weight,
//! shape / mutate / cutoff = bitcrush, morph / cx / stack = RGB split,
//! verb / room = feedback trail, spread = rumble), and an envelope follower
//! on the lane's own signal ([`Follower`]) scales them, so the effects hit
//! where and when the lane plays. Faint scanlines and a soft vignette cover
//! the screen.
//!
//! [`ScreenFrame`] is built once per UI tick by the app; [`view`] only
//! borrows it. The previous code-readout screen stays in `app.rs` (`F2`
//! switches).

use std::sync::Arc;

use iced::widget::{column, container, row, space, stack, text};
use iced::{Alignment, Background, Border, Color, Element, Font, Length, Padding};

use mesh_core::live::glue::Role;
use mesh_core::live::state::LaneView;
use mesh_core::live::scope::SCOPE_DECIMATION;
use mesh_core::live::style::CONTROLS_PER_LANE;
use mesh_core::live::generator::layout::DrumVoice;
use mesh_core::live::state::BarNote;
use mesh_core::live::{NUM_LANES, TICKS_PER_BAR};
use mesh_widgets::live_vfx::{
    columns, live_screen, live_wave, roll, RollNote, ScreenStyle, WaveFx, WaveStyle, ROLL_COLUMNS, ROLL_ROWS, WAVE_COLUMNS,
};

use super::readout::{form_window, LaneInfo};

// MUSICAL: the look, tune on the device screen.
/// Ground.
const BG: Color = Color::from_rgb(0.043, 0.051, 0.059);
/// The ink: pale ash phosphor.
const INK: Color = Color::from_rgb(0.84, 0.87, 0.85);
/// Ghost tints of the text split (red left, blue right).
const GHOST_R: Color = Color::from_rgba(0.95, 0.35, 0.32, 0.5);
const GHOST_B: Color = Color::from_rgba(0.35, 0.55, 0.98, 0.5);

/// A lane's ink: the ash with a faint hue per role.
fn lane_ink(role: Role) -> Color {
    match role {
        Role::Drums => Color::from_rgb(0.92, 0.86, 0.76),
        Role::Bass => Color::from_rgb(0.74, 0.88, 0.92),
        Role::Lead => Color::from_rgb(0.88, 0.80, 0.94),
        Role::Extra => Color::from_rgb(0.78, 0.92, 0.80),
    }
}

/// Beats the waveline spans: half before now, half after.
// MUSICAL: waveline zoom, tune on the device screen.
pub const VIEW_BEATS: f32 = 2.0;

pub const DATA_FONT: Font = Font::with_name("Share Tech Mono");
pub const DISPLAY_FONT: Font = Font::with_name("Major Mono Display");

/// Width of the lane's text column.
const INFO_WIDTH: f32 = 300.0;
/// Column right of the text, on every lane so the wavelines line up (the
/// drum lane's voice names).
const LABELS_WIDTH: f32 = 56.0;
/// Drum voices, top to bottom as the roll shows them (kick at the bottom).
const DRUM_WORDS: [&str; 8] = ["brk", "top", "perc", "shaker", "ride", "hats", "snare", "kick"];

// MUSICAL: effect response, tune on the device screen.
/// Follower: a fast and a slow envelope of the lane's level (attack share
/// and release factor per 30 Hz tick). The effects follow their difference
/// times [`ONSET_GAIN`], so hits fire them and a held level lets them fall.
const FAST: (f32, f32) = (0.8, 0.6);
const SLOW: (f32, f32) = (0.08, 0.97);
/// Share of the slow envelope taken off the fast one.
const SLOW_SHARE: f32 = 0.9;
const ONSET_GAIN: f32 = 2.0;
/// Effects follow the envelope to this power: peaks hit fully, the middle
/// of the envelope much less.
const ENV_POWER: f32 = 2.2;
/// Knobs set the amount to this power, so low and high settings look
/// clearly different.
const KNOB_POWER: f32 = 1.5;
/// Follower normalisation: the reference peak decays this much per tick and
/// never goes below the floor (quiet lanes are not blown up).
const NORM_DECAY: f32 = 0.995;
const NORM_FLOOR: f32 = 0.05;
/// Waveline gain floor (as the classic scope).
const GAIN_FLOOR: f32 = 0.3;
/// Swap transition (seed, instrument, form segment change): decay per tick,
/// about half a second.
const SWAP_DECAY: f32 = 0.86;
/// Kick shake: jolt in pixels and its decay per tick.
const SHAKE_PX: f32 = 5.0;
const SHAKE_DECAY: f32 = 0.62;
/// Pitched roll: fewest rows (small ranges sit in the middle).
const MIN_ROWS: i32 = 12;
/// Drum hits are drawn this many ticks long.
const HIT_TICKS: f32 = 4.0;

/// Effects a knob can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fx {
    Weight,
    Crush,
    Rgb,
    Trail,
    Rumble,
}

/// The effect a control drives, by its label (what it does to the sound).
pub fn fx_of(label: &str) -> Option<Fx> {
    Some(match label {
        "dens" | "density" => Fx::Weight,
        "shape" | "mutate" | "cutoff" | "drive" | "bite" => Fx::Crush,
        "morph" | "mod" | "cx" | "stack" => Fx::Rgb,
        "verb" | "room" | "reverb" | "delay" => Fx::Trail,
        "spread" => Fx::Rumble,
        _ => return None,
    })
}

/// Waveline span in scope points and in ticks, at `bpm`.
pub fn view_points(bpm: f64, sample_rate: u32) -> (f32, f32) {
    let per_beat = 60.0 / bpm.max(1.0) * sample_rate as f64 / SCOPE_DECIMATION as f64;
    (VIEW_BEATS * per_beat as f32, VIEW_BEATS * (TICKS_PER_BAR / 4) as f32)
}

/// Envelope follower on a lane's scope ring, UI side. `env` (0..1) gates
/// the lane's effects: their amounts are the knobs times `env`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Follower {
    written: usize,
    reference: f32,
    fast: f32,
    slow: f32,
    pub env: f32,
}

impl Follower {
    /// Feed the ring snapshot (oldest first) and the ring's write count.
    pub fn update(&mut self, points: &[f32], written: usize) {
        let fresh = written.wrapping_sub(self.written).min(points.len());
        self.written = written;
        let peak = points[points.len() - fresh..].iter().fold(0.0f32, |p, v| if v.is_finite() { p.max(v.abs()) } else { p });
        self.reference = (self.reference * NORM_DECAY).max(peak).max(NORM_FLOOR);
        let level = (peak / self.reference).clamp(0.0, 1.0);
        let follow = |e: f32, (attack, release): (f32, f32)| if level > e { e + (level - e) * attack } else { e * release };
        self.fast = follow(self.fast, FAST);
        self.slow = follow(self.slow, SLOW);
        self.env = (ONSET_GAIN * (self.fast - SLOW_SHARE * self.slow)).clamp(0.0, 1.0);
    }
}

/// A lane's motion between ticks: its follower and the swap transition.
#[derive(Debug, Clone, Copy, Default)]
pub struct LaneMotion {
    pub follower: Follower,
    /// Swap transition 0..1, fired when the seed, the instrument or the
    /// form segment changes (by hand or at a bar).
    pub swap: f32,
    last: Option<(u32, u16, u16, u8)>,
}

impl LaneMotion {
    pub fn update(&mut self, v: &LaneView) {
        let now = (v.seed, v.option, v.form, v.pos.segment);
        self.swap *= SWAP_DECAY;
        if self.last.is_some_and(|l| l != now) {
            self.swap = 1.0;
        }
        self.last = Some(now);
    }
}

/// Screen motion shared by the lanes: the kick shake.
#[derive(Debug, Clone, Copy, Default)]
pub struct Motion {
    pub lanes: [LaneMotion; NUM_LANES],
    pub shake: f32,
    last_step: Option<u8>,
}

impl Motion {
    /// The drum lane's playing step: a kick on any step passed since the last
    /// tick jolts the screen.
    pub fn kick(&mut self, drums: &LaneView) {
        self.shake *= SHAKE_DECAY;
        let step = drums.active_step % 16;
        let kick = DrumVoice::Kick;
        let audible = drums.on && drums.voices_on & kick.bit() != 0;
        if let Some(last) = self.last_step.filter(|&l| l != step) {
            let passed = (step + 16 - last) % 16;
            let hit = (1..=passed).any(|d| drums.drum_steps[kick as usize] & 1 << ((last + d) % 16) != 0);
            if hit && audible {
                self.shake = 1.0;
            }
        }
        self.last_step = Some(step);
    }
}

/// One form segment as shown.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub text: String,
    pub playing: bool,
}

/// What one lane shows.
pub struct LaneScreen {
    pub index: usize,
    pub name: String,
    pub role: Role,
    pub seed: u32,
    pub option: String,
    pub form: Vec<Token>,
    /// A queued form (shown after an arrow).
    pub form_next: Option<String>,
    pub controls: Vec<(String, f32)>,
    pub on: bool,
    pub on_target: bool,
    pub selected: bool,
    /// Drum lane: muted voices (bit per voice).
    pub muted: u8,
    pub columns: Arc<[[f32; 2]; WAVE_COLUMNS]>,
    pub roll: Arc<[[u32; 2]; ROLL_COLUMNS]>,
    pub wave: WaveStyle,
}

/// Everything the screen shows in one tick.
pub struct ScreenFrame {
    pub key: String,
    pub intent: Option<String>,
    pub bpm: String,
    pub half_time: String,
    pub chords: String,
    pub phrase: [bool; 4],
    pub bar: u64,
    pub shift: bool,
    pub recording: Option<String>,
    /// Toast, audio error or warnings (empty: nothing).
    pub status: String,
    pub lanes: Vec<LaneScreen>,
}

impl Default for ScreenFrame {
    fn default() -> Self {
        Self {
            key: String::new(),
            intent: None,
            bpm: String::new(),
            half_time: String::new(),
            chords: String::new(),
            phrase: [false; 4],
            bar: 0,
            shift: false,
            recording: None,
            status: String::new(),
            lanes: Vec::new(),
        }
    }
}

/// Inputs of one lane for [`lane_screen`].
pub struct LaneInput<'a> {
    pub index: usize,
    pub info: &'a LaneInfo,
    pub view: &'a LaneView,
    pub points: &'a [f32],
    pub notes: &'a [BarNote],
    pub motion: LaneMotion,
    pub shake: f32,
    pub selected: bool,
    /// Tick within the bar at the newest scope point.
    pub now_tick: f64,
    /// View span in scope points and in ticks ([`view_points`]).
    pub span: f32,
    pub span_ticks: f32,
}

/// The lane's notes as roll rows: drum voices (kick at the bottom), or
/// pitches over the bar's range (at least [`MIN_ROWS`]). Returns the rows.
fn roll_notes(role: Role, notes: &[BarNote], out: &mut Vec<RollNote>) -> u8 {
    out.clear();
    if role == Role::Drums {
        for n in notes {
            if let Some(v) = DrumVoice::of_target(n.target) {
                out.push(RollNote { start: n.tick as f32, len: HIT_TICKS, row: v as u8 });
            }
        }
        return 8;
    }
    let (Some(lo), Some(hi)) = (notes.iter().map(|n| n.key as i32).min(), notes.iter().map(|n| n.key as i32).max()) else {
        return 0;
    };
    let range = hi - lo + 1;
    let rows = range.clamp(MIN_ROWS, ROLL_ROWS as i32);
    let pad = (rows - range).max(0) / 2;
    for n in notes {
        let k = n.key as i32 - lo;
        let row = if range > rows { k * (rows - 1) / (range - 1).max(1) } else { k + pad };
        out.push(RollNote { start: n.tick as f32, len: n.gate.max(6) as f32, row: row as u8 });
    }
    rows as u8
}

/// Build a lane's screen data.
pub fn lane_screen(
    i: LaneInput<'_>,
    cols: &mut [[f32; 2]; WAVE_COLUMNS],
    rolls: &mut [[u32; 2]; ROLL_COLUMNS],
    scratch: &mut Vec<RollNote>,
) -> LaneScreen {
    let v = i.view;
    let info = i.info;
    // Gain from the span shown, so a lane's level holds across the view.
    let recent = &i.points[i.points.len().saturating_sub(i.span as usize * 2)..];
    let peak = recent.iter().fold(0.0f32, |p, x| if x.is_finite() { p.max(x.abs()) } else { p });
    columns(i.points, i.span, 1.0 / peak.max(GAIN_FLOOR), cols);
    let rows = roll_notes(info.role, i.notes, scratch);
    roll(scratch, TICKS_PER_BAR as f32, i.now_tick, i.span_ticks, rolls);

    let shown = v.option_pending.unwrap_or(v.option) as usize;
    let option = info.option_names.get(shown).map(|n| n.split('+').next().unwrap_or("").to_string()).unwrap_or_default();

    let segs: Vec<&str> = info.forms.get(v.form as usize).map(|f| f.split_whitespace().collect()).unwrap_or_default();
    let playing_at = v.pos.segment as usize;
    let (from, to) = form_window(segs.len(), playing_at);
    let form = segs[from..to]
        .iter()
        .enumerate()
        .map(|(k, s)| Token { text: s.to_string(), playing: v.on && from + k == playing_at })
        .collect();
    let form_next = v.form_pending.and_then(|f| info.forms.get(f as usize)).cloned();

    let controls: Vec<(String, f32)> = info
        .controls
        .iter()
        .take(CONTROLS_PER_LANE)
        .enumerate()
        .map(|(ci, c)| (c.label.clone(), v.controls[ci].clamp(0.0, 1.0)))
        .collect();
    let env = i.motion.follower.env;
    let react = env.powf(ENV_POWER);
    let mut fx = WaveFx::default();
    for (ci, c) in info.controls.iter().enumerate().take(CONTROLS_PER_LANE) {
        let knob = v.controls[ci].clamp(0.0, 1.0);
        let slot = match fx_of(&c.label) {
            Some(Fx::Weight) => &mut fx.weight,
            Some(Fx::Crush) => &mut fx.crush,
            Some(Fx::Rgb) => &mut fx.split,
            Some(Fx::Trail) => &mut fx.trail,
            Some(Fx::Rumble) => &mut fx.rumble,
            None => continue,
        };
        *slot = slot.max(knob.powf(KNOB_POWER) * react);
    }
    let muted = !v.voices_on;
    // The roll lists voices bottom up, the labels top down: kick is row 0.
    let muted_rows = if info.role == Role::Drums { muted } else { 0 };
    LaneScreen {
        index: i.index,
        name: info.name.clone(),
        role: info.role,
        seed: v.seed % 10_000,
        option,
        form,
        form_next,
        controls,
        on: v.on,
        on_target: v.on_target,
        selected: i.selected,
        muted,
        columns: Arc::new(*cols),
        roll: Arc::new(*rolls),
        wave: WaveStyle {
            tint: lane_ink(info.role),
            level: if v.on { 1.0 } else { 0.4 },
            envelope: react,
            fx,
            rows,
            muted_rows,
            shake: i.shake * SHAKE_PX,
            swap: i.motion.swap,
        },
    }
}

fn ink(a: f32) -> Color {
    Color { a, ..INK }
}

/// Text with a cold and a warm ghost `split` pixels to the sides.
fn ghost<'a, M: 'a>(s: String, size: f32, font: Font, color: Color, split: f32) -> Element<'a, M> {
    if split < 0.3 {
        return text(s).font(font).size(size).color(color).into();
    }
    let t = |c: Color| text(s.clone()).font(font).size(size).color(c);
    stack![
        container(t(GHOST_R)).padding(Padding { left: 0.0, right: split, top: 0.0, bottom: 0.0 }),
        container(t(GHOST_B)).padding(Padding { left: split * 2.0, right: 0.0, top: 0.0, bottom: 0.0 }),
        container(t(color)).padding(Padding { left: split, right: 0.0, top: 0.0, bottom: 0.0 }),
    ]
    .into()
}

fn label<'a, M: 'a>(s: &str, a: f32) -> Element<'a, M> {
    text(s.to_string()).font(DATA_FONT).size(12).color(ink(a)).into()
}

fn header<'a, M: 'a>(f: &'a ScreenFrame) -> Element<'a, M> {
    let mut cells = row![].spacing(5);
    for (i, lit) in f.phrase.into_iter().enumerate() {
        let a = if lit { if i + 1 < 4 && f.phrase[i + 1] { 1.0 } else { 0.6 } } else { 0.0 };
        cells = cells.push(container(space().width(26.0).height(12.0)).style(move |_| container::Style {
            background: (a > 0.0).then_some(Background::Color(ink(a))),
            border: Border { color: ink(0.5), width: if a > 0.0 { 0.0 } else { 1.0 }, radius: 0.0.into() },
            ..Default::default()
        }));
    }
    let field = |l: &'static str, v: Element<'a, M>| row![label(l, 0.62), v].spacing(10).align_y(Alignment::Center);
    let mut head = row![
        text("mesh//live").font(DISPLAY_FONT).size(20).color(ink(0.62)),
        field("KEY", ghost(f.key.trim_start_matches("KEY ").to_string(), 30.0, DATA_FONT, INK, 1.2)),
    ]
    .spacing(44)
    .align_y(Alignment::Center);
    if let Some(intent) = &f.intent {
        head = head.push(text(intent.clone()).font(DATA_FONT).size(16).color(ink(0.62)));
    }
    head = head
        .push(field("BPM", ghost(f.bpm.trim_end_matches(" BPM").trim().to_string(), 30.0, DATA_FONT, INK, 1.2)))
        .push(text(f.half_time.clone()).font(DATA_FONT).size(14).color(ink(0.45)))
        .push(field("PHR", row![cells, text(format!("{:04}", f.bar % 10_000)).font(DATA_FONT).size(22).color(INK)].spacing(12).align_y(Alignment::Center).into()))
        .push(field("CHD", text(f.chords.trim_start_matches("chords ").to_string()).font(DATA_FONT).size(20).color(INK).into()))
        .push(space().width(Length::Fill));
    if f.shift {
        head = head.push(text("SHIFT").font(DATA_FONT).size(16).color(INK));
    }
    if let Some(rec) = &f.recording {
        head = head.push(text(rec.clone()).font(DATA_FONT).size(18).color(INK));
    }
    container(head).padding(Padding::from([0.0, 24.0])).height(60.0).center_y(60.0).into()
}

fn lane<'a, M: 'a>(l: &'a LaneScreen) -> Element<'a, M> {
    let base = lane_ink(l.role);
    let ink = move |a: f32| Color { a, ..base };
    let dim = if l.on { 1.0 } else { 0.45 };
    // The text splits with the lane's RGB effect and tears open on a swap.
    let split = 2.6 * l.wave.fx.split + 8.0 * l.wave.swap;
    let number = text(format!("{:02}", l.index + 1)).font(DATA_FONT).size(12).color(ink(if l.selected { 0.9 } else { 0.42 }));
    let mut title = row![number, ghost(l.name.to_lowercase(), 24.0, DISPLAY_FONT, ink(dim), split)].spacing(12).align_y(Alignment::End);
    if l.selected {
        title = title.push(text("<").font(DATA_FONT).size(16).color(base));
    }
    if l.on != l.on_target {
        title = title.push(text(if l.on_target { "> on" } else { "> off" }).font(DATA_FONT).size(13).color(ink(0.62)));
    }
    let seed = row![
        label("SEED", 0.62),
        ghost(format!("{:04}", l.seed), 26.0, DATA_FONT, ink(dim), split),
        text(l.option.clone()).font(DATA_FONT).size(12).color(ink(0.42)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let mut form = row![].spacing(6).align_y(Alignment::Center);
    for t in &l.form {
        let playing = t.playing;
        let fg = if playing { BG } else { ink(0.62 * dim) };
        form = form.push(container(text(t.text.clone()).font(DATA_FONT).size(15).color(fg)).padding(Padding::from([0.0, 5.0])).style(
            move |_| container::Style { background: playing.then_some(Background::Color(base)), ..Default::default() },
        ));
    }
    if let Some(next) = &l.form_next {
        form = form.push(text(format!("> {next}")).font(DATA_FONT).size(13).color(ink(0.42)));
    }

    let mut knobs = row![].spacing(10);
    for (name, pos) in &l.controls {
        let filled = (pos * 100.0).round() as u16;
        let bar = row![
            container(space().height(2.0)).width(Length::FillPortion(filled.max(1))).style(move |_| container::Style {
                background: Some(Background::Color(ink(if filled == 0 { 0.0 } else { dim }))),
                ..Default::default()
            }),
            container(space().height(2.0)).width(Length::FillPortion((100 - filled.min(100)).max(1))).style(move |_| container::Style {
                background: Some(Background::Color(ink(0.22))),
                ..Default::default()
            }),
        ];
        knobs = knobs.push(column![text(name.clone()).font(DATA_FONT).size(10).color(ink(0.62)), bar].spacing(3).width(Length::Fill));
    }

    let gap = || space().height(Length::Fill);
    let info = column![title, gap(), seed, gap(), form, gap(), knobs].width(INFO_WIDTH).height(Length::Fill);
    let info = container(info).width(INFO_WIDTH).height(Length::Fill).padding(Padding::from([4.0, 0.0]));
    let wave = live_wave(l.index as u64, Arc::clone(&l.columns), Arc::clone(&l.roll), l.wave, Length::Fill, Length::Fill);
    let mut words = column![].height(Length::Fill);
    if l.role == Role::Drums {
        // Top down, as the roll's rows (kick is the bottom row).
        for (k, w) in DRUM_WORDS.iter().enumerate() {
            let muted = l.muted & (1 << (7 - k)) != 0;
            words = words.push(
                container(text(w.to_string()).font(DATA_FONT).size(10).color(ink(if muted { 0.42 } else { dim })))
                    .height(Length::Fill)
                    .center_y(Length::Fill),
            );
        }
    }
    let body = row![info, container(words).width(LABELS_WIDTH).height(Length::Fill), wave].spacing(16).height(Length::Fill);
    container(body)
        .padding(Padding { top: 4.0, bottom: 4.0, left: 24.0, right: 24.0 })
        .height(Length::FillPortion(1))
        .width(Length::Fill)
        .into()
}

/// The whole screen; a `toast` replaces the status line while it shows.
pub fn view<'a, M: 'a>(f: &'a ScreenFrame, toast: Option<&'a str>) -> Element<'a, M> {
    let mut col = column![header(f)].spacing(6);
    for l in &f.lanes {
        col = col.push(lane(l));
    }
    let status = toast.unwrap_or(&f.status);
    if !status.is_empty() {
        col = col.push(container(text(status.to_string()).font(DATA_FONT).size(12).color(ink(0.6))).padding(Padding::from([0.0, 24.0])));
    }
    let content = container(col.padding(Padding { top: 0.0, bottom: 8.0, left: 0.0, right: 0.0 }))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_| container::Style { background: Some(Background::Color(BG)), ..Default::default() });
    stack![content, live_screen(ScreenStyle::default())].into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knobs_drive_effects_by_what_they_do() {
        assert_eq!(fx_of("dens"), Some(Fx::Weight));
        assert_eq!(fx_of("shape"), Some(Fx::Crush));
        assert_eq!(fx_of("morph"), Some(Fx::Rgb));
        assert_eq!(fx_of("verb"), Some(Fx::Trail));
        assert_eq!(fx_of("spread"), Some(Fx::Rumble));
        assert_eq!(fx_of("style"), None);
        let (span, ticks) = view_points(120.0, 48_000);
        assert_eq!(span, 1500.0);
        assert_eq!(ticks, 192.0);
    }

    fn lane_view() -> LaneView {
        let shared = mesh_core::live::state::LiveShared::new(120.0, mesh_core::music::MusicalKey::new(9, true), 0);
        shared.lane(0).view()
    }

    #[test]
    fn swaps_fire_on_seed_option_and_segment_changes() {
        let mut m = LaneMotion::default();
        let mut v = lane_view();
        m.update(&v);
        assert_eq!(m.swap, 0.0, "the first view is no change");
        m.update(&v);
        assert_eq!(m.swap, 0.0);
        v.seed += 1;
        m.update(&v);
        assert_eq!(m.swap, 1.0);
        m.update(&v);
        assert!(m.swap < 1.0);
        v.option += 1;
        m.update(&v);
        assert_eq!(m.swap, 1.0);
        v.pos.segment += 1;
        m.update(&m.last.map(|_| v).unwrap());
        assert_eq!(m.swap, 1.0);
    }

    #[test]
    fn kicks_passed_since_the_last_tick_shake() {
        let mut m = Motion::default();
        let mut v = lane_view();
        v.on = true;
        v.voices_on = 0xFF;
        v.drum_steps[DrumVoice::Kick as usize] = 1 << 4;
        v.active_step = 2;
        m.kick(&v);
        v.active_step = 3;
        m.kick(&v);
        assert_eq!(m.shake, 0.0);
        // Steps 4 and 5 passed in one tick: the kick on 4 counts.
        v.active_step = 5;
        m.kick(&v);
        assert_eq!(m.shake, 1.0);
        v.active_step = 6;
        m.kick(&v);
        assert!(m.shake < 1.0 && (m.shake - SHAKE_DECAY).abs() < 1e-6);
        // A muted kick does not shake.
        v.voices_on = !DrumVoice::Kick.bit();
        v.active_step = 4;
        m.shake = 0.0;
        m.kick(&v);
        assert_eq!(m.shake, 0.0);
    }

    #[test]
    fn roll_rows_by_voice_or_pitch_range() {
        let mut out = Vec::new();
        let hit = |tick, target| BarNote { tick, gate: 0, key: 0, vel: 100, target };
        assert_eq!(roll_notes(Role::Drums, &[hit(0, 0), hit(96, 1)], &mut out), 8);
        assert_eq!((out[0].row, out[1].row), (DrumVoice::Kick as u8, DrumVoice::Snare as u8));
        let note = |key| BarNote { tick: 0, gate: 48, key, vel: 100, target: 0 };
        // A fifth sits in the middle of the 12 rows.
        assert_eq!(roll_notes(Role::Bass, &[note(40), note(47)], &mut out), 12);
        assert_eq!((out[0].row, out[1].row), (2, 9));
        // Five octaves squeeze into 32 rows.
        assert_eq!(roll_notes(Role::Lead, &[note(30), note(90)], &mut out), 32);
        assert_eq!((out[0].row, out[1].row), (0, 31));
        assert_eq!(roll_notes(Role::Lead, &[], &mut out), 0);
    }

    #[test]
    fn follower_fires_on_hits_and_falls_on_held_levels() {
        let mut f = Follower::default();
        let mut points = vec![0.0f32; 64];
        f.update(&points, 64);
        assert!(f.env < 0.01);
        // A hit fires the effects.
        points[63] = 0.8;
        f.update(&points, 65);
        assert!(f.env > 0.9, "{}", f.env);
        // The level holds: the effects fall away.
        for w in 66..160 {
            f.update(&points, w);
        }
        assert!(f.env < 0.25, "{}", f.env);
        // Silence: nothing.
        points[63] = 0.0;
        for w in 160..200 {
            f.update(&points, w);
        }
        assert!(f.env < 0.01, "{}", f.env);
    }

    #[test]
    fn follower_reads_only_new_points() {
        let mut f = Follower::default();
        let mut points = vec![0.0f32; 64];
        points[0] = 1.0;
        f.update(&points, 64);
        let first = f.env;
        // No new points: the old peak does not hit again.
        f.update(&points, 64);
        assert!(f.env < first);
    }
}
