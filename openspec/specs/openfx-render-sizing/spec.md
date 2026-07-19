# openfx-render-sizing

## Purpose

Defines how the OpenFX plugin maps host input/output image buffers onto the stabilization core's logical input/output frames, including aspect-ratio-mismatch handling (input content-band crop and output letterbox/pillarbox fit).
## Requirements
### Requirement: Output buffer aspect-ratio fit

The OpenFX render path SHALL map host buffers onto the stabilization core's logical frames using two centered-band derivations (`GyroflowPluginBase::get_center_rect`, existing 0.1 aspect tolerance) and an explicit aspect-space selection:

- `LegacyLogical` SHALL use the stretch-baked logical ratios only when `GYROFLOW_OFX_ANAMORPHIC_BAND=0|off|false`.
- `Physical` SHALL use the stretch-divided ratios and SHALL remain the default and every evidence-poor/inconsistent fallback.
- `HostParComposited` SHALL use the stretch-baked logical ratios for both input and output on a main Resolve Edit/Color `Fit` render when the actual source buffer does not match the source-native physical input aspect.

- **Input content band** (`src_rect`): when the effective `HostInputSizing` is `Auto` or `Fit`, or `DontDrawOutside` is enabled, the plugin SHALL guess the content band of the source buffer using the selected aspect space. The physical base is `params.size` divided per storage axis by the lens `input_horizontal_stretch` / `input_vertical_stretch` (factors <= 0.01 treated as 1.0), transposed when the effective InputRotation is 90/270. When the effective `HostInputSizing` is `FillCrop`, `CenterCrop`, or `Stretch`, the input rect SHALL remain `None` (full buffer, unchanged).

- **Output aspect fit** (`aspect_fit_output`): when `DontDrawOutside` is disabled, the plugin is not on the Fusion page, and the effective `HostInputSizing` is `Auto` or `Fit`, the plugin SHALL derive the output rect from the selected output aspect. The physical base is `params.output_size` with each display axis divided by the stretch factor that produced it (display-horizontal divided by `input_vertical_stretch` when InputRotation is 90/270 and by `input_horizontal_stretch` otherwise; conversely for display-vertical). The render-scale (proxy) composition SHALL apply the same selected derivation at the scaled dimensions.

`HostParComposited` SHALL require all of the following: Resolve host name `DaVinciResolve` (the canonical `com.blackmagicdesign.resolve` alias MAY also be accepted); Edit/Color `Fit`; `DontDrawOutside=false`; anamorphic raw stretch; a main render rather than preview/subscale; logical input aspect differing from the physical input aspect by more than 0.1; and actual source-buffer aspect not matching the physical input aspect within 1%. A physical match, preview/subscale request, another host, or failure of any gate SHALL select `Physical`. Fuscript clip fields and OFX clip/image PAR SHALL NOT participate in the decision.

The host contract is that the user applies the anamorphic PAR corresponding to the loaded `.gyroflow` raw lens stretch in Resolve. The logical aspect is therefore the expected post-PAR buffer aspect; the physical aspect remains the expected source-native squeezed buffer aspect.

Buffer-space squash SHALL occur only by the lens stretch factors (host-PAR pre-compensation); the plugin SHALL NOT otherwise stretch or squash the stabilized image to fill a mismatched buffer. Aspect mismatches beyond the stretch factors are still resolved by centered letterbox/pillarbox bands.

For non-anamorphic sources (both stretch factors = 1.0) every derived ratio SHALL equal the stretch-blind computation and rendering SHALL be byte-equivalent to the pre-change behavior.

The environment kill-switch `GYROFLOW_OFX_ANAMORPHIC_BAND=0|off|false` SHALL restore the stretch-blind (pre-change) guesses; it SHALL be read once per process and log once when disabled.

The `DontDrawOutside` output-rect derivation SHALL continue to be derived from `src_rect` (and therefore follows the corrected input band). Region-of-definition behavior and `getClipPreferences` SHALL remain unchanged.

#### Scenario: Vertical anamorphic on source-native buffer - full frame, no crop (DSC_3172)

- **WHEN** the source is stored 1920x1080 with rotation 270 and `input_vertical_stretch` 1.5 (`params.size` (1920,1620), `params.output_size` (1620,1920)), and Resolve (Edit page, effective `HostInputSizing` = `Fit`) supplies source-native 1080x1920 buffers fully covered by squeezed content
- **THEN** the input physical aspect is (1620/1.5 = 1080)/(1920/1.0 = 1920) transposed -> 1080/1920, equal to the buffer aspect, so `src_rect` is the full buffer and no content rows are discarded
- **AND** the output physical aspect is (1620/1.5)/(1920/1.0) = 0.5625, equal to the buffer aspect, so the output rect is the full buffer
- **AND** the kernel maps the 1620x1920 logical output onto the 1080x1920 buffer per axis (horizontal squeeze) and the host clip PAR widens it on display

#### Scenario: Landscape 2x anamorphic - full frame, no crop

- **WHEN** the source is stored 1920x1080 without rotation and `input_horizontal_stretch` 2.0 (`params.size` (3840,1080), `params.output_size` (3840,1080)), and the host supplies source-native 1920x1080 buffers
- **THEN** the input physical aspect is (3840/2.0)/(1080/1.0) = 16:9, equal to the buffer aspect, so `src_rect` is the full buffer (previously the stretch-blind guess 3.556 cropped the buffer to a centered 1920x540 band)
- **AND** the output rect is the full buffer and the kernel squeezes 3840->1920 horizontally; the host's 2.0x anamorphic PAR preset widens it exactly on display

#### Scenario: Restored PAR-composited content band - no double squeeze or side crop

- **WHEN** an anamorphic Resolve Edit/Color `Fit` main render has no clip-level fuscript evidence, reports host `DaVinciResolve`, and supplies a 1920x1080 timeline buffer whose source-native physical aspect is 0.5625 while the loaded logical content aspect is 0.9
- **THEN** both band ratios use `HostParComposited`, `get_center_rect` keeps the full 972x1080 logical content band, and the plugin does not shrink it again to a 608x1080 physical band

#### Scenario: Source-native buffer - physical behavior preserved

- **WHEN** the main buffer matches the physical input aspect, comes from another host, or any Resolve/Fit/anamorphic gate fails
- **THEN** the selector uses `Physical` and behavior is identical to `6b8cb14`

#### Scenario: Non-anamorphic sources - byte-equivalent

- **WHEN** the lens profile has `input_horizontal_stretch` = `input_vertical_stretch` = 1.0 (or unset/<=0.01)
- **THEN** every derived aspect equals the stretch-blind value and the rendered output is byte-equivalent to the pre-change behavior for all `HostInputSizing` modes, `DontDrawOutside`, Fusion page, and proxy scales

#### Scenario: Kill-switch restores pre-change guesses

- **WHEN** `GYROFLOW_OFX_ANAMORPHIC_BAND=0` is set in the environment
- **THEN** both guesses use the stretch-baked logical sizes (an anamorphic clip reproduces the pre-change cropped framing) and a single log line records that the feature is disabled

#### Scenario: Thumbnail / proxy previews remain physical

- **WHEN** the host requests a scaled buffer (`out_scale != 1.0`) for an anamorphic clip in `Fit` mode
- **THEN** the selector uses `Physical` regardless of the small buffer aspect, preserving the existing preview framing and y-inversion composition

### Requirement: Output aspect fit applies only on the Edit/Color page

The output aspect-ratio fit (`aspect_fit_output`) SHALL apply only when the plugin is running on the host's Edit or Color page. It SHALL NOT apply on the Fusion page, where the plugin processes the original video at its native resolution and the output buffer is already at the stabilized output's aspect ratio.

#### Scenario: Fusion page - output aspect fit skipped

- **WHEN** the plugin is running on the Fusion page (`is_fusion_page` is true)
- **THEN** the output-rect derivation is skipped (`output.rect = None`, stab uses the full buffer), unchanged from current behavior

### Requirement: Preserve existing aspect-handling overrides

The output aspect-ratio fit SHALL only take effect in the default Render path. It SHALL NOT change behavior when the user has enabled `DontDrawOutside` (which has its own output-rect handling), and it SHALL NOT apply when running under the Vegas host, where the output rect is intentionally left unset.

#### Scenario: DontDrawOutside enabled - overrides the aspect fit

- **WHEN** the `DontDrawOutside` parameter is checked
- **THEN** the output rect is computed by the existing `DontDrawOutside` logic and the aspect-fit letterbox does not override it

#### Scenario: Vegas host - aspect fit skipped

- **WHEN** the host name is `com.vegascreativesoftware.vegas`
- **THEN** the output rect remains unset (`None`), unchanged from current behavior

### Requirement: Output rect derivation composes with output-adjust post-affine

The output rect derivation (Fit / FillCrop / CenterCrop / Stretch / DontDrawOutside / Fusion-page / Vegas-host branches defined elsewhere in this capability) SHALL continue to produce the same `out_rect` value it would produce without the `openfx-output-adjust` capability. The output-adjust post-affine (zoom + rotation + X/Y offset, defined in `openfx-output-adjust`) SHALL be applied **on top of** the derived `out_rect` by the stabilization kernel, not by modifying `out_rect`. When the post-affine is identity (the default for new and pre-existing plugin instances), rendered output for this capability's existing scenarios SHALL be byte-equivalent to the pre-change behavior.

#### Scenario: Identity post-affine leaves all existing output-rect scenarios unchanged

- **WHEN** the user does not interact with the new `OutputAdjustGroup` controls (post-affine is at identity)
- **THEN** every existing scenario in this capability — `Fit` mode aspect-fit letterbox, `FillCrop` 1:1 buffer fill, Fusion-page unchanged behavior, `DontDrawOutside`-enabled behavior, Vegas-host `out_rect = None` behavior, proxy/half-resolution scale adjustment — produces byte-equivalent output to the pre-change kernel

#### Scenario: Non-identity post-affine composes on top of Fit-mode letterbox

- **WHEN** the host buffer is 4000×3000 (4:3), core's logical output is 2554×1080 (≈2.36:1), `HostInputSizing` resolves to `Fit`, `DontDrawOutside` disabled, AND the user sets `OutputZoom = 1.2`
- **THEN** the kernel first composes the centered 4000×1691 stabilized band (the existing Fit-mode result), and the post-affine then magnifies that band by 1.2× in a single resampling pass; the user perceives a 1.2× zoomed-in version of the same letterbox composition

#### Scenario: Non-identity post-affine composes on top of FillCrop

- **WHEN** `HostInputSizing` resolves to `FillCrop` (the plugin has reassigned `stab.params.output_size` to match the host buffer aspect) AND the user sets `OutputRotation = 5°`
- **THEN** the kernel writes the full host buffer using the FillCrop output dimensions, rotated 5° about the buffer center; existing FillCrop framing is preserved, the rotation is layered on top

#### Scenario: DontDrawOutside still wins on output rect, post-affine still applies

- **WHEN** `DontDrawOutside` is enabled (it owns the output rect derivation) AND the user sets `OutputZoom = 1.3`
- **THEN** the output rect is computed by the existing `DontDrawOutside` logic, the kernel writes into that rect, and the post-affine zoom is applied on top of the stabilized image within that rect

#### Scenario: Fusion-page bypasses the post-affine

- **WHEN** the plugin runs on the Fusion page AND the user has set non-identity post-affine values
- **THEN** the Fusion page renders as if the post-affine were identity (per `openfx-output-adjust`'s Fusion-page bypass), and this capability's Fusion-page scenario continues to apply unchanged
