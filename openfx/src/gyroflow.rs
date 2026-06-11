use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use ofx::*;
use super::fuscript::*;
use gyroflow_plugin_base::*;
use gyroflow_plugin_base::parking_lot::{ Mutex, RwLock };
use gyroflow_plugin_base::lru::LruCache;

plugin_module!(
    "xyz.niyien.gyroflow",
    ApiVersion(1),
    PluginVersion(1, 2),
    GyroflowPlugin::default
);

// Plugin-wide cache of the host's mismatched-resolution / timeline-dimensions fields, populated
// by the first successful fuscript query in a Resolve session. Subsequent CreateInstance calls
// (e.g. duplicating the OFX node onto another clip) reuse this cache to avoid the multi-second
// fuscript cold-start latency that otherwise produces a visible "first render passthrough, then
// FillCrop applied" two-stage UX. The cache only stores project / timeline-level fields — never
// per-clip data like file_path or frame_count.
#[derive(Clone, Debug)]
struct HostInputSizingCacheEntry {
    mismatch_mode: Option<String>,
    timeline_w: usize,
    timeline_h: usize,
    use_custom_settings: bool,
}

#[derive(Default)]
struct GyroflowPlugin {
    gyroflow_plugin: GyroflowPluginBase,
    // Lock contention here is minimal: write-once on first fuscript completion + on every
    // ReloadProject refresh, read-once per CreateInstance.
    host_input_sizing_cache: parking_lot::Mutex<Option<HostInputSizingCacheEntry>>,
}

pub fn frame_from_timetype(time: TimeType) -> f64 {
    match time {
        TimeType::Frame(x) => x,
        TimeType::FrameOrMicrosecond((Some(x), _)) => x,
        _ => panic!("Shouldn't happen"),
    }
}

// OpenFX-only enum describing how the host has resized the source image into the timeline
// buffer. `Auto` means "use whatever fuscript detected from Resolve's
// `timelineInputResMismatchBehavior` setting; fall back to `Fit` when fuscript is unavailable
// (Resolve Free / compound clip / non-Resolve host)". The explicit variants let the user
// override the auto detection when fuscript fails or returns the wrong value.
//
// This enum lives in `gyroflow.rs` (not common) because the underlying setting is unique to
// Resolve's OFX path; Adobe / Premiere / frei0r have no fuscript equivalent. Per the OFX
// choice-param contract, the wire format is the dropdown index (0..=4).
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum HostInputSizing {
    #[default]
    Auto = 0,
    Fit = 1,
    FillCrop = 2,
    CenterCrop = 3,
    Stretch = 4,
}

impl HostInputSizing {
    pub fn from_index(idx: i32) -> Self {
        match idx {
            1 => Self::Fit,
            2 => Self::FillCrop,
            3 => Self::CenterCrop,
            4 => Self::Stretch,
            _ => Self::Auto,
        }
    }
    #[allow(dead_code)] // Reserved for future round-trip wire serialization (OFX choice setter).
    pub fn to_index(self) -> i32 {
        self as i32
    }
}

// Map Resolve's `timelineInputResMismatchBehavior` enum strings to the plugin's enum. Returns
// `None` for unknown / empty strings so the resolver can fall back to `Fit`. The four mappings
// match Resolve's documented values (verified via fuscript spike 2026-05-17). Defined here
// alongside the enum so the parser unit tests live in this file.
pub fn parse_mismatch_mode(s: &str) -> Option<HostInputSizing> {
    match s.trim() {
        "scaleToFit"   => Some(HostInputSizing::Fit),
        "scaleToCrop"  => Some(HostInputSizing::FillCrop),
        "centerCrop"   => Some(HostInputSizing::CenterCrop),
        "stretch"      => Some(HostInputSizing::Stretch),
        _              => None,
    }
}

// Pure resolver implementing the override precedence from the spec:
//   1. DontDrawOutside: returns mode unchanged; caller must skip the mode-aware lens/params
//      transform (DontDrawOutside has its own rect logic that subsumes mode handling).
//   2. Fusion page: forces `Fit` (Fusion receives native-resolution clips, no mismatch).
//   3. Vegas host: forces `Fit` (the existing Vegas `out_rect = None` bypass owns this path).
//   4. UI dropdown == `Auto`: use the fuscript-detected mode, falling back to `Fit` when
//      fuscript is unavailable or returned an unrecognised string.
//   5. UI dropdown == explicit: that mode wins regardless of fuscript state.
//
// Note on DontDrawOutside: the return value is purposely the underlying resolved mode (not
// `Fit`), so callers can inspect what the user *would* have gotten and decide whether to
// emit a status hint. The skip-transform decision is a separate boolean caller responsibility.
pub fn resolve_host_input_sizing(
    ui_value: HostInputSizing,
    fuscript_info: Option<&CurrentFileInfo>,
    is_fusion_page: bool,
    host_name: Option<&str>,
    dont_draw_outside: bool,
) -> HostInputSizing {
    let _ = dont_draw_outside; // documented precedence; caller flag-gates the transform itself
    if is_fusion_page {
        return HostInputSizing::Fit;
    }
    if host_name == Some("com.vegascreativesoftware.vegas") {
        return HostInputSizing::Fit;
    }
    match ui_value {
        HostInputSizing::Auto => {
            fuscript_info
                .and_then(|info| info.mismatch_mode.as_deref())
                .and_then(parse_mismatch_mode)
                .unwrap_or(HostInputSizing::Fit)
        }
        explicit => explicit,
    }
}

// Compute the source-pixel crop rectangle (w, h, x, y) for FillCrop / CenterCrop, given the
// loaded source dimensions, the target timeline aspect, and any 90°/270° rotation applied to
// the source before it reaches the host. Quarter-turn rotations swap the source dimensions
// before applying the crop formula so the offset is in rotated-source space (matching the
// pipeline order Resolve / Gyroflow share: Clip Attributes rotation runs before timeline
// transform).
//
// Returns `(crop_w, crop_h, crop_x, crop_y)`. If `source_size` is already at `timeline_aspect`
// the function returns the full source unchanged (no-op).
pub fn compute_crop_geometry(
    source_size: (usize, usize),
    timeline_aspect: f64,
    video_rotation_deg: f64,
) -> (usize, usize, usize, usize) {
    // 90° / 270° (and their negatives) swap the apparent source dimensions before crop.
    let rotation = (((video_rotation_deg.round() as i64) % 360) + 360) % 360;
    let (sw, sh) = if rotation == 90 || rotation == 270 {
        (source_size.1, source_size.0)
    } else {
        source_size
    };
    if sw == 0 || sh == 0 || !timeline_aspect.is_finite() || timeline_aspect <= 0.0 {
        return (sw, sh, 0, 0);
    }
    let source_aspect = sw as f64 / sh as f64;
    if source_aspect > timeline_aspect {
        // Horizontal crop: clip the sides to match the timeline aspect.
        let crop_w = (sh as f64 * timeline_aspect).round() as usize;
        let crop_h = sh;
        let crop_x = sw.saturating_sub(crop_w) / 2;
        (crop_w, crop_h, crop_x, 0)
    } else {
        // Vertical crop (or exact-match: yields the full source with zero offsets).
        let crop_w = sw;
        let crop_h = (sw as f64 / timeline_aspect).round() as usize;
        let crop_y = sh.saturating_sub(crop_h) / 2;
        (crop_w, crop_h, 0, crop_y)
    }
}

define_params!(ParamHandler {
    strings: [
        Status              => status:           ParamHandle<String>,
        InstanceId          => instance_id:      ParamHandle<String>,
        ProjectData         => project_data:     ParamHandle<String>,
        EmbeddedLensProfile => embedded_lens:    ParamHandle<String>,
        EmbeddedPreset      => embedded_preset:  ParamHandle<String>,
        ProjectPath         => project_path:     ParamHandle<String>,
        OpenGyroflow        => open_in_gyroflow: ParamHandle<String>,
        ReloadProject       => reload_project:   ParamHandle<String>,
        OutputSizeSwap      => output_swap:      ParamHandle<String>,
        OutputSizeToTimeline=> output_size_fit:  ParamHandle<String>,
        LoadedProject       => loaded_project:   ParamHandle<String>,
        LoadedPreset        => loaded_preset:    ParamHandle<String>,
        LoadedLens          => loaded_lens:      ParamHandle<String>,
    ],
    bools: [
        DisableStretch        => disable_stretch:         ParamHandle<bool>,
        ToggleOverview        => toggle_overview:         ParamHandle<bool>,
        DontDrawOutside       => dont_draw_outside:       ParamHandle<bool>,
        IncludeProjectData    => include_project_data:    ParamHandle<bool>,
        UseGyroflowsKeyframes => use_gyroflows_keyframes: ParamHandle<bool>,
        // openfx-output-adjust-flip: must be listed here, else the macro-generated
        // get_bool_at_time match falls through to panic!("Wrong parameter type") on these.
        FlipHorizontal        => flip_horizontal:         ParamHandle<bool>,
        FlipVertical          => flip_vertical:           ParamHandle<bool>,
    ],
    f64s: [
        Fov                   => fov:                      ParamHandle<Double>,
        Smoothness            => smoothness:               ParamHandle<Double>,
        ZoomLimit             => zoom_limit:               ParamHandle<Double>,
        LensCorrectionStrength=> lens_correction_strength: ParamHandle<Double>,
        HorizonLockAmount     => horizon_lock_amount:      ParamHandle<Double>,
        HorizonLockRoll       => horizon_lock_roll:        ParamHandle<Double>,
        // PositionX             => positionx:                ParamHandle<Double>,
        // PositionY             => positiony:                ParamHandle<Double>,
        AdditionalYaw         => additional_yaw:           ParamHandle<Double>,
        AdditionalPitch       => additional_pitch:         ParamHandle<Double>,
        Rotation              => rotation:                 ParamHandle<Double>,
        VideoSpeed            => video_speed:              ParamHandle<Double>,
        OutputWidth           => output_width:             ParamHandle<Double>,
        OutputHeight          => output_height:            ParamHandle<Double>,
        // openfx-output-adjust-affine: must be listed here, else the macro-generated
        // get_f64_at_time match falls through to panic!("Wrong parameter type") on these.
        OutputZoom            => output_zoom:              ParamHandle<Double>,
        OutputRotation        => output_rotation_param:    ParamHandle<Double>,
        OutputOffsetX         => output_offset_x:          ParamHandle<Double>,
        OutputOffsetY         => output_offset_y:          ParamHandle<Double>,
        //FusionStartFrame      => fusion_start_frame:       ParamHandle<Double>,
    ],
    i32s: [
        InputRotation         => input_rotation:           ParamHandle<Int>,
        Interpolation         => interpolation:            ParamHandle<Int>,
        IntegrationMethod     => integration_method:       ParamHandle<Int>,
        ZoomMode              => zoom_mode:                ParamHandle<Int>,
    ],

    get_string:  _s p    { Ok(p.get_value()?) },
    set_string:  _s p, v { Ok(p.set_value(v.into())?) },
    get_bool:    _s p    { Ok(p.get_value() ?) },
    set_bool:    _s p, v { Ok(p.set_value(v)?) },
    get_f64:     _s p    { Ok(p.get_value() ?) },
    set_f64:     _s p, v { Ok(p.set_value(v)?) },
    get_i32:     _s p    { Ok(p.get_value() ?) },
    set_i32:     _s p, v { Ok(p.set_value(v)?) },
    set_label:   _s p, l { Ok(p.set_label(l)?) },
    set_hint:    _s p, h { Ok(p.set_hint(h) ?) },
    set_enabled: _s p, e { Ok(p.set_enabled(e)?) },
    get_bool_at_time: _s p, t    { Ok(p.get_value_at_time(frame_from_timetype(t))?) },
    get_f64_at_time:  _s p, t    { Ok(p.get_value_at_time(frame_from_timetype(t))?) },
    set_f64_at_time:  _s p, t, v { Ok(p.set_value_at_time(frame_from_timetype(t), v)?) },
    is_keyframed: _s p { p.get_num_keys().unwrap_or_default() > 0 },
    get_keyframes: _s p {
        let num_keys = p.get_num_keys().unwrap_or_default();
        let mut ret = Vec::with_capacity(num_keys as usize);
        for i in 0..num_keys {
            if let Ok(time) = p.get_key_time(i) {
                if let Ok(val) = p.get_value_at_time(time) {
                    ret.push((TimeType::Frame(time), val));
                }
            }
        }
        ret
    },
    clear_keyframes: _s p { Ok(p.delete_all_keys()?) },
});

struct InstanceData {
    source_clip: ClipInstance,
    output_clip: ClipInstance,

    params: ParamHandler,
    // Host-side manual-edit flags for each of the 5 paste-preservable params. These ride
    // across copy/paste, so they carry "B manually edited this" intent into A's instance.
    // No plugin-private shadow exists: by design, paste from B (where B did not manually
    // edit a param) discards A's prior manual edit on that param and falls back to A's
    // project default. The flag is enough to encode "B manually edited" intent.
    input_rotation_manually_edited:           ParamHandle<bool>,
    smoothness_manually_edited:               ParamHandle<bool>,
    lens_correction_strength_manually_edited: ParamHandle<bool>,
    horizon_lock_amount_manually_edited:      ParamHandle<bool>,
    zoom_mode_manually_edited:                ParamHandle<bool>,
    plugin: GyroflowPluginBaseInstance,
    supports_output_size: bool,
    is_fusion_page: bool,
    project_video_rotation: Option<f64>,
    // Captured at paste-detection time inside `check_pending_file_info`; consumed by the
    // post-reload merge step in `stab_manager`. `None` means no paste is pending.
    pending_paste_merge: Option<PendingPasteMerge>,
    // Set by `InstanceChanged(ProjectPath, Plugin)` when the incoming host value did not
    // match a plugin-initiated write. Consumed in `stab_manager` to actually run the
    // snapshot + reload + merge sequence after all paste-writes have completed for this turn.
    paste_detected: bool,
    // Cached at `CreateInstance` from `props.get_src_file_path()`. The "expected" ProjectPath
    // for this clip — used as the rewrite target when paste is detected.
    source_derived_project_path: Option<String>,
    // The last value the plugin itself wrote to `ProjectPath` (or expects to see after Browse/
    // OpenRecentProject etc.). When `InstanceChanged(ProjectPath)` fires with this value, the
    // event is our own and we consume the marker. Any other value indicates an external write
    // (paste from another node).
    expected_internal_project_path: Option<String>,
    file_path: Option<String>,

    current_file_info_pending: Arc<AtomicBool>,
    current_file_info: Arc<Mutex<Option<CurrentFileInfo>>>,

    // OFX choice param backing the HostInputSizing dropdown. Kept out of the common Params
    // enum because the underlying setting (Resolve's mismatched-resolution behaviour) is
    // unique to the Resolve OFX path. Stored as `Int` because OFX choice params are integers.
    host_input_sizing: ParamHandle<Int>,

    // Hidden OFX string param persisting the raw fuscript mismatch value (one of
    // "" / "scaleToFit" / "scaleToCrop" / "centerCrop" / "stretch"). Serialised to `.drp` and
    // copied through "Paste Attributes" by Resolve's standard OFX machinery, so on project
    // reopen each node's mismatch mode is restored without re-running fuscript.
    detected_mismatch_mode: ParamHandle<String>,

    // Tracks the mode currently baked into the stab manager so the transform is idempotent.
    // Without this, reapplying the transform on top of an already-transformed lens would
    // accumulate offsets every render.
    applied_host_input_sizing: Option<HostInputSizing>,

    // Weak ref to the stab the snapshots below were captured against. When the cache rebuilds
    // the stab (after ProjectPath change, ReloadProject, LoadLens, ever_changed trigger, ...),
    // the new Arc's identity differs and the weak upgrade either fails or returns a different
    // pointer. We detect that and reset the snapshots so the freshly-loaded lens becomes the
    // new pre-mode baseline.
    last_applied_stab: Option<std::sync::Weak<StabilizationManager>>,

    // Snapshot of lens/params state captured before the first host-input-sizing transform was
    // applied to the current stab. Used to revert to the pre-transform state when the user
    // picks a different mode, so the new transform starts from clean source-pixel coordinates
    // rather than already-cropped ones. Reset whenever `last_applied_stab` no longer matches.
    pre_mode_size: Option<(usize, usize)>,
    pre_mode_output_size: Option<(usize, usize)>,
    pre_mode_camera_matrix: Option<Vec<[f64; 3]>>,
    pre_mode_calib_dimension: Option<(usize, usize)>,

    // Guards the Stretch-mode "switch Resolve to scaleToFit/scaleToCrop" warning to once per
    // StabilizationManager instance, so it doesn't spam the log every render.
    host_input_sizing_stretch_warned: bool,
}

impl InstanceData {
    fn stab_manager(&mut self, manager_cache: &Mutex<LruCache<String, Arc<StabilizationManager>>>, output_rect: RectI, loading_pending_video_file: bool) -> Result<Arc<StabilizationManager>> {
        let out_size = ((output_rect.x2 - output_rect.x1) as usize, (output_rect.y2 - output_rect.y1) as usize);

        // If `InstanceChanged(ProjectPath)` saw an external write (paste) since last render,
        // do the paste-detection work now — once all the pasted params have settled into host.
        // We snapshot the current host state (which is B's pasted values + flags), rewrite
        // ProjectPath back to this clip's derived path, and arm the shared reload-from-project
        // block in `self.plugin.stab_manager` below. The post-reload merge then collapses the
        // snapshot against this instance's shadow per the spec's 3-tier priority.
        if self.paste_detected {
            self.paste_detected = false;
            if let Some(derived) = self.source_derived_project_path.clone() {
                self.pending_paste_merge = Some(snapshot_paste_state(
                    &self.params,
                    &self.smoothness_manually_edited,
                    &self.lens_correction_strength_manually_edited,
                    &self.horizon_lock_amount_manually_edited,
                    &self.zoom_mode_manually_edited,
                    &self.input_rotation_manually_edited,
                ));
                self.expected_internal_project_path = Some(derived.clone());
                let _ = self.params.set_string(Params::ProjectPath, &derived);
                self.plugin.reload_values_from_project = true;
                self.project_video_rotation = None;
                // Paste forces a stab cache reload — the upcoming stab Arc will be different
                // (likely fresh) and host-input-sizing snapshots / applied marker tied to the
                // previous Arc are now stale. Clear them so apply_host_input_sizing_if_needed
                // re-snapshots from the freshly-loaded baseline and re-applies the transform.
                // Without this, the new InstanceData would keep its initial None state and the
                // first render after paste might capture pre_mode against the wrong stab.
                self.applied_host_input_sizing = None;
                self.last_applied_stab = None;
                self.pre_mode_size = None;
                self.pre_mode_output_size = None;
                self.pre_mode_camera_matrix = None;
                self.pre_mode_calib_dimension = None;
                self.host_input_sizing_stretch_warned = false;
            }
        }

        // Enable the gated load-time InputRotation step in the shared stab-manager load path
        // (openfx-restore-rotation-order D1): on a cache-miss rebuild the InputRotation-implied
        // rotation + output-size transpose is applied before the post-mutation recompute, so a
        // host-restored ZoomMode never recomputes on the hybrid "rotated content + landscape
        // output_size" state. Decided per call: Fusion keeps the step disabled (the render-path
        // override is skipped there too), and Adobe/frei0r never touch this flag (default false).
        self.plugin.apply_input_rotation_on_load = !self.is_fusion_page;

        let stab = self.plugin.stab_manager(&mut self.params, manager_cache, out_size, loading_pending_video_file).map_err(|e| {
            log::error!("plugin.stab_manager error: {e:?}");
            Error::UnknownError
        })?;

        // Post-reload merge: if a paste was detected, the shared reload block above just wrote
        // `A.gyroflow` defaults into all five paste-preservable host params, overwriting both
        // B's pasted values and A's pre-paste values. Overlay the per-param merge result
        // (B-manual > A-shadow > project default) on top so the user sees the right outcome.
        if self.pending_paste_merge.is_some() && !self.is_fusion_page {
            self.apply_paste_merge()?;
            // Mirrors the old wrapper-removal block: once we've responded to a paste, avoid
            // re-running the reload on the next render even if the load reported partial success.
            self.plugin.reload_values_from_project = false;
        }
        // Whether or not a merge ran, clear the slot so a stale snapshot doesn't leak into a
        // future render (e.g. on Fusion where the merge is intentionally skipped).
        self.pending_paste_merge = None;

        Ok(stab)
    }

    // Drain `self.pending_paste_merge` and apply the 2-tier priority per param. After the
    // shared reload block has populated host params with `A.gyroflow` defaults, for each
    // param we either overwrite with B's manually-edited value (snapshot.flag = true) or
    // leave the reload's project default in place (snapshot.flag = false).
    fn apply_paste_merge(&mut self) -> Result<()> {
        let snapshot = self
            .pending_paste_merge
            .take()
            .expect("apply_paste_merge called without pending_paste_merge");

        // --- Smoothness (f64) ---
        let project_default = self.params.get_f64(Params::Smoothness).unwrap_or_default();
        let outcome = merge_paste_priority(
            snapshot.smoothness.map(|(v, f)| (PasteableValue::F64(v), f)),
            PasteableValue::F64(project_default),
        );
        if let PasteableValue::F64(v) = outcome.value {
            let _ = self.params.set_f64(Params::Smoothness, v);
        }
        let _ = self.smoothness_manually_edited.set_value(outcome.host_manual_flag);

        // --- LensCorrectionStrength (f64) ---
        let project_default = self.params.get_f64(Params::LensCorrectionStrength).unwrap_or_default();
        let outcome = merge_paste_priority(
            snapshot.lens_correction_strength.map(|(v, f)| (PasteableValue::F64(v), f)),
            PasteableValue::F64(project_default),
        );
        if let PasteableValue::F64(v) = outcome.value {
            let _ = self.params.set_f64(Params::LensCorrectionStrength, v);
        }
        let _ = self.lens_correction_strength_manually_edited.set_value(outcome.host_manual_flag);

        // --- HorizonLockAmount (f64) ---
        let project_default = self.params.get_f64(Params::HorizonLockAmount).unwrap_or_default();
        let outcome = merge_paste_priority(
            snapshot.horizon_lock_amount.map(|(v, f)| (PasteableValue::F64(v), f)),
            PasteableValue::F64(project_default),
        );
        if let PasteableValue::F64(v) = outcome.value {
            let _ = self.params.set_f64(Params::HorizonLockAmount, v);
        }
        let _ = self.horizon_lock_amount_manually_edited.set_value(outcome.host_manual_flag);

        // --- ZoomMode (i32) ---
        let project_default = self.params.get_i32(Params::ZoomMode).unwrap_or_default();
        let outcome = merge_paste_priority(
            snapshot.zoom_mode.map(|(v, f)| (PasteableValue::I32(v), f)),
            PasteableValue::I32(project_default),
        );
        if let PasteableValue::I32(v) = outcome.value {
            let _ = self.params.set_i32(Params::ZoomMode, v);
        }
        let _ = self.zoom_mode_manually_edited.set_value(outcome.host_manual_flag);

        // --- InputRotation (i32) ---
        let project_default = self.params.get_i32(Params::InputRotation).unwrap_or_default();
        let outcome = merge_paste_priority(
            snapshot.input_rotation.map(|(v, f)| (PasteableValue::I32(v), f)),
            PasteableValue::I32(project_default),
        );
        if let PasteableValue::I32(v) = outcome.value {
            let _ = self.params.set_i32(Params::InputRotation, v);
        }
        let _ = self.input_rotation_manually_edited.set_value(outcome.host_manual_flag);

        // Downstream in `Render` (and `InstanceChanged` for IR edits), `apply_openfx_input_rotation_override`
        // is called after `stab_manager` returns. It reads the (now merged) host `InputRotation`
        // and re-applies `video_rotation` / `output_size` to the StabilizationManager. So we rely
        // on the natural flow rather than calling the override here.
        Ok(())
    }
    // Apply (or re-apply, or revert + re-apply) the host-input-sizing transform to the given
    // stab. Called from `Render` after `stab_manager` returns. Idempotent: if the stab already
    // carries the target mode's transform, returns without touching it. Also detects when the
    // stab cache rebuilt the stab underneath us (different Arc identity) and rebases the
    // pre-mode snapshot from the freshly-loaded lens.
    //
    // `timeline_w` / `timeline_h` are the OFX output buffer dimensions (= timeline pixel dims
    // on Edit/Color page). `timeline_aspect` is derived from them; Stretch mode uses the raw
    // values directly as the new `stab.params.size`.
    fn apply_host_input_sizing_if_needed(
        &mut self,
        stab: &Arc<StabilizationManager>,
        mode: HostInputSizing,
        timeline_w: usize,
        timeline_h: usize,
    ) {
        let same_stab = self
            .last_applied_stab
            .as_ref()
            .and_then(|w| w.upgrade())
            .map(|s| Arc::ptr_eq(&s, stab))
            .unwrap_or(false);
        if !same_stab {
            // Fresh stab (cache rebuild) — discard stale snapshots so this stab becomes the new
            // pre-mode baseline. Also clears the once-per-stab Stretch warning gate.
            self.applied_host_input_sizing = None;
            self.pre_mode_size = None;
            self.pre_mode_output_size = None;
            self.pre_mode_camera_matrix = None;
            self.pre_mode_calib_dimension = None;
            self.host_input_sizing_stretch_warned = false;
        }
        if self.applied_host_input_sizing == Some(mode) {
            return;
        }

        // Snapshot the pre-mode baseline once per stab. We take it even for `Fit` so a later
        // switch from Fit -> FillCrop has a known-clean baseline to restore against.
        if self.pre_mode_size.is_none() {
            let params_lk = stab.params.read();
            self.pre_mode_size = Some(params_lk.size);
            self.pre_mode_output_size = Some(params_lk.output_size);
            drop(params_lk);
            let lens_lk = stab.lens.read();
            self.pre_mode_camera_matrix = Some(lens_lk.fisheye_params.camera_matrix.clone());
            self.pre_mode_calib_dimension = Some((lens_lk.calib_dimension.w, lens_lk.calib_dimension.h));
        }

        // Always restore to the pre-mode baseline before applying the new mode's transform.
        // Without this, switching FillCrop -> Stretch would Stretch an already-cropped lens.
        if let (Some(size), Some(out_size), Some(cm), Some(cd)) = (
            self.pre_mode_size,
            self.pre_mode_output_size,
            self.pre_mode_camera_matrix.as_ref().cloned(),
            self.pre_mode_calib_dimension,
        ) {
            {
                let mut params_lk = stab.params.write();
                params_lk.size = size;
                params_lk.output_size = out_size;
            }
            {
                let mut lens_lk = stab.lens.write();
                lens_lk.fisheye_params.camera_matrix = cm;
                lens_lk.calib_dimension = gyroflow_plugin_base::gyroflow_core::lens_profile::Dimensions { w: cd.0, h: cd.1 };
            }
        }

        // Apply the new mode-specific transform.
        let did_mutate = match mode {
            HostInputSizing::Auto | HostInputSizing::Fit => false,
            HostInputSizing::FillCrop | HostInputSizing::CenterCrop => {
                if timeline_w == 0 || timeline_h == 0 {
                    log::warn!(target: "ofx", "host_input_sizing: skipping FillCrop/CenterCrop transform — timeline dims are zero");
                    false
                } else {
                    let (source_size, video_rotation) = {
                        let p = stab.params.read();
                        (p.size, p.video_rotation)
                    };
                    let timeline_aspect = timeline_w as f64 / timeline_h as f64;
                    let (crop_w, crop_h, crop_x, crop_y) =
                        compute_crop_geometry(source_size, timeline_aspect, video_rotation);
                    if crop_w == 0 || crop_h == 0 {
                        log::warn!(target: "ofx", "host_input_sizing: skipping crop — geometry resolved to zero");
                        false
                    } else {
                        {
                            let mut lens_lk = stab.lens.write();
                            if lens_lk.fisheye_params.camera_matrix.len() >= 2 {
                                lens_lk.fisheye_params.camera_matrix[0][2] -= crop_x as f64;
                                lens_lk.fisheye_params.camera_matrix[1][2] -= crop_y as f64;
                            }
                            lens_lk.calib_dimension =
                                gyroflow_plugin_base::gyroflow_core::lens_profile::Dimensions { w: crop_w, h: crop_h };
                        }
                        {
                            let mut params_lk = stab.params.write();
                            params_lk.size = (crop_w, crop_h);
                            params_lk.output_size = (crop_w, crop_h);
                        }
                        log::info!(target: "ofx",
                            "host_input_sizing: mode={mode:?} crop=({crop_w}x{crop_h}) offset=({crop_x},{crop_y}) source={source_size:?} video_rotation={video_rotation}");
                        true
                    }
                }
            }
            HostInputSizing::Stretch => {
                if timeline_w == 0 || timeline_h == 0 {
                    false
                } else {
                    {
                        let mut params_lk = stab.params.write();
                        params_lk.size = (timeline_w, timeline_h);
                    }
                    if !self.host_input_sizing_stretch_warned {
                        self.host_input_sizing_stretch_warned = true;
                        log::warn!(target: "ofx",
                            "host_input_sizing: Stretch mode is best-effort — switch Resolve mismatched-resolution to scaleToFit or scaleToCrop for accurate stabilization");
                    }
                    true
                }
            }
        };

        if did_mutate {
            stab.init_size();
            // Mode transitions reshape the stab's effective camera and resampling targets, so
            // invalidate smoothing/zooming caches that depend on those dimensions before the
            // recompute. `recompute_blocking` runs the full smoothing + zoom + undistort chain
            // synchronously so the next `process_pixels` call sees consistent state.
            stab.invalidate_smoothing();
            stab.recompute_blocking();
        }

        self.applied_host_input_sizing = Some(mode);
        self.last_applied_stab = Some(Arc::downgrade(stab));
    }

    pub fn check_pending_file_info(&mut self) -> Result<bool> { // -> is_video_file
        if self.current_file_info_pending.load(SeqCst) {
            self.current_file_info_pending.store(false, SeqCst);
            let lock = self.current_file_info.lock();
            if let Some(ref current_file) = *lock {
                let new_path = current_file.project_path.clone().unwrap_or_else(|| current_file.file_path.clone());
                let old_path = self.params.get_string(Params::ProjectPath).unwrap_or_default();
                if !old_path.is_empty() && old_path != new_path {
                    // Paste detected: snapshot the incoming host state for the five
                    // paste-preservable params before triggering the reload that would clobber it.
                    // The post-reload merge in `stab_manager` consumes this and decides per param
                    // whether B-pasted, A-shadow, or A.gyroflow default wins.
                    self.pending_paste_merge = Some(snapshot_paste_state(
                        &self.params,
                        &self.smoothness_manually_edited,
                        &self.lens_correction_strength_manually_edited,
                        &self.horizon_lock_amount_manually_edited,
                        &self.zoom_mode_manually_edited,
                        &self.input_rotation_manually_edited,
                    ));
                    self.plugin.reload_values_from_project = true;
                    self.project_video_rotation = None;
                }
                // Mark this write as plugin-initiated.
                self.expected_internal_project_path = Some(new_path.clone());
                self.params.set_string(Params::ProjectPath, &new_path).unwrap(); // TODO: unwrap
                return Ok(current_file.project_path.is_none());
            }
        }
        Ok(false)
    }
}

// The five OpenFX UI-editable parameters that participate in the paste-time preservation
// framework. Each one has both a host-side `<Param>ManuallyEdited` checkbox (carrying B's
// manual-edit intent across copy/paste) and a private shadow slot on `InstanceData`
// (preserving A's prior manual value, which paste destroys in host state).
#[allow(dead_code)]
const PASTEABLE_PARAMS: [Params; 5] = [
    Params::Smoothness,
    Params::LensCorrectionStrength,
    Params::HorizonLockAmount,
    Params::ZoomMode,
    Params::InputRotation,
];

// Per-param value type tag, used by snapshot/merge logic to dispatch on the right typed
// host accessor without resorting to dyn Any.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PasteableValue {
    F64(f64),
    I32(i32),
}

// Snapshot of the 5 incoming host states captured at the moment paste is detected
// (before the shared reload block overwrites them with `A.gyroflow` defaults). Each field
// holds `Some((host_value, host_manual_flag))` once captured, `None` otherwise.
#[derive(Default, Clone, Debug, PartialEq)]
struct PendingPasteMerge {
    smoothness:               Option<(f64, bool)>,
    lens_correction_strength: Option<(f64, bool)>,
    horizon_lock_amount:      Option<(f64, bool)>,
    zoom_mode:                Option<(i32, bool)>,
    input_rotation:           Option<(i32, bool)>,
}

// Outcome of merging one paste-preservable parameter according to the 2-tier priority.
// Carries everything the caller needs to commit back to host state.
#[derive(Clone, Copy, Debug, PartialEq)]
struct MergeOutcome {
    value:            PasteableValue,
    host_manual_flag: bool,
}

// Per-param merge rule: `B manual > project default`. The "project default" was already
// written into host by the reload block, so when B did not manually edit the param we leave
// the host value alone (caller passes `project_default` from a post-reload host read purely
// so the test harness can reason about the final value without re-reading host).
//
// A's own prior manual edits are NOT preserved across paste: by design, any paste discards
// A's host-side edits on every param except those B explicitly edited. A's pre-paste host
// values for the 5 params are clobbered by paste itself before we even see the event, so
// after paste-detection's reload, only B's manual-flag intent remains as the override signal.
fn merge_paste_priority(
    b_snapshot: Option<(PasteableValue, bool)>,
    project_default: PasteableValue,
) -> MergeOutcome {
    if let Some((value, true)) = b_snapshot {
        // Priority 1: B manually edited the param. B's value wins.
        return MergeOutcome { value, host_manual_flag: true };
    }
    // Priority 2: B did not manually edit — project default already in host (from reload) stays.
    MergeOutcome { value: project_default, host_manual_flag: false }
}

// The rotation geometry (`input_rotation_target_rotation`, `input_rotation_output_size`,
// `apply_input_rotation_to_stab`) lives in gyroflow-plugin-base since
// openfx-restore-rotation-order — shared with the gated load-time rotation step inside the
// common `stab_manager` so the two paths can never fork.

fn openfx_project_rotation(project_video_rotation: &mut Option<f64>, original_project_rotation: Option<f64>, rotation_param: f64) -> f64 {
    // Prefer the rotation captured at import time (re-derived on every cache-miss rebuild —
    // the single source of truth, design D2). The `Rotation` param is written with the
    // *effective* rotation by the override itself and persisted by the host, so after a
    // restore it no longer holds the project's original rotation; reading it back is only a
    // legacy fallback for the window where no import has populated the captured value yet.
    *project_video_rotation.get_or_insert_with(|| original_project_rotation.unwrap_or(rotation_param))
}

// Capture the 5 incoming `(host_value, host_manual_flag)` pairs before the shared reload
// block overwrites them with `A.gyroflow` defaults. The caller wires the result into
// `InstanceData::pending_paste_merge`; the post-reload merge step then collapses each pair
// against A's shadow according to the per-param priority.
fn snapshot_paste_state(
    params: &ParamHandler,
    smoothness_flag:               &ParamHandle<bool>,
    lens_correction_strength_flag: &ParamHandle<bool>,
    horizon_lock_amount_flag:      &ParamHandle<bool>,
    zoom_mode_flag:                &ParamHandle<bool>,
    input_rotation_flag:           &ParamHandle<bool>,
) -> PendingPasteMerge {
    PendingPasteMerge {
        smoothness: params
            .get_f64(Params::Smoothness)
            .ok()
            .map(|v| (v, smoothness_flag.get_value().unwrap_or(false))),
        lens_correction_strength: params
            .get_f64(Params::LensCorrectionStrength)
            .ok()
            .map(|v| (v, lens_correction_strength_flag.get_value().unwrap_or(false))),
        horizon_lock_amount: params
            .get_f64(Params::HorizonLockAmount)
            .ok()
            .map(|v| (v, horizon_lock_amount_flag.get_value().unwrap_or(false))),
        zoom_mode: params
            .get_i32(Params::ZoomMode)
            .ok()
            .map(|v| (v, zoom_mode_flag.get_value().unwrap_or(false))),
        input_rotation: params
            .get_i32(Params::InputRotation)
            .ok()
            .map(|v| (v, input_rotation_flag.get_value().unwrap_or(false))),
    }
}

// `true` when the param event signals "user explicitly asked to re-derive A from project on
// disk" — i.e. they clicked one of the project-reload buttons. Such an event clears all five
// paste-preservable shadows and host flags so the next render reflects pure A.gyroflow.
fn clear_paste_shadow_for_explicit_reload(param: Params) -> bool {
    matches!(
        param,
        Params::ReloadProject | Params::LoadCurrent | Params::OpenRecentProject | Params::Browse
    )
}

fn apply_openfx_rotation_to_stab(
    project_rotation: f64,
    input_rotation_index: i32,
    output_size: (usize, usize),
    stab: &StabilizationManager,
) -> Option<f64> {
    // Shared in-place mutation (rotation + output-size transpose) from gyroflow-plugin-base;
    // this OpenFX wrapper adds the live-edit invalidation that the load-time step doesn't
    // need (there the §11.4 snapshot diff fires the single recompute instead). On a freshly
    // rebuilt stab the load-time step already applied the transpose, so the shared helper's
    // target == current early-out returns `None` and nothing is invalidated.
    let target_rotation = apply_input_rotation_to_stab(project_rotation, input_rotation_index, output_size, stab)?;
    stab.invalidate_blocking_zooming();
    stab.invalidate_blocking_undistortion();

    Some(target_rotation)
}

fn apply_openfx_input_rotation_override(
    is_fusion_page: bool,
    project_rotation: f64,
    params: &mut dyn GyroflowPluginParams,
    stab: &StabilizationManager,
) -> PluginResult<bool> {
    if is_fusion_page {
        return Ok(false);
    }

    let Some(effective_rotation) = apply_openfx_rotation_to_stab(
        project_rotation,
        params.get_i32(Params::InputRotation)?,
        (
            params.get_f64(Params::OutputWidth)? as _,
            params.get_f64(Params::OutputHeight)? as _,
        ),
        stab,
    ) else {
        return Ok(false);
    };

    params.set_f64(Params::Rotation, effective_rotation)?;

    Ok(true)
}

fn apply_openfx_input_rotation_override_to_managers(
    is_fusion_page: bool,
    project_rotation: f64,
    params: &mut dyn GyroflowPluginParams,
    managers: &mut LruCache<String, Arc<StabilizationManager>>,
) -> PluginResult<bool> {
    if is_fusion_page {
        return Ok(false);
    }

    let input_rotation_index = params.get_i32(Params::InputRotation)?;
    let output_size = (
        params.get_f64(Params::OutputWidth)? as _,
        params.get_f64(Params::OutputHeight)? as _,
    );
    let mut effective_rotation = None;
    for (_, stab) in managers.iter_mut() {
        if let Some(target_rotation) = apply_openfx_rotation_to_stab(
            project_rotation,
            input_rotation_index,
            output_size,
            stab,
        ) {
            effective_rotation = Some(target_rotation);
        }
    }

    if let Some(rotation) = effective_rotation {
        params.set_f64(Params::Rotation, rotation)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

impl Execute for GyroflowPlugin {
    #[allow(clippy::float_cmp)]
    fn execute(&mut self, _plugin_context: &PluginContext, action: &mut Action) -> Result<Int> {
        use Action::*;

        match *action {
            Render(ref mut effect, ref in_args) => {
                let _time = std::time::Instant::now();

                let time = in_args.get_time()?;
                let instance_data: &mut InstanceData = effect.get_instance_data()?;

                if let Some(path) = instance_data.file_path.take() {
                    let project_path = instance_data.params.get_string(Params::ProjectPath).unwrap_or_default();
                    let new_project_path = gyroflow_plugin_base::GyroflowPluginBase::get_project_path(&path).unwrap_or(path);
                    if project_path.is_empty() || project_path != new_project_path {
                        if !project_path.is_empty() {
                            // Same paste-detection path as `check_pending_file_info`: capture B's
                            // host state for all five paste-preservable params, then trigger reload.
                            instance_data.pending_paste_merge = Some(snapshot_paste_state(
                                &instance_data.params,
                                &instance_data.smoothness_manually_edited,
                                &instance_data.lens_correction_strength_manually_edited,
                                &instance_data.horizon_lock_amount_manually_edited,
                                &instance_data.zoom_mode_manually_edited,
                                &instance_data.input_rotation_manually_edited,
                            ));
                            instance_data.plugin.reload_values_from_project = true;
                            instance_data.project_video_rotation = None;
                            // Paste from another node is rewriting our ProjectPath to point at
                            // THIS clip's `.gyroflow` (target). The stab Arc is about to rebuild
                            // (cache miss because key path component just changed). Drop any
                            // host-input-sizing snapshots from before this paste — they reference
                            // the wrong source clip's lens/sizes. Without this reset, the next
                            // apply_host_input_sizing_if_needed would snapshot pre-mode from the
                            // freshly-built stab but reuse the stale `applied_host_input_sizing`
                            // marker, skipping the transform that the target clip actually needs.
                            instance_data.applied_host_input_sizing = None;
                            instance_data.last_applied_stab = None;
                            instance_data.pre_mode_size = None;
                            instance_data.pre_mode_output_size = None;
                            instance_data.pre_mode_camera_matrix = None;
                            instance_data.pre_mode_calib_dimension = None;
                            instance_data.host_input_sizing_stretch_warned = false;
                        }
                        // Mark this write as plugin-initiated so the followup InstanceChanged
                        // for ProjectPath does not re-trigger paste detection in a loop.
                        instance_data.expected_internal_project_path = Some(new_project_path.clone());
                        let _ = instance_data.params.set_string(Params::ProjectPath, &new_project_path);
                    }
                }

                let loading_pending_video_file = instance_data.check_pending_file_info()?;

                // Mirror the freshly-populated CurrentFileInfo into the plugin-global
                // host-input-sizing cache. `check_pending_file_info` is idempotent and only
                // does real work when the pending flag flips, so this Render-path mirror only
                // overwrites the cache when a new fuscript result came in.
                //
                // Same pass also persists the raw mismatch string into the per-node hidden
                // `DetectedMismatchMode` OFX param so subsequent `.drp` reopens can restore the
                // mode without fuscript. The set_value happens AFTER the locks above are
                // dropped, so the synchronous `InstanceChanged(DetectedMismatchMode)` callback
                // Resolve might fire (which would attempt to re-lock `current_file_info`)
                // never deadlocks. The InstanceChanged handler treats unknown param names as a
                // benign no-op (logs once), so this set_value path is safe.
                let persist_mismatch: Option<String> = {
                    let info_lock = instance_data.current_file_info.lock();
                    if let Some(ref info) = *info_lock {
                        let entry = HostInputSizingCacheEntry {
                            mismatch_mode: info.mismatch_mode.clone(),
                            timeline_w: info.timeline_w,
                            timeline_h: info.timeline_h,
                            use_custom_settings: info.use_custom_settings,
                        };
                        let mut cache_lock = self.host_input_sizing_cache.lock();
                        let needs_write = match cache_lock.as_ref() {
                            Some(existing) => {
                                existing.mismatch_mode != entry.mismatch_mode
                                    || existing.timeline_w != entry.timeline_w
                                    || existing.timeline_h != entry.timeline_h
                                    || existing.use_custom_settings != entry.use_custom_settings
                            }
                            None => true,
                        };
                        if needs_write {
                            *cache_lock = Some(entry);
                        }
                        info.mismatch_mode
                            .as_ref()
                            .filter(|s| !s.is_empty())
                            .cloned()
                    } else {
                        None
                    }
                };
                if let Some(raw) = persist_mismatch {
                    let already_persisted = instance_data
                        .detected_mismatch_mode
                        .get_value()
                        .unwrap_or_default();
                    if already_persisted != raw {
                        let _ = instance_data.detected_mismatch_mode.set_value(raw);
                    }
                }

                // Cold-fuscript first-render: rather than `return OK` (which leaves the dst
                // buffer uninitialised in Resolve — visible as a white flash for several frames
                // until fuscript responds), fall through to the normal render path. The
                // resolver falls back to `HostInputSizing::Fit` when fuscript info is missing,
                // which renders a letterboxed/centered band — visually safe and consistent
                // until the FlipX trigger fires another render with fuscript ready.

                let output_image = if in_args.get_opengl_enabled().unwrap_or_default() {
                    instance_data.output_clip.load_texture_mut(time, None)?
                } else {
                    instance_data.output_clip.get_image_mut(time)?
                };
                let output_image = output_image.borrow_mut();

                let output_rect: RectI = output_image.get_region_of_definition()?;

                let stab = instance_data.stab_manager(&self.gyroflow_plugin.manager_cache, output_rect, loading_pending_video_file)?;
                // Prefer the rotation captured at import time over the stab's current
                // `video_rotation`: after the load-time rotation step a freshly rebuilt stab
                // already carries the *effective* rotation (e.g. 90 on a restored portrait
                // setup), which would poison this cache as "project rotation" and make
                // flipping InputRotation back to 0° unable to return to the project's native
                // orientation. The stab fallback only covers the legacy window where no
                // cache-miss import has populated the captured value (then the stab is
                // unmutated and its rotation IS the project rotation).
                let original_rotation = instance_data.plugin.original_project_rotation;
                let project_rotation = *instance_data.project_video_rotation.get_or_insert_with(|| original_rotation.unwrap_or_else(|| stab.params.read().video_rotation));
                if apply_openfx_input_rotation_override(
                    instance_data.is_fusion_page,
                    project_rotation,
                    &mut instance_data.params,
                    &stab,
                ).map_err(|e| {
                    log::error!("input rotation override error: {e:?}");
                    Error::UnknownError
                })? {
                    let use_gyroflows_keyframes = instance_data.params.get_bool(Params::UseGyroflowsKeyframes).unwrap_or_default();
                    let num_frames = instance_data.plugin.num_frames;
                    let fps = instance_data.plugin.fps.max(1.0);
                    instance_data.plugin.cache_keyframes(&instance_data.params, use_gyroflows_keyframes, num_frames, fps);
                    // The override mutated `stab.params.video_rotation` / `output_size` in-place.
                    // Stale pre-mode snapshots from before the mutation would drive the apply
                    // helper's restore branch to revert the rotation swap on the next call. Reset
                    // them so apply re-snapshots from the post-override state and restore is a
                    // no-op for the rotation change. This is defense-in-depth: the
                    // InstanceChanged(InputRotation) handler already clears these synchronously,
                    // but any future entry point that triggers the override here is covered too.
                    instance_data.applied_host_input_sizing = None;
                    instance_data.last_applied_stab = None;
                    instance_data.pre_mode_size = None;
                    instance_data.pre_mode_output_size = None;
                    instance_data.pre_mode_camera_matrix = None;
                    instance_data.pre_mode_calib_dimension = None;
                }

                // Resolve and apply the host-input-sizing transform after the lens is loaded and
                // any input_rotation override has reshaped `video_rotation`/output_size. The
                // resolver and `apply_host_input_sizing_if_needed` together implement the spec's
                // override precedence: DontDrawOutside / Fusion / Vegas skip the transform; UI
                // == Auto picks up the fuscript-detected Resolve mode; UI == explicit forces it.
                //
                // Idempotency is enforced inside the apply helper, so calling this every render
                // is cheap when the mode hasn't changed and the stab Arc is the same.
                let dont_draw_outside_for_mode = instance_data
                    .params
                    .get_bool_at_time(Params::DontDrawOutside, TimeType::Frame(time))
                    .unwrap_or(false);
                let host_name_str = _plugin_context.get_host().get_name().ok();
                let host_name_ref = host_name_str.as_deref();
                let host_input_sizing_ui = HostInputSizing::from_index(
                    instance_data.host_input_sizing.get_value().unwrap_or(0),
                );
                let effective_host_input_sizing = {
                    let fuscript_lock = instance_data.current_file_info.lock();
                    resolve_host_input_sizing(
                        host_input_sizing_ui,
                        fuscript_lock.as_ref(),
                        instance_data.is_fusion_page,
                        host_name_ref,
                        dont_draw_outside_for_mode,
                    )
                };
                // Vegas + DontDrawOutside both subsume host-input-sizing on the output side; the
                // resolver already coerces Fusion to Fit, so the apply call is a cheap no-op there.
                let skip_host_input_sizing_transform = dont_draw_outside_for_mode
                    || host_name_ref == Some("com.vegascreativesoftware.vegas");
                // Thumbnail / proxy renders carry a non-1.0 render scale and arrive on a small
                // off-aspect buffer (e.g. 288×162). If they hit `apply_host_input_sizing_if_needed`
                // first (typical on a freshly-pasted instance), they bake a landscape crop into
                // `stab.params.size` and the idempotency guard then prevents the subsequent
                // full-res main render from recomputing — visible as vertical stretch on every
                // pasted clip. Defer the transform to the next full-res render where buffer
                // aspect matches the timeline's. Thumbnails will process the un-transformed stab,
                // which is acceptable for the small preview pane.
                // Thumbnail / inspector-preview renders arrive on a small buffer (typically
                // ~288x162 in DaVinci Resolve) whose aspect matches the timeline aspect, but
                // whose extent is much smaller than the timeline. When such a render hits
                // `apply_host_input_sizing_if_needed` BEFORE the corresponding full-res main
                // render, it bakes a crop computed for the wrong aspect — the main buffer's
                // aspect differs when the plugin's RoD override forces Resolve to allocate a
                // padded square buffer for the main render. The idempotency guard then prevents
                // the main render from re-applying with the correct buffer aspect, leaving
                // `stab.params.size` mismatched with the actual main buffer → visible stretch
                // on every pasted clip. Two complementary skip conditions:
                //   1. Sub-scale render (proxy / quality < 100%): host reports `render_scale<1`.
                //   2. Inspector thumbnail: buffer is < 50% of fuscript-reported timeline in
                //      either dimension. Thumbnails never exceed ~half-timeline.
                let render_scale_for_apply = output_image.get_render_scale().ok();
                let is_subscale_render = render_scale_for_apply
                    .map(|s| s.x < 0.99 || s.y < 0.99)
                    .unwrap_or(false);
                let buffer_w_usize = (output_rect.x2 - output_rect.x1) as usize;
                let buffer_h_usize = (output_rect.y2 - output_rect.y1) as usize;
                let (fuscript_tw, fuscript_th) = {
                    let info_lock = instance_data.current_file_info.lock();
                    info_lock.as_ref().map(|i| (i.timeline_w, i.timeline_h)).unwrap_or((0, 0))
                };
                // OR with an unconditional small-buffer cutoff so `.drp` restore via the
                // hidden-field fallback (no fuscript run yet → timeline_w/h still 0) is also
                // protected. Inspector thumbnails are well below 400px on a side; the smallest
                // sensible timeline render is far above it (480p shortest side = 480).
                let is_preview_thumbnail = (fuscript_tw > 0 && fuscript_th > 0
                    && (buffer_w_usize * 2 < fuscript_tw || buffer_h_usize * 2 < fuscript_th))
                    || (buffer_w_usize > 0 && buffer_h_usize > 0
                        && (buffer_w_usize < 400 || buffer_h_usize < 400));
                if !skip_host_input_sizing_transform && !is_subscale_render && !is_preview_thumbnail {
                    instance_data.apply_host_input_sizing_if_needed(
                        &stab,
                        effective_host_input_sizing,
                        buffer_w_usize,
                        buffer_h_usize,
                    );
                }

                if !instance_data.supports_output_size {
                    let _ = instance_data.params.output_width.set_enabled(false);
                    let _ = instance_data.params.output_height.set_enabled(false);
                    let _ = instance_data.params.output_swap.set_enabled(false);
                    let _ = instance_data.params.output_size_fit.set_enabled(false);
                }
                /*if !instance_data.is_fusion_page {
                    let _ = instance_data.params.fusion_start_frame.set_enabled(false);
                }*/

                // Rotation the host (e.g. DaVinci Resolve "Clip Attributes -> Rotate") applied to the clip
                // before it reached the effect. `InputRotation` is a 4-choice dropdown; map the index to
                // degrees. Defaulted from the loaded project's video_rotation in `stab_manager`. When it is
                // 90 or 270 the host handed us the rotated/displayed frame, so the input ROI must use the
                // rotated storage aspect (the full host buffer, or the correct band if the rotated frame is
                // itself letterboxed) instead of a centered storage-aspect band.
                let input_rotation_deg = input_rotation_deg_from_index(instance_data.params.get_i32(Params::InputRotation).unwrap_or(0));
                let input_rotated_90_270 = matches!((input_rotation_deg.round().abs() as i64) % 180, 90);

                let params = stab.params.read();
                let fps = params.fps;
                let src_fps = instance_data.source_clip.get_frame_rate().unwrap_or(fps);
                let org_ratio = if input_rotated_90_270 {
                    params.size.1 as f64 / params.size.0.max(1) as f64
                } else {
                    params.size.0 as f64 / params.size.1.max(1) as f64
                };
                // Aspect ratio of the core's logical output frame (`StabilizationManager` `output_size`).
                // Used to letterbox/pillarbox the stabilized output into a mismatched host buffer.
                let output_aspect = params.output_size.0 as f64 / params.output_size.1.max(1) as f64;
                let (has_accurate_timestamps, has_offsets) = {
                    let gyro = stab.gyro.read();
                    let md = gyro.file_metadata.read();
                    (md.has_accurate_timestamps, !gyro.get_offsets().is_empty())
                };

                let mut speed_stretch = 1.0;
                let mut time_adj = 0.0;
                if let Ok(range) = instance_data.source_clip.get_frame_range() {
                    if instance_data.is_fusion_page {
                        time_adj = range.min;
                    }
                    if range.max > 0.0 && !instance_data.is_fusion_page {
                        let duration_at_src_fps = (range.max / src_fps) * 1000.0;
                        speed_stretch = ((params.duration_ms.round() / duration_at_src_fps.round()) * 100.0).floor() / 100.0;
                    }
                }

                // This should cover most cases by default, and for the rest users will use Fusion
                if speed_stretch == 1.01 || speed_stretch == 0.99 || speed_stretch == 1.02 || speed_stretch == 0.98 || speed_stretch == 1.03 || speed_stretch == 0.97 {
                    speed_stretch = 1.0;
                }

                if !has_accurate_timestamps && !has_offsets {
                    instance_data.plugin.set_status(&mut instance_data.params, gyroflow_plugin_base::t!("status.not_synced"), gyroflow_plugin_base::t!("status.not_synced_hint"), false);
                } else {
                    instance_data.plugin.set_status(&mut instance_data.params, gyroflow_plugin_base::t!("status.ok"), gyroflow_plugin_base::t!("status.ok"), true);
                }

                let mut time = time;
                //let time_adj = if instance_data.is_fusion_page { instance_data.params.fusion_start_frame.get_value().unwrap_or_default() } else { 0.0 };
                time -= time_adj;
                let mut timestamp_us = ((time / src_fps * 1_000_000.0) * speed_stretch).round() as i64;

                // log::info!("fps: {fps:?}, src_fps: {src_fps:?}, speed_stretch: {speed_stretch:.6}, time: {time:?}, timestamp_us: {timestamp_us:?}");

                if (src_fps - fps).abs() > 0.01 {
                    let frame = (time / src_fps) * fps * speed_stretch;
                    timestamp_us = (frame.floor() * (1_000_000.0 / fps)).round() as i64;
                }
                if let Ok(frame) = in_args.get_src_frame() {
                    timestamp_us = (frame as f64 * (1_000_000.0 / fps)).round() as i64;
                }

                let source_timestamp_us = params.get_source_timestamp_at_ramped_timestamp(timestamp_us);
                drop(params);

                if source_timestamp_us != timestamp_us {
                    time = (source_timestamp_us as f64 / speed_stretch / 1_000_000.0 * src_fps).round();
                    timestamp_us = ((time / src_fps * 1_000_000.0) * speed_stretch).round() as i64;
                    if (src_fps - fps).abs() > 0.01 {
                        let frame = (time / src_fps) * fps * speed_stretch;
                        timestamp_us = (frame.floor() * (1_000_000.0 / fps)).round() as i64;
                    }
                }

                time += time_adj;
                let source_image = if in_args.get_opengl_enabled().unwrap_or_default() {
                    instance_data.source_clip.load_texture(time, None)?
                } else {
                    instance_data.source_clip.get_image(time)?
                };

                let source_rect: RectI = source_image.get_region_of_definition()?;

                let src_stride = source_image.get_row_bytes()? as usize;
                let out_stride = output_image.get_row_bytes()? as usize;
                let mut src_size = ((source_rect.x2 - source_rect.x1) as usize, (source_rect.y2 - source_rect.y1) as usize, src_stride);
                let mut out_size = ((output_rect.x2 - output_rect.x1) as usize, (output_rect.y2 - output_rect.y1) as usize, out_stride);

                if src_size.2 <= 0 { src_size.2 = src_size.0 * 4 * 4 }; // assuming 32-bit float
                if out_size.2 <= 0 { out_size.2 = out_size.0 * 4 * 4 }; // assuming 32-bit float

                let src_rect = GyroflowPluginBase::get_center_rect(src_size.0, src_size.1, org_ratio);

                let dont_draw_outside = instance_data.params.get_bool_at_time(Params::DontDrawOutside, TimeType::Frame(time)).unwrap(); // TODO: unwrap
                // `Fit` and DontDrawOutside both rely on the centered-content-band assumption.
                // FillCrop/CenterCrop/Stretch deliver a buffer whose entire extent is valid
                // source pixels (1:1 crop or stretched fill), so the input rect collapses to
                // None — the core's `get_rect` then treats the whole buffer as content.
                let mode_is_fit = matches!(effective_host_input_sizing, HostInputSizing::Auto | HostInputSizing::Fit);
                let effective_src_rect: Option<(usize, usize, usize, usize)> = if mode_is_fit || dont_draw_outside {
                    Some(src_rect)
                } else {
                    None
                };
                // Aspect-fit (letterbox) the stabilized output only on the Edit/Color page, where the host
                // buffers are sized to the timeline resolution and may not match the source aspect. The Fusion
                // page processes the original video at native resolution, so there is no mismatch there, and
                // `DontDrawOutside` has its own (narrower) output rect that must not be overridden.
                //
                // FillCrop/CenterCrop/Stretch already align `stab.params.output_size` to the
                // host buffer aspect, so the aspect-fit letterbox would synthesise bars that
                // don't exist — gate on `mode_is_fit` to keep those modes 1:1 buffer-filling.
                let aspect_fit_output = !dont_draw_outside && !instance_data.is_fusion_page && mode_is_fit && output_aspect.is_finite() && output_aspect > 0.0;

                let mut out_rect = if dont_draw_outside {
                    let output_ratio = out_size.0 as f64 / out_size.1 as f64;
                    let mut rect = GyroflowPluginBase::get_center_rect(src_rect.2, src_rect.3, output_ratio);
                    rect.0 += src_rect.0;
                    rect.1 += src_rect.1;
                    Some(rect)
                } else if aspect_fit_output {
                    // Largest centered sub-rect of the host buffer whose aspect ratio matches the core's
                    // logical output. When the aspects already match this is `(0, 0, out_w, out_h)`, which
                    // `StabilizationManager::get_rect` treats identically to `None` (full buffer) — so the
                    // matching-aspect path is unchanged.
                    Some(GyroflowPluginBase::get_center_rect(out_size.0, out_size.1, output_aspect))
                } else {
                    None
                };
                let out_scale = output_image.get_render_scale()?;
                if (out_scale.x != 1.0 || out_scale.y != 1.0) && !in_args.get_opengl_enabled().unwrap_or_default() {
                    // log::debug!("out_scale: {:?}", out_scale);
                    let w = (out_size.0 as f64 * out_scale.x as f64).round() as usize;
                    let h = (out_size.1 as f64 * out_scale.y as f64).round() as usize;
                    if out_size.1 > h {
                        if aspect_fit_output {
                            // Compose the proxy/half-res shrink with the aspect-fit band: recompute the band
                            // at the scaled dimensions, then translate it by the same amount the original
                            // full-buffer logic used (`out_size.1 - h`, because the y coordinate is inverted).
                            let (bx, by, bw, bh) = GyroflowPluginBase::get_center_rect(w, h, output_aspect);
                            out_rect = Some((bx, by + (out_size.1 - h), bw, bh));
                        } else {
                            out_rect = Some((
                                0,
                                out_size.1 - h, // because the coordinates are inverted
                                w,
                                h
                            ));
                        }
                    }
                }

                if _plugin_context.get_host().get_name().as_deref().ok() == Some("com.vegascreativesoftware.vegas") {
                    out_rect = None;
                }

                let input_rotation = Some(input_rotation_deg as f32);

                // openfx-output-adjust-affine: read the four sliders at the current frame
                // (host may keyframe them) and assemble PostAffine. Fusion page bypasses
                // (D7); identity values also bypass to short-circuit the cache-invalidate
                // path even though the kernel block self-short-circuits too (D4).
                // openfx-output-adjust-flip (2026-05-22): zoom range is now [1.0, 4.0] to
                // sidestep the sample-out-of-bounds bug at zoom<1; a future change can
                // re-extend the lower bound after reordering the post-affine in shaders.
                let output_post_affine: Option<PostAffine> = if instance_data.is_fusion_page {
                    None
                } else {
                    let raw_zoom = instance_data.params.get_f64_at_time(Params::OutputZoom,     TimeType::Frame(time)).unwrap_or(1.0);
                    let raw_rot  = instance_data.params.get_f64_at_time(Params::OutputRotation, TimeType::Frame(time)).unwrap_or(0.0);
                    let raw_ox   = instance_data.params.get_f64_at_time(Params::OutputOffsetX,  TimeType::Frame(time)).unwrap_or(0.0);
                    let raw_oy   = instance_data.params.get_f64_at_time(Params::OutputOffsetY,  TimeType::Frame(time)).unwrap_or(0.0);
                    let zoom   = raw_zoom.clamp(1.0, 4.0) as f32;
                    let rot    = raw_rot .clamp(-10.0, 10.0) as f32;
                    let off_x  = (raw_ox / 100.0).clamp(-0.5, 0.5) as f32;
                    let off_y  = (raw_oy / 100.0).clamp(-0.5, 0.5) as f32;
                    if zoom == 1.0 && rot == 0.0 && off_x == 0.0 && off_y == 0.0 {
                        None
                    } else {
                        log::info!(target: "app", "output_post_affine zoom={zoom} rot={rot} off=({off_x},{off_y})");
                        Some(PostAffine { rotation_deg: rot, zoom, offset_norm: [off_x, off_y] })
                    }
                };

                // openfx-output-adjust-flip: read the two checkboxes and apply Fusion-page
                // bypass (matches the post-affine bypass pattern above, D7 precedent).
                let (output_flip_h, output_flip_v): (bool, bool) = if instance_data.is_fusion_page {
                    (false, false)
                } else {
                    let raw_flip_h = instance_data.params.get_bool_at_time(Params::FlipHorizontal, TimeType::Frame(time)).unwrap_or(false);
                    let raw_flip_v = instance_data.params.get_bool_at_time(Params::FlipVertical,   TimeType::Frame(time)).unwrap_or(false);
                    if raw_flip_h || raw_flip_v {
                        log::info!(target: "app", "output_flip h={raw_flip_h} v={raw_flip_v}");
                    }
                    (raw_flip_h, raw_flip_v)
                };

                // log::debug!("src_size: {src_size:?} | src_rect: {src_rect:?}");
                // log::debug!("out_size: {out_size:?} | out_rect: {out_rect:?}");

                let buffers =
                    if in_args.get_opencl_enabled().unwrap_or_default() {
                        use std::ffi::c_void;
                        let queue = in_args.get_opencl_command_queue()? as *mut c_void;
                        Some((
                            BufferSource::OpenCL { texture: source_image.get_data()? as *mut c_void, queue },
                            BufferSource::OpenCL { texture: output_image.get_data()? as *mut c_void, queue },
                            false
                        ))
                    } else if in_args.get_metal_enabled().unwrap_or_default() {
                        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
                        { None }
                        #[cfg(any(target_os = "macos", target_os = "ios"))]
                        {
                            log::info!("metal: src_size: {src_size:?} | {src_stride}, out_size: {out_size:?} | {out_stride}");
                            instance_data.plugin.disable_opencl();
                            let command_queue = in_args.get_metal_command_queue()? as *mut std::ffi::c_void;

                            Some((
                                BufferSource::MetalBuffer { buffer: source_image.get_data()? as *mut std::ffi::c_void, command_queue },
                                BufferSource::MetalBuffer { buffer: output_image.get_data()? as *mut std::ffi::c_void, command_queue },
                                instance_data.is_fusion_page
                            ))
                        }
                    } else if in_args.get_cuda_enabled().unwrap_or_default() {
                        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
                        { None }
                        #[cfg(any(target_os = "windows", target_os = "linux"))]
                        {
                            instance_data.plugin.disable_opencl();
                            Some((
                                BufferSource::CUDABuffer { buffer: source_image.get_data()? as *mut std::ffi::c_void },
                                BufferSource::CUDABuffer { buffer: output_image.get_data()? as *mut std::ffi::c_void },
                                true
                            ))
                        }
                    } else if in_args.get_opengl_enabled().unwrap_or_default() {
                        log::info!("OpenGL: src_size: {src_size:?} | {src_stride}, out_size: {out_size:?} | {out_stride}");
                        let texture = source_image.get_opengl_texture_index()? as u32;
                        let out_texture = output_image.get_opengl_texture_index()? as u32;
                        let mut src_size = src_size;
                        let mut out_size = out_size;
                        src_size.2 = src_size.0 * 4 * (source_image.get_pixel_depth()?.bits() / 8);
                        out_size.2 = out_size.0 * 4 * (output_image.get_pixel_depth()?.bits() / 8);

                        log::info!("OpenGL in: {texture}, out: {out_texture} src_size: {src_size:?}, out_size: {out_size:?}, in_rect: {src_rect:?}, out_rect: {out_rect:?}");
                        Some((
                            BufferSource::OpenGL { texture: texture, context: std::ptr::null_mut() },
                            BufferSource::OpenGL { texture: out_texture, context: std::ptr::null_mut() },
                            true
                        ))
                    } else {
                        log::info!("CPU: src_size: {src_size:?} | {src_stride}, out_size: {out_size:?} | {out_stride}");
                        use std::slice::from_raw_parts_mut;
                        let src_buf = unsafe { match source_image.get_pixel_depth()? {
                            BitDepth::None  => { return FAILED; }
                            BitDepth::Byte  => { let b = source_image.get_descriptor::<RGBAColourB>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) },
                            BitDepth::Short => { let b = source_image.get_descriptor::<RGBAColourS>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) },
                            BitDepth::Half  => { let b = source_image.get_descriptor::<RGBAColourS>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) },
                            BitDepth::Float => { let b = source_image.get_descriptor::<RGBAColourF>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) }
                        } };
                        let dst_buf = unsafe { match output_image.get_pixel_depth()? {
                            BitDepth::None  => { return FAILED; }
                            BitDepth::Byte  => { let b = output_image.get_descriptor::<RGBAColourB>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) },
                            BitDepth::Short => { let b = output_image.get_descriptor::<RGBAColourS>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) },
                            BitDepth::Half  => { let b = output_image.get_descriptor::<RGBAColourS>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) },
                            BitDepth::Float => { let b = output_image.get_descriptor::<RGBAColourF>()?; let mut b = b.data(); from_raw_parts_mut(b.ptr_mut(0), b.bytes()) }
                        } };
                        Some((
                            BufferSource::Cpu { buffer: src_buf },
                            BufferSource::Cpu { buffer: dst_buf },
                            false
                        ))
                    };

                if effect.abort()? { return FAILED; }

                if let Some(buffers) = buffers {
                    let mut buffers = Buffers {
                        input:  BufferDescription { size: src_size, rect: effective_src_rect, data: buffers.0, rotation: input_rotation, texture_copy: buffers.2, post_affine: None,               flip_h: false,         flip_v: false         },
                        output: BufferDescription { size: out_size, rect: out_rect,           data: buffers.1, rotation: None,           texture_copy: buffers.2, post_affine: output_post_affine, flip_h: output_flip_h, flip_v: output_flip_v }
                    };

                    let processed = match output_image.get_pixel_depth()? {
                        BitDepth::None  => { return FAILED; },
                        BitDepth::Byte  => stab.process_pixels::<RGBA8>  (timestamp_us, None, &mut buffers),
                        BitDepth::Short => stab.process_pixels::<RGBA16> (timestamp_us, None, &mut buffers),
                        BitDepth::Half  => stab.process_pixels::<RGBAf16>(timestamp_us, None, &mut buffers),
                        BitDepth::Float => stab.process_pixels::<RGBAf>  (timestamp_us, None, &mut buffers)
                    };
                    match processed {
                        Ok(_) => {
                            // log::info!("Rendered | {}x{} in {:.2}ms: {:?}", src_size.0, src_size.1, _time.elapsed().as_micros() as f64 / 1000.0, _);
                            OK
                        },
                        Err(e) => {
                            log::warn!("Failed to render: {e:?}");
                            FAILED
                        }
                    }
                } else {
                    FAILED
                }
            }

            CreateInstance(ref mut effect) => {
                let param_set = effect.parameter_set()?;
                // let mut effect_props: EffectInstance = effect.properties()?;

                let source_clip = effect.get_simple_input_clip()?;
                let output_clip = effect.get_output_clip()?;

                let mut instance_data = InstanceData {
                    source_clip,
                    output_clip,
                    supports_output_size: true,
                    is_fusion_page: false,
                    project_video_rotation: None,
                    pending_paste_merge: None,
                    paste_detected: false,
                    source_derived_project_path: None,
                    expected_internal_project_path: None,
                    file_path: None,
                    input_rotation_manually_edited:           param_set.parameter("InputRotationManuallyEdited")?,
                    smoothness_manually_edited:               param_set.parameter("SmoothnessManuallyEdited")?,
                    lens_correction_strength_manually_edited: param_set.parameter("LensCorrectionStrengthManuallyEdited")?,
                    horizon_lock_amount_manually_edited:      param_set.parameter("HorizonLockAmountManuallyEdited")?,
                    zoom_mode_manually_edited:                param_set.parameter("ZoomModeManuallyEdited")?,
                    params: ParamHandler {
                        instance_id:              param_set.parameter("InstanceId")?,
                        project_data:             param_set.parameter("ProjectData")?,
                        embedded_lens:            param_set.parameter("EmbeddedLensProfile")?,
                        embedded_preset:          param_set.parameter("EmbeddedPreset")?,
                        project_path:             param_set.parameter("ProjectPath")?,
                        disable_stretch:          param_set.parameter("DisableStretch")?,
                        status:                   param_set.parameter("Status")?,
                        open_in_gyroflow:         param_set.parameter("OpenGyroflow")?,
                        reload_project:           param_set.parameter("ReloadProject")?,
                        toggle_overview:          param_set.parameter("ToggleOverview")?,
                        dont_draw_outside:        param_set.parameter("DontDrawOutside")?,
                        include_project_data:     param_set.parameter("IncludeProjectData")?,
                        input_rotation:           param_set.parameter("InputRotation")?,
                        use_gyroflows_keyframes:  param_set.parameter("UseGyroflowsKeyframes")?,
                        fov:                      param_set.parameter("Fov")?,
                        smoothness:               param_set.parameter("Smoothness")?,
                        zoom_limit:               param_set.parameter("ZoomLimit")?,
                        lens_correction_strength: param_set.parameter("LensCorrectionStrength")?,
                        horizon_lock_amount:      param_set.parameter("HorizonLockAmount")?,
                        horizon_lock_roll:        param_set.parameter("HorizonLockRoll")?,
                        video_speed:              param_set.parameter("VideoSpeed")?,
                        //positionx:                param_set.parameter("PositionX")?,
                        //positiony:                param_set.parameter("PositionY")?,
                        additional_pitch:         param_set.parameter("AdditionalPitch")?,
                        additional_yaw:           param_set.parameter("AdditionalYaw")?,
                        rotation:                 param_set.parameter("Rotation")?,
                        output_width:             param_set.parameter("OutputWidth")?,
                        output_height:            param_set.parameter("OutputHeight")?,
                        output_swap:              param_set.parameter("OutputSizeSwap")?,
                        output_size_fit:          param_set.parameter("OutputSizeToTimeline")?,
                        // openfx-output-adjust-affine: fetch handles for the 4 sliders.
                        output_zoom:              param_set.parameter("OutputZoom")?,
                        output_rotation_param:    param_set.parameter("OutputRotation")?,
                        output_offset_x:          param_set.parameter("OutputOffsetX")?,
                        output_offset_y:          param_set.parameter("OutputOffsetY")?,
                        // openfx-output-adjust-flip: fetch handles for the 2 checkboxes.
                        flip_horizontal:          param_set.parameter("FlipHorizontal")?,
                        flip_vertical:            param_set.parameter("FlipVertical")?,
                        interpolation:            param_set.parameter("Interpolation")?,
                        integration_method:       param_set.parameter("IntegrationMethod")?,
                        zoom_mode:                param_set.parameter("ZoomMode")?,

                        loaded_project:           param_set.parameter("LoadedProject")?,
                        loaded_lens:              param_set.parameter("LoadedLens")?,
                        loaded_preset:            param_set.parameter("LoadedPreset")?,

                        //fusion_start_frame:       param_set.parameter("FusionStartFrame")?,

                        fields: Default::default(),
                    },
                    plugin: GyroflowPluginBaseInstance {
                        managers:                    LruCache::new(std::num::NonZeroUsize::new(20).unwrap()),
                        original_output_size:        (0, 0),
                        original_video_size:         (0, 0),
                        timeline_size:               (0, 0),
                        num_frames:                  0,
                        fps:                         0.0,
                        reload_values_from_project:  false,
                        ever_changed:                false,
                        opencl_disabled:             false,
                        cache_keyframes_every_frame: true,
                        framebuffer_inverted:        true,
                        anamorphic_adjust_size:      true,
                        always_set_input_rotation:   false,
                        auto_disable_stretch:        true,
                        has_motion:                  false,
                        // Re-decided per stab_manager call (`!is_fusion_page`); false here only
                        // until the first call. Rotation capture happens on the first cache-miss
                        // import (openfx-restore-rotation-order).
                        apply_input_rotation_on_load: false,
                        original_project_rotation:    None,
                        keyframable_params: Arc::new(RwLock::new(KeyframableParams {
                            use_gyroflows_keyframes: param_set.parameter::<Bool>("UseGyroflowsKeyframes")?.get_value()?,
                            cached_keyframes:        KeyframeManager::default()
                        })),
                    },
                    current_file_info:         Arc::new(Mutex::new(None)),
                    current_file_info_pending: Arc::new(AtomicBool::new(false)),
                    host_input_sizing:                param_set.parameter("HostInputSizing")?,
                    detected_mismatch_mode:           param_set.parameter("DetectedMismatchMode")?,
                    applied_host_input_sizing:        None,
                    last_applied_stab:                None,
                    pre_mode_size:                    None,
                    pre_mode_output_size:             None,
                    pre_mode_camera_matrix:           None,
                    pre_mode_calib_dimension:         None,
                    host_input_sizing_stretch_warned: false,
                };
                let mut instance_id = instance_data.params.get_string(Params::InstanceId).unwrap_or_default();
                instance_data.plugin.initialize_instance_id(&mut instance_id);
                let _ = instance_data.params.set_string(Params::InstanceId, &instance_id);

                let props: EffectInstance = effect.properties()?;
                if matches!(props.get_resolve_page().as_deref(), Ok("Edit") | Ok("Color")) {
                    instance_data.supports_output_size = false;
                }
                if matches!(props.get_resolve_page().as_deref(), Ok("Fusion")) {
                    instance_data.is_fusion_page = true;
                    instance_data.plugin.auto_disable_stretch = false;
                }
                if let Ok(path) = props.get_src_file_path() {
                    if !path.is_empty() {
                        // Cache the gyroflow-project path derived from this clip's source video.
                        // Live paste detection compares incoming `host.ProjectPath` against this
                        // value: when they diverge it means another node's path was pasted in.
                        instance_data.source_derived_project_path = Some(
                            gyroflow_plugin_base::GyroflowPluginBase::get_project_path(&path)
                                .unwrap_or_else(|| path.clone()),
                        );
                        instance_data.file_path = Some(path.clone());
                    }
                }
                // The initial `host.ProjectPath` (loaded from saved project state, may be empty
                // for fresh instances) is the plugin's expected value going in — any later
                // InstanceChanged that brings a different value is external (paste).
                instance_data.expected_internal_project_path = Some(
                    instance_data.params.get_string(Params::ProjectPath).unwrap_or_default(),
                );

                // Host-input-sizing bootstrap: try to populate `current_file_info` from the
                // plugin-global cache first. Hit means a prior instance in this Resolve session
                // already paid the fuscript cold-start cost — the new instance gets the
                // project/timeline-level fields immediately and the first render applies the
                // correct FillCrop / Fit mode without a passthrough stage.
                //
                // Miss: spawn a silent query and short-block (1s) so a warm fuscript still gets
                // synchronised. If it doesn't come back in time, CreateInstance returns and the
                // Render path's Fit fallback keeps the first frames visually safe until the
                // async completion writes the cache and the query thread's FlipX trigger
                // re-renders with the correct mode.
                //
                // Fresh drop invalidation: when the user drops the plugin onto a clip with no
                // ProjectPath (fresh drop, not paste/restore), discard the global cache first.
                // The cache is otherwise sticky across the entire Resolve session — if the user
                // changed timeline / mismatched-resolution settings between session start and
                // this drop, every subsequent paste would keep using the stale mode. The fresh
                // drop is the natural "user reloads plugin to pick up new settings" signal.
                {
                    let project_path_at_create = instance_data
                        .params
                        .get_string(Params::ProjectPath)
                        .unwrap_or_default();
                    if project_path_at_create.is_empty() {
                        let mut cache_lock = self.host_input_sizing_cache.lock();
                        if cache_lock.is_some() {
                            log::info!(target: "ofx",
                                "host_input_sizing: fresh drop invalidating stale global cache");
                            *cache_lock = None;
                        }
                    }
                }
                let global_cache_hit: Option<HostInputSizingCacheEntry> =
                    self.host_input_sizing_cache.lock().clone();
                match global_cache_hit {
                    Some(entry) => {
                        // Synthesize a placeholder CurrentFileInfo carrying only the
                        // host-input-sizing fields. clip-level fields (fps / frame_count / file
                        // path) are intentionally left at placeholder defaults — `LoadCurrent`
                        // can still refresh them later if the user clicks that button.
                        *instance_data.current_file_info.lock() = Some(CurrentFileInfo {
                            file_path: String::new(),
                            project_path: None,
                            fps: 0.0,
                            duration_s: 0.0,
                            frame_count: 0,
                            width: 0,
                            height: 0,
                            pixel_aspect_ratio: String::new(),
                            mismatch_mode: entry.mismatch_mode.clone(),
                            timeline_w: entry.timeline_w,
                            timeline_h: entry.timeline_h,
                            use_custom_settings: entry.use_custom_settings,
                        });
                        log::info!(target: "ofx",
                            "host_input_sizing: CreateInstance hit global cache (mode={:?}, timeline {}x{})",
                            entry.mismatch_mode, entry.timeline_w, entry.timeline_h);
                    }
                    None => {
                        // Cache miss. Decide whether to actually spawn a fuscript query, or
                        // skip it entirely so this instance defers to whatever LATER becomes
                        // the cache value (via another instance's query, or a manual
                        // LoadCurrent / ReloadProject click on this instance).
                        //
                        // Heuristic: if `ProjectPath` is non-empty at CreateInstance time, the
                        // instance is being created from saved state — either pasted from
                        // another node (Resolve "Paste Attributes") or restored from a `.drp`
                        // project reopen. The user explicitly asked the paste path to NOT
                        // re-query fuscript; we extend the same treatment to .drp restore
                        // because both share the property that "the user did not just pick a
                        // fresh source clip", so the desired host-input-sizing data is
                        // expected to already match an existing global-cache entry (typical
                        // case: same session, cache already filled by an earlier instance) —
                        // and when it isn't (Resolve restart, no priming), the passthrough
                        // fallback in Render keeps the first frames visually clean until the
                        // user clicks LoadCurrent / ReloadProject to populate the cache.
                        //
                        // The fresh-drop case (user drags plugin onto a clip for the first
                        // time in this session) has empty `ProjectPath` at this point and
                        // does spawn the query, populating the cache for every subsequent
                        // paste / restore.
                        let project_path_at_create = instance_data
                            .params
                            .get_string(Params::ProjectPath)
                            .unwrap_or_default();
                        if !project_path_at_create.is_empty() {
                            // .drp restore / paste with empty global cache (typical: Resolve was
                            // just restarted). The per-node hidden field is the persistence
                            // layer that survives both save/restore and "Paste Attributes" — if
                            // it carries a previous fuscript result we can populate
                            // `current_file_info.mismatch_mode` directly and skip fuscript,
                            // restoring the FillCrop / scaleToFit / ... mode chosen earlier
                            // without any user action.
                            let persisted = instance_data
                                .detected_mismatch_mode
                                .get_value()
                                .unwrap_or_default();
                            if !persisted.is_empty() {
                                *instance_data.current_file_info.lock() = Some(CurrentFileInfo {
                                    file_path: String::new(),
                                    project_path: None,
                                    fps: 0.0,
                                    duration_s: 0.0,
                                    frame_count: 0,
                                    width: 0,
                                    height: 0,
                                    pixel_aspect_ratio: String::new(),
                                    mismatch_mode: Some(persisted.clone()),
                                    timeline_w: 0,
                                    timeline_h: 0,
                                    use_custom_settings: false,
                                });
                                log::info!(target: "host_input_sizing",
                                    "CreateInstance restored mismatch from hidden field (mode={persisted:?})");
                            }
                            log::info!(target: "ofx",
                                "host_input_sizing: CreateInstance with non-empty ProjectPath (paste / .drp restore) — \
                                 skipping fuscript query, relying on existing cache or user-triggered refresh");
                        } else if CurrentFileInfo::is_available() {
                            // Fire-and-forget: don't block CreateInstance on fuscript. Resolve
                            // calls CreateInstance on the UI thread, so any synchronous wait here
                            // freezes the host. The Render path's `Fit` fallback keeps the first
                            // frame visually safe, the Render path's pending-flag handler mirrors
                            // the result into the global cache once fuscript returns, and
                            // `query_silent`'s FlipX trigger forces a re-render to swap in the
                            // correct mode — accepting a deliberate two-stage UX in exchange for
                            // a non-blocking instance create.
                            CurrentFileInfo::query_silent(
                                instance_data.current_file_info.clone(),
                                instance_data.current_file_info_pending.clone(),
                            );
                        }
                    }
                }

                effect.set_instance_data(instance_data)?;

                OK
            }
            InstanceChanged(ref mut effect, ref mut in_args) => {
                let instance_data: &mut InstanceData = effect.get_instance_data()?;
                if in_args.get_name()? == "LoadCurrent" {
                    CurrentFileInfo::query(instance_data.current_file_info.clone(), instance_data.current_file_info_pending.clone());
                }
                // §8.2: re-query fuscript on ReloadProject so a user that just toggled Resolve's
                // mismatched-resolution setting between renders can have the new value picked up
                // without re-clicking "LoadCurrent". Silent — ReloadProject is a quiet refresh,
                // we don't want a dialog if fuscript happens to fail. Also clear the
                // idempotency marker so the resolved mode change is honored on the next render
                // without waiting for a stab rebuild.
                if in_args.get_name()? == "ReloadProject" && CurrentFileInfo::is_available() {
                    CurrentFileInfo::query_silent(instance_data.current_file_info.clone(), instance_data.current_file_info_pending.clone());
                    instance_data.applied_host_input_sizing = None;
                }
                // §2.5 + §7.2: handle the OFX-only HostInputSizing dropdown before the
                // FromStr-into-`Params` lookup (it's intentionally NOT in the common enum).
                // The UI dropdown is hidden (`set_secret(true)`) but the param is still defined so
                // paste round-trips serialize it. Clear `applied_host_input_sizing` so the next
                // render re-applies with the new mode (otherwise the idempotency guard would skip
                // the apply and the lens/size baked for the previous mode would persist).
                // Paste shadow is NOT cleared — `HostInputSizing` is OpenFX-only and was never
                // on the paste-preservable list.
                if in_args.get_name()? == "HostInputSizing" {
                    instance_data.applied_host_input_sizing = None;
                    if in_args.get_change_reason()? == Change::UserEdited {
                        log::info!(target: "ofx",
                            "HostInputSizing changed by user; will re-evaluate on next render");
                    }
                    return OK;
                }
                // Hidden persistence param. Plugin-initiated writes (mirror block in Render)
                // and Resolve-initiated writes (paste/.drp restore) both end up here. Either
                // way the param is pure storage — the runtime mismatch_mode comes from the
                // global cache / hidden-field fallback at CreateInstance — so this event has
                // no work to do beyond suppressing the `Unknown param name` log line below.
                if in_args.get_name()? == "DetectedMismatchMode" {
                    return OK;
                }
                if in_args.get_name()? == "Source" || in_args.get_name()? == "Output" || in_args.get_name()? == "ResolveUseAlphaForTrackCompositing" {
                    log::info!("InstanceChanged {:?} {:?}", in_args.get_name()?, in_args.get_change_reason()?);
                    return OK;
                }

                if let Ok(param) = std::str::FromStr::from_str(in_args.get_name()?.as_str()) {
                    if param == Params::OutputSizeToTimeline {
                        let rect = instance_data.source_clip.get_region_of_definition(0.0)?;
                        instance_data.plugin.timeline_size = ((rect.x2 - rect.x1) as usize, (rect.y2 - rect.y1) as usize);
                    }
                    if matches!(
                        param,
                        Params::ProjectPath | Params::ReloadProject | Params::LoadCurrent | Params::OpenRecentProject | Params::Browse
                    ) {
                        instance_data.project_video_rotation = None;
                    }
                    // Live paste detection: when a Resolve "paste node attributes" lands B's
                    // ProjectPath onto this (A's) instance, the host fires `InstanceChanged(
                    // ProjectPath, Plugin)`. Plugin-initiated writes pre-register their value in
                    // `expected_internal_project_path`, so a value that doesn't match has come
                    // from outside (paste). We mark `paste_detected` and defer the actual
                    // snapshot + reload to the next `stab_manager` call so all the other pasted
                    // params have settled into host first.
                    // NOTE: only read host.ProjectPath inside this branch to keep the FFI surface
                    // tight — calling `get_string(ProjectPath)` on every InstanceChanged (even for
                    // non-ProjectPath params) is what triggered Resolve's AV crash earlier.
                    if param == Params::ProjectPath {
                        let current_pp = instance_data
                            .params
                            .get_string(Params::ProjectPath)
                            .unwrap_or_default();
                        if instance_data.expected_internal_project_path.as_deref() == Some(current_pp.as_str()) {
                            instance_data.expected_internal_project_path = None;
                        } else if !current_pp.is_empty() {
                            instance_data.paste_detected = true;
                        }
                    }
                    if clear_paste_shadow_for_explicit_reload(param) {
                        // Explicit user request to re-derive A from disk: clear all five host
                        // manual-edit flags so the next paste correctly sees "no manual edits"
                        // and the reload's project default applies cleanly.
                        let _ = instance_data.input_rotation_manually_edited.set_value(false);
                        let _ = instance_data.smoothness_manually_edited.set_value(false);
                        let _ = instance_data.lens_correction_strength_manually_edited.set_value(false);
                        let _ = instance_data.horizon_lock_amount_manually_edited.set_value(false);
                        let _ = instance_data.zoom_mode_manually_edited.set_value(false);
                    }

                    if in_args.get_change_reason()? == Change::UserEdited {
                        // The user dragged a slider or picked a value for one of the five
                        // paste-preservable params: set the host flag so when this node is
                        // later copy/pasted *out* (A becomes the "B" source for another node C),
                        // the manual-edit intent propagates through paste.
                        match param {
                            Params::Smoothness                => { let _ = instance_data.smoothness_manually_edited.set_value(true); }
                            Params::LensCorrectionStrength    => { let _ = instance_data.lens_correction_strength_manually_edited.set_value(true); }
                            Params::HorizonLockAmount         => { let _ = instance_data.horizon_lock_amount_manually_edited.set_value(true); }
                            Params::ZoomMode                  => { let _ = instance_data.zoom_mode_manually_edited.set_value(true); }
                            Params::InputRotation             => { let _ = instance_data.input_rotation_manually_edited.set_value(true); }
                            _ => {}
                        }
                    }

                    instance_data.plugin.param_changed(&mut instance_data.params, &self.gyroflow_plugin.manager_cache, param, in_args.get_change_reason()? == Change::UserEdited).map_err(|e| {
                        log::error!("param_changed error: {e:?}");
                        Error::InvalidAction
                    })?;
                    // Browse / OpenRecentProject both internally call `set_string(ProjectPath, new)`
                    // via common::param_changed. The host then fires another InstanceChanged for
                    // ProjectPath — we pre-register the new value here so that followup event
                    // is consumed by the `expected_internal_project_path` discriminator instead of
                    // being misclassified as a paste. Only read ProjectPath when we know one of
                    // these buttons just ran, otherwise we'd be calling get_string for every event.
                    if matches!(param, Params::Browse | Params::OpenRecentProject) {
                        let new_pp = instance_data
                            .params
                            .get_string(Params::ProjectPath)
                            .unwrap_or_default();
                        if !new_pp.is_empty() {
                            instance_data.expected_internal_project_path = Some(new_pp);
                        }
                    }
                    if param == Params::InputRotation {
                        let project_rotation = openfx_project_rotation(
                            &mut instance_data.project_video_rotation,
                            instance_data.plugin.original_project_rotation,
                            instance_data.params.get_f64(Params::Rotation).unwrap_or_default(),
                        );
                        if apply_openfx_input_rotation_override_to_managers(
                            instance_data.is_fusion_page,
                            project_rotation,
                            &mut instance_data.params,
                            &mut instance_data.plugin.managers,
                        ).map_err(|e| {
                            log::error!("input rotation override error: {e:?}");
                            Error::InvalidAction
                        })? {
                            let use_gyroflows_keyframes = instance_data.params.get_bool(Params::UseGyroflowsKeyframes).unwrap_or_default();
                            let num_frames = instance_data.plugin.num_frames;
                            let fps = instance_data.plugin.fps.max(1.0);
                            instance_data.plugin.cache_keyframes(&instance_data.params, use_gyroflows_keyframes, num_frames, fps);
                        }
                        // InputRotation changes `stab.params.video_rotation` AND swaps
                        // `stab.params.output_size` via `apply_openfx_input_rotation_override_to_managers`.
                        // After the first param edit `ever_changed` is true, so subsequent
                        // InputRotation changes do NOT trigger `clear_stab` — the override is
                        // applied in-place on the existing stab Arc that the next render will
                        // reuse (same_stab=true). The stale `pre_mode_output_size` snapshot from
                        // before the override would then drive `apply`'s restore branch to revert
                        // the swap, leaving `output_size` back to the pre-rotation horizontal
                        // value while `video_rotation` stays at 90 — visible as picture offset
                        // (Fit mode) or wrong crop direction (FillCrop / CenterCrop). Clearing
                        // the snapshots forces the next apply to re-snapshot from the freshly
                        // overridden state so restore is a no-op for the rotation override.
                        instance_data.applied_host_input_sizing = None;
                        instance_data.last_applied_stab = None;
                        instance_data.pre_mode_size = None;
                        instance_data.pre_mode_output_size = None;
                        instance_data.pre_mode_camera_matrix = None;
                        instance_data.pre_mode_calib_dimension = None;
                    }
                } else {
                    log::error!("Unknown param name: {:?}", in_args.get_name()?);
                }

                OK
            }

            GetRegionOfDefinition(ref mut effect, ref in_args, ref mut out_args) => {
                let time = in_args.get_time()?;
                let instance_data = effect.get_instance_data::<InstanceData>()?;
                let rod = instance_data.source_clip.get_region_of_definition(time)?;
                let mut out_rod = rod;
                if instance_data.plugin.original_output_size != (0, 0) && !instance_data.params.get_bool_at_time(Params::DontDrawOutside, TimeType::Frame(time)).unwrap() { // TODO: unwrap
                    out_rod.x2 = instance_data.plugin.original_output_size.0 as f64;
                    out_rod.y2 = instance_data.plugin.original_output_size.1 as f64;
                }
                if let Ok(ow) = instance_data.params.get_f64(Params::OutputWidth)  { out_rod.x2 = ow; }
                if let Ok(oh) = instance_data.params.get_f64(Params::OutputHeight) { out_rod.y2 = oh; }
                out_args.set_effect_region_of_definition(out_rod)?;

                OK
            }

            DestroyInstance(ref mut effect) => {
                effect.get_instance_data::<InstanceData>()?.plugin.clear_stab(&self.gyroflow_plugin.manager_cache);
                OK
            },
            PurgeCaches(ref mut effect) => {
                effect.get_instance_data::<InstanceData>()?.plugin.clear_stab(&self.gyroflow_plugin.manager_cache);
                OK
            },

            DescribeInContext(ref mut effect, ref _in_args) => {
                let mut output_clip = effect.new_output_clip()?;
                output_clip.set_supported_components(&[ImageComponent::RGBA])?;

                let mut input_clip = effect.new_simple_input_clip()?;
                input_clip.set_supported_components(&[ImageComponent::RGBA])?;

                let mut param_set = effect.parameter_set()?;

                fn define_param(param_set: &mut ParamSetHandle, x: ParameterType, group: Option<&'static str>) -> Result<Int> {
                    match x {
                        ParameterType::HiddenString { id } => {
                            let mut param = param_set.param_define_string(id)?;
                            let _ = param.set_script_name(id);
                            param.set_secret(true)?;
                            if let Some(group) = group { param.set_parent(group)?; }
                        }
                        ParameterType::Button { id, label, hint, hidden } => {
                            if id == "CreateCamera" { return OK; }
                            if id == "LoadCurrent" && !CurrentFileInfo::is_available() {
                                return OK;
                            }
                            let mut param = param_set.param_define_button(id)?;
                            let _ = param.set_script_name(id);
                            param.set_label(label)?;
                            param.set_hint(hint)?;
                            if hidden { param.set_secret(true)?; }
                            if let Some(group) = group { param.set_parent(group)?; }
                        }
                        ParameterType::TextBox { id, label, hint, hidden } => {
                            let mut param = param_set.param_define_string(id)?;
                            let _ = param.set_script_name(id);
                            param.set_string_type(ParamStringType::SingleLine)?;
                            param.set_label(label)?;
                            param.set_hint(hint)?;
                            if hidden { param.set_secret(true)?; }
                            if let Some(group) = group { param.set_parent(group)?; }
                        }
                        ParameterType::Text { id, label, hint, hidden } => {
                            let mut param = param_set.param_define_string(id)?;
                            param.set_string_type(ParamStringType::SingleLine)?;
                            param.set_label(label)?;
                            param.set_hint(hint)?;
                            //param.set_enabled(false)?;
                            if hidden { param.set_secret(true)?; }
                            if let Some(group) = group { param.set_parent(group)?; }
                        }
                        ParameterType::Slider { id, label, hint, min, max, default, hidden } => {
                            let mut param = param_set.param_define_double(id)?;
                            param.set_default(default)?;
                            param.set_display_min(min)?;
                            param.set_display_max(max)?;
                            param.set_label(label)?;
                            param.set_hint(hint)?;
                            let _ = param.set_script_name(id);
                            if hidden { param.set_secret(true)?; }
                            if let Some(group) = group { param.set_parent(group)?; }
                        }
                        ParameterType::Checkbox { id, label, hint, default, hidden } => {
                            if id == "StabilizationSpeedRamp" { return OK; }
                            let mut param = param_set.param_define_boolean(id)?;
                            param.set_label(label)?;
                            param.set_hint(hint)?;
                            param.set_default(default)?;
                            let _ = param.set_script_name(id);
                            if hidden { param.set_secret(true)?; }
                            if let Some(group) = group { param.set_parent(group)?; }
                        }
                        ParameterType::Select { id, label, hint, options, default, hidden } => {
                            let mut param = param_set.param_define_choice(id)?;
                            param.set_label(label)?;
                            param.set_hint(hint)?;
                            param.set_default(options.iter().position(|x| *x == default).unwrap_or(0) as i32)?;
                            param.set_choices(&options)?;
                            let _ = param.set_script_name(id);
                            if hidden { param.set_secret(true)?; }
                            if let Some(group) = group { param.set_parent(group)?; }
                        }
                        ParameterType::Group { id, label, parameters, opened, hidden } => {
                            let mut param = param_set.param_define_group(id)?;
                            param.set_label(label)?;
                            param.set_group_open(opened)?;
                            if hidden { param.set_secret(true)?; }
                            if let Some(group) = group { param.set_parent(group)?; }

                            for x in parameters {
                                define_param(param_set, x, Some(id))?;
                            }
                        }
                    }
                    OK
                }

                for param in GyroflowPluginBase::get_param_definitions() {
                    define_param(&mut param_set, param, None)?;
                }
                define_param(
                    &mut param_set,
                    ParameterType::Checkbox {
                        id: "InputRotationManuallyEdited",
                        label: "Input rotation manually edited",
                        hint: "",
                        default: false,
                        hidden: true,
                    },
                    None,
                )?;
                for id in [
                    "SmoothnessManuallyEdited",
                    "LensCorrectionStrengthManuallyEdited",
                    "HorizonLockAmountManuallyEdited",
                    "ZoomModeManuallyEdited",
                ] {
                    define_param(
                        &mut param_set,
                        ParameterType::Checkbox {
                            id,
                            label: id,
                            hint: "",
                            default: false,
                            hidden: true,
                        },
                        None,
                    )?;
                }

                // Hidden per-node persistence for the fuscript mismatch result. OFX serialises
                // hidden string params to `.drp` and replicates them through "Paste Attributes",
                // so this single param gives both project save/restore and paste round-trips
                // for free. Holds the raw fuscript string (one of "" / "scaleToFit" /
                // "scaleToCrop" / "centerCrop" / "stretch"); empty means "not yet detected".
                {
                    let mut param = param_set.param_define_string("DetectedMismatchMode")?;
                    let _ = param.set_script_name("DetectedMismatchMode");
                    param.set_string_type(ParamStringType::SingleLine)?;
                    param.set_label("Detected mismatch mode")?;
                    param.set_hint("")?;
                    param.set_secret(true)?;
                }

                // OpenFX-only `HostInputSizing` choice param. Drives the input-side handling for
                // mismatched-resolution timelines. Defaults to `Auto`, which reads Resolve's
                // `timelineInputResMismatchBehavior` via fuscript (Studio + Local scripting), and
                // falls back to `Fit` (legacy letterbox path) when fuscript isn't available.
                //
                // Param is registered (so InstanceChanged / paste round-trips keep working) but
                // hidden — the Auto path covers every visible user need; manual override is reserved
                // for future debugging and is wired up via `set_secret(true)`.
                {
                    let mut param = param_set.param_define_choice("HostInputSizing")?;
                    param.set_label(gyroflow_plugin_base::t!("label.host_input_sizing"))?;
                    param.set_hint(gyroflow_plugin_base::t!("hint.host_input_sizing"))?;
                    param.set_choices(&[
                        gyroflow_plugin_base::t!("option.host_input_sizing_auto"),
                        gyroflow_plugin_base::t!("option.host_input_sizing_fit"),
                        gyroflow_plugin_base::t!("option.host_input_sizing_fill_crop"),
                        gyroflow_plugin_base::t!("option.host_input_sizing_center_crop"),
                        gyroflow_plugin_base::t!("option.host_input_sizing_stretch"),
                    ])?;
                    param.set_default(HostInputSizing::Auto as i32)?;
                    param.set_secret(true)?;
                    let _ = param.set_script_name("HostInputSizing");
                }

                param_set
                    .param_define_page("Main")?
                    .set_children(&[
                        "ProjectGroup",
                        "AdjustGroup",
                        "KeyframesGroup",
                        "ToggleOverview", "DontDrawOutside", "IncludeProjectData",
                    ])?;

                OK
            }

            OpenGLContextAttached(ref mut _effect) => { self.gyroflow_plugin.initialize_gpu_context();   OK },
            OpenGLContextDetached(ref mut _effect) => { self.gyroflow_plugin.deinitialize_gpu_context(); OK },

            Describe(ref mut effect) => {
                gyroflow_plugin_base::i18n::init();
                let supports_opencl = _plugin_context.get_host().get_opencl_render_supported().unwrap_or_default() == "true";
                let supports_opengl = _plugin_context.get_host().get_opengl_render_supported().unwrap_or_default() == "true";
                let supports_cuda   = _plugin_context.get_host().get_cuda_render_supported().unwrap_or_default() == "true";
                let supports_metal  = _plugin_context.get_host().get_metal_render_supported().unwrap_or_default() == "true";

                log::info!("Host name: {:?}", _plugin_context.get_host().get_name());
                log::info!("Host version: {:?}", _plugin_context.get_host().get_version_label());
                log::info!("Host supports OpenGL: {:?}", supports_opengl);
                log::info!("Host supports OpenCL: {:?}", supports_opencl);
                log::info!("Host supports CUDA: {:?}", supports_cuda);
                log::info!("Host supports Metal: {:?}", supports_metal);
                if !supports_opencl && !supports_opengl {
                    unsafe { std::env::set_var("NO_OPENCL", "1") };
                }
                if _plugin_context.get_host().get_name().as_deref().ok() == Some("com.vegascreativesoftware.vegas") {
                    unsafe { std::env::set_var("NO_OPENCL", "1") };
                }

                let mut effect_properties: EffectDescriptor = effect.properties()?;
                effect_properties.set_grouping("Warp")?;

                effect_properties.set_label(gyroflow_plugin_base::t!("ofx.plugin.label"))?;
                effect_properties.set_short_label(gyroflow_plugin_base::t!("ofx.plugin.short_label"))?;
                effect_properties.set_long_label(gyroflow_plugin_base::t!("ofx.plugin.long_label"))?;

                effect_properties.set_supported_pixel_depths(&[BitDepth::Byte, BitDepth::Short, BitDepth::Float])?;
                effect_properties.set_supported_contexts(&[ImageEffectContext::Filter])?;
                effect_properties.set_supports_tiles(false)?;

                effect_properties.set_single_instance(false)?;
                effect_properties.set_host_frame_threading(false)?;
                effect_properties.set_render_thread_safety(ImageEffectRender::FullySafe)?;
                effect_properties.set_supports_multi_resolution(true)?;
                effect_properties.set_temporal_clip_access(true)?;

                if supports_opengl && !supports_opencl && !supports_cuda && !supports_metal {
                    // We'll initialize the devices in OpenGLContextAttached
                    let _ = effect_properties.set_opengl_render_supported("true");
                    return OK;
                }

                let opencl_devices = gyroflow_plugin_base::opencl::OclWrapper::list_devices();
                let wgpu_devices = std::thread::spawn(|| gyroflow_plugin_base::wgpu::WgpuWrapper::list_devices()).join().unwrap();
                if !opencl_devices.is_empty() {
                    let _ = effect_properties.set_opencl_render_supported("true");
                    let _ = effect_properties.set_opengl_render_supported("true");
                }

                let _has_metal  = wgpu_devices.iter().any(|x| x.contains("(Metal)"));
                let _has_vulkan = wgpu_devices.iter().any(|x| x.contains("(Vulkan)"));
                let _has_dx12   = wgpu_devices.iter().any(|x| x.contains("(Dx12)"));

                #[cfg(target_os = "macos")]
                if !wgpu_devices.iter().any(|x| x.to_ascii_lowercase().contains("apple m")) {
                    unsafe {
                        std::env::set_var("NO_METAL", "1");
                        std::env::set_var("NO_WGPU", "1");
                    }
                }

                #[cfg(any(target_os = "macos", target_os = "ios"))]
                if _has_metal && std::env::var("NO_METAL").unwrap_or_default().is_empty() { let _ = effect_properties.set_metal_render_supported("true"); }
                #[cfg(any(target_os = "windows", target_os = "linux"))]
                if _has_vulkan || _has_dx12 { let _ = effect_properties.set_cuda_render_supported("true"); }

                OK
            }

            Load => {
				self.gyroflow_plugin.initialize_log("openfx");
                OK
            },

            _ => REPLY_DEFAULT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    // ============================================================================================
    // Rotation-mapping helpers (geometry shared with gyroflow-plugin-base since
    // openfx-restore-rotation-order; behavior unchanged). The IR-specific wrappers that used to
    // live here were removed when InputRotation joined the general paste-preserve framework.
    // ============================================================================================

    #[test]
    fn target_rotation_maps_dropdown_and_restores_project_rotation() {
        let cases = [
            (0.0, 0.0, 0, None),
            (0.0, 0.0, 1, Some(90.0)),
            (0.0, 0.0, 2, Some(-90.0)),
            (0.0, 0.0, 3, Some(180.0)),
            (270.0, 270.0, 2, None),
            (-90.0, -90.0, 2, None),
            (450.0, 90.0, 1, None),
            (90.0, 90.0, 0, None),
            (0.0, 90.0, 0, Some(0.0)),
        ];

        for (project_rotation, current_video_rotation, input_rotation_index, expected) in cases {
            assert_eq!(
                input_rotation_target_rotation(project_rotation, current_video_rotation, input_rotation_index),
                expected
            );
        }
    }

    #[test]
    fn runtime_output_size_swaps_when_rotation_quarter_turn_parity_changes() {
        assert_eq!(input_rotation_output_size(0.0, 90.0, 3840, 2160), (2160, 3840));
        assert_eq!(input_rotation_output_size(0.0, -90.0, 3840, 2160), (2160, 3840));
        assert_eq!(input_rotation_output_size(90.0, 0.0, 2160, 3840), (3840, 2160));
        assert_eq!(input_rotation_output_size(0.0, 180.0, 3840, 2160), (3840, 2160));
        assert_eq!(input_rotation_output_size(90.0, -90.0, 2160, 3840), (2160, 3840));
    }

    #[test]
    fn project_rotation_is_captured_once_before_input_rotation_overrides_mutate_rotation_param() {
        let mut project_rotation = None;

        assert_eq!(openfx_project_rotation(&mut project_rotation, None, 0.0), 0.0);
        assert_eq!(openfx_project_rotation(&mut project_rotation, None, 90.0), 0.0);
    }

    // D2: the imported project rotation (captured by the common stab_manager on every
    // cache-miss rebuild) wins over the persisted `Rotation` param, which holds the
    // *effective* rotation (90) after a restore — not the project's original (0). Without
    // this preference, flipping InputRotation back to 0° after a restart could never
    // return to the project's native orientation.
    #[test]
    fn project_rotation_prefers_imported_rotation_over_persisted_rotation_param() {
        let mut project_rotation = None;
        assert_eq!(openfx_project_rotation(&mut project_rotation, Some(0.0), 90.0), 0.0);
        // Once seeded, later calls keep the cached value regardless of inputs.
        assert_eq!(openfx_project_rotation(&mut project_rotation, Some(180.0), 90.0), 0.0);
    }

    #[test]
    fn input_rotation_override_does_not_deadlock_when_mutating_stab_params() {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = apply_openfx_rotation_to_stab(
                0.0,
                1,
                (1920, 1080),
                &StabilizationManager::default(),
            );
            let _ = tx.send(result == Some(90.0));
        });

        assert_eq!(rx.recv_timeout(Duration::from_secs(2)), Ok(true));
    }

    // 2.3: on a freshly rebuilt stab the load-time step (shared helper, runs inside the common
    // stab_manager's mutation block) already applied the transpose — the render-path override
    // must then hit the target == current early-out and mutate nothing (no re-transpose, no
    // invalidation churn, every render stays cheap).
    #[test]
    fn render_override_is_noop_after_load_time_rotation_step() {
        let stab = StabilizationManager::default();
        {
            let mut p = stab.params.write();
            p.size = (2048, 1080);
            p.output_size = (2048, 1080);
            p.video_rotation = 0.0;
        }

        // Load-time step: project rotation 0, restored InputRotation = 90° left (index 1),
        // OutputWidth/Height params still landscape.
        assert_eq!(apply_input_rotation_to_stab(0.0, 1, (2048, 1080), &stab), Some(90.0));
        assert_eq!(stab.params.read().video_rotation, 90.0);
        assert_eq!(stab.params.read().output_size, (1080, 2048));

        // Render-path override with the same param inputs: idempotent no-op.
        assert_eq!(apply_openfx_rotation_to_stab(0.0, 1, (2048, 1080), &stab), None);
        assert_eq!(stab.params.read().video_rotation, 90.0);
        assert_eq!(stab.params.read().output_size, (1080, 2048));
    }

    // ============================================================================================
    // §9 paste-preserve framework tests.
    // ============================================================================================

    // --- 9.4 explicit-reload predicate -----------------------------------------------------------

    #[test]
    fn clear_paste_shadow_for_explicit_reload_matches_only_reload_buttons() {
        assert!(clear_paste_shadow_for_explicit_reload(Params::ReloadProject));
        assert!(clear_paste_shadow_for_explicit_reload(Params::LoadCurrent));
        assert!(clear_paste_shadow_for_explicit_reload(Params::OpenRecentProject));
        assert!(clear_paste_shadow_for_explicit_reload(Params::Browse));
        // ProjectPath is the paste-detection signal, not an explicit reload — must not clear.
        assert!(!clear_paste_shadow_for_explicit_reload(Params::ProjectPath));
        // None of the five paste-preservable params themselves should trigger a clear.
        for p in PASTEABLE_PARAMS {
            assert!(!clear_paste_shadow_for_explicit_reload(p), "{p:?}");
        }
    }

    // --- merge_paste_priority core table (2-tier: B-manual > project default) -------------------

    fn b_manual(v: PasteableValue) -> Option<(PasteableValue, bool)> {
        Some((v, true))
    }
    fn b_unedited(v: PasteableValue) -> Option<(PasteableValue, bool)> {
        Some((v, false))
    }

    #[test]
    fn merge_paste_priority_b_manual_wins() {
        // B edited the param → B's value overrides whatever the reload wrote into host.
        let out = merge_paste_priority(
            b_manual(PasteableValue::F64(80.0)),
            PasteableValue::F64(50.0),
        );
        assert_eq!(
            out,
            MergeOutcome {
                value: PasteableValue::F64(80.0),
                host_manual_flag: true,
            },
        );
    }

    #[test]
    fn merge_paste_priority_b_unedited_falls_through_to_project_default() {
        // B did not edit → the reload's project default stays. Any prior A-side host value
        // was already clobbered by paste itself, so "project default" is the right outcome.
        let out = merge_paste_priority(
            b_unedited(PasteableValue::F64(50.0)),
            PasteableValue::F64(50.0),
        );
        assert_eq!(
            out,
            MergeOutcome {
                value: PasteableValue::F64(50.0),
                host_manual_flag: false,
            },
        );
    }

    #[test]
    fn merge_paste_priority_scenario_coverage_per_param() {
        // All 5 params × 2 cases. f64 params use 50/75; i32 use 0/1.
        let f64_cases = [
            // (param-label, b_value, b_flag, project_default, expected_value, expected_flag)
            ("smoothness:B-manual",      75.0, true,  50.0, 75.0, true),
            ("smoothness:project",       50.0, false, 50.0, 50.0, false),
            ("lens:B-manual",            75.0, true,  50.0, 75.0, true),
            ("lens:project",             50.0, false, 50.0, 50.0, false),
            ("horizon:B-manual",         75.0, true,  50.0, 75.0, true),
            ("horizon:project",          50.0, false, 50.0, 50.0, false),
        ];
        for (label, bv, bf, pd, ev, ef) in f64_cases {
            let out = merge_paste_priority(
                Some((PasteableValue::F64(bv), bf)),
                PasteableValue::F64(pd),
            );
            assert_eq!(out.value, PasteableValue::F64(ev), "{label}");
            assert_eq!(out.host_manual_flag, ef, "{label}");
        }

        let i32_cases = [
            ("zoom:B-manual",  1, true,  0, 1, true),
            ("zoom:project",   0, false, 0, 0, false),
            ("ir:B-manual",    1, true,  0, 1, true),
            ("ir:project",     0, false, 0, 0, false),
        ];
        for (label, bv, bf, pd, ev, ef) in i32_cases {
            let out = merge_paste_priority(
                Some((PasteableValue::I32(bv), bf)),
                PasteableValue::I32(pd),
            );
            assert_eq!(out.value, PasteableValue::I32(ev), "{label}");
            assert_eq!(out.host_manual_flag, ef, "{label}");
        }
    }

    #[test]
    fn merge_paste_priority_independent_per_parameter_evaluation() {
        // B edited only Smoothness; the rest fall through to project default.
        let out = merge_paste_priority(
            b_manual(PasteableValue::F64(75.0)),
            PasteableValue::F64(50.0),
        );
        assert_eq!(out.value, PasteableValue::F64(75.0));
        // Other 4 params: B did not edit → project default.
        for default in [PasteableValue::F64(50.0), PasteableValue::F64(40.0), PasteableValue::I32(0)] {
            let out = merge_paste_priority(Some((default, false)), default);
            assert_eq!(out.value, default);
            assert!(!out.host_manual_flag);
        }
    }

    // --- live paste detection: distinguish plugin-initiated writes from external paste ----------

    // Mirror of the live-paste discriminator used in `InstanceChanged(ProjectPath)`. The plugin
    // pre-registers its own writes via `expected_internal_project_path`; a host-fired
    // InstanceChanged whose new value matches the expected token is our own and is consumed;
    // any other non-empty value indicates external (paste) write.
    fn project_path_is_external(
        new_host_value: &str,
        expected: &mut Option<String>,
    ) -> bool {
        if expected.as_deref() == Some(new_host_value) {
            *expected = None;
            false
        } else {
            !new_host_value.is_empty()
        }
    }

    #[test]
    fn live_paste_discriminator_consumes_plugin_internal_writes() {
        // Plugin writes a derived path: expected = Some(derived). Followup InstanceChanged with
        // the same value is treated as internal and consumed.
        let mut expected = Some("/clips/A.gyroflow".to_string());
        assert!(!project_path_is_external("/clips/A.gyroflow", &mut expected));
        assert_eq!(expected, None);

        // Next InstanceChanged with a different value (paste from another node) is external.
        let mut expected = Some("/clips/A.gyroflow".to_string());
        assert!(project_path_is_external("/clips/B.gyroflow", &mut expected));
        // Detection does NOT consume `expected` so a subsequent plugin write can still match.
        assert_eq!(expected, Some("/clips/A.gyroflow".to_string()));
    }

    #[test]
    fn live_paste_discriminator_skips_empty_values() {
        // Empty ProjectPath (fresh instance, no project yet) is not paste.
        let mut expected = Some("/clips/A.gyroflow".to_string());
        assert!(!project_path_is_external("", &mut expected));
    }

    // --- 9.5 fusion page gating contract ---------------------------------------------------------

    // `apply_paste_merge` is called from `stab_manager` only when `pending_paste_merge.is_some() &&
    // !is_fusion_page`. The pure logic of that gate is captured here so the contract is testable
    // without an InstanceData (which requires a live OFX runtime).
    fn should_apply_paste_merge(pending: bool, is_fusion_page: bool) -> bool {
        pending && !is_fusion_page
    }

    #[test]
    fn fusion_page_skips_merge_even_with_pending_snapshot() {
        assert!(should_apply_paste_merge(true, false));   // Edit/Color: run merge
        assert!(!should_apply_paste_merge(true, true));   // Fusion: skip
        assert!(!should_apply_paste_merge(false, false)); // No pending: nothing to do
        assert!(!should_apply_paste_merge(false, true));  // Fusion + no pending: skip
    }

    // --- sequential pastes converge --------------------------------------------------------------

    #[test]
    fn sequential_pastes_converge() {
        // No plugin-private shadow: each paste is resolved purely against B's manual-flag and
        // the reload's project default. Sequential pastes therefore behave like independent
        // resolutions on each paste step.
        //
        // Setup: A's project default for Smoothness = 50, for LCS = 50.
        //
        // Paste from B (B has only LensCorrectionStrength manually edited to 40):
        //   Smoothness → B not edited → A's project default 50.
        //   LCS        → B edited     → 40.
        let sm = merge_paste_priority(
            b_unedited(PasteableValue::F64(50.0)),
            PasteableValue::F64(50.0),
        );
        assert_eq!(sm.value, PasteableValue::F64(50.0));
        assert!(!sm.host_manual_flag);

        let lcs = merge_paste_priority(
            b_manual(PasteableValue::F64(40.0)),
            PasteableValue::F64(50.0),
        );
        assert_eq!(lcs.value, PasteableValue::F64(40.0));
        assert!(lcs.host_manual_flag);

        // Paste from C (C has only Smoothness manually edited to 90):
        //   Smoothness → C edited     → 90.
        //   LCS        → C not edited → A's project default 50 (the prior 40 is gone).
        let sm = merge_paste_priority(
            b_manual(PasteableValue::F64(90.0)),
            PasteableValue::F64(50.0),
        );
        assert_eq!(sm.value, PasteableValue::F64(90.0));
        assert!(sm.host_manual_flag);

        let lcs = merge_paste_priority(
            b_unedited(PasteableValue::F64(50.0)),
            PasteableValue::F64(50.0),
        );
        assert_eq!(lcs.value, PasteableValue::F64(50.0));
        assert!(!lcs.host_manual_flag);
    }

    // ============================================================================================
    // §1-§4 host-input-sizing helpers.
    // ============================================================================================

    fn make_info(mismatch: Option<&str>) -> CurrentFileInfo {
        CurrentFileInfo {
            file_path: String::new(),
            project_path: None,
            fps: 30.0,
            duration_s: 1.0,
            frame_count: 30,
            width: 0,
            height: 0,
            pixel_aspect_ratio: String::new(),
            mismatch_mode: mismatch.map(|s| s.to_string()),
            timeline_w: 1920,
            timeline_h: 1920,
            use_custom_settings: false,
        }
    }

    #[test]
    fn parse_mismatch_mode_maps_all_four_strings() {
        assert_eq!(parse_mismatch_mode("scaleToFit"),  Some(HostInputSizing::Fit));
        assert_eq!(parse_mismatch_mode("scaleToCrop"), Some(HostInputSizing::FillCrop));
        assert_eq!(parse_mismatch_mode("centerCrop"),  Some(HostInputSizing::CenterCrop));
        assert_eq!(parse_mismatch_mode("stretch"),     Some(HostInputSizing::Stretch));
        // Trim whitespace defensively (fuscript stdout sometimes carries trailing newlines).
        assert_eq!(parse_mismatch_mode("  scaleToCrop\n"), Some(HostInputSizing::FillCrop));
    }

    #[test]
    fn parse_mismatch_mode_rejects_empty_and_unknown() {
        assert_eq!(parse_mismatch_mode(""), None);
        assert_eq!(parse_mismatch_mode("scaletofit"), None); // wrong case
        assert_eq!(parse_mismatch_mode("nope"), None);
    }

    #[test]
    fn resolve_host_input_sizing_fusion_forces_fit_over_ui_fill_crop() {
        let info = make_info(Some("scaleToCrop"));
        // Fusion takes precedence over both UI and fuscript: native-resolution clips don't mismatch.
        let mode = resolve_host_input_sizing(HostInputSizing::FillCrop, Some(&info), true, None, false);
        assert_eq!(mode, HostInputSizing::Fit);
    }

    #[test]
    fn resolve_host_input_sizing_vegas_forces_fit_under_auto() {
        let info = make_info(Some("scaleToCrop"));
        let mode = resolve_host_input_sizing(
            HostInputSizing::Auto,
            Some(&info),
            false,
            Some("com.vegascreativesoftware.vegas"),
            false,
        );
        assert_eq!(mode, HostInputSizing::Fit);
    }

    #[test]
    fn resolve_host_input_sizing_auto_picks_fuscript_scale_to_crop() {
        let info = make_info(Some("scaleToCrop"));
        let mode = resolve_host_input_sizing(HostInputSizing::Auto, Some(&info), false, Some("com.blackmagicdesign.resolve"), false);
        assert_eq!(mode, HostInputSizing::FillCrop);
    }

    #[test]
    fn resolve_host_input_sizing_auto_falls_back_to_fit_when_fuscript_missing() {
        // No fuscript info at all (Resolve Free / non-Resolve host).
        let mode = resolve_host_input_sizing(HostInputSizing::Auto, None, false, Some("org.darktable"), false);
        assert_eq!(mode, HostInputSizing::Fit);
        // fuscript ran but returned an unknown / empty value.
        let info = make_info(None);
        let mode = resolve_host_input_sizing(HostInputSizing::Auto, Some(&info), false, None, false);
        assert_eq!(mode, HostInputSizing::Fit);
    }

    #[test]
    fn resolve_host_input_sizing_explicit_ui_overrides_fuscript() {
        let info = make_info(Some("scaleToFit"));
        // User picked FillCrop explicitly — that wins even if Resolve says scaleToFit.
        let mode = resolve_host_input_sizing(HostInputSizing::FillCrop, Some(&info), false, None, false);
        assert_eq!(mode, HostInputSizing::FillCrop);
    }

    #[test]
    fn resolve_host_input_sizing_auto_picks_up_stretch() {
        let info = make_info(Some("stretch"));
        let mode = resolve_host_input_sizing(HostInputSizing::Auto, Some(&info), false, None, false);
        assert_eq!(mode, HostInputSizing::Stretch);
    }

    // --- compute_crop_geometry ------------------------------------------------------------------

    #[test]
    fn compute_crop_geometry_horizontal_crop() {
        // 3840×1920 source on a 1920×1920 (1.0 aspect) timeline -> trim the sides.
        let (w, h, x, y) = compute_crop_geometry((3840, 1920), 1.0, 0.0);
        assert_eq!((w, h, x, y), (1920, 1920, 960, 0));
    }

    #[test]
    fn compute_crop_geometry_vertical_crop() {
        // 1080×1920 vertical source on a 1920×1920 timeline -> trim top/bottom to a square.
        let (w, h, x, y) = compute_crop_geometry((1080, 1920), 1.0, 0.0);
        assert_eq!((w, h, x, y), (1080, 1080, 0, 420));
    }

    #[test]
    fn compute_crop_geometry_matching_aspect_is_noop() {
        // 1920×1080 source on a 16:9 timeline (1920/1080 = 1.7777…) -> exact match, full source.
        let (w, h, x, y) = compute_crop_geometry((1920, 1080), 1920.0 / 1080.0, 0.0);
        assert_eq!((w, h, x, y), (1920, 1080, 0, 0));
    }

    #[test]
    fn compute_crop_geometry_90deg_rotated_source_swaps_dims_first() {
        // Stored 1920×1080 with InputRotation=90°: the displayed frame is 1080×1920 (vertical).
        // On a 1920×1920 timeline this crops top/bottom to (1080,1080) at offset (0,420),
        // matching the rotated-source vertical-crop case.
        let (w, h, x, y) = compute_crop_geometry((1920, 1080), 1.0, 90.0);
        assert_eq!((w, h, x, y), (1080, 1080, 0, 420));
    }
}
