use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use gyroflow_plugin_base::parking_lot::Mutex;
use gyroflow_plugin_base::rfd;

const FAILED_MSG: &str = "This feature relies on external scripting and is only available in paid Resolve Studio. You have to allow executing scripts:\n
Set \"Preferences -> General -> External scripting using\" to \"Local\".\n\n
It must be the currently displayed video on the timeline.\n
It is also impossible to query file path on a compound clip.\n\nIn any case, you can just select the video or project file using the \"Browse\" button.";

/// Wall-clock budget for one fuscript query before the child is killed.
///
/// A warm query is ~85 ms and a cold one ~270 ms, so this is generous by more than an order of
/// magnitude. It is not a latency target — it exists because the request can hang indefinitely
/// when Resolve's main thread is saturated (playback, export), and an unkilled child then stays
/// resident for the rest of the session.
const QUERY_TIMEOUT_MS: u64 = 5_000;

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

    // When the fuscript query that produced these host-input-sizing fields completed.
    // `None` means they did not come from a query at all (restored from the per-node hidden
    // field), which makes the value immediately eligible for refresh. Instances synthesized
    // from the plugin-global cache inherit that entry's timestamp, so the render-path mirror
    // can tell "a new query landed" from "the same value is being re-read every frame" — the
    // latter must not keep resetting the cache's freshness window.
    pub queried_at: Option<std::time::Instant>,
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
        Self::query_inner(current_file_info, current_file_info_pending, false, None, true, None);
    }

    // Refresh variant used by the expiry-driven path (openfx-mismatch-mode-refresh). Differs from
    // `query_silent` in two ways: it owns the caller's single-flight guard and releases it when the
    // query thread finishes (every path, including early returns and panics), and it publishes
    // ONLY the host-input-sizing fields — never the clip-level fields, never the pending flag.
    // See the publication block in `query_inner` for why that separation is load-bearing.
    pub fn query_refresh(
        current_file_info: Arc<Mutex<Option<Self>>>,
        current_file_info_pending: Arc<AtomicBool>,
        in_flight: Arc<AtomicBool>,
        failures: Arc<std::sync::atomic::AtomicU32>,
    ) {
        Self::query_inner(current_file_info, current_file_info_pending, true, Some(in_flight), false, Some(failures));
    }

    // Silent variant: same query, but does not pop the rfd error dialog when fuscript fails.
    // Used by automatic triggers (CreateInstance, ReloadProject) where a failure is expected
    // on Resolve Free / non-Resolve hosts / compound clips and the user did not explicitly ask
    // for the query — we just want to populate `CurrentFileInfo` when it happens to be available
    // so the `HostInputSizing` Auto mode has fuscript data to consult.
    pub fn query_silent(current_file_info: Arc<Mutex<Option<Self>>>, current_file_info_pending: Arc<AtomicBool>) {
        Self::query_inner(current_file_info, current_file_info_pending, true, None, true, None);
    }

    fn query_inner(current_file_info: Arc<Mutex<Option<Self>>>, current_file_info_pending: Arc<AtomicBool>, silent: bool, in_flight: Option<Arc<AtomicBool>>, publish_clip_fields: bool, failures: Option<Arc<std::sync::atomic::AtomicU32>>) {
        std::thread::spawn(move || {
            // Releases the caller's single-flight guard on every exit path — parse failure,
            // fuscript spawn failure, early return, panic. A leaked guard would permanently
            // disable the expiry-driven refresh for the rest of the session.
            struct InFlightGuard(Option<Arc<AtomicBool>>);
            impl Drop for InFlightGuard {
                fn drop(&mut self) {
                    if let Some(flag) = &self.0 { flag.store(false, SeqCst); }
                }
            }
            let _in_flight_guard = InFlightGuard(in_flight);

            // Consecutive-failure counter driving the caller's retry back-off. Any exit that did
            // not publish a result counts as a failure: a timeout (Resolve busy), a lua error
            // (playhead parked on a title / gap / compound clip), a spawn failure. Without the
            // back-off those states retry every window forever — during playback that is one
            // spawned-and-killed process per window for as long as playback lasts.
            struct FailureTracker { counter: Option<Arc<std::sync::atomic::AtomicU32>>, succeeded: bool }
            impl Drop for FailureTracker {
                fn drop(&mut self) {
                    if let Some(c) = &self.counter {
                        if self.succeeded { c.store(0, SeqCst); } else { c.fetch_add(1, SeqCst); }
                    }
                }
            }
            let mut failure_tracker = FailureTracker { counter: failures, succeeded: false };

            let mut cmd = std::process::Command::new(Self::get_fuscript().unwrap());
            #[cfg(target_os = "windows")]
            { use std::os::windows::process::CommandExt; cmd.creation_flags(0x08000000); } // CREATE_NO_WINDOW

            // Extended query: the original 6 lines (FPS, Frames, Duration, PAR, Resolution, File Path)
            // come first to preserve the pre-existing parse-by-line-count expectation. The next 4
            // lines carry the host-input-sizing setting: useCustomSettings (timeline-level toggle),
            // the timeline override or project default for `timelineInputResMismatchBehavior`, and
            // timelineResolutionWidth/Height (used as `stab.params.size` in Stretch and for the
            // FillCrop/CenterCrop crop geometry). The resolution honors the same timeline
            // custom-settings override as the mismatch mode: a custom timeline can have a different
            // resolution than the project (e.g. a portrait 1080x1920 timeline in a 1920x1080
            // project), and the project-level read returned the wrong dimensions there. Empty
            // timeline-level values (older Resolve / missing keys) fall back to the project read.
            // Empty-string fallbacks (older Resolve versions / missing keys) keep the line count.
            //
            // Host read-path notes (re-verified live on Resolve 21.0.0.47, 2026-07-26 — an
            // earlier comment here claimed the opposite and was wrong):
            //  - The single-key form `GetSetting('key')` used below reflects the user's edit as
            //    soon as the settings dialog is saved, at BOTH project and timeline level. There
            //    is no need to restart Resolve or toggle "Use Project Settings".
            //  - Do NOT switch this to the no-argument `GetSetting()` dump form to look for the
            //    timeline override: on a timeline that form returns the project-level value set
            //    (or, once custom settings are on, only the overridden subset), so the effective
            //    value cannot be found in it. The original investigation searched that dump,
            //    found nothing, and wrongly concluded the host had no live read path.
            //  - The snapshot can carry cross-session residue until the first dialog save of the
            //    session (observed: a 6-day-old timeline resolution). The plugin's freshness
            //    window re-reads periodically, which makes that self-correcting.
            //  - Per-clip overrides (Inspector > Retime and Scaling > Scaling) are NOT visible
            //    here: they live in `timelineItem:GetProperty()['Scaling']` and never move
            //    `timelineInputResMismatchBehavior`. Known gap, tracked separately.
            let script = "proj = Resolve():GetProjectManager():GetCurrentProject();\
                              tl = proj:GetCurrentTimeline();\
                              p = tl:GetCurrentVideoItem():GetMediaPoolItem():GetClipProperty();\
                              print(p['FPS']);print(p['Frames']);print(p['Duration']);print(p['PAR']);print(p['Resolution']);print(p['File Path']);\
                              ucs = tl:GetSetting('useCustomSettings') or '';\
                              if ucs == '1' then mm = tl:GetSetting('timelineInputResMismatchBehavior') or ''; else mm = proj:GetSetting('timelineInputResMismatchBehavior') or ''; end;\
                              tw = ''; th = '';\
                              if ucs == '1' then tw = tl:GetSetting('timelineResolutionWidth') or ''; th = tl:GetSetting('timelineResolutionHeight') or ''; end;\
                              if tw == '' or th == '' then tw = proj:GetSetting('timelineResolutionWidth') or ''; th = proj:GetSetting('timelineResolutionHeight') or ''; end;\
                              print(ucs);print(mm);print(tw);print(th);";
            // Run the query with a deadline instead of `output()`.
            //
            // `output()` blocks forever, and fuscript reaches Resolve over IPC: while Resolve is
            // playing back, its main thread is saturated and the request is simply never
            // serviced. Live-observed 2026-07-26 — queries hung for >200 s during 60 fps
            // playback, and because the caller's stuck-guard reclamation re-arms on a timer, every
            // window leaked one more permanently hanging process (4 alive after ~3 minutes).
            //
            // Polling without draining the pipes cannot deadlock here: the query prints ten short
            // lines, orders of magnitude below the pipe buffer.
            let spawned = cmd
                .args(["-q", "-l", "lua", "-x", &script])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn();
            if let Ok(mut child) = spawned {
                let deadline = std::time::Instant::now()
                    + std::time::Duration::from_millis(QUERY_TIMEOUT_MS);
                let timed_out = loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break false,
                        Ok(None) => {
                            if std::time::Instant::now() >= deadline {
                                let _ = child.kill();
                                let _ = child.wait();
                                break true;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(20));
                        }
                        Err(_) => break false,
                    }
                };
                if timed_out {
                    log::warn!(target: "host_input_sizing",
                        "fuscript query exceeded {QUERY_TIMEOUT_MS}ms and was killed — Resolve is \
                         most likely busy (playback / export); the host input sizing mode keeps \
                         its previous value and the next window retries");
                    return;
                }
                let mut stdout = String::new();
                let mut stderr = String::new();
                {
                    use std::io::Read;
                    if let Some(mut pipe) = child.stdout.take() { let _ = pipe.read_to_string(&mut stdout); }
                    if let Some(mut pipe) = child.stderr.take() { let _ = pipe.read_to_string(&mut stderr); }
                }
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
                            queried_at: Some(std::time::Instant::now()),
                        };
                        log::debug!("{info:#?}");

                        // Two publication modes.
                        //
                        // `publish_clip_fields = true` (LoadCurrent / ReloadProject): replace the
                        // whole record and raise the pending flag. That is what makes the render
                        // path adopt the newly resolved clip and project path — the point of those
                        // two user actions.
                        //
                        // `publish_clip_fields = false` (expiry-driven refresh): update ONLY the
                        // host-input-sizing fields, and never raise the pending flag. A periodic
                        // refresh must not behave like "the user asked to load this clip". The lua
                        // script resolves `GetCurrentVideoItem()` — the PLAYHEAD's clip, not the
                        // clip belonging to the instance that armed the query — and
                        // `check_pending_file_info` unconditionally rewrites `ProjectPath` from it.
                        // `ProjectPath` is the first component of the stabilization cache key, and
                        // `param_changed` calls `clear_stab` for it regardless of `user_edited`, so
                        // wiring that to a timer meant a full project re-import every TTL window,
                        // and on a multi-clip timeline it silently adopted another clip's project.
                        let changed = if publish_clip_fields {
                            let changed = match current_file_info.lock().as_ref() {
                                Some(prev) => {
                                    prev.mismatch_mode          != info.mismatch_mode
                                        || prev.timeline_w          != info.timeline_w
                                        || prev.timeline_h          != info.timeline_h
                                        || prev.use_custom_settings != info.use_custom_settings
                                        || prev.file_path           != info.file_path
                                        || prev.project_path        != info.project_path
                                }
                                None => true,
                            };
                            *current_file_info.lock() = Some(info);
                            current_file_info_pending.store(true, SeqCst);
                            changed
                        } else {
                            let mut lock = current_file_info.lock();
                            match lock.as_mut() {
                                Some(prev) => {
                                    let changed = prev.mismatch_mode        != info.mismatch_mode
                                        || prev.timeline_w          != info.timeline_w
                                        || prev.timeline_h          != info.timeline_h
                                        || prev.use_custom_settings != info.use_custom_settings;
                                    prev.mismatch_mode       = info.mismatch_mode;
                                    prev.timeline_w          = info.timeline_w;
                                    prev.timeline_h          = info.timeline_h;
                                    prev.use_custom_settings = info.use_custom_settings;
                                    prev.queried_at          = info.queried_at;
                                    changed
                                }
                                None => {
                                    // Nothing published for this instance yet. Seed the
                                    // host-sizing fields only and leave the clip-level fields at
                                    // their empty defaults, so nothing downstream mistakes this
                                    // for a resolved clip.
                                    *lock = Some(Self {
                                        file_path: String::new(),
                                        project_path: None,
                                        fps: 0.0,
                                        duration_s: 0.0,
                                        frame_count: 0,
                                        width: 0,
                                        height: 0,
                                        pixel_aspect_ratio: String::new(),
                                        mismatch_mode: info.mismatch_mode,
                                        timeline_w: info.timeline_w,
                                        timeline_h: info.timeline_h,
                                        use_custom_settings: info.use_custom_settings,
                                        queried_at: info.queried_at,
                                    });
                                    true
                                }
                            }
                        };

                        failure_tracker.succeeded = true;

                        // Force Resolve to re-render so a changed mode reaches the screen.
                        //
                        // Build a FRESH Command: `Command::args` appends, so reusing `cmd` would
                        // spawn `-q -l lua -x <query> -x <flipx>` and re-run the query instead of
                        // triggering the redraw. Reap the child on a helper thread — `Child` does
                        // not reap on drop, and under a periodic refresh macOS/Linux would
                        // otherwise accumulate one zombie per window for the whole session.
                        if changed {
                            let script = "c = Resolve():GetProjectManager():GetCurrentProject():GetCurrentTimeline():GetCurrentVideoItem();
                                              c:SetProperty('FlipX', c:GetProperty('FlipX'))";
                            if let Some(exe) = Self::get_fuscript() {
                                let mut trigger = std::process::Command::new(exe);
                                #[cfg(target_os = "windows")]
                                { use std::os::windows::process::CommandExt; trigger.creation_flags(0x08000000); }
                                // Piped so the reaper can surface the lua error text. Reading the
                                // pipes only AFTER exit/kill cannot deadlock: the trigger prints at
                                // most a few short lines, orders of magnitude below the pipe buffer.
                                let spawned = trigger.args(["-q", "-l", "lua", "-x", script])
                                    .stdout(std::process::Stdio::piped())
                                    .stderr(std::process::Stdio::piped())
                                    .spawn();
                                match spawned {
                                    Ok(mut child) => {
                                    // Same deadline as the query itself. This trigger goes through
                                    // the same Resolve IPC endpoint, so it hangs under exactly the
                                    // same conditions — a plain `wait()` here would leak both the
                                    // process and the reaper thread for the rest of the session.
                                    //
                                    // Failure is logged but never retried: the next natural render
                                    // adopts the corrected cache value regardless. The log exists
                                    // because the lua side fails for the same playhead-on-a-gap /
                                    // title reasons as the query itself, and a silent failure here
                                    // means "cache corrected but the screen kept the stale frame"
                                    // with zero diagnostics (openfx-mismatch-switch-refresh).
                                    std::thread::spawn(move || {
                                        let deadline = std::time::Instant::now()
                                            + std::time::Duration::from_millis(QUERY_TIMEOUT_MS);
                                        let mut timed_out = false;
                                        let status = loop {
                                            match child.try_wait() {
                                                Ok(Some(status)) => break Some(status),
                                                Ok(None) => {
                                                    if std::time::Instant::now() >= deadline {
                                                        let _ = child.kill();
                                                        let _ = child.wait();
                                                        timed_out = true;
                                                        break None;
                                                    }
                                                    std::thread::sleep(std::time::Duration::from_millis(50));
                                                }
                                                Err(_) => break None,
                                            }
                                        };
                                        let mut stderr = String::new();
                                        if let Some(mut pipe) = child.stderr.take() {
                                            use std::io::Read;
                                            let _ = pipe.read_to_string(&mut stderr);
                                        }
                                        let lua_error = stderr.trim().lines()
                                            .filter(|line| !is_missing_python2(line))
                                            .collect::<Vec<_>>()
                                            .join(" | ");
                                        match status {
                                            Some(st) if st.success() && lua_error.is_empty() => {}
                                            Some(st) => log::warn!(target: "host_input_sizing",
                                                "forced re-render trigger failed (exit={:?}, stderr: {lua_error}) — the corrected mode reaches the screen on the next natural render",
                                                st.code()),
                                            None if timed_out => log::warn!(target: "host_input_sizing",
                                                "forced re-render trigger exceeded {QUERY_TIMEOUT_MS}ms and was killed — Resolve is most likely busy; the corrected mode reaches the screen on the next natural render"),
                                            None => log::warn!(target: "host_input_sizing",
                                                "forced re-render trigger could not be awaited (stderr: {lua_error})"),
                                        }
                                    });
                                    }
                                    Err(e) => log::warn!(target: "host_input_sizing",
                                        "forced re-render trigger failed to spawn: {e}"),
                                }
                            }
                        }
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
