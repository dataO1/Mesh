//! Plugin GUI management for CLAP plugins
//!
//! This module provides parameter learning mode for CLAP plugin GUIs,
//! enabling users to assign plugin parameters to UI knobs by touching them.

use mesh_core::clap::ClapGuiHandle;

/// Learning mode target - which knob is waiting for parameter assignment
#[derive(Debug, Clone)]
pub struct LearningTarget {
    /// Effect instance ID we're learning from
    pub effect_instance_id: String,
    /// Knob index to assign the learned parameter to
    pub knob_index: usize,
}

/// Manager for plugin GUI learning mode
///
/// Tracks which knob is currently in "learn" mode and polls for parameter
/// changes from the corresponding plugin's GUI handle.
pub struct PluginGuiManager {
    /// Current learning target (if any)
    learning_target: Option<LearningTarget>,
}

impl PluginGuiManager {
    /// Create a new plugin GUI manager
    pub fn new() -> Self {
        Self {
            learning_target: None,
        }
    }

    /// Start learning mode for a specific knob
    ///
    /// The next parameter change from the specified effect will be assigned
    /// to the given knob index.
    pub fn start_learning(&mut self, effect_instance_id: String, knob_index: usize) {
        log::info!(
            "[CLAP_LEARN] Started learning mode for effect '{}', knob {}",
            effect_instance_id,
            knob_index
        );
        self.learning_target = Some(LearningTarget {
            effect_instance_id,
            knob_index,
        });
    }

    /// Cancel learning mode
    pub fn cancel_learning(&mut self) {
        if self.learning_target.is_some() {
            log::info!("[CLAP_LEARN] Cancelled learning mode");
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

    /// Poll for parameter changes from the given GUI handle
    ///
    /// If we're in learning mode for this effect and a parameter changed,
    /// returns (param_id, param_name, knob_index). Otherwise returns None.
    pub fn poll_learning_changes(
        &mut self,
        effect_instance_id: &str,
        gui_handle: &ClapGuiHandle,
    ) -> Option<(u32, String, usize)> {
        // Only process if we're learning from this specific effect
        let target = match &self.learning_target {
            Some(t) if t.effect_instance_id == effect_instance_id => t,
            _ => return None,
        };

        let knob_index = target.knob_index;

        // Poll for parameter changes
        let changes = gui_handle.poll_param_changes();
        if changes.is_empty() {
            return None;
        }

        // Use the first change as the learned parameter
        let change = &changes[0];
        let param_name = gui_handle
            .param_name_for_id(change.param_id)
            .unwrap_or_else(|| format!("Param {}", change.param_id));

        log::info!(
            "[CLAP_LEARN] Learned parameter '{}' (id={}) for knob {}",
            param_name,
            change.param_id,
            knob_index
        );

        // Clear learning mode after successful learning
        self.learning_target = None;

        Some((change.param_id, param_name, knob_index))
    }

    /// Get effect instance IDs that we need to poll
    ///
    /// Returns the effect_instance_id if we're in learning mode, None otherwise.
    pub fn effect_to_poll(&self) -> Option<&str> {
        self.learning_target.as_ref().map(|t| t.effect_instance_id.as_str())
    }
}

impl Default for PluginGuiManager {
    fn default() -> Self {
        Self::new()
    }
}
