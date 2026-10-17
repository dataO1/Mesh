//! Strudel-like code readout of one lane (design §8, D33).
//!
//! [`format_lane`] is a pure function from the lane's static description and
//! a snapshot of its live state to one line of text plus highlight ranges.
//! The view turns the ranges into rich-text spans; nothing is parsed.
//!
//! ```text
//! drums.kit("clean-kit").s("kick snare hats ride shaker perc top brk").style("rolling").dens(0.62).seed(  77).form("A7 C1")
//! bass.sampler("reese-sample").cx(0.40).dens(0.50).drive(0.40).duck(0.50).seed(  51).form("A3 B1").oct(+1).rate("half")
//! ```
//!
//! The playing segment of the form is marked [`mark::PLAYING`], muted drum
//! voices [`mark::MUTED`], a queued form or mute [`mark::PENDING`].
//!
//! Every token whose content can change is padded to a fixed width, so with
//! a monospace font the line never shifts while values move.

use std::ops::Range;

use mesh_core::live::glue::Role;
use mesh_core::live::generator::layout::{voice_override, DrumVoice};
use mesh_core::live::state::{move_bits, LaneView, Rate, SpanTag};
use mesh_core::live::style::{ControlSpec, ControlTarget, CONTROLS_PER_LANE};

/// Highlight flags of a text range (combinable).
pub mod mark {
    /// Selected element (unused by the lane line, kept for the view).
    pub const SELECTED: u8 = 1 << 0;
    /// Value changed recently (its control is moving).
    pub const CHANGED: u8 = 1 << 1;
    /// Part that fired an event just now.
    pub const FIRING: u8 = 1 << 2;
    /// Form segment playing in this bar.
    pub const PLAYING: u8 = 1 << 3;
    /// Queued for the next bar (option browse, lane toggle).
    pub const PENDING: u8 = 1 << 4;
    /// Lane is muted.
    pub const MUTED: u8 = 1 << 5;
    /// Momentary move held.
    pub const MOVE: u8 = 1 << 6;
    /// Dimmed text (unused by the lane line, kept for the view).
    pub const BEYOND: u8 = 1 << 7;
}

/// What plays a lane option, shown as the method name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstrumentKind {
    Kit,
    Sampler,
    Synth,
}

impl InstrumentKind {
    fn method(self) -> &'static str {
        match self {
            InstrumentKind::Kit => "kit",
            InstrumentKind::Sampler => "sampler",
            InstrumentKind::Synth => "synth",
        }
    }
}

/// One performance control as the readout shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlInfo {
    pub label: String,
    pub spec: ControlSpec,
    /// Named positions instead of a number (the drum lane's archetypes on
    /// its `style` control).
    pub choices: Vec<String>,
}

/// Static description of a lane (from the style, fixed for the session).
#[derive(Debug, Clone, PartialEq)]
pub struct LaneInfo {
    pub name: String,
    pub role: Role,
    pub option_names: Vec<String>,
    /// Instrument kind per option (same length as `option_names`).
    pub option_kinds: Vec<InstrumentKind>,
    pub controls: Vec<ControlInfo>,
    /// The style's forms as text (`"A7 C1"`), index = form.
    pub forms: Vec<String>,
}

impl LaneInfo {
    /// Width of the form token, so form changes keep the layout.
    fn form_width(&self) -> usize {
        self.forms
            .iter()
            .map(|f| {
                let segs: Vec<&str> = f.split_whitespace().collect();
                (0..segs.len().max(1))
                    .map(|p| {
                        let (from, to) = form_window(segs.len(), p);
                        let marks = ELLIPSIS_CHARS * ((from > 0) as usize + (to < segs.len()) as usize);
                        segs[from..to].iter().map(|s| s.chars().count() + 1).sum::<usize>().saturating_sub(1) + marks
                    })
                    .max()
                    .unwrap_or(0)
                    + 9
            })
            .max()
            .unwrap_or(0)
    }

    /// Width of the instrument token, so option changes keep the layout.
    fn option_width(&self) -> usize {
        self.option_names
            .iter()
            .zip(self.option_kinds.iter().chain(std::iter::repeat(&InstrumentKind::Sampler)))
            .map(|(n, k)| k.method().len() + short_name(n).chars().count() + 5)
            .max()
            .unwrap_or(0)
    }
}

/// The drum lane's voice token: word and the tag its events fire with.
const DRUM_VOICES: [(DrumVoice, SpanTag, &str); 8] = [
    (DrumVoice::Kick, SpanTag::Backbone, "kick"),
    (DrumVoice::Snare, SpanTag::Backbone, "snare"),
    (DrumVoice::Hats, SpanTag::Hats, "hats"),
    (DrumVoice::Ride, SpanTag::Ride, "ride"),
    (DrumVoice::Shaker, SpanTag::Shaker, "shaker"),
    (DrumVoice::Perc, SpanTag::Perc, "perc"),
    (DrumVoice::Top, SpanTag::Top, "top"),
    (DrumVoice::Break, SpanTag::Break, "brk"),
];

/// Move tokens appended while held, by [`move_bits`].
const MOVES: [(u8, &str); 4] = [
    (move_bits::STUTTER, ".stut()"),
    (move_bits::FILL, ".fill()"),
    (move_bits::DOUBLE, ".dbl()"),
    (move_bits::VOID, ".void()"),
];

/// Live inputs of one readout line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaneFrame {
    pub view: LaneView,
    /// Controls whose position changed recently.
    pub changed_controls: [bool; CONTROLS_PER_LANE],
    /// The master seed changed recently.
    pub changed_seed: bool,
    /// Tag of an event fired just now.
    pub firing: Option<SpanTag>,
}

/// One formatted line: text plus `(byte range, mark flags)` highlights.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CodeLine {
    pub text: String,
    pub marks: Vec<(Range<usize>, u8)>,
}

impl CodeLine {
    fn push(&mut self, s: &str, flags: u8) {
        let start = self.text.len();
        self.text.push_str(s);
        if flags != 0 {
            self.marks.push((start..self.text.len(), flags));
        }
    }

    fn pad_to(&mut self, start: usize, width: usize) {
        let used = self.text[start..].chars().count();
        for _ in used..width {
            self.text.push(' ');
        }
    }

    /// Split the text into consecutive segments with the union of the marks
    /// covering each one.
    pub fn segments(&self) -> Vec<(Range<usize>, u8)> {
        let mut cuts: Vec<usize> = vec![0, self.text.len()];
        for (r, _) in &self.marks {
            cuts.push(r.start);
            cuts.push(r.end);
        }
        cuts.sort_unstable();
        cuts.dedup();
        cuts.windows(2)
            .filter(|w| w[0] < w[1])
            .map(|w| {
                let flags = self
                    .marks
                    .iter()
                    .filter(|(r, _)| r.start <= w[0] && w[1] <= r.end)
                    .fold(0, |f, (_, m)| f | m);
                (w[0]..w[1], flags)
            })
            .collect()
    }
}

/// Number with a stable width for values in the usual control ranges.
pub fn format_value(v: f32) -> String {
    if !v.is_finite() {
        return " nan".into();
    }
    let a = v.abs();
    let s = if a >= 100.0 {
        format!("{v:.0}")
    } else if a >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    };
    format!("{s:>4}")
}

/// Index of the named choice a mapped control value selects (nearest).
pub fn choice_index(value: f32, count: usize) -> usize {
    if count <= 1 || !value.is_finite() {
        return 0;
    }
    ((value.clamp(0.0, 1.0) * (count - 1) as f32).round() as usize).min(count - 1)
}

/// Format one lane.
pub fn format_lane(info: &LaneInfo, f: &LaneFrame) -> CodeLine {
    let v = &f.view;
    let mut line = CodeLine::default();
    let muted = !v.on;
    line.push(&info.name, if muted { mark::MUTED } else { 0 });

    // Instrument option: the browsed / queued one while pending.
    let shown = v.option_pending.unwrap_or(v.option) as usize;
    let kind = info.option_kinds.get(shown).copied().unwrap_or(InstrumentKind::Sampler);
    let name = info.option_names.get(shown).map(|n| short_name(n)).unwrap_or_else(|| "?".into());
    let start = line.text.len();
    let mut flags = 0;
    if v.option_pending.is_some() {
        flags |= mark::PENDING;
    }
    let instrument_fires = matches!(f.firing, Some(SpanTag::Note | SpanTag::Tie | SpanTag::Accent));
    if instrument_fires && info.role != Role::Drums {
        flags |= mark::FIRING;
    }
    line.push(&format!(".{}(\"{name}\")", kind.method()), flags);
    line.pad_to(start, info.option_width());

    if info.role == Role::Drums {
        line.push(".s(\"", 0);
        for (i, (voice, tag, word)) in DRUM_VOICES.iter().enumerate() {
            if i > 0 {
                line.push(" ", 0);
            }
            let bit = voice.bit();
            let mut flags = 0;
            if v.voices_on & bit == 0 {
                flags |= mark::MUTED;
            } else if f.firing == Some(*tag) {
                flags |= mark::FIRING;
            }
            if (voice_override::apply(v.voice_default, v.voice_overrides) ^ v.voices_on) & bit != 0 {
                flags |= mark::PENDING;
            }
            line.push(word, flags);
        }
        line.push("\")", 0);
    }

    for (ci, c) in info.controls.iter().enumerate().take(CONTROLS_PER_LANE) {
        let position = v.controls[ci];
        let value = c.spec.map(position);
        let flags = if f.changed_controls[ci] { mark::CHANGED } else { 0 };
        let start = line.text.len();
        if c.choices.is_empty() {
            line.push(&format!(".{}({})", c.label, format_value(value)), flags);
        } else {
            let choice = &c.choices[choice_index(value, c.choices.len())];
            line.push(&format!(".{}(\"{choice}\")", c.label), flags);
            let width = c.label.len() + 5 + c.choices.iter().map(|s| s.chars().count()).max().unwrap_or(0);
            line.pad_to(start, width);
        }
    }

    line.push(&format!(".seed({:>4})", v.seed % 10_000), if f.changed_seed { mark::CHANGED } else { 0 });

    // Form: the queued one while pending, its playing segment marked.
    let start = line.text.len();
    let pending = v.form_pending.is_some();
    let shown = v.form_pending.unwrap_or(v.form) as usize;
    line.push(".form(\"", if pending { mark::PENDING } else { 0 });
    if let Some(form) = info.forms.get(shown) {
        // Long forms show a window of segments around the playing one.
        let segs: Vec<&str> = form.split_whitespace().collect();
        let playing_at = if pending { 0 } else { v.pos.segment as usize };
        let (from, to) = form_window(segs.len(), playing_at);
        if from > 0 {
            line.push("… ", 0);
        }
        for (i, segment) in segs.iter().enumerate().take(to).skip(from) {
            if i > from {
                line.push(" ", 0);
            }
            let playing = !pending && v.on && i == v.pos.segment as usize;
            let flags = if pending { mark::PENDING } else if playing { mark::PLAYING } else { 0 };
            line.push(segment, flags);
        }
        if to < segs.len() {
            line.push(" …", 0);
        }
    }
    line.push("\")", if pending { mark::PENDING } else { 0 });
    line.pad_to(start, info.form_width());

    if info.role != Role::Drums {
        line.push(&format!(".oct({:+})", v.octave), 0);
        let rate = match v.rate {
            Rate::Normal => "norm",
            Rate::Half => "half",
            Rate::Double => "dbl ",
        };
        line.push(&format!(".rate(\"{rate}\")"), 0);
    }

    for (bit, token) in MOVES {
        if v.moves & bit != 0 {
            line.push(token, mark::MOVE);
        }
    }
    if v.on != v.on_target {
        line.push(if v.on_target { "  // → on at bar" } else { "  // → off at bar" }, mark::PENDING);
    } else if muted {
        line.push("  // muted", mark::MUTED);
    }
    line
}

/// Longest option name shown before it is cut with an ellipsis.
const NAME_MAX_CHARS: usize = 24;

/// Option name for the line: a `+`-joined set (the folder pairs of a kit)
/// shows its first member and `+N` for the rest, and no name is longer than
/// [`NAME_MAX_CHARS`], so the line can wrap between tokens only.
fn short_name(name: &str) -> String {
    let mut parts = name.split('+');
    let first = parts.next().unwrap_or("");
    let rest = parts.count();
    let mut out: String = if first.chars().count() > NAME_MAX_CHARS {
        first.chars().take(NAME_MAX_CHARS - 1).chain(std::iter::once('…')).collect()
    } else {
        first.to_string()
    };
    if rest > 0 {
        out.push_str(&format!("+{rest}"));
    }
    out
}

/// Segments a long form shows at once, and how many before the playing one.
const FORM_WINDOW: usize = 10;
const FORM_WINDOW_BEFORE: usize = 2;
/// Characters of a window's "… " / " …" marks.
const ELLIPSIS_CHARS: usize = 2;

/// Segments `from..to` of a form of `len` segments shown while segment
/// `playing` plays (an inserted break, out of range, shows the start).
pub(super) fn form_window(len: usize, playing: usize) -> (usize, usize) {
    if len <= FORM_WINDOW {
        return (0, len);
    }
    let playing = if playing < len { playing } else { 0 };
    let from = playing.saturating_sub(FORM_WINDOW_BEFORE).min(len - FORM_WINDOW);
    (from, from + FORM_WINDOW)
}

/// Readout description of a control, with archetype names for the drum
/// lane's `style` generator parameter.
pub fn control_info(label: &str, spec: &ControlSpec, role: Role, archetypes: &[String]) -> ControlInfo {
    let is_style = matches!(&spec.target, ControlTarget::Generator { param } if param == "style");
    let choices = if role == Role::Drums && is_style { archetypes.to_vec() } else { Vec::new() };
    ControlInfo { label: label.to_string(), spec: spec.clone(), choices }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesh_core::live::state::{FiredView, FormView};
    use mesh_core::live::style::Curve;

    fn spec(target: ControlTarget) -> ControlSpec {
        ControlSpec { label: None, target, min: 0.0, max: 1.0, curve: Curve::default(), default: 0.5, also: Vec::new() }
    }

    fn generator(param: &str) -> ControlTarget {
        ControlTarget::Generator { param: param.into() }
    }

    fn drums() -> LaneInfo {
        let arch: Vec<String> = ["half_time", "rolling", "jungle"].iter().map(|s| s.to_string()).collect();
        LaneInfo {
            name: "drums".into(),
            role: Role::Drums,
            option_names: vec!["clean-kit".into(), "dirty".into()],
            option_kinds: vec![InstrumentKind::Kit, InstrumentKind::Kit],
            controls: vec![
                control_info("style", &spec(generator("style")), Role::Drums, &arch),
                control_info("dens", &spec(generator("density")), Role::Drums, &arch),
                control_info("cut", &ControlSpec { min: 20.0, max: 2000.0, ..spec(generator("x")) }, Role::Drums, &arch),
                control_info("verb", &spec(generator("y")), Role::Drums, &arch),
            ],
            forms: forms(),
        }
    }

    fn bass() -> LaneInfo {
        LaneInfo {
            name: "bass".into(),
            role: Role::Bass,
            option_names: vec!["reese".into(), "surge-reese".into()],
            option_kinds: vec![InstrumentKind::Sampler, InstrumentKind::Synth],
            controls: ["cx", "dens", "drive", "duck"]
                .iter()
                .map(|l| control_info(l, &spec(generator(l)), Role::Bass, &[]))
                .collect(),
            forms: forms(),
        }
    }

    #[test]
    fn short_name_keeps_first_member_and_counts_rest() {
        assert_eq!(short_name("clean-kit"), "clean-kit");
        assert_eq!(short_name("pair-18+ES_31s_05(C#)_175+library+library"), "pair-18+3");
        let s = short_name(&format!("{}+b", "x".repeat(40)));
        assert_eq!(s.chars().count(), NAME_MAX_CHARS + 2);
        assert!(s.contains('…') && s.ends_with("+1"));
    }

    #[test]
    fn long_kit_name_is_shortened_in_line() {
        let mut info = drums();
        info.option_names[1] = "pair-18+ES_31s_05(C#)_175+library+library+library".into();
        let line = format_lane(&info, &frame(LaneView { option: 1, ..view() }));
        assert!(line.text.contains(".kit(\"pair-18+4\")"), "{}", line.text);
    }

    fn forms() -> Vec<String> {
        vec!["A3 B1".into(), "A7 B1 A'7 C1".into()]
    }

    fn view() -> LaneView {
        LaneView {
            on: true,
            on_target: true,
            seed: 77,
            prev_seed: 50,
            form: 1,
            form_pending: None,
            pos: FormView { segment: 1, letter: 1, prime: false, chord: 0 },
            octave: 0,
            rate: Rate::Normal,
            voice_overrides: 0,
            voice_overrides_playing: 0,
            voice_default: 0xFF,
            voices_on: 0xFF,
            controls: [0.5, 0.62, 0.0, 0.25],
            shift_controls: [0.5; 4],
            option: 0,
            option_pending: None,
            active_step: 0,
            fired: FiredView { count: 0, tag: SpanTag::None, fill: false },
            voices: 0,
            moves: 0,
            drum_steps: [0; 8],
        }
    }

    fn frame(view: LaneView) -> LaneFrame {
        LaneFrame { view, changed_controls: [false; 4], changed_seed: false, firing: None }
    }

    fn marked<'a>(line: &'a CodeLine, flag: u8) -> Vec<&'a str> {
        line.marks.iter().filter(|(_, f)| f & flag != 0).map(|(r, _)| &line.text[r.clone()]).collect()
    }

    #[test]
    fn drum_line_reads_like_code() {
        let line = format_lane(&drums(), &frame(view()));
        assert_eq!(
            line.text,
            "drums.kit(\"clean-kit\").s(\"kick snare hats ride shaker perc top brk\").style(\"rolling\")  .dens(0.62)\
             .cut(20.0).verb(0.25).seed(  77).form(\"A7 B1 A'7 C1\")"
        );
        assert_eq!(marked(&line, mark::PLAYING), vec!["B1"], "segment 1 plays");
        let line = format_lane(&bass(), &frame(view()));
        assert!(line.text.ends_with(".seed(  77).form(\"A7 B1 A'7 C1\").oct(+0).rate(\"norm\")"), "{}", line.text);
    }

    #[test]
    fn layout_is_stable_while_values_move() {
        let info = drums();
        let base = format_lane(&info, &frame(view()));
        let mut v = view();
        v.controls = [1.0, 0.07, 1.0, 0.99];
        v.seed = 3;
        v.form = 0;
        v.option = 1;
        let moved = format_lane(&info, &frame(v));
        assert_eq!(base.text.chars().count(), moved.text.chars().count());
        assert!(moved.text.contains(".style(\"jungle\")   .dens"), "{}", moved.text);
        assert!(moved.text.contains(".kit(\"dirty\")    "), "{}", moved.text);
        assert!(moved.text.contains(".cut(2000)"), "{}", moved.text);
    }

    #[test]
    fn changed_values_and_firing_parts_are_marked() {
        let mut f = frame(view());
        f.changed_controls[1] = true;
        f.changed_seed = true;
        f.firing = Some(SpanTag::Hats);
        let line = format_lane(&drums(), &f);
        assert_eq!(marked(&line, mark::CHANGED), vec![".dens(0.62)", ".seed(  77)"]);
        assert_eq!(marked(&line, mark::FIRING), vec!["hats"]);

        let mut f = frame(view());
        f.firing = Some(SpanTag::Note);
        let line = format_lane(&bass(), &f);
        assert_eq!(marked(&line, mark::FIRING), vec![".sampler(\"reese\")"]);
    }

    #[test]
    fn pending_option_muted_lane_and_moves() {
        let mut v = view();
        v.option_pending = Some(1);
        v.on = false;
        v.on_target = false;
        v.moves = move_bits::STUTTER | move_bits::VOID;
        let line = format_lane(&bass(), &frame(v));
        assert_eq!(marked(&line, mark::PENDING), vec![".synth(\"surge-reese\")"]);
        assert!(line.text.ends_with(".stut().void()  // muted"), "{}", line.text);
        assert_eq!(marked(&line, mark::PLAYING), Vec::<&str>::new(), "muted lanes show no playhead");

        let mut v = view();
        v.on_target = false;
        let line = format_lane(&bass(), &frame(v));
        assert!(line.text.ends_with("// → off at bar"));
    }

    #[test]
    fn segments_cover_text_and_merge_overlaps() {
        let line = CodeLine { text: "abcdef".into(), marks: vec![(1..4, mark::CHANGED), (2..3, mark::FIRING)] };
        let segs = line.segments();
        assert_eq!(
            segs,
            vec![(0..1, 0), (1..2, mark::CHANGED), (2..3, mark::CHANGED | mark::FIRING), (3..4, mark::CHANGED), (4..6, 0)]
        );
    }

    #[test]
    fn mutes_pending_forms_octave_and_rate() {
        let mut v = view();
        // Hats off now, the top loop's override lands on the next bar.
        v.voice_overrides = ((DrumVoice::Hats.bit() | DrumVoice::Top.bit()) as u16) << 8;
        v.voice_overrides_playing = (DrumVoice::Hats.bit() as u16) << 8;
        v.voices_on = 0xFF & !DrumVoice::Hats.bit();
        let line = format_lane(&drums(), &frame(v));
        assert_eq!(marked(&line, mark::MUTED), vec!["hats"]);
        assert_eq!(marked(&line, mark::PENDING), vec!["top"], "mute lands on the next bar");

        let mut v = view();
        v.form_pending = Some(0);
        v.octave = -2;
        v.rate = Rate::Double;
        let line = format_lane(&bass(), &frame(v));
        assert!(line.text.contains(".form(\"A3 B1\")"), "{}", line.text);
        assert!(marked(&line, mark::PENDING).contains(&"A3"), "queued form shown pending");
        assert!(marked(&line, mark::PLAYING).is_empty());
        assert!(line.text.ends_with(".oct(-2).rate(\"dbl \")"), "{}", line.text);
        let base = format_lane(&bass(), &frame(view()));
        assert_eq!(base.text.chars().count(), line.text.chars().count(), "form and rate keep the layout");
    }

    #[test]
    fn long_forms_show_a_window_around_the_playing_segment() {
        assert_eq!(form_window(4, 3), (0, 4), "short forms show whole");
        assert_eq!(form_window(30, 0), (0, FORM_WINDOW));
        assert_eq!(form_window(30, 12), (10, 10 + FORM_WINDOW));
        assert_eq!(form_window(30, 29), (30 - FORM_WINDOW, 30), "stays inside the form");
        assert_eq!(form_window(30, 255), (0, FORM_WINDOW), "an inserted break shows the start");
    }

    #[test]
    fn values_and_choices_format() {
        assert_eq!(format_value(0.5), "0.50");
        assert_eq!(format_value(12.34), "12.3");
        assert_eq!(format_value(440.0), " 440");
        assert_eq!(format_value(f32::NAN), " nan");
        assert_eq!(choice_index(0.0, 5), 0);
        assert_eq!(choice_index(0.4, 5), 2);
        assert_eq!(choice_index(1.0, 5), 4);
        assert_eq!(choice_index(0.7, 1), 0);
    }
}
