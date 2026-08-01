# Delta: openfx-host-input-sizing (ofx-anamorphic-logical-band)

## MODIFIED Requirements

### Requirement: PAR-composition classification survives restore without fuscript clip evidence

The requirement text is unchanged: the classifier still SHALL NOT require fuscript
`CurrentFileInfo.width`, `height`, `pixel_aspect_ratio`, or timeline dimensions, and still adds no
UI parameters, persistent project fields, or cache entries. Only the scenarios change — the
classification no longer consults the source-buffer aspect either.

#### Scenario: Restored main render defaults to the logical content band

- **WHEN** a Resolve Edit/Color `Fit` instance is restored with empty clip-level fuscript fields, its loaded lens is anamorphic, and the render is not a preview/subscale request
- **THEN** the render selects the host-PAR-composited band without re-running fuscript, whatever the actual source-buffer aspect is

#### Scenario: Source-buffer aspect does not alter the classification

- **WHEN** the same restored instance receives a buffer whose aspect happens to match the physical input aspect
- **THEN** the render still selects the host-PAR-composited band, because the buffer aspect cannot distinguish a host-desqueezed frame from a source-native one whenever the timeline aspect matches the squeezed source's

## REMOVED Scenarios

### Scenario: Restored source-native buffer remains physical

Replaced by "Source-buffer aspect does not alter the classification". The distinction it asserted is
not observable from the inputs the classifier has.
