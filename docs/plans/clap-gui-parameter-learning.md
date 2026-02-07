# Implementation Plan: CLAP Plugin GUI & Parameter Learning

## Overview

Add floating plugin GUI windows and parameter learning (click knob → tweak plugin control → auto-assign) for CLAP effects in the multiband editor.

## Goals

1. **Open CLAP plugin GUIs** in floating windows
2. **Parameter learning mode**: Click a knob, then adjust plugin UI control to assign it
3. **Cross-platform**: Linux (X11), Windows (Win32), macOS (Cocoa) from the start

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Mesh Player (Iced)                       │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │              Multiband Editor Modal                      │    │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐                  │    │
│  │  │ Effect  │  │ Effect  │  │ Effect  │                  │    │
│  │  │ Card    │  │ Card    │  │ Card    │                  │    │
│  │  │ [GUI]   │  │ [GUI]   │  │ [GUI]   │ ← "Open GUI" btn │    │
│  │  │ K1 K2   │  │ K1 K2   │  │ K1 K2   │ ← Knobs (click   │    │
│  │  │ K3 K4   │  │ K3 K4   │  │ K3 K4   │   to learn)      │    │
│  │  └─────────┘  └─────────┘  └─────────┘                  │    │
│  └─────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ Message: OpenPluginGui(effect_id)
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                     PluginGuiManager                            │
│  - Tracks open plugin windows                                   │
│  - Receives param change events from audio thread               │
│  - Forwards learning events to UI                               │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ baseview window creation
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                 Floating Plugin GUI Window                      │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │              Plugin's Native GUI                         │    │
│  │         (rendered by CLAP plugin via GUI ext)           │    │
│  └─────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ ParamValueEvent (output events)
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                     Audio Thread                                │
│  ClapEffect::process() captures output events                   │
│  → Sends (param_id, old_value, new_value) via channel          │
│  → UI receives and completes learning assignment                │
└─────────────────────────────────────────────────────────────────┘
```

---

## Implementation Tasks

### Phase 1: Output Event Capture (Foundation)

**Files:** `crates/mesh-core/src/clap/plugin.rs`, `effect.rs`

#### Task 1.1: Replace OutputEvents::void() with real capture
- Change `OutputEvents::void()` to a real `EventBuffer`
- After `process()`, iterate output events looking for `ParamValueEvent`
- Store detected changes in a thread-safe queue

```rust
// In ClapPluginWrapper
pub struct ClapPluginWrapper {
    // ... existing fields ...
    param_change_sender: crossbeam_channel::Sender<ParamChange>,
}

pub struct ParamChange {
    pub param_id: u32,
    pub old_value: f64,
    pub new_value: f64,
}
```

#### Task 1.2: Create channel for param changes
- Add `crossbeam-channel` dependency
- Create bounded channel (capacity ~64 changes)
- Audio thread sends, UI thread receives

**Estimated time: 2-3 hours**

---

### Phase 2: Dependencies & Nix Setup

**Files:** `Cargo.toml`, `nix/common.nix`

#### Task 2.1: Add Cargo dependencies

```toml
# crates/mesh-core/Cargo.toml
[dependencies]
crossbeam-channel = "0.5"

# crates/mesh-player/Cargo.toml
[dependencies]
baseview = "0.1"
raw-window-handle = "0.5"
```

#### Task 2.2: Update clack-extensions features

```toml
# crates/mesh-core/Cargo.toml
clack-extensions = {
    git = "https://github.com/prokopyl/clack.git",
    features = ["params", "gui", "raw-window-handle_05", "clack-host"]
}
```

#### Task 2.3: Update Nix dependencies

```nix
# nix/common.nix - add to runtimeInputs
xorg.libxcb  # Required by baseview on Linux
```

**Estimated time: 30 minutes**

---

### Phase 3: HostGui Extension Implementation

**Files:** `crates/mesh-core/src/clap/plugin.rs`

#### Task 3.1: Register HostGui extension

```rust
impl HostHandlers for MeshClapHost {
    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder
            .register::<HostParams>()
            .register::<HostGui>();  // NEW
    }
}
```

#### Task 3.2: Implement HostGuiImpl trait

```rust
impl HostGuiImpl for MeshClapHostShared {
    fn request_resize(&mut self, width: u32, height: u32) -> bool {
        // Send resize request to window manager
        if let Some(sender) = &self.gui_event_sender {
            let _ = sender.send(GuiEvent::ResizeRequested(width, height));
        }
        true
    }

    fn request_show(&mut self) -> bool { true }
    fn request_hide(&mut self) -> bool { true }
    fn closed(&mut self, _was_destroyed: bool) {}
    fn resize_hints_changed(&mut self) {}
}
```

**Estimated time: 3-4 hours**

---

### Phase 4: Plugin GUI Window Manager

**Files:** New file `crates/mesh-player/src/plugin_gui/mod.rs`

#### Task 4.1: Create PluginGuiManager struct

```rust
pub struct PluginGuiManager {
    /// Active plugin windows: effect_id → window handle
    windows: HashMap<String, PluginGuiWindow>,

    /// Receiver for param changes from audio thread
    param_change_rx: crossbeam_channel::Receiver<ParamChange>,

    /// Currently learning: (effect_id, knob_index)
    learning_target: Option<(String, usize)>,
}

pub struct PluginGuiWindow {
    handle: baseview::WindowHandle,
    plugin_id: String,
    size: (u32, u32),
}
```

#### Task 4.2: Implement window creation with baseview

```rust
impl PluginGuiManager {
    pub fn open_gui(&mut self, effect: &ClapEffect) -> Result<(), String> {
        let options = WindowOpenOptions {
            title: format!("{} - Mesh", effect.name()).into(),
            size: baseview::Size::new(800.0, 600.0),
            scale: WindowScalePolicy::SystemScaleFactor,
            gl_config: None,
        };

        // Create window and pass handle to plugin
        let handle = Window::open_blocking(options, |window| {
            // Get raw window handle for CLAP
            let raw_handle = window.raw_window_handle();

            // Tell plugin to create GUI with this parent
            effect.create_gui(raw_handle)?;
            effect.show_gui()?;

            PluginGuiHandler::new(effect.id().to_string())
        });

        self.windows.insert(effect.id().to_string(), PluginGuiWindow {
            handle,
            plugin_id: effect.id().to_string(),
            size: (800, 600),
        });

        Ok(())
    }

    pub fn close_gui(&mut self, effect_id: &str) {
        if let Some(window) = self.windows.remove(effect_id) {
            window.handle.close();
        }
    }
}
```

#### Task 4.3: Implement GUI event polling

```rust
impl PluginGuiManager {
    /// Poll for param changes - call this from UI update loop
    pub fn poll_param_changes(&mut self) -> Vec<LearnedParam> {
        let mut learned = Vec::new();

        while let Ok(change) = self.param_change_rx.try_recv() {
            // If we're in learning mode for this effect...
            if let Some((effect_id, knob_idx)) = &self.learning_target {
                if change.effect_id == *effect_id {
                    learned.push(LearnedParam {
                        effect_id: effect_id.clone(),
                        knob_index: *knob_idx,
                        param_id: change.param_id,
                        param_name: change.param_name.clone(),
                    });
                    self.learning_target = None;  // Exit learning mode
                }
            }
        }

        learned
    }
}
```

**Estimated time: 1 day**

---

### Phase 5: ClapPluginWrapper GUI Methods

**Files:** `crates/mesh-core/src/clap/plugin.rs`

#### Task 5.1: Add GUI lifecycle methods

```rust
impl ClapPluginWrapper {
    /// Check if plugin supports GUI
    pub fn supports_gui(&self) -> bool {
        // Query plugin for GUI extension support
        self.instance.as_ref()
            .map(|i| i.get_extension::<PluginGui>().is_some())
            .unwrap_or(false)
    }

    /// Create plugin GUI (call before show)
    pub fn create_gui(&mut self, api: GuiApiType, is_floating: bool) -> ClapResult<()> {
        let instance = self.instance.as_mut().ok_or(ClapError::NotActivated)?;
        let gui = instance.get_extension::<PluginGui>()
            .ok_or(ClapError::GuiNotSupported)?;

        if !gui.is_api_supported(api, is_floating) {
            return Err(ClapError::GuiApiNotSupported(api));
        }

        gui.create(api, is_floating)
            .then_some(())
            .ok_or(ClapError::GuiCreationFailed)
    }

    /// Set parent window for embedded GUI
    pub fn set_gui_parent(&mut self, window: GuiWindow) -> ClapResult<()> {
        let instance = self.instance.as_mut().ok_or(ClapError::NotActivated)?;
        let gui = instance.get_extension::<PluginGui>()
            .ok_or(ClapError::GuiNotSupported)?;

        gui.set_parent(window)
            .then_some(())
            .ok_or(ClapError::GuiParentFailed)
    }

    /// Get GUI size
    pub fn get_gui_size(&mut self) -> ClapResult<(u32, u32)> {
        let instance = self.instance.as_mut().ok_or(ClapError::NotActivated)?;
        let gui = instance.get_extension::<PluginGui>()
            .ok_or(ClapError::GuiNotSupported)?;

        let mut size = (0u32, 0u32);
        gui.get_size(&mut size)
            .then_some(size)
            .ok_or(ClapError::GuiSizeFailed)
    }

    /// Show the GUI
    pub fn show_gui(&mut self) -> ClapResult<()> {
        let instance = self.instance.as_mut().ok_or(ClapError::NotActivated)?;
        let gui = instance.get_extension::<PluginGui>()
            .ok_or(ClapError::GuiNotSupported)?;

        gui.show()
            .then_some(())
            .ok_or(ClapError::GuiShowFailed)
    }

    /// Hide the GUI
    pub fn hide_gui(&mut self) -> ClapResult<()> {
        let instance = self.instance.as_mut().ok_or(ClapError::NotActivated)?;
        let gui = instance.get_extension::<PluginGui>()
            .ok_or(ClapError::GuiNotSupported)?;

        gui.hide()
            .then_some(())
            .ok_or(ClapError::GuiHideFailed)
    }

    /// Destroy GUI resources
    pub fn destroy_gui(&mut self) -> ClapResult<()> {
        let instance = self.instance.as_mut().ok_or(ClapError::NotActivated)?;
        let gui = instance.get_extension::<PluginGui>()
            .ok_or(ClapError::GuiNotSupported)?;

        gui.destroy()
            .then_some(())
            .ok_or(ClapError::GuiDestroyFailed)
    }
}
```

#### Task 5.2: Add new error variants

```rust
// In crates/mesh-core/src/clap/error.rs
pub enum ClapError {
    // ... existing variants ...
    GuiNotSupported,
    GuiApiNotSupported(GuiApiType),
    GuiCreationFailed,
    GuiParentFailed,
    GuiSizeFailed,
    GuiShowFailed,
    GuiHideFailed,
    GuiDestroyFailed,
}
```

**Estimated time: 4-6 hours**

---

### Phase 6: UI Integration - Learning Mode

**Files:** `crates/mesh-widgets/src/multiband/state.rs`, `message.rs`, `view.rs`

#### Task 6.1: Add learning mode state

```rust
// In MultibandEditorState
pub struct MultibandEditorState {
    // ... existing fields ...

    /// Currently learning: (location, effect_idx, knob_idx)
    pub learning_knob: Option<(EffectChainLocation, usize, usize)>,
}
```

#### Task 6.2: Add messages

```rust
// In MultibandEditorMessage
pub enum MultibandEditorMessage {
    // ... existing variants ...

    /// Open plugin GUI window
    OpenPluginGui {
        location: EffectChainLocation,
        effect: usize,
    },

    /// Close plugin GUI window
    ClosePluginGui {
        location: EffectChainLocation,
        effect: usize,
    },

    /// Start learning mode for a knob
    StartLearning {
        location: EffectChainLocation,
        effect: usize,
        knob: usize,
    },

    /// Cancel learning mode
    CancelLearning,

    /// Parameter was learned (from polling)
    ParamLearned {
        location: EffectChainLocation,
        effect: usize,
        knob: usize,
        param_id: u32,
        param_name: String,
    },
}
```

#### Task 6.3: Update effect card view

```rust
// In effect_card() and fx_effect_card()
fn effect_card<'a>(...) -> Element<'a, MultibandEditorMessage> {
    let header = row![
        text(&effect.name).size(13).color(name_color),
        Space::new().width(Length::Fill),
        // NEW: Open GUI button (only for CLAP effects with GUI support)
        if effect.has_gui {
            button(text("⚙").size(13))
                .padding([1, 3])
                .on_press(MultibandEditorMessage::OpenPluginGui {
                    location,
                    effect: effect_idx,
                })
        } else {
            Space::new().width(0.0).height(0.0)
        },
        // ... existing bypass and remove buttons ...
    ];

    // ... rest of function ...
}
```

#### Task 6.4: Update knob click to start learning

Replace param picker modal with learning mode:

```rust
// When knob label is clicked:
.on_press(MultibandEditorMessage::StartLearning {
    location,
    effect: effect_idx,
    knob: param_idx,
})
```

#### Task 6.5: Add learning mode visual indicator

```rust
// In knob rendering, check if this knob is in learning mode
let is_learning = state.learning_knob == Some((location, effect_idx, param_idx));

let label_color = if is_learning {
    Color::from_rgb(1.0, 0.5, 0.0)  // Orange pulsing
} else if is_mapped {
    Color::from_rgb(0.4, 0.8, 0.4)  // Green
} else {
    TEXT_SECONDARY
};

let label_text = if is_learning {
    "LEARN".to_string()
} else if let Some(macro_idx) = mapped_macro {
    format!("M{}", macro_idx + 1)
} else {
    param_name[..param_name.len().min(3)].to_string()
};
```

**Estimated time: 3-4 hours**

---

### Phase 7: Message Handlers

**Files:** `crates/mesh-player/src/ui/handlers/multiband.rs`, `domain/mod.rs`

#### Task 7.1: Handle OpenPluginGui message

```rust
MultibandEditorMessage::OpenPluginGui { location, effect } => {
    // Get effect ID from UI state
    let effect_id = get_effect_id(&app.multiband_editor, location, effect);

    // Tell domain to open GUI
    if let Err(e) = app.domain.open_clap_gui(&effect_id) {
        log::warn!("Failed to open plugin GUI: {}", e);
    }
}
```

#### Task 7.2: Handle StartLearning message

```rust
MultibandEditorMessage::StartLearning { location, effect, knob } => {
    // Set learning mode
    app.multiband_editor.learning_knob = Some((location, effect, knob));

    // Tell domain to start listening for param changes
    let effect_id = get_effect_id(&app.multiband_editor, location, effect);
    app.domain.start_param_learning(&effect_id, knob);
}
```

#### Task 7.3: Poll for learned params in subscription

```rust
// In app.rs subscription()
fn subscription(&self) -> Subscription<Message> {
    // ... existing subscriptions ...

    // Poll for parameter learning events
    let learning_sub = if self.multiband_editor.learning_knob.is_some() {
        iced::time::every(Duration::from_millis(50)).map(|_| {
            Message::Multiband(MultibandEditorMessage::PollLearning)
        })
    } else {
        Subscription::none()
    };

    Subscription::batch([
        // ... existing subs ...
        learning_sub,
    ])
}
```

**Estimated time: 2-3 hours**

---

### Phase 8: Domain Layer Integration

**Files:** `crates/mesh-player/src/domain/mod.rs`

#### Task 8.1: Add GUI manager to Domain

```rust
pub struct Domain {
    // ... existing fields ...
    plugin_gui_manager: PluginGuiManager,
}
```

#### Task 8.2: Add domain methods

```rust
impl Domain {
    pub fn open_clap_gui(&mut self, effect_id: &str) -> Result<(), String> {
        // Find the effect in the engine
        let effect = self.find_clap_effect(effect_id)?;

        // Open GUI via manager
        self.plugin_gui_manager.open_gui(effect)
    }

    pub fn close_clap_gui(&mut self, effect_id: &str) {
        self.plugin_gui_manager.close_gui(effect_id);
    }

    pub fn start_param_learning(&mut self, effect_id: &str, knob_idx: usize) {
        self.plugin_gui_manager.set_learning_target(effect_id.to_string(), knob_idx);
    }

    pub fn poll_param_learning(&mut self) -> Vec<LearnedParam> {
        self.plugin_gui_manager.poll_param_changes()
    }
}
```

**Estimated time: 2-3 hours**

---

### Phase 9: Testing & Polish

#### Task 9.1: Test with real plugins
- Test LSP Compressor (22 params)
- Test other CLAP plugins from ~/.clap
- Verify GUI opens/closes correctly
- Verify parameter learning works

#### Task 9.2: Handle edge cases
- Plugin doesn't support GUI → show tooltip "No GUI available"
- Plugin GUI closes externally → update UI state
- Learning mode timeout → cancel after 30s of no activity
- Escape key → cancel learning mode

#### Task 9.3: Add toast notifications
- "Mapped 'Attack' to Knob 3"
- "Learning mode cancelled"
- "Plugin GUI not supported"

**Estimated time: 1 day**

---

## File Summary

| File | Changes |
|------|---------|
| `Cargo.toml` (mesh-core) | Add crossbeam-channel, update clack-extensions features |
| `Cargo.toml` (mesh-player) | Add baseview, raw-window-handle |
| `nix/common.nix` | Add xorg.libxcb |
| `clap/plugin.rs` | Output event capture, HostGui impl, GUI methods |
| `clap/effect.rs` | Forward GUI methods, param change channel |
| `clap/error.rs` | New GUI error variants |
| `plugin_gui/mod.rs` | NEW: PluginGuiManager |
| `domain/mod.rs` | Add GUI manager, domain methods |
| `multiband/state.rs` | Add learning_knob field |
| `multiband/message.rs` | Add GUI/learning messages |
| `multiband/view.rs` | GUI button, learning mode visuals |
| `handlers/multiband.rs` | Handle new messages |
| `ui/app.rs` | Learning poll subscription |

---

## Total Estimated Time

| Phase | Time |
|-------|------|
| Phase 1: Output event capture | 2-3 hours |
| Phase 2: Dependencies | 30 min |
| Phase 3: HostGui extension | 3-4 hours |
| Phase 4: Window manager | 1 day |
| Phase 5: Plugin GUI methods | 4-6 hours |
| Phase 6: UI integration | 3-4 hours |
| Phase 7: Message handlers | 2-3 hours |
| Phase 8: Domain integration | 2-3 hours |
| Phase 9: Testing & polish | 1 day |
| **Total** | **3-5 days** |

---

## Success Criteria

1. ✅ Can click "Open GUI" button on CLAP effect cards
2. ✅ Plugin GUI appears in floating window
3. ✅ Can click on a knob to enter learning mode
4. ✅ Adjusting plugin control auto-assigns to that knob
5. ✅ Works on Linux (X11), Windows, macOS
6. ✅ Multiple plugin GUIs can be open simultaneously
7. ✅ Escape cancels learning mode
8. ✅ Clear visual feedback during learning mode
