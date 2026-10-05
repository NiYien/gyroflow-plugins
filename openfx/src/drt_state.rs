// SPDX-License-Identifier: GPL-3.0-or-later

//! Process-global state of the DRT export that recovers the input sizing of vertical timelines,
//! together with the export pacing and the effective-mode decision built on top of it.
//!
//! One `DrtSharedState` exists per Resolve process. At most one export is in flight at a time,
//! and exports are paced by the duration of the last one, so the export cost bound holds for the
//! whole process rather than per plugin instance.

use std::path::{ Path, PathBuf };
use std::sync::Arc;
use std::sync::atomic::{ AtomicBool, AtomicU64, Ordering };
use std::time::{ Duration, Instant, SystemTime };

use gyroflow_plugin_base::parking_lot::Mutex;

use crate::drt_sizing::{ effective_input_sizing, ApiSizingReading, DrtRejection, DrtSizing };
use crate::fuscript::DrtScriptParams;

/// An export may take at most 1 / `DRT_COST_FACTOR` of wall time.
pub const DRT_COST_FACTOR: u64 = 100;
/// Cost recorded for an export whose query timed out; equals the query timeout.
pub const DRT_TIMEOUT_COST_MS: u64 = 5_000;
/// Temp files older than this are crash residue, whichever process wrote them.
pub const DRT_RESIDUE_MAX_AGE: Duration = Duration::from_secs(3600);
pub const DRT_FILE_PREFIX: &str = "gyroflow-ofx-sizing-";
const DRT_FILE_SUFFIX: &str = ".drt";

/// Minimum time between two interval-driven exports: the query TTL, or `DRT_COST_FACTOR` times
/// the last export duration when that is longer.
pub fn drt_export_interval_ms(ttl_ms: u64, last_export_ms: Option<u64>) -> u64 {
    ttl_ms.max(DRT_COST_FACTOR.saturating_mul(last_export_ms.unwrap_or(0)))
}

/// `<dir>/gyroflow-ofx-sizing-<pid>-<seq>.drt`
pub fn temp_file_path(dir: &Path, pid: u32, seq: u64) -> PathBuf {
    dir.join(format!("{DRT_FILE_PREFIX}{pid}-{seq}{DRT_FILE_SUFFIX}"))
}

/// Deletes DRT temp files in `dir` that belong to an earlier export of this process
/// (`pid` matches and the sequence number is below `below_seq`) or are older than
/// `DRT_RESIDUE_MAX_AGE`. Returns the number of files deleted. Failures are logged at debug
/// and retried by the next sweep.
pub fn sweep_temp_files(dir: &Path, pid: u32, below_seq: u64, now: SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(stem) = file_name.to_str()
            .and_then(|n| n.strip_prefix(DRT_FILE_PREFIX))
            .and_then(|n| n.strip_suffix(DRT_FILE_SUFFIX)) else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let own_older = parse_pid_seq(stem).is_some_and(|(p, s)| p == pid && s < below_seq);
        let expired = || meta.modified().ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > DRT_RESIDUE_MAX_AGE);
        if !own_older && !expired() {
            continue;
        }
        let path = entry.path();
        match std::fs::remove_file(&path) {
            Ok(()) => {
                removed += 1;
                log::debug!(target: "host_input_sizing", "host_input_sizing: swept DRT temp file {}", path.display());
            }
            Err(e) => log::debug!(target: "host_input_sizing", "host_input_sizing: could not sweep DRT temp file {}: {e}", path.display()),
        }
    }
    removed
}

fn parse_pid_seq(stem: &str) -> Option<(u32, u64)> {
    let (pid, seq) = stem.split_once('-')?;
    Some((pid.parse().ok()?, seq.parse().ok()?))
}

/// The Lua side builds the same key: `hex(name)|width|height|horizontal mode`.
pub fn timeline_key(name_hex: &str, width: usize, height: usize, api_mode: &str) -> String {
    format!("{name_hex}|{width}|{height}|{api_mode}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SizingSource {
    /// Landscape or square timeline: the API mode is the effective one.
    Api,
    /// Validated DRT value read in this query.
    Drt,
    /// Validated DRT value from an earlier query with the same timeline key.
    DrtCached,
    /// Vertical timeline without a validated value; carries the reason.
    ApiFallback(String),
}

impl SizingSource {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Drt => "drt",
            Self::DrtCached => "drt-cached",
            Self::ApiFallback(_) => "api-fallback",
        }
    }
}

/// Chooses the effective input sizing mode. Landscape always uses the API mode. A vertical
/// timeline prefers a value validated in this query, then a validated value cached for the same
/// key, and otherwise falls back to the API mode with a reason. `this_query` is `None` when no
/// export was attempted in this query.
pub fn decide(
    width: usize,
    height: usize,
    api_mode: Option<&str>,
    key: Option<&str>,
    this_query: Option<&Result<&'static str, String>>,
    cached: Option<(&str, &'static str)>,
) -> (Option<String>, SizingSource) {
    let api = api_mode.map(str::to_string);
    if height <= width {
        return (api, SizingSource::Api);
    }
    if let Some(Ok(mode)) = this_query {
        return (Some(mode.to_string()), SizingSource::Drt);
    }
    match cached {
        Some((cached_key, mode)) if Some(cached_key) == key => (Some(mode.to_string()), SizingSource::DrtCached),
        _ => {
            let reason = match this_query {
                Some(Err(reason)) => reason.clone(),
                _ => "no-validated-value".to_string(),
            };
            (api, SizingSource::ApiFallback(reason))
        }
    }
}

/// What the query reported about the DRT export.
#[derive(Debug)]
pub enum DrtOutcome {
    /// Explicit LoadCurrent / ReloadProject query: exports are never part of it.
    NotRequested,
    /// The refresh path declined the export, or the Lua block skipped it.
    Skipped,
    Unsupported,
    /// The export ran (or was attempted) and failed. `export_ms` is the duration Lua measured
    /// around the `Export` call, `None` when the block failed before printing one.
    Failed { message: String, export_ms: Option<f64> },
    TimedOut,
    Exported { export_ms: f64, bytes: std::io::Result<Vec<u8>> },
}

/// The API reading of the queried timeline.
#[derive(Debug, Clone, Copy)]
pub struct QueryContext<'a> {
    pub name_hex: Option<&'a str>,
    pub width: usize,
    pub height: usize,
    pub api_mode: Option<&'a str>,
}

#[derive(Debug)]
pub struct Decision {
    pub effective_mode: Option<String>,
    pub source: SizingSource,
    /// Set only when `(source, effective_mode)` differs from the last logged pair.
    pub log_line: Option<String>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Support { #[default] Unknown, Yes, No }

#[derive(Default)]
struct Inner {
    support: Support,
    last_export_at: Option<Instant>,
    last_export_ms: Option<u64>,
    /// The last attempt timed out: no export at all until the interval has passed.
    timed_out: bool,
    /// Key of the last export attempt, passed to Lua so an unchanged key does not re-export.
    last_key: Option<String>,
    /// Last validated (key, effective mode). Cleared when a later export's content for the same
    /// key is rejected.
    last_validated: Option<(String, &'static str)>,
    /// An explicit query asked for an export; consumed by the next `begin_request`.
    want: bool,
    last_logged: Option<(SizingSource, Option<String>)>,
}

#[derive(Default)]
pub struct DrtSharedState {
    inner: Mutex<Inner>,
    export_in_flight: AtomicBool,
    seq: AtomicU64,
}

/// Holds the process-wide export claim until dropped.
struct ExportClaim(Arc<DrtSharedState>);

impl Drop for ExportClaim {
    fn drop(&mut self) {
        self.0.export_in_flight.store(false, Ordering::Release);
    }
}

/// Deletes the export's temp file when dropped, on every exit path.
struct TempFileGuard(PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.0) {
            Ok(()) => log::debug!(target: "host_input_sizing", "host_input_sizing: removed DRT temp file {}", self.0.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::debug!(target: "host_input_sizing",
                "host_input_sizing: could not remove DRT temp file {} (left to the next sweep): {e}", self.0.display()),
        }
    }
}

/// One claimed export slot. Dropping it, whether or not it was passed to `finish`, deletes the
/// temp file and then releases the claim.
pub struct DrtRequest {
    pub path: PathBuf,
    pub params: DrtScriptParams,
    // Fields drop in declaration order: the file is gone before the next export can be claimed.
    _file: TempFileGuard,
    _claim: ExportClaim,
}

impl DrtSharedState {
    /// Claims the export slot for one refresh query. Returns `None` when exports are unsupported,
    /// another export is in flight, or the last attempt timed out and its interval has not passed.
    /// The explicit-path want is consumed on every call, including the ones that return `None`.
    pub fn begin_request(self: &Arc<Self>, now: Instant, ttl_ms: u64, forced: bool, temp_dir: &Path, pid: u32) -> Option<DrtRequest> {
        let (want, last_key) = {
            let mut inner = self.inner.lock();
            let consumed_want = std::mem::take(&mut inner.want);
            if inner.support == Support::No {
                return None;
            }
            let interval = Duration::from_millis(drt_export_interval_ms(ttl_ms, inner.last_export_ms));
            let interval_elapsed = inner.last_export_at.is_none_or(|at| now.saturating_duration_since(at) >= interval);
            if inner.timed_out && !interval_elapsed {
                return None;
            }
            if self.export_in_flight.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
                return None;
            }
            (forced || consumed_want || interval_elapsed, inner.last_key.clone().unwrap_or_default())
        };
        let claim = ExportClaim(Arc::clone(self));

        // The sweep runs outside the lock: the render path polls `want_pending`.
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        sweep_temp_files(temp_dir, pid, seq, SystemTime::now());
        let path = temp_file_path(temp_dir, pid, seq);
        let params = DrtScriptParams { allow: true, want, last_key, path: path.to_string_lossy().into_owned() };
        Some(DrtRequest { _file: TempFileGuard(path.clone()), path, params, _claim: claim })
    }

    /// Records the query outcome and returns the decision to publish.
    pub fn finish(&self, now: Instant, request: Option<DrtRequest>, outcome: DrtOutcome, ctx: &QueryContext<'_>) -> Decision {
        self.settle(now, request, outcome, ctx, true)
    }

    /// Records the query outcome exactly like `finish` for a result that is not published.
    /// The last logged pair is left untouched.
    pub fn finish_unpublished(&self, now: Instant, request: Option<DrtRequest>, outcome: DrtOutcome, ctx: &QueryContext<'_>) {
        self.settle(now, request, outcome, ctx, false);
    }

    /// Whether an explicit query asked for an export that no refresh has picked up yet.
    pub fn want_pending(&self) -> bool {
        self.inner.lock().want
    }

    fn settle(&self, now: Instant, request: Option<DrtRequest>, outcome: DrtOutcome, ctx: &QueryContext<'_>, publish: bool) -> Decision {
        let api_mode = ctx.api_mode.unwrap_or("");
        let named = ctx.name_hex
            .and_then(|hex| Some((decode_name_hex(hex)?, timeline_key(hex, ctx.width, ctx.height, api_mode))));
        let key = named.as_ref().map(|(_, key)| key.as_str());

        // Parse before locking: unzipping a large project's DRT takes a while.
        let mut parsed: Option<DrtSizing> = None;
        // This query's DRT content was read and rejected by the parser or the cross-checks.
        let mut content_rejected = false;
        let this_query: Option<Result<&'static str, String>> = match &outcome {
            DrtOutcome::NotRequested | DrtOutcome::Skipped => None,
            DrtOutcome::Unsupported => Some(Err("unsupported".to_string())),
            DrtOutcome::Failed { message, .. } => Some(Err(format!("export-failed({message})"))),
            DrtOutcome::TimedOut => Some(Err("timeout".to_string())),
            DrtOutcome::Exported { bytes: Err(e), .. } => Some(Err(format!("export-failed(io: {e})"))),
            DrtOutcome::Exported { bytes: Ok(bytes), .. } => Some(match &named {
                None => Err(DrtRejection::TimelineNotFound.reason()),
                Some((name, _)) => {
                    let reading = ApiSizingReading { timeline_name: name, width: ctx.width, height: ctx.height, horizontal_mode: api_mode };
                    let parse_started = Instant::now();
                    let result = match effective_input_sizing(bytes, &reading) {
                        Ok(sizing) => Ok(parsed.insert(sizing).effective_mode),
                        Err(rejection) => {
                            content_rejected = true;
                            Err(rejection.reason())
                        }
                    };
                    let parse_ms = parse_started.elapsed().as_secs_f64() * 1000.0;
                    log::debug!(target: "host_input_sizing", "{}", parse_details(parse_ms, bytes.len(), &result));
                    result
                }
            }),
        };

        let mut inner = self.inner.lock();
        match &outcome {
            DrtOutcome::Exported { export_ms, .. } => {
                inner.last_export_at = Some(now);
                inner.last_export_ms = Some(export_cost_ms(*export_ms));
                inner.support = Support::Yes;
                inner.timed_out = false;
                inner.want = false;
            }
            DrtOutcome::Failed { export_ms, .. } => {
                inner.last_export_at = Some(now);
                // A failed export that ran costs what it took, like an exported one. Without a
                // measured duration the last known cost stays.
                if let Some(ms) = export_ms {
                    inner.last_export_ms = Some(export_cost_ms(*ms));
                }
                inner.timed_out = false;
                inner.want = false;
            }
            DrtOutcome::TimedOut if request.is_some() => {
                inner.last_export_ms = Some(DRT_TIMEOUT_COST_MS);
                inner.last_export_at = Some(now);
                inner.timed_out = true;
            }
            DrtOutcome::Unsupported => inner.support = Support::No,
            _ => {}
        }
        if matches!(outcome, DrtOutcome::Exported { .. } | DrtOutcome::Failed { .. }) && let Some(key) = key {
            inner.last_key = Some(key.to_string());
        }
        if let (Some(key), Some(sizing)) = (key, &parsed) {
            inner.last_validated = Some((key.to_string(), sizing.effective_mode));
        }
        // Content read in this query and rejected outranks the value cached for the same key:
        // the result falls back with the rejection reason, and later skips for that key do not
        // return to the stale mode. Failures without content (io, timeout, export failure,
        // unsupported) keep the cache preference.
        if content_rejected && let Some(key) = key && inner.last_validated.as_ref().is_some_and(|(cached_key, _)| cached_key == key) {
            inner.last_validated = None;
        }

        let cached = inner.last_validated.as_ref().map(|(k, mode)| (k.as_str(), *mode));
        let (effective_mode, source) = decide(ctx.width, ctx.height, ctx.api_mode, key, this_query.as_ref(), cached);
        if matches!(outcome, DrtOutcome::NotRequested) && ctx.height > ctx.width && matches!(source, SizingSource::ApiFallback(_)) {
            inner.want = true;
        }

        let pair = (source.clone(), effective_mode.clone());
        let log_line = (publish && inner.last_logged.as_ref() != Some(&pair)).then(|| {
            let mut line = format!(
                "host_input_sizing: source={} effective={} api={} res={}x{} last_export_ms={}",
                source.label(),
                effective_mode.as_deref().unwrap_or("none"),
                ctx.api_mode.unwrap_or("none"),
                ctx.width, ctx.height,
                inner.last_export_ms.map_or_else(|| "none".to_string(), |ms| ms.to_string()),
            );
            if let SizingSource::ApiFallback(reason) = &source {
                line.push_str(&format!(" reason={reason}"));
            }
            if let Some(sizing) = &parsed {
                line.push_str(&format!(" @96={} @112={}", sizing.horizontal_raw, sizing.vertical_raw));
            }
            line
        });
        if log_line.is_some() {
            inner.last_logged = Some(pair);
        }
        drop(inner);
        // Releases the claim only after the pacing state above is visible.
        drop(request);
        Decision { effective_mode, source, log_line }
    }
}

/// The per-parse debug line: parse duration, DRT size and the result (mode or rejection
/// reason). Never the DRT content, which carries the user's media paths.
fn parse_details(parse_ms: f64, byte_count: usize, result: &Result<&'static str, String>) -> String {
    let result = match result {
        Ok(mode) => format!("mode={mode}"),
        Err(reason) => format!("reason={reason}"),
    };
    format!("host_input_sizing: DRT parse took {parse_ms:.1}ms, {byte_count} bytes -> {result}")
}

/// Recorded export cost: whole milliseconds rounded up. An unknown (non-finite or negative)
/// duration counts as the worst known cost.
fn export_cost_ms(export_ms: f64) -> u64 {
    if export_ms.is_finite() && export_ms >= 0.0 { export_ms.ceil() as u64 } else { DRT_TIMEOUT_COST_MS }
}

/// Decodes the hex-encoded timeline name; `None` when it is empty or not valid hex.
fn decode_name_hex(hex: &str) -> Option<String> {
    // `from_str_radix` alone would accept a leading `+`.
    if hex.is_empty() || hex.len() % 2 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let bytes = (0..hex.len()).step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drt_sizing::test_support::{ fixture, hex, qt_fields, synthetic_drt, QtValue };

    const PID: u32 = 4242;
    const TTL: u64 = 10_000;
    const VERTICAL_LOG: &str = "host_input_sizing: source=drt effective=stretch api=scaleToFit res=1080x1920 last_export_ms=80 @96=-1 @112=2";

    /// A unique directory under the system temp dir, removed when dropped.
    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "gyroflow-ofx-drt-state-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    fn name_hex() -> String { hex(b"Timeline A") }
    fn ctx<'a>(name_hex: &'a str, width: usize, height: usize) -> QueryContext<'a> {
        QueryContext { name_hex: Some(name_hex), width, height, api_mode: Some("scaleToFit") }
    }
    /// A DRT whose "Timeline A" is 1080x1920 with @96 = scaleToFit and @112 = stretch.
    fn valid_drt() -> Vec<u8> {
        synthetic_drt(&[("Timeline A", &hex(&qt_fields(&[("SequenceSetup", QtValue::Bytes(fixture("b3_fit_stretch")))])))])
    }
    fn exported(export_ms: f64) -> DrtOutcome { DrtOutcome::Exported { export_ms, bytes: Ok(valid_drt()) } }
    fn failed(message: &str, export_ms: Option<f64>) -> DrtOutcome { DrtOutcome::Failed { message: message.to_string(), export_ms } }
    fn begin(state: &Arc<DrtSharedState>, at: Instant, forced: bool, dir: &TestDir) -> Option<DrtRequest> {
        state.begin_request(at, TTL, forced, &dir.0, PID)
    }
    fn mode_and_source(d: &Decision) -> (Option<&str>, &SizingSource) { (d.effective_mode.as_deref(), &d.source) }
    fn fallback(reason: &str) -> SizingSource { SizingSource::ApiFallback(reason.to_string()) }

    #[test]
    fn interval_is_ttl_floor_or_100x_cost() {
        assert_eq!(
            [drt_export_interval_ms(10_000, None), drt_export_interval_ms(10_000, Some(80)), drt_export_interval_ms(10_000, Some(500)),
             drt_export_interval_ms(10_000, Some(2_000)), drt_export_interval_ms(0, Some(80))],
            [10_000, 10_000, 50_000, 200_000, 8_000]
        );
        assert_eq!(drt_export_interval_ms(10_000, Some(u64::MAX)), u64::MAX);
    }

    #[test]
    fn temp_file_path_format() {
        assert!(temp_file_path(Path::new("/t"), 42, 7).ends_with("gyroflow-ofx-sizing-42-7.drt"));
    }

    #[test]
    fn timeline_key_and_labels_match_the_protocol() {
        assert_eq!(timeline_key("6162", 1080, 1920, "scaleToFit"), "6162|1080|1920|scaleToFit");
        assert_eq!(
            [SizingSource::Api, SizingSource::Drt, SizingSource::DrtCached, fallback("x")].map(|s| s.label()),
            ["api", "drt", "drt-cached", "api-fallback"]
        );
    }

    #[test]
    fn decide_table() {
        let api = Some("scaleToFit");
        let ok: Result<&'static str, String> = Ok("stretch");
        let err: Result<&'static str, String> = Err("timeout".into());
        let some = |m: &str| Some(m.to_string());

        assert_eq!(decide(1920, 1080, Some("stretch"), Some("k"), None, None), (some("stretch"), SizingSource::Api));
        assert_eq!(decide(1080, 1920, api, Some("k"), Some(&ok), None), (some("stretch"), SizingSource::Drt));
        // A validated value from this query wins over the cache.
        assert_eq!(decide(1080, 1920, api, Some("k"), Some(&ok), Some(("k", "centerCrop"))), (some("stretch"), SizingSource::Drt));
        assert_eq!(decide(1080, 1920, api, Some("k"), Some(&err), Some(("k", "stretch"))), (some("stretch"), SizingSource::DrtCached));
        assert_eq!(decide(1080, 1920, api, Some("k"), None, Some(("k", "stretch"))), (some("stretch"), SizingSource::DrtCached));
        assert_eq!(decide(1080, 1920, api, Some("k"), None, Some(("other", "stretch"))), (some("scaleToFit"), fallback("no-validated-value")));
        assert_eq!(decide(1080, 1920, api, Some("k"), Some(&err), None), (some("scaleToFit"), fallback("timeout")));
        // Without a key nothing cached can match.
        assert_eq!(decide(1080, 1920, api, None, None, Some(("k", "stretch"))), (some("scaleToFit"), fallback("no-validated-value")));
        assert_eq!(decide(1080, 1920, None, Some("k"), None, None), (None, fallback("no-validated-value")));
    }

    #[test]
    fn landscape_never_uses_cached_vertical() {
        assert_eq!(
            decide(1920, 1080, Some("scaleToFit"), Some("k"), None, Some(("k", "stretch"))),
            (Some("scaleToFit".to_string()), SizingSource::Api)
        );
        // The same through the shared state: a validated vertical value, then a landscape query.
        let dir = TestDir::new();
        let state = Arc::new(DrtSharedState::default());
        let t0 = Instant::now();
        let hex = name_hex();
        let d = state.finish(t0, begin(&state, t0, false, &dir), exported(80.0), &ctx(&hex, 1080, 1920));
        assert_eq!(mode_and_source(&d), (Some("stretch"), &SizingSource::Drt));
        let d = state.finish(t0, None, DrtOutcome::Skipped, &ctx(&hex, 1920, 1080));
        assert_eq!(mode_and_source(&d), (Some("scaleToFit"), &SizingSource::Api));
        let d = state.finish(t0, None, DrtOutcome::NotRequested, &ctx(&hex, 1920, 1080));
        assert_eq!(mode_and_source(&d), (Some("scaleToFit"), &SizingSource::Api));
        assert!(!state.want_pending());
    }

    #[test]
    fn begin_request_wants_first_attempt_then_paces() {
        let dir = TestDir::new();
        let state = Arc::new(DrtSharedState::default());
        let t0 = Instant::now();
        let hex = name_hex();

        let first = begin(&state, t0, false, &dir).expect("first request");
        assert_eq!(
            (first.params.allow, first.params.want, first.params.last_key.as_str()),
            (true, true, "")
        );
        assert_eq!(first.params.path, first.path.to_string_lossy());
        assert_eq!(first.path.parent(), Some(dir.0.as_path()));
        state.finish(t0, Some(first), exported(80.0), &ctx(&hex, 1080, 1920));

        let paced = begin(&state, t0 + Duration::from_secs(5), false, &dir).expect("request within interval");
        assert!(!paced.params.want);
        assert_eq!(paced.params.last_key, timeline_key(&hex, 1080, 1920, "scaleToFit"));
        drop(paced);
        let forced = begin(&state, t0 + Duration::from_secs(5), true, &dir).expect("forced request");
        assert!(forced.params.want);
        drop(forced);
        let due = begin(&state, t0 + Duration::from_secs(10), false, &dir).expect("request at interval");
        assert!(due.params.want);
    }

    #[test]
    fn begin_request_returns_none_while_export_in_flight() {
        let dir = TestDir::new();
        let state = Arc::new(DrtSharedState::default());
        let t0 = Instant::now();
        let first = begin(&state, t0, false, &dir).expect("first request");
        assert!(begin(&state, t0, true, &dir).is_none());
        drop(first);
        assert!(begin(&state, t0, false, &dir).is_some());
    }

    #[test]
    fn claim_released_when_request_dropped_unfinished() {
        let dir = TestDir::new();
        let state = Arc::new(DrtSharedState::default());
        let t0 = Instant::now();
        let hex = name_hex();
        let vertical = ctx(&hex, 1080, 1920);
        state.finish(t0, begin(&state, t0, false, &dir), exported(80.0), &vertical);

        // The query fails before the DRT block, after Resolve already wrote the file.
        let request = begin(&state, t0 + Duration::from_secs(10), false, &dir).expect("request");
        let path = request.path.clone();
        std::fs::write(&path, b"partial").unwrap();
        drop(request);
        assert!(!path.exists());

        let next = begin(&state, t0 + Duration::from_secs(20), false, &dir);
        assert!(next.is_some());
        drop(next);
        // The validated value for the unchanged key survives.
        let d = state.finish(t0 + Duration::from_secs(20), None, DrtOutcome::Skipped, &vertical);
        assert_eq!(mode_and_source(&d), (Some("stretch"), &SizingSource::DrtCached));
    }

    #[test]
    fn finish_deletes_the_request_file() {
        let dir = TestDir::new();
        let state = Arc::new(DrtSharedState::default());
        let t0 = Instant::now();
        let hex = name_hex();
        let request = begin(&state, t0, false, &dir).expect("request");
        let path = request.path.clone();
        std::fs::write(&path, valid_drt()).unwrap();
        state.finish(t0, Some(request), exported(80.0), &ctx(&hex, 1080, 1920));
        assert!(!path.exists());
    }

    #[test]
    fn timeout_records_5000_and_blocks_exports_until_interval() {
        let dir = TestDir::new();
        let state = Arc::new(DrtSharedState::default());
        let t0 = Instant::now();
        let hex = name_hex();
        let d = state.finish(t0, begin(&state, t0, false, &dir), DrtOutcome::TimedOut, &ctx(&hex, 1080, 1920));
        assert_eq!(mode_and_source(&d), (Some("scaleToFit"), &fallback("timeout")));
        assert_eq!(
            d.log_line.as_deref(),
            Some("host_input_sizing: source=api-fallback effective=scaleToFit api=scaleToFit res=1080x1920 last_export_ms=5000 reason=timeout")
        );

        // Not even a forced or key-change export runs inside the 500 s window.
        assert!(begin(&state, t0 + Duration::from_secs(499), false, &dir).is_none());
        assert!(begin(&state, t0 + Duration::from_secs(499), true, &dir).is_none());
        let after = begin(&state, t0 + Duration::from_secs(500), false, &dir).expect("request after interval");
        assert!(after.params.want);
    }

    #[test]
    fn timeout_without_request_records_nothing() {
        let dir = TestDir::new();
        let state = Arc::new(DrtSharedState::default());
        let t0 = Instant::now();
        let hex = name_hex();
        state.finish_unpublished(t0, None, DrtOutcome::TimedOut, &ctx(&hex, 1080, 1920));
        let next = begin(&state, t0 + Duration::from_secs(1), false, &dir).expect("request");
        assert!(next.params.want);
    }

    #[test]
    fn export_cost_is_ceiled_and_unknown_cost_is_worst_case() {
        let dir = TestDir::new();
        let hex = name_hex();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);

        // 80.2 ms rounds up to 81 ms, so with a zero TTL the interval is 8.1 s.
        let state = Arc::new(DrtSharedState::default());
        state.finish(t0, state.begin_request(t0, 0, false, &dir.0, PID), exported(80.2), &ctx(&hex, 1080, 1920));
        assert!(!state.begin_request(at(8_099), 0, false, &dir.0, PID).expect("request").params.want);
        assert!(state.begin_request(at(8_100), 0, false, &dir.0, PID).expect("request").params.want);

        // A non-finite or negative cost counts as the timeout cost: interval 500 s, but no block.
        for bad in [f64::NAN, f64::INFINITY, -1.0] {
            let state = Arc::new(DrtSharedState::default());
            state.finish(t0, begin(&state, t0, false, &dir), exported(bad), &ctx(&hex, 1080, 1920));
            assert!(!begin(&state, at(499_999), false, &dir).expect("request").params.want);
            assert!(begin(&state, at(500_000), false, &dir).expect("request").params.want);
        }
    }

    #[test]
    fn failed_export_with_duration_records_its_cost() {
        let dir = TestDir::new();
        let hex = name_hex();
        let vertical = ctx(&hex, 1080, 1920);
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);

        // 2 s failed export: interval 200 s, recorded like an exported one.
        let state = Arc::new(DrtSharedState::default());
        let d = state.finish(t0, begin(&state, t0, false, &dir), failed("export-returned-false", Some(1999.2)), &vertical);
        assert_eq!(
            d.log_line.as_deref(),
            Some("host_input_sizing: source=api-fallback effective=scaleToFit api=scaleToFit res=1080x1920 last_export_ms=2000 reason=export-failed(export-returned-false)")
        );
        assert!(!begin(&state, at(199_999), false, &dir).expect("request").params.want);
        assert!(begin(&state, at(200_000), false, &dir).expect("request").params.want);

        // Without a duration the last known cost is kept.
        let state = Arc::new(DrtSharedState::default());
        state.finish(t0, begin(&state, t0, false, &dir), exported(2000.0), &vertical);
        state.finish(at(200_000), begin(&state, at(200_000), false, &dir), failed("x", None), &vertical);
        assert!(!begin(&state, at(399_999), false, &dir).expect("request").params.want);
        assert!(begin(&state, at(400_000), false, &dir).expect("request").params.want);

        // A non-finite duration counts as the worst known cost, as for an exported one.
        let state = Arc::new(DrtSharedState::default());
        state.finish(t0, begin(&state, t0, false, &dir), failed("x", Some(f64::NAN)), &vertical);
        assert!(!begin(&state, at(499_999), false, &dir).expect("request").params.want);
        assert!(begin(&state, at(500_000), false, &dir).expect("request").params.want);
    }

    #[test]
    fn parse_details_name_the_cost_and_the_result() {
        assert_eq!(
            parse_details(12.34, 27_309, &Ok("stretch")),
            "host_input_sizing: DRT parse took 12.3ms, 27309 bytes -> mode=stretch"
        );
        assert_eq!(
            parse_details(0.06, 812, &Err("vertical-out-of-range(7)".to_string())),
            "host_input_sizing: DRT parse took 0.1ms, 812 bytes -> reason=vertical-out-of-range(7)"
        );
    }

    #[test]
    fn unsupported_disables_exports_for_session() {
        let dir = TestDir::new();
        let state = Arc::new(DrtSharedState::default());
        let t0 = Instant::now();
        let hex = name_hex();
        let d = state.finish(t0, begin(&state, t0, false, &dir), DrtOutcome::Unsupported, &ctx(&hex, 1080, 1920));
        assert_eq!(mode_and_source(&d), (Some("scaleToFit"), &fallback("unsupported")));
        for secs in [0, 10, 1_000, 100_000] {
            assert!(begin(&state, t0 + Duration::from_secs(secs), true, &dir).is_none());
        }
    }

    #[test]
    fn validated_export_is_reused_while_key_unchanged() {
        let dir = TestDir::new();
        let state = Arc::new(DrtSharedState::default());
        let t0 = Instant::now();
        let hex = name_hex();
        let vertical = ctx(&hex, 1080, 1920);

        let d = state.finish(t0, begin(&state, t0, false, &dir), exported(80.0), &vertical);
        assert_eq!(mode_and_source(&d), (Some("stretch"), &SizingSource::Drt));
        assert_eq!(d.log_line.as_deref(), Some(VERTICAL_LOG));

        let d = state.finish(t0 + Duration::from_secs(5), begin(&state, t0 + Duration::from_secs(5), false, &dir), DrtOutcome::Skipped, &vertical);
        assert_eq!(mode_and_source(&d), (Some("stretch"), &SizingSource::DrtCached));
        assert_eq!(
            d.log_line.as_deref(),
            Some("host_input_sizing: source=drt-cached effective=stretch api=scaleToFit res=1080x1920 last_export_ms=80")
        );

        // A different key (here: another horizontal mode) does not reuse the value.
        let other = QueryContext { api_mode: Some("scaleToCrop"), ..ctx(&hex, 1080, 1920) };
        let d = state.finish(t0 + Duration::from_secs(6), None, DrtOutcome::Skipped, &other);
        assert_eq!(mode_and_source(&d), (Some("scaleToCrop"), &fallback("no-validated-value")));
    }

    #[test]
    fn failed_export_updates_last_key_so_key_trigger_does_not_repeat() {
        let dir = TestDir::new();
        let t0 = Instant::now();
        let hex = name_hex();
        let mismatched = ctx(&hex, 1080, 1350);

        let state = Arc::new(DrtSharedState::default());
        let d = state.finish(t0, begin(&state, t0, false, &dir), exported(80.0), &mismatched);
        assert_eq!(mode_and_source(&d), (Some("scaleToFit"), &fallback("resolution-mismatch(drt=1080x1920 api=1080x1350)")));
        let next = begin(&state, t0 + Duration::from_secs(5), false, &dir).expect("request");
        assert_eq!(next.params.last_key, timeline_key(&hex, 1080, 1350, "scaleToFit"));
        assert!(!next.params.want);
        drop(next);

        let state = Arc::new(DrtSharedState::default());
        let d = state.finish(t0, begin(&state, t0, false, &dir), failed("export-returned-false", None), &mismatched);
        assert_eq!(mode_and_source(&d), (Some("scaleToFit"), &fallback("export-failed(export-returned-false)")));
        let next = begin(&state, t0 + Duration::from_secs(5), false, &dir).expect("request");
        assert_eq!(next.params.last_key, timeline_key(&hex, 1080, 1350, "scaleToFit"));
        assert!(!next.params.want);
    }

    #[test]
    fn export_without_timeline_name_is_not_found() {
        let dir = TestDir::new();
        let t0 = Instant::now();
        for name_hex in [None, Some(""), Some("zz"), Some("616"), Some("+a")] {
            let state = Arc::new(DrtSharedState::default());
            let query = QueryContext { name_hex, width: 1080, height: 1920, api_mode: Some("scaleToFit") };
            let d = state.finish(t0, begin(&state, t0, false, &dir), exported(80.0), &query);
            assert_eq!(mode_and_source(&d), (Some("scaleToFit"), &fallback("timeline-not-found")));
            let next = begin(&state, t0 + Duration::from_secs(5), false, &dir).expect("request");
            assert_eq!(next.params.last_key, "");
        }
    }

    #[test]
    fn unreadable_file_falls_back_to_cached_for_same_key() {
        let dir = TestDir::new();
        let t0 = Instant::now();
        let hex = name_hex();
        let vertical = ctx(&hex, 1080, 1920);
        let unreadable = || DrtOutcome::Exported { export_ms: 80.0, bytes: Err(std::io::Error::new(std::io::ErrorKind::NotFound, "gone")) };

        let state = Arc::new(DrtSharedState::default());
        state.finish(t0, begin(&state, t0, false, &dir), exported(80.0), &vertical);
        let t1 = t0 + Duration::from_secs(10);
        let d = state.finish(t1, begin(&state, t1, false, &dir), unreadable(), &vertical);
        assert_eq!(mode_and_source(&d), (Some("stretch"), &SizingSource::DrtCached));

        // Without a validated value for the key the io error is the reason.
        let state = Arc::new(DrtSharedState::default());
        let d = state.finish(t0, begin(&state, t0, false, &dir), unreadable(), &vertical);
        assert_eq!(mode_and_source(&d), (Some("scaleToFit"), &fallback("export-failed(io: gone)")));
    }

    #[test]
    fn content_rejection_overrides_cached_and_clears_it() {
        let dir = TestDir::new();
        let state = Arc::new(DrtSharedState::default());
        let t0 = Instant::now();
        let at = |secs: u64| t0 + Duration::from_secs(secs);
        let tl_hex = name_hex();
        let vertical = ctx(&tl_hex, 1080, 1920);

        let d = state.finish(t0, begin(&state, t0, false, &dir), exported(80.0), &vertical);
        assert_eq!(mode_and_source(&d), (Some("stretch"), &SizingSource::Drt));

        // The same timeline key, but this export's vertical field is out of range.
        let mut setup = fixture("b3_fit_stretch");
        setup[112..116].copy_from_slice(&7i32.to_be_bytes());
        let rejected = synthetic_drt(&[("Timeline A", &hex(&qt_fields(&[("SequenceSetup", QtValue::Bytes(setup))])))]);
        let d = state.finish(at(10), begin(&state, at(10), false, &dir), DrtOutcome::Exported { export_ms: 80.0, bytes: Ok(rejected) }, &vertical);
        assert_eq!(mode_and_source(&d), (Some("scaleToFit"), &fallback("vertical-out-of-range(7)")));

        // The stale mode is gone for this key, not just outranked once.
        let d = state.finish(at(15), begin(&state, at(15), false, &dir), DrtOutcome::Skipped, &vertical);
        assert_eq!(mode_and_source(&d), (Some("scaleToFit"), &fallback("no-validated-value")));
    }

    #[test]
    fn content_rejection_keeps_the_value_cached_for_another_key() {
        let dir = TestDir::new();
        let state = Arc::new(DrtSharedState::default());
        let t0 = Instant::now();
        let hex = name_hex();
        let vertical = ctx(&hex, 1080, 1920);
        state.finish(t0, begin(&state, t0, false, &dir), exported(80.0), &vertical);

        // A rejection under another key (here: resolution mismatch) leaves the first key's value.
        let other = ctx(&hex, 1080, 1350);
        let d = state.finish(t0, begin(&state, t0, false, &dir), exported(80.0), &other);
        assert_eq!(mode_and_source(&d), (Some("scaleToFit"), &fallback("resolution-mismatch(drt=1080x1920 api=1080x1350)")));
        let d = state.finish(t0, None, DrtOutcome::Skipped, &vertical);
        assert_eq!(mode_and_source(&d), (Some("stretch"), &SizingSource::DrtCached));
    }

    #[test]
    fn explicit_path_requests_export_on_fallback() {
        let t0 = Instant::now();
        let hex = name_hex();

        let state = Arc::new(DrtSharedState::default());
        let d = state.finish(t0, None, DrtOutcome::NotRequested, &ctx(&hex, 1080, 1920));
        assert_eq!(mode_and_source(&d), (Some("scaleToFit"), &fallback("no-validated-value")));
        assert!(state.want_pending());

        // Landscape and an already validated value request nothing.
        let state = Arc::new(DrtSharedState::default());
        state.finish(t0, None, DrtOutcome::NotRequested, &ctx(&hex, 1920, 1080));
        assert!(!state.want_pending());
        let dir = TestDir::new();
        state.finish(t0, begin(&state, t0, false, &dir), exported(80.0), &ctx(&hex, 1080, 1920));
        let d = state.finish(t0, None, DrtOutcome::NotRequested, &ctx(&hex, 1080, 1920));
        assert_eq!(mode_and_source(&d), (Some("stretch"), &SizingSource::DrtCached));
        assert!(!state.want_pending());
    }

    #[test]
    fn refresh_skip_never_requests_export() {
        let dir = TestDir::new();
        let t0 = Instant::now();
        let hex = name_hex();
        let vertical = ctx(&hex, 1080, 1920);

        let state = Arc::new(DrtSharedState::default());
        let d = state.finish(t0, begin(&state, t0, false, &dir), DrtOutcome::Skipped, &vertical);
        assert_eq!(mode_and_source(&d), (Some("scaleToFit"), &fallback("no-validated-value")));
        assert!(!state.want_pending());
        let d = state.finish(t0, None, DrtOutcome::Skipped, &vertical);
        assert_eq!(mode_and_source(&d), (Some("scaleToFit"), &fallback("no-validated-value")));
        assert!(!state.want_pending());
        state.finish_unpublished(t0, None, DrtOutcome::Skipped, &vertical);
        assert!(!state.want_pending());
    }

    #[test]
    fn begin_request_consumes_want_even_when_declined() {
        let dir = TestDir::new();
        let state = Arc::new(DrtSharedState::default());
        let t0 = Instant::now();
        let hex = name_hex();
        let vertical = ctx(&hex, 1080, 1920);
        state.finish(t0, begin(&state, t0, false, &dir), DrtOutcome::Unsupported, &vertical);
        state.finish(t0, None, DrtOutcome::NotRequested, &vertical);
        assert!(state.want_pending());
        assert!(begin(&state, t0 + Duration::from_secs(1), false, &dir).is_none());
        assert!(!state.want_pending());

        // Declined because an export is in flight: the want is consumed as well.
        let state = Arc::new(DrtSharedState::default());
        let in_flight = begin(&state, t0, false, &dir).expect("request");
        state.finish(t0, None, DrtOutcome::NotRequested, &vertical);
        assert!(state.want_pending());
        assert!(begin(&state, t0, false, &dir).is_none());
        assert!(!state.want_pending());
        drop(in_flight);
    }

    #[test]
    fn want_is_carried_into_params_once() {
        let dir = TestDir::new();
        let state = Arc::new(DrtSharedState::default());
        let t0 = Instant::now();
        let hex = name_hex();
        let vertical = ctx(&hex, 1080, 1920);
        state.finish(t0, begin(&state, t0, false, &dir), failed("x", None), &vertical);
        state.finish(t0 + Duration::from_secs(1), None, DrtOutcome::NotRequested, &vertical);
        assert!(state.want_pending());

        let first = begin(&state, t0 + Duration::from_secs(2), false, &dir).expect("request");
        assert!(first.params.want);
        assert!(!state.want_pending());
        drop(first);
        let second = begin(&state, t0 + Duration::from_secs(3), false, &dir).expect("request");
        assert!(!second.params.want);
    }

    #[test]
    fn finish_unpublished_records_pacing_without_logging() {
        let dir = TestDir::new();
        let state = Arc::new(DrtSharedState::default());
        let t0 = Instant::now();
        let at = |secs: u64| t0 + Duration::from_secs(secs);
        let hex = name_hex();
        let vertical = ctx(&hex, 1080, 1920);

        state.finish_unpublished(t0, begin(&state, t0, false, &dir), exported(80.0), &vertical);
        let paced = begin(&state, at(5), false, &dir).expect("request");
        assert!(!paced.params.want);
        assert_eq!(paced.params.last_key, timeline_key(&hex, 1080, 1920, "scaleToFit"));
        drop(paced);

        // The unpublished result logged nothing, so the first published one does.
        let d = state.finish(at(10), begin(&state, at(10), false, &dir), exported(80.0), &vertical);
        assert_eq!(mode_and_source(&d), (Some("stretch"), &SizingSource::Drt));
        assert_eq!(d.log_line.as_deref(), Some(VERTICAL_LOG));

        // An unpublished result with another decision leaves the logged pair untouched.
        state.finish_unpublished(at(11), None, DrtOutcome::Skipped, &vertical);
        let d = state.finish(at(20), begin(&state, at(20), false, &dir), exported(80.0), &vertical);
        assert_eq!(mode_and_source(&d), (Some("stretch"), &SizingSource::Drt));
        assert_eq!(d.log_line, None);

        // The explicit-path want is recorded the same way.
        let other = ctx(&hex, 1080, 1350);
        state.finish_unpublished(at(21), None, DrtOutcome::NotRequested, &other);
        assert!(state.want_pending());
    }

    #[test]
    fn log_line_only_on_change() {
        let state = Arc::new(DrtSharedState::default());
        let t0 = Instant::now();
        let hex = name_hex();
        let vertical = ctx(&hex, 1080, 1920);
        let first = state.finish(t0, None, DrtOutcome::Skipped, &vertical);
        assert_eq!(
            first.log_line.as_deref(),
            Some("host_input_sizing: source=api-fallback effective=scaleToFit api=scaleToFit res=1080x1920 last_export_ms=none reason=no-validated-value")
        );
        assert_eq!(state.finish(t0, None, DrtOutcome::Skipped, &vertical).log_line, None);

        // A changed reason is a changed source.
        let d = state.finish(t0, None, failed("x", None), &vertical);
        assert_eq!(
            d.log_line.as_deref(),
            Some("host_input_sizing: source=api-fallback effective=scaleToFit api=scaleToFit res=1080x1920 last_export_ms=none reason=export-failed(x)")
        );
        let d = state.finish(t0, None, DrtOutcome::Skipped, &QueryContext { api_mode: None, ..ctx(&hex, 1920, 1080) });
        assert_eq!(
            d.log_line.as_deref(),
            Some("host_input_sizing: source=api effective=none api=none res=1920x1080 last_export_ms=none")
        );
    }

    #[test]
    fn sweep_removes_own_older_seq_and_old_foreign() {
        let dir = TestDir::new();
        let own = |seq| temp_file_path(&dir.0, PID, seq);
        let foreign_fresh = temp_file_path(&dir.0, PID + 1, 1);
        let foreign_old = temp_file_path(&dir.0, PID + 1, 9);
        let unrelated = dir.0.join("other.drt");
        for path in [own(1), own(2), own(3), foreign_fresh.clone(), foreign_old.clone(), unrelated.clone()] {
            std::fs::write(&path, b"x").unwrap();
        }
        let now = SystemTime::now();
        std::fs::File::options().write(true).open(&foreign_old).unwrap()
            .set_modified(now - Duration::from_secs(2 * 3600)).unwrap();

        assert_eq!(sweep_temp_files(&dir.0, PID, 3, now), 3);
        let exists = [own(1), own(2), own(3), foreign_fresh, foreign_old, unrelated].map(|p| p.exists());
        assert_eq!(exists, [false, false, true, true, false, true]);
    }

    #[test]
    fn begin_request_sweeps_own_older_residue() {
        let dir = TestDir::new();
        let state = Arc::new(DrtSharedState::default());
        let t0 = Instant::now();
        let first = begin(&state, t0, false, &dir).expect("request");
        let late = first.path.clone();
        drop(first);
        // Resolve finishes writing after the guard ran (fuscript killed on timeout).
        std::fs::write(&late, b"late").unwrap();
        let second = begin(&state, t0, false, &dir).expect("request");
        assert_ne!(second.path, late);
        assert!(!late.exists());
    }
}
