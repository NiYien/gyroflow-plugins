use gyroflow_plugin_base::StabilizationManager;

use crate::GFRenderParameters;

pub(crate) fn validate_render_parameters(parameters: &GFRenderParameters) -> Result<(), String> {
    let finite_values = [
        parameters.fov,
        parameters.smoothness,
        parameters.lens_correction,
        parameters.horizon_lock_amount,
        parameters.horizon_lock_roll,
    ];
    if finite_values.iter().any(|value| !value.is_finite()) {
        return Err("render parameters must be finite".to_string());
    }
    if !(0.1..=3.0).contains(&parameters.fov) {
        return Err("FOV must be in [0.1, 3.0]".to_string());
    }
    if !(1.0..=300.0).contains(&parameters.smoothness) {
        return Err("Smoothness must be in [1, 300]".to_string());
    }
    if !(0.0..=100.0).contains(&parameters.lens_correction) {
        return Err("Lens Correction must be in [0, 100]".to_string());
    }
    if !(0.0..=100.0).contains(&parameters.horizon_lock_amount) {
        return Err("Horizon Lock amount must be in [0, 100]".to_string());
    }
    if !(-100.0..=100.0).contains(&parameters.horizon_lock_roll) {
        return Err("Horizon Lock roll must be in [-100, 100]".to_string());
    }
    if !(0..=2).contains(&parameters.zoom_mode) {
        return Err("Zoom Mode must be 0, 1, or 2".to_string());
    }
    if parameters.overview > 1 || parameters.reserved != [0; 3] {
        return Err("Overview or reserved parameter bytes are invalid".to_string());
    }
    Ok(())
}

pub(crate) fn snapshot_project_parameters(
    manager: &StabilizationManager,
) -> Result<GFRenderParameters, String> {
    let params = manager.params.read();
    let smoothing = manager.smoothing.read();
    let horizon = &smoothing.horizon_lock;
    let zoom_mode = if params.adaptive_zoom_window == 0.0 {
        0
    } else if params.adaptive_zoom_window < 0.0 {
        2
    } else {
        1
    };
    let snapshot = GFRenderParameters {
        fov: params.fov,
        smoothness: smoothing.current().get_parameter("smoothness") * 100.0,
        lens_correction: (params.lens_correction_amount * 100.0).min(100.0),
        horizon_lock_amount: if horizon.lock_enabled {
            horizon.horizonlockpercent
        } else {
            0.0
        },
        horizon_lock_roll: if horizon.lock_enabled {
            horizon.horizonroll
        } else {
            0.0
        },
        zoom_mode,
        overview: u8::from(params.fov_overview),
        reserved: [0; 3],
    };
    validate_render_parameters(&snapshot)?;
    Ok(snapshot)
}
