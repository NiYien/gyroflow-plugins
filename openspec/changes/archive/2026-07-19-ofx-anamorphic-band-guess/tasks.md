# Tasks: ofx-anamorphic-band-guess

## 1. Implementation (openfx/src/gyroflow.rs)

- [x] 1.1 Add kill-switch helper `ofx_anamorphic_band_enabled()` (OnceLock, `GYROFLOW_OFX_ANAMORPHIC_BAND=0|off|false` disables, log once when disabled) mirroring `adobe_anamorphic_full_frame_enabled()`
- [x] 1.2 Read lens stretch factors in the render section (`stab.lens.read().input_horizontal_stretch/input_vertical_stretch`, ≤0.01 → 1.0 guard) and derive physical input size `(size.0/h, size.1/v)` — gated on the kill-switch (disabled → factors forced to 1.0, byte-equivalent old path). Lens lock scoped before the params lock (no overlap)
- [x] 1.3 Switch `org_ratio` (:1118) to the physical components (same rotated/non-rotated branch structure)
- [x] 1.4 Switch `output_aspect` (:1125) to the physical output aspect (display-horizontal ÷ `v_stretch` when rotated 90/270 else ÷ `h_stretch`; conversely for display-vertical)
- [x] 1.5 Add a one-line debug log with both derived ratios + stretch factors when stretch ≠ 1.0 (diagnosis aid, silent for normal sources)

## 1b. fuscript timeline-resolution fix (added during verification, user-directed)

- [x] 1b.1 `fuscript.rs`: read `timelineResolutionWidth/Height` from `tl:GetSetting` when `ucs='1'`, empty → project-level fallback; document the Resolve 21.0 stale-snapshot caveat (dialog edits don't refresh the GetSetting store; toggle "Use Project Settings" off/on or restart to resync)
- [x] 1b.2 User verified `timeline_w: 1080, timeline_h: 1920` in log; FillCrop then exposed 1c (below)

## 1c. FillCrop anamorphic-aware crop model (found when the crop mode first ran, user-verified regression on DSC_3172)

- [x] 1c.1 Root cause: `apply_host_input_sizing_if_needed` fed the desqueezed `params.size` (1920×1620) to `compute_crop_geometry` → phantom 1.5× horizontal crop on a matching-aspect timeline → params.size/output_size overwritten to (1080,1920) → 720-wide render with side pillarbox bars (log: `output_aspect=0.3750`)
- [x] 1c.2 Add `compute_fillcrop_geometry_desqueezed` (crop in physical space, scale back per display axis, `None` = identity → skip mutation entirely) + `lens_baked_stretch` helper; FillCrop branch rewired; 4 unit tests (35/35 pass)
- [x] 1c.3 User verified 2026-07-17: DSC_3172 side bars gone; R5MK2 renders full-frame with correct crop model (also confirms the Resolve physical-pixel FillCrop premise, audit F12)

## 1d. InputRotation × FillCrop baseline poisoning (pre-existing, first exposed when crop mode ran; C0016 user-verified)

- [x] 1d.1 Root cause: FillCrop mutation ran at rotation=0 (genuine landscape crop 1215×2160), then the InputRotation param-change handler applied the rotation override on the mutated state and cleared the pre-mode snapshots — adopting the cropped state as the new baseline (log: run 040 `size=(1215, 2160)` after runs 038/039 healthy `(3840, 2160)`)
- [x] 1d.2 Add `restore_host_input_sizing_baseline()` and call it before `apply_openfx_input_rotation_override_to_managers` in the InputRotation handler — rotation now operates on the clean baseline; next apply re-snapshots clean+rotated state and the identity guard (1c) leaves it untouched
- [x] 1d.3 User verified 2026-07-17: C0016 with InputRotation 90° left stabilizes correctly on the portrait timeline
- [x] 1d.4 Known boundary (design.md): only the last-applied stab's snapshots exist; a sibling cached manager mutated earlier in the same lifetime cannot be restored until its next rebuild — acceptable, rebuilds are frequent; audit fix 1e.2 makes restore converge shared stabs instead of splitting them

## 1e. Four-agent adversarial audit (user-requested) + fixes

- [x] 1e.1 Audit verdicts: all three target-scenario claims arithmetically CONFIRMED by independent recomputation; non-anamorphic paths proven bit-equivalent; "divide by raw again post-mutation" proven an invariant (and unconsumed in FillCrop mode); calib==size invariant holds for NiYien lens-group profiles; no double-transpose; fuscript change safe
- [x] 1e.2 Fix: restore blocks (apply + helper) now compare-before-write and trigger recompute/invalidations when values actually changed — closes the Stretch→FillCrop-identity / FillCrop→Fit stale-ComputeParams windows (the Fit one was pre-existing) and makes no-op restores truly silent
- [x] 1e.3 Fix: render-path rotation override now prechecks with `input_rotation_target_rotation` and restores the baseline first — closes the second C0016-style door; no-op overrides never trigger a restore (would silently drop an active crop)
- [x] 1e.4 Fix: `physical_band_aspects` degenerate zero dims keep stretch-blind gate semantics (numerators unclamped → ratio 0.0 → aspect-fit gate closed, as before the change)
- [x] 1e.5 Fix: silence pre-existing `Unknown param name: *ManuallyEdited` ERROR noise
- [x] 1e.6 35/35 tests pass; release build clean
- [x] 1e.7 Documented known boundaries (pre-existing, NOT fixed, need their own change if wanted): rotated clip + genuine FillCrop writes display-orientation size + wrong camera-matrix shift axis (HEAD identical); calib_dimension←crop breaks focal rescale for calib≠size lens profiles (dormant for lens-group profiles); early-out not keyed on timeline-geometry changes; custom output_size + FillCrop identity divergence (niche, needs live look); shared-InstanceId stab bookkeeping is per-instance (pre-existing danger zone, restore now converges it instead of splitting it)

## 2. Verification

- [x] 2.1 `cargo build --release` clean; deploy via elevated copy to the OFX bundle (2026-07-17 08:20 build, 25,206,272 B)
- [x] 2.2 User verified 2026-07-17 (Fit phase): DSC_3172 full frame, no top/bottom crop; later crop-mode phase re-verified after 1c/1d
- [x] 2.3 Regression: audit proved bit-level equivalence for non-anamorphic paths (35/35 tests incl. 8 new); C0016/C6505/Sony clips on the live timeline behave correctly
- [x] 2.4 Kill-switch live-tested 2026-07-19: default physical band produced `org_ratio=0.5625` and a 608x1080 centered source rect; `GYROFLOW_OFX_ANAMORPHIC_BAND=0` restored the full 972x1080 rect and the user confirmed the image returned to normal

## 1f. PAR-composited Fit buffer extension (2026-07-19)

- [x] 1f.1 Diagnose and isolate the second Resolve buffer model: for the affected vertical-to-wide clip, Edit/Color `Fit` supplies a timeline-aspect source buffer with clip PAR already composited; the physical-band rule applies the squeeze a second time and crops left/right
- [x] 1f.2 Approve a conservative three-space selector: kill-switch only -> `LegacyLogical`; default/evidence-poor -> `Physical`; complete per-clip Resolve evidence -> `HostParComposited` (`physical aspect * actual clip PAR`)
- [x] 1f.3 Add failing pure unit tests for the PAR-composited selection and conservative non-regression gates; RED confirmed with 18 missing-symbol errors before implementation
- [x] 1f.4 Implement the evidence-gated selector using only per-clip fuscript source size/PAR, timeline size, actual source-buffer size, effective Fit/Edit-Color mode, `DontDrawOutside=false`, and anamorphic stretch
- [x] 1f.5 Add one diagnostic line for anamorphic renders recording the selected aspect space and evidence without changing global cache contents or stabilization state

## 2b. Extension verification

- [x] 2b.1 Run the focused OpenFX unit tests and confirm the new regression test fails before implementation, then passes after the minimal fix
- [x] 2b.2 Run the complete OpenFX test suite (40/40 pass) and `cargo check -p gyroflow-ofx` (exit 0)
- [x] 2b.3 Build the release OFX bundle, deploy it, and verify the original clip without the kill-switch
- [x] 2b.4 Re-check a source-native squeezed anamorphic Fit clip plus non-anamorphic/Fusion/FillCrop guard tests for unchanged behavior

## 1g. Restore-safe `.gyroflow` aspect-space selector (2026-07-19)

- [x] 1g.1 Live probe the restored target instance: `evidence=None`, `clip_par=Some(1.0)`, and `image_par=Some(1.0)`; OFX PAR describes Resolve's square-pixel effect buffer and cannot replace Media Pool PAR
- [x] 1g.2 User decision: applying the anamorphic PAR corresponding to the loaded `.gyroflow` raw stretch is the user's responsibility; restore classification may use the loaded logical/physical spaces and actual source-buffer aspect
- [x] 1g.3 Update proposal, design, delta specs, tasks, and implementation plan to supersede fuscript-only D6 with restore-safe D7
- [x] 1g.4 Add a failing restored-instance test: no fuscript evidence and OFX PAR 1.0, but a Resolve Fit source buffer matching logical `0.9` must select `HostParComposited`; RED confirmed against the D6 signature, plus an overlap RED for buffer matching both tolerance windows
- [x] 1g.5 Implement the minimum Resolve-only buffer-aspect selector; source-native/ambiguous/other-host paths remain `Physical`; independent review found no remaining issues
- [x] 1g.6 Run focused and complete OpenFX tests (`40/40`), `cargo check -p gyroflow-ofx`, and release build

## 2c. Restore-safe live verification

- [x] 2c.1 Deploy the release DLL and verify `New Project 1 / CDC_1155.MOV` selects `HostParComposited` without the kill-switch and has no side bars
- [x] 2c.2 Re-check source-native anamorphic Fit plus non-anamorphic, Fusion, FillCrop/CenterCrop/Stretch, `DontDrawOutside`, Vegas/other-host, thumbnail/proxy, and kill-switch behavior

Final verification evidence (2026-07-19):

- Deployed DLL and fresh release output are byte-identical: SHA-256 `4492AF63B886E97C96AC79BD30C0965C5A4B653932790DD75E3806292D984BFE`, 25,503,744 bytes.
- `New Project 1 / CDC_1155.MOV` main render logged `space=HostParComposited`, `buffer=(1920, 1080)`, `selected=(0.9000, 0.9000)`, producing the confirmed 972x1080 framing without side bars; preview/subscale probes stayed `Physical`.
- After a normal Resolve restart, `vertical_test / DSC_3172.MP4` reported `mismatch_mode="scaleToCrop"`; its 1920x1920 main render used `in_rect=None` and `out_rect=None`, and the user confirmed the black bars were gone. The same session exercised non-anamorphic square-timeline clips without changing their existing paths.
- Fresh verification: OpenFX tests 41/41, `cargo check -p gyroflow-ofx`, release build, and `git diff --check` all passed. Selector/geometry tests cover source-native physical matching, non-anamorphic, Fusion, FillCrop/CenterCrop/Stretch, `DontDrawOutside`, Vegas/other-host, preview/subscale, and kill-switch gates; prior live checks cover source-native Fit and kill-switch behavior.

## 1h. Final Resolve host-layout correction (2026-07-19)

- [x] 1h.1 Deploy D7 and probe the target main render: Resolve reports `host=Some("DaVinciResolve")`, `buffer=(1920, 1080)`, logical `0.9`, physical `0.5625`; D7 remains `Physical`
- [x] 1h.2 Reconcile the live layout with the user contract: the matching host PAR is assumed applied, so a main Resolve Fit buffer that does not match physical defaults to the logical content band; physical match still protects source-native input
- [x] 1h.3 Update proposal, design, delta specs, tasks, and implementation plan with D8; preview/subscale remains Physical
- [x] 1h.4 Add a failing test for observed host `DaVinciResolve` and 1920×1080 main buffer selecting logical `0.9`; add source-native and preview regression gates; RED confirmed with `E0061` before the selector accepted preview/subscale state
- [x] 1h.5 Implement the minimum D8 precedence and rerun focused/full OpenFX verification (focused 6/6; physical-band 4/4; full 41/41)

## 3. Wrap-up

- [x] 3.1 No CLAUDE.md in this repo; env var + as-built recorded in design.md appendix and delta specs
- [x] 3.2 Committed (no Co-Authored-By per user convention), push left to user
