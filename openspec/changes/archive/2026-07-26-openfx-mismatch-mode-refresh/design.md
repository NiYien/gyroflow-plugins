# Design: openfx-mismatch-mode-refresh

## 1. Re-verification record (2026-07-26)

Host: `Resolve.exe` ProductVersion **21.0.0.47**, `fuscript.exe` 21.0, Windows 11, Resolve Studio with
External scripting = Local. Project `vertical_test`, timeline `Timeline 1`, clip `DSC_3172.MP4`
(1080×1920 source). Probe scripts and raw results are session-scratch artifacts; the values below are
the record.

This section exists because it **overturns a previously recorded conclusion**. Anyone tempted to
re-blame the host should read it first.

### 1.1 What the old conclusion claimed

From `archive/2026-07-19-ofx-anamorphic-band-guess/design.md:133` and the caveat comment in
`fuscript.rs:96-99`:

> Resolve 21.0.0.47 timeline-settings-dialog edits do not refresh the scripting-API snapshot store
> (`GetSetting` stale until project load / API `SetSetting` / toggling "Use Project Settings" off-on).
> …there is no read path to the live value.

### 1.2 Probe A — is `GetSetting` actually stale?

Method: run `fuscript -q -l lua -x <query>` directly (no plugin involved) before and after each UI edit,
reading `useCustomSettings`, `timelineInputResMismatchBehavior` (timeline and project), and
`timelineResolutionWidth/Height` (timeline and project).

| Step | User action | `ucs` | `tl_mismatch` | `proj_mismatch` | `tl_res` |
|---|---|---|---|---|---|
| A1 | (baseline) | 1 | `scaleToCrop` | `scaleToFit` | 1920×1920 |
| A2 | timeline dialog: mismatch → Fill+Crop, Save | 1 | `scaleToCrop` | `scaleToFit` | 1920×1080 |
| A3 | timeline dialog: mismatch → **Center crop**, Save | 1 | **`centerCrop`** | `scaleToFit` | 1920×1080 |
| A4 | timeline → *Use Project Settings*; project dialog: mismatch → **Fill+Crop**, Save | **0** | `scaleToCrop` | **`scaleToCrop`** | 1920×1080 |

**A3 is the decisive step**: known starting value `scaleToCrop`, user selects Center crop, the single-key
read returns `centerCrop` immediately. A full 314-key dump diff across A2→A3 shows exactly one key
moving:

```
tl.timelineInputResMismatchBehavior = scaleToCrop   ->   centerCrop
```

**A4** repeats the result at project level, which is the configuration most users are in (no timeline
custom settings).

**Verdict: the host refreshes on dialog save, at both levels. The old conclusion is false.**

### 1.3 Why the original spike reached the wrong answer

The original spike dumped settings with the **no-argument** form `tl:GetSetting()` and concluded from
the absence of a live value that "the live model has no read interface (69 keys dumped)". The two forms
are not equivalent:

| Form | What it returned at A1 | Matches the dialog? |
|---|---|---|
| `tl:GetSetting('key')` (single key) | `scaleToCrop`, 1920×1920, `ucs=1` | ✗ |
| `tl:GetSetting()` (no-arg dump) | project-value copy: `scaleToFit`, 1920×1080, no `useCustomSettings` key | ✓ |

The no-arg dump on a timeline returns the project-level value set (and, once custom settings are on,
only the overridden subset — at A2 the dump shrank and gained `tl.useCustomSettings = 1`). Searching it
for a timeline override cannot succeed by construction. The inference chain broke at its first link.

### 1.4 Probe B — does the plugin ever re-read?

The OFX log (`%LOCALAPPDATA%\NiYien\GyroflowNiYien\gyroflow-openfx.log`, timestamps **UTC**, local = +8)
across the whole probe session:

```
06:06:30  CreateInstance restored mismatch from hidden field (mode="scaleToCrop")
06:06:33  CurrentFileInfo { ... mismatch_mode: Some("scaleToCrop"), timeline_w: 1920, timeline_h: 1920 }
          <-- last fuscript query of the session

06:12:27  anamorphic band guess ... size=(1620, 1080)     (user changed the setting at 06:13 / 06:18)
06:13:18  backend=wgpu frames=1
06:16:03  backend=wgpu frames=1
06:17:53  anamorphic band guess ... size=(1620, 1080)
```

The plugin was rendering throughout and issued **zero** queries after instance creation, still deriving
geometry from `scaleToCrop` long after the setting had become `centerCrop`.

Cross-check on the A1 residue: the plugin's own 06:06:33 query and the independent probe 8 s later agree
exactly (`ucs=1 / scaleToCrop / 1920×1920`), i.e. both read the same stale snapshot, while the render
log's `size=(1620,1080)` proves the timeline was actually 1920×1080. So a cross-session snapshot residue
is real — it survived a Resolve restart and a project open — but it is a *first-read* problem, not the
reported *change-does-not-apply* problem, and periodic re-reading dissolves it.

### 1.5 Probe C — clip-level override

| Step | User action | `clip.Scaling` | `tl_mismatch` | `proj_mismatch` |
|---|---|---|---|---|
| C1 | (baseline) | 0 | `centerCrop` | `scaleToFit` |
| C2 | Inspector → Retime and Scaling → Scaling = **Crop** | **1** | `centerCrop` (unchanged) | `scaleToFit` (unchanged) |
| C3 | Scaling = **Stretch** (last dropdown entry) | **4** | — | — |

Dropdown order is `Project Settings / Crop / Fit / Fill / Stretch`, numbered 0..4 in order. Semantic
mapping (user-verified against actual rendering, not inferred from the labels):

| `clip.Scaling` | Inspector label | Equivalent enum | Note |
|---|---|---|---|
| 0 | Project Settings | (defer to timeline/project) | not a mode — must fall through |
| 1 | Crop | `centerCrop` | **not** `scaleToCrop` despite the name |
| 2 | Fit | `scaleToFit` | |
| 3 | Fill | `scaleToCrop` | |
| 4 | Stretch | `stretch` | |

A per-clip override is invisible in `timelineInputResMismatchBehavior`. Out of scope here (see
proposal); the mapping is recorded so the follow-up change does not have to re-derive it, and so nobody
maps "Crop" onto `scaleToCrop` by name.

### 1.6 Cost measurement

Six consecutive invocations of the plugin's own query against a live Resolve:

```
cold  266 ms
warm   82 / 86 / 81 / 83 / 93 ms
```

The historical "fuscript is slow" concern traces to `846b1b3` (2026-05-21, *Stop blocking CreateInstance
on fuscript query*), which removed a **1 s synchronous spin-wait on the UI thread** — Resolve calls
CreateInstance on the UI thread, so waiting there froze the host. The ~85 ms itself is cheap **provided
it stays on a background thread**, which the current fire-and-forget structure already guarantees.

Caveat: measured with Resolve idle. fuscript is a separate process reaching Resolve over IPC, so the
main thread must service the request; latency under heavy playback/export is unmeasured.

## 2. Design decisions

### D1 — Freshness window on the shared cache, not per-instance querying

The setting is per-timeline/per-project. The existing plugin-global `HostInputSizingCache` is the right
shape and is kept; only expiry is added. Rejected alternative: query on every CreateInstance
(unconditional). With 300 plugin-bearing clips that is 300 concurrent `fuscript.exe` processes against a
serialized IPC endpoint — ≥24 s and a process storm. Rejected explicitly by the user on cost grounds.

```
consumer needs the mode
        |
        +-- entry age < TTL  -> use it (zero cost)
        |
        +-- entry age >= TTL -> arm single-flight, spawn ONE background query,
                                use the current (stale) value for this frame;
                                the query's completion updates the cache and,
                                only if the value changed, forces one re-render
```

TTL = **10 s** (user-decided). Steady-state cost is one ~85 ms background query per 10 s process-wide,
independent of clip count.

### D2 — Single-flight is mandatory, not an optimization

Without it, N instances observing expiry in the same frame each spawn a query — reintroducing exactly
the storm D1 avoids, just time-shifted. An atomic in-flight flag (compare-and-swap, cleared by the query
thread on completion) is the minimum. The flag must be cleared on the failure path too, or one failed
query wedges refresh for the rest of the session.

### D3 — One acquisition path; delete the `ProjectPath` gate

Today CreateInstance branches three ways (global-cache hit / `ProjectPath` non-empty → skip fuscript and
read the hidden field / fresh drop → query). The middle branch is what makes a `.drp` restore reuse a
frozen mode. With D1 in place the branch is unnecessary: a restore that finds a fresh cache uses it, and
one that finds a stale/empty cache refreshes exactly once for all restored instances.

The fresh-drop cache invalidation is **removed too** (user-decided 2026-07-26). It existed only because
the cache had no expiry — re-dropping the node was the sole user-reachable way to force a re-read. Once
a TTL governs staleness it is redundant, costs one extra query per drop, and leaves two differently
shaped invalidation routes to reason about. After this change CreateInstance has exactly one shape:
call the freshness-checked helper, same as the render path.

### D4 — Hidden field demoted to cold-start fallback

`DetectedMismatchMode` stays defined (paste/`.drp` round-trips must keep serializing it) and stays
useful for the genuine no-fuscript cases (Resolve Free, compound clip, scripting disabled). It must not
outrank a returning query, otherwise a value frozen in the project file resurrects itself every session
— which is what probe A1 caught happening. Precedence: **live query > global cache (fresh) > global
cache (stale, pending refresh) > hidden field > `Fit` fallback**.

### D5 — FlipX only on change

`fuscript.rs:163-165` fires `c:SetProperty('FlipX', c:GetProperty('FlipX'))` after every successful
query to force a re-render. Under periodic refresh that becomes a forced redraw every TTL window →
visible preview flicker. The trigger must be conditional on the parsed value differing from what the
cache already holds. This is a correctness requirement of D1, not a nicety.

### D6 — Kill-switch

`GYROFLOW_OFX_MISMATCH_TTL_MS`: unset → 10000; `0` → never expire (sticky-cache, pre-change behavior);
otherwise the parsed value, clamped to a sane band. Invalid → default + warn. Resolve once via
`OnceLock` and log one line (`target: "ofx"`) alongside the other resolved-config lines. Note this is a
behavioral revert, not a byte-equivalent one — D3/D4/D5 remain active at TTL=0. Full byte-level revert
is `git revert`.

### D7a — Attempt pacing, not just single-flight (found during implementation)

Single-flight bounds *concurrency*, not *rate*. On a host where the query can never succeed
(Resolve Free, external scripting disabled, compound clip) the cache never gets populated, so every
frame evaluates as stale, and the guard is released the instant each attempt fails — the result is one
spawned `fuscript.exe` per rendered frame, which is worse than the bug being fixed.

A separate "last attempt" timestamp gates retries to the same cadence as successful refreshes. With
expiry disabled (`TTL=0`) the cache can still be empty on a fuscript-less host, so the gate falls back
to the default 10 s cadence rather than to no gap at all.

Same reasoning applies to `CurrentFileInfo::is_available()`, which stats the filesystem: it is called
only after the pacing gate, so a non-Resolve host pays one stat per cadence window, not one per frame.

### D7b — Timestamp follows the query, not the mirror (found during implementation)

The render-path mirror block runs on **every** render. Refreshing `populated_at` there unconditionally
would keep the entry permanently fresh and silently disable expiry — the change would compile, pass
tests, and do nothing.

`CurrentFileInfo` therefore carries `queried_at: Option<Instant>`, set by the query thread. The mirror
promotes an entry into the shared cache only when `queried_at` is newer than the cached
`populated_at`. Two cases correctly read as "not new": a value adopted from the cache (inherits the
cache timestamp) and a hidden-field restore (`None`, never queried). The latter also prevents a mode
frozen in the project file from being promoted into the shared cache — which is how a stale value used
to resurrect itself every session.

Cross-instance propagation falls out of the same field: `ensure_host_input_sizing_fresh` adopts the
shared entry into an instance whose `queried_at` is older, so a refresh armed by one instance reaches
every other instance on its next render. Without that, only the instance that happened to win the
single-flight race would ever see the new mode.

**Lock order**: the cache entry is cloned and its lock dropped before `current_file_info` is taken,
because the mirror block acquires them in the opposite order (info → cache). Overlapping them in
`ensure_host_input_sizing_fresh` would invert the order and deadlock.

### D7c — `LoadCurrent` / `ReloadProject` stay outside single-flight

Those two paths are explicit user actions and keep their unguarded `query` / `query_silent` calls. They
can in principle race a refresh query; last writer wins, both write the same fields from the same
source, and a user-initiated read should not be suppressed by a background one. Accepted.

### D7 — Playback/export not suppressed — **FALSIFIED by live testing 2026-07-26**

Original reasoning: "Re-query is not gated on transport state. At 10 s TTL the load is one background
query per 10 s, and suppressing during playback would withhold the update precisely when the user is
looking at the result. If IPC latency during heavy playback turns out to matter, gating is a one-line
addition." The unmeasured premise was flagged in §1.6 and turned out to be the wrong way round.

**What actually happens**: during 60 fps playback the query does not merely get slower — it *never
returns*. Resolve's main thread is saturated and never services the IPC request. Measured live: four
`fuscript.exe` processes alive simultaneously (212 / 170 / 110 / 50 s), spaced at exactly the 60 s
stuck-guard reclamation interval, growing without bound for as long as playback continued.

The stuck-guard reclamation (D2/9.2) did its job — the refresh stayed alive rather than wedging — but
it re-armed while the previous child was still hanging, converting a wedge into a process leak.

**Resolution** (two layers, both needed):

1. The query owns a deadline (`QUERY_TIMEOUT_MS`, 5 s): spawn + poll + kill, replacing the unbounded
   `output()`. This bounds the damage regardless of why the host is unresponsive, and demotes the 60 s
   reclamation back to pure insurance.
2. Consecutive-failure back-off stretches the retry cadence 1x → 6x TTL and resets on the first
   success. Without it, playback still costs one spawned-and-killed process per window; with it, a
   long playback settles at one attempt per minute.

Transport state is still not consulted. It does not need to be: the two layers above make a busy host
cheap to probe, and any heuristic for "is the user playing back" would also have to cover export,
background caching and scrubbing. The failure counter measures the thing that actually matters —
whether queries are getting through — rather than guessing at the cause.

### D7e — Pre-existing: the host-input-sizing diagnostics were never reaching the log

Found while verifying 7.2 live. `common/src/lib.rs:259` builds the logger with
`add_filter_ignore_str` over `["mp4parse", "wgpu", "naga", "akaze", "ureq", "rustls", "ofx"]`, and
simplelog drops any record whose target starts with one of those. All eight of the plugin's own
host-input-sizing diagnostics used `target: "ofx"` and were therefore **silently discarded** — including

```
host_input_sizing: mode=… crop=(WxH) offset=(x,y) source=… baked_stretch=… video_rotation=…
```

which is the one line that shows what the FillCrop crop model actually computed. This is pre-existing
(the `"ofx"` filter entry predates the host-input-sizing feature; it exists to silence the `ofx` crate)
and explains why FillCrop geometry has been hard to diagnose across several sessions: the evidence was
being generated and thrown away.

All eight moved to `target: "host_input_sizing"`, which is not filtered and was already in use by the
hidden-field restore line — the one host-input-sizing log that has always been visible.

### D7d — Known boundary: the cache is written by the render mirror, not the query thread

A query writes only the `CurrentFileInfo` of the instance that armed it; the shared cache is populated
by that instance's next render (the mirror block). Every other instance then adopts it via
`ensure_host_input_sizing_fresh`. This keeps the change small — the cache lives on `GyroflowPlugin`,
which the query thread in `fuscript.rs` cannot reach without hoisting it to a module-level static.

Boundary: if the instance that armed the query never renders again, the result never reaches the shared
cache and other instances keep their previous value until one of them arms its own refresh (one pacing
window later). In practice the arming instance is the one being rendered, so this is theoretical; if it
ever bites, the fix is to make the cache a process-global static that the query thread writes directly.

### D9 — FillCrop write-back must transpose to storage orientation (scope extension)

Evidence, from the first live run with the refresh working (log 07:30:21, portrait anamorphic
DSC_3172, timeline 1920×1080, `scaleToCrop`):

```
host_input_sizing: mode=FillCrop crop=(1620x608) offset=(0,656)
                   source=(1920, 1620) baked_stretch=(1.0, 1.5) video_rotation=90
```

Trace through `compute_fillcrop_geometry_desqueezed`:

| step | value | correct? |
|---|---|---|
| desqueeze `source / stretch` | `phys = (1920, 1080)` | ✓ |
| rotation 90 transpose (in `compute_crop_geometry`) | display `(1080, 1920)` | ✓ |
| FillCrop into a 1.7778 timeline | visible band `1080 × 608`, offset `(0, 656)` | ✓ |
| map back through display-axis stretch `(1.5, 1.0)` | `(1620, 608)`, offset `(0, 656)` | ✓ *as a display rect* |
| caller writes it into `params.size` | `(1620, 608)` | ✗ **storage orientation expected** |

The returned rect is display-oriented; `params.output_size` is display-oriented and is correct, but
`params.size`, `calib_dimension` and the camera-matrix principal point are storage-oriented. The
original values make the relationship obvious: `size=(1920,1620)` vs `output_size=(1620,1920)` — a
transpose pair. After the crop both were being set to the same `(1620,608)`.

Correct write-back under 90/270:

| field | orientation | was | should be |
|---|---|---|---|
| `params.size` | storage | (1620, 608) | **(608, 1620)** |
| `params.output_size` | display | (1620, 608) | (1620, 608) — unchanged |
| `calib_dimension` | storage | (1620, 608) | (608, 1620) |
| camera matrix `cx/cy` offset | storage | −(0, 656) | −(656, 0) |

Fix shape: keep the geometry function's display-oriented contract (it is the natural space for the
crop computation and `compute_crop_geometry` already transposes into it), and add an explicit
`crop_display_to_storage(crop, rotation)` mapping at the write-back site. Extracted as a pure function
so the transpose is unit-testable without a `StabilizationManager`.

Non-rotated clips are unaffected: display and storage coincide, and the mapping is the identity —
byte-equivalent to the previous behavior. Offsets need no 90-vs-270 distinction because the crop is
always centered, so the offset is the centering value on each axis and stays centered under transpose.

Why it stayed hidden: symmetric lenses put the principal point at the calibration centre, so the
`cx`/`cy` axis swap produces no visible shift, and until the refresh landed the plugin never resolved
`scaleToCrop` on a rotated clip in the first place.

## 3. Risks

- **Unmeasured IPC latency under load** (D7). Mitigation: background thread, single-flight, and the
  render path never waits on the result — it uses the previous value for the current frame.
- **Refresh churn on the stab manager.** A mode change invalidates `applied_host_input_sizing` and
  re-applies the input-side transform. Correct, but it must go through the existing
  `restore_host_input_sizing_baseline()` discipline (`gyroflow.rs:679`) so an incoming change restores
  the clean baseline before applying the new mode — otherwise a Fit→FillCrop→Fit round trip accumulates
  mutation, the same class of bug as the InputRotation baseline poisoning fixed in `6b8cb14`.
- **Clip-level override still wrong** (out of scope). Users on Inspector-level Scaling see no
  improvement; the release note must say so.
- **`ReloadProject` remains hidden.** This change makes the automatic path work, so the button is no
  longer load-bearing; unhiding it is a separate UI decision, not required here.
