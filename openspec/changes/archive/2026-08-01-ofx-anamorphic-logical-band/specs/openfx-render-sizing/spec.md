# Delta: openfx-render-sizing (ofx-anamorphic-logical-band)

## MODIFIED Requirements

### Requirement: Output buffer aspect-ratio fit

Only the `HostParComposited` selection rule changes. The two centered-band derivations, the
`LegacyLogical` and `Physical` definitions, the input/output derivation bullets, the buffer-space
squash rule, the non-anamorphic byte-equivalence rule, the kill-switch, and the `DontDrawOutside`
derivation are all unchanged.

- `HostParComposited` SHALL use the stretch-baked logical ratios for both input and output on every main Resolve Edit/Color `Fit` render of an anamorphic lens, independent of the actual source-buffer aspect.

`HostParComposited` SHALL require all of the following: Resolve host name `DaVinciResolve` (the canonical `com.blackmagicdesign.resolve` alias MAY also be accepted); Edit/Color `Fit`; `DontDrawOutside=false`; not the Fusion page; anamorphic raw stretch (factors <= 0.01 treated as 1.0); a main render rather than preview/subscale; and a non-degenerate source buffer. Failure of any gate SHALL select `Physical`. The source-buffer aspect SHALL NOT participate in the decision, and neither SHALL fuscript clip fields nor OFX clip/image PAR.

The host contract is that the user applies the anamorphic PAR corresponding to the loaded `.gyroflow` raw lens stretch in Resolve's Clip Attributes, so a main Fit render receives the already-desqueezed frame composited into the timeline buffer. The band is therefore fully determined by the project's own `output_size` together with the existing `HostInputSizing` and `InputRotation` parameters.

An anamorphic clip left un-desqueezed in Resolve is outside this contract: its main Fit render receives the logical band as well, which is wrong for it. Distinguishing the two requires a host signal stating what the host did to the buffer, and none is available that is both instance-bound and durable — the fuscript clip PAR resolves from the playhead's clip, is not republished by the expiry-driven refresh, and returns empty after a project reopen. An earlier revision inferred the distinction from the source-buffer aspect instead; that is degenerate whenever the timeline aspect matches the squeezed source's (the ordinary 16:9 pairing) and produced warped top/bottom black bands plus rolling-shutter jello on landscape 1.5x material.

#### Scenario: Protective gates - physical behavior preserved

- **WHEN** the render comes from another host, is on the Fusion page, has `DontDrawOutside` enabled, is a preview/subscale request, uses a non-`Fit` `HostInputSizing` mode, carries a non-anamorphic lens, or supplies a degenerate buffer
- **THEN** the selector uses `Physical` and behavior is identical to `6b8cb14`

#### Scenario: Buffer aspect matching the physical one still selects logical

- **WHEN** a main Resolve `Fit` anamorphic render supplies a buffer whose outer aspect matches the physical input aspect - the ordinary pairing of a 16:9 timeline with a 16:9 squeezed source
- **THEN** the selector still uses `HostParComposited`, because that coincidence carries no information about what the host did to the buffer; keying on it produced warped top/bottom black bands and rolling-shutter jello on landscape 1.5x material

## REMOVED Scenarios

### Scenario: Source-native buffer - physical behavior preserved

Replaced by the two scenarios above. Its condition ("the main buffer matches the physical input
aspect") is the coincidence that cannot be relied on: with a 16:9 timeline and a 16:9 squeezed
source it holds for host-desqueezed buffers too.
