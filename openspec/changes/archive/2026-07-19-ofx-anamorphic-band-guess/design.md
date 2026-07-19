# Design: ofx-anamorphic-band-guess

## Context

The OFX render path (`openfx/src/gyroflow.rs`, render section ~1106-1258) makes two aspect-ratio "guesses" via `GyroflowPluginBase::get_center_rect` (tolerance 0.1):

1. **Input content band** (`org_ratio`, :1118-1122 → `src_rect` :1199): guesses where the host placed real content inside the source buffer, assuming everything outside a centered band of the video's aspect is letterbox bars. Applied when `effective_host_input_sizing` is `Auto`/`Fit` or `DontDrawOutside` is on; `None` (full buffer) for `FillCrop`/`CenterCrop`/`Stretch`.
2. **Output aspect fit** (`output_aspect`, :1125 → `aspect_fit_output` :1220, out_rect :1233, proxy branch :1247): letterboxes the stabilized logical output into a mismatched host buffer. Gated on `!dont_draw_outside && !is_fusion_page && mode_is_fit`.

Both guesses use stretch-baked logical sizes: `params.size` has the lens `input_horizontal_stretch`/`input_vertical_stretch` applied (log evidence: post-mutation diff `["size","input_vertical_stretch"]` mutates (1920,1080)→(1920,1620)); `params.output_size` is the desqueezed logical output. The host buffer, however, contains **physical squeezed pixels** filling the entire extent (verified: DSC_3172 buffer stride 17280 = 1080 px × 16 B), and the host applies clip PAR only at display time.

For DSC_3172 (Nikon ZR, stored 1920×1080, rotation 270, v_stretch 1.5, buffer 1080×1920):
- input: `org_ratio` = 1620/1920 = 0.84375 vs buffer 0.5625 → diff 0.28 > 0.1 → phantom-letterbox verdict → band 1080×1280, top/bottom 320 real rows each discarded → the reported crop.
- output: `output_aspect` = 1620/1920 = 0.84375 → out_rect letterboxed to 1080×1280 → even with input fixed, picture would render small with bars and wrong display aspect after host PAR.

That source-native model is not universal. On 2026-07-19 a second clip produced a 972×1080 timeline-aspect source buffer after Resolve had already composited its 1.6 PAR. The physical ratio 0.5625 therefore generated a 608×1080 band and cropped both sides; the kill-switch's stretch-blind ratio selected the full 972×1080 buffer and restored the image. The old statement "the clip PAR is applied only at display time" is valid for the 2026-07-17 DSC_3172 path but false as a global Resolve invariant.

`StabilizationParams` has no raw `video_size` field — the physical size must be recovered by dividing out the lens stretch.

Prior art: the Premiere v2 fix (c536ce8, `adobe-rotated-anamorphic-full-frame`) established that the stabilization kernel's output-rect mapping is **per-axis linear** (`map_coord`), so passing a full-buffer rect with a mismatched-aspect `output_size` squeezes the frame anisotropically with **zero gyroflow-core changes**. This change reuses that fact; it does NOT port the Premiere mechanism (desktop-state gating, output-size override restore) — explicitly out of scope per user decision.

Note on the base spec: `openfx-render-sizing/spec.md` describes an `OutputFitMode` parameter that never existed in code (`git log -S OutputFitMode` is empty; the archived 2026-06-18 change's implementation went another way and was later superseded by `HostInputSizing`). The delta spec rewrites the affected requirement against the as-built `aspect_fit_output` mechanism and removes the phantom parameter requirement.

## Goals / Non-Goals

**Goals:**
- Vertical (and landscape) anamorphic sources render the full frame in Resolve Edit/Color page Fit mode — no content discarded as phantom letterbox.
- Output fills the physical buffer squeezed; host clip PAR performs display widening (accepting the known ~11% approximation when the user picks PAR 1.33 because Resolve lacks a 1.5 preset).
- Byte-equivalent behavior for non-anamorphic sources (stretch = 1.0).
- Env kill-switch restoring pre-change guesses.
- Correctly classify PAR-composited Resolve Edit/Color `Fit` buffers without changing source-native squeezed or evidence-poor paths.

**Non-Goals:**
- Porting the Premiere v2 pipeline (gating, output-size dance) — ruled out.
- Inferring host composition from global/project-only cache entries or adding persistent/UI state for the classification.
- gyroflow-core changes, new UI parameters, Adobe/frei0r changes, RoD/getClipPreferences changes.

## Decisions

### D1: Recover physical size by dividing `params.size` by lens stretch (not a new core field)

`physical = (size.0 / h_stretch, size.1 / v_stretch)` where `h_stretch`/`v_stretch` come from `stab.lens.read().input_horizontal_stretch/input_vertical_stretch` with the house guard `if s <= 0.01 { 1.0 }` (pattern at `common/src/lib.rs:1153`). Alternative — adding `video_size` to `StabilizationParams` — rejected: requires a core change for a plugin-local need and this repo pins core to a git rev.

Stretch factors are defined on **storage axes**; `params.size` is storage-orientation, so the division happens before any orientation swap:

- Input band (`org_ratio`): `if input_rotated_90_270 { ph/pw } else { pw/ph }` — same branch structure as today, stretch-divided components. DSC_3172: (1920/1.0, 1620/1.5) = (1920, 1080) → rotated ratio 1080/1920 = 0.5625 = buffer ratio → full band.
- Output aspect: `params.output_size` is display-orientation (rotation already applied by the core), so the storage-axis stretch maps through the rotation: display-horizontal was stretched by `v_stretch` when rotated 90/270, by `h_stretch` otherwise (and vice versa for display-vertical). `out_pw = output_size.0 / (rot ? v : h)`, `out_ph = output_size.1 / (rot ? h : v)`, `output_aspect = out_pw/out_ph`. DSC_3172: (1620/1.5, 1920/1.0) → 0.5625 → full-buffer out_rect.

`input_rotated_90_270` (:1113) already exists and is reused for both mappings.

### D2: Output squash rides the kernel's existing per-axis rect mapping

With `output_aspect` physical, `get_center_rect(out_size, output_aspect)` returns the full buffer for a physical-aspect buffer; core treats full rect ≡ `None` and linearly maps `output_size` (1620×1920) onto the buffer (1080×1920) per axis → horizontal squash → host PAR widens on display. Confirmed viable by the Premiere v2 as-built ("let the kernel existing per-axis output-rect mapping squeeze the desqueezed frame", zero core changes). The proxy/render-scale branch (:1247) and `DontDrawOutside` derivation (:1224, from `src_rect`) consume the same variables and stay consistent automatically.

The old "SHALL NOT squash" clause in the base spec was written PAR-blind (assumes square-pixel display). Squashing in **buffer space** here is PAR pre-compensation, not visual distortion — the displayed image is *less* distorted.

### D3: Fix both guesses together, not input-only

Input-only would trade "cropped" for "complete but letterboxed-small with wrong display aspect" (out_rect 1080×1280 band, then host PAR ×1.33). Both sites read from the same two derived values, so the increment is one more division. The user-visible acceptance criterion is "full frame, correct-ish proportions", which requires both.

### D4: Kill-switch `GYROFLOW_OFX_ANAMORPHIC_BAND=0|off|false`

Mirrors `adobe_anamorphic_full_frame_enabled()` (`common/src/lib.rs:932-945`): `OnceLock<bool>`, read once, log once when disabled. Placed in `openfx/src/gyroflow.rs` (OFX-local, like the behavior it guards). When off, both guesses use the pre-change stretch-baked values. Non-anamorphic sources are unaffected either way (division by 1.0), so the switch only matters for anamorphic diagnosis.

### D5: Delta spec rewrites the stale requirement instead of patching around it

`MODIFIED` on "Output buffer aspect-ratio fit" with full as-built content (aspect_fit_output + HostInputSizing gating + stretch-aware guesses), `REMOVED` on the phantom "OpenFX-only OutputFitMode parameter definition and default". Alternative — ADDED-only delta leaving the stale text — rejected: would leave the archived spec asserting a parameter and mode set that never existed, contradicting the modified behavior text.

### D6 (superseded): Select the aspect space from complete per-clip evidence

The render path has three explicit spaces:

- `LegacyLogical`: the pre-`6b8cb14` ratios. It is reachable only when `GYROFLOW_OFX_ANAMORPHIC_BAND` disables the feature.
- `Physical`: the `6b8cb14` stretch-divided ratios. It remains the normal path and every conservative fallback.
- `HostParComposited`: `physical org/output aspect × parsed Resolve clip PAR`.

`HostParComposited` is selected only when every gate passes: feature enabled; Edit/Color; effective `Fit`; `DontDrawOutside=false`; raw lens stretch differs from 1; per-clip fuscript source dimensions, numeric PAR, and timeline dimensions are valid; the fuscript source-native display aspect agrees with the core physical input aspect within 1%; the actual source-buffer aspect agrees with the timeline aspect within 1%; the timeline differs from the source-native physical aspect by more than the existing absolute 0.1 `get_center_rect` tolerance; and the `physical × PAR` candidate agrees with the timeline aspect within 1%. Any failed gate returns `Physical`.

The selector consumes the live instance's `CurrentFileInfo`. Global cache placeholders intentionally keep `width/height/PAR` empty, so they cannot accidentally classify a different clip. It is a pure calculation: it does not mutate cache entries, parameters, lens state, or stabilization state. One anamorphic debug line records the selected space and evidence.

This decision failed the `.drp` restore acceptance path: CreateInstance intentionally restores only mismatch mode and skips fuscript, leaving the clip-specific fields empty. The OFX-native replacement hypothesis also failed live probing: both clip and Image `kOfxImagePropPixelAspectRatio` values were `1.0`, because Resolve exposes the already-composited effect buffer as square-pixel rather than preserving the Media Pool clip PAR.

### D7 (superseded): Classify the actual buffer against `.gyroflow` logical and physical spaces

Per the user's 2026-07-19 host contract, applying the anamorphic PAR corresponding to the loaded `.gyroflow` raw lens stretch is the user's responsibility. The loaded stab already provides both expected spaces on every restore:

- `Physical`: raw stretch divided back out, representing a source-native squeezed buffer.
- `HostParComposited`: the stretch-baked logical aspect, representing a buffer after the user-applied host PAR.

The selector first keeps the existing global gates: feature enabled; Resolve host name `com.blackmagicdesign.resolve`; Edit/Color `Fit`; `DontDrawOutside=false`; anamorphic raw stretch. It then compares the actual source-buffer aspect with the logical input aspect. A 1% match plus an absolute logical-vs-physical separation greater than 0.1 selects `HostParComposited` and reuses the logical input/output pair. Matching the physical input aspect or any ambiguity returns `Physical`. The kill-switch still returns `LegacyLogical`.

This decision does not query fuscript on restore, add hidden fields, change the global cache, inspect pixels, or use OFX PAR. The OFX PAR probe remains diagnostic because its observed `1.0` explains the host boundary but cannot identify the original clip setting. Vegas and other hosts are explicitly excluded so removing fuscript evidence cannot broaden behavior beyond Resolve.

The final deployed D7 probe did not select this branch. Resolve reported host name `DaVinciResolve` rather than `com.blackmagicdesign.resolve`, and the main source image RoD was the full 1920×1080 timeline buffer. The logical 0.9 content occupies a centered 972×1080 band inside that buffer, so the buffer aspect itself is neither the logical 0.9 nor the physical 0.5625. Requiring a buffer/logical match therefore cannot model Resolve's actual compositing layout.

### D8: Physical match wins; otherwise main Resolve Fit defaults to logical

The user's host contract is the deciding evidence: the matching anamorphic PAR is expected to have been applied in Resolve. On a main anamorphic Resolve Edit/Color `Fit` render, the selector therefore uses the following precedence:

1. Kill-switch disabled behavior -> `LegacyLogical`.
2. Non-Resolve, non-Fit, Fusion, `DontDrawOutside`, non-anamorphic, preview/subscale, degenerate dimensions, or logical/physical separation <= 0.1 -> `Physical`.
3. Actual source-buffer aspect matches the physical input aspect within 1% -> `Physical` (source-native squeezed buffer).
4. Otherwise -> `HostParComposited`, reusing the logical input/output pair.

The Resolve host gate accepts the observed OFX host name `DaVinciResolve`; the canonical `com.blackmagicdesign.resolve` spelling is retained as a compatibility alias. Preview/subscale is passed from the existing render-scale and small-buffer detection so inspector thumbnails do not inherit the new default. For the target main render, `get_center_rect(1920, 1080, 0.9)` yields the correct 972×1080 logical content band; the physical 0.5625 path incorrectly yields 608×1080.

This remains a pure selection and preserves D7's no-fuscript/no-persistence/no-cache-mutation constraints. A user who does not apply the matching host PAR is outside the approved contract; the plugin deliberately does not inspect pixels to second-guess that host setup.

## Risks / Trade-offs

- **[User does not apply the matching anamorphic PAR in Resolve]** → a main Resolve Fit buffer can represent a different host transform than the loaded lens expects. This is outside the plugin contract by user decision; only source-native physical matches and failed gates fall back to `Physical`.
- **[Display aspect still ~11% off with PAR 1.33]** → inherent to Resolve's preset list (no 1.5). Communicated in proposal; not fixable in code; users with 2.0x anamorphic get an exact preset and are unaffected.
- **[`output_size` overridden by user (OutputWidth/Height) while anamorphic]** → the PAR pre-compensation divide still applies to whatever logical output the user chose; the host PAR stretch is a property of the clip, not of our output size, so dividing remains correct. Edge verified in reasoning only — covered by a manual spot-check during verification.
- **[Float ratio near the 0.1 `get_center_rect` tolerance]** → for exact physical matches the diff is ~0, far from the threshold; sources whose physical aspect genuinely differs from the buffer (true host letterbox of a non-anamorphic clip) keep working as today since stretch=1 leaves ratios untouched.

## Migration Plan

Single-commit code change + delta spec. Deploy via the normal `just ofx release` (elevated copy). Rollback: env kill-switch for diagnosis; `git revert` for permanent rollback. No persisted-state or project-format impact (no new params, nothing serialized).

## Open Questions

(none — direction and scope decided by user 2026-07-17; kernel per-axis mapping confirmed by c536ce8 as-built)

## As-built appendix (2026-07-17, post-verification)

The change grew three additional layers during live verification, plus audit fixes — tasks.md sections 1b-1e are the authoritative log. Summary of deviations from the original design:

- **D1 correction**: the stretch must be read from `lens.input_*_stretch_raw()` (raw mirrors), NOT the live fields — OFX auto-DisableStretch (`adjust_size=true`) bakes the stretch into `params.size` and resets the live fields to 1.0. Input divisor = raw/live ("baked"), output divisor = raw (PAR compensation). First deployed build read live fields and was a silent no-op.
- **fuscript timeline resolution** (1b): `ucs='1'` → `tl:GetSetting` for `timelineResolutionWidth/Height`, empty → project fallback. Discovered because the test timeline is a custom portrait 1080×1920 inside a 1920×1080 project.
- **FillCrop anamorphic crop model** (1c): `compute_fillcrop_geometry_desqueezed` — crop in physical space, scale back per display axis, identity(±1px) → `None` → no mutation at all. Fixes the phantom 1.5× crop AND (incidentally) HEAD's calib_dimension pollution on identity writes.
- **InputRotation baseline restore** (1d): `restore_host_input_sizing_baseline()` before the rotation override in BOTH entry points (InstanceChanged handler + render-path precheck via `input_rotation_target_rotation`).
- **Audit fixes** (1e): compare-before-write restores + recompute on real change; degenerate-zero gate semantics preserved; `*ManuallyEdited` log noise silenced.
- **Host environment findings** (recorded for future sessions): Resolve 21.0.0.47 timeline-settings-dialog edits do not refresh the scripting-API snapshot store (GetSetting stale until project load / API SetSetting / toggling "Use Project Settings" off-on — the user-level refresh recipe); Resolve Edit-page `Fit` can expose either source-native squeezed or PAR-composited timeline buffers depending on clip/timeline state, so neither model is a global invariant; Resolve's FillCrop input-sizing decision uses physical pixel dims (confirmed live via R5MK2).
- **2026-07-19 extension**: kill-switch live A/B confirmed the PAR-composited failure; D6 supersedes the original unconditional physical-band assumption only for the evidence-complete `Fit` case. FillCrop/CenterCrop/Stretch, rotation restore, cache semantics, and all earlier audit boundaries remain unchanged.
- **2026-07-19 restore correction**: D7 supersedes D6 after the restored-instance probe returned `evidence=None`, `clip_par=Some(1.0)`, and `image_par=Some(1.0)`. The selector now compares the actual buffer with the `.gyroflow` logical/physical spaces under the user-applied-PAR contract.
- **2026-07-19 final host-layout correction**: D8 supersedes D7 after the deployed log reported `host=Some("DaVinciResolve")` and main `buffer=(1920, 1080)`. The logical 0.9 content is a 972×1080 band inside that timeline buffer, not the buffer's own aspect.
- **Known boundaries left for future changes** (all pre-existing, HEAD-identical): rotated clip + genuine FillCrop writes display-orientation size into the storage-orientation field + camera-matrix shift axis/pixel-space mismatch (masked for symmetric lenses); `calib_dimension←crop` breaks the size/calib focal rescale for calib≠size lens profiles (dormant for NiYien lens-group profiles); the apply early-out keys on mode only, not timeline geometry; custom output_size + FillCrop identity divergence (niche).
