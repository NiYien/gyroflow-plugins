# Proposal: openfx-mismatch-mode-refresh

## Why

Users report that changing DaVinci Resolve's mismatched-resolution setting does not take effect in the
plugin until Resolve is closed and reopened. The previous verdict on this symptom — recorded in
`archive/2026-07-19-ofx-anamorphic-band-guess/design.md:133` — attributed it to a Resolve 21.0.0.47 host
defect ("timeline-settings-dialog edits do not refresh the scripting-API snapshot store; `GetSetting`
stale until project load / API `SetSetting` / toggling *Use Project Settings*").

**That verdict is wrong.** A live re-verification on 2026-07-26, on the same host build
(`Resolve.exe` ProductVersion `21.0.0.47`), shows `GetSetting` refreshing immediately after the settings
dialog is saved — at both the timeline level and the project level. The full probe record is in
`design.md`. Two defects, both on the plugin side, produce the reported symptom:

1. **The plugin never re-reads.** `CurrentFileInfo::query*` has exactly three call sites
   (`gyroflow.rs:2020` CreateInstance-with-empty-`ProjectPath`, `:2035` `LoadCurrent`, `:2044`
   `ReloadProject`). None of them corresponds to "the user changed a Resolve setting", and the render
   path never re-queries. During the live probe the plugin was actively rendering while the setting was
   changed twice and issued **zero** fuscript queries, continuing to compute geometry from the mode it
   had read at instance-creation time. Restarting Resolve appears to help only because it forces
   instance re-creation, which happens to hit one of the three call sites — not because it refreshes
   anything host-side.
2. **The documented refresh path is unreachable.** Requirement §8.2 re-queries fuscript on
   `ReloadProject` specifically so "a user that just toggled Resolve's mismatched-resolution setting
   between renders can have the new value picked up". That button is defined with `hidden: true`
   (`common/src/lib.rs:539`, set since `2846f3b` 2026-04-28, i.e. before this feature existed) and is
   therefore `set_secret(true)` in OFX — no user can click it.

The plugin-global `HostInputSizingCache` already solves the "don't query once per clip" problem
correctly (one query per session, shared by every instance). What it lacks is expiry: once filled it is
sticky for the entire Resolve session, invalidated only by a fresh drop with an empty `ProjectPath`.
The hidden `DetectedMismatchMode` param then freezes a stale value into the `.drp` so it survives
restarts as well.

Scaling constraint that shapes the fix: the naive "re-query whenever an instance is created" approach
does not scale. A timeline with 300 plugin-bearing clips would spawn 300 concurrent `fuscript.exe`
processes at project open, each requiring a serialized IPC response from Resolve's main thread
(measured 81–93 ms warm, 266 ms cold → ≥24 s aggregate, plus process-storm risk). The setting is
per-timeline/per-project, not per-clip, so one query must serve all instances.

## What Changes

- **Give the plugin-global cache a freshness window.** `HostInputSizingCacheEntry` gains a timestamp.
  A read whose entry is older than the TTL (default **10 s**, user-decided) triggers exactly one
  background re-query; fresh entries are used as-is at zero cost.
- **Single-flight the re-query.** An in-flight flag guarantees that N instances simultaneously observing
  an expired entry produce one `fuscript` invocation, not N. Cost is independent of clip count: one
  ~85 ms background query per TTL window for the whole session, whether the timeline has 1 clip or 500.
- **Unify the value-acquisition path.** Every point that needs the mode (CreateInstance included) goes
  through the same "check freshness → use or refresh" helper. This removes the
  `ProjectPath`-non-empty-skips-fuscript special case (`gyroflow.rs:1978`), which is what makes a
  `.drp` restore silently reuse a frozen mode.
- **Demote the hidden `DetectedMismatchMode` param to a cold-start-only fallback.** It is consulted only
  when the global cache is empty *and* no query has yet returned (i.e. fuscript unavailable / Resolve
  Free / compound clip). A returning query always wins over the persisted value.
- **Trigger the FlipX forced re-render only on an actual value change.** `query_silent` currently fires
  `c:SetProperty('FlipX', c:GetProperty('FlipX'))` unconditionally on every successful query. Under a
  periodic re-query that would force a redraw every TTL window and make the preview flicker.
- **Add kill-switch `GYROFLOW_OFX_MISMATCH_TTL_MS`.** `0` disables expiry, restoring the sticky-cache
  behavior for A/B diagnosis. Non-zero overrides the 10 s default. Clamped, invalid → default + warn,
  resolution logged once (house pattern).
- **Not in scope**: the clip-level `Scaling` override (see below), any gyroflow-core change, any Adobe /
  frei0r change, any new visible UI parameter.

### Scope extension (user-decided 2026-07-26, during live verification): FillCrop rotation transpose

Live verification of the refresh worked — and immediately exposed a pre-existing geometry defect that
the stale cache had been hiding. This was already on record as leftover defect #1 from `6b8cb14`
("旋转+真裁切写回朝向转置+camera matrix 轴向错（对称镜头被 calib/2 覆盖遮蔽）"), deferred at the time
because a symmetric lens hides the principal-point axis swap.

`compute_fillcrop_geometry_desqueezed` returns a **display-oriented** crop rect. The caller writes it
verbatim into `params.size`, `calib_dimension` and the camera-matrix principal point, all of which are
**storage-oriented** — orientations that a 90°/270° `video_rotation` transposes. Observed live on a
portrait anamorphic clip (stored 1920×1080, rotation 90, `v_stretch` 1.5) in a 1920×1080 timeline set to
`scaleToCrop`: `size` became `(1620, 608)` instead of `(608, 1620)`, flattening the picture so it looked
stretched. The physical crop itself (`1080×608` before the stretch mapping) is correct; only the
write-back orientation is wrong.

Included here rather than deferred because the refresh mechanism is what makes FillCrop actually run on
rotated clips — shipping the refresh alone would turn a dormant defect into a visible regression.

### Explicitly out of scope: clip-level `Scaling` override

The probe found a **second, independent** defect: Resolve's Inspector → Retime and Scaling → **Scaling**
dropdown is a per-clip override of the mismatch behavior, stored in `timelineItem:GetProperty()` as
`clip.Scaling` (verified mapping: `0` Project Settings / `1` Crop=`centerCrop` / `2` Fit=`scaleToFit` /
`3` Fill=`scaleToCrop` / `4` Stretch=`stretch`; semantics user-confirmed). Changing it moves **only**
`clip.Scaling` — `timelineInputResMismatchBehavior` does not move at all, so the key the plugin reads is
blind to it. No amount of re-querying fixes this.

It is deliberately deferred because it needs a different mechanism: `GetCurrentVideoItem()` follows the
playhead (the probe returned `clip = <none>` once the playhead left the clip), which is unusable at
render time on multi-track or batch renders. Correct clip identification requires walking
`GetItemListInTrack` and matching by file path, with its own live verification matrix. Tracked as a
follow-up change.

## Verification status at archive (2026-07-26)

Archived with the main path live-verified and six edge cases deliberately left unverified. Shipped in
commit `4061a5a`.

**Live-verified**: setting changes picked up without restart at both project and timeline level; no
periodic project re-import; no plugin-spawned process leak; failure back-off curve (1x→4x TTL) and its
reset on success; FillCrop rotation transpose; CenterCrop geometry (log values identical to the unit
test). 55 unit tests pass.

**Unverified, with risk notes in `tasks.md` §7.4–7.8 / §8.6**: idle-preview no-flicker; ≥20-instance
project open issuing a single query; `.drp` restore precedence; `TTL=0` revert path; Resolve
Free / scripting-disabled behaviour; unrotated-clip regression. The two carrying the most residual
risk are the `.drp` restore path (§7.6 — substantially reworked, only reasoned about) and the
many-instance case (§7.5 — the scenario the design is optimised for).

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `openfx-host-input-sizing`: the plugin-global mismatch cache SHALL carry a freshness window and SHALL
  be refreshed by a single-flight background query when stale; every consumer (CreateInstance and the
  render path) SHALL acquire the mode through that one path; the hidden per-node field SHALL be a
  cold-start fallback only, never an override of a returning query; the forced re-render SHALL fire only
  when the queried value differs from the cached one.

## Impact

- **Code**: `openfx/src/gyroflow.rs` (cache entry + TTL/single-flight helper, CreateInstance bootstrap,
  render-path refresh hook, hidden-field demotion) and `openfx/src/fuscript.rs` (change-gated FlipX,
  TTL config helper). No `common/` change required unless the follow-up decision is to also unhide
  `ReloadProject`.
- **Behavior**:
  - Changing the mismatched-resolution setting (project level or timeline level) takes effect within
    ~10 s with no user action, no node re-drop, and no Resolve restart.
  - Project open with N plugin instances: one query total (unchanged from today's shared-cache
    behavior); the removed `ProjectPath` gate does not multiply queries because the freshness check,
    not the gate, is what suppresses them.
  - Steady-state fuscript rate is bounded at one invocation per TTL window process-wide.
  - Stale values can no longer be frozen into the `.drp` and resurrected on the next session.
  - `GYROFLOW_OFX_MISMATCH_TTL_MS=0` returns to sticky-cache behavior.
- **Known host quirk, now bypassed rather than fixed**: the single-key snapshot can carry cross-session
  residue (probe A1 read a 6-day-old `scaleToCrop / 1920×1920 / ucs=1` while the timeline was actually
  1920×1080, corroborated by the render log's `size=(1620,1080)`; neither the Resolve restart nor the
  project open cleared it). Because any dialog save refreshes it, periodic re-reading makes this
  self-correcting. No workaround is implemented for it.
- **Open knob** (implementer's default, reversible): re-query is *not* suppressed during playback or
  export. At a 10 s TTL the added load is one ~85 ms background query per 10 s; suppressing it would
  withhold updates exactly when the user is watching. Revisit if IPC latency during heavy playback
  proves material.
