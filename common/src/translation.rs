// SPDX-License-Identifier: GPL-3.0-or-later

use crate::{GyroflowPluginParams, ParameterType, Params, PluginResult, StabilizationManager, t};

pub const CONTROLS: [Params; 6] = [
    Params::TranslationEnabled, Params::TranslationAuto, Params::TranslationReference,
    Params::TranslationSmoothness, Params::TranslationAlongAxis, Params::TranslationStatus,
];

pub fn is_setting(param: Params) -> bool {
    CONTROLS[..5].contains(&param)
}

pub fn definitions() -> Vec<ParameterType> {
    vec![
        ParameterType::Checkbox { id: "TranslationInitialized", label: "", hint: "", default: false, hidden: true },
        ParameterType::Checkbox { id: "TranslationEnabled", label: t!("label.translation_enabled"), hint: t!("hint.translation_enabled"), default: false, hidden: true },
        ParameterType::Checkbox { id: "TranslationAuto", label: t!("label.translation_auto"), hint: t!("hint.translation_auto"), default: false, hidden: true },
        ParameterType::Slider { id: "TranslationReference", label: t!("label.translation_reference"), hint: t!("hint.translation_reference"), min: 0.0, max: 200.0, default: 100.0, hidden: true },
        ParameterType::Slider { id: "TranslationSmoothness", label: t!("label.translation_smoothness"), hint: t!("hint.translation_smoothness"), min: 0.1, max: 10.0, default: 1.0, hidden: true },
        ParameterType::Checkbox { id: "TranslationAlongAxis", label: t!("label.translation_along_axis"), hint: t!("hint.translation_along_axis"), default: true, hidden: true },
        ParameterType::Text { id: "TranslationStatus", label: t!("label.translation_status"), hint: "", hidden: true },
    ]
}

pub fn load(params: &mut dyn GyroflowPluginParams, stab: &StabilizationManager, reload: bool) -> PluginResult<()> {
    if stab.gyro.read().optical_translation.is_none() { return Ok(()); }
    if reload || !params.get_bool(Params::TranslationInitialized).unwrap_or(false) {
        let ui = *stab.optical_ui.read();
        params.set_bool(Params::TranslationEnabled, ui.translation_enabled)?;
        params.set_bool(Params::TranslationAuto, ui.translation_settings.auto)?;
        params.set_f64(Params::TranslationReference, ui.translation_settings.reference * 100.0)?;
        params.set_f64(Params::TranslationSmoothness, ui.translation_settings.smoothness_s)?;
        params.set_bool(Params::TranslationAlongAxis, ui.translation_settings.along_axis)?;
        params.set_bool(Params::TranslationInitialized, true)?;
    }
    Ok(())
}

// Return whether the renderer needs a new curve and crop. Unchanged host
// values must not invalidate the caches on every frame or cache rebuild.
pub fn apply(params: &dyn GyroflowPluginParams, stab: &StabilizationManager) -> PluginResult<bool> {
    if stab.gyro.read().optical_translation.is_none()
        || !params.get_bool(Params::TranslationInitialized).unwrap_or(false) { return Ok(false); }
    let enabled = params.get_bool(Params::TranslationEnabled)?;
    let auto = params.get_bool(Params::TranslationAuto)?;
    let along_axis = params.get_bool(Params::TranslationAlongAxis)?;
    let reference = params.get_f64(Params::TranslationReference)? / 100.0;
    let seconds = params.get_f64(Params::TranslationSmoothness)?;
    if !reference.is_finite() || !seconds.is_finite() {
        return Err("Translation parameters must be finite".into());
    }
    let reference = reference.clamp(0.0, 2.0);
    let seconds = seconds.clamp(0.1, 10.0);
    let before = *stab.optical_ui.read();
    let mut changed = false;
    if before.translation_settings.reference != reference { stab.set_translation_reference(reference); changed = true; }
    if before.translation_settings.smoothness_s != seconds { stab.set_translation_smoothness(seconds); changed = true; }
    if before.translation_settings.auto != auto { stab.set_translation_auto(auto); changed = true; }
    if before.translation_settings.along_axis != along_axis { stab.set_translation_along_axis(along_axis); changed = true; }
    if before.translation_enabled != enabled { stab.set_translation_stabilization_enabled(enabled); changed = true; }
    Ok(changed)
}

pub fn update_ui(params: &mut dyn GyroflowPluginParams, stab: Option<&StabilizationManager>) {
    let state = stab.and_then(|s| {
        let gyro = s.gyro.read();
        let result = gyro.optical_translation.as_ref()?;
        let ui = *s.optical_ui.read();
        Some((ui.translation_enabled, ui.translation_settings.auto, result.applies))
    });
    for param in CONTROLS {
        let _ = params.set_visible(param, state.is_some());
    }
    if let Some((enabled, auto, applies)) = state {
        let _ = params.set_enabled(Params::TranslationEnabled, true);
        let _ = params.set_enabled(Params::TranslationAuto, enabled);
        let _ = params.set_enabled(Params::TranslationAlongAxis, enabled);
        let _ = params.set_enabled(Params::TranslationReference, enabled && !auto);
        let _ = params.set_enabled(Params::TranslationSmoothness, enabled && !auto);
        let _ = params.set_enabled(Params::TranslationStatus, false);
        let status = if !enabled { t!("status.translation_disabled") }
            else if !applies { t!("status.translation_stale") }
            else { t!("status.translation_active") };
        if params.get_string(Params::TranslationStatus).ok().as_deref() != Some(status) {
            let _ = params.set_string(Params::TranslationStatus, status);
        }
    }
}
