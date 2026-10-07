// SPDX-License-Identifier: GPL-3.0-or-later

use super::{GFError, GFStatus, GFFinalCutInstance, LoadedProject, clear_error_slot, set_error};
use gyroflow_plugin_base::StabilizationManager;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GFTranslationParameters {
    pub reference_percent: f64,
    pub smoothness_seconds: f64,
    pub enabled: u8,
    pub automatic: u8,
    pub along_axis: u8,
    pub initialized: u8,
    pub reserved: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GFTranslationInfo {
    pub parameters: GFTranslationParameters,
    pub available: u8,
    pub active: u8,
    pub stale: u8,
    pub reserved: [u8; 5],
}

pub(crate) fn snapshot(manager: &StabilizationManager) -> GFTranslationInfo {
    let gyro = manager.gyro.read();
    let ui = *manager.optical_ui.read();
    let result = gyro.optical_translation.as_ref();
    GFTranslationInfo {
        parameters: GFTranslationParameters {
            reference_percent: ui.translation_settings.reference * 100.0,
            smoothness_seconds: ui.translation_settings.smoothness_s,
            enabled: u8::from(ui.translation_enabled),
            automatic: u8::from(ui.translation_settings.auto),
            along_axis: u8::from(ui.translation_settings.along_axis),
            initialized: u8::from(result.is_some()),
            reserved: [0; 4],
        },
        available: u8::from(result.is_some()),
        active: u8::from(result.is_some_and(|r| r.is_active())),
        stale: u8::from(result.is_some_and(|r| !r.applies || !r.has_valid_geometry())),
        reserved: [0; 5],
    }
}

fn validate(parameters: &GFTranslationParameters) -> Result<(), String> {
    if [parameters.enabled, parameters.automatic, parameters.along_axis, parameters.initialized]
        .iter().any(|v| *v > 1) || parameters.reserved != [0; 4] {
        return Err("Translation flags or reserved bytes are invalid".into());
    }
    if parameters.initialized != 0 && (!parameters.reference_percent.is_finite()
        || !(0.0..=200.0).contains(&parameters.reference_percent)
        || !parameters.smoothness_seconds.is_finite()
        || !(0.1..=10.0).contains(&parameters.smoothness_seconds)) {
        return Err("Translation reference must be in [0, 200] and smoothness in [0.1, 10]".into());
    }
    Ok(())
}

// The legacy render parameter setter finishes smoothing and crop recomputation.
// Batch translation first so both controls affect the same published frame.
pub(crate) fn apply(project: &LoadedProject, parameters: &GFTranslationParameters) -> Result<(), String> {
    validate(parameters)?;
    let manager = &project.manager;
    if manager.gyro.read().optical_translation.is_none() { return Ok(()); }
    let parameters = if parameters.initialized != 0 { *parameters }
        else { project.original_translation_parameters };
    let ui = *manager.optical_ui.read();
    let reference = parameters.reference_percent / 100.0;
    if ui.translation_settings.reference != reference { manager.set_translation_reference(reference); }
    if ui.translation_settings.smoothness_s != parameters.smoothness_seconds { manager.set_translation_smoothness(parameters.smoothness_seconds); }
    if ui.translation_settings.auto != (parameters.automatic != 0) { manager.set_translation_auto(parameters.automatic != 0); }
    if ui.translation_settings.along_axis != (parameters.along_axis != 0) { manager.set_translation_along_axis(parameters.along_axis != 0); }
    if ui.translation_enabled != (parameters.enabled != 0) { manager.set_translation_stabilization_enabled(parameters.enabled != 0); }
    Ok(())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_instance_get_project_translation_info(
    instance: *const GFFinalCutInstance, output: *mut GFTranslationInfo, error: *mut *mut GFError,
) -> GFStatus {
    unsafe { clear_error_slot(error) };
    if instance.is_null() || output.is_null() {
        unsafe { set_error(error, GFStatus::NullPointer, "Translation lookup requires an instance and output") };
        return GFStatus::NullPointer;
    }
    unsafe { *output = GFTranslationInfo::default() };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let state = unsafe { &*instance }.state.read().map_err(|_| (GFStatus::Panic, "Project lock is poisoned"))?;
        let project = state.project.as_ref().ok_or((GFStatus::InvalidProject, "Translation lookup requires a project"))?;
        let mut info = snapshot(&project.manager);
        info.parameters = project.original_translation_parameters;
        Ok::<_, (GFStatus, &str)>(info)
    }));
    match result {
        Ok(Ok(info)) => { unsafe { *output = info }; GFStatus::Ok },
        Ok(Err((status, message))) => { unsafe { set_error(error, status, message) }; status },
        Err(_) => { unsafe { set_error(error, GFStatus::Panic, "Translation lookup panicked") }; GFStatus::Panic },
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_instance_set_translation_parameters(
    instance: *mut GFFinalCutInstance, parameters: *const GFTranslationParameters, error: *mut *mut GFError,
) -> GFStatus {
    unsafe { clear_error_slot(error) };
    if instance.is_null() || parameters.is_null() {
        unsafe { set_error(error, GFStatus::NullPointer, "Translation update requires an instance and parameters") };
        return GFStatus::NullPointer;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let state = unsafe { &*instance }.state.read().map_err(|_| (GFStatus::Panic, "Project lock is poisoned".to_string()))?;
        let project = state.project.as_ref().ok_or((GFStatus::InvalidProject, "Translation update requires a project".to_string()))?;
        apply(project, unsafe { &*parameters }).map_err(|message| (GFStatus::InvalidArgument, message))
    }));
    match result {
        Ok(Ok(())) => GFStatus::Ok,
        Ok(Err((status, message))) => { unsafe { set_error(error, status, &message) }; status },
        Err(_) => { unsafe { set_error(error, GFStatus::Panic, "Translation update panicked") }; GFStatus::Panic },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> LoadedProject {
        let mut project = super::super::parse_project(include_bytes!("../tests/fixtures/phase0-valid.gyroflow")).unwrap();
        project.manager.set_translation_stabilization_enabled(true);
        project.manager.set_translation_reference(0.7);
        project.manager.set_translation_smoothness(2.5);
        let settings = project.manager.optical_ui.read().translation_settings;
        project.manager.gyro.write().optical_translation = Some(
            gyroflow_plugin_base::gyroflow_core::gyro_source::OpticalTranslation::new(Vec::new(), settings));
        project.original_translation_parameters = snapshot(&project.manager).parameters;
        project
    }

    #[test]
    fn translation_abi_keeps_the_original_render_layout() {
        assert_eq!(std::mem::size_of::<super::super::GFRenderParameters>(), 48);
        assert_eq!(std::mem::size_of::<GFTranslationParameters>(), 24);
        assert_eq!(std::mem::size_of::<GFTranslationInfo>(), 32);
        assert_eq!(std::mem::offset_of!(GFTranslationParameters, initialized), 19);
    }

    #[test]
    fn no_result_does_not_enable_or_show_translation() {
        let project = super::super::parse_project(include_bytes!("../tests/fixtures/phase0-valid.gyroflow")).unwrap();
        let before = *project.manager.optical_ui.read();
        assert_eq!(snapshot(&project.manager).available, 0);
        let parameters = GFTranslationParameters {
            reference_percent: 120.0, smoothness_seconds: 0.5, initialized: 1, enabled: 1,
            ..Default::default()
        };
        apply(&project, &parameters).unwrap();
        assert_eq!(*project.manager.optical_ui.read(), before);
    }

    #[test]
    fn overrides_preserve_results_and_restore_the_original_on_legacy_state() {
        let project = fixture();
        let parameters = GFTranslationParameters {
            reference_percent: 140.0, smoothness_seconds: 0.5, enabled: 0, automatic: 1,
            along_axis: 1, initialized: 1, reserved: [0; 4],
        };
        apply(&project, &parameters).unwrap();
        assert_eq!(snapshot(&project.manager).parameters, parameters);
        assert!(project.manager.gyro.read().optical_translation.is_some());
        apply(&project, &GFTranslationParameters::default()).unwrap();
        assert_eq!(snapshot(&project.manager).parameters, project.original_translation_parameters);
    }

    #[test]
    fn invalid_parameters_do_not_mutate_the_manager() {
        let project = fixture();
        let before = *project.manager.optical_ui.read();
        for change in 0..5 {
            let mut parameters = project.original_translation_parameters;
            match change {
                0 => parameters.reference_percent = f64::NAN,
                1 => parameters.smoothness_seconds = 10.1,
                2 => parameters.enabled = 2,
                3 => parameters.reserved[0] = 1,
                _ => parameters.reference_percent = 201.0,
            }
            assert!(apply(&project, &parameters).is_err());
            assert_eq!(*project.manager.optical_ui.read(), before);
        }
    }

    #[test]
    fn translation_ffi_checks_pointers_and_requires_a_project() {
        let mut info = GFTranslationInfo::default();
        let parameters = GFTranslationParameters::default();
        unsafe {
            assert_eq!(gf_finalcut_instance_get_project_translation_info(std::ptr::null(), &mut info, std::ptr::null_mut()), GFStatus::NullPointer);
            let instance = super::super::gf_finalcut_instance_create(std::ptr::null_mut());
            assert_eq!(gf_finalcut_instance_get_project_translation_info(instance, &mut info, std::ptr::null_mut()), GFStatus::InvalidProject);
            assert_eq!(gf_finalcut_instance_set_translation_parameters(instance, &parameters, std::ptr::null_mut()), GFStatus::InvalidProject);
            super::super::gf_finalcut_instance_free(instance);
        }
    }

    #[test]
    #[ignore = "requires an explicitly supplied project with translation analysis"]
    fn translation_finalcut_real_project_round_trip() {
        use gyroflow_plugin_base::gyroflow_core::stabilization::{ComputeParams, FrameTransform};
        let bytes = std::fs::read(std::env::var("GYROFLOW_FINALCUT_TRANSLATION_PROJECT").unwrap()).unwrap();
        let project = super::super::parse_project(&bytes).unwrap();
        assert_eq!(snapshot(&project.manager).active, 1);
        let render = super::super::snapshot_project_parameters(&project.manager).unwrap();
        super::super::apply_render_parameters(&project, &render).unwrap();
        let frame = (project.manager.params.read().frame_count / 2).min(300);
        let timestamp = gyroflow_plugin_base::gyroflow_core::timestamp_at_frame(frame as i32, project.manager.params.read().fps);
        let matrices = || FrameTransform::at_timestamp(&ComputeParams::from_manager(&project.manager), timestamp, frame).matrices;
        let original = matrices();
        let mut settings = project.original_translation_parameters;
        settings.enabled = 0;
        apply(&project, &settings).unwrap();
        super::super::apply_render_parameters(&project, &render).unwrap();
        assert_eq!(snapshot(&project.manager).active, 0);
        let disabled = matrices();
        assert!(original != disabled);
        settings.enabled = 1;
        settings.automatic = 0;
        settings.reference_percent = 140.0;
        settings.smoothness_seconds = 0.5;
        apply(&project, &settings).unwrap();
        super::super::apply_render_parameters(&project, &render).unwrap();
        assert_eq!(snapshot(&project.manager).active, 1);
        assert!(matrices() != disabled && matrices() != original);
        apply(&project, &GFTranslationParameters::default()).unwrap();
        super::super::apply_render_parameters(&project, &render).unwrap();
        assert!(matrices() == original, "restoring project settings must restore the original matrices");
    }
}
