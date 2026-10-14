//! Kontrol F1 live layout (design §7): raw controller events → live commands,
//! and engine state → LEDs / 7-segment displays.

use mesh_core::live::engine::LiveController;
use mesh_core::live::state::LiveShared;
use mesh_midi::types::FeedbackCommand;
use mesh_midi::types::ControlEvent;

/// What the UI should highlight (selected lane/slot, shift state).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SurfaceView {
    pub selected_lane: u8,
    pub shift: bool,
}

/// The two-F1 live control surface.
pub struct LiveControlSurface {
    view: SurfaceView,
}

impl LiveControlSurface {
    /// `left_serial`: HID serial of the left F1 (`live.f1_left_serial`).
    pub fn new(_left_serial: Option<String>) -> Self {
        Self { view: SurfaceView::default() }
    }

    /// Handle one raw controller event, sending commands through `ctl`.
    pub fn handle(&mut self, _ev: &ControlEvent, _ctl: &mut LiveController) {}

    /// Called at ~30 Hz from the UI tick. Appends LED commands and
    /// 7-segment texts (device id, command / text).
    pub fn feedback(
        &mut self,
        _shared: &LiveShared,
        _ctl: &LiveController,
        _leds: &mut Vec<(String, FeedbackCommand)>,
        _displays: &mut Vec<(String, String)>,
    ) {
    }

    pub fn view(&self) -> SurfaceView {
        self.view
    }
}
