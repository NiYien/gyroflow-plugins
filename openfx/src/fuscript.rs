use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use gyroflow_plugin_base::parking_lot::Mutex;
use gyroflow_plugin_base::rfd;

const FAILED_MSG: &str = "This feature relies on external scripting and is only available in paid Resolve Studio. You have to allow executing scripts:\n
Set \"Preferences -> General -> External scripting using\" to \"Local\".\n\n
It must be the currently displayed video on the timeline.\n
It is also impossible to query file path on a compound clip.\n\nIn any case, you can just select the video or project file using the \"Browse\" button.";

fn replace_frame_count(input: &str) -> String {
    use regex::Regex;
    let re = Regex::new(r"\[(\d+)-(\d+)\]").unwrap();

    re.replace_all(input, |caps: &regex::Captures| {
        format!("{}", &caps[1])
    }).to_string()
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct CurrentFileInfo {
    pub file_path: String,
    pub project_path: Option<String>,
    pub fps: f64,
    pub duration_s: f64,
    pub frame_count: usize,
    pub width: usize,
    pub height: usize,
    pub pixel_aspect_ratio: String,

    // Host-input-sizing fields populated alongside the core 6 lines by the extended lua script.
    // `mismatch_mode` is the raw `timelineInputResMismatchBehavior` string (`scaleToFit` /
    // `scaleToCrop` / `centerCrop` / `stretch`), already disambiguated by `useCustomSettings`.
    // `timeline_w`/`timeline_h` come from the project (or timeline override) resolution settings;
    // they are used by Stretch mode to set `stab.params.size` to the host buffer dimensions.
    // `use_custom_settings` is the raw timeline `useCustomSettings` value, kept for diagnostics.
    pub mismatch_mode: Option<String>,
    pub timeline_w: usize,
    pub timeline_h: usize,
    pub use_custom_settings: bool,
}
impl CurrentFileInfo {
    pub fn get_fuscript() -> Option<std::path::PathBuf> {
        if cfg!(target_os = "windows") {
            Some(std::path::Path::new("fuscript.exe").to_path_buf())
        } else if cfg!(target_os = "macos") {
            Some(std::path::Path::new("../Libraries/Fusion/fuscript").to_path_buf())
        } else if cfg!(target_os = "linux") {
            let p1 = std::path::Path::new("../libs/Fusion/fuscript");
            let p2 = std::path::Path::new("./libs/Fusion/fuscript");
            if p1.exists() { return Some(p1.to_path_buf()); }
            if p2.exists() { return Some(p2.to_path_buf()); }
            None
        } else {
            None
        }
    }
    pub fn is_available() -> bool {
        Self::get_fuscript().map(|x| x.exists()).unwrap_or_default()
    }
    pub fn query(current_file_info: Arc<Mutex<Option<Self>>>, current_file_info_pending: Arc<AtomicBool>) {
        Self::query_inner(current_file_info, current_file_info_pending, false);
    }

    // Silent variant: same query, but does not pop the rfd error dialog when fuscript fails.
    // Used by automatic triggers (CreateInstance, ReloadProject) where a failure is expected
    // on Resolve Free / non-Resolve hosts / compound clips and the user did not explicitly ask
    // for the query — we just want to populate `CurrentFileInfo` when it happens to be available
    // so the `HostInputSizing` Auto mode has fuscript data to consult.
    pub fn query_silent(current_file_info: Arc<Mutex<Option<Self>>>, current_file_info_pending: Arc<AtomicBool>) {
        Self::query_inner(current_file_info, current_file_info_pending, true);
    }

    fn query_inner(current_file_info: Arc<Mutex<Option<Self>>>, current_file_info_pending: Arc<AtomicBool>, silent: bool) {
        std::thread::spawn(move || {
            let mut cmd = std::process::Command::new(Self::get_fuscript().unwrap());
            #[cfg(target_os = "windows")]
            { use std::os::windows::process::CommandExt; cmd.creation_flags(0x08000000); } // CREATE_NO_WINDOW

            // Extended query: the original 6 lines (FPS, Frames, Duration, PAR, Resolution, File Path)
            // come first to preserve the pre-existing parse-by-line-count expectation. The next 4
            // lines carry the host-input-sizing setting: useCustomSettings (timeline-level toggle),
            // the timeline override or project default for `timelineInputResMismatchBehavior`, and
            // the project's timelineResolutionWidth/Height (used as `stab.params.size` in Stretch).
            // Empty-string fallbacks (older Resolve versions / missing keys) keep the line count.
            let script = "proj = Resolve():GetProjectManager():GetCurrentProject();\
                              tl = proj:GetCurrentTimeline();\
                              p = tl:GetCurrentVideoItem():GetMediaPoolItem():GetClipProperty();\
                              print(p['FPS']);print(p['Frames']);print(p['Duration']);print(p['PAR']);print(p['Resolution']);print(p['File Path']);\
                              ucs = tl:GetSetting('useCustomSettings') or '';\
                              if ucs == '1' then mm = tl:GetSetting('timelineInputResMismatchBehavior') or ''; else mm = proj:GetSetting('timelineInputResMismatchBehavior') or ''; end;\
                              tw = proj:GetSetting('timelineResolutionWidth') or '';\
                              th = proj:GetSetting('timelineResolutionHeight') or '';\
                              print(ucs);print(mm);print(tw);print(th);";
            if let Ok(out) = cmd.args(["-q", "-l", "lua", "-x", &script]).output() {
                let stdout = String::from_utf8(out.stdout).unwrap_or_default();
                let stderr = String::from_utf8(out.stderr).unwrap_or_default();
                // There is a weird bug in DaVinci Resolve fuscript that it complains about
                // missing python2 even regardless of explicitly specified `-l lua` argument.
                // The error message itself is a subject to localization, so it can't be hardcoded in whole.
                // See https://github.com/gyroflow/gyroflow-plugins/issues/24
                fn is_missing_python2(line: &str) -> bool {
                    line.starts_with("sh:") && line.contains("python2:")
                }
                let errors = stderr.trim().lines()
                        .filter(|line| !is_missing_python2(line))
                        .collect::<Vec<_>>();
                let lines = stdout.trim().lines().collect::<Vec<_>>();
                // Accept exactly 10 lines from the extended query. Older Resolve versions without
                // the extra settings keys still emit empty strings (`print('')`) so the line count
                // stays the same; only a true script failure produces fewer lines.
                if errors.is_empty() && lines.len() == 10 {
                    let fps = lines[0].parse::<f64>().unwrap_or_default();
                    let frame_count = lines[1].parse::<usize>().unwrap_or_default();
                    let duration_s = Self::parse_duration(lines[2], fps);
                    let par = lines[3];
                    let resolution = lines[4].split("x").filter_map(|x| x.parse::<usize>().ok()).collect::<Vec<_>>();
                    let file_path = replace_frame_count(lines[5]);
                    let use_custom_settings = lines[6].trim() == "1";
                    let mismatch_mode_raw = lines[7].trim();
                    let mismatch_mode = if mismatch_mode_raw.is_empty() {
                        None
                    } else {
                        Some(mismatch_mode_raw.to_string())
                    };
                    let timeline_w = lines[8].trim().parse::<usize>().unwrap_or_default();
                    let timeline_h = lines[9].trim().parse::<usize>().unwrap_or_default();
                    if fps > 0.0 && frame_count > 0 && duration_s > 0.0 && !file_path.is_empty() {
                        let info = Self {
                            file_path: file_path.to_string(),
                            fps,
                            duration_s,
                            frame_count,
                            width: *resolution.get(0).unwrap_or(&0),
                            height: *resolution.get(1).unwrap_or(&0),
                            pixel_aspect_ratio: par.to_string(),
                            project_path: gyroflow_plugin_base::GyroflowPluginBase::get_project_path(&file_path),
                            mismatch_mode,
                            timeline_w,
                            timeline_h,
                            use_custom_settings,
                        };
                        log::debug!("{info:#?}");
                        *current_file_info.lock() = Some(info);
                        current_file_info_pending.store(true, SeqCst);

                        // Trigger render
                        let script = "c = Resolve():GetProjectManager():GetCurrentProject():GetCurrentTimeline():GetCurrentVideoItem();
                                          c:SetProperty('FlipX', c:GetProperty('FlipX'))";
                        let _ = cmd.args(["-x", &script]).spawn();
                    }
                } else {
                    log::debug!("fuscript stdout: {stdout}");
                    log::debug!("fuscript stderr: {stderr}");
                    if !silent {
                        rfd::MessageDialog::new()
                            .set_title("Failed to query current video file path.")
                            .set_description(FAILED_MSG)
                            .set_level(rfd::MessageLevel::Warning)
                            .show();
                    }
                }
            }
        });
    }

    fn parse_duration(v: &str, fps: f64) -> f64 {
        let parts = v.replace(";", ":").split(':').filter_map(|x| x.parse::<f64>().ok()).collect::<Vec<_>>();
        if parts.len() == 4 {
            parts[0] * 60.0 * 60.0 + // h
            parts[1] * 60.0 + // m
            parts[2] + // s
            parts[3] / fps.max(1.0)
        } else {
            0.0
        }
    }
}
