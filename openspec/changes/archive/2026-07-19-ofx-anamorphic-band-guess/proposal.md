# Proposal: ofx-anamorphic-band-guess

## Why

Loading the OpenFX plugin on a rotated anamorphic clip in DaVinci Resolve (Edit page, scaleToFit) crops the visible content: DSC_3172 (Nikon ZR, stored 1920×1080, rotation 270, `input_vertical_stretch` 1.5) loses the top and bottom 1/6 of the frame. Root cause (pinned via bisect + log evidence, 2026-07-17): the render path's two aspect-ratio "content band" guesses use the stretch-baked logical size (`params.size` / `params.output_size`) where the physical (squeezed) pixel aspect is required. The host hands a source-native squeezed buffer that is fully covered by content; the mismatched ratio makes `get_center_rect` misclassify 320 real rows on each side as letterbox bars and discard them. Landscape anamorphic clips hit the same defect as left/right cropping. Non-anamorphic clips are unaffected because logical and physical aspects coincide.

Live verification on 2026-07-19 exposed a second valid Resolve buffer model: a vertical-to-wide anamorphic clip in Edit/Color `Fit` arrived as a timeline-aspect buffer with the clip PAR already composited. The unconditional physical-band rule from `6b8cb14` then applied the squeeze a second time (`org_ratio=0.5625`, 608×1080 centered band instead of the full 972×1080 buffer), cropping the left and right sides. `GYROFLOW_OFX_ANAMORPHIC_BAND=0` restored the full buffer and the user confirmed the image returned to normal. The fix therefore cannot globally choose either logical or physical space; it must classify the actual host buffer between those two known spaces.

The first classifier used fuscript `CurrentFileInfo` source dimensions/PAR/timeline dimensions. A `.drp` restore intentionally skips fuscript and persists only mismatch mode, so that evidence was absent and the classifier fell back to `Physical`. A 2026-07-19 live OFX probe then confirmed that both `kOfxImagePropPixelAspectRatio` reads return `1.0` after Resolve has composited the clip into a square-pixel effect buffer; they do not expose the Media Pool clip PAR. Per the user's host contract, applying the anamorphic PAR that corresponds to the loaded `.gyroflow` raw lens stretch is the user's responsibility. The restore-safe classifier therefore uses the loaded logical/physical aspects plus the actual source-buffer aspect and does not depend on fuscript clip fields or OFX PAR.

## What Changes

- Keep the existing physical input/output band derivations as the default and conservative fallback for source-native squeezed buffers.
- Add a three-space selector for Resolve Edit/Color `Fit`: `LegacyLogical` exists only behind the kill-switch; a main source buffer matching the physical input aspect stays `Physical`; otherwise `HostParComposited` uses the loaded `.gyroflow` logical aspects under the user-applied-PAR contract.
- Gate the selection on the Resolve host (observed OFX name `DaVinciResolve`, canonical alias accepted), Edit/Color `Fit`, `DontDrawOutside=false`, anamorphic raw stretch, and a non-preview render. Source-native buffers matching `Physical`, preview/subscale renders, Fusion, non-Fit modes, `DontDrawOutside`, Vegas/other hosts, and non-anamorphic sources stay on `Physical`.
- Keep OFX clip/image PAR in the diagnostic line only. Resolve returning `1.0` for the square-pixel effect buffer is expected and is not classification evidence.
- Add an env kill-switch `GYROFLOW_OFX_ANAMORPHIC_BAND=0` that restores the previous (stretch-blind) guesses, following the house pattern (`GYROFLOW_ADOBE_ANAMORPHIC_FULL_FRAME`).
- **fuscript timeline-resolution fix** (found during live verification, user-directed 2026-07-17): when `useCustomSettings='1'`, read `timelineResolutionWidth/Height` from the timeline instead of the project (empty → project fallback). A portrait 1080×1920 custom timeline inside a 1920×1080 project previously fed the wrong dimensions into Stretch-mode `stab.params.size` and the FillCrop/CenterCrop crop geometry.
- **Not** porting the Premiere v2 mechanism (desktop-state gating, output-size override dance) — explicitly ruled out by the user. Scope is the two guess sites only.
- No gyroflow-core changes, no new UI parameters, no Adobe/frei0r changes.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `openfx-render-sizing`: the input content-band and output aspect-fit guesses SHALL select among legacy-logical, physical, and host-PAR-composited aspect spaces. A physical-aspect source-native buffer wins; otherwise a main Resolve Edit/Color `Fit` render defaults to the `.gyroflow` logical space because applying the corresponding host PAR is the user's responsibility.
- `openfx-host-input-sizing`: the fuscript query's timeline resolution SHALL honor the timeline custom-settings override (`ucs='1'` → `tl:GetSetting`, empty → project fallback).

## Impact

- **Code**: `openfx/src/gyroflow.rs` only for the extension (pure buffer-space selector, render wiring, tests). No scripting query, global-cache expansion, UI parameter, or persisted project field is required.
- **Behavior**:
  - Vertical anamorphic on Resolve Edit page: crop disappears; full frame rendered squeezed, widened on display by clip PAR. Display aspect carries the known ~11% approximation when the user must pick PAR 1.33 ("16:9 anamorphic") because Resolve has no 1.5 preset — outside code's reach, unchanged by this fix.
  - Landscape anamorphic: same fix applies (previously cropped left/right).
  - PAR-composited timeline buffer: a source-native physical match wins; otherwise a main Resolve Edit/Color `Fit` render uses the loaded logical band under the user-applied-PAR contract, avoiding a second squeeze.
  - Non-anamorphic sources (stretch = 1.0): both ratios divide by 1.0 — byte-equivalent behavior, no regression surface.
  - FillCrop/CenterCrop/Stretch host sizing modes and `DontDrawOutside`: unaffected (band guesses bypassed or derived consistently).
- **Host contract**: users are responsible for applying the anamorphic PAR corresponding to the loaded `.gyroflow` raw lens stretch in Resolve. On a main Resolve `Fit` render, a buffer that does not match the source-native physical aspect is therefore treated as host-PAR-composited and uses the logical content-band aspect.
- **Dependencies**: relies on the kernel's existing per-axis output-rect mapping (verified as-built in `adobe-rotated-anamorphic-full-frame`, commit c536ce8, "zero gyroflow-core changes").
