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

/// How long the query thread waits for the stdout of a query killed on timeout. The pipe can
/// stay open when another process still holds its write end; an incomplete read counts as "the
/// export had not started".
const TIMEOUT_STDOUT_READ_MS: u64 = 500;

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
    print('gf_drt_begin=1')
    if io ~= nil and io.flush ~= nil then io.flush() end
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
            // error in the core query, output not framed as 12 core lines, spawn failure, panic)
            // it is dropped first: the temp file is deleted and the claim released before the
            // single-flight guard is, so a re-armed refresh never finds the claim still held.
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
                    // A requested export that had started may still be running inside Resolve:
                    // record the timeout cost so no export runs again until its interval has
                    // passed. The flushed `gf_drt_begin` marker in what already reached the pipe
                    // tells whether it had started; a query killed before it (Resolve busy in the
                    // core query, e.g. during playback) releases the request at no cost. Nothing
                    // is published, so there is no timeline reading to pass along.
                    if let Some(state) = &drt_state {
                        let outcome = match &request {
                            Some(_) => {
                                let partial_stdout = child.stdout.take().and_then(|pipe| {
                                    read_to_end_within(pipe, std::time::Duration::from_millis(TIMEOUT_STDOUT_READ_MS))
                                });
                                timed_out_outcome(partial_stdout.as_deref())
                            }
                            // Without a claimed slot no export can have started; nothing is recorded.
                            None => DrtOutcome::TimedOut,
                        };
                        state.finish_unpublished(std::time::Instant::now(), request, outcome,
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
                // The 12 core lines from the extended query come first. Older Resolve versions
                // without the extra settings keys still emit empty strings (`print('')`) so the
                // line count stays the same; only a true script failure produces fewer lines. The
                // core lines are the lines before the first `gf_` line and must number exactly 12;
                // from that line on come the `gf_`-prefixed DRT fields in order-independent
                // format, where unknown lines are ignored. The 12 core lines keep their exact
                // meaning.
                //
                // The `errors.is_empty()` rule also holds for queries that export the DRT: stderr
                // was observed empty across `Export` calls (3/3, Resolve 21.0.0.47).
                let framed = split_query_lines(&lines);
                // From here on `lines` are the 12 core lines.
                if errors.is_empty() && let Some((lines, drt_part)) = framed {
                    let fps = lines[0].parse::<f64>().unwrap_or_default();
                    let frame_count = lines[1].parse::<usize>().unwrap_or_default();
                    let duration_s = Self::parse_duration(lines[2], fps);
                    let par = lines[3];
                    let resolution = lines[4].split("x").filter_map(|x| x.parse::<usize>().ok()).collect::<Vec<_>>();
                    let file_path = replace_frame_count(lines[5]);
                    let use_custom_settings = lines[6].trim() == "1";
                    let source_start_frame = lines[10].trim().parse::<f64>().ok();
                    let source_end_frame   = lines[11].trim().parse::<f64>().ok();
                    // `export_log`: per-export details, logged at debug once the export is
                    // settled. The temp file's deletion is logged by its guard.
                    let (drt_lines, outcome, export_log) =
                        framed_drt_outcome(allow_export, request.as_ref().map(|req| req.path.as_path()), drt_part);
                    // Lines 8-10 (mode and timeline resolution) are read once, for the DRT state
                    // and for publishing alike.
                    let drt_ctx = query_context(lines, &drt_lines);
                    // The raw API value; the published mode is the effective one decided below.
                    let api_mismatch_mode = drt_ctx.api_mode.map(str::to_string);
                    let timeline_w = drt_ctx.width;
                    let timeline_h = drt_ctx.height;
                    if fps > 0.0 && frame_count > 0 && duration_s > 0.0 && !file_path.is_empty() {
                        // Everything published below — both publication modes, the change test
                        // behind the forced re-render and the render-path mirror — sees only the
                        // effective mode. The raw API value stays in the state's log line.
                        let mismatch_mode = match &drt_state {
                            Some(state) => {
                                let decision = state.finish(std::time::Instant::now(), request, outcome, &drt_ctx);
                                // The info line below appears only when the decision changes;
                                // this one tells whether every single export validated.
                                if let Some(details) = &export_log {
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
                        if let Some(details) = &export_log {
                            log::debug!(target: "host_input_sizing", "host_input_sizing: {details}; not published (no usable clip under the playhead)");
                        }
                    }
                } else {
                    // stderr carried errors although the output is framed: the DRT block ran,
                    // so an export may have run too. Account for it exactly as on the success
                    // path, but publish nothing; the failure handling below is unchanged.
                    if let (Some(state), Some((core, drt_part))) = (&drt_state, framed) {
                        let (drt_lines, outcome, export_log) =
                            framed_drt_outcome(allow_export, request.as_ref().map(|req| req.path.as_path()), drt_part);
                        state.finish_unpublished(std::time::Instant::now(), request, outcome, &query_context(core, &drt_lines));
                        if let Some(details) = &export_log {
                            log::debug!(target: "host_input_sizing", "host_input_sizing: {details}; not published (fuscript reported errors)");
                        }
                    }
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
    /// `gf_drt_begin=1`: the block reached the `Export` call. Printed and flushed before it, so
    /// it survives fuscript being killed during the export.
    pub export_started: bool,
}

/// The DRT lines of the query output: every line from the first `gf_` line on (empty when there
/// is none).
fn drt_part<'a, 'b>(lines: &'b [&'a str]) -> &'b [&'a str] {
    let start = lines.iter().position(|line| line.starts_with("gf_")).unwrap_or(lines.len());
    &lines[start..]
}

/// The outcome of a refresh query that held an export request and was killed on timeout.
/// `partial_stdout` is what fuscript had written to the pipe before it was killed, `None` when
/// it could not be read in time. Only an export that had started is charged (`TimedOut`); a
/// query killed before the marker, e.g. in the core query while Resolve is busy with playback,
/// is `Skipped`, which releases the request at no cost.
fn timed_out_outcome(partial_stdout: Option<&str>) -> DrtOutcome {
    let started = partial_stdout.is_some_and(|stdout| {
        let lines = stdout.trim().lines().collect::<Vec<_>>();
        parse_drt_lines(drt_part(&lines)).export_started
    });
    if started { DrtOutcome::TimedOut } else { DrtOutcome::Skipped }
}

/// Reads `pipe` to its end on a helper thread and waits at most `limit` for it. `None` on a
/// read error or when the read did not finish in time; the helper thread then ends whenever the
/// pipe closes. Invalid UTF-8 is replaced rather than failing the read.
fn read_to_end_within(mut pipe: impl std::io::Read + Send + 'static, limit: std::time::Duration) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = tx.send(pipe.read_to_end(&mut bytes).map(|_| bytes));
    });
    let bytes = rx.recv_timeout(limit).ok()?.ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
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
                "gf_drt_begin" => {
                    if value == "1" {
                        result.export_started = true;
                    }
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
/// the state request an export from the next refresh. On the refresh path only a query that
/// claimed the export slot can have attempted an export, so only its status is trusted as an
/// export attempt (`Exported`, `Unsupported`, `Failed`). Without a request every status is a
/// skip: a request-less `Failed` (e.g. `GetName` raising inside the block) must not touch the
/// pacing state, where it would end a timeout block early. `read` loads the requested file and
/// is called for nothing else. A missing duration of an exported DRT is passed on as NaN, which
/// the state records as the worst-case cost; a failed export carries its duration, if any, as is.
fn drt_outcome(
    allow_export: bool,
    request_path: Option<&std::path::Path>,
    lines: &DrtLines,
    read: impl FnOnce(&std::path::Path) -> std::io::Result<Vec<u8>>,
) -> DrtOutcome {
    if !allow_export {
        return DrtOutcome::NotRequested;
    }
    let Some(path) = request_path else { return DrtOutcome::Skipped };
    match &lines.status {
        DrtLineStatus::Exported => DrtOutcome::Exported { export_ms: lines.export_ms.unwrap_or(f64::NAN), bytes: read(path) },
        DrtLineStatus::Skipped | DrtLineStatus::Absent => DrtOutcome::Skipped,
        DrtLineStatus::Unsupported => DrtOutcome::Unsupported,
        DrtLineStatus::Failed(msg) => DrtOutcome::Failed { message: msg.clone(), export_ms: lines.export_ms },
    }
}

/// Number of core query lines (FPS through source end frame).
const CORE_LINE_COUNT: usize = 12;

/// Splits the query output into the core lines and the DRT lines. The core lines are the lines
/// before the first `gf_` line (all lines when there is none) and must number exactly
/// `CORE_LINE_COUNT`; the DRT lines are the rest. `None` when the framing is invalid.
fn split_query_lines<'a, 'b>(lines: &'b [&'a str]) -> Option<(&'b [&'a str], &'b [&'a str])> {
    let drt = drt_part(lines);
    let core = &lines[..lines.len() - drt.len()];
    (core.len() == CORE_LINE_COUNT).then_some((core, drt))
}

/// The timeline reading the DRT state needs from a framed query: the API mode (line 8, empty =
/// none) and the timeline resolution (lines 9-10) from the core lines, and the timeline name from
/// the DRT lines.
fn query_context<'a>(core: &[&'a str], drt_lines: &'a DrtLines) -> QueryContext<'a> {
    let api_mode = core[7].trim();
    QueryContext {
        name_hex: drt_lines.timeline_name_hex.as_deref(),
        width: core[8].trim().parse::<usize>().unwrap_or_default(),
        height: core[9].trim().parse::<usize>().unwrap_or_default(),
        api_mode: (!api_mode.is_empty()).then_some(api_mode),
    }
}

/// The DRT part of a framed query, computed the same way on every path that settles it: the
/// parsed DRT lines, the outcome (which reads the requested file when an export was reported)
/// and the per-export debug details.
fn framed_drt_outcome(allow_export: bool, request_path: Option<&std::path::Path>, drt_part: &[&str]) -> (DrtLines, DrtOutcome, Option<String>) {
    let drt_lines = parse_drt_lines(drt_part);
    let outcome = drt_outcome(allow_export, request_path, &drt_lines, |path| std::fs::read(path));
    let export_log = export_details(request_path, &outcome);
    (drt_lines, outcome, export_log)
}

/// The per-export debug details of a claimed export, successful or failed: duration (`none`
/// when Lua printed none), the byte count, read error or failure message, and the temp path.
/// `None` when no export was attempted. Never the bytes or the outcome itself: a DRT carries
/// the user's media paths.
fn export_details(request_path: Option<&std::path::Path>, outcome: &DrtOutcome) -> Option<String> {
    let path = request_path?.display();
    // An exported outcome carries a missing duration as NaN (see `drt_outcome`).
    let took = |export_ms: Option<f64>| match export_ms {
        Some(ms) if ms.is_finite() => format!("{ms:.1}ms"),
        _ => "none".to_string(),
    };
    match outcome {
        DrtOutcome::Exported { export_ms, bytes: Ok(bytes) } => Some(format!("DRT export took {}, read {} bytes from {path}", took(Some(*export_ms)), bytes.len())),
        DrtOutcome::Exported { export_ms, bytes: Err(e) } => Some(format!("DRT export took {}, could not read {path}: {e}", took(Some(*export_ms)))),
        DrtOutcome::Failed { message, export_ms } => Some(format!("DRT export failed: {message}; took {}, temp file {path}", took(*export_ms))),
        DrtOutcome::NotRequested | DrtOutcome::Skipped | DrtOutcome::Unsupported | DrtOutcome::TimedOut => None,
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
                timeline_name_hex: Some("6162".into()),
                export_started: false,
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

    #[test]
    fn parse_drt_lines_recognizes_export_begin_marker() {
        assert!(!DrtLines::default().export_started);
        assert!(!parse_drt_lines(&["gf_tl_name_hex=6162"]).export_started);
        let l = parse_drt_lines(&["gf_tl_name_hex=6162", "gf_drt_begin=1\r"]);
        assert!(l.export_started);
        assert_eq!(l.status, DrtLineStatus::Absent);
        assert!(!parse_drt_lines(&["gf_drt_begin=0"]).export_started);
    }

    #[test]
    fn query_script_marks_export_begin_before_timing() {
        for params in [DrtScriptParams::disabled(), DrtScriptParams { allow: true, want: true, last_key: String::new(), path: "x.drt".into() }] {
            let s = build_query_script(&params);
            // Printed and flushed right before the timed `Export` call, at the block's indentation.
            assert!(s.contains(concat!(
                "\n    print('gf_drt_begin=1')\n",
                "    if io ~= nil and io.flush ~= nil then io.flush() end\n",
                "    local t0 = bmd.gettime()\n",
            )));
            assert_eq!(s.matches("gf_drt_begin").count(), 1);
            // Only after every early return of the block.
            assert!(s.find("gf_drt=unsupported").unwrap() < s.find("gf_drt_begin=1").unwrap());
        }
    }

    #[test]
    fn timeout_charges_only_a_started_export() {
        let core = "25\n100\n00:00:04:00\n1\n1920x1080\nC:/clip.mov\n0\nscaleToFit\n1080\n1920\n0\n100\n";
        let started = format!("{core}gf_tl_name_hex=6162\ngf_drt_begin=1\n");
        let not_started = format!("{core}gf_tl_name_hex=6162\n");
        assert_eq!(outcome_label(&timed_out_outcome(Some(&started))), "timed-out");
        assert_eq!(outcome_label(&timed_out_outcome(Some(&started.replace('\n', "\r\n")))), "timed-out");
        // Killed in the core query (e.g. during playback) or before the marker: nothing started.
        assert_eq!(outcome_label(&timed_out_outcome(Some(&not_started))), "skipped");
        assert_eq!(outcome_label(&timed_out_outcome(Some("25\n100\n"))), "skipped");
        assert_eq!(outcome_label(&timed_out_outcome(Some(""))), "skipped");
        // The bounded read did not complete: counted as not started.
        assert_eq!(outcome_label(&timed_out_outcome(None)), "skipped");
    }

    /// A pipe whose read blocks until the test drops the sender, like a pipe still held open by
    /// another process.
    struct BlockingPipe(std::sync::mpsc::Receiver<()>);
    impl std::io::Read for BlockingPipe {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            let _ = self.0.recv();
            Ok(0)
        }
    }

    struct FailingPipe;
    impl std::io::Read for FailingPipe {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken"))
        }
    }

    #[test]
    fn killed_stdout_read_is_bounded() {
        let limit = std::time::Duration::from_millis(50);
        assert_eq!(
            read_to_end_within(std::io::Cursor::new(b"gf_drt_begin=1\n".to_vec()), limit).as_deref(),
            Some("gf_drt_begin=1\n")
        );
        assert_eq!(read_to_end_within(FailingPipe, limit), None);

        let (release, rx) = std::sync::mpsc::channel();
        let started = std::time::Instant::now();
        assert_eq!(read_to_end_within(BlockingPipe(rx), limit), None);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        drop(release);
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
            DrtOutcome::Failed { message, .. } => format!("failed:{message}"),
            DrtOutcome::TimedOut => "timed-out".into(),
            DrtOutcome::Exported { export_ms, bytes: Ok(bytes) } => format!("exported:{export_ms}:{}", bytes.len()),
            DrtOutcome::Exported { export_ms, bytes: Err(e) } => format!("exported:{export_ms}:err:{e}"),
        }
    }

    fn drt_lines_with(status: DrtLineStatus, export_ms: Option<f64>) -> DrtLines {
        DrtLines { status, export_ms, timeline_name_hex: Some("6162".into()), export_started: false }
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
        // Without a claimed export slot no export was attempted, so no status is trusted as an
        // export attempt: an `exported`, `unsupported` or `failed` line must not touch the
        // pacing state (a request-less `failed` would otherwise end a timeout block early).
        assert_eq!(label(DrtLineStatus::Exported), "skipped");
        assert_eq!(label(DrtLineStatus::Skipped), "skipped");
        assert_eq!(label(DrtLineStatus::Absent), "skipped");
        assert_eq!(label(DrtLineStatus::Unsupported), "skipped");
        assert_eq!(label(DrtLineStatus::Failed("boom".into())), "skipped");
    }

    #[test]
    fn drt_outcome_failed_carries_the_duration() {
        let path = std::path::Path::new("gyroflow-ofx-sizing-1-0.drt");
        for export_ms in [Some(3.0), None] {
            let lines = drt_lines_with(DrtLineStatus::Failed("export-returned-false".into()), export_ms);
            match drt_outcome(true, Some(path), &lines, |_| panic!("a failed export has no file to read")) {
                DrtOutcome::Failed { message, export_ms: got } => assert_eq!((message.as_str(), got), ("export-returned-false", export_ms)),
                other => panic!("expected a failed export, got {}", outcome_label(&other)),
            }
        }
    }

    #[test]
    fn export_details_cover_every_claimed_export() {
        let path = std::path::Path::new("gyroflow-ofx-sizing-1-0.drt");
        let exported = |export_ms, bytes| DrtOutcome::Exported { export_ms, bytes };
        let failed = |export_ms| DrtOutcome::Failed { message: "export-returned-false".into(), export_ms };

        assert_eq!(
            export_details(Some(path), &exported(72.5, Ok(vec![0; 3]))).as_deref(),
            Some("DRT export took 72.5ms, read 3 bytes from gyroflow-ofx-sizing-1-0.drt")
        );
        assert_eq!(
            export_details(Some(path), &exported(72.5, Err(std::io::Error::new(std::io::ErrorKind::NotFound, "gone")))).as_deref(),
            Some("DRT export took 72.5ms, could not read gyroflow-ofx-sizing-1-0.drt: gone")
        );
        assert_eq!(
            export_details(Some(path), &failed(Some(3.0))).as_deref(),
            Some("DRT export failed: export-returned-false; took 3.0ms, temp file gyroflow-ofx-sizing-1-0.drt")
        );
        // Lua prints no duration when the block failed before the `Export` call returned.
        assert_eq!(
            export_details(Some(path), &failed(None)).as_deref(),
            Some("DRT export failed: export-returned-false; took none, temp file gyroflow-ofx-sizing-1-0.drt")
        );
        // A missing duration reaches an exported outcome as NaN (see `drt_outcome`).
        assert_eq!(
            export_details(Some(path), &exported(f64::NAN, Ok(vec![0; 3]))).as_deref(),
            Some("DRT export took none, read 3 bytes from gyroflow-ofx-sizing-1-0.drt")
        );

        // No export was attempted: nothing to report.
        for outcome in [DrtOutcome::NotRequested, DrtOutcome::Skipped, DrtOutcome::Unsupported, DrtOutcome::TimedOut] {
            assert_eq!(export_details(Some(path), &outcome), None, "{}", outcome_label(&outcome));
        }
        for outcome in [exported(1.0, Ok(vec![0; 3])), failed(Some(1.0))] {
            assert_eq!(export_details(None, &outcome), None, "{}", outcome_label(&outcome));
        }
    }

    const CORE: [&str; 12] = ["25", "100", "00:00:04:00", "1", "1920x1080", "C:/clip.mov", "1", " scaleToFit ", "1080", " 1920", "", ""];
    const DRT: [&str; 2] = ["gf_tl_name_hex=6162", "gf_drt=skipped"];

    #[test]
    fn split_query_lines_requires_exactly_12_core_lines() {
        // The last two core lines (source in/out) are empty: still 12 core lines, indices intact.
        let lines = [&CORE[..], &DRT[..]].concat();
        let (core, drt) = split_query_lines(&lines).expect("12 core lines");
        assert_eq!(core, &CORE[..]);
        assert_eq!(drt, &DRT[..]);
        assert_eq!((core[0], core[5], core[9], core[10], core[11]), ("25", "C:/clip.mov", " 1920", "", ""));

        // 13 lines before the first `gf_` line: the framing is invalid, not "12 plus a junk line".
        let thirteen = [&CORE[..], &["stray"][..], &DRT[..]].concat();
        assert_eq!(split_query_lines(&thirteen), None);
        let eleven = [&CORE[..11], &DRT[..]].concat();
        assert_eq!(split_query_lines(&eleven), None);

        // Without a `gf_` line every line is a core line, and exactly 12 are required.
        assert_eq!(split_query_lines(&CORE), Some((&CORE[..], &[][..])));
        assert_eq!(split_query_lines(&CORE[..11]), None);
        assert_eq!(split_query_lines(&[&CORE[..], &["stray"][..]].concat()), None);
    }

    #[test]
    fn query_context_reads_mode_resolution_and_name() {
        let drt_lines = parse_drt_lines(&DRT);
        let c = query_context(&CORE, &drt_lines);
        assert_eq!((c.name_hex, c.width, c.height, c.api_mode), (Some("6162"), 1080, 1920, Some("scaleToFit")));

        let mut core = CORE;
        core[7] = " ";
        core[8] = "x";
        let no_name = DrtLines::default();
        let c = query_context(&core, &no_name);
        assert_eq!((c.name_hex, c.width, c.height, c.api_mode), (None, 0, 1920, None));
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
