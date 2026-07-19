# Delta: openfx-host-input-sizing (ofx-anamorphic-band-guess)

## ADDED Requirements

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

### Requirement: Timeline resolution honors the timeline custom-settings override

When the current timeline reports `useCustomSettings == '1'`, the fuscript query SHALL read `timelineResolutionWidth` / `timelineResolutionHeight` from the timeline (`tl:GetSetting`), falling back to the project-level read (`proj:GetSetting`) when the timeline-level values are empty (older Resolve versions / missing keys). When `useCustomSettings` is not `'1'`, the project-level read SHALL be used, unchanged.

The resulting `timeline_w` / `timeline_h` feed `stab.params.size` in Stretch mode and the FillCrop/CenterCrop crop-geometry computation; on a custom-resolution timeline (e.g. a portrait 1080×1920 timeline inside a 1920×1080 project) the project-level read returns the wrong dimensions and mis-models the crop axis.

#### Scenario: Portrait custom timeline in a landscape project

- **WHEN** the current timeline has `useCustomSettings='1'` with resolution 1080×1920 while the project's timeline resolution setting is 1920×1080
- **THEN** the parsed `CurrentFileInfo.timeline_w/timeline_h` is 1080×1920 (the timeline-level values), and the FillCrop/CenterCrop crop geometry and Stretch-mode `stab.params.size` use the actual timeline dimensions

#### Scenario: Timeline-level resolution keys unavailable

- **WHEN** `useCustomSettings='1'` but `tl:GetSetting('timelineResolutionWidth')` returns an empty value (older Resolve)
- **THEN** the query falls back to the project-level `timelineResolutionWidth/Height` and behavior matches the pre-change read

#### Scenario: Default-settings timeline unchanged

- **WHEN** the current timeline reports `useCustomSettings` ≠ `'1'`
- **THEN** the resolution is read from the project level exactly as before the change

### Requirement: FillCrop/CenterCrop crop model uses physical source dimensions

The FillCrop/CenterCrop transform SHALL model the host's crop in physical (squeezed) pixel space: `stab.params.size` divided per storage axis by the lens stretch baked into it (raw λ ÷ live, ≤0.01 guarded, `GYROFLOW_OFX_ANAMORPHIC_BAND=0` → 1.0), because Resolve's input-sizing decision operates on the clip's storage pixels (clip PAR is not applied when compositing into the effect buffer). The resulting physical crop SHALL be scaled back to the desqueezed space per display axis (mapped through the 90/270 rotation) before mutating `stab.params.size` / `output_size` / `calib_dimension` / `camera_matrix`.

When the physical crop is an identity (zero offsets, crop within ±1 px of the full physical display size), the transform SHALL NOT mutate the stab state at all — the untouched (Fit-equivalent) state renders that geometry correctly.

#### Scenario: Vertical anamorphic clip matching the timeline — no phantom crop

- **WHEN** DSC_3172 (stored 1920×1080, rotation 270, v_stretch 1.5, `params.size` (1920,1620)) sits on a 1080×1920 timeline with effective mode FillCrop
- **THEN** the physical display size (1080×1920) matches the timeline aspect, the transform is skipped, and `params.size`/`output_size` keep their desqueezed values (previously the desqueezed 0.84 aspect fabricated a 1.5× horizontal crop → 720-wide render with side pillarbox bars)

#### Scenario: Landscape anamorphic clip genuinely cropped by the host

- **WHEN** a 5760×2160-desqueezed (h_stretch 1.5) clip sits on a 1080×1920 timeline with effective mode FillCrop
- **THEN** the crop is computed on the physical 3840×2160 frame (centered 1215×2160) and scaled back to the desqueezed space as a centered 1823×2160 crop at x-offset 1968

#### Scenario: Non-anamorphic clips unchanged (except identity skip)

- **WHEN** the lens has no anamorphic stretch
- **THEN** genuine crops compute the same values as before the change; identity crops (source display aspect already matches the timeline) now skip the mutation instead of rewriting equal values
