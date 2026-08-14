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
// by the first successful fuscript query in a Resolve session. Every instance shares it, so a
// timeline with hundreds of plugin-bearing clips still costs one query rather than one per
// instance (a per-instance query would spawn one fuscript.exe each against Resolve's serialized
// IPC endpoint). The cache only stores project / timeline-level fields — never per-clip data
// like file_path or frame_count.
//
// `populated_at` gives the entry a freshness window (openfx-mismatch-mode-refresh): the setting
// it mirrors is user-editable at any time in Resolve, and the host sends no notification, so a
// cache without expiry silently serves a stale mode for the rest of the session. Expiry is the
// only staleness mechanism — there is deliberately no instance-lifecycle-driven invalidation.
#[derive(Clone, Debug)]
struct HostInputSizingCacheEntry {
    mismatch_mode: Option<String>,
    timeline_w: usize,
    timeline_h: usize,
    use_custom_settings: bool,
    populated_at: std::time::Instant,
}

#[derive(Default)]
struct GyroflowPlugin {
    gyroflow_plugin: GyroflowPluginBase,
    // Lock contention here is minimal: written when a fuscript query completes, read once per
    // CreateInstance and once per render (an age comparison, no query).
    host_input_sizing_cache: parking_lot::Mutex<Option<HostInputSizingCacheEntry>>,
    // Single-flight guard for the expiry-driven refresh. Without it, N instances observing the
    // same expired entry in the same frame would each spawn a query, reintroducing exactly the
    // process storm the shared cache exists to prevent. Released by the query thread on both
    // the success and the failure path — a wedged flag would disable refresh for the session.
    host_input_sizing_refresh_in_flight: Arc<AtomicBool>,
    // When a refresh was last *attempted*, successful or not. The single-flight guard alone is
    // not enough to pace retries: on a host where the query can never succeed (Resolve Free,
    // external scripting disabled, compound clip) the cache stays empty, so every frame sees
    // "stale", and the guard is released the moment each attempt fails — spawning one
    // fuscript.exe per frame. Attempt pacing bounds that to the same cadence as success.
    host_input_sizing_last_attempt: parking_lot::Mutex<Option<std::time::Instant>>,
    // Consecutive failed refresh attempts, used to back off the retry cadence. Two states retry
    // forever otherwise: Resolve busy (the query hangs and is killed on its own deadline) and the
    // playhead parked on a title / gap / compound clip (the lua errors out). Both are common and
    // long-lived, and retrying each window means one spawned-and-killed process per window for
    // the whole duration. Reset to zero by the first successful query.
    host_input_sizing_failures: Arc<std::sync::atomic::AtomicU32>,
    // Pending forced refresh, set by CreateInstance (openfx-mismatch-switch-refresh). Instance
    // recreation is the host's only observable signal that the project context may have changed
    // (the shared cache carries no project identity), so a switch to a project with different
    // mismatch settings would otherwise serve the previous project's values for a full TTL.
    // Consumed only when a forced query is actually armed — consuming it on a read that merely
    // observed it (single-flight busy, pacing floor) would let a query armed BEFORE the switch
    // land with the old project's values under a fresh timestamp and reinstate the stale window.
    host_input_sizing_force_refresh: AtomicBool,
}

/// Build the placeholder `CurrentFileInfo` that carries only host-input-sizing fields.
/// Clip-level fields (fps / frame_count / file path) stay at their defaults — `LoadCurrent`
/// refreshes those on demand. `queried_at` inherits the cache entry's timestamp so the
/// render-path mirror does not mistake a cache read for a fresh query result.
fn host_sizing_placeholder(entry: &HostInputSizingCacheEntry) -> CurrentFileInfo {
    CurrentFileInfo {
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
        // Clip-level fields stay at their defaults (playhead contract) — the
        // host-trim window is per-clip and never comes from the global cache.
        source_start_frame: None,
        source_end_frame: None,
        queried_at: Some(entry.populated_at),
    }
}

impl GyroflowPlugin {
    /// Single entry point for obtaining an up-to-date host input sizing mode
    /// (openfx-mismatch-mode-refresh). Called from CreateInstance and once per render.
    ///
    /// Two jobs, in order:
    ///  1. Adopt a newer plugin-global value into this instance's `CurrentFileInfo`. A refresh
    ///     triggered by any one instance writes the global cache, and every other instance
    ///     picks it up here — without this, only the instance that happened to arm the query
    ///     would ever see the new mode.
    ///  2. Arm a background refresh when the global entry is missing or past its TTL.
    ///
    /// Never blocks: the caller keeps rendering with the value it already has, and the query's
    /// completion forces a re-render only if something actually changed.
    ///
    /// Lock discipline: the cache entry is cloned out and its lock released before
    /// `current_file_info` is taken, because the render-path mirror acquires them in the
    /// opposite order (info → cache). Overlapping them here would invert the order and deadlock.
    fn ensure_host_input_sizing_fresh(
        &self,
        current_file_info: &Arc<Mutex<Option<CurrentFileInfo>>>,
        current_file_info_pending: &Arc<AtomicBool>,
    ) {
        let cached: Option<HostInputSizingCacheEntry> = self.host_input_sizing_cache.lock().clone();

        if let Some(entry) = cached.as_ref() {
            let mut info_lock = current_file_info.lock();
            let instance_ts = info_lock.as_ref().and_then(|i| i.queried_at);
            let cache_is_newer = instance_ts.map_or(true, |ts| entry.populated_at > ts);
            if cache_is_newer {
                match info_lock.as_mut() {
                    Some(info) => {
                        info.mismatch_mode       = entry.mismatch_mode.clone();
                        info.timeline_w          = entry.timeline_w;
                        info.timeline_h          = entry.timeline_h;
                        info.use_custom_settings = entry.use_custom_settings;
                        info.queried_at          = Some(entry.populated_at);
                    }
                    None => *info_lock = Some(host_sizing_placeholder(entry)),
                }
            }
        }

        // Arm decision (openfx-mismatch-switch-refresh): freshness, attempt pacing, failure
        // back-off, the TTL=0 kill-switch and the forced-arm bypass all live in one pure,
        // unit-tested gate. The force flag is only PEEKED here — it is consumed at the arm
        // point below, so a skipped read (pacing floor, single-flight busy) leaves it pending.
        let ttl = mismatch_ttl_ms();
        let forced = self.host_input_sizing_force_refresh.load(SeqCst);
        let decision = {
            let last_attempt = self.host_input_sizing_last_attempt.lock();
            host_sizing_arm_decision(
                forced,
                cached.as_ref().map(|e| e.populated_at.elapsed().as_millis() as u64),
                last_attempt.map(|t| t.elapsed().as_millis() as u64),
                self.host_input_sizing_failures.load(SeqCst),
                ttl,
            )
        };
        let ArmDecision::Arm { reset_failures } = decision else { return; };

        if !CurrentFileInfo::is_available() {
            // Burn the window anyway so a host without fuscript does not stat the filesystem on
            // every rendered frame. A pending forced arm can never be satisfied on such a host,
            // so consume it too — otherwise the flag pins the gate open and this branch runs
            // its filesystem stat on every rendered frame for the rest of the session.
            *self.host_input_sizing_last_attempt.lock() = Some(std::time::Instant::now());
            self.host_input_sizing_force_refresh.store(false, SeqCst);
            return;
        }

        // Single-flight: only the caller that flips false -> true spawns a query. The guard is
        // released by the query thread (RAII, covers failure and panic paths).
        if self.host_input_sizing_refresh_in_flight
            .compare_exchange(false, true, SeqCst, SeqCst)
            .is_err()
        {
            // A query is already in flight. `cmd.output()` has no deadline and fuscript reaches
            // Resolve over IPC, so a query that never returns would hold the guard for the rest
            // of the session and silently kill the refresh — reinstating the exact bug this
            // change fixes, with no log signal. `last_attempt` is only stamped when a query is
            // actually armed, so its age is the age of the in-flight query. Past a threshold far
            // beyond any plausible query, steal the guard and re-arm; the stuck thread's RAII
            // release then becomes a harmless no-op.
            let stuck = {
                let last = self.host_input_sizing_last_attempt.lock();
                (*last).map_or(false, |t| (t.elapsed().as_millis() as u64) >= STUCK_QUERY_MS)
            };
            if !stuck { return; }
            log::warn!(target: "host_input_sizing",
                "host_input_sizing: refresh query exceeded {STUCK_QUERY_MS}ms — reclaiming the single-flight guard");
            self.host_input_sizing_refresh_in_flight.store(false, SeqCst);
            if self.host_input_sizing_refresh_in_flight
                .compare_exchange(false, true, SeqCst, SeqCst)
                .is_err()
            {
                return;
            }
        }
        // A query is being armed for real: consume the forced state now (and only now — see the
        // field comment for why earlier consumption reopens the stale-cache window), and reset
        // the failure streak accumulated against the previous project context so its back-off
        // (up to 6x TTL) does not delay the very re-read the switch needs.
        if forced { self.host_input_sizing_force_refresh.store(false, SeqCst); }
        if reset_failures { self.host_input_sizing_failures.store(0, SeqCst); }
        *self.host_input_sizing_last_attempt.lock() = Some(std::time::Instant::now());
        CurrentFileInfo::query_refresh(
            current_file_info.clone(),
            current_file_info_pending.clone(),
            self.host_input_sizing_refresh_in_flight.clone(),
            self.host_input_sizing_failures.clone(),
        );
    }
}

pub fn frame_from_timetype(time: TimeType) -> f64 {
    match time {
        TimeType::Frame(x) => x,
        TimeType::FrameOrMicrosecond((Some(x), _)) => x,
        _ => panic!("Shouldn't happen"),
    }
}

/// Diagnostic kill-switch for the anamorphic physical-aspect band guesses
/// (ofx-anamorphic-band-guess). `GYROFLOW_OFX_ANAMORPHIC_BAND=0|off|false`
/// restores the stretch-blind (pre-change) input/output band guesses.
/// Read once.
fn ofx_anamorphic_band_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        match std::env::var("GYROFLOW_OFX_ANAMORPHIC_BAND") {
            Ok(v) => {
                let v = v.trim().to_ascii_lowercase();
                let off = v == "0" || v == "off" || v == "false";
                if off { log::info!("OpenFX anamorphic physical-aspect band guesses disabled via GYROFLOW_OFX_ANAMORPHIC_BAND"); }
                !off
            }
            Err(_) => true,
        }
    })
}

/// Default freshness window for the plugin-global host-input-sizing cache.
const MISMATCH_TTL_DEFAULT_MS: u64 = 10_000;
/// Lower clamp for a non-zero TTL. Below this the render path would re-query almost
/// continuously; single-flight bounds the concurrency but not the serial rate.
const MISMATCH_TTL_MIN_MS: u64 = 500;
/// Upper clamp. Beyond ten minutes the refresh stops being a refresh.
const MISMATCH_TTL_MAX_MS: u64 = 600_000;
/// How long an in-flight refresh query may run before the single-flight guard is treated as
/// wedged and reclaimed. A warm query is ~85 ms and a cold one ~270 ms; anything past a minute
/// means fuscript is not coming back, and holding the guard forever would silently disable the
/// refresh for the rest of the session.
const STUCK_QUERY_MS: u64 = 60_000;

/// Pure parse for `GYROFLOW_OFX_MISMATCH_TTL_MS`, returning `(ttl_ms, source)`.
///
/// `0` is meaningful and is **not** clamped: it disables expiry entirely, restoring the
/// sticky-cache behavior from before `openfx-mismatch-mode-refresh` for A/B diagnosis.
/// Any other value is clamped into `[MISMATCH_TTL_MIN_MS, MISMATCH_TTL_MAX_MS]`.
/// Kept free of env and `OnceLock` access so it can be unit-tested directly.
fn parse_mismatch_ttl(raw: Option<&str>) -> (u64, &'static str) {
    let Some(raw) = raw else { return (MISMATCH_TTL_DEFAULT_MS, "default"); };
    let trimmed = raw.trim();
    if trimmed.is_empty() { return (MISMATCH_TTL_DEFAULT_MS, "default"); }
    match trimmed.parse::<u64>() {
        Ok(0)                              => (0,                     "env"),
        Ok(ms) if ms < MISMATCH_TTL_MIN_MS => (MISMATCH_TTL_MIN_MS,   "env_clamped"),
        Ok(ms) if ms > MISMATCH_TTL_MAX_MS => (MISMATCH_TTL_MAX_MS,   "env_clamped"),
        Ok(ms)                             => (ms,                    "env"),
        Err(_)                             => (MISMATCH_TTL_DEFAULT_MS, "default_invalid"),
    }
}

/// Freshness window for the plugin-global host-input-sizing cache, in milliseconds.
/// `0` disables expiry. Resolved once per process; logs the resolution alongside the
/// other resolved-config lines.
fn mismatch_ttl_ms() -> u64 {
    static TTL: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *TTL.get_or_init(|| {
        let raw = std::env::var("GYROFLOW_OFX_MISMATCH_TTL_MS").ok();
        let (ttl, source) = parse_mismatch_ttl(raw.as_deref());
        log::info!(target: "host_input_sizing", "mismatch ttl resolved: ttl_ms={ttl} source={source}");
        ttl
    })
}

/// Outcome of `host_sizing_arm_decision`. `reset_failures` is true only for a forced arm:
/// the consecutive-failure streak was accumulated against the previous project context and
/// must not impose its back-off on the new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArmDecision {
    Skip,
    Arm { reset_failures: bool },
}

/// Pure gate for "should this call arm a background host-input-sizing refresh?"
/// (openfx-mismatch-switch-refresh). Kept free of `Instant`, env and locks so the
/// forced / normal / kill-switch matrix is unit-testable, following the
/// `parse_mismatch_ttl` precedent. The caller keeps the locking and single-flight
/// choreography.
///
/// - `forced`: a `CreateInstance` armed a forced refresh that has not been consumed yet.
/// - `entry_age_ms`: `None` when the cache is empty.
/// - `elapsed_since_attempt_ms`: `None` when no query attempt was ever stamped.
///
/// TTL 0 is the kill-switch: at most one bootstrap attempt per session, and the forced
/// arm is disabled entirely — its promise takes precedence.
/// A forced arm bypasses both the freshness check and the normal
/// `TTL x (1 + failures)` pacing, but still honors a short floor
/// (`MISMATCH_TTL_MIN_MS`) so a burst of CreateInstances is bounded to ~2 queries.
fn host_sizing_arm_decision(
    forced: bool,
    entry_age_ms: Option<u64>,
    elapsed_since_attempt_ms: Option<u64>,
    failures: u32,
    ttl_ms: u64,
) -> ArmDecision {
    if ttl_ms == 0 {
        // Kill-switch: bootstrap at most once (empty cache, no prior attempt), forced ignored.
        return if entry_age_ms.is_none() && elapsed_since_attempt_ms.is_none() {
            ArmDecision::Arm { reset_failures: false }
        } else {
            ArmDecision::Skip
        };
    }
    if forced {
        return match elapsed_since_attempt_ms {
            Some(elapsed) if elapsed < MISMATCH_TTL_MIN_MS => ArmDecision::Skip,
            _ => ArmDecision::Arm { reset_failures: true },
        };
    }
    let stale = entry_age_ms.map_or(true, |age| age >= ttl_ms);
    if !stale {
        return ArmDecision::Skip;
    }
    match elapsed_since_attempt_ms {
        Some(elapsed) => {
            let min_gap_ms = ttl_ms.saturating_mul(1 + failures.min(5) as u64);
            if elapsed < min_gap_ms { ArmDecision::Skip } else { ArmDecision::Arm { reset_failures: false } }
        }
        None => ArmDecision::Arm { reset_failures: false },
    }
}

/// Physical (source-native squeezed) aspect ratios for the input content-band and output
/// aspect-fit guesses (ofx-anamorphic-band-guess). Both guesses divide the lens anamorphic
/// stretch back out of the logical sizes. Resolve can also supply PAR-composited timeline
/// buffers; `select_anamorphic_band_aspects` classifies that case after the buffer is loaded.
/// - `params.size` carries the stretch only after `disable_lens_stretch(adjust_size=true)`
///   (auto-enabled when an anamorphic lens loads on OpenFX) baked it in and reset the live
///   lens stretch to 1.0; the `_raw` mirrors keep the original λ. The stretch baked into
///   `size` is therefore `raw / live` per storage axis (1.0 while the stretch is still
///   active and `size` is the raw storage size).
/// - `output_size` is the desqueezed logical output in display orientation; the host PAR
///   widening always corresponds to the full raw stretch, mapped through InputRotation
///   (storage-vertical becomes display-horizontal under 90/270).
/// Returns `(org_ratio, output_aspect)`. Stretch factors ≤ 0.01 are treated as 1.0, so
/// non-anamorphic sources reproduce the stretch-blind ratios exactly.
/// Storage-axis stretch factors baked into `stab.params.size` (raw λ ÷ live), guarded and
/// kill-switch aware. (1.0, 1.0) for non-anamorphic lenses, when the stretch is still live
/// (size is the raw storage size), or when `GYROFLOW_OFX_ANAMORPHIC_BAND=0`.
fn lens_baked_stretch(stab: &StabilizationManager) -> (f64, f64) {
    if !ofx_anamorphic_band_enabled() {
        return (1.0, 1.0);
    }
    let lens = stab.lens.read();
    let guard = |s: f64| if s > 0.01 { s } else { 1.0 };
    let live = (guard(lens.input_horizontal_stretch), guard(lens.input_vertical_stretch));
    let raw = (
        guard(lens.input_horizontal_stretch_raw().unwrap_or(live.0)),
        guard(lens.input_vertical_stretch_raw().unwrap_or(live.1)),
    );
    (raw.0 / live.0, raw.1 / live.1)
}

fn physical_band_aspects(
    size: (usize, usize),
    output_size: (usize, usize),
    rotated_90_270: bool,
    live_stretch: (f64, f64),
    raw_stretch: (f64, f64),
) -> (f64, f64) {
    let guard = |s: f64| if s > 0.01 { s } else { 1.0 };
    let (live_h, live_v) = (guard(live_stretch.0), guard(live_stretch.1));
    let (raw_h, raw_v) = (guard(raw_stretch.0), guard(raw_stretch.1));
    // Degenerate (zero) dimensions keep the stretch-blind semantics: numerator unclamped so a
    // zero size/output component yields ratio 0.0 (which the aspect-fit gate treats as
    // "disabled", same as before this change); only denominators are clamped.
    let phys_w_num = size.0 as f64 / (raw_h / live_h);
    let phys_h_num = size.1 as f64 / (raw_v / live_v);
    let phys_w_den = size.0.max(1) as f64 / (raw_h / live_h);
    let phys_h_den = size.1.max(1) as f64 / (raw_v / live_v);
    let org_ratio = if rotated_90_270 { phys_h_num / phys_w_den } else { phys_w_num / phys_h_den };
    let (out_h_squeeze, out_v_squeeze) = if rotated_90_270 { (raw_v, raw_h) } else { (raw_h, raw_v) };
    let output_aspect = (output_size.0 as f64 / out_h_squeeze) / (output_size.1.max(1) as f64 / out_v_squeeze);
    (org_ratio, output_aspect)
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum BandAspectSpace {
    LegacyLogical,
    Physical,
    HostParComposited,
}

#[derive(Copy, Clone, Debug)]
struct BandAspectSelection {
    space: BandAspectSpace,
    org_ratio: f64,
    output_aspect: f64,
}

/// Which aspect space the input content-band and output aspect-fit guesses operate in.
///
/// Supported workflow (2026-08-01): anamorphic sources are desqueezed by Resolve itself, via
/// Clip Attributes' pixel aspect ratio. What reaches the effect is therefore the LOGICAL
/// (already widened) frame that the host composited into the timeline buffer — letterboxed under
/// `scaleToFit`, full-bleed under the crop/stretch modes where the band is ignored anyway. The
/// band is then fully determined by the project's own `output_size` plus the existing
/// `HostInputSizing` and `InputRotation` parameters; nothing about the host has to be inferred
/// from buffer geometry.
///
/// The previous revision tried to tell "host already desqueezed" from "host handed the raw
/// squeezed source" by comparing the buffer's outer aspect against the physical one. Those two
/// cases are indistinguishable whenever the timeline aspect matches the squeezed source's — the
/// ordinary 16:9-timeline pairing — which is what produced warped top/bottom black bands plus
/// rolling-shutter jello on landscape 1.5x material (measured 2026-08-01).
///
/// KNOWN LIMIT, deliberate: an anamorphic clip left un-desqueezed in Resolve (Clip Attributes
/// PAR still 1.0) now gets the logical band as well, which is wrong for it. That configuration
/// is out of the supported workflow; restoring it needs a signal that says what the host did,
/// and the host does not offer one that is both instance-bound and reliable — the fuscript clip
/// PAR is resolved from the PLAYHEAD's clip, is not republished by the expiry-driven refresh,
/// and comes back empty after a project reopen.
fn select_anamorphic_band_aspects(
    enabled: bool,
    host_name: Option<&str>,
    mode_is_fit: bool,
    is_fusion_page: bool,
    dont_draw_outside: bool,
    is_preview_or_subscale: bool,
    raw_stretch: (f64, f64),
    logical_aspects: (f64, f64),
    physical_aspects: (f64, f64),
    source_buffer_size: (usize, usize),
) -> BandAspectSelection {
    let selection = |space, (org_ratio, output_aspect)| BandAspectSelection {
        space,
        org_ratio,
        output_aspect,
    };
    if !enabled {
        return selection(BandAspectSpace::LegacyLogical, logical_aspects);
    }

    let is_resolve = matches!(host_name, Some("DaVinciResolve" | "com.blackmagicdesign.resolve"));
    // Same "≤ 0.01 means unset, treat as 1.0" convention as `physical_band_aspects`, so the
    // uninitialised stretch form does not read as anamorphic. The render path already guards it
    // before calling; this keeps the function correct on its own terms.
    let guard = |s: f64| if s > 0.01 { s } else { 1.0 };
    let (raw_h, raw_v) = (guard(raw_stretch.0), guard(raw_stretch.1));
    let anamorphic = (raw_h - 1.0).abs() > 0.01 || (raw_v - 1.0).abs() > 0.01;
    // Every gate below keeps its pre-existing meaning; only the final decision changed.
    // Non-Fit modes hand over a buffer whose whole extent is valid source pixels, Fusion and
    // DontDrawOutside carry their own content-band contracts, previews/proxies are not the
    // composited timeline frame, and non-anamorphic lenses make the two spaces identical.
    if !is_resolve
        || !anamorphic
        || !mode_is_fit
        || is_fusion_page
        || dont_draw_outside
        || is_preview_or_subscale
    {
        return selection(BandAspectSpace::Physical, physical_aspects);
    }

    let (buffer_w, buffer_h) = source_buffer_size;
    if buffer_w == 0 || buffer_h == 0 {
        return selection(BandAspectSpace::Physical, physical_aspects);
    }

    selection(BandAspectSpace::HostParComposited, logical_aspects)
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

// plugins-host-timeline-trim (out-of-window passthrough): source-domain timestamp
// for an Edit/Color-page render request. Shared by Render and IsIdentity so the
// passthrough verdict can never drift from the timestamp Render actually uses.
// Exactly the historical Render math: timeline time -> source microseconds via
// speed_stretch, with the mixed-fps variant flooring to the frame grid. The
// Render-only SrcFrame override is deliberately NOT part of this function —
// IsIdentity has no access to that in-arg, which is why its boundary slack must
// stay <= the widen guard's slack (see the IsIdentity arm).
fn ofx_source_timestamp_us(time: f64, src_fps: f64, fps: f64, speed_stretch: f64) -> i64 {
    let mut timestamp_us = ((time / src_fps * 1_000_000.0) * speed_stretch).round() as i64;
    if (src_fps - fps).abs() > 0.01 {
        let frame = (time / src_fps) * fps * speed_stretch;
        timestamp_us = (frame.floor() * (1_000_000.0 / fps)).round() as i64;
    }
    timestamp_us
}

// Speed factor between the project media duration and the host clip's reported
// frame range (Resolve scales the range on retimed clips). Small deviations are
// rounding noise, not retime — absorbed to 1.0, matching the historical Render
// behavior ("for the rest users will use Fusion").
fn ofx_speed_stretch(duration_ms: f64, src_fps: f64, frame_range_max: f64) -> f64 {
    let mut speed_stretch = 1.0;
    if frame_range_max > 0.0 {
        let duration_at_src_fps = (frame_range_max / src_fps) * 1000.0;
        speed_stretch = ((duration_ms.round() / duration_at_src_fps.round()) * 100.0).floor() / 100.0;
    }
    if speed_stretch == 1.01 || speed_stretch == 0.99 || speed_stretch == 1.02 || speed_stretch == 0.98 || speed_stretch == 1.03 || speed_stretch == 0.97 {
        speed_stretch = 1.0;
    }
    speed_stretch
}

// Shared verdict for both passthrough layers (IsIdentity + render-copy): TRUE
// only when the timestamp is provably outside EVERY window range by more than
// the half-frame boundary slack — the SAME tolerance widen_host_trim_for_render
// uses, so a frame that renders because this said "inside" can never trigger
// a widening. Multi-range windows (a multi-range project trim) make frames in
// the holes between ranges pass through, matching the app where they are not
// exported.
fn host_trim_frame_outside(window_s: &[(f64, f64)], timestamp_us: i64, fps: f64) -> bool {
    if !(fps > 0.0) {
        return false;
    }
    let slack_s = 0.5 / fps;
    let t_s = timestamp_us as f64 / 1_000_000.0;
    let mut any_valid = false;
    for &(start_s, end_s) in window_s {
        if end_s > start_s {
            any_valid = true;
            if t_s >= start_s - slack_s && t_s <= end_s + slack_s {
                return false;
            }
        }
    }
    any_valid
}

// Log formatting for a (possibly multi-range) window, seconds.
fn format_trim_ranges_s(ranges: &[(f64, f64)]) -> String {
    ranges.iter().map(|&(a, b)| format!("[{a:.3}, {b:.3}]")).collect::<Vec<_>>().join(", ")
}

// plugins-host-timeline-trim D7: out-of-window passthrough frame copy for the
// Render path. STRICTLY matching buffers only — identical width/height/stride
// and both buffers on a copyable backend. Anything else returns Err and the
// caller falls back to the pre-D7 behavior (widen guard + stabilized render);
// the failure direction is always "stabilize like before", never a corrupted
// frame. CUDA + CPU cover Resolve on Windows/Linux (NVIDIA path confirmed live:
// `in_src=cuda(dev=0)`); OpenCL/Metal/OpenGL deliberately unsupported until a
// live setup exists to verify them.
fn passthrough_copy_source_to_output(buffers: &mut Buffers) -> std::result::Result<(), &'static str> {
    let (sw, sh, ss) = buffers.input.size;
    let (ow, oh, os) = buffers.output.size;
    if sw != ow || sh != oh || ss != os {
        return Err("buffer geometry mismatch");
    }
    let bytes = sh.checked_mul(ss).ok_or("buffer size overflow")?;
    if bytes == 0 {
        return Err("empty buffer");
    }
    match (&mut buffers.input.data, &mut buffers.output.data) {
        (BufferSource::Cpu { buffer: src }, BufferSource::Cpu { buffer: dst }) => {
            if src.len() < bytes || dst.len() < bytes {
                return Err("cpu buffer shorter than geometry");
            }
            dst[..bytes].copy_from_slice(&src[..bytes]);
            Ok(())
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        (BufferSource::CUDABuffer { buffer: src }, BufferSource::CUDABuffer { buffer: dst }) => {
            cuda_passthrough::memcpy_dtod(*dst as usize, *src as usize, bytes)
        }
        _ => Err("unsupported buffer backend for passthrough"),
    }
}

// Minimal CUDA driver-API surface for the passthrough copy. Device-to-device,
// synchronized on the default stream before returning (the host owns any wider
// synchronization, same contract as the stabilized render path).
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod cuda_passthrough {
    use std::sync::OnceLock;

    type CUresult = i32;
    struct Api {
        memcpy_dtod: unsafe extern "system" fn(dst: usize, src: usize, byte_count: usize) -> CUresult,
        stream_sync: unsafe extern "system" fn(stream: usize) -> CUresult,
        _lib: libloading::Library,
    }
    // SAFETY: the function pointers are plain C ABI entry points resolved from
    // the driver library, valid for the lifetime of `_lib` which is stored
    // alongside them and never dropped.
    unsafe impl Send for Api {}
    unsafe impl Sync for Api {}

    static API: OnceLock<Option<Api>> = OnceLock::new();

    fn api() -> Option<&'static Api> {
        API.get_or_init(|| unsafe {
            #[cfg(target_os = "windows")]
            let lib = libloading::Library::new("nvcuda.dll").ok()?;
            #[cfg(target_os = "linux")]
            let lib = libloading::Library::new("libcuda.so.1").ok()?;
            let memcpy_dtod = *lib.get(b"cuMemcpyDtoD_v2\0").ok()?;
            let stream_sync = *lib.get(b"cuStreamSynchronize\0").ok()?;
            Some(Api { memcpy_dtod, stream_sync, _lib: lib })
        }).as_ref()
    }

    pub fn memcpy_dtod(dst: usize, src: usize, bytes: usize) -> std::result::Result<(), &'static str> {
        let api = api().ok_or("CUDA driver library unavailable")?;
        // The host render thread arrives with its CUDA context current (the
        // same assumption the stabilized render path has always made).
        let res = unsafe { (api.memcpy_dtod)(dst, src, bytes) };
        if res != 0 {
            log::warn!("passthrough cuMemcpyDtoD_v2 failed: CUresult={res}");
            return Err("cuMemcpyDtoD_v2 failed");
        }
        let res = unsafe { (api.stream_sync)(0) };
        if res != 0 {
            log::warn!("passthrough cuStreamSynchronize failed: CUresult={res}");
            return Err("cuStreamSynchronize failed");
        }
        Ok(())
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
// Anamorphic-aware wrapper over `compute_crop_geometry` (ofx-anamorphic-band-guess).
// `stab.params.size` carries the lens desqueeze baked in (`disable_lens_stretch(adjust_size)`),
// but Resolve's input-sizing decision operates on the clip's physical storage pixels (clip PAR
// is not applied when compositing into the effect buffer — proven by the source-native squeezed
// buffers observed in Fit mode). Modeling the crop with the desqueezed size fabricates a crop
// the host never performed (e.g. a squeezed 1080×1920 clip on a 1080×1920 timeline: desqueezed
// aspect 0.84 vs 0.5625 → phantom 1.5× horizontal crop → pillarboxed render).
//
// Divides the baked stretch out, crops in physical space, and scales the result back to the
// desqueezed space the stab params live in (display axes map through the rotation). Returns
// `None` when the host performs no crop (physical display aspect already matches the timeline,
// ±1 px rounding) — the caller must leave the stab state untouched in that case.
pub fn compute_fillcrop_geometry_desqueezed(
    source_size: (usize, usize),
    baked_stretch: (f64, f64),
    timeline_aspect: f64,
    video_rotation_deg: f64,
) -> Option<(usize, usize, usize, usize)> {
    let (bh, bv) = baked_stretch;
    let phys = (
        ((source_size.0 as f64 / bh).round() as usize).max(1),
        ((source_size.1 as f64 / bv).round() as usize).max(1),
    );
    let (pw, ph, px, py) = compute_crop_geometry(phys, timeline_aspect, video_rotation_deg);
    let rotation = (((video_rotation_deg.round() as i64) % 360) + 360) % 360;
    let rotated = rotation == 90 || rotation == 270;
    let disp_phys = if rotated { (phys.1, phys.0) } else { phys };
    if px == 0 && py == 0 && pw.abs_diff(disp_phys.0) <= 1 && ph.abs_diff(disp_phys.1) <= 1 {
        return None;
    }
    // Storage-axis stretch factors mapped to display axes (crop values are display-oriented).
    let (s_dw, s_dh) = if rotated { (bv, bh) } else { (bh, bv) };
    Some((
        (pw as f64 * s_dw).round() as usize,
        (ph as f64 * s_dh).round() as usize,
        (px as f64 * s_dw).round() as usize,
        (py as f64 * s_dh).round() as usize,
    ))
}

/// Crop geometry for Resolve's `centerCrop` — "Center crop with **no resizing**". The source is
/// placed 1:1 at the timeline centre, so the visible source region is `min(timeline, source)` per
/// axis: an absolute clamp, **not** the aspect-matched band `scaleToCrop` produces. Sharing the
/// FillCrop model here (as the code did before) under-reports the visible region whenever the
/// source is larger than the timeline, and reports no crop at all when the aspects happen to match
/// while the host is in fact centre-cropping.
///
/// Takes the timeline's pixel dimensions rather than just its aspect, because "no resizing" makes
/// the absolute size the deciding factor. Mirrors `compute_fillcrop_geometry_desqueezed`: the
/// clamp is computed in physical (squeezed) pixels — what Resolve compares — and the result is
/// scaled back into the desqueezed space `stab.params` lives in, in display orientation.
///
/// Returns `None` when the source fits inside the timeline on both axes: it is then fully visible
/// (letterboxed or pillarboxed) and there is nothing to crop.
pub fn compute_centercrop_geometry_desqueezed(
    source_size: (usize, usize),
    baked_stretch: (f64, f64),
    timeline_size: (usize, usize),
    video_rotation_deg: f64,
) -> Option<(usize, usize, usize, usize)> {
    let (timeline_w, timeline_h) = timeline_size;
    if timeline_w == 0 || timeline_h == 0 { return None; }

    let (bh, bv) = baked_stretch;
    let phys = (
        ((source_size.0 as f64 / bh).round() as usize).max(1),
        ((source_size.1 as f64 / bv).round() as usize).max(1),
    );
    let rotation = (((video_rotation_deg.round() as i64) % 360) + 360) % 360;
    let rotated = rotation == 90 || rotation == 270;
    let disp_phys = if rotated { (phys.1, phys.0) } else { phys };

    // Same exact-centring requirement as the FillCrop path — see `even_margin`.
    let visible_w = even_margin(disp_phys.0, disp_phys.0.min(timeline_w));
    let visible_h = even_margin(disp_phys.1, disp_phys.1.min(timeline_h));
    if visible_w >= disp_phys.0 && visible_h >= disp_phys.1 {
        return None;
    }
    let offset_x = (disp_phys.0 - visible_w) / 2;
    let offset_y = (disp_phys.1 - visible_h) / 2;

    // Storage-axis stretch factors mapped to display axes (the result is display-oriented).
    let (s_dw, s_dh) = if rotated { (bv, bh) } else { (bh, bv) };
    Some((
        (visible_w as f64 * s_dw).round() as usize,
        (visible_h as f64 * s_dh).round() as usize,
        (offset_x as f64 * s_dw).round() as usize,
        (offset_y as f64 * s_dh).round() as usize,
    ))
}

/// Map a display-oriented crop rect (as returned by `compute_fillcrop_geometry_desqueezed`) onto
/// storage orientation, which is what `params.size`, `lens.calib_dimension` and the camera-matrix
/// principal point live in. A 90°/270° `video_rotation` transposes the two orientations; every other
/// rotation makes them identical and this is the identity mapping.
///
/// Returns `((storage_w, storage_h), (storage_x, storage_y))`.
///
/// Offsets need no 90-vs-270 distinction: `compute_crop_geometry` always centres the crop, so each
/// offset is that axis' centring value and stays the centring value after the transpose.
pub fn crop_display_to_storage(
    crop: (usize, usize, usize, usize),
    video_rotation_deg: f64,
) -> ((usize, usize), (usize, usize)) {
    let (w, h, x, y) = crop;
    let rotation = (((video_rotation_deg.round() as i64) % 360) + 360) % 360;
    if rotation == 90 || rotation == 270 {
        ((h, w), (y, x))
    } else {
        ((w, h), (x, y))
    }
}

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
        let crop_w = even_margin(sw, (sh as f64 * timeline_aspect).round() as usize);
        let crop_h = sh;
        let crop_x = sw.saturating_sub(crop_w) / 2;
        (crop_w, crop_h, crop_x, 0)
    } else {
        // Vertical crop (or exact-match: yields the full source with zero offsets).
        let crop_w = sw;
        let crop_h = even_margin(sh, (sw as f64 / timeline_aspect).round() as usize);
        let crop_y = sh.saturating_sub(crop_h) / 2;
        (crop_w, crop_h, 0, crop_y)
    }
}

/// Shrink `crop` by at most one pixel so the margin `extent - crop` is even, i.e. so the crop is
/// **exactly** centred.
///
/// With an odd margin the `/2` floor puts the extra pixel on one side, and which side that is
/// depends on the rotation origin: under a 90/270 transpose or a 180 flip the true offset is
/// `extent - offset - crop`, which differs from `offset` by exactly 1 when the margin is odd.
/// The display→storage mapping cannot recover that distinction, and the resulting 1 px
/// principal-point error is invisible on symmetric lenses (the profile overwrites `cx`/`cy` with
/// the calibration centre) but real on `asymmetrical` profiles.
///
/// Giving up at most one pixel of crop removes the ambiguity at the source instead of trying to
/// model each rotation's origin — cheaper and impossible to get backwards.
fn even_margin(extent: usize, crop: usize) -> usize {
    let crop = crop.min(extent);
    if (extent - crop) % 2 == 1 { crop.saturating_sub(1).max(1) } else { crop }
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

    // plugins-host-timeline-trim: hidden OFX string param persisting the host-derived
    // stabilization window as "<start_s>:<end_s>" (master-media seconds). Same
    // persistence contract as DetectedMismatchMode: `.drp` reopen restores the window
    // without fuscript; empty means "no bounds detected" (whole-clip window).
    detected_host_trim: ParamHandle<String>,

    // plugins-host-timeline-trim: apply-once bookkeeping. The window is applied when
    // the RESOLVED SOURCE VALUE changes (or the stab was rebuilt), NOT whenever the
    // stab's current window differs from it — a per-render diff-based apply would
    // fight the out-of-window render guard (apply narrows, guard widens, one
    // invalidate+recompute pair per frame past the boundary). The weak stab ref
    // detects cache rebuilds so a fresh (empty-window) stab gets the value again.
    // Empty = no window applied yet (first Render populates it). One entry for
    // a host sub-range, one or more for the project-trim fallback (multi-range
    // trims are honored in full since out-of-window passthrough).
    applied_host_trim: Vec<(f64, f64)>,
    applied_host_trim_stab: Option<std::sync::Weak<StabilizationManager>>,

    // plugins-host-timeline-trim (out-of-window passthrough): last IsIdentity
    // verdict (true = outside the window, host reuses the source frame). Kept
    // only to log window enter/exit transitions once instead of per frame.
    host_trim_passthrough_state: Option<bool>,

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

        // openfx-input-rotation-paste-flip: the load-time InputRotation step
        // (openfx-restore-rotation-order D1) applies the rotation DURING the rebuild and the
        // subsequent post-mutation recompute produces a ~1.78x over-zoomed + sideways-warped
        // result (gyroflow-openfx.log run 011 final_fov 0.243 = correct 0.431 / 1.78). The
        // render-path override (`apply_openfx_input_rotation_override`, run after stab_manager on
        // every render) applies the exact same rotation via the eager mechanism from the settled
        // vr=0 baseline and is CORRECT (run 013 final_fov 0.431). So defer rotation entirely to
        // the render-path override: keep the load-time step disabled. (Preserving smoothing in the
        // load path did not help — `set_output_size`→`init_size` invalidates smoothing in both
        // paths; the divergence is the rebuild-time state, not the invalidation set.)
        self.plugin.apply_input_rotation_on_load = false;

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
    // Undo any mode transform (FillCrop/CenterCrop/Stretch size/lens mutation) on the stab it
    // was applied to, so a load-level transform (InputRotation) operates on the clean baseline
    // and the next mode apply re-snapshots a baseline that already includes that transform.
    // Without this, the rotation override runs on the mode-mutated state and the cleared
    // snapshots adopt it as the new baseline — permanently baking in a crop computed for the
    // pre-rotation aspect (C0016: landscape 1215×2160 crop kept after rotating to portrait,
    // where the host no longer crops at all).
    fn restore_host_input_sizing_baseline(&mut self) {
        let Some(stab) = self.last_applied_stab.as_ref().and_then(|w| w.upgrade()) else { return; };
        if let (Some(size), Some(out_size), Some(cm), Some(cd)) = (
            self.pre_mode_size,
            self.pre_mode_output_size,
            self.pre_mode_camera_matrix.as_ref().cloned(),
            self.pre_mode_calib_dimension,
        ) {
            // Compare-before-write: a no-op restore (Fit/identity instances) must stay a true
            // no-op — no writes, no invalidation, no log.
            let mut changed = false;
            {
                let mut params_lk = stab.params.write();
                if params_lk.size != size || params_lk.output_size != out_size {
                    params_lk.size = size;
                    params_lk.output_size = out_size;
                    changed = true;
                }
            }
            {
                let mut lens_lk = stab.lens.write();
                if lens_lk.fisheye_params.camera_matrix != cm
                    || lens_lk.calib_dimension.w != cd.0
                    || lens_lk.calib_dimension.h != cd.1
                {
                    lens_lk.fisheye_params.camera_matrix = cm;
                    lens_lk.calib_dimension = gyroflow_plugin_base::gyroflow_core::lens_profile::Dimensions { w: cd.0, h: cd.1 };
                    changed = true;
                }
            }
            if changed {
                // The restore mutated real geometry: refresh sizes and mark the lazily-recomputed
                // kernel state stale (same flags the rotation override uses), so the next
                // process_pixels rebuilds ComputeParams from the restored params even when the
                // override that follows turns out to be a no-op.
                stab.init_size();
                stab.invalidate_smoothing();
                stab.invalidate_blocking_zooming();
                stab.invalidate_blocking_undistortion();
                log::info!(target: "host_input_sizing",
                    "host_input_sizing: baseline restored before input-rotation override (size={size:?} output_size={out_size:?})");
            }
        }
    }

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
        // Track whether the restore actually changed values: when the new mode's arm then does
        // NOT mutate (Fit, or the FillCrop identity/None path), the recompute at the bottom must
        // still run — otherwise the kernel keeps ComputeParams baked for the previous mode's
        // dimensions while the live params are back at the baseline (stale-geometry renders on
        // e.g. Stretch -> FillCrop-identity or FillCrop -> Fit transitions).
        let mut restore_changed = false;
        if let (Some(size), Some(out_size), Some(cm), Some(cd)) = (
            self.pre_mode_size,
            self.pre_mode_output_size,
            self.pre_mode_camera_matrix.as_ref().cloned(),
            self.pre_mode_calib_dimension,
        ) {
            {
                let mut params_lk = stab.params.write();
                if params_lk.size != size || params_lk.output_size != out_size {
                    params_lk.size = size;
                    params_lk.output_size = out_size;
                    restore_changed = true;
                }
            }
            {
                let mut lens_lk = stab.lens.write();
                if lens_lk.fisheye_params.camera_matrix != cm
                    || lens_lk.calib_dimension.w != cd.0
                    || lens_lk.calib_dimension.h != cd.1
                {
                    lens_lk.fisheye_params.camera_matrix = cm;
                    lens_lk.calib_dimension = gyroflow_plugin_base::gyroflow_core::lens_profile::Dimensions { w: cd.0, h: cd.1 };
                    restore_changed = true;
                }
            }
        }

        // Apply the new mode-specific transform.
        let did_mutate = match mode {
            HostInputSizing::Auto | HostInputSizing::Fit => false,
            HostInputSizing::FillCrop | HostInputSizing::CenterCrop => {
                if timeline_w == 0 || timeline_h == 0 {
                    log::warn!(target: "host_input_sizing", "host_input_sizing: skipping FillCrop/CenterCrop transform — timeline dims are zero");
                    false
                } else {
                    let (source_size, video_rotation) = {
                        let p = stab.params.read();
                        (p.size, p.video_rotation)
                    };
                    // Anamorphic-aware crop model: crop in physical (squeezed) space, scale back
                    // to the desqueezed space `stab.params` live in. `None` = the host performs
                    // no crop — leave the stab untouched; the Fit-equivalent render path is
                    // already correct for that geometry.
                    //
                    // FillCrop and CenterCrop are NOT the same model and no longer share one.
                    // `scaleToCrop` scales the source to cover the timeline and keeps an
                    // aspect-matched band; `centerCrop` does no resizing at all and keeps
                    // `min(timeline, source)` per axis. Sharing the FillCrop math under-reported
                    // the visible region for centerCrop, and returned "no crop" in the
                    // matching-aspect case where the host is in fact centre-cropping.
                    let baked_stretch = lens_baked_stretch(stab);
                    let geometry = if mode == HostInputSizing::CenterCrop {
                        compute_centercrop_geometry_desqueezed(
                            source_size, baked_stretch, (timeline_w, timeline_h), video_rotation,
                        )
                    } else {
                        let timeline_aspect = timeline_w as f64 / timeline_h as f64;
                        compute_fillcrop_geometry_desqueezed(
                            source_size, baked_stretch, timeline_aspect, video_rotation,
                        )
                    };
                    match geometry {
                        None => {
                            log::info!(target: "host_input_sizing",
                                "host_input_sizing: mode={mode:?} no host crop (physical aspect matches timeline) — leaving stab untouched; source={source_size:?} baked_stretch={baked_stretch:?} video_rotation={video_rotation}");
                            false
                        }
                        Some((crop_w, crop_h, crop_x, crop_y)) if crop_w == 0 || crop_h == 0 => {
                            log::warn!(target: "host_input_sizing", "host_input_sizing: skipping crop — geometry resolved to zero");
                            false
                        }
                        Some((crop_w, crop_h, crop_x, crop_y)) => {
                            // The crop rect is display-oriented. `output_size` is display-oriented
                            // too and takes it as-is, but `params.size`, `calib_dimension` and the
                            // principal point are storage-oriented, which a 90/270 rotation
                            // transposes. Writing the display rect into them was leftover defect #1
                            // from 6b8cb14 ("rotation + real crop writes the wrong orientation"),
                            // dormant until the mismatch refresh made FillCrop actually run on a
                            // rotated clip — it flattened a portrait anamorphic clip to
                            // size=(1620,608) where (608,1620) was required.
                            let ((store_w, store_h), (store_x, store_y)) =
                                crop_display_to_storage((crop_w, crop_h, crop_x, crop_y), video_rotation);
                            {
                                let mut lens_lk = stab.lens.write();
                                // Express the crop in CALIBRATION space rather than assuming the
                                // profile was calibrated at exactly the source resolution.
                                // The runtime lens scale is `params.size / calib_dimension`;
                                // writing the crop straight into `calib_dimension` forces that
                                // ratio to 1.0, which is only harmless when the profile happens
                                // to be calibrated at the source size (true for the built-in
                                // anamorphic presets, false for an external profile shot at a
                                // different resolution — there it silently rescales the focal
                                // length, e.g. 1.5x vertically for calib 1920x1080 on a
                                // 1920x1620 source). Scaling by the existing ratio preserves it;
                                // when calib == size both factors are 1.0 and this is a no-op.
                                let scale_x = lens_lk.calib_dimension.w.max(1) as f64
                                    / source_size.0.max(1) as f64;
                                let scale_y = lens_lk.calib_dimension.h.max(1) as f64
                                    / source_size.1.max(1) as f64;
                                if lens_lk.fisheye_params.camera_matrix.len() >= 2 {
                                    lens_lk.fisheye_params.camera_matrix[0][2] -= store_x as f64 * scale_x;
                                    lens_lk.fisheye_params.camera_matrix[1][2] -= store_y as f64 * scale_y;
                                }
                                lens_lk.calib_dimension =
                                    gyroflow_plugin_base::gyroflow_core::lens_profile::Dimensions {
                                        w: ((store_w as f64 * scale_x).round() as usize).max(1),
                                        h: ((store_h as f64 * scale_y).round() as usize).max(1),
                                    };
                            }
                            {
                                let mut params_lk = stab.params.write();
                                params_lk.size = (store_w, store_h);
                                params_lk.output_size = (crop_w, crop_h);
                            }
                            log::info!(target: "host_input_sizing",
                                "host_input_sizing: mode={mode:?} crop=({crop_w}x{crop_h}) offset=({crop_x},{crop_y}) \
                                 store=({store_w}x{store_h}) store_offset=({store_x},{store_y}) \
                                 source={source_size:?} baked_stretch={baked_stretch:?} video_rotation={video_rotation}");
                            true
                        }
                    }
                }
            }
            HostInputSizing::Stretch => {
                if timeline_w == 0 || timeline_h == 0 {
                    false
                } else {
                    {
                        let mut params_lk = stab.params.write();
                        // `timeline_w/h` are the OFX output buffer dimensions, i.e. display
                        // orientation, but `params.size` is storage-oriented. Writing them
                        // verbatim is the same defect D9 fixes for FillCrop a few lines up —
                        // it just never produced a visible symptom because Stretch is already
                        // documented as best-effort. Map through the same helper.
                        let ((store_w, store_h), _) = crop_display_to_storage(
                            (timeline_w, timeline_h, 0, 0),
                            params_lk.video_rotation,
                        );
                        params_lk.size = (store_w, store_h);
                    }
                    if !self.host_input_sizing_stretch_warned {
                        self.host_input_sizing_stretch_warned = true;
                        log::warn!(target: "host_input_sizing",
                            "host_input_sizing: Stretch mode is best-effort — switch Resolve mismatched-resolution to scaleToFit or scaleToCrop for accurate stabilization");
                    }
                    true
                }
            }
        };

        if did_mutate || restore_changed {
            stab.init_size();
            // Mode transitions reshape the stab's effective camera and resampling targets, so
            // invalidate smoothing/zooming caches that depend on those dimensions before the
            // recompute. `recompute_blocking` runs the full smoothing + zoom + undistort chain
            // synchronously so the next `process_pixels` call sees consistent state. Also runs
            // when only the restore changed values (non-mutating arm after a mutating mode),
            // so the restored baseline geometry is actually recomputed into the kernel state.
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
            // plugins-host-timeline-trim (out-of-window passthrough): frames outside
            // the resolved stabilization window are declared identity — the host
            // reuses the source frame untouched (no rotation, no crop, no lens
            // correction), matching the app, where out-of-trim frames are simply
            // not part of the export. Every gate below answers REPLY_DEFAULT
            // (= render normally): passthrough may only fire when the frame is
            // PROVABLY outside the window. Frames that do reach Render keep the
            // widen guard, so a host that never asks IsIdentity degrades to the
            // pre-passthrough behavior (widened window), never to black borders.
            IsIdentity(ref mut effect, ref in_args, ref mut out_args) => {
                // Diagnostic (D7 as-built): does this host dispatch IsIdentity at all?
                // The first live Resolve test showed no passthrough despite correct
                // windows — the render-copy layer below is the primary passthrough
                // path; this line settles the host-behavior question per session.
                static ISIDENTITY_SEEN: std::sync::Once = std::sync::Once::new();
                ISIDENTITY_SEEN.call_once(|| log::info!("IsIdentity action dispatched by host"));
                let instance_data: &mut InstanceData = effect.get_instance_data()?;
                // The Fusion page has no timeline item; the env gates keep both
                // rollback contracts (HOST_TRIM=0 / KEEP_PROJECT_TRIM=1) intact —
                // with either active this arm must behave as if it did not exist.
                if instance_data.is_fusion_page || !gyroflow_plugin_base::host_timeline_trim_enabled() {
                    return REPLY_DEFAULT;
                }
                // The window this instance actually applied (apply-once bookkeeping
                // in Render). Empty until the first Render — the first frame always
                // renders normally, which also populates these fields.
                if instance_data.applied_host_trim.is_empty() {
                    return REPLY_DEFAULT;
                }
                let window = instance_data.applied_host_trim.clone();
                let Some(stab) = instance_data.applied_host_trim_stab.as_ref().and_then(|w| w.upgrade()) else {
                    return REPLY_DEFAULT;
                };
                let time = in_args.get_time()?;
                let (fps, duration_ms) = {
                    let params = stab.params.read();
                    (params.fps, params.duration_ms)
                };
                if !(fps > 0.0) || !(duration_ms > 0.0) {
                    return REPLY_DEFAULT;
                }
                let src_fps = instance_data.source_clip.get_frame_rate().unwrap_or(fps);
                let speed_stretch = instance_data.source_clip.get_frame_range()
                    .map(|range| ofx_speed_stretch(duration_ms, src_fps, range.max))
                    .unwrap_or(1.0);
                // Retimed clips: Render additionally honors the host's SrcFrame
                // in-arg, which IsIdentity cannot read — near the boundary the
                // verdict could disagree with the frame Render would produce.
                // Conservative: retimes and speed ramps render normally.
                if speed_stretch != 1.0 {
                    return REPLY_DEFAULT;
                }
                let timestamp_us = ofx_source_timestamp_us(time, src_fps, fps, speed_stretch);
                if stab.params.read().get_source_timestamp_at_ramped_timestamp(timestamp_us) != timestamp_us {
                    return REPLY_DEFAULT;
                }
                // Shared verdict (half-frame slack — the SAME tolerance
                // widen_host_trim_for_render uses). Keeping the two equal is
                // load-bearing: every frame IsIdentity lets through to Render
                // lands inside the guard's tolerance, so passthrough can never
                // hand Render a frame that would widen the window.
                let outside = host_trim_frame_outside(&window, timestamp_us, fps);
                if instance_data.host_trim_passthrough_state != Some(outside) {
                    instance_data.host_trim_passthrough_state = Some(outside);
                    log::info!(
                        "host trim passthrough (IsIdentity) {}: t={:.3}s window={}s",
                        if outside { "entered" } else { "left" },
                        timestamp_us as f64 / 1_000_000.0,
                        format_trim_ranges_s(&window)
                    );
                }
                if outside {
                    out_args.set_name(&image_effect_simple_source_clip_name())?;
                    out_args.set_time(time)?;
                    OK
                } else {
                    REPLY_DEFAULT
                }
            }

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

                // Keep the host input sizing mode current (openfx-mismatch-mode-refresh). Cheap
                // on the common path: clone the shared entry, compare its age, done. Only an
                // expired or absent entry arms a background query, and only one instance wins
                // that race. Runs before `check_pending_file_info` so a result that landed since
                // the previous frame is consumed by the existing pending-flag machinery below.
                self.ensure_host_input_sizing_fresh(
                    &instance_data.current_file_info,
                    &instance_data.current_file_info_pending,
                );

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
                        let mut cache_lock = self.host_input_sizing_cache.lock();
                        // Only a genuine query result advances the cache, and with it the
                        // freshness window. This block runs on EVERY render: writing the entry
                        // unconditionally would refresh `populated_at` every frame and the TTL
                        // would never elapse, silently disabling the refresh this change adds.
                        // `queried_at` is None for a hidden-field restore (never queried) and
                        // equals the cache timestamp for a value adopted from the cache, so
                        // both correctly compare as "not new".
                        let is_new_query = match (info.queried_at, cache_lock.as_ref()) {
                            (Some(queried_at), Some(existing)) => queried_at > existing.populated_at,
                            (Some(_), None)                    => true,
                            (None, _)                          => false,
                        };
                        if is_new_query {
                            *cache_lock = Some(HostInputSizingCacheEntry {
                                mismatch_mode: info.mismatch_mode.clone(),
                                timeline_w: info.timeline_w,
                                timeline_h: info.timeline_h,
                                use_custom_settings: info.use_custom_settings,
                                populated_at: info.queried_at.unwrap_or_else(std::time::Instant::now),
                            });
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

                // plugins-host-timeline-trim: the item's source-bounds window in
                // master-media seconds, derived from the same fuscript record.
                // Clip-level fields — only a LoadCurrent/ReloadProject publish sets
                // them, never the expiry refresh (playhead contract). The per-node
                // hidden param persists a genuine sub-range across `.drp` reopens;
                // an item spanning (approximately) the whole media carries no trim
                // information — an untrimmed timeline item — and clears any stale
                // persisted window instead, letting the project's own trim drive
                // the window (applied at the stab site below). set_value runs with
                // no locks held (same deadlock rationale as DetectedMismatchMode).
                let (fuscript_range, fuscript_reported): (Option<(f64, f64)>, bool) = {
                    let info_lock = instance_data.current_file_info.lock();
                    match info_lock.as_ref() {
                        Some(info) => match (info.source_start_frame, info.source_end_frame) {
                            (Some(s), Some(e)) if info.fps > 0.0 && e > s => {
                                let (start_s, end_s) = (s / info.fps, e / info.fps);
                                let eps = 1.5 / info.fps;
                                let is_subrange = start_s > eps || end_s < info.duration_s - eps;
                                (is_subrange.then_some((start_s, end_s)), true)
                            }
                            _ => (None, false),
                        },
                        None => (None, false),
                    }
                };
                let host_trim_s = match fuscript_range {
                    Some(r) => {
                        let encoded = format!("{:.6}:{:.6}", r.0, r.1);
                        if instance_data.detected_host_trim.get_value().unwrap_or_default() != encoded {
                            let _ = instance_data.detected_host_trim.set_value(encoded);
                        }
                        Some(r)
                    }
                    None => {
                        if fuscript_reported {
                            // The host explicitly reported an untrimmed item — any
                            // persisted window is stale (incl. full-span residue
                            // from earlier builds). Clear it so it cannot mask the
                            // project-trim fallback.
                            if !instance_data.detected_host_trim.get_value().unwrap_or_default().is_empty() {
                                let _ = instance_data.detected_host_trim.set_value(String::new());
                            }
                            None
                        } else {
                            gyroflow_plugin_base::parse_host_trim_field(
                                &instance_data.detected_host_trim.get_value().unwrap_or_default(),
                            )
                        }
                    }
                };

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

                // plugins-host-timeline-trim: the stabilization window follows the host
                // item's source bounds when the item is actually trimmed; an untrimmed
                // item falls back to the project's own trim (captured at import by
                // clear_imported_project_trim, populated by the stab_manager call above —
                // which is why the fallback joins HERE and not in the chain block).
                // APPLY-ONCE per (source value, stab identity): a per-render diff-based
                // apply would fight the out-of-window render guard below (apply narrows,
                // guard widens — one invalidate+recompute pair per frame past the
                // boundary). Gated by GYROFLOW_PLUGIN_HOST_TRIM / an active
                // KEEP_PROJECT_TRIM inside the helper. The Fusion page has no timeline
                // item and its own time base — skip it there.
                if !instance_data.is_fusion_page {
                    let resolved: Vec<(f64, f64)> = host_trim_s
                        .map(|r| vec![r])
                        .unwrap_or_else(|| instance_data.plugin.original_project_trim_ranges.clone());
                    let same_stab = instance_data
                        .applied_host_trim_stab
                        .as_ref()
                        .and_then(|w| w.upgrade())
                        .map_or(false, |prev| Arc::ptr_eq(&prev, &stab));
                    if !same_stab || instance_data.applied_host_trim != resolved {
                        gyroflow_plugin_base::apply_host_timeline_trim(&stab, &resolved);
                        instance_data.applied_host_trim = resolved;
                        instance_data.applied_host_trim_stab = Some(Arc::downgrade(&stab));
                    }
                }

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
                // Same contract as the InstanceChanged(InputRotation) handler: when the override
                // is about to act, it must operate on the clean baseline, not on a
                // FillCrop/Stretch-mutated state (the snapshot-clearing below would otherwise
                // adopt the mutated state as the new baseline). Precheck with the same pure
                // decision the override uses so a no-op override never triggers a restore
                // (restoring without clearing `applied_host_input_sizing` would silently drop
                // an active crop). The restore does not touch `video_rotation`, so the precheck
                // result is unaffected by it.
                if !instance_data.is_fusion_page {
                    let input_rotation_index = instance_data.params.get_i32(Params::InputRotation).unwrap_or(0);
                    let current_video_rotation = stab.params.read().video_rotation;
                    if input_rotation_target_rotation(project_rotation, current_video_rotation, input_rotation_index).is_some() {
                        instance_data.restore_host_input_sizing_baseline();
                    }
                }
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

                // Lens stretch factors for the physical band guesses. The live fields are 1.0
                // after `disable_lens_stretch` (auto-enabled on anamorphic lens load); the `_raw`
                // mirrors keep the original λ — see `physical_band_aspects`. Lens lock is scoped
                // before the params lock below.
                let guard_stretch = |s: f64| if s > 0.01 { s } else { 1.0 };
                let anamorphic_band_enabled = ofx_anamorphic_band_enabled();
                let (live_stretch, raw_stretch) = {
                    let lens = stab.lens.read();
                    let live = (guard_stretch(lens.input_horizontal_stretch), guard_stretch(lens.input_vertical_stretch));
                    let raw = (
                        guard_stretch(lens.input_horizontal_stretch_raw().unwrap_or(live.0)),
                        guard_stretch(lens.input_vertical_stretch_raw().unwrap_or(live.1)),
                    );
                    (live, raw)
                };

                let params = stab.params.read();
                let fps = params.fps;
                let src_fps = instance_data.source_clip.get_frame_rate().unwrap_or(fps);
                // Compute both spaces; the selector below picks between them. On a main Edit/Color
                // Fit render of an anamorphic lens it takes the logical one — the host has already
                // desqueezed the clip, so that is what the buffer carries. Every other path keeps
                // the physical (stretch-divided) space.
                let logical_aspect_pair = physical_band_aspects(
                    params.size,
                    params.output_size,
                    input_rotated_90_270,
                    (1.0, 1.0),
                    (1.0, 1.0),
                );
                let physical_aspect_pair = physical_band_aspects(
                    params.size,
                    params.output_size,
                    input_rotated_90_270,
                    live_stretch,
                    raw_stretch,
                );
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
                    } else {
                        speed_stretch = ofx_speed_stretch(params.duration_ms, src_fps, range.max);
                    }
                }

                if !has_accurate_timestamps && !has_offsets {
                    instance_data.plugin.set_status(&mut instance_data.params, gyroflow_plugin_base::t!("status.not_synced"), gyroflow_plugin_base::t!("status.not_synced_hint"), false);
                } else {
                    instance_data.plugin.set_status(&mut instance_data.params, gyroflow_plugin_base::t!("status.ok"), gyroflow_plugin_base::t!("status.ok"), true);
                }

                let mut time = time;
                //let time_adj = if instance_data.is_fusion_page { instance_data.params.fusion_start_frame.get_value().unwrap_or_default() } else { 0.0 };
                time -= time_adj;
                let mut timestamp_us = ofx_source_timestamp_us(time, src_fps, fps, speed_stretch);

                // log::info!("fps: {fps:?}, src_fps: {src_fps:?}, speed_stretch: {speed_stretch:.6}, time: {time:?}, timestamp_us: {timestamp_us:?}");

                if let Ok(frame) = in_args.get_src_frame() {
                    timestamp_us = (frame as f64 * (1_000_000.0 / fps)).round() as i64;
                }

                let source_timestamp_us = params.get_source_timestamp_at_ramped_timestamp(timestamp_us);
                drop(params);

                if source_timestamp_us != timestamp_us {
                    time = (source_timestamp_us as f64 / speed_stretch / 1_000_000.0 * src_fps).round();
                    timestamp_us = ofx_source_timestamp_us(time, src_fps, fps, speed_stretch);
                }

                // plugins-host-timeline-trim D7: passthrough verdict on the FINAL
                // timestamp (SrcFrame + ramp corrections included). Decided BEFORE
                // the widen guard runs — a frame that will not be stabilized must
                // not widen the window (that would poison the in-window zoom). The
                // guard itself moved next to process_pixels below: every frame that
                // IS stabilized still passes it first (black-border invariant
                // unchanged). The explicit enabled-gate matters: the apply-once
                // bookkeeping records `applied_host_trim` even when the env
                // contracts made the apply itself a no-op.
                let host_trim_render_passthrough = !instance_data.is_fusion_page
                    && gyroflow_plugin_base::host_timeline_trim_enabled()
                    && host_trim_frame_outside(&instance_data.applied_host_trim, timestamp_us, fps);
                if !instance_data.applied_host_trim.is_empty()
                    && !instance_data.is_fusion_page
                    && instance_data.host_trim_passthrough_state != Some(host_trim_render_passthrough)
                {
                    instance_data.host_trim_passthrough_state = Some(host_trim_render_passthrough);
                    log::info!(
                        "host trim passthrough (render) {}: t={:.3}s window={}s",
                        if host_trim_render_passthrough { "entered" } else { "left" },
                        timestamp_us as f64 / 1_000_000.0,
                        format_trim_ranges_s(&instance_data.applied_host_trim)
                    );
                }

                time += time_adj;
                let source_image = if in_args.get_opengl_enabled().unwrap_or_default() {
                    instance_data.source_clip.load_texture(time, None)?
                } else {
                    instance_data.source_clip.get_image(time)?
                };
                let source_clip_pixel_aspect_ratio = instance_data.source_clip.get_pixel_aspect_ratio().ok();
                let source_image_pixel_aspect_ratio = source_image.get_pixel_aspect_ratio().ok();

                let source_rect: RectI = source_image.get_region_of_definition()?;

                let src_stride = source_image.get_row_bytes()? as usize;
                let out_stride = output_image.get_row_bytes()? as usize;
                let mut src_size = ((source_rect.x2 - source_rect.x1) as usize, (source_rect.y2 - source_rect.y1) as usize, src_stride);
                let mut out_size = ((output_rect.x2 - output_rect.x1) as usize, (output_rect.y2 - output_rect.y1) as usize, out_stride);

                if src_size.2 <= 0 { src_size.2 = src_size.0 * 4 * 4 }; // assuming 32-bit float
                if out_size.2 <= 0 { out_size.2 = out_size.0 * 4 * 4 }; // assuming 32-bit float

                let dont_draw_outside = instance_data.params.get_bool_at_time(Params::DontDrawOutside, TimeType::Frame(time)).unwrap(); // TODO: unwrap
                // `Fit` and DontDrawOutside both rely on the centered-content-band assumption.
                // FillCrop/CenterCrop/Stretch deliver a buffer whose entire extent is valid
                // source pixels (1:1 crop or stretched fill), so the input rect collapses to
                // None — the core's `get_rect` then treats the whole buffer as content.
                let mode_is_fit = matches!(effective_host_input_sizing, HostInputSizing::Auto | HostInputSizing::Fit);
                let band_selection = select_anamorphic_band_aspects(
                    anamorphic_band_enabled,
                    host_name_ref,
                    mode_is_fit,
                    instance_data.is_fusion_page,
                    dont_draw_outside,
                    is_subscale_render || is_preview_thumbnail,
                    raw_stretch,
                    logical_aspect_pair,
                    physical_aspect_pair,
                    (src_size.0, src_size.1),
                );
                let org_ratio = band_selection.org_ratio;
                let output_aspect = band_selection.output_aspect;
                if (raw_stretch.0 - 1.0).abs() > 0.01 || (raw_stretch.1 - 1.0).abs() > 0.01 {
                    log::debug!(
                        "anamorphic band guess: space={:?} host={host_name_ref:?} live_stretch={live_stretch:?} raw_stretch={raw_stretch:?} rotated_90_270={input_rotated_90_270} logical={logical_aspect_pair:?} physical={physical_aspect_pair:?} buffer=({}, {}) selected=({org_ratio:.4}, {output_aspect:.4}) clip_par={source_clip_pixel_aspect_ratio:?} image_par={source_image_pixel_aspect_ratio:?}",
                        band_selection.space,
                        src_size.0,
                        src_size.1,
                    );
                }
                let src_rect = GyroflowPluginBase::get_center_rect(src_size.0, src_size.1, org_ratio);
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

                    // plugins-host-timeline-trim D7: out-of-window frames copy the
                    // source buffer to the output untouched instead of stabilizing.
                    // A failed copy (mismatched geometry, unsupported backend) falls
                    // back to the pre-D7 path below — widen + stabilized render.
                    if host_trim_render_passthrough {
                        match passthrough_copy_source_to_output(&mut buffers) {
                            Ok(()) => { return OK; }
                            Err(reason) => {
                                log::warn!("host trim passthrough copy unavailable ({reason}) — falling back to widened stabilized render");
                            }
                        }
                    }

                    // Out-of-window render guard: a frame must never be STABILIZED
                    // while excluded from the stabilization window (the black-border
                    // mechanism plugins-ignore-project-trim removed) — a stale window
                    // widens to include the requested frame and recomputes inside
                    // process_pixels before the frame is produced. Runs immediately
                    // before process_pixels so passthrough frames (which render no
                    // stabilized pixels) never widen the window.
                    if !instance_data.is_fusion_page {
                        gyroflow_plugin_base::widen_host_trim_for_render(&stab, timestamp_us);
                    }

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
                        // OpenFX never enables the Adobe-only host-placement neutralization step
                        // (host owns orientation); flag off → shared stab-manager path unchanged.
                        host_owns_orientation:        false,
                        original_project_rotation:    None,
                        // Captured on cache-miss import; the OFX render path uses it as the
                        // stabilization window when the timeline item is untrimmed
                        // (plugins-host-timeline-trim project-trim fallback).
                        original_project_trim_ranges: Vec::new(),
                        // Captured during cache-miss builds but never consumed by OpenFX (the
                        // Premiere-only media pre-rotation compensation reads it).
                        container_media_rotation:     None,
                        // OpenFX never enables the Premiere-only rotated-anamorphic-full-frame
                        // step (adobe-rotated-anamorphic-full-frame); the shared stab-manager
                        // gating resolves to false → load path byte-identical.
                        premiere_rotated_anamorphic:  false,
                        keyframable_params: Arc::new(RwLock::new(KeyframableParams {
                            use_gyroflows_keyframes: param_set.parameter::<Bool>("UseGyroflowsKeyframes")?.get_value()?,
                            cached_keyframes:        KeyframeManager::default()
                        })),
                    },
                    current_file_info:         Arc::new(Mutex::new(None)),
                    current_file_info_pending: Arc::new(AtomicBool::new(false)),
                    host_input_sizing:                param_set.parameter("HostInputSizing")?,
                    detected_mismatch_mode:           param_set.parameter("DetectedMismatchMode")?,
                    detected_host_trim:               param_set.parameter("DetectedHostTrim")?,
                    applied_host_trim:                Vec::new(),
                    applied_host_trim_stab:           None,
                    host_trim_passthrough_state:      None,
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

                // Host-input-sizing bootstrap (openfx-mismatch-mode-refresh): one shape for every
                // instance — adopt whatever the shared cache holds, and arm a background refresh
                // when that entry is missing or past its TTL. `ensure_host_input_sizing_fresh`
                // never blocks: Resolve calls CreateInstance on the UI thread, so a synchronous
                // wait here would freeze the host. The render path's `Fit` fallback keeps the
                // first frames visually safe until a query lands, and the query's completion
                // forces a re-render only if the value actually moved.
                //
                // Two branches this change deliberately removed:
                //  - the `ProjectPath`-non-empty branch that skipped fuscript entirely for paste
                //    and `.drp` restore. It pinned whatever mode had been frozen into the project
                //    file, which is precisely the "changed the Resolve setting, plugin ignores it
                //    until I restart" bug.
                //  - the fresh-drop cache invalidation. It existed only because the cache had no
                //    expiry, making a node re-drop the sole user-reachable way to force a re-read.
                //    Expiry subsumes it, and keeping it would cost an extra query per drop.
                //
                // Forced arm (openfx-mismatch-switch-refresh): instance recreation is the host's
                // only observable signal that the project context may have changed — the shared
                // cache carries no project identity, so after a project switch a young entry
                // still holds the PREVIOUS project's mismatch mode and timeline resolution.
                // Treat the entry as suspect regardless of age: keep serving it, but arm one
                // background refresh now instead of waiting out the TTL. Skipped under the TTL=0
                // kill-switch, whose "at most one bootstrap query" promise takes precedence.
                if mismatch_ttl_ms() != 0 {
                    let cache_age_ms = self.host_input_sizing_cache.lock().as_ref()
                        .map(|e| e.populated_at.elapsed().as_millis() as u64);
                    self.host_input_sizing_force_refresh.store(true, SeqCst);
                    match cache_age_ms {
                        Some(age) => log::info!(target: "host_input_sizing",
                            "CreateInstance armed forced refresh (cache_age_ms={age})"),
                        None => log::info!(target: "host_input_sizing",
                            "CreateInstance armed forced refresh (cache=empty)"),
                    }
                }
                self.ensure_host_input_sizing_fresh(
                    &instance_data.current_file_info,
                    &instance_data.current_file_info_pending,
                );

                // Cold-start fallback ONLY: no shared-cache entry and no query result yet — the
                // genuine no-fuscript cases (Resolve Free, external scripting disabled, compound
                // clip). The per-node hidden field then supplies a mode detected in an earlier
                // session. `queried_at: None` marks it never-queried, which means (a) it is
                // immediately eligible for refresh and (b) the render-path mirror will not
                // promote it into the shared cache — a value frozen in the project file must not
                // outlive the next successful query.
                let needs_hidden_fallback = instance_data.current_file_info.lock().is_none();
                if needs_hidden_fallback {
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
                            // The host-trim fallback has its own hidden field
                            // (DetectedHostTrim, parsed in the render path).
                            source_start_frame: None,
                            source_end_frame: None,
                            queried_at: None,
                        });
                        log::info!(target: "host_input_sizing",
                            "CreateInstance restored mismatch from hidden field (mode={persisted:?})");
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
                        log::info!(target: "host_input_sizing",
                            "HostInputSizing changed by user; will re-evaluate on next render");
                    }
                    return OK;
                }
                // Hidden persistence param. Plugin-initiated writes (mirror block in Render)
                // and Resolve-initiated writes (paste/.drp restore) both end up here. Either
                // way the param is pure storage — the runtime mismatch_mode comes from the
                // global cache / hidden-field fallback at CreateInstance — so this event has
                // no work to do beyond suppressing the `Unknown param name` log line below.
                if in_args.get_name()? == "DetectedMismatchMode" || in_args.get_name()? == "DetectedHostTrim" {
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
                        // Undo any FillCrop/CenterCrop/Stretch mutation first: the rotation
                        // override must operate on the clean baseline, and the snapshot-clearing
                        // below would otherwise adopt the mode-mutated state as the new baseline
                        // (baking in a crop computed for the pre-rotation aspect).
                        instance_data.restore_host_input_sizing_baseline();
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
                    let name = in_args.get_name()?;
                    // Hidden paste-preserve shadow params (*ManuallyEdited) intentionally have no
                    // handler arm — they only exist for snapshot/merge; keep them out of the
                    // error log (pre-existing noise: "Unknown param name: InputRotationManuallyEdited").
                    if !name.ends_with("ManuallyEdited") {
                        log::error!("Unknown param name: {:?}", name);
                    }
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

                // plugins-host-timeline-trim: hidden per-node storage for the host-derived
                // stabilization window ("<start_s>:<end_s>"). Same persistence contract as
                // DetectedMismatchMode above.
                {
                    let mut param = param_set.param_define_string("DetectedHostTrim")?;
                    let _ = param.set_script_name("DetectedHostTrim");
                    param.set_string_type(ParamStringType::SingleLine)?;
                    param.set_label("Detected host trim")?;
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

    // ============================================================================================
    // FillCrop/CenterCrop anamorphic-aware crop geometry (ofx-anamorphic-band-guess): the crop
    // model runs in physical (squeezed) pixel space — the space Resolve's input sizing actually
    // operates in — and scales back to the desqueezed stab space. Identity crops return None.
    // ============================================================================================

    #[test]
    fn fillcrop_vertical_anamorphic_matching_timeline_is_noop() {
        // DSC_3172 on a portrait 1080×1920 timeline: physical display 1080×1920 == timeline
        // aspect → the host performs no crop; the desqueezed-size model used to fabricate a
        // phantom 1.5× horizontal crop here (pillarboxed render).
        let crop = compute_fillcrop_geometry_desqueezed((1920, 1620), (1.0, 1.5), 1080.0 / 1920.0, 270.0);
        assert_eq!(crop, None);
    }

    #[test]
    fn fillcrop_landscape_anamorphic_on_portrait_timeline_scales_back() {
        // R5MK2 (5760×2160 desqueezed, h=1.5) on a portrait 1080×1920 timeline: Resolve crops
        // the physical 3840×2160 frame to a centered slice, scaled back to the desqueezed space.
        //
        // The aspect-exact crop is 1215 wide, which leaves an ODD 2625 px margin — 1312 on the
        // left, 1313 on the right. `even_margin` trims the crop to 1214 so both sides are exactly
        // 1313 (see that helper: an off-centre crop makes the display->storage offset ambiguous
        // by 1 px under rotation). Desqueezed: 1214*1.5 = 1821 at 1313*1.5 ≈ 1970.
        //
        // This asserted 1823/1968 before the centring fix; a real camera config reaching the odd
        // margin is why that fix is not theoretical.
        let crop = compute_fillcrop_geometry_desqueezed((5760, 2160), (1.5, 1.0), 1080.0 / 1920.0, 0.0);
        assert_eq!(crop, Some((1821, 2160, 1970, 0)));
    }

    #[test]
    fn fillcrop_non_anamorphic_matches_raw_geometry() {
        // Non-anamorphic ultrawide crop: identical numbers to the raw compute_crop_geometry.
        let crop = compute_fillcrop_geometry_desqueezed((3840, 2160), (1.0, 1.0), 1080.0 / 1920.0, 0.0);
        let raw = compute_crop_geometry((3840, 2160), 1080.0 / 1920.0, 0.0);
        assert_eq!(crop, Some(raw));
    }

    #[test]
    fn fillcrop_non_anamorphic_exact_match_is_noop() {
        // Rotated FHD clip whose display orientation matches the portrait timeline exactly.
        let crop = compute_fillcrop_geometry_desqueezed((1920, 1080), (1.0, 1.0), 1080.0 / 1920.0, 90.0);
        assert_eq!(crop, None);
    }

    // ============================================================================================
    // Physical band aspects (ofx-anamorphic-band-guess): the input content-band and output
    // aspect-fit guesses divide the lens anamorphic stretch back out so they operate in the
    // host buffer's physical pixel space.
    // ============================================================================================

    #[test]
    fn physical_band_vertical_anamorphic_disabled_stretch() {
        // DSC_3172 as-built: disable_lens_stretch(adjust_size=true) baked v=1.5 into size
        // (1920,1080)→(1920,1620) and reset the live stretch to 1.0; raw mirrors keep 1.5.
        // Buffer is 1080×1920 physical → both ratios must equal 0.5625.
        let (org, out) = physical_band_aspects((1920, 1620), (1620, 1920), true, (1.0, 1.0), (1.0, 1.5));
        assert!((org - 0.5625).abs() < 1e-9, "org_ratio={org}");
        assert!((out - 0.5625).abs() < 1e-9, "output_aspect={out}");
    }

    #[test]
    fn physical_band_vertical_anamorphic_active_stretch() {
        // User un-checked DisableStretch: live stretch stays 1.5, size is the raw storage
        // size (baked factor = raw/live = 1.0). Same physical ratios as the disabled path.
        let (org, out) = physical_band_aspects((1920, 1080), (1620, 1920), true, (1.0, 1.5), (1.0, 1.5));
        assert!((org - 0.5625).abs() < 1e-9, "org_ratio={org}");
        assert!((out - 0.5625).abs() < 1e-9, "output_aspect={out}");
    }

    #[test]
    fn physical_band_landscape_2x_anamorphic_disabled_stretch() {
        // Landscape 2x anamorphic, stretch baked into size (1920,1080)→(3840,1080).
        // Buffer is 1920×1080 physical → both ratios must equal 16:9.
        let (org, out) = physical_band_aspects((3840, 1080), (3840, 1080), false, (1.0, 1.0), (2.0, 1.0));
        let expected = 1920.0 / 1080.0;
        assert!((org - expected).abs() < 1e-9, "org_ratio={org}");
        assert!((out - expected).abs() < 1e-9, "output_aspect={out}");
    }

    #[test]
    fn physical_band_non_anamorphic_identity() {
        // No stretch (and the unset 0.0 form): ratios must match the stretch-blind values.
        for stretch in [(1.0, 1.0), (0.0, 0.0)] {
            let (org, out) = physical_band_aspects((1920, 1080), (1620, 1920), false, stretch, stretch);
            assert!((org - 1920.0 / 1080.0).abs() < 1e-9, "org_ratio={org}");
            assert!((out - 1620.0 / 1920.0).abs() < 1e-9, "output_aspect={out}");
            let (org_r, _) = physical_band_aspects((1920, 1080), (1620, 1920), true, stretch, stretch);
            assert!((org_r - 1080.0 / 1920.0).abs() < 1e-9, "rotated org_ratio={org_r}");
        }
    }

    #[test]
    fn resolve_anamorphic_main_fit_render_uses_the_logical_band() {
        // Supported workflow: the clip is desqueezed in Resolve, so the frame reaching the effect
        // on the main Edit/Color Fit render is the logical one composited into the timeline
        // buffer. Measured 2026-08-01 on landscape 1.5x material — buffer 4023x2268 carrying a
        // centered 4023x1509 band. Within this path the answer must not depend on the buffer's
        // own aspect, which is what the replaced heuristic keyed on and got wrong.
        for raw in [(1.5, 1.0), (1.0, 1.6)] {
            for host in ["DaVinciResolve", "com.blackmagicdesign.resolve"] {
                for buffer in [(4023, 2268), (1920, 1080), (1080, 1920)] {
                    let selected = select_anamorphic_band_aspects(
                        true,
                        Some(host),
                        true,
                        false,
                        false,
                        false,
                        raw,
                        (2.6666666666666665, 2.6666666666666665),
                        (1.7777777777777777, 1.7777777777777777),
                        buffer,
                    );

                    assert_eq!(selected.space, BandAspectSpace::HostParComposited, "host={host} raw={raw:?} buffer={buffer:?}");
                    assert!((selected.org_ratio - 2.6666666666666665).abs() < 1e-9);
                    assert!((selected.output_aspect - 2.6666666666666665).abs() < 1e-9);
                }
            }
        }
    }

    #[test]
    fn undesqueezed_anamorphic_clip_also_gets_the_logical_band_by_design() {
        // Pins the documented limit rather than leaving it to be rediscovered: a clip whose
        // Resolve pixel aspect ratio was left at 1.0 is out of the supported workflow and now
        // receives the logical band on the main Fit render too, which is wrong for it. Nothing in
        // the inputs can tell it apart from a desqueezed clip — that is exactly why the
        // buffer-aspect heuristic this replaced misfired. Revisit only with a host signal that is
        // instance-bound and survives a project reopen.
        let selected = select_anamorphic_band_aspects(
            true,
            Some("DaVinciResolve"),
            true,
            false,
            false,
            false,
            (1.0, 1.5),
            (0.9, 0.9),
            (0.5625, 0.5625),
            (1080, 1920),
        );

        assert_eq!(selected.space, BandAspectSpace::HostParComposited);
        assert!((selected.org_ratio - 0.9).abs() < 1e-9);
    }

    #[test]
    fn protective_gates_stay_physical() {
        // Every one of these kept its pre-existing meaning across the band-decision change; only
        // the final Resolve/anamorphic/Fit/main-render decision moved. Tuple order:
        // (host, mode_is_fit, fusion, dont_draw_outside, preview_or_subscale, raw, buffer).
        let cases = [
            // Degenerate buffers.
            (Some("com.blackmagicdesign.resolve"), true, false, false, false, (1.0, 1.6), (0, 1080)),
            (Some("com.blackmagicdesign.resolve"), true, false, false, false, (1.0, 1.6), (972, 0)),
            // Other hosts / no host.
            (None, true, false, false, false, (1.0, 1.6), (1920, 1080)),
            (Some("com.vegascreativesoftware.vegas"), true, false, false, false, (1.0, 1.6), (1920, 1080)),
            (Some("com.example.other"), true, false, false, false, (1.0, 1.6), (1920, 1080)),
            // Non-Fit modes hand over a fully valid buffer; the band is not consulted.
            (Some("com.blackmagicdesign.resolve"), false, false, false, false, (1.0, 1.6), (1920, 1080)),
            // Fusion page and DontDrawOutside carry their own content-band contracts.
            (Some("com.blackmagicdesign.resolve"), true, true, false, false, (1.0, 1.6), (1920, 1080)),
            (Some("com.blackmagicdesign.resolve"), true, false, true, false, (1.0, 1.6), (1920, 1080)),
            // Previews / proxies are not the composited timeline frame.
            (Some("com.blackmagicdesign.resolve"), true, false, false, true, (1.0, 1.6), (288, 162)),
            // Non-anamorphic lens, including the uninitialised 0.0 form.
            (Some("com.blackmagicdesign.resolve"), true, false, false, false, (1.0, 1.0), (1920, 1080)),
            (Some("DaVinciResolve"), true, false, false, false, (0.0, 0.0), (1920, 1080)),
        ];
        for (host_name, mode_is_fit, fusion, dont_draw_outside, preview, raw, buffer) in cases {
            let selected = select_anamorphic_band_aspects(
                true,
                host_name,
                mode_is_fit,
                fusion,
                dont_draw_outside,
                preview,
                raw,
                (0.9, 0.9),
                (0.5625, 0.5625),
                buffer,
            );
            assert_eq!(selected.space, BandAspectSpace::Physical, "case host={host_name:?} fit={mode_is_fit} fusion={fusion} ddo={dont_draw_outside} preview={preview} raw={raw:?} buffer={buffer:?}");
            assert_eq!(selected.org_ratio, 0.5625);
            assert_eq!(selected.output_aspect, 0.5625);
        }
    }

    #[test]
    fn kill_switch_selects_legacy_logical() {
        let selected = select_anamorphic_band_aspects(
            false,
            Some("com.blackmagicdesign.resolve"),
            true,
            false,
            false,
            false,
            (1.0, 1.6),
            (0.9, 0.9),
            (0.5625, 0.5625),
            (972, 1080),
        );

        assert_eq!(selected.space, BandAspectSpace::LegacyLogical);
        assert!((selected.org_ratio - 0.9).abs() < 1e-9);
    }

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
            queried_at: None,
            source_start_frame: None,
            source_end_frame: None,
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
    fn even_margin_forces_an_exactly_centreable_crop() {
        // Even margin: untouched.
        assert_eq!(even_margin(1920, 1080), 1080); // margin 840
        assert_eq!(even_margin(1920, 1920), 1920); // margin 0, no crop
        // Odd margin: give up one pixel so both sides are equal.
        assert_eq!(even_margin(1920, 1215), 1214); // margin 705 -> 706
        assert_eq!(even_margin(101, 100), 99);     // margin 1 -> 2
        // Never returns 0, and never exceeds the extent.
        assert_eq!(even_margin(2, 1), 1);
        assert_eq!(even_margin(1080, 99999), 1080);
    }

    #[test]
    fn crop_geometry_offsets_are_exactly_centred_after_even_margin() {
        // 1920 wide into a 1.0 timeline aspect on a 1215-tall source would leave an odd
        // horizontal margin; the crop is trimmed so both sides match and the transpose cannot
        // land on the wrong side.
        let (w, h, x, y) = compute_crop_geometry((1920, 1215), 1.0, 0.0);
        assert_eq!(h, 1215);
        assert_eq!((1920 - w) % 2, 0, "margin must be even");
        assert_eq!(x, (1920 - w) / 2);
        assert_eq!(1920 - x - w, x, "both sides equal");
        assert_eq!(y, 0);
    }

    #[test]
    fn centercrop_keeps_min_of_timeline_and_source_not_an_aspect_band() {
        // Live config, had the user picked "Center crop" instead of "Fill+Crop":
        // portrait anamorphic, physical display 1080x1920, timeline 1920x1080.
        // No resizing => visible region is 1080x1080 centred at y=420, NOT the 1080x608
        // aspect-matched band FillCrop produces.
        let cc = compute_centercrop_geometry_desqueezed((1920, 1620), (1.0, 1.5), (1920, 1080), 90.0)
            .expect("source is taller than the timeline, so it is cropped");
        // Display-oriented, desqueezed: width 1080*1.5=1620, height 1080*1.0, offset y 420*1.0.
        assert_eq!(cc, (1620, 1080, 0, 420));

        let fc = compute_fillcrop_geometry_desqueezed((1920, 1620), (1.0, 1.5), 1920.0 / 1080.0, 90.0)
            .expect("same source is also cropped under FillCrop");
        assert_ne!(cc, fc, "the two models must not agree here — that was the bug");
    }

    #[test]
    fn centercrop_crops_when_aspects_match_but_source_is_larger() {
        // The case the old shared model got silently wrong: 3840x2160 source in a 1920x1080
        // timeline. Aspects match, so the FillCrop model reports no crop at all, while the host
        // is really showing a 2x centre crop.
        let cc = compute_centercrop_geometry_desqueezed((3840, 2160), (1.0, 1.0), (1920, 1080), 0.0)
            .expect("a 2x oversized source is centre-cropped");
        assert_eq!(cc, (1920, 1080, 960, 540));

        assert_eq!(
            compute_fillcrop_geometry_desqueezed((3840, 2160), (1.0, 1.0), 1920.0 / 1080.0, 0.0),
            None,
            "FillCrop correctly reports no crop here — which is why sharing it was wrong"
        );
    }

    #[test]
    fn centercrop_returns_none_when_source_fits_inside_the_timeline() {
        // Source smaller than the timeline on both axes: fully visible, pillar/letterboxed,
        // nothing to crop.
        assert_eq!(
            compute_centercrop_geometry_desqueezed((1280, 720), (1.0, 1.0), (1920, 1080), 0.0),
            None
        );
        // Exact match is also a no-op.
        assert_eq!(
            compute_centercrop_geometry_desqueezed((1920, 1080), (1.0, 1.0), (1920, 1080), 0.0),
            None
        );
        // Degenerate timeline dims never mutate.
        assert_eq!(
            compute_centercrop_geometry_desqueezed((1920, 1080), (1.0, 1.0), (0, 0), 0.0),
            None
        );
    }

    #[test]
    fn centercrop_crops_only_the_overflowing_axis() {
        // Wider than the timeline, shorter than it: only x is clamped.
        let cc = compute_centercrop_geometry_desqueezed((3840, 720), (1.0, 1.0), (1920, 1080), 0.0)
            .expect("x overflows");
        assert_eq!(cc, (1920, 720, 960, 0));
    }

    #[test]
    fn crop_display_to_storage_transposes_under_quarter_turns() {
        // Live case (DSC_3172, portrait anamorphic in a 1920x1080 timeline, scaleToCrop):
        // display crop 1620x608 at (0,656) must land as storage 608x1620 at (656,0).
        assert_eq!(
            crop_display_to_storage((1620, 608, 0, 656), 90.0),
            ((608, 1620), (656, 0))
        );
        assert_eq!(
            crop_display_to_storage((1620, 608, 0, 656), 270.0),
            ((608, 1620), (656, 0))
        );
        // Negative and out-of-range rotations normalize the same way.
        assert_eq!(
            crop_display_to_storage((1620, 608, 0, 656), -90.0),
            ((608, 1620), (656, 0))
        );
        assert_eq!(
            crop_display_to_storage((1620, 608, 0, 656), 450.0),
            ((608, 1620), (656, 0))
        );
    }

    #[test]
    fn crop_display_to_storage_is_identity_without_quarter_turn() {
        // Unrotated clips must stay byte-equivalent to the pre-fix write-back.
        for rot in [0.0, 180.0, 360.0, -180.0] {
            assert_eq!(
                crop_display_to_storage((1215, 2160, 352, 0), rot),
                ((1215, 2160), (352, 0)),
                "rotation {rot} should be identity"
            );
        }
    }

    #[test]
    fn fillcrop_portrait_anamorphic_into_landscape_timeline_writes_storage_orientation() {
        // End-to-end over the two pure functions, reproducing the logged live values:
        // source (1920,1620) desqueezed, stretch (1.0,1.5), rotation 90, timeline 1920x1080.
        let crop = compute_fillcrop_geometry_desqueezed((1920, 1620), (1.0, 1.5), 1920.0 / 1080.0, 90.0)
            .expect("a portrait clip in a landscape timeline is cropped by the host");
        assert_eq!(crop, (1620, 608, 0, 656), "display-oriented crop rect");

        let (storage_size, storage_offset) = crop_display_to_storage(crop, 90.0);
        assert_eq!(storage_size, (608, 1620), "params.size / calib_dimension");
        assert_eq!(storage_offset, (656, 0), "principal-point shift");
    }

    #[test]
    fn parse_mismatch_ttl_defaults_when_unset_or_blank() {
        assert_eq!(parse_mismatch_ttl(None),        (MISMATCH_TTL_DEFAULT_MS, "default"));
        assert_eq!(parse_mismatch_ttl(Some("")),    (MISMATCH_TTL_DEFAULT_MS, "default"));
        assert_eq!(parse_mismatch_ttl(Some("   ")), (MISMATCH_TTL_DEFAULT_MS, "default"));
    }

    #[test]
    fn parse_mismatch_ttl_accepts_valid_values() {
        assert_eq!(parse_mismatch_ttl(Some("10000")),  (10_000, "env"));
        assert_eq!(parse_mismatch_ttl(Some("3000")),   (3_000,  "env"));
        assert_eq!(parse_mismatch_ttl(Some(" 750\n")), (750,    "env"));
    }

    #[test]
    fn parse_mismatch_ttl_zero_disables_expiry_and_is_never_clamped() {
        // `0` is the documented kill-switch (sticky cache). It must survive the min clamp.
        assert_eq!(parse_mismatch_ttl(Some("0")), (0, "env"));
    }

    #[test]
    fn parse_mismatch_ttl_clamps_out_of_band_values() {
        assert_eq!(parse_mismatch_ttl(Some("1")),        (MISMATCH_TTL_MIN_MS, "env_clamped"));
        assert_eq!(parse_mismatch_ttl(Some("499")),      (MISMATCH_TTL_MIN_MS, "env_clamped"));
        assert_eq!(parse_mismatch_ttl(Some("99999999")), (MISMATCH_TTL_MAX_MS, "env_clamped"));
    }

    #[test]
    fn parse_mismatch_ttl_falls_back_on_garbage() {
        assert_eq!(parse_mismatch_ttl(Some("abc")),  (MISMATCH_TTL_DEFAULT_MS, "default_invalid"));
        assert_eq!(parse_mismatch_ttl(Some("-1")),   (MISMATCH_TTL_DEFAULT_MS, "default_invalid"));
        assert_eq!(parse_mismatch_ttl(Some("1.5")),  (MISMATCH_TTL_DEFAULT_MS, "default_invalid"));
    }

    const ARM: ArmDecision = ArmDecision::Arm { reset_failures: false };
    const ARM_RESET: ArmDecision = ArmDecision::Arm { reset_failures: true };
    const SKIP: ArmDecision = ArmDecision::Skip;
    const TTL: u64 = MISMATCH_TTL_DEFAULT_MS; // 10_000

    #[test]
    fn arm_decision_normal_fresh_entry_skips() {
        assert_eq!(host_sizing_arm_decision(false, Some(3_000), Some(3_000), 0, TTL), SKIP);
        // Age exactly at the TTL boundary counts as stale (>=, matching the pre-change is_stale).
        assert_eq!(host_sizing_arm_decision(false, Some(TTL), None, 0, TTL), ARM);
    }

    #[test]
    fn arm_decision_normal_stale_or_empty_arms_without_reset() {
        assert_eq!(host_sizing_arm_decision(false, None,          None, 0, TTL), ARM);
        assert_eq!(host_sizing_arm_decision(false, Some(15_000),  None, 0, TTL), ARM);
        assert_eq!(host_sizing_arm_decision(false, Some(15_000), Some(12_000), 0, TTL), ARM);
    }

    #[test]
    fn arm_decision_normal_pacing_and_backoff() {
        // Healthy: one attempt per TTL window.
        assert_eq!(host_sizing_arm_decision(false, None, Some(5_000),  0, TTL), SKIP);
        assert_eq!(host_sizing_arm_decision(false, None, Some(10_000), 0, TTL), ARM);
        // Back-off stretches the gap to TTL x (1 + failures)…
        assert_eq!(host_sizing_arm_decision(false, None, Some(25_000), 2, TTL), SKIP);
        assert_eq!(host_sizing_arm_decision(false, None, Some(30_000), 2, TTL), ARM);
        // …capped at 6x.
        assert_eq!(host_sizing_arm_decision(false, None, Some(59_000), 9, TTL), SKIP);
        assert_eq!(host_sizing_arm_decision(false, None, Some(60_000), 9, TTL), ARM);
    }

    #[test]
    fn arm_decision_forced_bypasses_freshness_and_backoff() {
        // Fresh entry, recent attempt, big failure streak — forced still arms and resets.
        assert_eq!(host_sizing_arm_decision(true, Some(2_000), None, 0, TTL), ARM_RESET);
        assert_eq!(host_sizing_arm_decision(true, Some(2_000), Some(3_000), 5, TTL), ARM_RESET);
        assert_eq!(host_sizing_arm_decision(true, None, Some(1_000), 3, TTL), ARM_RESET);
    }

    #[test]
    fn arm_decision_forced_honors_the_burst_floor() {
        // A CreateInstance burst is bounded by MISMATCH_TTL_MIN_MS between forced arms.
        assert_eq!(host_sizing_arm_decision(true, Some(2_000), Some(100), 0, TTL), SKIP);
        assert_eq!(host_sizing_arm_decision(true, Some(2_000), Some(MISMATCH_TTL_MIN_MS), 0, TTL), ARM_RESET);
    }

    #[test]
    fn arm_decision_ttl_zero_bootstraps_once_and_ignores_forced() {
        // Bootstrap: empty cache, never attempted.
        assert_eq!(host_sizing_arm_decision(false, None, None, 0, 0), ARM);
        // After any attempt, or with any entry, never again — forced or not.
        assert_eq!(host_sizing_arm_decision(false, None, Some(999_999), 0, 0), SKIP);
        assert_eq!(host_sizing_arm_decision(false, Some(999_999), None, 0, 0), SKIP);
        assert_eq!(host_sizing_arm_decision(true,  Some(1_000), None, 0, 0), SKIP);
        assert_eq!(host_sizing_arm_decision(true,  None, Some(999_999), 0, 0), SKIP);
        // Forced during bootstrap is fine but must not carry the reset marker (nothing to reset).
        assert_eq!(host_sizing_arm_decision(true, None, None, 0, 0), ARM);
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

    // plugins-host-timeline-trim (out-of-window passthrough): the shared
    // timestamp/speed helpers must reproduce the historical Render math —
    // IsIdentity's verdict is only safe while both paths agree.
    #[test]
    fn ofx_source_timestamp_matches_render_math() {
        // Same-fps path: plain round of time/src_fps.
        assert_eq!(ofx_source_timestamp_us(50.0, 25.0, 25.0, 1.0), 2_000_000);
        // Mixed-fps path (|src_fps - fps| > 0.01): floor to the target frame grid.
        assert_eq!(ofx_source_timestamp_us(50.0, 50.0, 25.0, 1.0), 1_000_000);
        // speed_stretch scales the source time.
        assert_eq!(ofx_source_timestamp_us(50.0, 25.0, 25.0, 2.0), 4_000_000);
    }

    #[test]
    fn ofx_speed_stretch_absorbs_rounding_noise_only() {
        // Exact match -> 1.0.
        assert_eq!(ofx_speed_stretch(10_000.0, 25.0, 250.0), 1.0);
        // Rounding noise (1.01) is absorbed to 1.0.
        assert_eq!(ofx_speed_stretch(10_100.0, 25.0, 250.0), 1.0);
        // A real retime survives.
        assert_eq!(ofx_speed_stretch(20_000.0, 25.0, 250.0), 2.0);
        // No usable frame range -> neutral.
        assert_eq!(ofx_speed_stretch(10_000.0, 25.0, 0.0), 1.0);
    }

    #[test]
    fn host_trim_frame_outside_uses_half_frame_slack() {
        let win = &[(2.0, 5.0)][..];
        // Inside the window.
        assert!(!host_trim_frame_outside(win, 3_000_000, 25.0));
        // Boundary frames within half-frame slack (20ms at 25fps) stay inside.
        assert!(!host_trim_frame_outside(win, 5_010_000, 25.0));
        assert!(!host_trim_frame_outside(win, 1_990_000, 25.0));
        // Clearly outside on both sides.
        assert!(host_trim_frame_outside(win, 1_000_000, 25.0));
        assert!(host_trim_frame_outside(win, 6_000_000, 25.0));
        // No window / degenerate inputs -> never outside.
        assert!(!host_trim_frame_outside(&[], 1_000_000, 25.0));
        assert!(!host_trim_frame_outside(win, 1_000_000, 0.0));
        assert!(!host_trim_frame_outside(&[(5.0, 2.0)], 1_000_000, 25.0));
    }

    #[test]
    fn host_trim_frame_outside_honors_multi_range_holes() {
        let win = &[(2.0, 3.0), (6.0, 8.0)][..];
        // Inside either range: not outside.
        assert!(!host_trim_frame_outside(win, 2_500_000, 25.0));
        assert!(!host_trim_frame_outside(win, 7_000_000, 25.0));
        // Range boundaries within slack stay inside.
        assert!(!host_trim_frame_outside(win, 3_010_000, 25.0));
        assert!(!host_trim_frame_outside(win, 5_990_000, 25.0));
        // In the hole and beyond both ends: outside (passthrough).
        assert!(host_trim_frame_outside(win, 4_500_000, 25.0));
        assert!(host_trim_frame_outside(win, 1_000_000, 25.0));
        assert!(host_trim_frame_outside(win, 9_000_000, 25.0));
        // A degenerate entry is ignored, valid ones still count.
        assert!(host_trim_frame_outside(&[(5.0, 2.0), (6.0, 8.0)], 4_500_000, 25.0));
        assert!(!host_trim_frame_outside(&[(5.0, 2.0), (6.0, 8.0)], 7_000_000, 25.0));
    }

    #[test]
    fn passthrough_copy_cpu_requires_matching_geometry() {
        fn make_buffers<'a>(src: &'a mut [u8], dst: &'a mut [u8], src_size: (usize, usize, usize), dst_size: (usize, usize, usize)) -> Buffers<'a> {
            Buffers {
                input:  BufferDescription { size: src_size, rect: None, data: BufferSource::Cpu { buffer: src }, rotation: None, texture_copy: false, post_affine: None, flip_h: false, flip_v: false },
                output: BufferDescription { size: dst_size, rect: None, data: BufferSource::Cpu { buffer: dst }, rotation: None, texture_copy: false, post_affine: None, flip_h: false, flip_v: false },
            }
        }
        // Matching geometry: source bytes land in the output untouched.
        let mut src = vec![7u8; 4 * 2 * 16];
        let mut dst = vec![0u8; 4 * 2 * 16];
        let mut buffers = make_buffers(&mut src, &mut dst, (4, 2, 64), (4, 2, 64));
        assert!(passthrough_copy_source_to_output(&mut buffers).is_ok());
        assert!(dst.iter().all(|&b| b == 7));
        // Geometry mismatch refuses instead of guessing.
        let mut src = vec![7u8; 4 * 2 * 16];
        let mut dst = vec![0u8; 4 * 4 * 16];
        let mut buffers = make_buffers(&mut src, &mut dst, (4, 2, 64), (4, 4, 64));
        assert!(passthrough_copy_source_to_output(&mut buffers).is_err());
        assert!(dst.iter().all(|&b| b == 0));
        // Undersized backing slice refuses.
        let mut src = vec![7u8; 10];
        let mut dst = vec![0u8; 4 * 2 * 16];
        let mut buffers = make_buffers(&mut src, &mut dst, (4, 2, 64), (4, 2, 64));
        assert!(passthrough_copy_source_to_output(&mut buffers).is_err());
    }
}
