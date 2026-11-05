// SPDX-License-Identifier: GPL-3.0-or-later

#[test]
#[ignore = "requires an explicitly supplied real project and its source video"]
fn loaded_context_real_project_cache_hit_restores_originals_and_zoom_ui() {
    let path = std::env::var("GYROFLOW_PLUGIN_TRANSLATION_PROJECT").expect("Set the real project fixture path");
    let data = std::fs::read_to_string(&path).unwrap();
    let mut project: serde_json::Value = serde_json::from_str(&data).unwrap();
    project["video_info"]["rotation"] = 90.0.into();
    project["output"]["output_width"] = 2160.into();
    project["output"]["output_height"] = 3840.into();
    project["trim_ranges_ms"] = serde_json::json!([[1000.0, 8000.0]]);

    let cache = Mutex::new(LruCache::new(std::num::NonZeroUsize::new(8).unwrap()));
    let mut host_a = TestParams::default();
    host_a.set_string(Params::ProjectPath, &path).unwrap();
    host_a.set_string(Params::InstanceId, "loaded-context-project").unwrap();
    host_a.set_string(Params::ProjectData, &serde_json::to_string(&project).unwrap()).unwrap();
    host_a.set_bool(Params::IncludeProjectData, true).unwrap();
    let mut instance_a = GyroflowPluginBaseInstance { always_set_input_rotation: true, ..Default::default() };
    drop(instance_a.stab_manager(&mut host_a, &cache, (3840, 2160), false).unwrap());
    host_a.set_f64(Params::Smoothness, 48.0).unwrap();
    instance_a.param_changed(&mut host_a, &cache, Params::Smoothness, true).unwrap();
    let manager_a = instance_a.stab_manager(&mut host_a, &cache, (3840, 2160), false).unwrap();
    assert!(instance_a.has_motion && manager_a.gyro.read().has_motion());
    assert_eq!(instance_a.original_video_size, (3840, 2160));
    assert_eq!(instance_a.original_output_size, (2160, 3840));
    assert_eq!(instance_a.original_project_rotation, Some(90.0));
    assert_eq!(instance_a.container_media_rotation, Some(0));
    assert_eq!(instance_a.original_project_trim_ranges.len(), 1);
    assert!((instance_a.original_project_trim_ranges[0].0 - 1.0).abs() < 1e-9);
    assert!((instance_a.original_project_trim_ranges[0].1 - 8.0).abs() < 1e-9);
    assert!(manager_a.params.read().trim_ranges.is_empty());

    // A cached manager may already carry host geometry and a host trim window.
    // The next instance must recover the imported context, not infer it from these values.
    {
        let mut current = manager_a.params.write();
        current.size = (1280, 720);
        current.output_size = (720, 1280);
        current.video_rotation = 270.0;
        current.trim_ranges = vec![(0.2, 0.3)];
    }

    let mut host_b = host_a.clone();
    host_b.set_string(Params::LoadedProject, "").unwrap();
    host_b.set_string(Params::LoadedLens, "").unwrap();
    host_b.set_string(Params::LoadedPreset, "stale preset").unwrap();
    let mut instance_b = GyroflowPluginBaseInstance {
        reload_values_from_project: false,
        timeline_size: (640, 360),
        framebuffer_inverted: true,
        auto_disable_stretch: false,
        ..Default::default()
    };
    let manager_b = instance_b.stab_manager(&mut host_b, &cache, (320, 180), false).unwrap();
    assert!(Arc::ptr_eq(&manager_a, &manager_b));
    assert_eq!(instance_b.original_video_size, (3840, 2160));
    assert_eq!(instance_b.original_output_size, (2160, 3840));
    assert_eq!(instance_b.original_project_rotation, Some(90.0));
    assert_eq!(instance_b.original_project_trim_ranges, instance_a.original_project_trim_ranges);
    assert_eq!(instance_b.container_media_rotation, Some(0));
    assert_eq!(instance_b.num_frames, 600);
    assert_eq!(instance_b.fps, 60_000.0 / 1001.0);
    assert!(instance_b.has_motion);
    assert_eq!(instance_b.timeline_size, (640, 360));
    assert!(instance_b.framebuffer_inverted);
    assert!(!instance_b.auto_disable_stretch);
    assert!(!instance_b.ever_changed && !instance_b.reload_values_from_project);
    assert_eq!(host_b.get_f64(Params::Smoothness).unwrap(), 48.0);
    assert_eq!(manager_b.keyframes.read().value_at_video_timestamp(&KeyframeType::SmoothingParamSmoothness, 0.0), Some(0.48));
    assert_eq!(host_b.get_string(Params::LoadedProject).unwrap(), "yzs0115.gyroflow");
    assert!(!host_b.get_string(Params::LoadedLens).unwrap().is_empty());
    assert!(host_b.get_string(Params::LoadedPreset).unwrap().is_empty());
    host_b.value_writes.clear();
    host_b.enabled_writes.clear();
    host_b.label_writes.clear();
    instance_b.stab_manager(&mut host_b, &cache, (320, 180), false).unwrap();
    assert!(host_b.value_writes.is_empty(), "an unchanged cache hit must not republish values");
    assert!(host_b.label_writes.is_empty(), "an unchanged cache hit must not republish loaded-state labels");
    assert!(host_b.enabled_writes.iter().all(|p| translation::CONTROLS.contains(p)), "the existing translation UI refresh must not expand to all controls on every hit");
    // A later host packet may restore empty display strings without losing the local manager.
    host_b.set_string(Params::LoadedProject, "").unwrap();
    host_b.set_string(Params::LoadedLens, "").unwrap();
    host_b.value_writes.clear();
    host_b.enabled_writes.clear();
    host_b.label_writes.clear();
    instance_b.stab_manager(&mut host_b, &cache, (320, 180), false).unwrap();
    assert_eq!(host_b.get_string(Params::LoadedProject).unwrap(), "yzs0115.gyroflow");
    assert!(!host_b.get_string(Params::LoadedLens).unwrap().is_empty());
    assert_eq!(host_b.value_writes, vec![Params::LoadedProject, Params::LoadedLens]);
    assert!(host_b.label_writes.is_empty());
    assert!(host_b.enabled_writes.iter().all(|p| translation::CONTROLS.contains(p)));
    instance_b.param_changed(&mut host_b, &cache, Params::ZoomMode, true).unwrap();
    assert!(!host_b.enabled[&Params::UseDynamicZoom]);
    assert!(host_b.enabled[&Params::UseStaticZoom]);

    let video = filesystem::url_to_path(project["videofile"].as_str().unwrap());
    host_b.set_string(Params::ProjectPath, &video).unwrap();
    host_b.set_string(Params::ProjectData, "").unwrap();
    host_b.set_bool(Params::IncludeProjectData, false).unwrap();
    instance_b.param_changed(&mut host_b, &cache, Params::ReloadProject, true).unwrap();
    instance_b.stab_manager(&mut host_b, &cache, (3840, 2160), false).unwrap();
    assert!(instance_b.original_project_trim_ranges.is_empty(), "a bare video must not inherit the previous project's trim context");
}
