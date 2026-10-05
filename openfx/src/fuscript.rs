use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use gyroflow_plugin_base::parking_lot::Mutex;
use gyroflow_plugin_base::rfd;
use crate::drt_state::{ DrtOutcome, DrtSharedState, QueryContext, SizingSource };

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
// Lines 11-12 (plugins-host-timeline-trim): the item's source-domain in/out
// frames. Method existence is probed before calling (older Resolve lacks
// GetSourceStartFrame — an unguarded call would nil-error and take the whole
// 12-line query down with it). The GetLeftOffset+GetDuration fallback is
// timeline-domain for the duration part, so it is only used when the source
// accessors are absent; empty strings keep the line count on any failure.
const CORE_QUERY_LUA: &str = "proj = Resolve():GetProjectManager():GetCurrentProject();\
                              tl = proj:GetCurrentTimeline();\
                              it = tl:GetCurrentVideoItem();\
                              p = it:GetMediaPoolItem():GetClipProperty();\
                              print(p['FPS']);print(p['Frames']);print(p['Duration']);print(p['PAR']);print(p['Resolution']);print(p['File Path']);\
                              ucs = tl:GetSetting('useCustomSettings') or '';\
                              if ucs == '1' then mm = tl:GetSetting('timelineInputResMismatchBehavior') or ''; else mm = proj:GetSetting('timelineInputResMismatchBehavior') or ''; end;\
                              tw = ''; th = '';\
                              if ucs == '1' then tw = tl:GetSetting('timelineResolutionWidth') or ''; th = tl:GetSetting('timelineResolutionHeight') or ''; end;\
                              if tw == '' or th == '' then tw = proj:GetSetting('timelineResolutionWidth') or ''; th = proj:GetSetting('timelineResolutionHeight') or ''; end;\
                              print(ucs);print(mm);print(tw);print(th);\
                              ss = ''; se = '';\
                              if it.GetSourceStartFrame ~= nil then ss = it:GetSourceStartFrame() or ''; se = it:GetSourceEndFrame() or ''; end;\
                              if ss == '' and it.GetLeftOffset ~= nil then lo = it:GetLeftOffset(); du = it:GetDuration(); if lo ~= nil and du ~= nil then ss = lo; se = lo + du; end; end;\
                              print(ss);print(se);";

/// Fixed-protocol DRT export block appended after the core query. It reads the core script's
/// globals `tl`, `tw`, `th` and `mm`, plus the `GF_*` header globals.
const DRT_BLOCK_LUA: &str = r#"local function gf_hex(s) return (s:gsub('.', function(c) return string.format('%02x', c:byte()) end)) end
local gf_ok, gf_err = pcall(function()
    local name_hex = gf_hex(tl:GetName() or '')
    print('gf_tl_name_hex=' .. name_hex)
    local w, h = tonumber(tw) or 0, tonumber(th) or 0
    if GF_ALLOW_DRT ~= 1 or h <= w then print('gf_drt=skipped'); return end
    local key = name_hex .. '|' .. w .. '|' .. h .. '|' .. tostring(mm)
    if GF_WANT_DRT ~= 1 and key == GF_LAST_KEY then print('gf_drt=skipped'); return end
    local r = Resolve()
    if r.EXPORT_DRT == nil or tl.Export == nil then print('gf_drt=unsupported'); return end
    local t0 = bmd.gettime()
    local ok = tl:Export(GF_DRT_PATH, r.EXPORT_DRT)
    print(string.format('gf_drt_ms=%.3f', (bmd.gettime() - t0) * 1000))
    print(ok and 'gf_drt=exported' or 'gf_drt=failed:export-returned-false')
end)
if not gf_ok then print('gf_drt=failed:' .. (string.gsub(tostring(gf_err), '[%c]', ' '))) end"#;

/// Parameters of the DRT part of the query script.
#[derive(Debug, Clone, PartialEq)]
pub struct DrtScriptParams { pub allow: bool, pub want: bool, pub last_key: String, pub path: String }

impl DrtScriptParams {
    /// No export is allowed or requested; the DRT block only reports the timeline name.
    pub fn disabled() -> Self {
        Self { allow: false, want: false, last_key: String::new(), path: String::new() }
    }
}

/// Renders `s` as a single-quoted, pure-ASCII Lua string literal. Bytes outside 0x20..=0x7E, plus
/// `\` and `'`, are written as decimal escapes. Every escape uses exactly three digits, because
/// Lua 5.1 reads up to three digits after `\` and a shorter escape followed by an ASCII digit
/// would absorb it.
pub fn lua_ascii_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for &b in s.as_bytes() {
        if (0x20..=0x7E).contains(&b) && b != b'\\' && b != b'\'' {
            out.push(b as char);
        } else {
            out.push_str(&format!("\\{b:03}"));
        }
    }
    out.push('\'');
    out
}

/// Builds the full fuscript query: four `GF_*` header globals, the core query, then the DRT block.
pub fn build_query_script(drt: &DrtScriptParams) -> String {
    // The core text has no trailing newline, so the separator keeps the block's first statement
    // on its own line.
    format!(
        "GF_ALLOW_DRT = {}\nGF_WANT_DRT = {}\nGF_LAST_KEY = {}\nGF_DRT_PATH = {}\n{}\n{}",
        drt.allow as u8, drt.want as u8,
        lua_ascii_literal(&drt.last_key), lua_ascii_literal(&drt.path),
        CORE_QUERY_LUA, DRT_BLOCK_LUA
    )
}

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
    // `mismatch_mode` is the effective input sizing mode (`scaleToFit` / `scaleToCrop` /
    // `centerCrop` / `stretch`): on landscape and square timelines the raw
    // `timelineInputResMismatchBehavior` value, already disambiguated by `useCustomSettings`; on
    // vertical timelines the validated vertical field of the timeline's DRT export, or the raw
    // value when no validated one exists for the current timeline (see `drt_state::decide`).
    // `timeline_w`/`timeline_h` come from the project (or timeline override) resolution settings;
    // they are used by Stretch mode to set `stab.params.size` to the host buffer dimensions.
    // `use_custom_settings` is the raw timeline `useCustomSettings` value, kept for diagnostics.
    pub mismatch_mode: Option<String>,
    pub timeline_w: usize,
    pub timeline_h: usize,
    pub use_custom_settings: bool,

    // plugins-host-timeline-trim: the playhead item's source-domain in/out
    // bounds in frames at the clip's media fps (`GetSourceStartFrame` /
    // `GetSourceEndFrame`, with a `GetLeftOffset`+`GetDuration` fallback for
    // older Resolve). CLIP-LEVEL fields: published only in
    // `publish_clip_fields` mode — an expiry refresh resolves the PLAYHEAD's
    // item, which may not be the instance's clip. `None` = not reported
    // (older Resolve / Fusion page / lua fallback empty).
    pub source_start_frame: Option<f64>,
    pub source_end_frame: Option<f64>,

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
    // Explicit query (LoadCurrent). With `drt`, the published mode is decided against the shared
    // DRT state like every other query, but this query never exports: on a vertical timeline
    // without a validated value it only asks the next refresh to export.
    pub fn query(current_file_info: Arc<Mutex<Option<Self>>>, current_file_info_pending: Arc<AtomicBool>, drt: Option<Arc<DrtSharedState>>) {
        Self::query_inner(current_file_info, current_file_info_pending, false, None, true, None, drt.map(|state| (state, false, false, 0)));
    }

    // Refresh variant used by the expiry-driven path (openfx-mismatch-mode-refresh). Differs from
    // `query_silent` in two ways: it owns the caller's single-flight guard and releases it when the
    // query thread finishes (every path, including early returns and panics), and it publishes
    // ONLY the host-input-sizing fields — never the clip-level fields, never the pending flag.
    // See the publication block in `query_inner` for why that separation is load-bearing.
    // It is also the only query that may export the DRT of a vertical timeline: `forced` and
    // `ttl_ms` drive the export pacing in `DrtSharedState::begin_request`.
    pub fn query_refresh(
        current_file_info: Arc<Mutex<Option<Self>>>,
        current_file_info_pending: Arc<AtomicBool>,
        in_flight: Arc<AtomicBool>,
        failures: Arc<std::sync::atomic::AtomicU32>,
        drt: Arc<DrtSharedState>,
        forced: bool,
        ttl_ms: u64,
    ) {
        Self::query_inner(current_file_info, current_file_info_pending, true, Some(in_flight), false, Some(failures), Some((drt, true, forced, ttl_ms)));
    }

    // Silent variant: same query, but does not pop the rfd error dialog when fuscript fails.
    // Used by automatic triggers (CreateInstance, ReloadProject) where a failure is expected
    // on Resolve Free / non-Resolve hosts / compound clips and the user did not explicitly ask
    // for the query — we just want to populate `CurrentFileInfo` when it happens to be available
    // so the `HostInputSizing` Auto mode has fuscript data to consult. Like `query`, it never
    // exports the DRT.
    pub fn query_silent(current_file_info: Arc<Mutex<Option<Self>>>, current_file_info_pending: Arc<AtomicBool>, drt: Option<Arc<DrtSharedState>>) {
        Self::query_inner(current_file_info, current_file_info_pending, true, None, true, None, drt.map(|state| (state, false, false, 0)));
    }

    // `drt` is `(state, allow_export, forced, ttl_ms)`. Without it the raw API mode is published,
    // exactly as before the DRT state existed.
    fn query_inner(
        current_file_info: Arc<Mutex<Option<Self>>>,
        current_file_info_pending: Arc<AtomicBool>,
        silent: bool,
        in_flight: Option<Arc<AtomicBool>>,
        publish_clip_fields: bool,
        failures: Option<Arc<std::sync::atomic::AtomicU32>>,
        drt: Option<(Arc<DrtSharedState>, bool, bool, u64)>,
    ) {
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

            // Only the refresh path claims the process-wide export slot. The request is declared
            // after the guards above, so on every exit that does not hand it to the state (lua
            // error, fewer than 12 lines, spawn failure, panic) it is dropped first: the temp file
            // is deleted and the claim released before the single-flight guard is, so a re-armed
            // refresh never finds the claim still held.
            let (drt_state, allow_export) = match &drt {
                Some((state, allow, _, _)) => (Some(Arc::clone(state)), *allow),
                None => (None, false),
            };
            let request = match &drt {
                Some((state, true, forced, ttl_ms)) => state.begin_request(
                    std::time::Instant::now(), *ttl_ms, *forced, &std::env::temp_dir(), std::process::id()),
                _ => None,
            };
            let disabled = DrtScriptParams::disabled();
            let script = build_query_script(request.as_ref().map_or(&disabled, |req| &req.params));
            // Run the query with a deadline instead of `output()`.
            //
            // `output()` blocks forever, and fuscript reaches Resolve over IPC: while Resolve is
            // playing back, its main thread is saturated and the request is simply never
            // serviced. Live-observed 2026-07-26 — queries hung for >200 s during 60 fps
            // playback, and because the caller's stuck-guard reclamation re-arms on a timer, every
            // window leaked one more permanently hanging process (4 alive after ~3 minutes).
            //
            // Polling without draining the pipes cannot deadlock here: the query prints at most ~16 short
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
                    // A requested export may still be running inside Resolve: record the timeout
                    // cost so no export runs again until its interval has passed. Nothing is
                    // published, so there is no timeline reading to pass along.
                    if let Some(state) = &drt_state {
                        state.finish_unpublished(std::time::Instant::now(), request, DrtOutcome::TimedOut,
                            &QueryContext { name_hex: None, width: 0, height: 0, api_mode: None });
                    }
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
                // Accept the 12 core lines from the extended query. Older Resolve versions without
                // the extra settings keys still emit empty strings (`print('')`) so the line count
                // stays the same; only a true script failure produces fewer lines. Lines after the 12th
                // are optional `gf_`-prefixed DRT fields in order-independent format; unknown lines are
                // ignored. The first 12 lines keep their exact meaning.
                //
                // The `errors.is_empty()` rule also holds for queries that export the DRT: stderr
                // was observed empty across `Export` calls (3/3, Resolve 21.0.0.47).
                if errors.is_empty() && lines.len() >= 12 {
                    let fps = lines[0].parse::<f64>().unwrap_or_default();
                    let frame_count = lines[1].parse::<usize>().unwrap_or_default();
                    let duration_s = Self::parse_duration(lines[2], fps);
                    let par = lines[3];
                    let resolution = lines[4].split("x").filter_map(|x| x.parse::<usize>().ok()).collect::<Vec<_>>();
                    let file_path = replace_frame_count(lines[5]);
                    let use_custom_settings = lines[6].trim() == "1";
                    let mismatch_mode_raw = lines[7].trim();
                    // The raw API value; the published mode is the effective one decided below.
                    let api_mismatch_mode = if mismatch_mode_raw.is_empty() {
                        None
                    } else {
                        Some(mismatch_mode_raw.to_string())
                    };
                    let timeline_w = lines[8].trim().parse::<usize>().unwrap_or_default();
                    let timeline_h = lines[9].trim().parse::<usize>().unwrap_or_default();
                    let source_start_frame = lines[10].trim().parse::<f64>().ok();
                    let source_end_frame   = lines[11].trim().parse::<f64>().ok();
                    let drt_lines = parse_drt_lines(&lines[12..]);
                    let outcome = drt_outcome(allow_export, request.as_ref().map(|req| req.path.as_path()), &drt_lines, |path| std::fs::read(path));
                    // Per-export details, logged at debug once the export is settled. Never the
                    // bytes or the outcome itself: a DRT carries the user's media paths. The temp
                    // file's deletion is logged by its guard.
                    let export_details = match (&request, &outcome) {
                        (Some(req), DrtOutcome::Exported { export_ms, bytes }) => Some(match bytes {
                            Ok(bytes) => format!("DRT export took {export_ms:.1}ms, read {} bytes from {}", bytes.len(), req.path.display()),
                            Err(e) => format!("DRT export took {export_ms:.1}ms, could not read {}: {e}", req.path.display()),
                        }),
                        _ => None,
                    };
                    let drt_ctx = QueryContext {
                        name_hex: drt_lines.timeline_name_hex.as_deref(),
                        width: timeline_w,
                        height: timeline_h,
                        api_mode: api_mismatch_mode.as_deref(),
                    };
                    if fps > 0.0 && frame_count > 0 && duration_s > 0.0 && !file_path.is_empty() {
                        // Everything published below — both publication modes, the change test
                        // behind the forced re-render and the render-path mirror — sees only the
                        // effective mode. The raw API value stays in the state's log line.
                        let mismatch_mode = match &drt_state {
                            Some(state) => {
                                let decision = state.finish(std::time::Instant::now(), request, outcome, &drt_ctx);
                                // The info line below appears only when the decision changes;
                                // this one tells whether every single export validated.
                                if let Some(details) = &export_details {
                                    let source = match &decision.source {
                                        SizingSource::ApiFallback(reason) => format!("api-fallback reason={reason}"),
                                        source => source.label().to_string(),
                                    };
                                    log::debug!(target: "host_input_sizing", "host_input_sizing: {details}; source={source}");
                                }
                                if let Some(line) = &decision.log_line {
                                    log::info!(target: "host_input_sizing", "{line}");
                                }
                                decision.effective_mode
                            }
                            None => api_mismatch_mode,
                        };
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
                            source_start_frame,
                            source_end_frame,
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
                                        || prev.source_start_frame  != info.source_start_frame
                                        || prev.source_end_frame    != info.source_end_frame
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
                                        // Clip-level fields stay empty in a refresh seed
                                        // (playhead contract) — same as file_path above.
                                        source_start_frame: None,
                                        source_end_frame: None,
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
                    } else if let Some(state) = &drt_state {
                        // No usable clip under the playhead (e.g. a compound clip has no file
                        // path): nothing is published, exactly as before, but an export that ran
                        // is still accounted for in the pacing and the validated value.
                        state.finish_unpublished(std::time::Instant::now(), request, outcome, &drt_ctx);
                        if let Some(details) = &export_details {
                            log::debug!(target: "host_input_sizing", "host_input_sizing: {details}; not published (no usable clip under the playhead)");
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

#[derive(Debug, Clone, PartialEq, Default)]
pub enum DrtLineStatus {
    #[default]
    Absent,
    Exported,
    Skipped,
    Unsupported,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DrtLines {
    pub status: DrtLineStatus,
    pub export_ms: Option<f64>,
    pub timeline_name_hex: Option<String>,
}

pub fn parse_drt_lines(extra: &[&str]) -> DrtLines {
    let mut result = DrtLines::default();

    for line in extra {
        if let Some(eq_pos) = line.find('=') {
            let key = &line[..eq_pos];
            let mut value = &line[eq_pos + 1..];

            // Trim trailing \r from values (Windows pipes)
            if value.ends_with('\r') {
                value = &value[..value.len() - 1];
            }

            match key {
                "gf_drt" => {
                    if value == "exported" {
                        result.status = DrtLineStatus::Exported;
                    } else if value == "skipped" {
                        result.status = DrtLineStatus::Skipped;
                    } else if value == "unsupported" {
                        result.status = DrtLineStatus::Unsupported;
                    } else if let Some(msg) = value.strip_prefix("failed:") {
                        result.status = DrtLineStatus::Failed(msg.to_string());
                    }
                }
                "gf_drt_ms" => {
                    if let Ok(ms) = value.parse::<f64>() {
                        result.export_ms = Some(ms);
                    }
                }
                "gf_tl_name_hex" => {
                    result.timeline_name_hex = Some(value.to_string());
                }
                _ => {
                    // Ignore unknown keys
                }
            }
        }
    }

    result
}

/// Maps the DRT lines of a completed query to the outcome recorded in the shared DRT state.
///
/// Explicit queries (`allow_export = false`) never export: their script carries
/// `GF_ALLOW_DRT = 0`, so whatever the Lua block printed they report `NotRequested`, which lets
/// the state request an export from the next refresh. On the refresh path a skip is a skip
/// whether or not this query claimed the export slot, and only a claimed export is trusted as an
/// export: `read` loads the requested file and is called for nothing else. A missing duration
/// is passed on as NaN, which the state records as the worst-case cost.
fn drt_outcome(
    allow_export: bool,
    request_path: Option<&std::path::Path>,
    lines: &DrtLines,
    read: impl FnOnce(&std::path::Path) -> std::io::Result<Vec<u8>>,
) -> DrtOutcome {
    if !allow_export {
        return DrtOutcome::NotRequested;
    }
    match &lines.status {
        DrtLineStatus::Exported => match request_path {
            Some(path) => DrtOutcome::Exported { export_ms: lines.export_ms.unwrap_or(f64::NAN), bytes: read(path) },
            // Lua only exports to the path of a request, so this cannot happen.
            None => DrtOutcome::Skipped,
        },
        DrtLineStatus::Skipped | DrtLineStatus::Absent => DrtOutcome::Skipped,
        DrtLineStatus::Unsupported => DrtOutcome::Unsupported,
        DrtLineStatus::Failed(msg) => DrtOutcome::Failed(msg.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_drt_lines_absent_for_core_only() {
        assert_eq!(parse_drt_lines(&[]), DrtLines::default());
    }

    #[test]
    fn parse_drt_lines_exported() {
        assert_eq!(
            parse_drt_lines(&["gf_tl_name_hex=6162", "gf_drt_ms=72.500", "gf_drt=exported"]),
            DrtLines {
                status: DrtLineStatus::Exported,
                export_ms: Some(72.5),
                timeline_name_hex: Some("6162".into())
            }
        );
    }

    #[test]
    fn parse_drt_lines_failed_keeps_message() {
        assert_eq!(
            parse_drt_lines(&["gf_drt=failed:attempt to call nil"]).status,
            DrtLineStatus::Failed("attempt to call nil".into())
        );
    }

    #[test]
    fn parse_drt_lines_ignores_unknown_and_order() {
        let l = parse_drt_lines(&["gf_future=1", "gf_drt=skipped", "junk", "gf_tl_name_hex=00"]);
        assert_eq!((l.status, l.timeline_name_hex.as_deref()), (DrtLineStatus::Skipped, Some("00")));
    }

    // Lua 5.1 string unescape for what `lua_ascii_literal` can emit: `\ddd` (1-3 decimal digits),
    // `\\` and `\'`; every other byte is copied as is.
    fn lua51_unescape(s: &str) -> Vec<u8> {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] != b'\\' { out.push(b[i]); i += 1; continue; }
            i += 1;
            if b[i].is_ascii_digit() {
                let mut v = 0u32;
                let mut n = 0;
                while n < 3 && i < b.len() && b[i].is_ascii_digit() { v = v * 10 + (b[i] - b'0') as u32; i += 1; n += 1; }
                out.push(v as u8);
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        out
    }

    #[test]
    fn lua_literal_is_pure_ascii_and_round_trips() {
        for s in ["C:/Users/张三/AppData/Local/Temp/x.drt", "/var/folders/ab/T/한국어/x.drt", "clip 🎬", "it's a \\ test", ""] {
            let lit = lua_ascii_literal(s);
            assert!(lit.is_ascii() && lit.starts_with('\'') && lit.ends_with('\''));
            assert_eq!(lua51_unescape(&lit[1..lit.len() - 1]), s.as_bytes());
        }
    }

    #[test]
    fn lua_literal_non_ascii_byte_before_digit_round_trips() {
        let lit = lua_ascii_literal("张1");
        assert!(lit.is_ascii());
        assert_eq!(lua51_unescape(&lit[1..lit.len() - 1]), "张1".as_bytes());
    }

    #[test]
    fn query_script_keeps_core_query_verbatim() {
        let s = build_query_script(&DrtScriptParams::disabled());
        assert!(s.contains(CORE_QUERY_LUA) && s.contains(DRT_BLOCK_LUA));
        assert!(s.starts_with("GF_ALLOW_DRT = 0\nGF_WANT_DRT = 0\nGF_LAST_KEY = ''\nGF_DRT_PATH = ''\n"));
    }

    /// A comparable rendering of an outcome; `DrtOutcome` has no `PartialEq` (it carries an
    /// `io::Error`).
    fn outcome_label(outcome: &DrtOutcome) -> String {
        match outcome {
            DrtOutcome::NotRequested => "not-requested".into(),
            DrtOutcome::Skipped => "skipped".into(),
            DrtOutcome::Unsupported => "unsupported".into(),
            DrtOutcome::Failed(msg) => format!("failed:{msg}"),
            DrtOutcome::TimedOut => "timed-out".into(),
            DrtOutcome::Exported { export_ms, bytes: Ok(bytes) } => format!("exported:{export_ms}:{}", bytes.len()),
            DrtOutcome::Exported { export_ms, bytes: Err(e) } => format!("exported:{export_ms}:err:{e}"),
        }
    }

    fn drt_lines_with(status: DrtLineStatus, export_ms: Option<f64>) -> DrtLines {
        DrtLines { status, export_ms, timeline_name_hex: Some("6162".into()) }
    }

    fn every_status() -> [DrtLineStatus; 5] {
        [DrtLineStatus::Absent, DrtLineStatus::Exported, DrtLineStatus::Skipped, DrtLineStatus::Unsupported, DrtLineStatus::Failed("x".into())]
    }

    #[test]
    fn drt_outcome_explicit_paths_are_never_an_export() {
        let path = std::path::Path::new("gyroflow-ofx-sizing-1-0.drt");
        for status in every_status() {
            for request in [None, Some(path)] {
                let outcome = drt_outcome(false, request, &drt_lines_with(status.clone(), Some(72.5)), |_| {
                    panic!("an explicit query never reads a DRT file")
                });
                assert_eq!(outcome_label(&outcome), "not-requested", "{status:?} request={request:?}");
            }
        }
    }

    #[test]
    fn drt_outcome_refresh_with_request() {
        let path = std::path::Path::new("gyroflow-ofx-sizing-1-0.drt");
        let read_ok = |p: &std::path::Path| { assert_eq!(p, path); Ok(vec![1, 2, 3]) };
        let read_err = |_: &std::path::Path| Err(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
        let label = |status: DrtLineStatus, export_ms: Option<f64>| {
            outcome_label(&drt_outcome(true, Some(path), &drt_lines_with(status, export_ms), read_ok))
        };

        assert_eq!(label(DrtLineStatus::Exported, Some(72.5)), "exported:72.5:3");
        // A missing duration is recorded as an unknown (worst-case) cost.
        assert_eq!(label(DrtLineStatus::Exported, None), "exported:NaN:3");
        assert_eq!(
            outcome_label(&drt_outcome(true, Some(path), &drt_lines_with(DrtLineStatus::Exported, Some(80.0)), read_err)),
            "exported:80:err:gone"
        );
        assert_eq!(label(DrtLineStatus::Skipped, None), "skipped");
        assert_eq!(label(DrtLineStatus::Absent, None), "skipped");
        assert_eq!(label(DrtLineStatus::Unsupported, None), "unsupported");
        assert_eq!(label(DrtLineStatus::Failed("export-returned-false".into()), Some(3.0)), "failed:export-returned-false");
    }

    #[test]
    fn drt_outcome_refresh_without_request() {
        let label = |status: DrtLineStatus| {
            outcome_label(&drt_outcome(true, None, &drt_lines_with(status, Some(72.5)), |_| {
                panic!("no request, no file to read")
            }))
        };
        // Lua exports only to a requested path, so an `exported` line without a request is not
        // trusted as an export.
        assert_eq!(label(DrtLineStatus::Exported), "skipped");
        assert_eq!(label(DrtLineStatus::Skipped), "skipped");
        assert_eq!(label(DrtLineStatus::Absent), "skipped");
        assert_eq!(label(DrtLineStatus::Unsupported), "unsupported");
        assert_eq!(label(DrtLineStatus::Failed("boom".into())), "failed:boom");
    }

    #[test]
    fn query_script_embeds_flags_and_escaped_path() {
        let p = DrtScriptParams { allow: true, want: true, last_key: "6162|1080|1920|scaleToFit".into(), path: "C:/Users/张三/Temp/gyroflow-ofx-sizing-1-2.drt".into() };
        let s = build_query_script(&p);
        assert!(s.is_ascii() && !s.contains('张'));
        assert!(s.contains("GF_ALLOW_DRT = 1\nGF_WANT_DRT = 1\nGF_LAST_KEY = '6162|1080|1920|scaleToFit'\n"));
        assert!(s.contains(&format!("GF_DRT_PATH = {}\n", lua_ascii_literal(&p.path))));
    }
}
