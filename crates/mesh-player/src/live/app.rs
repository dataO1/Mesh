//! `LiveApp`: the live-mode iced application (design §8).
//!
//! # Refresh model
//!
//! - Controller input and LED feedback run on the input thread
//!   ([`super::input`]), not on the UI loop.
//! - The UI ticks at [`FRAME_INTERVAL`] (~30 Hz). Each tick reads
//!   [`LiveShared`] (atomics, lock-free), snapshots the scope rings and builds
//!   the frame's texts once ([`Frame`]); `view` only borrows them.
//! - Scopes are one custom shader primitive each
//!   ([`mesh_widgets::scope`]): a 2 KB uniform write and one fullscreen
//!   triangle per lane per frame. Code readouts are rich text; highlight
//!   backgrounds and outlines are quads drawn by iced.
//!
//! # Recording and captures
//!
//! [`LiveApp::toggle_recording`] starts / stops a set recording of the
//! master bus (D37), on every USB stick with a mesh collection, else in the
//! local collection. It is bound to `Ctrl+R` and reachable as
//! [`Message::ToggleRecording`] for a future control-surface binding.
//! Captures (D16) are saved to the first USB mesh collection, else the
//! local one; a toast confirms each save.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::widget::{column, container, rich_text, row, space, span, text};
use iced::{keyboard, time, window, Background, Border, Color, Element, Font, Length, Padding, Size, Subscription, Task};

use mesh_core::live::command::LiveCommand;
use mesh_core::live::engine::{LiveHandles, DEFAULT_TEMPO_RAMP_BARS};
use mesh_core::live::glue::Role;
use mesh_core::live::render::RendererHandle;
use mesh_core::live::scope::SCOPE_POINTS;
use mesh_core::live::state::{move_bits, LiveShared, SpanTag};
use mesh_core::live::style::{InstrumentSource, Style, CONTROLS_PER_LANE};
use mesh_core::live::{NUM_LANES, SLOTS_PER_CHAIN};
use mesh_core::recording::{RecordingEvent, RecordingHandle};
use mesh_core::usb::{UsbDevice, UsbManager, UsbMessage};
use mesh_widgets::scope::{scope, ScopeStyle};
use mesh_widgets::theme::MeshTheme;

use super::controls::{LiveControlSurface, SurfaceView};
use super::header;
use super::input::{self, InputThread, LiveCtl, SharedCtl};
use super::keys::{self, KeyAction};
use super::readout::{self, mark, CodeLine, InstrumentKind, LaneFrame, LaneInfo};
use super::startup::StyleProblem;

/// UI refresh period (design §8: visuals at 30 Hz).
pub const FRAME_INTERVAL: Duration = Duration::from_millis(33);
/// How long a changed value stays outlined.
const CHANGED_HOLD: Duration = Duration::from_millis(700);
/// Length of the pulse of a firing part.
const FIRE_PULSE: Duration = Duration::from_millis(160);
/// How long a toast stays up.
const TOAST_HOLD: Duration = Duration::from_secs(3);
/// Scope gain floor: quieter lanes are not blown up beyond 1 / this.
const SCOPE_GAIN_FLOOR: f32 = 0.3;

const MONO: Font = Font::with_name("JetBrains Mono");

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    Key(keyboard::Event),
    Resized(Size),
    GotMonitorSize(Option<Size>),
    /// Start or stop the set recording (for a future control-surface binding).
    #[allow(dead_code)]
    ToggleRecording,
}

/// Everything `main` prepared for the live app.
pub enum LiveBoot {
    Running {
        style: Style,
        handles: LiveHandles,
        sample_rate: u32,
        /// Set when no audio device could be opened (the UI still runs).
        audio_error: Option<String>,
        collection: PathBuf,
        theme: MeshTheme,
        left_serial: Option<String>,
    },
    Failed { problem: StyleProblem, collection: PathBuf, theme: MeshTheme },
}

enum Screen {
    Running(Box<Running>),
    Failed(StyleProblem),
}

/// Per-lane UI tracking between ticks.
#[derive(Default)]
struct LaneTrack {
    controls: [f32; CONTROLS_PER_LANE],
    control_changed: [Option<Instant>; CONTROLS_PER_LANE],
    seeds: [u32; SLOTS_PER_CHAIN],
    seed_changed: [Option<Instant>; SLOTS_PER_CHAIN],
    fired_count: u32,
    fired: Option<(SpanTag, Instant)>,
    fill_at: Option<Instant>,
    seen: bool,
}

/// Texts and data of one frame, rebuilt each tick.
struct Frame {
    key: String,
    intent: Option<String>,
    bpm: String,
    half_time: String,
    progression: String,
    phrase: [bool; 4],
    quantize: bool,
    lines: [CodeLine; NUM_LANES],
    pulse: [f32; NUM_LANES],
    on: [bool; NUM_LANES],
    selected_slot: [u8; NUM_LANES],
    seeds: [[u32; SLOTS_PER_CHAIN]; NUM_LANES],
    fill_flash: [bool; NUM_LANES],
    scopes: [Arc<[f32]>; NUM_LANES],
    scope_gain: [f32; NUM_LANES],
    playing_slot: u8,
    surface: SurfaceView,
}

struct Recording {
    started: Instant,
    /// Dropping them finalizes the WAV files.
    _handles: Vec<RecordingHandle>,
    events: std::sync::mpsc::Receiver<RecordingEvent>,
    _tx: std::sync::mpsc::Sender<RecordingEvent>,
}

struct Running {
    ctl: SharedCtl,
    shared: Arc<LiveShared>,
    _renderer: RendererHandle,
    input: Option<InputThread>,
    style_name: String,
    lanes: [LaneInfo; NUM_LANES],
    progressions: Vec<String>,
    warnings: Vec<String>,
    sample_rate: u32,
    audio_error: Option<String>,
    collection: PathBuf,
    usb: UsbManager,
    usb_devices: Vec<UsbDevice>,
    capture_target: PathBuf,
    recording: Option<Recording>,
    tracks: [LaneTrack; NUM_LANES],
    scope_buf: Box<[f32; SCOPE_POINTS]>,
    frame: Frame,
    kb_lane: u8,
    intent: f32,
}

pub struct LiveApp {
    theme: MeshTheme,
    iced_theme: iced::Theme,
    collection: PathBuf,
    width: f32,
    screen: Screen,
    toast: Option<(String, Instant)>,
}

/// Lane colour from the theme's stem colours, by role.
fn lane_color(theme: &MeshTheme, role: Role) -> Color {
    // stems = [Vocals, Drums, Bass, Other]
    theme.stems[match role {
        Role::Drums => 1,
        Role::Bass => 2,
        Role::Lead => 0,
        Role::Extra => 3,
    }]
}

fn with_alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

fn lane_infos(style: &Style, handles: &LiveHandles) -> [LaneInfo; NUM_LANES] {
    let archetypes = style.spec.lanes.first().map(|l| l.archetypes.clone()).unwrap_or_default();
    std::array::from_fn(|i| {
        let lc = handles.controller.lane(i);
        let spec = style.spec.lanes.get(i);
        let option_kinds = (0..lc.option_names.len())
            .map(|o| {
                if lc.role == Role::Drums {
                    return InstrumentKind::Kit;
                }
                match spec.and_then(|s| s.options.get(o)).and_then(|opt| opt.source()) {
                    Some(InstrumentSource::Clap(_)) => InstrumentKind::Synth,
                    _ => InstrumentKind::Sampler,
                }
            })
            .collect();
        LaneInfo {
            name: lc.name.clone(),
            role: lc.role,
            option_names: lc.option_names.clone(),
            option_kinds,
            controls: lc
                .controls
                .iter()
                .map(|c| readout::control_info(&c.label, &c.spec, lc.role, &archetypes))
                .collect(),
        }
    })
}

fn empty_frame() -> Frame {
    let empty: Arc<[f32]> = Arc::from(vec![0.0f32; SCOPE_POINTS]);
    Frame {
        key: String::new(),
        intent: None,
        bpm: String::new(),
        half_time: String::new(),
        progression: String::new(),
        phrase: [false; 4],
        quantize: true,
        lines: Default::default(),
        pulse: [0.0; NUM_LANES],
        on: [true; NUM_LANES],
        selected_slot: [0; NUM_LANES],
        seeds: [[0; SLOTS_PER_CHAIN]; NUM_LANES],
        fill_flash: [false; NUM_LANES],
        scopes: std::array::from_fn(|_| Arc::clone(&empty)),
        scope_gain: [1.0; NUM_LANES],
        playing_slot: 0,
        surface: SurfaceView::default(),
    }
}

impl LiveApp {
    pub fn new(boot: LiveBoot) -> (Self, Task<Message>) {
        let (screen, theme, collection) = match boot {
            LiveBoot::Failed { problem, collection, theme } => (Screen::Failed(problem), theme, collection),
            LiveBoot::Running { style, handles, sample_rate, audio_error, collection, theme, left_serial } => {
                let lanes = lane_infos(&style, &handles);
                let LiveHandles { mut controller, shared, renderer, warnings } = handles;
                controller.set_capture_collection(Some(collection.clone()));
                let ctl: SharedCtl = Arc::new(std::sync::Mutex::new(LiveCtl {
                    controller,
                    surface: LiveControlSurface::new(left_serial),
                }));
                let input = match InputThread::spawn(Arc::clone(&ctl), Arc::clone(&shared)) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        log::warn!("[LIVE] input thread not started: {e}");
                        None
                    }
                };
                // Captures and recordings follow the USB stick (D16, D37).
                let usb = UsbManager::spawn(None);
                usb.set_monitor_paused(false);
                usb.refresh_devices();
                for w in &warnings {
                    log::warn!("[LIVE] style: {w}");
                }
                let style_name = style.spec.name.clone();
                let screen = Screen::Running(Box::new(Running {
                    ctl,
                    shared,
                    _renderer: renderer,
                    input,
                    style_name,
                    lanes,
                    progressions: style.spec.progressions.iter().map(|p| p.name.clone()).collect(),
                    warnings,
                    sample_rate,
                    audio_error,
                    capture_target: collection.clone(),
                    collection: collection.clone(),
                    usb,
                    usb_devices: Vec::new(),
                    recording: None,
                    tracks: Default::default(),
                    scope_buf: Box::new([0.0; SCOPE_POINTS]),
                    frame: empty_frame(),
                    kb_lane: 0,
                    intent: 0.0,
                }));
                (screen, theme, collection)
            }
        };
        let iced_theme = theme.iced_theme();
        let app = Self { theme, iced_theme, collection, width: 1920.0, screen, toast: None };
        let monitor = window::oldest().then(|id| match id {
            Some(id) => window::monitor_size(id).map(Message::GotMonitorSize),
            None => Task::done(Message::GotMonitorSize(None)),
        });
        (app, monitor)
    }

    pub fn theme(&self) -> iced::Theme {
        self.iced_theme.clone()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            time::every(FRAME_INTERVAL).map(|_| Message::Tick),
            keyboard::listen().map(Message::Key),
            window::resize_events().map(|(_, size)| Message::Resized(size)),
        ])
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => self.tick(),
            Message::Key(event) => self.key(event),
            Message::Resized(size) => self.width = size.width,
            Message::GotMonitorSize(Some(size)) => {
                log::info!("[LIVE] monitor size {}x{}", size.width, size.height);
                return window::oldest().then(move |id| match id {
                    Some(id) => window::resize(id, size),
                    None => Task::none(),
                });
            }
            Message::GotMonitorSize(None) => {}
            Message::ToggleRecording => self.toggle_recording(),
        }
        Task::none()
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn tick(&mut self) {
        let now = Instant::now();
        if self.toast.as_ref().is_some_and(|(_, at)| now.duration_since(*at) > TOAST_HOLD) {
            self.toast = None;
        }
        let Screen::Running(run) = &mut self.screen else { return };
        let mut toasts = Vec::new();

        // USB sticks: capture target follows the first mesh collection.
        while let Some(msg) = run.usb.try_recv() {
            run.handle_usb(msg);
        }
        let target = run.preferred_collection();
        let mut ctl = input::lock(&run.ctl);
        if target != run.capture_target {
            log::info!("[LIVE] captures now go to {}", target.display());
            ctl.controller.set_capture_collection(Some(target.clone()));
            run.capture_target = target;
        }
        while let Some(saved) = ctl.controller.take_saved_capture() {
            toasts.push(match saved {
                Ok(path) => format!("Capture saved · {}", path.display()),
                Err(e) => format!("Capture NOT saved: {e}"),
            });
        }
        let surface = ctl.surface.view();
        drop(ctl);

        if let Some(rec) = &run.recording {
            while let Ok(ev) = rec.events.try_recv() {
                match ev {
                    RecordingEvent::Error { path, message } => {
                        toasts.push(format!("Recording error ({}): {message}", path.display()))
                    }
                    RecordingEvent::Started { path } => log::info!("[LIVE] recording to {}", path.display()),
                    RecordingEvent::Stopped { path, .. } => log::info!("[LIVE] recording saved {}", path.display()),
                }
            }
        }

        run.build_frame(now, surface);
        for t in toasts {
            self.toast(t);
        }
    }

    fn selected_lane(run: &Running) -> u8 {
        if run.input.as_ref().is_some_and(|i| i.hid_devices() > 0) {
            run.frame.surface.selected_lane
        } else {
            run.kb_lane
        }
    }

    fn key(&mut self, event: keyboard::Event) {
        let Screen::Running(run) = &mut self.screen else { return };
        let lane = Self::selected_lane(run);
        let Some(action) = keys::map_key(&event, lane) else { return };
        let mut ctl = input::lock(&run.ctl);
        let c = &mut ctl.controller;
        match action {
            KeyAction::SelectLane(l) => run.kb_lane = l,
            KeyAction::Command(cmd) => c.send(cmd),
            KeyAction::ControlDelta { control, delta } => {
                let value = c.lane(lane as usize).positions[control as usize] + delta;
                c.send(LiveCommand::SetControl { lane, control, value: value.clamp(0.0, 1.0) });
            }
            KeyAction::IntentStep(step) => {
                run.intent = (run.intent + step).clamp(-1.0, 1.0);
                c.send(LiveCommand::SetKeyIntent(run.intent));
            }
            KeyAction::TempoDelta(delta) => {
                let bpm = run.shared.clock().target_bpm + delta;
                c.send(LiveCommand::RequestTempo { bpm, bars: DEFAULT_TEMPO_RAMP_BARS });
            }
            KeyAction::ToggleQuantize => {
                let on = !run.shared.quantize();
                c.send(LiveCommand::SetQuantize(on));
            }
            KeyAction::ToggleRecording => {
                drop(ctl);
                self.toggle_recording();
            }
        }
    }

    /// Start or stop the set recording of the master bus (D37).
    pub fn toggle_recording(&mut self) {
        let Screen::Running(run) = &mut self.screen else { return };
        if run.recording.take().is_some() {
            input::lock(&run.ctl).controller.send(LiveCommand::StopRecording);
            // Dropping the handles finalizes the WAV files.
            self.toast("Recording stopped");
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let targets = run.recording_targets();
        let local = targets.len() == 1 && targets[0].0 == run.collection;
        let mut handles = Vec::new();
        let mut errors = Vec::new();
        {
            let mut ctl = input::lock(&run.ctl);
            for (mount, free) in &targets {
                match mesh_core::recording::start_recording(mount, run.sample_rate, *free, tx.clone()) {
                    Ok((producer, handle)) => {
                        ctl.controller.send(LiveCommand::StartRecording(producer));
                        handles.push(handle);
                    }
                    Err(e) => {
                        log::error!("[LIVE] recording not started on {}: {e}", mount.display());
                        errors.push(e);
                    }
                }
            }
        }
        if handles.is_empty() {
            self.toast(format!("Recording failed: {}", errors.join("; ")));
            return;
        }
        let msg = if local {
            "Recording to local collection".to_string()
        } else {
            format!("Recording to {} USB stick(s)", handles.len())
        };
        run.recording = Some(Recording { started: Instant::now(), _handles: handles, events: rx, _tx: tx });
        self.toast(msg);
    }

    // ── View ──

    pub fn view(&self) -> Element<'_, Message> {
        let content = match &self.screen {
            Screen::Failed(problem) => self.view_problem(problem),
            Screen::Running(run) => self.view_running(run),
        };
        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(self.theme.ui.background)),
                ..Default::default()
            })
            .into()
    }

    fn code_size(&self) -> f32 {
        // ~115 monospace columns fill 60 % of the width (0.6 em per column).
        (self.width * 0.6 / (115.0 * 0.6)).clamp(11.0, 26.0).floor()
    }

    fn view_problem<'a>(&'a self, p: &'a StyleProblem) -> Element<'a, Message> {
        let ui = &self.theme.ui;
        let mono = |s: String| text(s).font(MONO).size(16).color(ui.text);
        let mut col = column![
            text("Live mode cannot start").size(30).color(ui.danger),
            text(&p.title).size(22).color(ui.text),
        ]
        .spacing(10);
        for d in &p.details {
            col = col.push(mono(d.clone()));
        }
        col = col.push(space().height(10));
        let styles_dir = mesh_core::live::style::styles_dir(&self.collection);
        if p.available.is_empty() {
            col = col.push(mono(format!("No styles found in {}", styles_dir.display())));
        } else {
            col = col.push(text(format!("Available styles in {}:", styles_dir.display())).color(ui.text));
            for s in &p.available {
                col = col.push(mono(format!("  • {s}")));
            }
        }
        col = col.push(space().height(10));
        col = col.push(
            text("Set `mode: live` and `live.style: <name>` in player-config.yaml, then restart.")
                .color(with_alpha(ui.text, 0.7)),
        );
        container(col).padding(40).center_x(Length::Fill).into()
    }

    fn view_running<'a>(&'a self, run: &'a Running) -> Element<'a, Message> {
        let ui = &self.theme.ui;
        let f = &run.frame;
        let size = self.code_size();
        let dim = with_alpha(ui.text, 0.55);
        let mono = move |s: String, c: Color| text(s).font(MONO).size(size).color(c);
        let selected = Self::selected_lane(run);

        // Header strip.
        let mut cells = row![].spacing(4);
        for lit in f.phrase {
            cells = cells.push(container(space().width(size * 0.7).height(size * 0.9)).style(move |_| {
                container::Style {
                    background: Some(Background::Color(if lit { ui.accent } else { with_alpha(ui.text, 0.15) })),
                    border: Border { radius: 2.0.into(), ..Default::default() },
                    ..Default::default()
                }
            }));
        }
        let mut head = row![
            mono(f.key.clone(), ui.text),
            mono(f.intent.clone().unwrap_or_default(), ui.warning),
            mono(f.bpm.clone(), ui.text),
            mono(f.half_time.clone(), ui.text),
            mono(f.progression.clone(), ui.text),
            cells,
            mono(if f.quantize { "Q bar".into() } else { "Q now".into() }, dim),
        ]
        .spacing(size * 1.5)
        .align_y(iced::Alignment::Center);
        head = head.push(space().width(Length::Fill));
        let lane_name = &run.lanes[selected as usize % NUM_LANES].name;
        head = head.push(mono(
            format!("{}{}", lane_name, if f.surface.shift { " · SHIFT" } else { "" }),
            lane_color(&self.theme, run.lanes[selected as usize % NUM_LANES].role),
        ));
        if let Some(rec) = &run.recording {
            head = head.push(mono(header::recording_text(rec.started.elapsed()), ui.danger));
        }

        let mut col = column![container(head).padding(Padding::from([6.0, 12.0]))].spacing(6);

        for li in 0..NUM_LANES {
            col = col.push(self.view_lane(run, li, li as u8 == selected));
        }

        // Status line: toast, otherwise warnings / audio state.
        let status = if let Some((t, _)) = &self.toast {
            mono(t.clone(), ui.success)
        } else if let Some(e) = &run.audio_error {
            mono(format!("No audio output: {e}"), ui.danger)
        } else {
            let mut s = format!("style {}", run.style_name);
            if !run.warnings.is_empty() {
                s.push_str(&format!(" · {} warning(s), see log", run.warnings.len()));
            }
            let starved = run.shared.starved_bars();
            if starved > 0 {
                s.push_str(&format!(" · {starved} late bar(s)"));
            }
            let hid = run.input.as_ref().map(|i| i.hid_devices()).unwrap_or(0);
            s.push_str(&format!(" · {hid} controller(s) · captures → {}", run.capture_target.display()));
            mono(s, dim)
        };
        col = col.push(container(status).padding(Padding::from([4.0, 12.0])));
        col.padding(8).into()
    }

    fn view_lane<'a>(&'a self, run: &'a Running, li: usize, selected: bool) -> Element<'a, Message> {
        let ui = &self.theme.ui;
        let f = &run.frame;
        let color = lane_color(&self.theme, run.lanes[li].role);
        let on = f.on[li];
        let size = self.code_size();
        let text_color = if on { ui.text } else { with_alpha(ui.text, 0.35) };
        let pulse = f.pulse[li];

        // Code readout.
        let line = &f.lines[li];
        let spans: Vec<_> = line
            .segments()
            .into_iter()
            .map(|(r, flags)| {
                let mut s = span::<(), Font>(&line.text[r]).font(MONO).color(text_color);
                if flags & mark::PENDING != 0 {
                    s = s.color(ui.warning);
                }
                if flags & mark::MUTED != 0 {
                    s = s.color(with_alpha(ui.text, 0.35));
                }
                let mut bg = None;
                let mut border = None;
                if flags & mark::PLAYING != 0 {
                    bg = Some(with_alpha(color, 0.18));
                }
                if flags & mark::CHANGED != 0 {
                    bg = Some(with_alpha(color, 0.22));
                    border = Some(Border { color, width: 1.0, radius: 3.0.into() });
                }
                if flags & mark::SELECTED != 0 {
                    border = Some(Border { color, width: 2.0, radius: 3.0.into() });
                }
                if flags & mark::MOVE != 0 {
                    bg = Some(with_alpha(color, 0.5));
                }
                if flags & mark::FIRING != 0 && pulse > 0.0 {
                    bg = Some(with_alpha(color, 0.2 + 0.6 * pulse));
                }
                if let Some(b) = bg {
                    s = s.background(Background::Color(b));
                }
                if let Some(b) = border {
                    s = s.border(b);
                }
                s
            })
            .collect();
        let code = rich_text(spans).size(size).wrapping(text::Wrapping::None);

        // Scope.
        let scope_style = ScopeStyle {
            color: if on { color } else { with_alpha(color, 0.3) },
            gain: f.scope_gain[li],
            thickness: 2.0,
            fill_alpha: 0.18,
        };
        let trace = scope(li as u64, Arc::clone(&f.scopes[li]), scope_style, Length::Fill, Length::Fill);

        // Chain grid.
        let mut chain = row![].spacing(6).height(Length::Fill);
        for slot in 0..SLOTS_PER_CHAIN {
            let playing = on && f.playing_slot as usize == slot;
            let sel = f.selected_slot[li] as usize == slot;
            let flash = playing && f.fill_flash[li];
            let bg = if flash {
                Color::WHITE
            } else if playing {
                with_alpha(color, 0.85)
            } else if on {
                with_alpha(color, 0.15)
            } else {
                with_alpha(ui.text, 0.05)
            };
            let fg = if playing || flash { ui.background } else { text_color };
            let cell = container(text(format!("{}", f.seeds[li][slot])).font(MONO).size(size * 0.8).color(fg))
                .center(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(Background::Color(bg)),
                    border: Border {
                        color: if sel { ui.text } else { with_alpha(color, 0.4) },
                        width: if sel { 2.0 } else { 1.0 },
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                });
            chain = chain.push(cell);
        }

        let body = row![
            container(code).width(Length::FillPortion(6)).center_y(Length::Fill),
            container(trace).width(Length::FillPortion(3)).height(Length::Fill),
            container(chain).width(Length::FillPortion(2)).height(Length::Fill),
        ]
        .spacing(16)
        .height(Length::Fill);

        container(body)
            .padding(Padding::from([6.0, 14.0]))
            .width(Length::Fill)
            .height(Length::FillPortion(1))
            .style(move |_| container::Style {
                background: Some(Background::Color(with_alpha(color, if selected { 0.10 } else { 0.04 }))),
                border: Border {
                    color: if selected { color } else { with_alpha(color, 0.25) },
                    width: if selected { 2.0 } else { 1.0 },
                    radius: 6.0.into(),
                },
                ..Default::default()
            })
            .into()
    }
}

impl Running {
    fn handle_usb(&mut self, msg: UsbMessage) {
        match msg {
            UsbMessage::DevicesRefreshed(devices) => self.usb_devices = devices,
            UsbMessage::DeviceConnected(d) => {
                self.usb_devices.retain(|x| x.device_path != d.device_path);
                self.usb_devices.push(d);
            }
            UsbMessage::DeviceDisconnected { device_path } => {
                self.usb_devices.retain(|x| x.device_path != device_path)
            }
            UsbMessage::MountComplete { result: Ok(d) } => {
                self.usb_devices.retain(|x| x.device_path != d.device_path);
                self.usb_devices.push(d);
            }
            _ => {}
        }
    }

    fn usb_collections(&self) -> impl Iterator<Item = &UsbDevice> {
        self.usb_devices.iter().filter(|d| d.has_mesh_collection && d.mount_point.is_some())
    }

    /// Capture target: first USB mesh collection, else the local one.
    fn preferred_collection(&self) -> PathBuf {
        self.usb_collections().find_map(|d| d.collection_root()).unwrap_or_else(|| self.collection.clone())
    }

    /// Recording targets like the DJ app: every USB stick with a mesh
    /// collection (its mount point and free space), else the local collection.
    fn recording_targets(&self) -> Vec<(PathBuf, u64)> {
        let usb: Vec<_> =
            self.usb_collections().filter_map(|d| d.mount_point.clone().map(|m| (m, d.available_bytes))).collect();
        if usb.is_empty() {
            vec![(self.collection.clone(), u64::MAX)]
        } else {
            usb
        }
    }

    fn build_frame(&mut self, now: Instant, surface: SurfaceView) {
        let shared = &self.shared;
        let clock = shared.clock();
        let key = shared.key();
        let pending = shared.pending_key();
        let f = &mut self.frame;
        f.key = header::key_text(key, pending);
        f.intent = header::intent_text(key, pending, shared.intent_key());
        f.bpm = header::bpm_text(&clock);
        f.half_time = header::half_time_text(shared.half_time(), shared.half_time_target());
        f.progression = header::progression_text(
            &self.progressions,
            shared.progression(),
            shared.pending_progression(),
            shared.chord_degree(),
        );
        f.phrase = header::phrase_cells(clock.bar_in_phrase);
        f.quantize = shared.quantize();
        f.playing_slot = clock.bar_in_phrase % SLOTS_PER_CHAIN as u8;
        f.surface = surface;

        let recent = |t: Option<Instant>, hold: Duration| t.is_some_and(|t| now.duration_since(t) < hold);
        for li in 0..NUM_LANES {
            let lane = shared.lane(li);
            let view = lane.view();
            let tr = &mut self.tracks[li];
            if tr.seen {
                for c in 0..CONTROLS_PER_LANE {
                    if (view.controls[c] - tr.controls[c]).abs() > 1e-4 {
                        tr.control_changed[c] = Some(now);
                    }
                }
                for s in 0..SLOTS_PER_CHAIN {
                    if view.seeds[s] != tr.seeds[s] {
                        tr.seed_changed[s] = Some(now);
                    }
                }
                if view.fired.count != tr.fired_count {
                    tr.fired = Some((view.fired.tag, now));
                    if view.fired.fill {
                        tr.fill_at = Some(now);
                    }
                }
            }
            tr.seen = true;
            tr.controls = view.controls;
            tr.seeds = view.seeds;
            tr.fired_count = view.fired.count;

            let firing = tr.fired.filter(|(_, t)| now.duration_since(*t) < FIRE_PULSE);
            f.pulse[li] = firing.map(|(_, t)| 1.0 - now.duration_since(t).as_secs_f32() / FIRE_PULSE.as_secs_f32()).unwrap_or(0.0);
            let frame = LaneFrame {
                view,
                playing_slot: f.playing_slot,
                changed_controls: std::array::from_fn(|c| recent(tr.control_changed[c], CHANGED_HOLD)),
                changed_seeds: std::array::from_fn(|s| recent(tr.seed_changed[s], CHANGED_HOLD)),
                firing: firing.map(|(tag, _)| tag),
            };
            f.lines[li] = readout::format_lane(&self.lanes[li], &frame);
            f.on[li] = view.on;
            f.selected_slot[li] = view.selected_slot;
            f.seeds[li] = view.seeds;
            f.fill_flash[li] = view.moves & move_bits::FILL != 0 || recent(tr.fill_at, FIRE_PULSE);

            lane.scope().snapshot(&mut self.scope_buf);
            let peak = self.scope_buf.iter().fold(0.0f32, |p, v| p.max(v.abs()));
            f.scope_gain[li] = 1.0 / peak.max(SCOPE_GAIN_FLOOR);
            f.scopes[li] = Arc::from(&self.scope_buf[..]);
        }
    }
}
