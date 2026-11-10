// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

fn load_through_plugin(path: &std::path::Path) -> Arc<StabilizationManager> {
    let mut instance = GyroflowPluginBaseInstance {
        reload_values_from_project: true,
        ..Default::default()
    };
    let mut params = TestParams::default();
    params.set_string(Params::ProjectPath, &path.to_string_lossy()).unwrap();
    params.set_string(Params::InstanceId, "project-compatibility").unwrap();
    let cache = Mutex::new(LruCache::new(std::num::NonZeroUsize::new(1).unwrap()));
    instance.stab_manager(&mut params, &cache, (0, 0), false).unwrap()
}

#[test]
fn embedded_gyro_survives_scalar_and_dual_axis_lens_metadata() {
    let dir = seq_tmp();
    // An accessible external file must not replace the embedded, rebased samples.
    std::fs::write(dir.join("recording.bin"), b"external gyro must not be read").unwrap();
    for (name, data, expected_focal) in [
        ("scalar", include_str!("../../tests/fixtures/embedded-gyro-scalar.gyroflow"), (500.0, 500.0)),
        ("dual-axis", include_str!("../../tests/fixtures/embedded-gyro-dual-axis.gyroflow"), (500.0, 550.0)),
    ] {
        let path = dir.join(format!("{name}.gyroflow"));
        std::fs::write(&path, data).unwrap();
        let stab = load_through_plugin(&path);
        let gyro = stab.gyro.read();
        let metadata = gyro.file_metadata.read();
        assert_eq!(metadata.raw_imu.len(), 65, "{name}");
        assert_eq!(metadata.raw_imu.first().unwrap().timestamp_ms, 0.0);
        assert_eq!(metadata.raw_imu.last().unwrap().timestamp_ms, 640.0);
        assert_eq!(metadata.lens_params[&0].pixel_focal_length, Some(expected_focal));
        assert_eq!(gyro.quaternions.len(), 65);
        assert_eq!(gyro.get_offsets(), &BTreeMap::from([(200_000, -100.0)]));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "requires GYROFLOW_COMPAT_FIXTURE pointing to an original feedback project"]
fn feedback_project_preserves_embedded_samples_and_sync_offsets() {
    let path = std::path::PathBuf::from(std::env::var("GYROFLOW_COMPAT_FIXTURE").unwrap());
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let expected: gyroflow_core::gyro_source::FileMetadata = gyroflow_core::util::decompress_from_base91_cbor(
        json["gyro_source"]["file_metadata"].as_str().unwrap(),
    ).unwrap();
    let offsets: BTreeMap<i64, f64> = serde_json::from_value(json["offsets"].clone()).unwrap();
    assert!(!expected.raw_imu.is_empty());
    let stab = load_through_plugin(&path);
    let gyro = stab.gyro.read();
    let metadata = gyro.file_metadata.read();
    assert_eq!(metadata.raw_imu.len(), expected.raw_imu.len());
    for (actual, expected) in metadata.raw_imu.iter().zip(&expected.raw_imu) {
        assert_eq!(actual.timestamp_ms, expected.timestamp_ms);
        assert_eq!(actual.gyro, expected.gyro);
        assert_eq!(actual.accl, expected.accl);
    }
    assert_eq!(metadata.lens_params.len(), expected.lens_params.len());
    assert_eq!(gyro.get_offsets(), &offsets);
    assert_eq!(gyro.quaternions.len(), expected.raw_imu.len());
    eprintln!("plugin feedback import: samples={} lens_params={} offsets={:?}", metadata.raw_imu.len(), metadata.lens_params.len(), gyro.get_offsets());
}

#[test]
#[ignore = "requires GYROFLOW_PLUGIN_OPTIMAL_PROJECT with an applied pure-visual Optimal path project"]
fn optimal_project_load_overrides_reopen_and_legacy_method() {
    let fixture = std::env::var("GYROFLOW_PLUGIN_OPTIMAL_PROJECT").unwrap();
    let project: serde_json::Value = serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
    assert_eq!(project["stabilization"]["method"], "Optimal path");
    assert_eq!(project["stabilization"]["fixed_zoom"], true);
    let dir = seq_tmp();
    for expected in ["Optimal path", "Default"] {
        let mut data = project.clone();
        data["stabilization"]["method"] = expected.into();
        data["stabilization"]["smoothing_params"] = serde_json::json!([{"name": "smoothness", "value": 0.5}]);
        let path = dir.join(format!("{}.gyroflow", expected.replace(' ', "-")));
        std::fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
        let mut host = TestParams::default();
        host.set_string(Params::ProjectPath, &path.to_string_lossy()).unwrap();
        host.set_string(Params::InstanceId, expected).unwrap();
        let cache = Mutex::new(LruCache::new(std::num::NonZeroUsize::new(8).unwrap()));
        let mut instance = GyroflowPluginBaseInstance { ever_changed: true, ..Default::default() };
        let first = instance.stab_manager(&mut host, &cache, (0, 0), false).unwrap();
        assert_eq!(first.smoothing.read().current().get_name(), expected);
        assert!(has_active_video_motion(&first));
        assert!(first.gyro.read().get_offsets().is_empty());
        assert_eq!(host.get_i32(Params::ZoomMode).unwrap(), 2);
        let first_fovs = first.params.read().fovs.clone();
        assert!(!first_fovs.is_empty());
        assert!(first_fovs.iter().all(|f| f.to_bits() == first_fovs[0].to_bits()));
        assert!(first_fovs[0] <= 1.0 / 1.25 + 1e-12);
        let original = first.gyro.read().smoothed_quaternions.clone();

        host.set_f64(Params::Smoothness, 90.0).unwrap();
        instance.param_changed(&mut host, &cache, Params::Smoothness, true).unwrap();
        host.set_f64(Params::ZoomLimit, 150.0).unwrap();
        instance.param_changed(&mut host, &cache, Params::ZoomLimit, true).unwrap();
        host_reload_pending_render_work(&first);
        assert_eq!(first.smoothing.read().current().get_name(), expected);
        assert_eq!(first.keyframes.read().value_at_video_timestamp(&KeyframeType::SmoothingParamSmoothness, 0.0), Some(0.9));
        assert_eq!(first.keyframes.read().value_at_video_timestamp(&KeyframeType::MaxZoom, 0.0), Some(150.0));
        let edited = first.gyro.read().smoothed_quaternions.clone();
        assert!(host_reload_angle_difference(&original, &edited) > 1e-8);
        let edited_fovs = first.params.read().fovs.clone();
        assert!(edited_fovs.iter().all(|f| f.to_bits() == edited_fovs[0].to_bits()));
        assert!(edited_fovs[0] <= 1.0 / 1.5 + 1e-12);
        let exported: serde_json::Value = serde_json::from_str(&first.export_gyroflow_data(
            gyroflow_core::GyroflowProjectType::WithGyroData, "{}", None).unwrap()).unwrap();
        assert_eq!(exported["stabilization"]["fixed_zoom"], true);

        let mut reopened = GyroflowPluginBaseInstance { reload_values_from_project: false, ever_changed: true, ..Default::default() };
        let reopened_cache = Mutex::new(LruCache::new(std::num::NonZeroUsize::new(8).unwrap()));
        let restored = reopened.stab_manager(&mut host, &reopened_cache, (0, 0), false).unwrap();
        host_reload_pending_render_work(&restored);
        assert_eq!(restored.smoothing.read().current().get_name(), expected);
        assert!(has_active_video_motion(&restored));
        assert!(host_reload_angle_difference(&edited, &restored.gyro.read().smoothed_quaternions) < 1e-10);
        assert_eq!(restored.params.read().fovs, edited_fovs);

        reopened.param_changed(&mut host, &reopened_cache, Params::ReloadProject, true).unwrap();
        let reloaded = reopened.stab_manager(&mut host, &reopened_cache, (0, 0), false).unwrap();
        host_reload_pending_render_work(&reloaded);
        assert_eq!(reloaded.smoothing.read().current().get_name(), expected);
        assert!(host_reload_angle_difference(&original, &reloaded.gyro.read().smoothed_quaternions) < 1e-10);
        assert_eq!(reloaded.params.read().fovs, first_fovs);
        eprintln!("method={expected:?} applied_visual=true offsets=0 fixed_zoom=true initial_fov={} edited_fov={} host_overrides_and_reopen=ok", first_fovs[0], edited_fovs[0]);

        reloaded.set_optical_correction_enabled(false);
        reloaded.recompute_blocking();
        assert!(!has_active_video_motion(&reloaded));
    }
    assert!(!has_active_video_motion(&StabilizationManager::default()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "requires GYROFLOW_PLUGIN_OPTIMAL_PROJECT with an applied pure-visual project"]
fn pure_visual_zoom_actions_remain_enabled() {
    let path = std::env::var("GYROFLOW_PLUGIN_OPTIMAL_PROJECT").unwrap();
    let mut host = TestParams::default();
    host.set_string(Params::ProjectPath, &path).unwrap();
    host.set_string(Params::InstanceId, "pure-visual-zoom-actions").unwrap();
    let cache = Mutex::new(LruCache::new(std::num::NonZeroUsize::new(8).unwrap()));
    let mut instance = GyroflowPluginBaseInstance { ever_changed: true, ..Default::default() };
    let stab = instance.stab_manager(&mut host, &cache, (0, 0), false).unwrap();
    assert!(has_active_video_motion(&stab));
    instance.param_changed(&mut host, &cache, Params::UseDynamicZoom, true).unwrap();
    assert!(host.enabled[&Params::UseStaticZoom]);
    assert!(!host.enabled[&Params::UseDynamicZoom]);
    instance.param_changed(&mut host, &cache, Params::UseStaticZoom, true).unwrap();
    assert!(host.enabled[&Params::UseDynamicZoom]);
    assert!(!host.enabled[&Params::UseStaticZoom]);
}
