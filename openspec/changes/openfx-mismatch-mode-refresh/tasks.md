# Tasks: openfx-mismatch-mode-refresh

## 1. Config plumbing

- [x] 1.1 Add `mismatch_ttl_ms()` helper (`OnceLock`, env `GYROFLOW_OFX_MISMATCH_TTL_MS`, default 10000,
      `0` = never expire, clamp to a sane band, invalid → default + warn) mirroring the existing
      resolved-config helpers; emit one `target: "ofx"` line
      `mismatch ttl resolved: ttl_ms=… source=default|env|env_clamped|default_invalid`
      — clamp band `[500, 600000]`; `0` deliberately bypasses the min clamp
- [x] 1.2 Unit-test the pure parse function (default / valid / `0` / clamped / garbage) with no env or
      `OnceLock` involvement, same shape as the other parse helpers in the tree — 5 tests added

## 2. Cache with freshness + single-flight

- [x] 2.1 Extend `HostInputSizingCacheEntry` with a monotonic timestamp (`Instant`); populate it at
      every write site (render-path mirror block, and any new write)
      — **D7b**: the timestamp had to follow the *query*, not the mirror. The mirror runs every
      render; refreshing it there unconditionally keeps the entry permanently fresh and silently
      disables expiry. Added `CurrentFileInfo::queried_at: Option<Instant>`; the mirror promotes an
      entry only when `queried_at` is newer than the cached `populated_at`.
- [x] 2.2 Add a process-global in-flight flag (`AtomicBool`) next to `host_input_sizing_cache`
- [x] 2.3 Add `ensure_host_input_sizing_fresh(&self, info_arc, pending_arc)`: read the cache, return the
      current value immediately, and — when the entry is missing or older than the TTL and the
      in-flight CAS succeeds — spawn one background query. Never blocks.
      — also adopts a newer shared entry into this instance (cross-instance propagation, D7b) and
      clones the entry before taking `current_file_info` to preserve the mirror's lock order
- [x] 2.4 Clear the in-flight flag on **both** the success and failure paths of the query thread
      — implemented as an RAII `InFlightGuard` in `query_inner`, which also covers early returns
      and panics
- [x] 2.5 Verify TTL=0 short-circuits the expiry check so the entry is used regardless of age
      — `HostInputSizingCacheEntry::is_stale()` returns false when TTL is 0; parse-level coverage in
      `parse_mismatch_ttl_zero_disables_expiry_and_is_never_clamped`. Behavioural check is 7.7 (live).
- [x] 2.6 **Added during implementation (D7a)**: attempt pacing. Single-flight bounds concurrency, not
      rate — on a host where the query can never succeed the cache stays empty, every frame reads
      stale, and the guard releases on each failure, spawning one process per frame. Added
      `host_input_sizing_last_attempt` gating retries to the same cadence (falling back to the
      default cadence when TTL=0). `is_available()` (a filesystem stat) moved behind that gate.

## 3. Single acquisition path

- [x] 3.1 CreateInstance: replace the three-way branch with one call to
      `ensure_host_input_sizing_fresh`; delete the `project_path_at_create.is_empty()` gate that skips
      fuscript on paste / `.drp` restore (D3)
- [x] 3.2 Render path: call `ensure_host_input_sizing_fresh` once per render before resolving the mode
      (cheap — a clone plus an age compare when fresh), placed before `check_pending_file_info` so a
      result that landed since the previous frame is consumed by the existing pending-flag machinery
- [x] 3.3 Remove the fresh-drop cache invalidation block — user-decided 2026-07-26, expiry subsumes it
      (D3). CreateInstance now has exactly one shape: call the freshness-checked helper, same as the
      render path.
- [x] 3.4 Confirm the existing idempotency guard picks up a changed mode without extra plumbing —
      `apply_host_input_sizing_if_needed` compares `applied_host_input_sizing == Some(mode)`, so a
      refreshed value re-applies on the next render. No change needed.

## 4. Hidden-field demotion (D4)

- [x] 4.1 Consult `DetectedMismatchMode` only when the global cache is empty AND no query has returned
      this session; never let it overwrite a query result — gated on
      `current_file_info.lock().is_none()` *after* `ensure_host_input_sizing_fresh` has had its
      chance to populate it (bool hoisted out of the `if` condition: a `parking_lot` guard in an
      `if` condition lives to the end of the statement and would self-deadlock on the inner lock)
- [x] 4.2 Keep the render-path mirror write (persistence for genuine no-fuscript hosts) but ensure it
      writes the *latest queried* value, so a stale project-file value cannot resurrect — the
      hidden-field restore sets `queried_at: None`, which makes the mirror's `is_new_query` false,
      so a persisted value is never promoted into the shared cache
- [x] 4.3 Keep the param defined and secret; paste / `.drp` round-trips must still serialize it —
      untouched

## 5. FlipX on change only (D5)

- [x] 5.1 `fuscript.rs`: fire the `SetProperty('FlipX', …)` re-render trigger only when the parsed
      mismatch mode / timeline dims / `useCustomSettings` differ from the previous value — compared
      against the instance's own prior `CurrentFileInfo`, captured before the overwrite.
      `file_path` / `project_path` are included in the comparison so `LoadCurrent` still forces its
      redraw when it resolves a new clip.
- [x] 5.2 Confirm the cold-start case still triggers exactly one re-render — previous value `None`
      evaluates as changed

## 6. Comment / doc corrections

- [x] 6.1 Replace the "Known host caveat (Resolve 21.0.0.47)" block in `fuscript.rs` — it asserted the
      disproven claim. Now states the measured behaviour, warns against switching to the no-arg
      `GetSetting()` dump form, and notes the per-clip `Scaling` gap.
- [x] 6.2 Correct the same claim in `archive/2026-07-19-ofx-anamorphic-band-guess/design.md` — original
      text struck through in place, dated retraction note appended below it (history not rewritten)
- [x] 6.3 User-facing docs: `README-OpenFX.md` "Runtime caveat" told users to press `Reload project`,
      a button that is `hidden: true` and unclickable. Replaced with the automatic-refresh behaviour +
      the env var, plus a new **Known limitation** paragraph for the per-clip Scaling override.

## 6b. Log visibility (found during live verification)

- [x] 6b.1 **Pre-existing defect (D7e)**: all 8 host-input-sizing diagnostics used `target: "ofx"`, which
      `common/src/lib.rs:259` adds to simplelog's ignore list — they were never written to the log.
      Moved to `target: "host_input_sizing"` (already in use, not filtered). This restores the
      `mode=… crop=(WxH) offset=(x,y) source=… baked_stretch=… video_rotation=…` line needed to
      diagnose FillCrop geometry.

## 8. FillCrop rotation transpose (scope extension, user-decided 2026-07-26 during live verification)

- [x] 8.1 Root cause pinned from the first live run with the refresh working (log 07:30:21):
      `compute_fillcrop_geometry_desqueezed` returns a **display-oriented** rect, the caller wrote it
      verbatim into the **storage-oriented** `params.size` / `calib_dimension` / principal point.
      Portrait anamorphic clip got `size=(1620,608)` where `(608,1620)` was required → flattened
      picture. This is leftover defect #1 from `6b8cb14`, dormant because symmetric lenses hide the
      principal-point axis swap and FillCrop never ran on a rotated clip before.
- [x] 8.2 Add pure `crop_display_to_storage(crop, rotation) -> ((w,h),(x,y))` — transpose on 90/270,
      identity otherwise. Offsets need no 90-vs-270 split (the crop is always centred).
- [x] 8.3 Rewire the FillCrop/CenterCrop write-back: `output_size` keeps the display rect;
      `params.size`, `calib_dimension` and the `cx`/`cy` shift use the transposed values. Log line
      extended with `store=(WxH) store_offset=(x,y)`.
- [x] 8.4 Tests: quarter-turn transpose (incl. negative / >360 normalization), identity for
      0/180/360/−180, and an end-to-end check over both pure functions asserting the exact values
      observed live (`(1920,1620)` + stretch `(1.0,1.5)` + rot 90 + 1.7778 timeline → display
      `(1620,608,0,656)` → storage `((608,1620),(656,0))`). 49 passed, 0 failed.
- [ ] 8.5 **Live**: the portrait anamorphic clip under `scaleToCrop` renders with correct geometry
      (not flattened); log shows `store=(608x1620) store_offset=(656,0)`
- [ ] 8.6 **Live regression**: an unrotated clip under `scaleToCrop` is unchanged from before the fix
      (identity path)

## 9. Audit remediation (4 parallel adversarial audits, 2026-07-26)

Audits confirmed as correct and requiring no change: the D9 transpose (independently re-derived
from first principles, all four fields match), the lock-ordering claim (verified with a `try_lock`
compile probe, not by reading), `InFlightGuard` coverage, absence of deadlock, `Fit↔FillCrop`
round-trip exactness, unrotated byte-equivalence, and the per-frame cost (~80-200 ns, zero syscalls).

- [x] 9.1 **CRITICAL regression** (found by three audits independently, confirmed against the live
      log): `query_refresh` reused the full publication path, so every TTL window republished
      clip-level fields and raised `current_file_info_pending` → `check_pending_file_info`
      unconditionally rewrote `ProjectPath` → `param_changed` calls `clear_stab` for `ProjectPath`
      regardless of `user_edited` → full project re-import. Log shows the whole chain twice, 20 s
      apart (`new stab manager` → `[import_gyroflow]` → 3× `recompute_blocking` →
      `backend_rebuild first_init`). Second symptom: the lua reads `GetCurrentVideoItem()` — the
      *playhead's* clip — so on a multi-clip timeline an instance could adopt another clip's
      project path, and it persists into the `.drp`.
      Fixed with a `publish_clip_fields: bool` on `query_inner`: the refresh path updates only the
      four host-sizing fields plus `queried_at`, and never touches the pending flag.
- [x] 9.2 **Single-flight could wedge permanently**: `cmd.output()` has no deadline, so a fuscript
      that never returns held the guard for the session and silently killed the refresh — with no
      log signal. Added `STUCK_QUERY_MS` (60 s) guard reclamation; `last_attempt` is now stamped
      only when a query is actually armed, so its age is the in-flight query's age.
- [x] 9.3 **FlipX child never reaped**: `Child` does not reap on drop; on macOS/Linux a periodic
      trigger would accumulate one zombie per window (~2,880 per 8 h session, against a default
      `kern.maxprocperuid` near 2,666). Now reaped on a helper thread.
- [x] 9.4 **`Command::args` appends**: the FlipX trigger reused `cmd`, so its argv was
      `-q -l lua -x <query> -x <flipx>` — it may have been re-running the query rather than
      triggering the redraw. Now builds a fresh `Command`.
- [x] 9.5 **`GYROFLOW_OFX_MISMATCH_TTL_MS=0` did not do what the README promised**: with an empty
      cache it still armed a query every 10 s forever (exactly the population that would reach for
      the switch). Now bootstraps at most once, then never re-queries.
- [x] 9.6 **CenterCrop used FillCrop geometry** (pre-existing, High; user-approved scope
      extension). Resolve's `centerCrop` is "no resizing" — visible region is
      `min(timeline, source)` per axis. Two failure modes: the visible band was under-reported
      1.78× on the live config, and in the matching-aspect case (3840×2160 → 1920×1080) the shared
      model returned `None` while the host was really doing a 2× centre crop. Added
      `compute_centercrop_geometry_desqueezed` (takes timeline *dimensions*, not aspect) and split
      the match arm. 4 new tests.
- [x] 9.7 **Stretch wrote a display-oriented size** (pre-existing, Medium; user-approved). Same
      defect class as D9, three lines below it. Routed through `crop_display_to_storage`.
- [x] 9.8 README: the Center Crop bullet claimed "same math as Fill+Crop for the common case",
      which is affirmatively false. Rewritten.
- [x] 9.9 **1 px off-centre crop under rotation** (user-directed 2026-07-26, was deferred). Fixed at
      the source instead of modelling each rotation's origin: `even_margin` trims the crop by at
      most one pixel so `extent - crop` is even and the crop is *exactly* centred, making the
      display→storage offset unambiguous for 90/270/180. Applied to both the FillCrop and the
      CenterCrop paths. Not theoretical — the existing R5MK2 test (5760×2160, h=1.5, portrait
      timeline) hits an odd 2625 px margin and was asserting the off-centre values (1312 left /
      1313 right); updated to the centred result with the reason recorded in the test.
- [x] 9.10 **`calib_dimension := crop` rebased the lens scale** (user-directed 2026-07-26, was
      deferred). The runtime scale is `params.size / calib_dimension`; writing the crop straight
      into `calib_dimension` forced it to 1.0, which silently rescaled the focal length whenever
      the profile was calibrated at a different resolution than the source (e.g. 1.5× vertical
      error for a calib-1920×1080 profile on a 1920×1620 source). The crop and the principal-point
      shift are now expressed in calibration space via the existing ratio, so the scale is
      preserved. When `calib == size` both factors are 1.0 and the behaviour is unchanged — which
      is the built-in anamorphic preset case, hence no test churn there.

## 10. Live-verification round 2 (2026-07-26): query hangs during playback

Round-2 live run confirmed 9.1 fixed — no `new stab manager` / `[import_gyroflow]` attributable to
a refresh, and the setting change still takes effect. It also falsified design decision **D7**
("playback/export not suppressed; the added load is one ~85 ms background query per 10 s").

Observed: during 60 fps playback, `fuscript` queries **never return**. Resolve's main thread is
saturated and the IPC request is not serviced. The 9.2 stuck-guard reclamation kept the refresh
alive, but re-armed every 60 s while the previous child was still hanging — 4 live `fuscript.exe`
processes after ~3 minutes, one per reclaim window, growing without bound. Evidence: two
`reclaiming the single-flight guard` warnings, and `Get-Process fuscript` showing PIDs alive for
212 / 170 / 110 / 50 s at exactly 60 s spacing.

- [x] 10.1 Give the query itself a deadline (`QUERY_TIMEOUT_MS`, 5 s — ~20x a cold query). Replaces
      the unbounded `output()` with spawn + `try_wait` polling + `kill`. Polling without draining
      is safe here: the query prints ten short lines, far below the pipe buffer. This is the fix
      that stops the leak; the 60 s reclamation reverts to being pure insurance.
- [x] 10.2 Add consecutive-failure back-off (`host_input_sizing_failures`, RAII `FailureTracker`):
      retry cadence stretches from 1x to 6x the TTL while attempts keep failing, resets on the
      first success. Without it, playback still costs one spawned-and-killed process every window
      for as long as playback lasts. Also covers the lua-error case (playhead parked on a title,
      gap or compound clip — seen in the same log:
      `fuscript stderr: [string "???"]:1: attempt to index a nil value`).
- [x] 10.3 **Live verified**: playback produced four `exceeded 5000ms and was killed` warnings at
      20 / 31 / 40 s spacing — the back-off curve (2x, 3x, 4x TTL) behaving exactly as designed —
      and **zero** `reclaiming the single-flight guard` warnings, i.e. the query deadline took over
      from the reclamation. Reload markers appear only at project load.
- [x] 10.4 **Live verified**: after playback stopped, queries succeeded again at `08:47:34` and
      `08:47:45` (11 s apart = back-off reset to 1x TTL by the first success).
- [x] 10.5 **Gap found during 10.3 review**: the FlipX re-render trigger had reaping but no
      deadline. It goes through the same Resolve IPC endpoint as the query, so it hangs under the
      same conditions, and a plain `wait()` would leak both the child and the reaper thread for
      the session. Given the same `QUERY_TIMEOUT_MS` deadline.
- [x] 10.6 **Live verified — no process leak.** A single surviving `fuscript.exe` after each run was
      traced via `Win32_Process`: command line `fuscript.exe -s`, parent `Resolve`. That is
      Resolve's own persistent scripting server, not a query — the plugin's queries use
      `-q -l lua -x <script>`. Zero plugin-spawned processes survive, against four leaking at 60 s
      intervals before 10.1/10.2. (Diagnostic note for future sessions: match on the command line,
      not the image name — Resolve keeps one `-s` instance alive for its whole session.)
- [x] 10.7 **Live verified — CenterCrop correct on first real run.** Log:
      `mode=CenterCrop crop=(1620x1080) offset=(0,420) store=(1080x1620) store_offset=(420,0)
      source=(1920, 1620) baked_stretch=(1.0, 1.5) video_rotation=90` — identical to the unit test
      `centercrop_keeps_min_of_timeline_and_source_not_an_aspect_band`. The old shared FillCrop
      model produced `(1620, 608) @ (0, 656)` for the same input.

## 7. Verification

- [x] 7.1 Unit tests green — `cargo test -p gyroflow-ofx --lib`: **46 passed, 0 failed** (41 pre-existing
      + 5 new TTL-parse tests). `cargo check -p gyroflow-ofx` clean.
- [ ] 7.2 **Live**: change the mismatch setting at *project* level while the plugin renders → log shows
      one new `CurrentFileInfo` within the TTL window, geometry switches, no restart, no node re-drop
- [ ] 7.3 **Live**: same at *timeline* level (`useCustomSettings=1`)
- [ ] 7.4 **Live**: preview parked on a clip for ≥60 s with the setting untouched → no visible flicker
      (proves D5) and one query per TTL window in the log, not per frame
- [ ] 7.5 **Scale**: a timeline with many plugin-bearing clips (≥20 as a proxy for the 300 case) →
      project open produces **one** query, not one per instance; check with a log count
- [ ] 7.6 **Restore**: set mode, save `.drp`, change the setting in Resolve, reopen the project →
      the refreshed value wins over the persisted hidden field (D4)
- [ ] 7.7 `GYROFLOW_OFX_MISMATCH_TTL_MS=0` → sticky behavior returns (A/B revert path works)
- [ ] 7.8 Regression: Resolve Free / scripting disabled → no dialog spam, `Fit` fallback intact,
      hidden-field fallback still supplies a mode when present
