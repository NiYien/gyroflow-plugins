# Proposal: ofx-anamorphic-logical-band

## Why

A user reported "anamorphic lens stabilization is wrong, and the sync point is correct" against
Gyroflow(NiYien) 1.6.3 on macOS. The visible defects were warped black bands along the top and bottom
of the frame plus rolling-shutter jello. The same `.gyroflow` project stabilized correctly in the
desktop app on both the reporter's machine and the maintainer's, so the defect was in the OpenFX
plugin only.

Root cause: the aspect-space selection added by `archive/2026-07-19-ofx-anamorphic-band-guess`
(commit `6b8cb14`) decides between the **physical** (source-native squeezed) and **logical**
(host-desqueezed) content bands by comparing the source buffer's outer aspect against the physical
one:

```rust
if !logical_and_physical_are_distinct
    || aspects_match_within_one_percent(source_buffer_aspect, physical_aspects.0)
{ Physical } else { HostParComposited }
```

That comparison carries no information in the ordinary case. A 16:9 timeline and a 16:9 squeezed
source have the same outer aspect, so both the host-desqueezed and the source-native buffer present
as "matches physical". Landscape 1.5x material therefore selected `Physical` and the whole
4023x2268 buffer was taken as content, while only the middle 1509 rows carry picture:

- the 379-row letterbox on each side was sampled as image and dragged into frame by the warp
  → warped top/bottom black bands;
- the rolling-shutter row→time mapping was spread over 2268 rows instead of 1509
  → jello, with the sync offset itself untouched.

That archived change was verified on three assets (DSC_3172 portrait, R5MK2, C0016) and its record
states plainly that landscape was **not** verified. This is that unverified axis.

## What Changes

`select_anamorphic_band_aspects` keeps every protective gate and replaces only the final decision:
on a main Resolve Edit/Color `Fit` render of an anamorphic lens it now returns the logical band
unconditionally. The buffer-aspect comparison and its helper `aspects_match_within_one_percent` are
removed.

This rests on an explicit workflow contract, decided by the maintainer: **anamorphic clips are
desqueezed in Resolve's Clip Attributes**. Under it the frame reaching the effect is the already
widened one composited into the timeline buffer, so the band follows from the project's own
`output_size` together with the existing `HostInputSizing` and `InputRotation` parameters. Nothing
about the host has to be inferred from buffer geometry, and no new input is read.

## Impact

- **Fixed**: main Fit renders of anamorphic material desqueezed in Resolve, landscape and rotated
  alike.
- **Regressed, accepted**: an anamorphic clip left un-desqueezed in Resolve now receives the logical
  band too, which is wrong for it. This is the configuration `6b8cb14` was built for. Pinned by
  `undesqueezed_anamorphic_clip_also_gets_the_logical_band_by_design` so it cannot be "fixed" back
  by accident.
- **Unchanged**: non-Resolve hosts, non-`Fit` sizing modes, the Fusion page, `DontDrawOutside`,
  preview/subscale renders, non-anamorphic lenses, degenerate buffers, and the
  `GYROFLOW_OFX_ANAMORPHIC_BAND` kill-switch.
- No new parameters, no new persisted fields, no new host queries, no i18n strings.
