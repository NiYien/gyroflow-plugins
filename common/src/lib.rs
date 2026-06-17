pub mod i18n;

use lru::LruCache;
use parking_lot::{ Mutex, RwLock };
use std::sync::{ Arc, atomic::AtomicBool };

pub use gyroflow_core::{ StabilizationManager, keyframes::*, stabilization::*, filesystem, gpu::* };
pub use gyroflow_core;

// re-exports
pub use rfd;
pub use parking_lot;
pub use lru;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub type PluginResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct GyroflowLaunchCommand {
    program: String,
    args: Vec<String>,
    hide_window: bool,
}

impl GyroflowLaunchCommand {
    fn spawn(self) -> std::io::Result<std::process::Child> {
        let mut cmd = std::process::Command::new(&self.program);
        #[cfg(target_os = "windows")]
        {
            if self.hide_window {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
            }
        }
        cmd.args(&self.args).spawn()
    }
}

// Snapshot of `StabilizationManager` fields that feed
// `gyroflow_core::stabilization::compute_params::ComputeParams::from_manager`.
// Captured pre- and post-mutation inside `stab_manager()` so the post-import
// recompute pair (`invalidate_smoothing()` + `recompute_blocking()`) can be
// skipped when no relevant input changed. Field list derived from a direct
// code audit of `from_manager` (see gyroflow/src/core/stabilization/compute_params.rs).
//
// Raw stretch mirror fields (`input_horizontal_stretch_raw`,
// `input_vertical_stretch_raw`) are included even though they are not directly
// read by `from_manager`. The motivation comes from §10: `apply_anamorphic_decay`
// reads them, and any later change in the raw mirror semantically affects the
// compute output. Both raw and mutating fields are tracked to detect any state
// change. In the common anamorphic case, `disable_lens_stretch` only changes
// the mutating fields (raw stays at λ), so the snapshot still diffs and fires
// — this is correct: size also changed, so a recompute is required. The §10
// gain is render coherence with the desktop app, not skip-count.
#[derive(Clone, Debug, PartialEq)]
struct ComputeInputsSnapshot {
    size: (usize, usize),
    output_size: (usize, usize),
    video_rotation: f64,
    adaptive_zoom_window: f64,
    adaptive_zoom_method: i32,
    input_horizontal_stretch: f64,
    input_vertical_stretch: f64,
    input_horizontal_stretch_raw: Option<f64>,
    input_vertical_stretch_raw: Option<f64>,
    integration_method: usize,
    keyframes_hash: u64,
    smoothing_hash: u64,
}

fn hash_str(s: &str) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(s.as_bytes());
    h.finish()
}

// Lock-ordering contract: acquires read guards on `params`, `lens`, `gyro`,
// `keyframes`, and `smoothing` in that exact order. Callers MUST NOT hold a
// write guard on any of those locks when invoking, otherwise parking_lot's
// non-reentrant RwLock will deadlock. Callers SHOULD NOT hold any other read
// guard on the same stab manager either (read-read against parking_lot is
// safe today but a concurrent writer between calls can cause writer-starvation
// fairness issues). Current call sites in `stab_manager` satisfy this — the
// pre/post snapshots run in the cache-miss construction block where the
// manager is exclusively owned. Future call sites in render / async paths
// MUST re-audit before adding.
fn snapshot_compute_inputs(stab: &StabilizationManager) -> ComputeInputsSnapshot {
    let p = stab.params.read();
    let lens = stab.lens.read();
    let gyro = stab.gyro.read();
    let kf = stab.keyframes.read();
    let smoothing = stab.smoothing.read();

    let keyframes_hash = serde_json::to_string(&*kf)
        .map(|s| hash_str(&s))
        .unwrap_or(0);
    let smoothing_hash = hash_str(&smoothing.current().get_parameters_json().to_string());

    ComputeInputsSnapshot {
        size: p.size,
        output_size: p.output_size,
        video_rotation: p.video_rotation,
        adaptive_zoom_window: p.adaptive_zoom_window,
        adaptive_zoom_method: p.adaptive_zoom_method,
        input_horizontal_stretch: lens.input_horizontal_stretch,
        input_vertical_stretch: lens.input_vertical_stretch,
        input_horizontal_stretch_raw: lens.input_horizontal_stretch_raw(),
        input_vertical_stretch_raw: lens.input_vertical_stretch_raw(),
        integration_method: gyro.integration_method,
        keyframes_hash,
        smoothing_hash,
    }
}

// §11.7: throttle the retry storm seen in gyroflow-openfx.log (~70 attempts in
// 11 s on a wrong path). Resolve calls stab_manager() many times during a
// missing-file state; caching the NotFound result short-circuits the kernel
// open(2) and the surrounding log noise. Cache TTL is 5 s so a user fixing a
// typo still gets a fresh attempt within a few frames after correction. The
// cache keys on the resolved video path (input to filesystem::open_file).
const NOT_FOUND_THROTTLE_MS: u128 = 5_000;
static NOT_FOUND_CACHE: std::sync::OnceLock<Mutex<std::collections::HashMap<String, (std::time::Instant, String)>>> = std::sync::OnceLock::new();

fn not_found_cache() -> &'static Mutex<std::collections::HashMap<String, (std::time::Instant, String)>> {
    NOT_FOUND_CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn check_not_found_cache(path: &str) -> Option<String> {
    let cache = not_found_cache().lock();
    cache.get(path).and_then(|(t, msg)| {
        if t.elapsed().as_millis() < NOT_FOUND_THROTTLE_MS {
            Some(msg.clone())
        } else {
            None
        }
    })
}

fn record_not_found(path: &str, msg: String) {
    not_found_cache().lock().insert(path.to_string(), (std::time::Instant::now(), msg));
}

fn clear_not_found(path: &str) {
    not_found_cache().lock().remove(path);
}

impl ComputeInputsSnapshot {
    fn diff(&self, other: &Self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        if self.size != other.size { out.push("size"); }
        if self.output_size != other.output_size { out.push("output_size"); }
        if self.video_rotation != other.video_rotation { out.push("video_rotation"); }
        if self.adaptive_zoom_window != other.adaptive_zoom_window { out.push("adaptive_zoom_window"); }
        if self.adaptive_zoom_method != other.adaptive_zoom_method { out.push("adaptive_zoom_method"); }
        if self.input_horizontal_stretch != other.input_horizontal_stretch { out.push("input_horizontal_stretch"); }
        if self.input_vertical_stretch != other.input_vertical_stretch { out.push("input_vertical_stretch"); }
        if self.input_horizontal_stretch_raw != other.input_horizontal_stretch_raw { out.push("input_horizontal_stretch_raw"); }
        if self.input_vertical_stretch_raw != other.input_vertical_stretch_raw { out.push("input_vertical_stretch_raw"); }
        if self.integration_method != other.integration_method { out.push("integration_method"); }
        if self.keyframes_hash != other.keyframes_hash { out.push("keyframes"); }
        if self.smoothing_hash != other.smoothing_hash { out.push("smoothing"); }
        out
    }
}

#[derive(Debug, Copy, Clone, Hash, PartialEq, PartialOrd, Eq, Ord, serde::Serialize, serde::Deserialize)]
pub enum Params {
    Logo,
    InstanceId,
    ProjectData,
    EmbeddedLensProfile,
    EmbeddedPreset,
    ProjectGroup, ProjectGroupEnd,
    LoadCurrent,
    ProjectPath,
    Browse,
    LoadLens,
    OpenGyroflow,
    ReloadProject,
    OpenRecentProject,
    Status,
    AdjustGroup, AdjustGroupEnd,
    Fov,
    Smoothness,
    ZoomLimit,
    LensCorrectionStrength,
    HorizonLockAmount,
    HorizonLockRoll,
    // PositionX,
    // PositionY,
    AdditionalPitch,
    AdditionalYaw,
    InputRotation,
    Rotation,
    VideoSpeed,
    DisableStretch,
    IntegrationMethod,
    ZoomMode,
    KeyframesGroup, KeyframesGroupEnd,
    UseGyroflowsKeyframes,
    RecalculateKeyframes,
    // openfx-output-adjust-affine: output-stage post-affine UI (OpenFX only).
    OutputAdjustGroup, OutputAdjustGroupEnd,
    OutputZoom,
    OutputRotation,
    OutputOffsetX,
    OutputOffsetY,
    // openfx-output-adjust-flip: post-stab mirror toggles, OpenFX only.
    FlipHorizontal,
    FlipVertical,
    OutputSizeGroup, OutputSizeGroupEnd,
    OutputWidth,
    OutputHeight,
    OutputSizeToTimeline,
    OutputSizeSwap,
    ToggleOverview,
    DontDrawOutside,
    IncludeProjectData,
    StabilizationSpeedRamp,
    InfoGroup, InfoGroupEnd,
    LoadedProject,
    LoadedPreset,
    LoadedLens,
    CreateCamera,
    Interpolation,
    FusionStartFrame,
}

// ---- Plugin file logger ----
//
// The After Effects entry macro (after-effects crate) registers `win_dbg_logger` as the
// process-global `log` logger at plugin load (PF_Cmd_GLOBAL_SETUP / EntryPointFunc), before any of
// our command/render code runs. Because `log::set_logger` is one-shot, `simplelog::WriteLogger::init`
// in the old `initialize_log` then failed silently: `gyroflow-adobe.log` was created (truncated) but
// never written, and every record went to the debugger only. The Adobe plugin wins the global slot
// from a DLL-load constructor (see `adobe/src/lib.rs`) that calls `ensure_file_logger` BEFORE the
// macro. To stay safe under the Windows loader lock, the constructor only performs cheap atomic
// registration here; the real file open (`data_dir()` -> `SHGetKnownFolderPath`, `File::create`) is
// deferred to the first emitted record, which happens at command/render time, outside loader lock.
// Records are tee'd to `win_dbg_logger` so debug-build DebugView output is preserved unchanged.
struct PluginFileLogger {
    inner: RwLock<Option<Box<dyn log::Log>>>,
    name: std::sync::OnceLock<String>,
    open_attempted: AtomicBool,
}
impl PluginFileLogger {
    const fn new() -> Self {
        Self { inner: RwLock::new(None), name: std::sync::OnceLock::new(), open_attempted: AtomicBool::new(false) }
    }
    fn ensure_inner(&self) {
        use std::sync::atomic::Ordering;
        if self.open_attempted.load(Ordering::Acquire) { return; }
        let mut guard = self.inner.write();
        if self.open_attempted.swap(true, Ordering::AcqRel) { return; }
        let name = self.name.get().map(String::as_str).unwrap_or("plugin");
        let log_config = [ "mp4parse", "wgpu", "naga", "akaze", "ureq", "rustls", "ofx" ]
            .into_iter()
            .fold(simplelog::ConfigBuilder::new(), |mut cfg, x| { cfg.add_filter_ignore_str(x); cfg })
            .build();
        let data_path = gyroflow_core::settings::data_dir().join(format!("gyroflow-{name}.log"));
        let tmp_path  = std::env::temp_dir().join(format!("gyroflow-{name}.log"));
        let file = std::fs::File::create(&data_path).or_else(|_| std::fs::File::create(&tmp_path));
        match file {
            Ok(file) => { *guard = Some(simplelog::WriteLogger::new(log::LevelFilter::Debug, log_config, file)); }
            Err(e)   => { eprintln!("Failed to create plugin log file: {data_path:?} / {tmp_path:?}: {e}"); }
        }
    }
}
impl log::Log for PluginFileLogger {
    fn enabled(&self, _: &log::Metadata) -> bool { true }
    fn log(&self, record: &log::Record) {
        // Tee to the debugger (DebugView) so debug-build behavior is unchanged.
        log::Log::log(&win_dbg_logger::DEBUGGER_LOGGER, record);
        self.ensure_inner();
        let guard = self.inner.read();
        if let Some(inner) = guard.as_deref() { inner.log(record); }
    }
    fn flush(&self) {
        let guard = self.inner.read();
        if let Some(inner) = guard.as_deref() { inner.flush(); }
    }
}
static PLUGIN_FILE_LOGGER: PluginFileLogger = PluginFileLogger::new();

/// Register the plugin file logger as the process-global `log` logger and remember which file to
/// write (`gyroflow-{name}.log`). Cheap and loader-lock-safe: only atomic registration happens here,
/// the file is opened lazily on the first record. Idempotent — safe to call from a DLL-load
/// constructor and again from `initialize_log`. Winning the global slot first makes the host's later
/// `win_dbg_logger` registration a no-op while still tee'ing records to the debugger.
pub fn ensure_file_logger(name: &str) {
    let _ = PLUGIN_FILE_LOGGER.name.set(name.to_string());
    if log::set_logger(&PLUGIN_FILE_LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Debug);
    }
}

pub struct GyroflowPluginBase {
    // We should cache managers globally because it's common to have the effect applied to the same clip and cut the clip into multiple pieces
    // We don't want to create a new manager for each piece of the same clip
    // Cache key is specific enough
    pub manager_cache: Mutex<LruCache<String, Arc<StabilizationManager>>>,

    pub context_initialized: bool,
}
impl Default for GyroflowPluginBase {
    fn default() -> Self {
        Self {
            manager_cache: Mutex::new(LruCache::new(std::num::NonZeroUsize::new(8).unwrap())),
            context_initialized: false,
        }
    }
}

impl GyroflowPluginBase {
    /// If `disable_stretch` is true, inject a `plugin_disable_stretch` flag into gyroflow JSON data
    /// so that the setting persists when the data is embedded in a preset or project.
    fn maybe_inject_disable_stretch(data: &str, disable_stretch: bool) -> String {
        if !disable_stretch { return data.to_string(); }
        if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(data) {
            json["plugin_disable_stretch"] = serde_json::Value::Bool(true);
            if let Ok(s) = serde_json::to_string(&json) {
                return s;
            }
        }
        data.to_string()
    }

    pub fn initialize_gpu_context(&mut self) {
        log::info!("GyroflowPluginBase::initialize_gpu_context");
        if !self.context_initialized {
            gyroflow_core::gpu::initialize_contexts();
            self.context_initialized = true;
        }
    }
    pub fn deinitialize_gpu_context(&mut self) {
        log::info!("GyroflowPluginBase::deinitialize_gpu_context");
    }

    pub fn initialize_log(&mut self, name: &str) {
        // Install the file logger (idempotent; usually already done by the DLL-load constructor on
        // the Adobe plugin, but the only init point on the OpenFX / frei0r plugins which have no
        // competing global logger).
        ensure_file_logger(name);
        // (Re-)install the panic logger. Done here, at command/render time, rather than in the
        // constructor, so it runs AFTER the host entry macro sets its own panic hook and therefore
        // wins — preserving the prior behavior of routing panics to the log file.
        static PANIC_HOOK: std::sync::Once = std::sync::Once::new();
        PANIC_HOOK.call_once(log_panics::init);
    }

    pub fn get_center_rect(width: usize, height: usize, org_ratio: f64) -> (usize, usize, usize, usize) {
        // If aspect ratio is different
        let new_ratio = width as f64 / height as f64;
        if (new_ratio - org_ratio).abs() > 0.1 {
            // Get center rect of original aspect ratio
            let rect = if new_ratio > org_ratio {
                ((height as f64 * org_ratio).round() as usize, height)
            } else {
                (width, (width as f64 / org_ratio).round() as usize)
            };
            (
                (width - rect.0) / 2, // x
                (height - rect.1) / 2, // y
                rect.0, // width
                rect.1 // height
            )
        } else {
            (0, 0, width, height)
        }
    }

    pub fn get_project_path(file_path: &str) -> Option<String> {
        let mut project_path = std::path::Path::new(file_path).with_extension("gyroflow");
        if !project_path.exists() {
            // Find first project path that begins with the file name
            if let Some(parent) = project_path.parent() {
                if let Ok(paths) = std::fs::read_dir(parent) {
                    if let Some(fname) = project_path.with_extension("").file_name().map(|x| x.to_string_lossy().to_string()) {
                        for path in paths {
                            if let Ok(path) = path {
                                let path_fname = path.file_name().to_string_lossy().to_string();
                                if path_fname.starts_with(&fname) && path_fname.ends_with(".gyroflow") {
                                    project_path = path.path();
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        if project_path.exists() {
            Some(project_path.to_string_lossy().to_string())
        } else {
            None
        }
    }

    pub fn get_gyroflow_location() -> Option<String> {
        match gyroflow_core::settings::try_get("exeLocation") {
            Some(serde_json::Value::String(v)) if !v.is_empty() => {
                Some(v)
            },
            _ => {
                if cfg!(target_os = "macos") && std::path::Path::new("/Applications/GyroflowNiYien.app").exists() {
                    Some("/Applications/GyroflowNiYien.app".into())
                } else if cfg!(target_os = "macos") && std::path::Path::new("/Applications/Gyroflow.app/Contents/MacOS/gyroflow").exists() {
                    Some("/Applications/Gyroflow.app".into())
                } else {
                    None
                }
            }
        }
    }

    fn gyroflow_launch_command(location: &str, project_path: Option<&str>, target_os: &str) -> Option<GyroflowLaunchCommand> {
        if location.is_empty() {
            return None;
        }

        let project = project_path.unwrap_or_default();
        let (program, args, hide_window) = if !project.is_empty() {
            if target_os == "macos" {
                // Keep this as a file argument so LaunchServices delivers an open-file event to a running app.
                ("open", vec!["-a", location, project], false)
            } else if target_os == "windows" && location.starts_with("shell:") {
                ("cmd.exe", vec!["/c", "start", "", location, "--open", project], true)
            } else {
                (location, vec!["--open", project], false)
            }
        } else {
            if target_os == "macos" {
                ("open", vec!["-a", location], false)
            } else if target_os == "windows" && location.starts_with("shell:") {
                ("cmd.exe", vec!["/c", "start", "", location], true)
            } else {
                (location, Vec::new(), false)
            }
        };

        Some(GyroflowLaunchCommand {
            program: program.to_string(),
            args: args.into_iter().map(ToString::to_string).collect(),
            hide_window,
        })
    }

    pub fn open_gyroflow(project_path: Option<&str>) {
        if let Some(v) = Self::get_gyroflow_location() {
            if let Some(command) = Self::gyroflow_launch_command(&v, project_path, std::env::consts::OS) {
                if let Err(e) = command.spawn() {
                    rfd::MessageDialog::new()
                        .set_description(crate::i18n::tf("dialog.unable_start_gyroflow", &[("error", &format!("{e:?}"))]))
                        .show();
                }
            }
        } else {
            rfd::MessageDialog::new()
                .set_description(t!("dialog.gyroflow_not_found"))
                .show();
        }
    }

    pub fn get_param_definitions() -> [ParameterType; 13] {
        [
            ParameterType::HiddenString { id: "InstanceId" },
            ParameterType::HiddenString { id: "ProjectPath" },
            ParameterType::HiddenString { id: "ProjectData" },
            ParameterType::HiddenString { id: "EmbeddedLensProfile" },
            ParameterType::HiddenString { id: "EmbeddedPreset" },
            ParameterType::Group { id: "ProjectGroup", label: t!("group.project"), opened: true, hidden: false, parameters: vec![
                ParameterType::Text    { id: "Status",            label: t!("label.status"),            hint: t!("hint.status"),                  hidden: false },
                ParameterType::Text    { id: "LoadedProject",     label: t!("label.loaded_project"),    hint: t!("hint.loaded_project"),          hidden: false },
                ParameterType::Button  { id: "LoadCurrent",       label: t!("label.load_current"),      hint: t!("hint.load_current"),            hidden: false },
                ParameterType::Button  { id: "Browse",            label: t!("label.browse"),            hint: t!("hint.browse"),                  hidden: false },
                ParameterType::Button  { id: "LoadLens",          label: t!("label.load_lens"),         hint: t!("hint.load_lens"),               hidden: true },
                ParameterType::Button  { id: "OpenGyroflow",      label: t!("label.open_gyroflow"),     hint: t!("hint.open_gyroflow"),           hidden: false },
                ParameterType::Button  { id: "ReloadProject",     label: t!("label.reload_project"),    hint: t!("hint.reload_project"),          hidden: true },
                ParameterType::Button  { id: "OpenRecentProject", label: t!("label.open_recent_project"), hint: t!("hint.open_recent_project"),   hidden: true },
            ] },
            ParameterType::Group { id: "AdjustGroup", label: t!("group.adjust"), opened: true, hidden: false, parameters: vec![
                ParameterType::Slider   { id: "Smoothness",             label: t!("label.smoothness"),               hint: t!("hint.smoothness"),                   min: 1.0,    max: 300.0, default: 15.0,    hidden: false },
                ParameterType::Slider   { id: "ZoomLimit",              label: t!("label.zoom_limit"),               hint: t!("hint.zoom_limit"),                   min: 51.0,   max: 300.0, default: 130.0,   hidden: true },
                ParameterType::Slider   { id: "LensCorrectionStrength", label: t!("label.lens_correction_strength"), hint: t!("hint.lens_correction_strength"),     min: 0.0,    max: 100.0, default: 100.0,   hidden: false },
                ParameterType::Slider   { id: "HorizonLockAmount",      label: t!("label.horizon_lock_amount"),      hint: t!("hint.horizon_lock_amount"),          min: 0.0,    max: 100.0, default: 0.0,     hidden: false },
                ParameterType::Slider   { id: "HorizonLockRoll",        label: t!("label.horizon_lock_roll"),        hint: t!("hint.horizon_lock_roll"),            min: -100.0, max: 100.0, default: 0.0,     hidden: true },
                ParameterType::Slider   { id: "AdditionalPitch",        label: t!("label.additional_pitch"),         hint: t!("hint.additional_pitch"),             min: -180.0, max: 180.0, default: 0.0,     hidden: true },
                ParameterType::Slider   { id: "AdditionalYaw",          label: t!("label.additional_yaw"),           hint: t!("hint.additional_yaw"),               min: -180.0, max: 180.0, default: 0.0,     hidden: true },
                ParameterType::Slider   { id: "Rotation",               label: t!("label.rotation"),                 hint: t!("hint.rotation"),                     min: -360.0, max: 360.0, default: 0.0,     hidden: true },
                // The rotation the host applied to the clip before it reached the effect (e.g. DaVinci Resolve
                // "Clip Attributes -> Rotate"). A 4-choice dropdown mirroring Resolve's options; the index maps
                // to degrees via `input_rotation_{deg_from_index,index_from_deg}`. Defaulted from the loaded
                // project's video_rotation in `stab_manager`.
                ParameterType::Select   { id: "InputRotation",          label: t!("label.input_rotation"),           hint: t!("hint.input_rotation"),
                    options: vec![ "0°", "90° left", "90° right", "180°" ],
                    default: "0°", hidden: false },
                ParameterType::Slider   { id: "Fov",                    label: t!("label.fov"),                      hint: t!("hint.fov"),                          min: 0.1,    max: 3.0,   default: 1.0,     hidden: true },
                ParameterType::Slider   { id: "VideoSpeed",             label: t!("label.video_speed"),              hint: t!("hint.video_speed"),                  min: 0.0001, max: 1000.0, default: 100.0,  hidden: true },
                ParameterType::Checkbox { id: "DisableStretch",         label: t!("label.disable_stretch"),          hint: t!("hint.disable_stretch"),              default: false, hidden: true },
                ParameterType::Select   { id: "IntegrationMethod",      label: t!("label.integration_method"),       hint: t!("hint.integration_method"),
                    options: vec![
                        t!("option.integration_none"),
                        t!("option.integration_complementary"),
                        "VQF",
                        t!("option.integration_simple_gyro"),
                        t!("option.integration_simple_gyro_accel"),
                        "Mahony",
                        "Madgwick",
                    ],
                    default: "VQF", hidden: true },
                ParameterType::Select   { id: "ZoomMode",               label: t!("label.zoom_mode"),                hint: t!("hint.zoom_mode"),
                    options: vec![
                        t!("option.zoom_mode_none"),
                        t!("option.zoom_mode_dynamic"),
                        t!("option.zoom_mode_static"),
                    ],
                    default: t!("option.zoom_mode_dynamic"), hidden: false },
                ParameterType::Checkbox { id: "ToggleOverview",         label: t!("label.toggle_overview"),          hint: t!("hint.toggle_overview"),              default: false, hidden: false },
            ] },
            ParameterType::Group { id: "KeyframesGroup", label: t!("group.keyframes"), opened: false, hidden: true, parameters: vec![
                ParameterType::Checkbox { id: "UseGyroflowsKeyframes",   label: t!("label.use_gyroflows_keyframes"),   hint: t!("hint.use_gyroflows_keyframes"),   default: false, hidden: true },
                ParameterType::Checkbox { id: "StabilizationSpeedRamp",  label: t!("label.stabilization_speed_ramp"),  hint: t!("hint.stabilization_speed_ramp"),  default: true,  hidden: true },
                ParameterType::Button   { id: "RecalculateKeyframes",    label: t!("label.recalculate_keyframes"),     hint: t!("hint.recalculate_keyframes"),       hidden: true },
                ParameterType::Button   { id: "CreateCamera",            label: t!("label.create_camera"),             hint: t!("hint.create_camera"),               hidden: true },
            ] },
            // openfx-output-adjust-affine: collapsed by default, identity defaults.
            // OpenFX render path reads these; Adobe/frei0r ignore them.
            // openfx-output-adjust-flip (2026-05-22): zoom range tightened from [0.5, 2.0]
            // to [1.0, 4.0] to sidestep the sample-out-of-bounds jitter that exposed itself
            // when zoom < 1.0. Two Boolean flip toggles added after the four sliders.
            ParameterType::Group { id: "OutputAdjustGroup", label: t!("group.output_adjust"), opened: false, hidden: false, parameters: vec![
                ParameterType::Slider  { id: "OutputZoom",     label: t!("label.output_zoom"),     hint: t!("hint.output_zoom"),     min:  1.0, max:  4.0, default: 1.0, hidden: false },
                ParameterType::Slider  { id: "OutputRotation", label: t!("label.output_rotation"), hint: t!("hint.output_rotation"), min: -10.0, max: 10.0, default: 0.0, hidden: false },
                ParameterType::Slider  { id: "OutputOffsetX",  label: t!("label.output_offset_x"), hint: t!("hint.output_offset_x"), min: -50.0, max: 50.0, default: 0.0, hidden: false },
                ParameterType::Slider  { id: "OutputOffsetY",  label: t!("label.output_offset_y"), hint: t!("hint.output_offset_y"), min: -50.0, max: 50.0, default: 0.0, hidden: false },
                ParameterType::Checkbox { id: "FlipHorizontal", label: t!("label.flip_horizontal"), hint: t!("hint.flip_horizontal"), default: false, hidden: false },
                ParameterType::Checkbox { id: "FlipVertical",   label: t!("label.flip_vertical"),   hint: t!("hint.flip_vertical"),   default: false, hidden: false },
            ] },
            ParameterType::Group { id: "OutputSizeGroup", label: t!("group.output_size"), opened: false, hidden: true, parameters: vec![
                ParameterType::Slider   { id: "OutputWidth",          label: t!("label.output_width"),           hint: t!("hint.output_width"),           min: 1.0, max: 16384.0, default: 3840.0, hidden: true },
                ParameterType::Slider   { id: "OutputHeight",         label: t!("label.output_height"),          hint: t!("hint.output_height"),          min: 1.0, max: 16384.0, default: 2160.0, hidden: true },
                ParameterType::Button   { id: "OutputSizeToTimeline", label: t!("label.output_size_to_timeline"), hint: t!("hint.output_size_to_timeline"), hidden: true },
                ParameterType::Button   { id: "OutputSizeSwap",       label: t!("label.output_size_swap"),       hint: t!("hint.output_size_swap"),       hidden: true },
                ParameterType::Select   { id: "Interpolation",        label: t!("label.interpolation"),          hint: t!("hint.interpolation"),
                    options: vec!["Lanczos4", "RobidouxSharp", "Bilinear", "Bicubic", "Robidoux", "Mitchell", "CatmullRom"], default: "Lanczos4", hidden: true },
            ] },
            ParameterType::Checkbox { id: "DontDrawOutside",      label: t!("label.dont_draw_outside"),   hint: t!("hint.dont_draw_outside"),     default: false, hidden: true },
            ParameterType::Checkbox { id: "IncludeProjectData",   label: t!("label.include_project_data"),hint: t!("hint.include_project_data"),  default: false, hidden: true },
            ParameterType::Group { id: "InfoGroup", label: t!("group.info"), opened: true, hidden: true, parameters: vec![
                ParameterType::Text { id: "LoadedPreset",  label: t!("label.loaded_preset"),  hint: t!("hint.loaded_preset"),  hidden: true },
                ParameterType::Text { id: "LoadedLens",   label: t!("label.loaded_lens"),    hint: t!("hint.loaded_lens"),     hidden: true },
            ] },
        ]
    }
}

pub enum ParameterType {
    HiddenString { id: &'static str },
    TextBox      { id: &'static str, label: &'static str, hint: &'static str, hidden: bool },
    Text         { id: &'static str, label: &'static str, hint: &'static str, hidden: bool },
    Slider       { id: &'static str, label: &'static str, hint: &'static str, min: f64, max: f64, default: f64, hidden: bool },
    Checkbox     { id: &'static str, label: &'static str, hint: &'static str, default: bool, hidden: bool },
    Button       { id: &'static str, label: &'static str, hint: &'static str, hidden: bool },
    Group        { id: &'static str, label: &'static str, opened: bool, parameters: Vec<ParameterType>, hidden: bool },
    Select       { id: &'static str, label: &'static str, hint: &'static str, options: Vec<&'static str>, default: &'static str, hidden: bool },
}

#[derive(Debug, Clone)]
pub enum TimeType {
    Frame(f64),
    Milliseconds(f64),
    Microseconds(i64),
    FrameOrMicrosecond((Option<f64>, Option<i64>))
}
pub trait GyroflowPluginParams {
    fn set_enabled(&mut self, param: Params, enabled: bool) -> PluginResult<()>;
    fn set_label(&mut self, param: Params, label: &str) -> PluginResult<()>;
    fn set_hint(&mut self, param: Params, hint: &str) -> PluginResult<()>;

    fn set_f64(&mut self, param: Params, value: f64) -> PluginResult<()>;
    fn get_f64(&self, param: Params) -> PluginResult<f64>;
    fn get_f64_at_time(&self, param: Params, time: TimeType) -> PluginResult<f64>;
    fn set_bool(&mut self, param: Params, value: bool) -> PluginResult<()>;
    fn get_bool(&self, param: Params) -> PluginResult<bool>;
    fn get_bool_at_time(&self, param: Params, time: TimeType) -> PluginResult<bool>;
    fn set_string(&mut self, param: Params, value: &str) -> PluginResult<()>;
    fn get_string(&self, param: Params) -> PluginResult<String>;
    fn set_i32(&mut self, param: Params, value: i32) -> PluginResult<()>;
    fn get_i32(&self, param: Params) -> PluginResult<i32>;

    fn is_keyframed(&self, param: Params) -> bool;
    fn get_keyframes(&self, param: Params) -> Vec<(TimeType, f64)>;
    fn clear_keyframes(&mut self, param: Params) -> PluginResult<()>;
    fn set_f64_at_time(&mut self, param: Params, time: TimeType, value: f64) -> PluginResult<()>;
}

#[derive(Default, Clone)]
pub struct KeyframableParams {
    pub use_gyroflows_keyframes: bool,
    pub cached_keyframes: KeyframeManager
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GyroflowPluginBaseInstance {
    #[serde(skip)]
    pub keyframable_params: Arc<RwLock<KeyframableParams>>,

    #[serde(skip)]
    pub managers: LruCache<String, Arc<StabilizationManager>>,

    pub reload_values_from_project: bool,

    pub original_video_size: (usize, usize),
    pub original_output_size: (usize, usize),
    pub timeline_size: (usize, usize),
    pub num_frames: usize,
    pub fps: f64,
    pub has_motion: bool,
    pub ever_changed: bool,
    pub cache_keyframes_every_frame: bool,
    pub framebuffer_inverted: bool,
    pub anamorphic_adjust_size: bool,
    pub always_set_input_rotation: bool,
    pub auto_disable_stretch: bool,

    pub opencl_disabled: bool,

    // Gate for the load-time InputRotation step in `stab_manager` (openfx-restore-rotation-order).
    // Only the OpenFX plugin enables this (per call, Edit/Color pages only); Adobe and frei0r
    // never set it, keeping their load path byte-identical to before the step existed.
    // Not persisted: the owning wrapper re-decides it on every `stab_manager` call.
    #[serde(skip)]
    pub apply_input_rotation_on_load: bool,

    // The project's rotation as imported (`stab.params.video_rotation` right after import,
    // before any mutation or runtime InputRotation override touches it). Re-captured on every
    // cache-miss rebuild; single source of truth for "InputRotation = 0°" semantics. Not
    // persisted by design — it must always be re-derived from the project itself (spec:
    // "Original project rotation survives restore").
    #[serde(skip)]
    pub original_project_rotation: Option<f64>,
}
impl Clone for GyroflowPluginBaseInstance {
    fn clone(&self) -> Self {
        Self {
            managers:                       self.managers.clone(),
            original_output_size:           self.original_output_size,
            original_video_size:            self.original_video_size,
            timeline_size:                  self.timeline_size,
            num_frames:                     self.num_frames,
            fps:                            self.fps,
            has_motion:                     self.has_motion,
            reload_values_from_project:     self.reload_values_from_project,
            ever_changed:                   self.ever_changed,
            opencl_disabled:                self.opencl_disabled,
            cache_keyframes_every_frame:    self.cache_keyframes_every_frame,
            framebuffer_inverted:           self.framebuffer_inverted,
            anamorphic_adjust_size:         self.anamorphic_adjust_size,
            always_set_input_rotation:      self.always_set_input_rotation,
            auto_disable_stretch:           self.auto_disable_stretch,
            apply_input_rotation_on_load:   self.apply_input_rotation_on_load,
            original_project_rotation:      self.original_project_rotation,
            keyframable_params:             Arc::new(RwLock::new(self.keyframable_params.read().clone())),
        }
    }
}
impl Default for GyroflowPluginBaseInstance {
    fn default() -> Self {
        Self {
            managers:                       LruCache::new(std::num::NonZeroUsize::new(20).unwrap()),
            original_output_size:           (0, 0),
            original_video_size:            (0, 0),
            timeline_size:                  (0, 0),
            num_frames:                     0,
            fps:                            0.0,
            has_motion:                     false,
            reload_values_from_project:     true,
            ever_changed:                   false,
            opencl_disabled:                false,
            cache_keyframes_every_frame:    true,
            framebuffer_inverted:           false,
            anamorphic_adjust_size:         true,
            always_set_input_rotation:      false,
            auto_disable_stretch:           true,
            apply_input_rotation_on_load:   false,
            original_project_rotation:      None,
            keyframable_params: Arc::new(RwLock::new(KeyframableParams {
                use_gyroflows_keyframes:  false, // TODO param_set.parameter::<Bool>("UseGyroflowsKeyframes")?.get_value()?,
                cached_keyframes:         KeyframeManager::default()
            })),
        }
    }
}

/// Map ZoomMode dropdown index to gyroflow_core's `adaptive_zoom_window`:
///   0 = No zoom    -> 0.0
///   1 = Dynamic    -> 4.0   (gyroflow default, smoothing window in seconds)
///   2 = Static     -> -1.0  (sentinel for static crop in core)
pub fn zoom_window_from_mode_index(idx: i32) -> f64 {
    match idx {
        0 => 0.0,
        2 => -1.0,
        _ => 4.0,
    }
}
/// Inverse mapping: derive the dropdown index from a project's `adaptive_zoom_window`.
pub fn mode_index_from_zoom_window(window: f64) -> i32 {
    if window <= -0.9 { 2 }
    else if window < 0.0001 { 0 }
    else { 1 }
}

/// `InputRotation` is a 4-choice dropdown matching DaVinci Resolve's Clip-Attributes "Rotate" options:
///   0 = 0°        -> 0
///   1 = 90° left  -> 90
///   2 = 90° right -> -90  (== 270°)
///   3 = 180°      -> 180
pub fn input_rotation_deg_from_index(index: i32) -> f64 {
    match index {
        1 => 90.0,
        2 => -90.0,
        3 => 180.0,
        _ => 0.0,
    }
}
/// Inverse mapping: derive the dropdown index from a rotation in degrees (accepts 0/90/180/270 and
/// their negatives; anything that isn't a quarter turn falls back to 0°).
pub fn input_rotation_index_from_deg(deg: f64) -> i32 {
    match (((deg % 360.0) + 360.0) % 360.0).round() as i64 {
        90 => 1,
        270 => 2,
        180 => 3,
        _ => 0,
    }
}

// ------------------------------------------------------------------------------------------------
// InputRotation runtime-override geometry (openfx-restore-rotation-order).
// Hoisted from openfx/src/gyroflow.rs so the gated load-time rotation step in `stab_manager`
// and the OpenFX render-path override share one implementation (no logic fork). These are pure
// functions plus one in-place stab mutation; invalidation/recompute policy stays with the caller
// (the load path relies on the §11.4 snapshot diff, the OpenFX live-edit path invalidates
// zooming/undistortion explicitly).
// ------------------------------------------------------------------------------------------------

/// Normalize an arbitrary rotation in degrees to a quarter-turn bucket in `[0, 360)`.
pub fn normalized_quarter_turn_deg(deg: f64) -> i32 {
    let rounded = deg.round() as i32;
    ((rounded % 360) + 360) % 360
}

/// `true` when the rotation swaps width/height (90° or 270°).
pub fn is_sideways_rotation(deg: f64) -> bool {
    matches!(normalized_quarter_turn_deg(deg), 90 | 270)
}

/// Resolve the effective `video_rotation` implied by the `InputRotation` dropdown against the
/// project's original rotation. Returns `None` when the target already matches the stab's
/// current rotation (idempotence early-out), `Some(target)` when a change must be applied.
pub fn input_rotation_target_rotation(
    project_rotation: f64,
    current_video_rotation: f64,
    input_rotation_index: i32,
) -> Option<f64> {
    let input_rotation = input_rotation_deg_from_index(input_rotation_index);
    let target_rotation = if normalized_quarter_turn_deg(input_rotation) == 0 {
        project_rotation
    } else {
        input_rotation
    };

    if normalized_quarter_turn_deg(target_rotation) == normalized_quarter_turn_deg(current_video_rotation) {
        None
    } else {
        Some(target_rotation)
    }
}

/// Transpose the requested output size when the target rotation changes sideways parity
/// relative to the project rotation.
pub fn input_rotation_output_size(project_rotation: f64, target_rotation: f64, output_width: usize, output_height: usize) -> (usize, usize) {
    if is_sideways_rotation(project_rotation) != is_sideways_rotation(target_rotation) {
        (output_height, output_width)
    } else {
        (output_width, output_height)
    }
}

/// Apply the InputRotation-implied `video_rotation` + `output_size` transpose to a stab manager
/// in place. Returns the effective rotation when a change was applied, `None` when the target
/// already matches (no mutation). Performs NO invalidation or recompute — callers decide:
/// the load-time step lets the post-mutation snapshot diff fire the single recompute, while the
/// OpenFX render/live-edit wrapper invalidates zooming/undistortion itself.
/// Note the order: `video_rotation` is written first so `set_output_size`'s aspect constraint
/// (`constrained_output_size` in gyroflow-core) sees the rotated input orientation.
pub fn apply_input_rotation_to_stab(
    project_rotation: f64,
    input_rotation_index: i32,
    output_size: (usize, usize),
    stab: &StabilizationManager,
) -> Option<f64> {
    let current_video_rotation = stab.params.read().video_rotation;
    let target_rotation = input_rotation_target_rotation(project_rotation, current_video_rotation, input_rotation_index)?;
    {
        let mut stab_params = stab.params.write();
        stab_params.video_rotation = target_rotation;
    }
    let output_size = input_rotation_output_size(project_rotation, target_rotation, output_size.0, output_size.1);
    stab.set_output_size(output_size.0, output_size.1);

    Some(target_rotation)
}

impl GyroflowPluginBaseInstance {
    pub fn update_loaded_state(&mut self, params: &mut dyn GyroflowPluginParams, loaded: bool) {
        let _ = params.set_enabled(Params::Fov, loaded);
        let _ = params.set_enabled(Params::Smoothness, loaded);
        let _ = params.set_enabled(Params::ZoomLimit, loaded);
        let _ = params.set_enabled(Params::LensCorrectionStrength, loaded);
        let _ = params.set_enabled(Params::HorizonLockAmount, loaded);
        let _ = params.set_enabled(Params::HorizonLockRoll, loaded);
        //let _ = params.set_enabled(Params::PositionX, loaded);
        //let _ = params.set_enabled(Params::PositionY, loaded);
        let _ = params.set_enabled(Params::AdditionalPitch, loaded);
        let _ = params.set_enabled(Params::AdditionalYaw, loaded);
        let _ = params.set_enabled(Params::Rotation, loaded);
        let _ = params.set_enabled(Params::VideoSpeed, loaded);
        let _ = params.set_enabled(Params::DisableStretch, loaded);
        let _ = params.set_enabled(Params::IntegrationMethod, loaded);
        let _ = params.set_enabled(Params::ZoomMode, loaded);
        let _ = params.set_enabled(Params::ToggleOverview, loaded);
        let _ = params.set_enabled(Params::ReloadProject, loaded);
        let _ = params.set_enabled(Params::OutputWidth, loaded);
        let _ = params.set_enabled(Params::OutputHeight, loaded);
        let _ = params.set_enabled(Params::OutputSizeToTimeline, loaded);
        let _ = params.set_enabled(Params::OutputSizeSwap, loaded);
        let _ = params.set_string(Params::Status, if loaded { t!("status.ok") } else { t!("status.project_not_loaded") });
        let _ = params.set_label(Params::OpenGyroflow, if loaded { t!("label.open_gyroflow_loaded") } else { t!("label.open_gyroflow") });
    }

    pub fn initialize_instance_id(&mut self, instance_id: &mut String) {
        if instance_id.is_empty() {
            self.ever_changed = true;
            self.reload_values_from_project = true;
            *instance_id = format!("{}", fastrand::u64(..));
        }
    }

    pub fn set_keyframe_provider(&self, stab: &StabilizationManager) {
        let kparams = self.keyframable_params.clone();
        stab.keyframes.write().set_custom_provider(move |kf, typ, timestamp_ms| -> Option<f64> {
            let params = kparams.read();
            if params.use_gyroflows_keyframes && kf.is_keyframed_internally(typ) { return None; }
            params.cached_keyframes.value_at_video_timestamp(typ, timestamp_ms)
        });
    }
    pub fn cache_keyframes(&mut self, params: &dyn GyroflowPluginParams, use_gyroflows_keyframes: bool, num_frames: usize, fps: f64) {
        let mut mgr = KeyframeManager::new();
        macro_rules! cache_key {
            ($typ:expr, $param:expr, $scale:expr) => {
                if params.is_keyframed($param) {
                    log::info!("param: {:?} is keyframed, cache_keyframes_every_frame: {}", $param, self.cache_keyframes_every_frame);
                    if self.cache_keyframes_every_frame { // Query every frame
                        for t in 0..num_frames {
                            let time = t as f64;
                            let timestamp_us = ((time / fps * 1_000_000.0)).round() as i64;

                            if let Ok(v) = params.get_f64_at_time($param, TimeType::FrameOrMicrosecond((Some(time), Some(timestamp_us)))) {
                                mgr.set(&$typ, timestamp_us, v / $scale);
                            }
                        }
                    } else {
                        // Cache only the keyframes at their timestamps
                        for (t, v) in params.get_keyframes($param) {
                            let timestamp_us = match t {
                                TimeType::FrameOrMicrosecond((Some(f), None)) |
                                TimeType::Frame(f) => ((f / fps * 1_000_000.0)).round() as i64,
                                TimeType::Milliseconds(ms) => (ms * 1_000.0).round() as i64,
                                TimeType::Microseconds(us) => us,
                                TimeType::FrameOrMicrosecond((_,    Some(timestamp_us))) => timestamp_us,
                                TimeType::FrameOrMicrosecond((None, None)) => unreachable!(),
                            };

                            mgr.set(&$typ, timestamp_us, v / $scale);
                        }
                    }
                } else {
                    log::info!("param: {:?} NOT keyframed", $param);
                    if let Ok(v) = params.get_f64($param) {
                        mgr.set(&$typ, 0, v / $scale);
                    }
                }
            };
        }
        cache_key!(KeyframeType::Fov,                       Params::Fov,                    1.0);
        cache_key!(KeyframeType::MaxZoom,                   Params::ZoomLimit,              1.0);
        cache_key!(KeyframeType::SmoothingParamSmoothness,  Params::Smoothness,             100.0);
        cache_key!(KeyframeType::LensCorrectionStrength,    Params::LensCorrectionStrength, 100.0);
        cache_key!(KeyframeType::LockHorizonAmount,         Params::HorizonLockAmount,      1.0);
        cache_key!(KeyframeType::LockHorizonRoll,           Params::HorizonLockRoll,        1.0);
        cache_key!(KeyframeType::VideoSpeed,                Params::VideoSpeed,             100.0);
        cache_key!(KeyframeType::VideoRotation,             Params::Rotation,               1.0);
        //cache_key!(KeyframeType::ZoomingCenterX,            Params::PositionX,              100.0);
        //cache_key!(KeyframeType::ZoomingCenterY,            Params::PositionY,              100.0);
        cache_key!(KeyframeType::AdditionalRotationX,       Params::AdditionalYaw,          1.0);
        cache_key!(KeyframeType::AdditionalRotationY,       Params::AdditionalPitch,        1.0);

        let mut kparams = self.keyframable_params.write();
        kparams.use_gyroflows_keyframes = use_gyroflows_keyframes;
        kparams.cached_keyframes = mgr;
    }

    fn maybe_auto_disable_stretch_for_lens(
        &self,
        params: &mut dyn GyroflowPluginParams,
        disable_stretch: &mut bool,
        input_horizontal_stretch: f64,
        input_vertical_stretch: f64,
    ) -> PluginResult<()> {
        if !self.auto_disable_stretch || *disable_stretch {
            return Ok(());
        }

        let lens_has_stretch =
            (input_horizontal_stretch > 0.01 && (input_horizontal_stretch - 1.0).abs() > 1e-6)
            || (input_vertical_stretch > 0.01 && (input_vertical_stretch - 1.0).abs() > 1e-6);
        if lens_has_stretch {
            params.set_bool(Params::DisableStretch, true)?;
            *disable_stretch = true;
        }
        Ok(())
    }

    fn maybe_auto_disable_stretch_from_embedded_data(
        &self,
        params: &mut dyn GyroflowPluginParams,
        disable_stretch: &mut bool,
    ) -> PluginResult<()> {
        if !self.auto_disable_stretch || *disable_stretch {
            return Ok(());
        }

        for param_id in [Params::EmbeddedLensProfile, Params::EmbeddedPreset, Params::ProjectData] {
            if let Ok(d) = params.get_string(param_id) {
                if !d.is_empty() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&d) {
                        if v.get("plugin_disable_stretch").and_then(|v| v.as_bool()).unwrap_or(false) {
                            *disable_stretch = true;
                            let _ = params.set_bool(Params::DisableStretch, true);
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    // Gated load-time InputRotation step (openfx-restore-rotation-order D1). Called from the
    // cache-miss mutation block in `stab_manager`. Returns the effective rotation when a
    // rotation + output-size transpose was applied to the stab, `None` when the step is
    // disabled (flag unset — Adobe/frei0r/Fusion), no project rotation was captured yet, or
    // the InputRotation target already matches the stab's current rotation (no mutation).
    // Reads the same param inputs as the OpenFX render-path override
    // (`InputRotation` + `OutputWidth`/`OutputHeight`), so that override's
    // target-== -current early-out makes it a no-op on freshly rebuilt stabs.
    pub fn maybe_apply_input_rotation_on_load(&self, params: &dyn GyroflowPluginParams, stab: &StabilizationManager) -> Option<f64> {
        if !self.apply_input_rotation_on_load {
            return None;
        }
        let project_rotation = self.original_project_rotation?;
        let input_rotation_index = params.get_i32(Params::InputRotation).ok()?;
        let output_size = (
            params.get_f64(Params::OutputWidth).ok()? as usize,
            params.get_f64(Params::OutputHeight).ok()? as usize,
        );
        apply_input_rotation_to_stab(project_rotation, input_rotation_index, output_size, stab)
    }

    pub fn stab_manager(&mut self, params: &mut dyn GyroflowPluginParams, manager_cache: &Mutex<LruCache<String, Arc<StabilizationManager>>>, out_size: (usize, usize), open_gyroflow_if_no_data: bool) -> PluginResult<Arc<StabilizationManager>> {
        let mut disable_stretch = params.get_bool(Params::DisableStretch)?;

        let instance_id = params.get_string(Params::InstanceId)?;
        let path = params.get_string(Params::ProjectPath)?;
        if path.is_empty() {
            self.update_loaded_state(params, false);
            return Err("Path is empty".into());
        }

        if self.timeline_size == (0, 0) {
            self.timeline_size = out_size;
        }

        let key = format!("{path}{disable_stretch}{instance_id}");
        let cloned = manager_cache.lock().get(&key).map(Arc::clone);
        let stab = if let Some(stab) = cloned {
            // Cache it in this instance as well
            if !self.managers.contains(&key) {
                self.managers.put(key.to_owned(), stab.clone());
            }
            self.set_keyframe_provider(&stab);
            stab
        } else {
            log::info!("new stab manager for key: {key}");
            let mut stab = StabilizationManager::default();
            {
                // Find first lens profile database with loaded profiles
                let lock = manager_cache.lock();
                for (_, v) in lock.iter() {
                    if v.lens_profile_db.read().loaded {
                        stab.lens_profile_db = v.lens_profile_db.clone();
                        break;
                    }
                }
            }
            {
                let mut stab = stab.stabilization.write();
                stab.share_wgpu_instances = true;
                stab.interpolation = match params.get_i32(Params::Interpolation) {
                    Ok(1) => gyroflow_core::stabilization::Interpolation::RobidouxSharp,
                    Ok(2) => gyroflow_core::stabilization::Interpolation::Bilinear,
                    Ok(3) => gyroflow_core::stabilization::Interpolation::Bicubic,
                    Ok(4) => gyroflow_core::stabilization::Interpolation::Robidoux,
                    Ok(5) => gyroflow_core::stabilization::Interpolation::Mitchell,
                    Ok(6) => gyroflow_core::stabilization::Interpolation::CatmullRom,
                    _     => gyroflow_core::stabilization::Interpolation::Lanczos4,
                };
                log::info!("Interpolation: {:?}", stab.interpolation);
            }

            if !path.ends_with(".gyroflow") {
                // §11.7: short-circuit the retry storm before invoking open_file.
                if let Some(cached_msg) = check_not_found_cache(&path) {
                    return Err(format!("open_file (cached NotFound, retry throttled): {cached_msg}").into());
                }
                let url = filesystem::path_to_url(&path);
                let mut file = match filesystem::open_file(&url, false, false) {
                    Ok(f) => {
                        clear_not_found(&path);
                        f
                    }
                    Err(e) => {
                        let is_not_found = matches!(
                            &e,
                            gyroflow_core::filesystem::FilesystemError::IOError(io)
                                if io.kind() == std::io::ErrorKind::NotFound,
                        );
                        if is_not_found {
                            record_not_found(&path, format!("{e}"));
                        }
                        return Err(e.into());
                    }
                };
                let filesize = file.size;
                match stab.load_video_file(file.get_file(), filesize, &url, None, true) {
                    Ok(md) => {
                        if out_size != (0, 0) {
                            stab.params.write().output_size = out_size; // Default to timeline output size
                        }
                        if let Some(preset_out_size) = stab.input_file.read().preset_output_size {
                            stab.params.write().output_size = preset_out_size;
                        }

                        if let Ok(d) = params.get_string(Params::EmbeddedLensProfile) {
                            if !d.is_empty() {
                                if let Err(e) = stab.load_lens_profile(&d) {
                                    rfd::MessageDialog::new()
                                        .set_description(crate::i18n::tf("dialog.failed_load_lens", &[("error", &format!("{e:?}"))]))
                                        .show();
                                }
                            }
                        }
                        if let Ok(d) = params.get_string(Params::EmbeddedPreset) {
                            if !d.is_empty() {
                                let mut is_preset = false;
                                if let Err(e) = stab.import_gyroflow_data(d.as_bytes(), true, None, |_|(), Arc::new(AtomicBool::new(false)), &mut is_preset, true) {
                                    rfd::MessageDialog::new()
                                        .set_description(crate::i18n::tf("dialog.failed_load_preset", &[("error", &format!("{e:?}"))]))
                                        .show();
                                }
                            }
                        }
                        if params.get_bool(Params::IncludeProjectData)? {
                            if let Ok(data) = stab.export_gyroflow_data(gyroflow_core::GyroflowProjectType::WithGyroData, "{}", None) {
                                let data = GyroflowPluginBase::maybe_inject_disable_stretch(&data, disable_stretch);
                                params.set_string(Params::ProjectData, &data)?;
                            }
                        }
                        if md.rotation != 0 && self.reload_values_from_project {
                            let r = ((360 - md.rotation) % 360) as f64;
                            params.set_i32(Params::InputRotation, input_rotation_index_from_deg(r))?;
                            stab.params.write().video_rotation = r;
                        }
                        params.set_string(Params::LoadedProject, &filesystem::get_filename(&filesystem::path_to_url(&path)))?;
                        if !stab.gyro.read().file_metadata.read().has_accurate_timestamps && open_gyroflow_if_no_data {
                            GyroflowPluginBase::open_gyroflow(params.get_string(Params::ProjectPath).ok().as_deref());
                        }
                    },
                    Err(e) => {
                        let embedded_data = params.get_string(Params::ProjectData)?;
                        if !embedded_data.is_empty() {
                            let mut is_preset = false;
                            stab.import_gyroflow_data(embedded_data.as_bytes(), true, None, |_|(), Arc::new(AtomicBool::new(false)), &mut is_preset, true).map_err(|e| {
                                self.update_loaded_state(params, false);
                                format!("load_gyro_data error: {e}")
                            })?;
                        } else {
                            log::error!("An error occured: {e:?}");
                            self.update_loaded_state(params, false);
                            params.set_string(Params::Status, t!("status.failed_load_info"))?;
                            params.set_hint(Params::Status, &crate::i18n::tf("status.error_loading", &[("path", &path), ("error", &format!("{e:?}"))]))?;
                            if open_gyroflow_if_no_data {
                                GyroflowPluginBase::open_gyroflow(params.get_string(Params::ProjectPath).ok().as_deref());
                            }
                            return Err(e.into());
                        }
                    }
                }
            } else {
                let project_data = {
                    if params.get_bool(Params::IncludeProjectData)? && !params.get_string(Params::ProjectData)?.is_empty() {
                        params.get_string(Params::ProjectData)?
                    } else if let Ok(data) = std::fs::read_to_string(&path) {
                        if params.get_bool(Params::IncludeProjectData)? {
                            params.set_string(Params::ProjectData, &data)?;
                        } else {
                            params.set_string(Params::ProjectData, "")?;
                        }
                        data
                    } else {
                        "".to_string()
                    }
                };
                let mut is_preset = false;
                stab.import_gyroflow_data(project_data.as_bytes(), true, Some(&filesystem::path_to_url(&path)), |_|(), Arc::new(AtomicBool::new(false)), &mut is_preset, true).map_err(|e| {
                    self.update_loaded_state(params, false);
                    format!("load_gyro_data error: {e}")
                })?;
                params.set_string(Params::LoadedProject, &filesystem::get_filename(&filesystem::path_to_url(&path)))?;

                if self.always_set_input_rotation {
                    let url = stab.input_file.read().url.clone();
                    let mut file = filesystem::open_file(&url, false, false)?;
                    let filesize = file.size;
                    if let Ok(video_md) = gyroflow_core::util::get_video_metadata(file.get_file(), filesize, &url) {
                        if video_md.rotation != 0 && self.reload_values_from_project {
                            let r = ((360 - video_md.rotation) % 360) as f64;
                            params.set_i32(Params::InputRotation, input_rotation_index_from_deg(r))?;
                            stab.params.write().video_rotation = r;
                        }
                    }
                }
            }

            let loaded = {
                stab.params.write().calculate_ramped_timestamps(&stab.keyframes.read(), false, true);
                let gf_params = stab.params.read();
                self.original_video_size = gf_params.size;
                self.original_output_size = gf_params.output_size;
                // Capture the project's rotation as imported, before the mutation block below or
                // any runtime InputRotation override can touch it. Re-captured on every cache-miss
                // rebuild — never read back from a parameter the override writes (design D2).
                self.original_project_rotation = Some(gf_params.video_rotation);
                self.num_frames = gf_params.frame_count;
                self.fps = gf_params.fps;
                let loaded = gf_params.duration_ms > 0.0;
                if loaded && self.reload_values_from_project {
                    self.reload_values_from_project = false;
                    let smooth = stab.smoothing.read();
                    let smoothness = smooth.current().get_parameter("smoothness");
                    params.set_f64(Params::Fov,                    gf_params.fov)?;
                    params.set_f64(Params::Smoothness,             smoothness * 100.0)?;
                    params.set_f64(Params::ZoomLimit,              gf_params.max_zoom.unwrap_or(0.0))?;
                    params.set_f64(Params::LensCorrectionStrength, (gf_params.lens_correction_amount * 100.0).min(100.0))?;
                    params.set_f64(Params::HorizonLockAmount,      if smooth.horizon_lock.lock_enabled { smooth.horizon_lock.horizonlockpercent } else { 0.0 })?;
                    params.set_f64(Params::HorizonLockRoll,        if smooth.horizon_lock.lock_enabled { smooth.horizon_lock.horizonroll } else { 0.0 })?;
                    params.set_f64(Params::VideoSpeed,             gf_params.video_speed * 100.0)?;
                    //params.set_f64(Params::PositionX,              gf_params.adaptive_zoom_center_offset.0 * 100.0)?;
                    //params.set_f64(Params::PositionY,              gf_params.adaptive_zoom_center_offset.1 * 100.0)?;
                    params.set_f64(Params::AdditionalYaw,          gf_params.additional_rotation.0)?;
                    params.set_f64(Params::AdditionalPitch,        gf_params.additional_rotation.1)?;
                    params.set_f64(Params::Rotation,               gf_params.video_rotation)?;
                    // Default the host-applied clip rotation to the project's video_rotation. The host (e.g.
                    // Resolve Clip Attributes) derives its rotation from the same container metadata Gyroflow
                    // derived video_rotation from, so for clips with rotation metadata this matches. Gated by
                    // reload_values_from_project (first load / reload / new lens), so a user-set value is not
                    // clobbered on subsequent renders. The bare-video path above already sets it to the same
                    // (360 - md.rotation) % 360 value, so this is idempotent there. Stored as a dropdown index.
                    params.set_i32(Params::InputRotation,          input_rotation_index_from_deg(gf_params.video_rotation))?;
                    params.set_i32(Params::IntegrationMethod,      stab.gyro.read().integration_method as i32)?;
                    params.set_i32(Params::ZoomMode,               mode_index_from_zoom_window(gf_params.adaptive_zoom_window))?;

                    params.set_f64(Params::OutputWidth,            self.original_output_size.0 as f64)?;
                    params.set_f64(Params::OutputHeight,           self.original_output_size.1 as f64)?;

                    params.set_i32(Params::Interpolation, match stab.stabilization.read().interpolation {
                        gyroflow_core::stabilization::Interpolation::Lanczos4      => 0,
                        gyroflow_core::stabilization::Interpolation::RobidouxSharp => 1,
                        gyroflow_core::stabilization::Interpolation::Bilinear      => 2,
                        gyroflow_core::stabilization::Interpolation::Bicubic       => 3,
                        gyroflow_core::stabilization::Interpolation::Robidoux      => 4,
                        gyroflow_core::stabilization::Interpolation::Mitchell      => 5,
                        gyroflow_core::stabilization::Interpolation::CatmullRom    => 6,
                    })?;

                    let keyframes = stab.keyframes.read();
                    let all_keys = keyframes.get_all_keys();
                    params.set_bool(Params::UseGyroflowsKeyframes, !all_keys.is_empty())?;
                    if let Some(name) = stab.input_file.read().preset_name.clone() {
                        params.set_string(Params::LoadedPreset, &name)?;
                    }
                    params.set_string(Params::LoadedLens, &stab.lens.read().get_display_name())?;

                    // Auto-enable DisableStretch if the loaded lens has anamorphic stretch != 1.
                    // Gated by reload_values_from_project (true only on first load / project reload / new lens),
                    // so the user can manually un-check afterwards without it being re-applied each frame.
                    let (xs, ys) = {
                        let lens = stab.lens.read();
                        (lens.input_horizontal_stretch, lens.input_vertical_stretch)
                    };
                    self.maybe_auto_disable_stretch_for_lens(params, &mut disable_stretch, xs, ys)?;

                    for k in all_keys {
                        if let Some(keys) = keyframes.get_keyframes(k) {
                            if !keys.is_empty() {
                                macro_rules! set_keys {
                                    ($name:expr, $scale:expr) => {
                                        params.clear_keyframes($name)?;
                                        for (ts, v) in keys {
                                            let ts = if k == &KeyframeType::VideoSpeed { gf_params.get_source_timestamp_at_ramped_timestamp(*ts) } else { *ts };
                                            let time = (((ts as f64 / 1000.0) * gf_params.fps) / 1000.0).round();
                                            params.set_f64_at_time($name, TimeType::Frame(time), v.value * $scale)?;
                                        }
                                    };
                                }
                                match k {
                                    KeyframeType::Fov                      => { set_keys!(Params::Fov,                    1.0); },
                                    KeyframeType::SmoothingParamSmoothness => { set_keys!(Params::Smoothness,             100.0); },
                                    KeyframeType::MaxZoom                  => { set_keys!(Params::ZoomLimit,              1.0); },
                                    KeyframeType::LensCorrectionStrength   => { set_keys!(Params::LensCorrectionStrength, 100.0); },
                                    KeyframeType::LockHorizonAmount        => { set_keys!(Params::HorizonLockAmount,      1.0); },
                                    KeyframeType::LockHorizonRoll          => { set_keys!(Params::HorizonLockRoll,        1.0); },
                                    KeyframeType::VideoSpeed               => { set_keys!(Params::VideoSpeed,             100.0); },
                                    KeyframeType::VideoRotation            => { set_keys!(Params::Rotation,               1.0); },
                                    //KeyframeType::ZoomingCenterX           => { set_keys!(Params::PositionX,              100.0); },
                                    //KeyframeType::ZoomingCenterY           => { set_keys!(Params::PositionY,              100.0); },
                                    KeyframeType::AdditionalRotationX      => { set_keys!(Params::AdditionalYaw,          1.0); },
                                    KeyframeType::AdditionalRotationY      => { set_keys!(Params::AdditionalPitch,        1.0); },
                                    _ => { }
                                }
                            }
                        }
                    }
                }
                let use_gyroflows_keyframes = params.get_bool(Params::UseGyroflowsKeyframes).unwrap_or_default();
                self.cache_keyframes(params, use_gyroflows_keyframes, self.num_frames, self.fps.max(1.0));
                self.has_motion = stab.gyro.read().has_motion();
                loaded
            };

            self.update_loaded_state(params, loaded);

            // §11.3: snapshot compute-params inputs immediately after import #1
            // and before the OFX-side mutation block. Captured here (vs inside
            // the `let loaded = { ... }` scope) so the read locks acquired by
            // snapshot_compute_inputs do not nest with the gf_params guard held
            // inside that block.
            let pre_snapshot = snapshot_compute_inputs(&stab);

            // Check if loaded preset/project/lens data contains the plugin_disable_stretch flag
            self.maybe_auto_disable_stretch_from_embedded_data(params, &mut disable_stretch)?;

            if disable_stretch {
                stab.disable_lens_stretch(self.anamorphic_adjust_size);
            }

            stab.set_fov_overview(params.get_bool(Params::ToggleOverview)?);

            {
                let mut params = stab.params.write();
                params.framebuffer_inverted = self.framebuffer_inverted;
            }

            stab.init_size();
            stab.set_output_size(params.get_f64(Params::OutputWidth)? as _, params.get_f64(Params::OutputHeight)? as _);

            // Load-time InputRotation step (openfx-restore-rotation-order D1): apply the
            // InputRotation-implied video_rotation + output_size transpose before the §11.4
            // post-mutation snapshot, so the first recompute after a rebuild never observes the
            // hybrid "rotated content + untransposed output_size" state (which made the core
            // Max-Zoom limiter misclassify every frame on host restore). Must run after the
            // `set_output_size(OutputWidth/Height)` call above — that call would otherwise
            // overwrite the transposed size with the persisted landscape params — and before the
            // ZoomMode bucket-preserve step below. Gated by an instance flag that only the OpenFX
            // plugin enables (Edit/Color pages); Adobe/frei0r never set it (flag defaults false),
            // keeping their load path byte-identical. When InputRotation already matches the
            // project rotation (fresh drop, landscape clips) this is a no-op and the
            // skip-recompute fast path is preserved.
            if let Some(effective_rotation) = self.maybe_apply_input_rotation_on_load(&*params, &stab) {
                log::info!(target: "stab.load", "load-time input rotation applied: effective_rotation={} output_size={:?}", effective_rotation, stab.params.read().output_size);
            }

            self.set_keyframe_provider(&stab);

            if let Ok(im) = params.get_i32(Params::IntegrationMethod) {
                let mut gyro = stab.gyro.write();
                gyro.integration_method = im as usize;
                gyro.apply_transforms();
            }

            if let Ok(zm) = params.get_i32(Params::ZoomMode) {
                // ZoomMode is a 3-value OFX dropdown but adaptive_zoom_window
                // is a continuous f64 in the project file. The naive
                // `params.write() = zoom_window_from_mode_index(zm)` lossy
                // round-trip clobbers any project value that doesn't land
                // exactly on {0.0, -1.0, 4.0} — e.g. a desktop-saved
                // smoothing window of 2.0 gets quietly forced to 4.0 on
                // every OFX load, producing fovs that disagree with the
                // desktop app's preview and re-firing the §11 snapshot diff
                // (see optimize-stab-load-pipeline design.md §29).
                // Only overwrite when the user's OFX choice changes the
                // bucket; within the same bucket preserve the project's
                // exact value.
                let current = stab.params.read().adaptive_zoom_window;
                if mode_index_from_zoom_window(current) != zm {
                    stab.params.write().adaptive_zoom_window = zoom_window_from_mode_index(zm);
                }
            }

            // §11.4 + §11.5: skip the redundant second recompute when nothing
            // relevant changed since import #1. The skip target is the
            // 25-iter / ~85 s Max-Zoom limiter rerun observed in
            // gyroflow-openfx.log for a non-anamorphic Canon MXF where import
            // #1 reported `any above limit: false` but the post-mutation
            // recompute disagreed. Diff list goes to `stab.load` so a future
            // log shows which mutation actually triggers the rerun.
            let post_snapshot = snapshot_compute_inputs(&stab);
            if pre_snapshot != post_snapshot {
                let diff = pre_snapshot.diff(&post_snapshot);
                log::info!(target: "stab.load", "post-mutation recompute fired, diff={diff:?}");
                stab.invalidate_smoothing();
                stab.recompute_blocking();
            } else {
                log::info!(target: "stab.load", "post-mutation recompute skipped, no input change");
            }
            let inverse = !(params.get_bool(Params::UseGyroflowsKeyframes)? && stab.keyframes.read().is_keyframed_internally(&KeyframeType::VideoSpeed));
            stab.params.write().calculate_ramped_timestamps(&stab.keyframes.read(), inverse, inverse);

            let stab = Arc::new(stab);
            // Recompute cache key in case disable_stretch was auto-flipped (lens-stretch detection
            // above or plugin_disable_stretch JSON flag). Caching under the original false-key
            // would otherwise pin the disabled stab manager (lens permanently stretched=1.0) under
            // the disable=false key, making toggling DisableStretch off look identical to on.
            let key = format!("{path}{disable_stretch}{instance_id}");
            // Insert to static global cache
            manager_cache.lock().put(key.to_owned(), stab.clone());
            // Cache it in this instance as well
            self.managers.put(key.to_owned(), stab.clone());

            stab
        };

        Ok(stab)
    }

    pub fn clear_stab(&mut self, manager_cache: &Mutex<LruCache<String, Arc<StabilizationManager>>>) {
        let local_keys = self.managers.iter().map(|x| x.0.clone()).collect::<Vec<_>>();
        self.managers.clear();

        // If there are no more local references, delete it from global cache
        let mut lock = manager_cache.lock();
        for key in local_keys {
            if let Some(v) = lock.get(&key) {
                if Arc::strong_count(v) == 1 {
                    lock.pop(&key);
                }
            }
        }
    }

    pub fn disable_opencl(&mut self) {
        if !self.opencl_disabled {
            unsafe { std::env::set_var("NO_OPENCL", "1") };
            self.opencl_disabled = true;
        }
    }

    pub fn set_status(&mut self, params: &mut dyn GyroflowPluginParams, status: &str, hint: &str, ok: bool) {
        if params.get_string(Params::Status).unwrap_or_default() != status {
            let _ = params.set_string(Params::Status, status);
            let _ = params.set_hint(Params::Status, hint);
            if ok {
                self.update_loaded_state(params, ok);
            }
        }
    }

    pub fn browse(current_path: &str) -> String {
        let mut d = rfd::FileDialog::new()
            .add_filter("Project and video files", &["mp4", "mov", "mxf", "braw", "r3d", "insv", "gyroflow"]);
        if !current_path.is_empty() {
            if let Some(path) = std::path::Path::new(current_path).parent() {
                d = d.set_directory(path);
            }
        }
        if let Some(d) = d.pick_file() {
            d.display().to_string()
        } else {
            String::new()
        }
    }

    pub fn param_changed(&mut self, params: &mut dyn GyroflowPluginParams, manager_cache: &Mutex<LruCache<String, Arc<StabilizationManager>>>, param: Params, user_edited: bool) -> Result<(), Box<dyn std::error::Error>> {
        if param == Params::Browse {
            let new_path = Self::browse(&params.get_string(Params::ProjectPath)?);
            if !new_path.is_empty() {
                params.set_string(Params::ProjectPath, &new_path)?;
                self.reload_values_from_project = true;
            }
        }
        if param == Params::LoadLens {
            let lens_directory = gyroflow_core::settings::data_dir().join("lens_profiles");
            log::info!("lens directory: {lens_directory:?}");

            let mut d = rfd::FileDialog::new().add_filter("Lens profiles and presets", &["json", "gyroflow"]);
            d = d.set_directory(lens_directory);
            if let Some(d) = d.pick_file() {
                let d = d.display().to_string();
                if !d.is_empty() {
                    if let Ok(contents) = std::fs::read_to_string(&d) {
                        if d.ends_with(".json") {
                            params.set_string(Params::EmbeddedLensProfile, &contents)?;
                        } else {
                            params.set_string(Params::EmbeddedPreset, &contents)?;
                        }
                        self.reload_values_from_project = true;
                    }
                    self.clear_stab(&manager_cache);
                }
            }
        }
        if param == Params::OpenGyroflow {
            GyroflowPluginBase::open_gyroflow(params.get_string(Params::ProjectPath).ok().as_deref());
        }
        if param == Params::OpenRecentProject {
            let last_project = gyroflow_core::settings::get_str("lastProject", "");
            if !last_project.is_empty() {
                params.set_string(Params::ProjectPath, &last_project)?;
                self.reload_values_from_project = true;
                self.clear_stab(&manager_cache);
            }
        }
        if param == Params::ProjectPath || param == Params::ReloadProject || param == Params::LoadCurrent || param == Params::DontDrawOutside {
            if (param == Params::ProjectPath && user_edited) || param == Params::ReloadProject || param == Params::LoadCurrent {
                self.reload_values_from_project = true;
            }
            self.clear_stab(&manager_cache);
        }
        if param == Params::IncludeProjectData {
            let path = params.get_string(Params::ProjectPath)?;
            let ds = params.get_bool(Params::DisableStretch).unwrap_or(false);
            if params.get_bool(Params::IncludeProjectData).unwrap_or_default() {
                if path.ends_with(".gyroflow") {
                    if let Ok(data) = std::fs::read_to_string(&path) {
                        if StabilizationManager::project_has_motion_data(data.as_bytes()) {
                            let data = GyroflowPluginBase::maybe_inject_disable_stretch(&data, ds);
                            params.set_string(Params::ProjectData, &data)?;
                        } else {
                            if let Some((_, stab)) = self.managers.peek_lru() {
                                if let Ok(data) = stab.export_gyroflow_data(gyroflow_core::GyroflowProjectType::WithGyroData, "{}", None) {
                                    let data = GyroflowPluginBase::maybe_inject_disable_stretch(&data, ds);
                                    params.set_string(Params::ProjectData, &data)?;
                                }
                            }
                        }
                    } else {
                        params.set_string(Params::ProjectData, "")?;
                    }
                } else {
                    if let Some((_, stab)) = self.managers.peek_lru() {
                        if let Ok(data) = stab.export_gyroflow_data(gyroflow_core::GyroflowProjectType::WithGyroData, "{}", None) {
                            let data = GyroflowPluginBase::maybe_inject_disable_stretch(&data, ds);
                            params.set_string(Params::ProjectData, &data)?;
                        }
                    }
                }
            } else {
                params.set_string(Params::ProjectData, &"")?;
            }
        }
        if user_edited {
            if param == Params::OutputWidth || param == Params::OutputHeight || param == Params::OutputSizeSwap || param == Params::OutputSizeToTimeline {
                if param == Params::OutputSizeSwap {
                    let (w, h) = (params.get_f64(Params::OutputWidth)?, params.get_f64(Params::OutputHeight)? as _);
                    params.set_f64(Params::OutputWidth, h)?;
                    params.set_f64(Params::OutputHeight, w)?;
                }
                if param == Params::OutputSizeToTimeline {
                    params.set_f64(Params::OutputWidth, self.timeline_size.0 as f64)?;
                    params.set_f64(Params::OutputHeight, self.timeline_size.1 as f64)?;
                }
                for (_, v) in self.managers.iter_mut() {
                    v.set_output_size(params.get_f64(Params::OutputWidth)? as _, params.get_f64(Params::OutputHeight)? as _);
                    v.invalidate_blocking_zooming();
                }
            }
            match param {
                Params::Fov | Params::Smoothness | Params::ZoomLimit | Params::LensCorrectionStrength |
                Params::HorizonLockAmount | Params::HorizonLockRoll |
                //Params::PositionX | Params::PositionY |
                Params::AdditionalPitch | Params::AdditionalYaw |
                Params::Rotation | Params::InputRotation | Params::VideoSpeed | Params::IntegrationMethod | Params::ZoomMode |
                Params::UseGyroflowsKeyframes | Params::RecalculateKeyframes => {

                    params.set_string(Params::Status, t!("status.calculating"))?;
                    if !self.ever_changed {
                        self.ever_changed = true;
                        params.set_string(Params::InstanceId, &format!("{}", fastrand::u64(..)))?;
                        self.clear_stab(manager_cache);
                    }
                    let use_gyroflows_keyframes = params.get_bool(Params::UseGyroflowsKeyframes).unwrap_or_default();
                    self.cache_keyframes(params, use_gyroflows_keyframes, self.num_frames, self.fps.max(1.0));
                    for (_, v) in self.managers.iter_mut() {
                        match param {
                            Params::IntegrationMethod => {
                                if let Ok(im) = params.get_i32(Params::IntegrationMethod) {
                                    let mut gyro = v.gyro.write();
                                    gyro.integration_method = im as usize;
                                    gyro.apply_transforms();
                                }
                                v.invalidate_blocking_smoothing();
                                v.invalidate_blocking_zooming();
                            }
                            Params::ZoomMode => {
                                if let Ok(zm) = params.get_i32(Params::ZoomMode) {
                                    v.params.write().adaptive_zoom_window = zoom_window_from_mode_index(zm);
                                }
                                v.invalidate_blocking_zooming();
                            }
                            Params::Smoothness | Params::ZoomLimit | Params::HorizonLockAmount | Params::HorizonLockRoll |
                            Params::AdditionalPitch | Params::AdditionalYaw | Params::RecalculateKeyframes => {
                                v.invalidate_blocking_smoothing();
                                v.invalidate_blocking_zooming();
                            },
                            //Params::PositionX | Params::PositionY |
                            Params::LensCorrectionStrength | Params::Rotation => {
                                v.invalidate_blocking_zooming();
                            },
                            _ => { }
                        }
                        v.invalidate_blocking_undistortion();
                        match param {
                            Params::VideoSpeed | Params::UseGyroflowsKeyframes | Params::RecalculateKeyframes => {
                                let inverse = !(use_gyroflows_keyframes && v.keyframes.read().is_keyframed_internally(&KeyframeType::VideoSpeed));
                                v.params.write().calculate_ramped_timestamps(&v.keyframes.read(), inverse, inverse);
                            },
                            _ => { }
                        }
                    }
                    params.set_string(Params::Status, t!("status.ok"))?;
                },
                _ => { }
            }
            if param == Params::ToggleOverview {
                let on = params.get_bool(Params::ToggleOverview)?;
                for (_, v) in self.managers.iter_mut() {
                    v.set_fov_overview(on);
                    v.invalidate_blocking_undistortion();
                }
            }
            if param == Params::Interpolation {
                self.managers.clear();
                manager_cache.lock().clear();
            }
        }

        Ok(())
    }
}

pub fn hash_string(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

impl std::str::FromStr for Params {
    type Err = serde_json::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(&format!("\"{}\"", s))
    }
}
impl ToString for Params {
    fn to_string(&self) -> String {
        format!("{:?}", self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct TestParams {
        strings: BTreeMap<Params, String>,
        bools: BTreeMap<Params, bool>,
        f64s: BTreeMap<Params, f64>,
        i32s: BTreeMap<Params, i32>,
    }

    impl GyroflowPluginParams for TestParams {
        fn set_enabled(&mut self, _param: Params, _enabled: bool) -> PluginResult<()> { Ok(()) }
        fn set_label(&mut self, _param: Params, _label: &str) -> PluginResult<()> { Ok(()) }
        fn set_hint(&mut self, _param: Params, _hint: &str) -> PluginResult<()> { Ok(()) }

        fn set_f64(&mut self, param: Params, value: f64) -> PluginResult<()> {
            self.f64s.insert(param, value);
            Ok(())
        }
        fn get_f64(&self, param: Params) -> PluginResult<f64> {
            Ok(*self.f64s.get(&param).unwrap_or(&0.0))
        }
        fn get_f64_at_time(&self, param: Params, _time: TimeType) -> PluginResult<f64> {
            self.get_f64(param)
        }
        fn set_bool(&mut self, param: Params, value: bool) -> PluginResult<()> {
            self.bools.insert(param, value);
            Ok(())
        }
        fn get_bool(&self, param: Params) -> PluginResult<bool> {
            Ok(*self.bools.get(&param).unwrap_or(&false))
        }
        fn get_bool_at_time(&self, param: Params, _time: TimeType) -> PluginResult<bool> {
            self.get_bool(param)
        }
        fn set_string(&mut self, param: Params, value: &str) -> PluginResult<()> {
            self.strings.insert(param, value.to_owned());
            Ok(())
        }
        fn get_string(&self, param: Params) -> PluginResult<String> {
            Ok(self.strings.get(&param).cloned().unwrap_or_default())
        }
        fn set_i32(&mut self, param: Params, value: i32) -> PluginResult<()> {
            self.i32s.insert(param, value);
            Ok(())
        }
        fn get_i32(&self, param: Params) -> PluginResult<i32> {
            Ok(*self.i32s.get(&param).unwrap_or(&0))
        }

        fn is_keyframed(&self, _param: Params) -> bool { false }
        fn get_keyframes(&self, _param: Params) -> Vec<(TimeType, f64)> { Vec::new() }
        fn clear_keyframes(&mut self, _param: Params) -> PluginResult<()> { Ok(()) }
        fn set_f64_at_time(&mut self, param: Params, _time: TimeType, value: f64) -> PluginResult<()> {
            self.set_f64(param, value)
        }
    }

    fn cache_with_instance_manager(instance: &mut GyroflowPluginBaseInstance) -> Mutex<LruCache<String, Arc<StabilizationManager>>> {
        let cache = Mutex::new(LruCache::new(std::num::NonZeroUsize::new(1).unwrap()));
        let stab = Arc::new(StabilizationManager::default());
        instance.managers.put("cached".to_owned(), stab.clone());
        cache.lock().put("cached".to_owned(), stab);
        cache
    }

    #[test]
    fn fresh_instance_initialization_enables_project_value_reload() {
        let mut instance = GyroflowPluginBaseInstance {
            reload_values_from_project: false,
            ever_changed: false,
            ..Default::default()
        };
        let mut instance_id = String::new();

        instance.initialize_instance_id(&mut instance_id);

        assert!(!instance_id.is_empty());
        assert!(instance.ever_changed);
        assert!(instance.reload_values_from_project);
    }

    #[test]
    fn copied_instance_initialization_preserves_reload_policy() {
        let mut instance = GyroflowPluginBaseInstance {
            reload_values_from_project: false,
            ever_changed: false,
            ..Default::default()
        };
        let mut instance_id = "copied-instance".to_owned();

        instance.initialize_instance_id(&mut instance_id);

        assert_eq!(instance_id, "copied-instance");
        assert!(!instance.ever_changed);
        assert!(!instance.reload_values_from_project);
    }

    #[test]
    fn host_project_path_change_clears_cache_without_reloading_project_values() {
        let mut instance = GyroflowPluginBaseInstance {
            reload_values_from_project: false,
            ..Default::default()
        };
        let cache = cache_with_instance_manager(&mut instance);
        let mut params = TestParams::default();

        instance.param_changed(&mut params, &cache, Params::ProjectPath, false).unwrap();

        assert!(!instance.reload_values_from_project);
        assert!(instance.managers.iter().next().is_none());
        assert!(cache.lock().iter().next().is_none());
    }

    #[test]
    fn user_project_path_change_reloads_project_values() {
        let mut instance = GyroflowPluginBaseInstance {
            reload_values_from_project: false,
            ..Default::default()
        };
        let cache = Mutex::new(LruCache::new(std::num::NonZeroUsize::new(1).unwrap()));
        let mut params = TestParams::default();

        instance.param_changed(&mut params, &cache, Params::ProjectPath, true).unwrap();

        assert!(instance.reload_values_from_project);
    }

    #[test]
    fn explicit_reload_actions_reload_project_values() {
        for param in [Params::ReloadProject, Params::LoadCurrent] {
            let mut instance = GyroflowPluginBaseInstance {
                reload_values_from_project: false,
                ..Default::default()
            };
            let cache = Mutex::new(LruCache::new(std::num::NonZeroUsize::new(1).unwrap()));
            let mut params = TestParams::default();

            instance.param_changed(&mut params, &cache, param, false).unwrap();

            assert!(instance.reload_values_from_project);
        }
    }

    #[test]
    fn open_recent_project_reloads_project_values_and_clears_cache() {
        let data_dir = std::env::temp_dir().join(format!("gyroflow-plugin-test-{}", fastrand::u64(..)));
        unsafe {
            std::env::set_var("GYROFLOW_DATA_DIR", data_dir);
        }
        let recent_project = "C:/projects/recent.gyroflow";
        gyroflow_core::settings::set("lastProject", recent_project.into());

        let mut instance = GyroflowPluginBaseInstance {
            reload_values_from_project: false,
            ..Default::default()
        };
        let cache = cache_with_instance_manager(&mut instance);
        let mut params = TestParams::default();

        instance.param_changed(&mut params, &cache, Params::OpenRecentProject, false).unwrap();

        assert_eq!(params.get_string(Params::ProjectPath).unwrap(), recent_project);
        assert!(instance.reload_values_from_project);
        assert!(instance.managers.iter().next().is_none());
        assert!(cache.lock().iter().next().is_none());
    }

    #[test]
    fn automatic_lens_stretch_disable_respects_instance_policy() {
        let instance = GyroflowPluginBaseInstance {
            auto_disable_stretch: false,
            ..Default::default()
        };
        let mut params = TestParams::default();
        let mut disable_stretch = false;

        instance.maybe_auto_disable_stretch_for_lens(&mut params, &mut disable_stretch, 1.33, 1.0).unwrap();

        assert!(!disable_stretch);
        assert!(!params.get_bool(Params::DisableStretch).unwrap());
    }

    #[test]
    fn automatic_lens_stretch_disable_remains_enabled_by_default() {
        let instance = GyroflowPluginBaseInstance::default();
        let mut params = TestParams::default();
        let mut disable_stretch = false;

        instance.maybe_auto_disable_stretch_for_lens(&mut params, &mut disable_stretch, 1.33, 1.0).unwrap();

        assert!(disable_stretch);
        assert!(params.get_bool(Params::DisableStretch).unwrap());
    }

    #[test]
    fn embedded_disable_stretch_flag_respects_instance_policy() {
        let instance = GyroflowPluginBaseInstance {
            auto_disable_stretch: false,
            ..Default::default()
        };
        let mut params = TestParams::default();
        params.set_string(Params::ProjectData, r#"{"plugin_disable_stretch":true}"#).unwrap();
        let mut disable_stretch = false;

        instance.maybe_auto_disable_stretch_from_embedded_data(&mut params, &mut disable_stretch).unwrap();

        assert!(!disable_stretch);
        assert!(!params.get_bool(Params::DisableStretch).unwrap());
    }

    #[test]
    fn embedded_disable_stretch_flag_remains_enabled_by_default() {
        let instance = GyroflowPluginBaseInstance::default();
        let mut params = TestParams::default();
        params.set_string(Params::ProjectData, r#"{"plugin_disable_stretch":true}"#).unwrap();
        let mut disable_stretch = false;

        instance.maybe_auto_disable_stretch_from_embedded_data(&mut params, &mut disable_stretch).unwrap();

        assert!(disable_stretch);
        assert!(params.get_bool(Params::DisableStretch).unwrap());
    }

    #[test]
    fn deserialized_legacy_instance_keeps_auto_disable_stretch_enabled() {
        let instance: GyroflowPluginBaseInstance = serde_json::from_str("{}").unwrap();

        assert!(instance.auto_disable_stretch);
    }

    #[test]
    fn macos_project_open_uses_file_open_event() {
        let command = GyroflowPluginBase::gyroflow_launch_command(
            "/Applications/GyroflowNiYien.app",
            Some("/Users/jhe/project.gyroflow"),
            "macos",
        ).unwrap();

        assert_eq!(command.program, "open");
        assert_eq!(command.args, vec![
            "-a",
            "/Applications/GyroflowNiYien.app",
            "/Users/jhe/project.gyroflow",
        ]);
        assert!(!command.args.iter().any(|arg| arg == "--args" || arg == "--open"));
    }

    #[test]
    fn macos_without_project_activates_app() {
        let command = GyroflowPluginBase::gyroflow_launch_command(
            "/Applications/GyroflowNiYien.app",
            None,
            "macos",
        ).unwrap();

        assert_eq!(command.program, "open");
        assert_eq!(command.args, vec!["-a", "/Applications/GyroflowNiYien.app"]);
    }

    // §11.8 unit stand-in: full integration test (mock GyroflowPluginParams +
    // .gyroflow fixture + log capture) requires test infra that doesn't exist
    // in this crate. Verify the testable invariant directly: snapshot of an
    // un-mutated manager equals itself, so the `if pre != post` skip branch
    // would fire on the same-state load path.
    #[test]
    fn snapshot_of_unchanged_manager_is_equal() {
        let stab = StabilizationManager::default();
        let pre = snapshot_compute_inputs(&stab);
        let post = snapshot_compute_inputs(&stab);
        assert_eq!(pre, post);
        assert!(pre.diff(&post).is_empty());
    }

    // §11.9 unit stand-in: mutate the field that the OFX ZoomMode round-trip
    // writes (`stab.params.write().adaptive_zoom_window = ...` at
    // common/src/lib.rs:960), then verify the diff names that exact field.
    #[test]
    fn snapshot_diff_detects_adaptive_zoom_window_mutation() {
        let stab = StabilizationManager::default();
        let original = stab.params.read().adaptive_zoom_window;
        let pre = snapshot_compute_inputs(&stab);
        stab.params.write().adaptive_zoom_window = original + 1.0;
        let post = snapshot_compute_inputs(&stab);
        let diff = pre.diff(&post);
        assert_eq!(diff, vec!["adaptive_zoom_window"]);
    }

    // Coverage for the other commonly-mutated fields in the OFX flow so the
    // skip decision is provably safe across the mutation table in design.md.
    #[test]
    fn snapshot_diff_detects_integration_method_change() {
        let stab = StabilizationManager::default();
        let original = stab.gyro.read().integration_method;
        let pre = snapshot_compute_inputs(&stab);
        stab.gyro.write().integration_method = original.wrapping_add(1);
        let post = snapshot_compute_inputs(&stab);
        assert_eq!(pre.diff(&post), vec!["integration_method"]);
    }

    #[test]
    fn snapshot_diff_detects_output_size_change() {
        let stab = StabilizationManager::default();
        let pre = snapshot_compute_inputs(&stab);
        stab.params.write().output_size = (1920, 1080);
        let post = snapshot_compute_inputs(&stab);
        assert_eq!(pre.diff(&post), vec!["output_size"]);
    }

    // §11.7 throttle behavior: first NotFound caches; second within the window
    // returns the cached msg; after explicit clear the cache misses again.
    #[test]
    fn not_found_cache_throttles_within_window_and_clears() {
        let path = format!("not_found_cache_test_{}", fastrand::u64(..));
        assert!(check_not_found_cache(&path).is_none());
        record_not_found(&path, "boom".into());
        assert_eq!(check_not_found_cache(&path).as_deref(), Some("boom"));
        clear_not_found(&path);
        assert!(check_not_found_cache(&path).is_none());
    }

    // ============================================================================================
    // Load-time InputRotation step (openfx-restore-rotation-order). Mirrors the call inside
    // `stab_manager`'s mutation block: the step runs between `set_output_size(OutputWidth/Height)`
    // and the §11.4 post_snapshot, so its mutations are exactly what the snapshot diff sees.
    // ============================================================================================

    // Build a stab manager that mimics the post-import state of the reproduction project:
    // landscape 2048x1080 source, video_rotation = 0 (the .gyroflow's value).
    fn landscape_project_stab() -> StabilizationManager {
        let stab = StabilizationManager::default();
        {
            let mut p = stab.params.write();
            p.size = (2048, 1080);
            p.output_size = (2048, 1080);
            p.video_rotation = 0.0;
        }
        stab
    }

    // Params as persisted by the host on restore: InputRotation = 90° left (index 1), the
    // OutputWidth/Height params still hold the landscape project values.
    fn restored_rotation_params() -> TestParams {
        let mut params = TestParams::default();
        params.set_i32(Params::InputRotation, 1).unwrap();
        params.set_f64(Params::OutputWidth, 2048.0).unwrap();
        params.set_f64(Params::OutputHeight, 1080.0).unwrap();
        params
    }

    // (a) Flag off (Adobe / frei0r / Fusion): the step is unreachable and the stab is untouched —
    // the byte-identical guarantee for non-OpenFX consumers of `stab_manager`.
    #[test]
    fn load_time_rotation_step_flag_off_leaves_stab_untouched() {
        let instance = GyroflowPluginBaseInstance {
            original_project_rotation: Some(0.0),
            ..Default::default() // apply_input_rotation_on_load defaults to false
        };
        let params = restored_rotation_params();
        let stab = landscape_project_stab();

        let pre = snapshot_compute_inputs(&stab);
        assert_eq!(instance.maybe_apply_input_rotation_on_load(&params, &stab), None);
        let post = snapshot_compute_inputs(&stab);

        assert_eq!(pre, post);
        assert_eq!(stab.params.read().video_rotation, 0.0);
        assert_eq!(stab.params.read().output_size, (2048, 1080));
    }

    // (b) Flag on + restored InputRotation=90 + landscape project: the step transposes the output
    // before the snapshot diff, so the single post-mutation recompute runs on portrait geometry
    // (the diff includes "output_size" — the recompute fires, on the correct size).
    #[test]
    fn load_time_rotation_step_transposes_output_before_snapshot_diff() {
        let instance = GyroflowPluginBaseInstance {
            apply_input_rotation_on_load: true,
            original_project_rotation: Some(0.0),
            ..Default::default()
        };
        let params = restored_rotation_params();
        let stab = landscape_project_stab();

        let pre = snapshot_compute_inputs(&stab);
        assert_eq!(instance.maybe_apply_input_rotation_on_load(&params, &stab), Some(90.0));
        let post = snapshot_compute_inputs(&stab);

        assert_eq!(stab.params.read().video_rotation, 90.0);
        assert_eq!(stab.params.read().output_size, (1080, 2048));
        let diff = pre.diff(&post);
        assert!(diff.contains(&"output_size"), "diff={diff:?}");
        assert!(diff.contains(&"video_rotation"), "diff={diff:?}");
    }

    // (c) Flag on + InputRotation matching the project rotation (fresh drop on a landscape clip):
    // the step is a no-op and the skip-recompute fast path (equal snapshots) is preserved.
    #[test]
    fn load_time_rotation_step_matching_rotation_keeps_fast_path() {
        let instance = GyroflowPluginBaseInstance {
            apply_input_rotation_on_load: true,
            original_project_rotation: Some(0.0),
            ..Default::default()
        };
        let mut params = restored_rotation_params();
        params.set_i32(Params::InputRotation, 0).unwrap(); // 0° == project rotation

        let stab = landscape_project_stab();

        let pre = snapshot_compute_inputs(&stab);
        assert_eq!(instance.maybe_apply_input_rotation_on_load(&params, &stab), None);
        let post = snapshot_compute_inputs(&stab);

        assert_eq!(pre, post);
        assert!(pre.diff(&post).is_empty());
    }

    // Flag on but no project rotation captured yet (defensive: capture happens right after
    // import in the cache-miss branch, so this state should not occur in practice).
    #[test]
    fn load_time_rotation_step_without_captured_rotation_is_noop() {
        let instance = GyroflowPluginBaseInstance {
            apply_input_rotation_on_load: true,
            original_project_rotation: None,
            ..Default::default()
        };
        let params = restored_rotation_params();
        let stab = landscape_project_stab();

        assert_eq!(instance.maybe_apply_input_rotation_on_load(&params, &stab), None);
        assert_eq!(stab.params.read().video_rotation, 0.0);
        assert_eq!(stab.params.read().output_size, (2048, 1080));
    }
}

#[macro_export]
macro_rules! define_params {
    ($name:ident {
        strings: [ $($str_enum:ident  => $str_field:ident: $str_host_type:ty,)* ],
        bools:   [ $($bool_enum:ident => $bool_field:ident: $bool_host_type:ty,)* ],
        f64s:    [ $($f64_enum:ident  => $f64_field:ident: $f64_host_type:ty,)* ],
        i32s:    [ $($i32_enum:ident  => $i32_field:ident: $i32_host_type:ty,)* ],

        get_string:  $gstr_s:ident   $gstr_p:ident                    $gstr_block:block,
        set_string:  $sstr_s:ident   $sstr_p:ident,   $sstr_v:ident   $sstr_block:block,
        get_bool:    $gbool_s:ident  $gbool_p:ident                   $gbool_block:block,
        set_bool:    $sbool_s:ident  $sbool_p:ident,  $sbool_v:ident  $sbool_block:block,
        get_f64:     $gf64_s:ident   $gf64_p:ident                    $gf64_block:block,
        set_f64:     $sf64_s:ident   $sf64_p:ident,   $sf64_v:ident   $sf64_block:block,
        get_i32:     $gi32_s:ident   $gi32_p:ident                    $gi32_block:block,
        set_i32:     $si32_s:ident   $si32_p:ident,   $si32_v:ident   $si32_block:block,
        set_label:   $slabel_s:ident $slabel_p:ident, $slabel_v:ident $slabel_block:block,
        set_hint:    $shint_s:ident  $shint_p:ident,  $shint_v:ident  $shint_block:block,
        set_enabled: $sen_s:ident    $sen_p:ident,    $sen_v:ident    $sen_block:block,
        get_bool_at_time: $gtbool_s:ident  $gtbool_p:ident, $gtbool_t:ident                $gtbool_block:block,
        get_f64_at_time:  $gtf64_s:ident   $gtf64_p:ident,  $gtf64_t:ident                 $gtf64_block:block,
        set_f64_at_time:  $stf64_s:ident  $stf64_p:ident,  $stf64_t:ident, $stf64_v:ident $stf64_block:block,
        is_keyframed: $iskeyframe_s:ident  $iskeyframe_p:ident $iskeyframe_block:block,
        get_keyframes: $gkeyframes_s:ident $gkeyframes_p:ident $gkeyframes_block:block,
        clear_keyframes: $clr_s:ident      $clr_p:ident $clr_block:block,

        $($additional_fields:ident: $additional_fields_t:ty,)*
    }) => {
        #[derive(Default)]
        pub struct ParamsAdditionalFields {
            $( pub $additional_fields: $additional_fields_t, )*
        }
        pub struct $name {
            $( $str_field: $str_host_type, )*
            $( $bool_field: $bool_host_type, )*
            $( $f64_field: $f64_host_type, )*
            $( $i32_field: $i32_host_type, )*

            pub fields: ParamsAdditionalFields,
        }
        impl GyroflowPluginParams for $name {
            fn get_string(&self, param: Params) -> $crate::PluginResult<String> {
                let $gstr_s = &self.fields;
                match param {
                    $( Params::$str_enum => { let $gstr_p = &self.$str_field; $gstr_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn set_string(&mut self, param: Params, value: &str) -> $crate::PluginResult<()> {
                let mut $sstr_s = &mut self.fields;
                match param {
                    $( Params::$str_enum => { let $sstr_p = &mut self.$str_field; let $sstr_v = value; $sstr_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn get_bool(&self, param: Params) -> $crate::PluginResult<bool> {
                let $gbool_s = &self.fields;
                match param {
                    $( Params::$bool_enum => { let $gbool_p = &self.$bool_field; $gbool_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn set_bool(&mut self, param: Params, value: bool) -> $crate::PluginResult<()> {
                let mut $sbool_s = &mut self.fields;
                match param {
                    $( Params::$bool_enum => { let $sbool_p = &mut self.$bool_field; let $sbool_v = value; $sbool_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn get_f64(&self, param: Params) -> $crate::PluginResult<f64> {
                let $gf64_s = &self.fields;
                match param {
                    $( Params::$f64_enum => { let $gf64_p = &self.$f64_field; $gf64_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn set_f64(&mut self, param: Params, value: f64) -> $crate::PluginResult<()> {
                let mut $sf64_s = &mut self.fields;
                match param {
                    $( Params::$f64_enum => { let $sf64_p = &mut self.$f64_field; let $sf64_v = value; $sf64_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn get_i32(&self, param: Params) -> $crate::PluginResult<i32> {
                let $gi32_s = &self.fields;
                match param {
                    $( Params::$i32_enum => { let $gi32_p = &self.$i32_field; $gi32_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn set_i32(&mut self, param: Params, value: i32) -> $crate::PluginResult<()> {
                let mut $si32_s = &mut self.fields;
                match param {
                    $( Params::$i32_enum => { let $si32_p = &mut self.$i32_field; let $si32_v = value; $si32_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn set_label(&mut self, param: Params, label: &str) -> $crate::PluginResult<()> {
                let mut $slabel_s = &mut self.fields;
                let $slabel_v = label;
                match param {
                    $( Params::$str_enum  => { let $slabel_p = &mut self.$str_field;  $slabel_block }, )*
                    $( Params::$bool_enum => { let $slabel_p = &mut self.$bool_field; $slabel_block }, )*
                    $( Params::$f64_enum  => { let $slabel_p = &mut self.$f64_field;  $slabel_block }, )*
                    $( Params::$i32_enum  => { let $slabel_p = &mut self.$i32_field;  $slabel_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn set_hint(&mut self, param: Params, hint: &str) -> $crate::PluginResult<()> {
                let mut $shint_s = &mut self.fields;
                let $shint_v = hint;
                match param {
                    $( Params::$str_enum  => { let $shint_p = &mut self.$str_field;  $shint_block }, )*
                    $( Params::$bool_enum => { let $shint_p = &mut self.$bool_field; $shint_block }, )*
                    $( Params::$f64_enum  => { let $shint_p = &mut self.$f64_field;  $shint_block }, )*
                    $( Params::$i32_enum  => { let $shint_p = &mut self.$i32_field;  $shint_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn set_enabled(&mut self, param: Params, enabled: bool) -> $crate::PluginResult<()> {
                let mut $sen_s = &mut self.fields;
                let $sen_v = enabled;
                match param {
                    $( Params::$str_enum  => { let $sen_p = &mut self.$str_field;  $sen_block }, )*
                    $( Params::$bool_enum => { let $sen_p = &mut self.$bool_field; $sen_block }, )*
                    $( Params::$f64_enum  => { let $sen_p = &mut self.$f64_field;  $sen_block }, )*
                    $( Params::$i32_enum  => { let $sen_p = &mut self.$i32_field;  $sen_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn get_f64_at_time(&self, param: Params, time: TimeType) -> $crate::PluginResult<f64> {
                let $gtf64_s = &self.fields;
                match param {
                    $( Params::$f64_enum => { let $gtf64_p = &self.$f64_field; let $gtf64_t = time; $gtf64_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn get_bool_at_time(&self, param: Params, time: TimeType) -> $crate::PluginResult<bool> {
                let $gtbool_s = &self.fields;
                match param {
                    $( Params::$bool_enum => { let $gtbool_p = &self.$bool_field; let $gtbool_t = time; $gtbool_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn clear_keyframes(&mut self, param: Params) -> $crate::PluginResult<()> {
                let mut $clr_s = &mut self.fields;
                match param {
                    $( Params::$f64_enum => { let $clr_p = &mut self.$f64_field; $clr_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn is_keyframed(&self, param: Params) -> bool {
                let $iskeyframe_s = &self.fields;
                match param {
                    $( Params::$f64_enum => { let $iskeyframe_p = &self.$f64_field; $iskeyframe_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn get_keyframes(&self, param: Params) -> Vec<(TimeType, f64)> {
                let $gkeyframes_s = &self.fields;
                match param {
                    $( Params::$f64_enum => { let $gkeyframes_p = &self.$f64_field; $gkeyframes_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
            fn set_f64_at_time(&mut self, param: Params, time: TimeType, value: f64) -> $crate::PluginResult<()> {
                let mut $stf64_s = &mut self.fields;
                match param {
                    $( Params::$f64_enum => { let $stf64_p = &mut self.$f64_field; let $stf64_t = time; let $stf64_v = value; $stf64_block }, )*
                    _ => panic!("Wrong parameter type"),
                }
            }
        }
    };
}
