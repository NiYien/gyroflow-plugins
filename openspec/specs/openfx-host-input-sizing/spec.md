# openfx-host-input-sizing

## Purpose

Defines how the OpenFX plugin handles input-side timeline-mismatch behavior for DaVinci Resolve. Reads Resolve's `timelineInputResMismatchBehavior` setting via fuscript and maps it onto an internal `HostInputSizing` enum (`Auto` / `Fit` / `FillCrop` / `CenterCrop` / `Stretch`) that drives lens-calibration and `stab.params.{size, output_size}` adjustments before rendering. The mode is also exposed via a hidden OpenFX choice param so paste round-trips serialize cleanly and future debugging can override the auto-detected value.
## Requirements
### Requirement: Detect host input sizing mode via fuscript

The OpenFX plugin SHALL query the DaVinci Resolve scripting API via `fuscript` to read the host's mismatched-resolution behavior setting (`timelineInputResMismatchBehavior`) and use it to drive input-side handling on every clip load and on every render where the cached value is stale.

The plugin SHALL honor timeline-level override: when the current timeline's `useCustomSettings` is `"1"`, the plugin SHALL read the setting from the timeline (`tl:GetSetting('timelineInputResMismatchBehavior')`); otherwise it SHALL read from the project (`proj:GetSetting('timelineInputResMismatchBehavior')`).

The plugin SHALL map the four observed string values to its internal `HostInputSizing` enum:

| fuscript value | `HostInputSizing` |
|---|---|
| `scaleToFit` | `Fit` |
| `scaleToCrop` | `FillCrop` |
| `centerCrop` | `CenterCrop` |
| `stretch` | `Stretch` |

#### Scenario: fuscript reports scaleToCrop on Resolve Studio

- **WHEN** the host is Resolve Studio with Local scripting enabled, a non-compound clip is on the current timeline, and `Project Settings → Image Scaling → Mismatched Resolution Files` is `Scale full frame with crop`
- **THEN** the plugin's effective `HostInputSizing` is `FillCrop` and the render path takes the FillCrop branch

#### Scenario: Timeline override on top of project default

- **WHEN** the host project setting is `scaleToFit` but the current timeline has `useCustomSettings='1'` with `timelineInputResMismatchBehavior='scaleToCrop'`
- **THEN** the plugin's effective `HostInputSizing` is `FillCrop` (timeline override wins)

### Requirement: Timeline resolution honors the timeline custom-settings override

When the current timeline reports `useCustomSettings == '1'`, the fuscript query SHALL read `timelineResolutionWidth` / `timelineResolutionHeight` from the timeline (`tl:GetSetting`), falling back to the project-level read (`proj:GetSetting`) when the timeline-level values are empty (older Resolve versions / missing keys). When `useCustomSettings` is not `'1'`, the project-level read SHALL be used, unchanged.

The resulting `timeline_w` / `timeline_h` feed `stab.params.size` in Stretch mode and the FillCrop/CenterCrop crop-geometry computation; on a custom-resolution timeline (e.g. a portrait 1080x1920 timeline inside a 1920x1080 project) the project-level read returns the wrong dimensions and mis-models the crop axis.

#### Scenario: Portrait custom timeline in a landscape project

- **WHEN** the current timeline has `useCustomSettings='1'` with resolution 1080x1920 while the project's timeline resolution setting is 1920x1080
- **THEN** the parsed `CurrentFileInfo.timeline_w/timeline_h` is 1080x1920 (the timeline-level values), and the FillCrop/CenterCrop crop geometry and Stretch-mode `stab.params.size` use the actual timeline dimensions

#### Scenario: Timeline-level resolution keys unavailable

- **WHEN** `useCustomSettings='1'` but `tl:GetSetting('timelineResolutionWidth')` returns an empty value (older Resolve)
- **THEN** the query falls back to the project-level `timelineResolutionWidth/Height` and behavior matches the pre-change read

#### Scenario: Default-settings timeline unchanged

- **WHEN** the current timeline reports `useCustomSettings` != `'1'`
- **THEN** the resolution is read from the project level exactly as before the change

### Requirement: UI dropdown fallback when fuscript fails

The OpenFX plugin SHALL expose an OpenFX-specific choice parameter `HostInputSizing` with choices `Auto` (default), `Fit`, `FillCrop`, `CenterCrop`, `Stretch`. When the UI value is `Auto` the plugin SHALL use the fuscript-detected value; when it is one of the explicit modes the plugin SHALL override fuscript and force that mode.

When fuscript is unavailable (Resolve Free edition, "External scripting using" not set to `Local`, compound clip / nested timeline where `GetCurrentVideoItem()` is empty, non-Resolve OFX host), the plugin SHALL treat the fuscript result as `Fit` and rely entirely on the UI dropdown for mode selection.

`HostInputSizing` SHALL be defined as an OpenFX-only parameter and SHALL NOT be added to `gyroflow-plugins/common/src/lib.rs::Params` or `GyroflowPluginBase::get_param_definitions()`. Adobe / Premiere / frei0r SHALL NOT define this parameter.

`HostInputSizing` SHALL NOT be added to the paste-preservable merge set (`PASTEABLE_PARAMS`).

The `HostInputSizing` dropdown SHALL be hidden from the visible plugin UI (registered with `set_secret(true)` and excluded from page children) so the parameter survives paste round-trips and remains available as a debugging override, while the Auto path covers the visible user need.

#### Scenario: User overrides Auto to force FillCrop on Resolve Free

- **WHEN** running on Resolve Free, fuscript returns failure, and the user sets the OpenFX `HostInputSizing` dropdown to `FillCrop`
- **THEN** the render path takes the FillCrop branch regardless of fuscript state

#### Scenario: Auto on a compound clip falls back to Fit

- **WHEN** the active clip is a Resolve compound clip (fuscript can't resolve `GetCurrentVideoItem`), and `HostInputSizing` is `Auto`
- **THEN** the effective mode resolves to `Fit` and existing letterbox behavior applies; the plugin SHOULD surface a status hint that the user can manually pick a mode

### Requirement: scaleToFit (Fit) render path is identity-preserving

When `HostInputSizing == Fit`, the OpenFX plugin's input-side handling SHALL be byte-for-byte identical to pre-change behavior. Specifically: `src_rect` SHALL be computed by the existing `GyroflowPluginBase::get_center_rect(src_size.0, src_size.1, org_ratio)`, the lens calibration data SHALL NOT be mutated by host-input-sizing logic, `stab.params.size` / `stab.params.output_size` SHALL NOT be overwritten by host-input-sizing logic, and no extra `init_size` / `recompute_blocking` SHALL be triggered relative to current behavior.

#### Scenario: scaleToFit regression baseline

- **WHEN** Resolve is set to `Scale entire image to fit`, source 3840×1920 with `unit_pixel_focal_length=0.5`, timeline 1920×1920
- **THEN** lens `camera_matrix`, `stab.params.size`, `stab.params.output_size` retain their loaded values from the `.gyroflow` project / lens profile and the rendered output is identical to the pre-change plugin

### Requirement: scaleToCrop (FillCrop) input-side lens transform

When `HostInputSizing == FillCrop`, the plugin SHALL treat the host-supplied input buffer as a 1:1 center crop of the source frame, compute the source-pixel crop rectangle, mutate the cached lens calibration's principal point and dimensions accordingly, and reset `stab.params.{size, output_size}` to the source-pixel crop dimensions. `fx`, `fy`, distortion coefficients (`k1..k4`, `p1`, `p2`), and `unit_pixel_focal_length` SHALL remain unchanged.

For anamorphic sources whose logical size has lens stretch baked in, the physical-space recovery and per-display-axis scaling defined by "FillCrop/CenterCrop crop model uses physical source dimensions" SHALL be applied before this transform.

The crop geometry is derived from the loaded `.gyroflow`/lens-profile source size `(sw, sh)` and the host buffer aspect `(tw/th)` reported by the OpenFX `source_clip` ROD:

```
if sw/sh > tw/th:                  # horizontal crop
    crop_w = round(sh * tw/th)
    crop_h = sh
    crop_x = (sw - crop_w) / 2
    crop_y = 0
else:                              # vertical crop
    crop_w = sw
    crop_h = round(sw * th/tw)
    crop_x = 0
    crop_y = (sh - crop_h) / 2
```

The plugin SHALL apply the following transform after loading the StabilizationManager and before the first `recompute_blocking`:

```
lens.camera_matrix[0][2] -= crop_x         # cx offset
lens.camera_matrix[1][2] -= crop_y         # cy offset
lens.calib_dimension = (crop_w, crop_h)
# camera_matrix[0][0] (fx), camera_matrix[1][1] (fy) unchanged
# distortion coefficients unchanged

stab.params.size        = (crop_w, crop_h)
stab.params.output_size = (crop_w, crop_h)
stab.init_size()
stab.recompute_blocking()
```

The plugin SHALL set `BufferDescription.input.rect = None` (the entire host buffer is valid pixels, no letterbox band to clip) and let the core's existing `HAS_OUTPUT_RECT + map_coord` machinery handle the `stab.output_size != buffers.output.size` mapping (equivalent to virtual downscale during write-back).

#### Scenario: Horizontal crop (source wider than timeline)

- **WHEN** source 3840×1920 (`unit_pixel_focal_length=0.5`, hence `fx=fy=1920`, `cx=1920`, `cy=960`), timeline 1920×1920, `HostInputSizing=FillCrop`
- **THEN** crop geometry resolves to `crop_w=1920, crop_h=1920, crop_x=960, crop_y=0`, lens transforms to `fx=fy=1920` (unchanged), `cx=960, cy=960`, `calib_dimension=(1920,1920)`; `stab.params.size=(1920,1920)`, `stab.params.output_size=(1920,1920)`

#### Scenario: Vertical crop (source taller than timeline)

- **WHEN** source 1080×1920 (vertical clip with `fx=fy=540`, `cx=540`, `cy=960`), timeline 1920×1920, `HostInputSizing=FillCrop`
- **THEN** crop resolves to `crop_w=1080, crop_h=1080, crop_x=0, crop_y=420`; lens transforms to `cx=540` (unchanged), `cy=540`, `calib_dimension=(1080,1080)`; `stab.params.size=(1080,1080)`, `stab.params.output_size=(1080,1080)`

#### Scenario: Source already matches timeline aspect — no-op transform

- **WHEN** source 1920×1080, timeline 1920×1080, `HostInputSizing=FillCrop`
- **THEN** `crop_w=1920, crop_h=1080, crop_x=0, crop_y=0`, lens & stab params equal their pre-crop values and the render is identical to the `Fit` path on the same input

### Requirement: FillCrop/CenterCrop crop model uses physical source dimensions

The FillCrop/CenterCrop transform SHALL model the host's crop in physical (squeezed) pixel space: `stab.params.size` divided per storage axis by the lens stretch baked into it (raw lambda divided by live, values <= 0.01 guarded, `GYROFLOW_OFX_ANAMORPHIC_BAND=0` -> 1.0), because Resolve's input-sizing decision operates on the clip's storage pixels (clip PAR is not applied when compositing into the effect buffer). The resulting physical crop SHALL be scaled back to the desqueezed space per display axis (mapped through the 90/270 rotation) before mutating `stab.params.size` / `output_size` / `calib_dimension` / `camera_matrix`.

When the physical crop is an identity (zero offsets, crop within +/-1 px of the full physical display size), the transform SHALL NOT mutate the stab state at all; the untouched (Fit-equivalent) state renders that geometry correctly.

#### Scenario: Vertical anamorphic clip matching the timeline - no phantom crop

- **WHEN** DSC_3172 (stored 1920x1080, rotation 270, v_stretch 1.5, `params.size` (1920,1620)) sits on a 1080x1920 timeline with effective mode FillCrop
- **THEN** the physical display size (1080x1920) matches the timeline aspect, the transform is skipped, and `params.size`/`output_size` keep their desqueezed values (previously the desqueezed 0.84 aspect fabricated a 1.5x horizontal crop -> 720-wide render with side pillarbox bars)

#### Scenario: Landscape anamorphic clip genuinely cropped by the host

- **WHEN** a 5760x2160-desqueezed (h_stretch 1.5) clip sits on a 1080x1920 timeline with effective mode FillCrop
- **THEN** the crop is computed on the physical 3840x2160 frame (centered 1215x2160) and scaled back to the desqueezed space as a centered 1823x2160 crop at x-offset 1968

#### Scenario: Non-anamorphic clips unchanged (except identity skip)

- **WHEN** the lens has no anamorphic stretch
- **THEN** genuine crops compute the same values as before the change; identity crops (source display aspect already matches the timeline) now skip the mutation instead of rewriting equal values

### Requirement: centerCrop mode handling

When `HostInputSizing == CenterCrop`, the plugin SHALL apply the same lens transform and stab-params reassignment as `FillCrop` (Resolve's `centerCrop` behaves as 1:1 source-pixel crop without resizing, identical at the buffer level to `scaleToCrop` whenever timeline ≤ source). The mode is kept as a distinct enum to permit future divergence (e.g. when timeline > source and `centerCrop` would leave per-side padding).

#### Scenario: centerCrop on a source larger than timeline

- **WHEN** source 3840×1920, timeline 1920×1920, `HostInputSizing=CenterCrop`
- **THEN** the transform applied is identical to the `FillCrop` case above

### Requirement: stretch mode handling

When `HostInputSizing == Stretch`, Resolve has non-uniformly scaled the source into the timeline buffer. The plugin SHALL set `stab.params.size = (timeline_w, timeline_h)` (taken from the OpenFX `source_clip` ROD width/height) and SHALL accept the resulting aspect distortion. Lens calibration SHALL NOT be mutated (the transform is non-uniform; a meaningful intrinsic update is out of scope for this MVP).

The plugin SHALL log a `warn` indicating that stretch mode is best-effort and recommending the user switch Resolve's mismatched-resolution setting to `scaleToFit` or `scaleToCrop` for accurate stabilization.

#### Scenario: Stretch mode on aspect-mismatched timeline

- **WHEN** source 3840×1920, timeline 1920×1920, `HostInputSizing=Stretch`
- **THEN** `stab.params.size = (1920, 1920)` (= timeline size from source_clip ROD); lens `camera_matrix` is unchanged; a warning is logged once per StabilizationManager instance

### Requirement: Override precedence

The `HostInputSizing` resolution SHALL be applied only when none of the higher-priority overrides are active. Priority (highest first):

1. `DontDrawOutside == true` — existing `DontDrawOutside` rect logic runs and `HostInputSizing` lens/params transforms SHALL NOT execute
2. `is_fusion_page == true` — Fusion page receives the clip at native resolution; `HostInputSizing` SHALL be forced to `Fit` regardless of fuscript / UI value
3. Vegas host (`com.vegascreativesoftware.vegas`) — existing `out_rect = None` bypass runs; `HostInputSizing` lens/params transforms SHALL NOT execute
4. Otherwise, the resolved `HostInputSizing` mode controls the input-side branch

#### Scenario: Fusion page forces Fit

- **WHEN** `HostInputSizing` UI is `FillCrop` and the plugin is rendering on the Fusion page (`is_fusion_page == true`)
- **THEN** the FillCrop lens transform SHALL NOT run; behavior matches the pre-change Fusion path

#### Scenario: DontDrawOutside wins over HostInputSizing

- **WHEN** `DontDrawOutside == true` and effective `HostInputSizing == FillCrop`
- **THEN** the `DontDrawOutside` output rect logic runs and the FillCrop lens transform SHALL NOT run

### Requirement: Apply input_rotation before crop offset

When the loaded `.gyroflow`/source has non-zero `video_rotation` (90°/180°/270°), the plugin SHALL compute the crop offset against the rotated source dimensions (i.e. swap `sw`/`sh` for 90°/270° before computing `crop_x`/`crop_y`). This matches Resolve's pipeline order (Clip Attributes Image Orientation rotation happens before timeline transform).

When the user changes `InputRotation` post-paste (overriding the loaded clip's native rotation), the plugin SHALL invalidate the cached pre-mode snapshots (`pre_mode_size`, `pre_mode_output_size`, `pre_mode_camera_matrix`, `pre_mode_calib_dimension`, `last_applied_stab`) in addition to clearing `applied_host_input_sizing`. Without this invalidation, the rotation override's in-place mutation of `stab.params.output_size` is reverted by the next apply's restore branch (which uses the stale pre-rotation snapshot), leaving `video_rotation` and `output_size` in an inconsistent state and producing visible picture offset in `Fit` mode (output rect is computed for one aspect while content is in the other).

#### Scenario: 90° rotated vertical source under FillCrop

- **WHEN** source 1920×1080 with `InputRotation=90°` (rotated source aspect 1080:1920 ≈ 0.5625), timeline 1920×1920 (1.0), `HostInputSizing=FillCrop`
- **THEN** rotated `(sw, sh) = (1080, 1920)`; vertical crop branch resolves to `crop_w=1080, crop_h=1080, crop_x=0, crop_y=420`; lens transform `cy -= 420`, `calib_dimension=(1080,1080)`, `stab.params.size=(1080,1080)`

#### Scenario: User changes InputRotation post-paste in Fit mode

- **WHEN** a horizontal source clip is pasted with `InputRotation=0` and `HostInputSizing=Fit` has already been applied (snapshots taken), then the user changes `InputRotation` to `90 left` via the OpenFX dropdown
- **THEN** the plugin clears `pre_mode_*` snapshots + `last_applied_stab` in addition to `applied_host_input_sizing`, so the next render's apply re-snapshots from the post-override state (with `output_size` already swapped to vertical), restore is a no-op, and the final render preserves `video_rotation=90` with `output_size` matching the rotated orientation

### Requirement: Defer apply on preview thumbnails and sub-scale renders

The plugin SHALL defer the `HostInputSizing` apply (which mutates `stab.params.size` / `output_size` and `lens.camera_matrix`) when the OpenFX render request is a preview thumbnail or sub-scale proxy, where applying the transform against the wrong buffer aspect would pollute `stab.params` and the idempotency guard would then prevent the subsequent full-resolution main render from recomputing.

Two complementary skip conditions:

1. **Sub-scale render**: `output_image.get_render_scale()` reports a value below 0.99 on either axis (proxy / quality preview).
2. **Inspector thumbnail**: the OpenFX output rect is less than 50% of the fuscript-reported timeline dimensions on either axis. Thumbnails arrive on tiny buffers (e.g. 288×162) at `render_scale=1.0` and would otherwise pass the sub-scale check; the dimension comparison catches them.

When either condition holds and the resolver is not in an override path (Fusion / Vegas / `DontDrawOutside`), the apply SHALL be skipped and the existing `stab.params.{size, output_size}` SHALL be left untouched. The full-resolution main render that follows fires the apply with the correct buffer aspect.

#### Scenario: Inspector thumbnail does not pollute stab

- **WHEN** Resolve issues a 288×162 inspector thumbnail render with `render_scale=1.0`, fuscript reports timeline 1920×1080, `HostInputSizing=FillCrop`
- **THEN** the plugin computes `is_preview_thumbnail = true` (288*2 < 1920), skips the apply, and `stab.params.size` retains the pre-load value; the next full-resolution main render with buffer 1920×1920 fires the apply with the correct aspect

### Requirement: Fresh-drop invalidates stale plugin-global cache

The plugin SHALL maintain a process-global `HostInputSizingCache` (mismatch mode + timeline dimensions + `useCustomSettings`) so that paste / .drp-restore instances created with a non-empty `ProjectPath` can skip the fuscript query and reuse the most recent value. When a new instance is created with an empty `ProjectPath` (the user dragged the plugin onto a clip for the first time in this DaVinci session, or after deleting and re-adding the plugin), the plugin SHALL invalidate the global cache before issuing a fresh fuscript query. This forces the new query to repopulate the cache with the user's current Resolve timeline / mismatched-resolution setting and prevents subsequent paste targets from inheriting a stale mode.

#### Scenario: User changes Resolve mismatch setting, deletes and re-adds plugin

- **WHEN** the user had `HostInputSizingCache = scaleToCrop` from a previous instance, changes Resolve's mismatched-resolution to `scaleToFit`, deletes the existing plugin node, then drops the plugin onto a clip again
- **THEN** the new CreateInstance sees `ProjectPath == ""`, invalidates the cache, spawns a fresh fuscript query, and the cache repopulates with `scaleToFit`; subsequent paste targets in the same session resolve to `Fit` mode

### Requirement: Cold-fuscript Fit fallback (no passthrough)

When the OpenFX `HostInputSizing` UI is `Auto` and the fuscript cache is still cold (CreateInstance hasn't received its first fuscript response yet), the plugin SHALL fall through to the normal render path with the resolver's `Fit` fallback applied. The plugin SHALL NOT return `OK` without writing to the destination buffer (the previous passthrough behavior produced a white flash because the OFX host does not pre-fill the destination buffer with source content).

#### Scenario: First render of a freshly-loaded plugin with cold fuscript

- **WHEN** the user first drops the plugin onto a clip, fuscript has not yet returned a value, and the OpenFX `HostInputSizing` UI is `Auto`
- **THEN** the resolver falls back to `Fit`, the plugin renders the existing letterbox / centered-band path, and the user sees a stable letterboxed frame instead of a white flash; the subsequent render after fuscript fires with the resolved mode (e.g. `FillCrop`) and switches to that path

### Requirement: Persist detected mismatch mode in a hidden OFX param per node

The OpenFX plugin SHALL expose a hidden string parameter `DetectedMismatchMode` on every plugin instance. The parameter SHALL be registered with `set_secret(true)` and excluded from the visible page tree. Permitted values are the empty string (uninitialized) and the four raw fuscript strings: `scaleToFit`, `scaleToCrop`, `centerCrop`, `stretch`.

The plugin SHALL use the raw fuscript string (not the mapped `HostInputSizing` enum index) so the persisted wire format remains stable when Resolve adds new mismatch modes in the future.

Because OFX-hidden parameters are serialised to `.drp` by the host and copied by "Paste Attributes", the persisted mismatch mode SHALL survive project save / restore and node copy / paste without any explicit serialisation logic in the plugin.

#### Scenario: Reopening a project restores per-node mismatch mode without fuscript

- **WHEN** a project containing N Gyroflow OFX nodes (each with `DetectedMismatchMode = "scaleToCrop"` persisted from a previous session) is reopened in a fresh Resolve process
- **AND** the plugin-global `host_input_sizing_cache` is empty (fresh plugin process)
- **THEN** every restored `CreateInstance` reads `DetectedMismatchMode = "scaleToCrop"` from its hidden field, populates `current_file_info.mismatch_mode = Some("scaleToCrop")`, and the `Auto` resolver returns `FillCrop` for every node without issuing any fuscript queries

#### Scenario: Paste Attributes carries mismatch mode

- **WHEN** node A has `DetectedMismatchMode = "scaleToCrop"` and the user invokes Resolve's "Paste Attributes" onto node B
- **THEN** node B's `DetectedMismatchMode` slot is populated with `"scaleToCrop"` by Resolve's standard OFX paste machinery, with no plugin-side paste handling required

### Requirement: CreateInstance reads the hidden field only when the global cache is empty

`CreateInstance` SHALL preserve the existing decision tree (global cache hit / cache miss + ProjectPath non-empty / cache miss + ProjectPath empty) without restructuring. The only extension SHALL be inside the "cache miss + ProjectPath non-empty" branch: before the existing skip-fuscript log line, the plugin SHALL read `DetectedMismatchMode` from the node and, when non-empty, populate `instance.current_file_info = Some(CurrentFileInfo { mismatch_mode: Some(raw), .. })` with the other CurrentFileInfo fields left at safe defaults (`timeline_w` / `timeline_h` = 0).

The plugin SHALL NOT consult the hidden field in the cache-hit branch (the cache value wins) or in the fresh-drop branch (fuscript runs as before). The plugin SHALL NOT consult `ProjectPath` for any decision other than the existing branch selection.

#### Scenario: Cache hit ignores the hidden field

- **WHEN** the global cache has `mismatch_mode = Some("scaleToCrop")` and the node's hidden field is `"scaleToFit"`
- **THEN** `CreateInstance` populates `current_file_info` from the cache (`scaleToCrop`); the hidden field is not consulted

#### Scenario: Fresh drop ignores the hidden field

- **WHEN** the user drags a Gyroflow node onto a clip and `ProjectPath` is empty
- **THEN** `CreateInstance` invalidates the cache (existing behaviour) and runs fuscript; the hidden field is not consulted

#### Scenario: Legacy `.drp` with empty hidden field falls back to existing behaviour

- **WHEN** a node restored from a `.drp` saved by the pre-change plugin has `ProjectPath` non-empty but `DetectedMismatchMode = ""`
- **THEN** the plugin logs the existing "skipping fuscript" message and leaves `current_file_info = None`, falling through to the `Auto`-resolver-`Fit`-fallback path — identical to pre-change behaviour

### Requirement: Render-path mirror block persists mismatch into the hidden field

The existing Render-path mirror block, which copies `current_file_info` into `host_input_sizing_cache` whenever the values differ, SHALL be extended to also write the raw mismatch string into the node's `DetectedMismatchMode` hidden field. The hidden-field write SHALL happen AFTER all `current_file_info` and `host_input_sizing_cache` locks have been released, so a synchronous `InstanceChanged` callback dispatched by Resolve in response to `set_value` cannot deadlock on a re-entered lock.

The hidden-field write SHALL be skipped when the new value is empty, and SHALL be skipped when the value already matches what is persisted on the node (avoiding spurious `InstanceChanged` events and OFX serialisation churn).

The plugin SHALL NOT write `DetectedMismatchMode` from inside `CreateInstance` (any path), because calling `set_value` before `effect.set_instance_data(...)` would let a synchronous `InstanceChanged` callback fire while the instance data is not yet stored, causing `effect.get_instance_data` to return invalid memory.

#### Scenario: First render after fresh drop persists mismatch

- **WHEN** node A is fresh-dropped (CreateInstance runs fuscript, populates current_file_info), then the first Render fires
- **THEN** the mirror block writes the cache as before AND calls `detected_mismatch_mode.set_value("scaleToCrop")`, so subsequent `.drp` reopens find the value persisted

#### Scenario: No spurious writes when value is unchanged

- **WHEN** Render fires repeatedly on a node whose hidden field already matches `current_file_info.mismatch_mode`
- **THEN** the mirror block skips the `set_value` call

### Requirement: InstanceChanged short-circuits the hidden persistence param

The `InstanceChanged` handler SHALL include a short-circuit branch for parameter name `DetectedMismatchMode` that returns `OK` without other work. This branch SHALL be placed before the existing `from_str(Params::*)` lookup so that the hidden persistence param does not fall through to the `log::error!("Unknown param name: ...")` path.

The branch SHALL NOT inspect `Change` reason, modify `current_file_info`, clear apply markers, or trigger any other side effect — the hidden field is pure storage and its state is consumed only at `CreateInstance` time.

#### Scenario: Plugin-initiated write echoes back without log noise

- **WHEN** the Render-path mirror block calls `detected_mismatch_mode.set_value("scaleToCrop")` and Resolve dispatches `InstanceChanged(DetectedMismatchMode)`
- **THEN** the handler returns `OK` immediately; the existing `Unknown param name` error log line does not fire

#### Scenario: Paste-initiated write does not affect render state

- **WHEN** Resolve's "Paste Attributes" lands a new `DetectedMismatchMode` value on this node and dispatches `InstanceChanged(DetectedMismatchMode)`
- **THEN** the handler returns `OK` immediately; `applied_host_input_sizing` and `pre_mode_*` snapshots are NOT cleared (the per-render apply path picks up the new value from the cache or hidden-field fallback on the next CreateInstance / first Render after the paste-induced reload)

### Requirement: Thumbnail guard widened to cover the hidden-field fallback

The Render path's `is_preview_thumbnail` heuristic, which prevents `apply_host_input_sizing_if_needed` from baking a crop computed against a tiny inspector / thumbnail buffer, SHALL be widened to OR the legacy `fuscript_tw/th`-based check with an unconditional small-buffer cutoff: any render whose output buffer is non-zero in both dimensions and whose width OR height is below 400 pixels SHALL be treated as a thumbnail and SHALL skip the apply call.

This widening is required because the hidden-field fallback path leaves `current_file_info.timeline_w / timeline_h = 0`, defeating the legacy `fuscript_tw > 0` precondition. Without the cutoff, a thumbnail render running before the main render bakes a crop computed for the thumbnail buffer aspect, and the main render reuses the baked size via the `applied_host_input_sizing` idempotency guard, producing a stretched main output.

The legacy `fuscript_tw/th`-based check SHALL remain in place; the new cutoff is purely additive (`legacy_check OR new_cutoff`) so cache-hit-path behaviour is preserved bit-for-bit.

#### Scenario: Hidden-field fallback render skips apply on thumbnail buffer

- **WHEN** a node restored via the hidden-field fallback (timeline_w/h = 0) receives a Render call with output buffer 288×162
- **THEN** the buffer-size cutoff matches (`288 < 400`), `is_preview_thumbnail` evaluates true, and apply is skipped — the main render is the first to bake `stab.params.size`

#### Scenario: Cache-hit path is unaffected

- **WHEN** a cache-hit node receives a Render call with a 1920×1080 buffer
- **THEN** both the legacy check and the new cutoff evaluate false; apply runs as in the unmodified plugin

### Requirement: PAR-composition classification survives restore without fuscript clip evidence

The PAR-composited band classifier SHALL NOT require fuscript `CurrentFileInfo.width`, `height`, `pixel_aspect_ratio`, or timeline dimensions. A restored or pasted instance with only the persisted mismatch mode SHALL classify from the loaded `.gyroflow` logical/physical aspects and the actual OFX source-buffer aspect. The existing plugin-global cache and hidden mismatch-mode field SHALL remain unchanged; the classification SHALL NOT add UI parameters, persistent project fields, or source/PAR values to the global cache.

The host contract is that users apply the anamorphic PAR corresponding to the loaded `.gyroflow` raw lens stretch in Resolve. OFX clip/image PAR values describe Resolve's square-pixel effect buffer and MAY be logged for diagnosis, but SHALL NOT be used as the original clip PAR.

#### Scenario: Restored main render defaults to the logical content band

- **WHEN** a Resolve Edit/Color `Fit` instance is restored with empty clip-level fuscript fields, its loaded lens is anamorphic, the render is not a preview/subscale request, and the actual source-buffer aspect does not match the physical input aspect
- **THEN** the render selects the host-PAR-composited band without re-running fuscript

#### Scenario: Restored source-native buffer remains physical

- **WHEN** the same restored instance receives a source-native squeezed buffer whose aspect matches the physical input aspect
- **THEN** the render keeps the physical band behavior

#### Scenario: Preview and subscale renders remain physical

- **WHEN** Resolve requests an inspector thumbnail, proxy, or other subscale render
- **THEN** the classifier keeps the physical behavior regardless of the small buffer aspect

#### Scenario: Cache and persistence semantics remain unchanged

- **WHEN** a fresh fuscript query populates clip-level fields or a restored instance has only the hidden mismatch mode
- **THEN** those fields continue to drive their existing host-input-sizing responsibilities but do not change the buffer-space classifier
