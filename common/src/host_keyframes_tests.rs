// SPDX-License-Identifier: GPL-3.0-or-later

#[test]
fn host_keyframe_comparison_preserves_constant_fast_path() {
    let stab = manager_for_snapshot_stretch((1.0, 1.0));
    let mut host = KeyframableParams::default();
    assert!(changed_host_keyframes(&stab, &host).is_empty());
    host.cached_keyframes.set(&KeyframeType::LockHorizonAmount, 0, 0.0);
    host.cached_keyframes.set(&KeyframeType::LockHorizonRoll, 0, 0.0);
    host.cached_keyframes.set(&KeyframeType::Fov, 0, stab.params.read().fov);
    host.cached_keyframes.set(&KeyframeType::VideoSpeed, 0, stab.params.read().video_speed);
    assert!(changed_host_keyframes(&stab, &host).is_empty());
    host.cached_keyframes.set(&KeyframeType::LockHorizonAmount, 1_000_000, 100.0);
    assert_eq!(changed_host_keyframes(&stab, &host), vec![KeyframeType::LockHorizonAmount]);
    host.cached_keyframes.set(&KeyframeType::LockHorizonAmount, 1_000_000, 0.0);
    assert!(changed_host_keyframes(&stab, &host).is_empty());
    host.cached_keyframes.set(&KeyframeType::SmoothingParamSmoothness, 0, 42.0);
    assert_eq!(changed_host_keyframes(&stab, &host), vec![KeyframeType::SmoothingParamSmoothness]);
}

#[test]
fn host_keyframe_comparison_respects_project_precedence_and_easing() {
    use gyroflow_core::keyframes::Easing;
    let stab = manager_for_snapshot_stretch((1.0, 1.0));
    let typ = KeyframeType::LockHorizonAmount;
    let mut host = KeyframableParams::default();
    for (ts, value) in [(0, 100.0), (2_000_000, 30.0)] {
        stab.keyframes.write().set(&typ, ts, value);
        host.cached_keyframes.set(&typ, ts, value);
    }
    assert!(changed_host_keyframes(&stab, &host).is_empty());
    host.cached_keyframes.set_easing(&typ, 0, Easing::NoEasing);
    assert_eq!(changed_host_keyframes(&stab, &host), vec![typ]);
    host.use_gyroflows_keyframes = true;
    assert!(changed_host_keyframes(&stab, &host).is_empty());
    host.use_gyroflows_keyframes = false;
    host.cached_keyframes.clear_type(&typ);
    host.cached_keyframes.set(&typ, 0, 100.0);
    assert_eq!(changed_host_keyframes(&stab, &host), vec![typ]);
    stab.keyframes.write().set(&typ, 2_000_000, 100.0);
    assert!(changed_host_keyframes(&stab, &host).is_empty());

    stab.keyframes.write().set(&typ, 2_000_000, 30.0);
    host.cached_keyframes.set(&typ, 2_000_000, 30.0);
    host.cached_keyframes.timestamp_scale = Some(2.0);
    assert_eq!(changed_host_keyframes(&stab, &host), vec![typ]);
}

fn host_reload_project(amount: f64, roll: f64) -> String {
    use gyroflow_core::gyro_source::{FileMetadata, Quat64};
    let stab = manager_for_snapshot_stretch((1.0, 1.0));
    {
        let mut p = stab.params.write();
        p.frame_count = 120;
        p.duration_ms = 4000.0;
        p.adaptive_zoom_window = 4.0;
        p.max_zoom = Some(180.0);
        p.max_zoom_iterations = 5;
    }
    stab.input_file.write().url = "file:///synthetic-host-reload.mp4".into();
    let quaternions = (0..400).map(|i| {
        let t = i as f64 / 100.0;
        (i * 10_000, Quat64::from_euler_angles(0.10 * (t * 0.7).sin(), 0.08 * (t * 1.1).sin(), 0.25 + 0.05 * t.sin()))
    }).collect();
    {
        let mut gyro = stab.gyro.write();
        gyro.duration_ms = 4000.0;
        gyro.load_from_telemetry(FileMetadata {
            quaternions,
            detected_source: Some("Synthetic host reload".into()),
            ..Default::default()
        });
    }
    {
        let mut smoothing = stab.smoothing.write();
        smoothing.horizon_lock.lock_enabled = amount > 0.0;
        smoothing.horizon_lock.horizonlockpercent = amount;
        smoothing.horizon_lock.horizonroll = roll;
    }
    stab.recompute_blocking();
    stab.export_gyroflow_data(gyroflow_core::GyroflowProjectType::WithGyroData, "{}", None).unwrap()
}

fn host_reload_pending_render_work(stab: &StabilizationManager) {
    use std::sync::atomic::Ordering::SeqCst;
    // Match process_pixels before GPU dispatch; do not force a recompute that the host would skip.
    if stab.smoothing_invalidated.load(SeqCst) {
        stab.recompute_smoothness();
        stab.smoothing_invalidated.store(false, SeqCst);
    }
    if stab.zooming_invalidated.load(SeqCst) {
        stab.recompute_adaptive_zoom();
        stab.zooming_invalidated.store(false, SeqCst);
    }
    if stab.undistortion_invalidated.load(SeqCst) {
        stab.recompute_undistortion();
        stab.undistortion_invalidated.store(false, SeqCst);
    }
}

fn host_reload_angle_difference(a: &gyroflow_core::gyro_source::TimeQuat, b: &gyroflow_core::gyro_source::TimeQuat) -> f64 {
    assert_eq!(a.len(), b.len());
    assert!(!a.is_empty());
    a.iter().map(|(t, q)| q.angle_to(&b[t]).to_degrees()).fold(0.0, f64::max)
}

fn host_reload_matrices(stab: &StabilizationManager) -> Vec<[f32; 14]> {
    gyroflow_core::stabilization::FrameTransform::at_timestamp(
        &gyroflow_core::stabilization::ComputeParams::from_manager(stab), 2000.0, 60,
    ).matrices
}

#[test]
fn host_horizon_settings_survive_eviction_purge_and_reopen() {
    for (name, project_amount, project_roll, host_amount, host_roll) in [
        ("enable", 0.0, 0.0, 100.0, 0.0),
        ("disable", 100.0, 0.0, 0.0, 0.0),
        ("roll", 100.0, 0.0, 100.0, 12.0),
        ("unchanged", 100.0, 3.0, 100.0, 3.0),
    ] {
        for rebuild in ["lru_eviction", "purge", "reopen"] {
            let mut host = TestParams::default();
            host.set_string(Params::ProjectPath, &format!("C:/synthetic-{name}-{rebuild}.gyroflow")).unwrap();
            host.set_string(Params::InstanceId, "same-saved-instance").unwrap();
            host.set_bool(Params::IncludeProjectData, true).unwrap();
            host.set_string(Params::ProjectData, &host_reload_project(project_amount, project_roll)).unwrap();
            let cache = Mutex::new(LruCache::new(std::num::NonZeroUsize::new(8).unwrap()));
            let mut instance = GyroflowPluginBaseInstance { ever_changed: true, ..Default::default() };
            let first = instance.stab_manager(&mut host, &cache, (1920, 1080), false).unwrap();
            host_reload_pending_render_work(&first);
            assert!(changed_host_keyframes(&first, &instance.keyframable_params.read()).is_empty());
            let initial = first.gyro.read().smoothed_quaternions.clone();
            host.set_f64(Params::HorizonLockAmount, host_amount).unwrap();
            host.set_f64(Params::HorizonLockRoll, host_roll).unwrap();
            instance.param_changed(&mut host, &cache, Params::HorizonLockAmount, true).unwrap();
            host_reload_pending_render_work(&first);
            let edited = first.gyro.read().smoothed_quaternions.clone();
            let edited_matrices = host_reload_matrices(&first);
            let cached = instance.stab_manager(&mut host, &cache, (1920, 1080), false).unwrap();
            assert!(Arc::ptr_eq(&first, &cached));
            drop(cached);
            drop(first);
            match rebuild {
                "lru_eviction" => {
                    for n in 0..8 {
                        cache.lock().put(format!("another-clip-{n}"), Arc::new(StabilizationManager::default()));
                    }
                    assert_eq!(instance.managers.len(), 1);
                },
                "purge" => instance.clear_stab(&cache),
                "reopen" => {
                    instance.clear_stab(&cache);
                    instance = GyroflowPluginBaseInstance { reload_values_from_project: false, ..Default::default() };
                },
                _ => unreachable!(),
            }
            let restored = instance.stab_manager(&mut host, &cache, (1920, 1080), false).unwrap();
            host_reload_pending_render_work(&restored);
            assert_eq!(host.get_f64(Params::HorizonLockAmount).unwrap(), host_amount);
            assert_eq!(host.get_f64(Params::HorizonLockRoll).unwrap(), host_roll);
            assert_eq!(restored.keyframes.read().value_at_video_timestamp(&KeyframeType::LockHorizonAmount, 2000.0), Some(host_amount));
            assert_eq!(restored.keyframes.read().value_at_video_timestamp(&KeyframeType::LockHorizonRoll, 2000.0), Some(host_roll));
            let error = host_reload_angle_difference(&edited, &restored.gyro.read().smoothed_quaternions);
            assert!(error < 1e-8, "{name}/{rebuild}: stabilization changed by {error} degrees");
            assert_eq!(edited_matrices, host_reload_matrices(&restored), "{name}/{rebuild}: render matrices changed");
            drop(restored);
            instance.param_changed(&mut host, &cache, Params::ReloadProject, true).unwrap();
            let reloaded = instance.stab_manager(&mut host, &cache, (1920, 1080), false).unwrap();
            host_reload_pending_render_work(&reloaded);
            assert_eq!(host.get_f64(Params::HorizonLockAmount).unwrap(), project_amount);
            assert_eq!(host.get_f64(Params::HorizonLockRoll).unwrap(), project_roll);
            assert!(host_reload_angle_difference(&initial, &reloaded.gyro.read().smoothed_quaternions) < 1e-8);
        }
    }
}
