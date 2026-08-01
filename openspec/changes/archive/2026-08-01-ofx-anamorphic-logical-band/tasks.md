# Tasks: ofx-anamorphic-logical-band

Backfilled: the work was done ad hoc from a user report and this record was written after the fact.

## 1. Diagnosis

- [x] 1.1 Compare the same clip's `max_zoom_entry` between the reporter's and the maintainer's
      desktop app runs; confirm bit-identical `min_fov` and rule out the whole Gyroflow side
- [x] 1.2 Reproduce in Resolve on the maintainer's machine (Windows/CUDA/21.0.0) and confirm the
      log shape matches the reporter's (macOS/Metal/20.2.2)
- [x] 1.3 A/B with `GYROFLOW_OFX_ANAMORPHIC_BAND=0`; record `in_rect` in both states
- [x] 1.4 Establish the workflow (clip desqueezed via Resolve Clip Attributes PAR = 1.5) and confirm
      `CurrentFileInfo.pixel_aspect_ratio` is the clip-level field carrying it
- [x] 1.5 Rule out clip PAR and `timeline_w/h` as replacement signals (playhead clip identity,
      refresh publication, restore emptiness, single-sample risk)

## 2. Implementation

- [x] 2.1 Replace the buffer-aspect comparison in `select_anamorphic_band_aspects` with an
      unconditional `HostParComposited(logical_aspects)`
- [x] 2.2 Keep all protective gates (host, anamorphic, `mode_is_fit`, Fusion, `DontDrawOutside`,
      preview/subscale, degenerate buffer) with their previous meaning
- [x] 2.3 Delete the now-unused `aspects_match_within_one_percent`
- [x] 2.4 Apply the `≤ 0.01 means unset` stretch convention inside the selector
- [x] 2.5 Correct the call-site comment at `gyroflow.rs:1799`, which described the removed precedence

## 3. Tests

- [x] 3.1 `resolve_anamorphic_main_fit_render_uses_the_logical_band` — 2 stretches x 2 host strings
      x 3 buffer sizes, asserting the result does not depend on the buffer
- [x] 3.2 `protective_gates_stay_physical` — 11 cases across every gate plus the degenerate buffer
      and the uninitialised stretch form
- [x] 3.3 `undesqueezed_anamorphic_clip_also_gets_the_logical_band_by_design` — pins the accepted
      regression
- [x] 3.4 `kill_switch_selects_legacy_logical` — `GYROFLOW_OFX_ANAMORPHIC_BAND=0` still reverts
- [x] 3.5 `cargo test -p gyroflow-ofx --lib` → 53 passed, 0 failed

## 4. Specs

- [x] 4.1 `openfx-render-sizing`: rewrite the `HostParComposited` gate list and host contract; add
      the un-desqueezed limit; replace the source-native scenario with a protective-gate scenario
      plus a reverse scenario pinning "buffer matching physical still selects logical"
- [x] 4.2 `openfx-host-input-sizing`: drop the buffer-aspect condition from the restore scenarios

## 5. Verification and release

- [x] 5.1 Deploy to `C:\Program Files\Common Files\OFX\Plugins\...`, keeping a `.bak` of the
      previous build
- [x] 5.2 Live-verify landscape and rotated-vertical with no environment variables set
- [x] 5.3 Commit (`2e129a2`)
- [ ] 5.4 Publish a plugin build so end users receive the fix — **not done**; users are still on
      2.1.2.37 (2026-07-26 build)
