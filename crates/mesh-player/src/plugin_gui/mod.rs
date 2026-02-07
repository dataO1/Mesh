//! Plugin GUI management for CLAP plugins
//!
//! This module provides floating window management for CLAP plugin GUIs,
//! enabling parameter learning through GUI interaction.

use std::collections::HashMap;

use baseview::WindowHandle;
use crossbeam::channel::{self, Receiver, Sender};

/// Events sent from the GUI manager to the UI
#[derive(Debug, Clone)]
pub enum PluginGuiEvent {
    /// A parameter was changed while in learning mode
    ParamLearned {
        /// The effect ID that owns the parameter
        effect_id: String,
        /// The CLAP parameter ID
        param_id: u32,
        /// The parameter name
        param_name: String,
    },
    /// Plugin GUI was closed
    GuiClosed {
        effect_id: String,
    },
}

/// Tracks an open plugin GUI window
struct OpenPluginGui {
    /// Window handle (for closing)
    #[allow(dead_code)]
    handle: WindowHandle,
    /// Effect ID
    effect_id: String,
    /// Window size
    #[allow(dead_code)]
    size: (u32, u32),
}

/// Learning mode target - which knob is waiting for parameter assignment
#[derive(Debug, Clone)]
pub struct LearningTarget {
    /// Effect ID we're learning from
    pub effect_id: String,
    /// Knob index to assign the learned parameter to
    pub knob_index: usize,
}

/// Manager for plugin GUI windows and parameter learning
pub struct PluginGuiManager {
    /// Currently open plugin GUIs: effect_id -> window info
    open_guis: HashMap<String, OpenPluginGui>,

    /// Current learning target (if any)
    learning_target: Option<LearningTarget>,

    /// Channel sender for GUI events to the UI
    event_sender: Sender<PluginGuiEvent>,

    /// Channel receiver for GUI events (UI polls this)
    event_receiver: Receiver<PluginGuiEvent>,
}

impl PluginGuiManager {
    /// Create a new plugin GUI manager
    pub fn new() -> Self {
        let (sender, receiver) = channel::bounded(64);
        Self {
            open_guis: HashMap::new(),
            learning_target: None,
            event_sender: sender,
            event_receiver: receiver,
        }
    }

    /// Check if a plugin GUI is currently open
    pub fn is_gui_open(&self, effect_id: &str) -> bool {
        self.open_guis.contains_key(effect_id)
    }

    /// Get the number of open GUIs
    pub fn open_gui_count(&self) -> usize {
        self.open_guis.len()
    }

    /// Start learning mode for a specific knob
    ///
    /// The next parameter change from the specified effect will be assigned
    /// to the given knob index.
    pub fn start_learning(&mut self, effect_id: String, knob_index: usize) {
        log::info!(
            "Started learning mode for effect '{}', knob {}",
            effect_id,
            knob_index
        );
        self.learning_target = Some(LearningTarget {
            effect_id,
            knob_index,
        });
    }

    /// Cancel learning mode
    pub fn cancel_learning(&mut self) {
        if self.learning_target.is_some() {
            log::info!("Cancelled learning mode");
            self.learning_target = None;
        }
    }

    /// Check if we're in learning mode
    pub fn is_learning(&self) -> bool {
        self.learning_target.is_some()
    }

    /// Get the current learning target
    pub fn learning_target(&self) -> Option<&LearningTarget> {
        self.learning_target.as_ref()
    }

    /// Poll for GUI events (call from UI update loop)
    pub fn poll_events(&self) -> Vec<PluginGuiEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_receiver.try_recv() {
            events.push(event);
        }
        events
    }

    /// Process parameter changes from a GUI handle, checking for learning mode
    ///
    /// Call this periodically with each GUI handle. If we're in learning mode
    /// for this effect, the first change will trigger a ParamLearned event.
    ///
    /// Returns the learned parameter info if learning completed.
    pub fn process_param_changes(
        &mut self,
        effect_id: &str,
        gui_handle: &ClapGuiHandle,
    ) -> Option<(u32, String, usize)> {
        let changes = gui_handle.poll_param_changes();

        if changes.is_empty() {
            return None;
        }

        // Check if we're learning from this effect
        if let Some(target) = &self.learning_target {
            if target.effect_id == effect_id {
                // Use the first change as the learned parameter
                let change = &changes[0];
                let param_name = gui_handle
                    .param_name_for_id(change.param_id)
                    .unwrap_or_else(|| format!("Param {}", change.param_id));

                let knob_index = target.knob_index;
                let param_id = change.param_id;

                // Send event to UI
                let _ = self.event_sender.try_send(PluginGuiEvent::ParamLearned {
                    effect_id: effect_id.to_string(),
                    param_id,
                    param_name: param_name.clone(),
                });

                // Clear learning mode
                self.learning_target = None;

                log::info!(
                    "Learned parameter '{}' (id={}) for knob {}",
                    param_name,
                    param_id,
                    knob_index
                );

                return Some((param_id, param_name, knob_index));
            }
        }

        None
    }

    /// Close a plugin GUI
    pub fn close_gui(&mut self, effect_id: &str) {
        if let Some(_gui) = self.open_guis.remove(effect_id) {
            log::info!("Closed GUI for effect '{}'", effect_id);
            // Window will be closed when handle is dropped
        }
    }

    /// Close all plugin GUIs
    pub fn close_all_guis(&mut self) {
        let ids: Vec<_> = self.open_guis.keys().cloned().collect();
        for id in ids {
            self.close_gui(&id);
        }
    }
}

impl Default for PluginGuiManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PluginGuiManager {
    fn drop(&mut self) {
        self.close_all_guis();
    }
}

// Re-export types used by consumers
pub use mesh_core::clap::{ParamChangeEvent, ClapGuiHandle};
