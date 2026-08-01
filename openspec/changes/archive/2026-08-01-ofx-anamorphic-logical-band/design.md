# Design: ofx-anamorphic-logical-band

## Evidence chain

### Source material

Nikon ZR, Blazar Remus 45mm 1.5x horizontal anamorphic, landscape. The `.gyroflow` project was built
on the camera's 1920x1080 proxy MP4 (`proxy_output: 1`, `nraw_width/height: 4032/2268`); telemetry
resolves `unit_px_fl=53.48189187724539`, `pixel_focal_length=2406.685`,
`frame_readout_time=6.174590163934426`, giving `size=(1920,1080)` → `output_size=(2880,1080)` with
`input_horizontal_stretch=1.5`.

### The desktop app is not involved

Same clip (`DSC_5472`, 1284 frames), reporter's macOS/M4 machine vs maintainer's Windows/RTX 4060 Ti:

| field | reporter | maintainer |
|---|---|---|
| `unit_px_fl` | 53.48189187724539 | identical |
| `pixel_focal_length` | 2406.685 | identical |
| `frame_readout_time` | 6.174590163934426 | identical |
| `size` → `output_size` | (1920,1080) → (2880,1080) | identical |
| `h_stretch` / `scaling_factor` | 1.5000 / 1.000000 | identical |
| **`min_fov`** | **0.828903** | **0.828903** |
| `max_fov` | 0.985831 | 0.985830 |

`max_fov` differs in the sixth decimal (1 ulp, platform rounding); everything else is bit-identical.
The stabilization computation is therefore the same on both machines and on both GPU backends, which
rules out the whole lens/geometry/settings surface and localizes the defect to the plugin.

**Process note**: this comparison should have been the first step. Four earlier root-cause
candidates (anamorphic focal-length convention, the Blazar preset's poly5 coefficients, the Simple
mode project-import gate stripping `lens_correction_amount`, and the app/plugin split) were all
pursued and falsified before it was run.

### The plugin's own numbers

From `gyroflow-openfx.log`, reproduced identically on macOS/Metal + Resolve 20.2.2 and on
Windows/CUDA + Resolve 21.0.0:

```
max_zoom_entry seq=1 size=(1920,1080) output_size=(2880,1080) h_stretch=1.5000 min_fov=0.828903
post-mutation recompute fired, diff=["size", "input_horizontal_stretch"]
max_zoom_entry seq=2 size=(2880,1080) output_size=(2880,1080) h_stretch=1.0000 min_fov=0.807792
anamorphic band guess: space=Physical ... logical=(2.6667,2.6667) physical=(1.7778,1.7778)
                       buffer=(4023,2268) selected=(1.7778,1.7778) clip_par=Some(1.0)
backend_rebuild ... in_rect=Some((0, 0, 4023, 2268)) ... proc_size=(2880,1080)->(2880,1080)
```

`run 000` (before `disable_lens_stretch`) matches the app's `min_fov=0.8289` exactly; the divergence
appears only after the plugin's own geometry handling.

### A/B that isolated the decision

Running Resolve with `GYROFLOW_OFX_ANAMORPHIC_BAND=0` — which forces `LegacyLogical`, i.e. the
pre-`6b8cb14` behaviour — produced a correct picture:

| | default (broken) | `BAND=0` (correct) |
|---|---|---|
| space | `Physical` | `LegacyLogical` |
| selected | (1.7778, 1.7778) | (2.6667, 2.6667) |
| `in_rect` / `out_rect` | `(0, **0**, 4023, **2268**)` | `(0, **379**, 4023, **1509**)` |

`4023 / 2.6667 = 1509`; `(2268 - 1509) / 2 = 379`. The correct band is the logical one and the buffer
carries a 379-row letterbox on each side.

### Why the heuristic cannot work

The maintainer confirmed the workflow: the clip's pixel aspect ratio is set to 1.5 in Resolve's Clip
Attributes, so Resolve desqueezes 1920x1080 → 2.667:1 and fits that into the 4023x2268 timeline
frame, letterboxing it.

The discriminator the code used is `buffer_outer_aspect ≈ physical_aspect`. For a 16:9 timeline and
a 16:9 squeezed source those coincide, so the test returns "matches physical" for **both** the
host-desqueezed and the source-native case. Two candidate replacement signals were examined and
rejected:

- **Clip PAR from fuscript.** `CurrentFileInfo.pixel_aspect_ratio` does carry the 1.5, but it is
  resolved from `tl:GetCurrentVideoItem():GetMediaPoolItem():GetClipProperty()` (`fuscript.rs:167`)
  — the **playhead's** clip, not the instance being rendered. Publishing it per refresh would
  reproduce the cross-clip `ProjectPath` contamination that `fuscript.rs:280` already warns about.
  It is also absent in normal operation: the expiry-driven refresh writes back only
  `mismatch_mode / timeline_w / timeline_h / use_custom_settings / queried_at` (`fuscript.rs:302-315`),
  the `changed` comparison does not include it (`:287-296`), and both `host_sizing_placeholder`
  (`gyroflow.rs:76`) and the `.drp` hidden-field restore (`:2345`) leave it empty. After a project
  reopen the value is gone until the user explicitly triggers `LoadCurrent`/`ReloadProject`.
- **Buffer size versus `timeline_w/h`.** Timeline-level, so free of the clip-identity hazard, and it
  *is* republished on refresh — but no measured sample was available for the portrait case, and
  building a second heuristic on one sample is what produced this regression in the first place.

With no reliable instance-bound signal available, the two cases are not separable, and any rule can
only trade one error for the other. The maintainer therefore fixed the workflow instead of guessing
at it.

## Decision

Keep every gate; replace only the terminal decision.

```
!enabled                                                        → LegacyLogical(logical)
!is_resolve || !anamorphic || !mode_is_fit || is_fusion_page
  || dont_draw_outside || is_preview_or_subscale || degenerate  → Physical(physical)
                                                                → HostParComposited(logical)
```

Rejected alternatives, and why:

- **Drop the gates too** (an intermediate revision did this). `DontDrawOutside` still derives from
  `src_rect` (`gyroflow.rs:1891`), and the Fusion page does not render through timeline compositing
  at all; neither has a measured sample. Reverted after review.
- **Manual `Auto / Physical / Host PAR Composited` override.** Deterministic and immune to every
  fuscript problem, but the maintainer scoped it out once the workflow contract was fixed, since the
  automatic answer is then unambiguous.

Also folded in: `select_anamorphic_band_aspects` now applies the same "≤ 0.01 means unset, treat as
1.0" stretch convention as `physical_band_aspects`. The render path already guards this before
calling, so production behaviour is unchanged; it makes the function correct on its own terms.

## Verification

`cargo test -p gyroflow-ofx --lib` → 53 passed, 0 failed. `cargo build --release` clean.

Live in Resolve 21.0.0 with the plugin deployed and **no environment variables set** (the
`GYROFLOW_OFX_ANAMORPHIC_BAND` default is `enabled = true`, i.e. the fixed path — end users need no
configuration):

```
landscape raw=(1.5,1.0) rotated=false buffer=(4023,2268)
  → space=HostParComposited selected=(2.6667,2.6667)  in_rect=(0,379,4023,1509)
vertical  raw=(1.0,1.5) rotated_90_270=true buffer=(1920,1080)
  → space=HostParComposited selected=(0.8438,0.8438)
thumbnails buffer=(138,69) / (432,243)
  → space=Physical  (preview gate holds)
```

Both accepted by the maintainer. The vertical case additionally exercises `rotated_90_270=true`,
which had no prior sample — the rotation axis is handled inside `logical_aspect_pair`
(`gyroflow.rs:1778`) and needs no special casing here.

## Known limits

1. **Un-desqueezed anamorphic clips are wrong.** Accepted; see proposal. Test-pinned.
2. **Thumbnails stay `Physical` while the viewer uses `HostParComposited`.** Not a regression — the
   preview gate predates this change and thumbnail behaviour is unchanged — but the inspector
   thumbnail can now differ from the viewer. Revisiting needs the original rationale for that gate,
   which is not recorded anywhere but the code.
