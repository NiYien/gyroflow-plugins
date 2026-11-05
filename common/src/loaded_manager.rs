// SPDX-License-Identifier: GPL-3.0-or-later

use crate::{filesystem, GyroflowPluginBaseInstance, StabilizationManager};
use crate::parking_lot::Mutex;
use crate::lru::LruCache;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct LoadedContext {
    pub original_video_size: (usize, usize),
    pub original_output_size: (usize, usize),
    pub original_project_rotation: Option<f64>,
    pub original_project_trim_ranges: Vec<(f64, f64)>,
    pub container_media_rotation: Option<i32>,
    pub num_frames: usize,
    pub fps: f64,
    pub has_motion: bool,
    pub loaded: bool,
    pub loaded_project: String,
    pub loaded_lens: String,
    pub loaded_preset: Option<String>,
}

impl LoadedContext {
    pub(crate) fn from_import(instance: &GyroflowPluginBaseInstance, manager: &StabilizationManager, path: &str, loaded: bool) -> Self {
        Self {
            original_video_size: instance.original_video_size,
            original_output_size: instance.original_output_size,
            original_project_rotation: instance.original_project_rotation,
            original_project_trim_ranges: instance.original_project_trim_ranges.clone(),
            container_media_rotation: instance.container_media_rotation,
            num_frames: instance.num_frames,
            fps: instance.fps,
            has_motion: instance.has_motion,
            loaded,
            loaded_project: filesystem::get_filename(&filesystem::path_to_url(path)),
            loaded_lens: manager.lens.read().get_display_name(),
            loaded_preset: manager.input_file.read().preset_name.clone(),
        }
    }
}

#[derive(Clone)]
pub struct LoadedManager {
    pub manager: Arc<StabilizationManager>,
    pub context: Arc<LoadedContext>,
}

pub type ManagerCache = Mutex<LruCache<String, LoadedManager>>;

#[cfg(test)]
impl LoadedManager {
    pub(crate) fn from_unadapted_test_manager(manager: Arc<StabilizationManager>) -> Self {
        let original_project_trim_ranges = crate::clear_imported_project_trim(&manager);
        let params = manager.params.read();
        let context = LoadedContext {
            original_video_size: params.size,
            original_output_size: params.output_size,
            original_project_rotation: Some(params.video_rotation),
            original_project_trim_ranges,
            container_media_rotation: None,
            num_frames: params.frame_count,
            fps: params.fps,
            has_motion: manager.gyro.read().has_motion(),
            loaded: params.duration_ms > 0.0,
            loaded_project: String::new(),
            loaded_lens: manager.lens.read().get_display_name(),
            loaded_preset: manager.input_file.read().preset_name.clone(),
        };
        drop(params);
        Self { manager, context: Arc::new(context) }
    }
}
