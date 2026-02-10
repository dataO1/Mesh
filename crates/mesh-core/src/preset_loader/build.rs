//! Build a MultibandHost from a MultibandBuildSpec
//!
//! This function runs on the loader thread and performs all the expensive work:
//! - Creating CLAP/PD effect instances (file I/O, DSP initialization)
//! - Setting all parameter values
//! - Configuring bands, crossovers, dry/wet, macros

use crate::clap::ClapManager;
use crate::effect::multiband::MultibandHost;
use crate::pd::PdManager;

use super::{EffectSourceType, MultibandBuildSpec};

/// Build a fully-configured MultibandHost from a build spec.
///
/// This is the core function that replaces 300-1000+ individual engine commands
/// with direct API calls on the MultibandHost. Runs on the loader thread.
///
/// # Steps
/// 1. Create MultibandHost with single band (default)
/// 2. Add extra bands (N-1 for N bands)
/// 3. Set crossover frequencies
/// 4. Create + configure pre-fx effects
/// 5. For each band: create + configure effects, set gain/mute/solo/chain_dry_wet
/// 6. Create + configure post-fx effects
/// 7. Set all dry/wet values
/// 8. Add macro mappings
pub fn build_multiband(
    spec: &MultibandBuildSpec,
    buffer_size: usize,
    clap_manager: &mut ClapManager,
    pd_manager: &mut PdManager,
) -> Result<MultibandHost, String> {
    let mut multiband = MultibandHost::new(buffer_size);

    // ─────────────────────────────────────────────────────────────────────
    // Step 1: Add extra bands (MultibandHost starts with 1 band)
    // ─────────────────────────────────────────────────────────────────────
    for i in 1..spec.bands.len() {
        multiband.add_band().map_err(|e| {
            format!("Failed to add band {}: {}", i, e)
        })?;
    }

    // ─────────────────────────────────────────────────────────────────────
    // Step 2: Set crossover frequencies
    // ─────────────────────────────────────────────────────────────────────
    for (i, &freq) in spec.crossover_freqs.iter().enumerate() {
        multiband.set_crossover_frequency(i, freq).map_err(|e| {
            format!("Failed to set crossover {} to {} Hz: {}", i, freq, e)
        })?;
    }

    // ─────────────────────────────────────────────────────────────────────
    // Step 3: Create + configure pre-fx effects
    // ─────────────────────────────────────────────────────────────────────
    for (effect_idx, effect_spec) in spec.pre_fx.iter().enumerate() {
        let effect = create_effect(effect_spec, clap_manager, pd_manager)
            .map_err(|e| format!("Pre-fx effect {} '{}': {}", effect_idx, effect_spec.plugin_id, e))?;

        multiband.add_pre_fx(effect).map_err(|e| {
            format!("Failed to add pre-fx {}: {}", effect_idx, e)
        })?;

        // Set all parameters
        for &(param_idx, value) in &effect_spec.params {
            let _ = multiband.set_pre_fx_param(effect_idx, param_idx, value);
        }

        // Set bypass
        if effect_spec.bypass {
            let _ = multiband.set_pre_fx_bypass(effect_idx, true);
        }

        // Set per-effect dry/wet
        let _ = multiband.set_pre_fx_effect_dry_wet(effect_idx, effect_spec.dry_wet);
    }

    // ─────────────────────────────────────────────────────────────────────
    // Step 4: Create + configure per-band effects
    // ─────────────────────────────────────────────────────────────────────
    for (band_idx, band_spec) in spec.bands.iter().enumerate() {
        // Set band properties
        let _ = multiband.set_band_gain(band_idx, band_spec.gain);
        let _ = multiband.set_band_mute(band_idx, band_spec.muted);
        let _ = multiband.set_band_solo(band_idx, band_spec.soloed);
        let _ = multiband.set_band_chain_dry_wet(band_idx, band_spec.chain_dry_wet);

        // Create + configure effects for this band
        for (effect_idx, effect_spec) in band_spec.effects.iter().enumerate() {
            let effect = create_effect(effect_spec, clap_manager, pd_manager)
                .map_err(|e| format!("Band {} effect {} '{}': {}", band_idx, effect_idx, effect_spec.plugin_id, e))?;

            multiband.add_effect_to_band(band_idx, effect).map_err(|e| {
                format!("Failed to add effect {} to band {}: {}", effect_idx, band_idx, e)
            })?;

            // Set all parameters
            for &(param_idx, value) in &effect_spec.params {
                let _ = multiband.set_effect_param(band_idx, effect_idx, param_idx, value);
            }

            // Set bypass
            if effect_spec.bypass {
                let _ = multiband.set_effect_bypass(band_idx, effect_idx, true);
            }

            // Set per-effect dry/wet
            let _ = multiband.set_band_effect_dry_wet(band_idx, effect_idx, effect_spec.dry_wet);
        }
    }

    // ─────────────────────────────────────────────────────────────────────
    // Step 5: Create + configure post-fx effects
    // ─────────────────────────────────────────────────────────────────────
    for (effect_idx, effect_spec) in spec.post_fx.iter().enumerate() {
        let effect = create_effect(effect_spec, clap_manager, pd_manager)
            .map_err(|e| format!("Post-fx effect {} '{}': {}", effect_idx, effect_spec.plugin_id, e))?;

        multiband.add_post_fx(effect).map_err(|e| {
            format!("Failed to add post-fx {}: {}", effect_idx, e)
        })?;

        // Set all parameters
        for &(param_idx, value) in &effect_spec.params {
            let _ = multiband.set_post_fx_param(effect_idx, param_idx, value);
        }

        // Set bypass
        if effect_spec.bypass {
            let _ = multiband.set_post_fx_bypass(effect_idx, true);
        }

        // Set per-effect dry/wet
        let _ = multiband.set_post_fx_effect_dry_wet(effect_idx, effect_spec.dry_wet);
    }

    // ─────────────────────────────────────────────────────────────────────
    // Step 6: Set chain-level and global dry/wet
    // ─────────────────────────────────────────────────────────────────────
    multiband.set_pre_fx_chain_dry_wet(spec.pre_fx_chain_dry_wet);
    multiband.set_post_fx_chain_dry_wet(spec.post_fx_chain_dry_wet);
    multiband.set_global_dry_wet(spec.global_dry_wet);

    // ─────────────────────────────────────────────────────────────────────
    // Step 7: Add macro mappings
    // ─────────────────────────────────────────────────────────────────────
    for (macro_index, mapping_spec) in &spec.macro_mappings {
        let mapping = mapping_spec.to_macro_mapping();
        if let Err(e) = multiband.add_macro_mapping(*macro_index, mapping) {
            log::warn!(
                "[PRESET_LOADER] Failed to add macro mapping (macro={}, {:?}): {}",
                macro_index, mapping_spec.location, e
            );
        }
    }

    Ok(multiband)
}

/// Create a single effect from a build spec using the appropriate manager.
fn create_effect(
    spec: &super::EffectBuildSpec,
    clap_manager: &mut ClapManager,
    pd_manager: &mut PdManager,
) -> Result<Box<dyn crate::effect::Effect>, String> {
    match spec.source {
        EffectSourceType::Clap => {
            clap_manager
                .create_effect(&spec.plugin_id)
                .map_err(|e| format!("CLAP create_effect failed: {}", e))
        }
        EffectSourceType::Pd => {
            pd_manager
                .create_effect(&spec.plugin_id)
                .map_err(|e| format!("PD create_effect failed: {}", e))
        }
    }
}
