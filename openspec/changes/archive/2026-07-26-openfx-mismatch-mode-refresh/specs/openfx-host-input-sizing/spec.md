# Delta: openfx-host-input-sizing (openfx-mismatch-mode-refresh)

## ADDED Requirements

### Requirement: Plugin-global mismatch cache carries a freshness window

The plugin SHALL maintain a process-global `HostInputSizingCache` (mismatch mode + timeline dimensions +
`useCustomSettings` + population timestamp) shared by every plugin instance in the session. Expiry is
the sole staleness mechanism; there SHALL NOT be an instance-lifecycle-driven invalidation path.

The cache SHALL record the time at which its entry was populated. A consumer requesting the host input
sizing mode SHALL compare the entry's age against a configurable TTL (default 10000 ms). An entry younger than the TTL SHALL be used without contacting fuscript. An entry
older than the TTL, or absent, SHALL cause exactly one background fuscript query to be issued while the
requesting render continues with the currently cached (possibly stale) value.

The refresh SHALL NOT block the calling thread. Resolve invokes CreateInstance and the render entry
points on threads whose stalling degrades or freezes the host.

The TTL SHALL be configurable via `GYROFLOW_OFX_MISMATCH_TTL_MS`. A value of `0` SHALL disable expiry
entirely, restoring the sticky-cache behavior for diagnosis. Invalid values SHALL fall back to the
default with a warning, and the resolved configuration SHALL be logged once.

#### Scenario: Setting changed while the plugin renders

- **WHEN** the user changes Resolve's mismatched-resolution setting (project or timeline level) and
  saves, while a plugin instance is rendering and its cached entry is older than the TTL
- **THEN** the next render observes the expiry, issues one background query, and within one TTL window
  the resolved `HostInputSizing` reflects the new setting — with no Resolve restart, no node re-drop,
  and no button press

#### Scenario: Fresh entry costs nothing

- **WHEN** a render requests the mode and the cached entry is younger than the TTL
- **THEN** no fuscript process is spawned and the cached value is used directly

#### Scenario: Expiry disabled

- **WHEN** `GYROFLOW_OFX_MISMATCH_TTL_MS=0`
- **THEN** the entry is used regardless of age and no periodic refresh occurs

### Requirement: Refresh is single-flight across all instances

When multiple plugin instances observe an expired or absent cache entry concurrently, the plugin SHALL
issue exactly one fuscript query. An atomic in-flight guard SHALL be acquired before spawning the query
and SHALL be released on both the success and the failure path of the query thread.

This bounds the steady-state fuscript rate at one invocation per TTL window process-wide, independent of
the number of plugin instances on the timeline. Per-instance querying is prohibited: a timeline with
several hundred plugin-bearing clips would otherwise spawn one `fuscript.exe` process per instance
against a serialized IPC endpoint.

#### Scenario: Project open with many plugin instances

- **WHEN** a project containing N plugin-bearing clips is opened and the global cache is empty
- **THEN** exactly one fuscript query is issued for the whole project open, and all N instances resolve
  their mode from the resulting cache entry

#### Scenario: Failed query does not wedge refresh

- **WHEN** a background refresh query fails (fuscript unavailable, compound clip, scripting disabled)
- **THEN** the in-flight guard is released and a later expiry check is able to issue a new query

### Requirement: Forced re-render fires only when the queried value changes

The fuscript query's completion trigger SHALL fire only when the parsed mismatch mode, timeline
dimensions, or `useCustomSettings` differ from the values the plugin-global cache already holds. That
trigger is `SetProperty('FlipX', GetProperty('FlipX'))`, which forces Resolve to re-render the current
frame.

Under a periodic refresh an unconditional trigger would force a redraw every TTL window and produce
visible preview flicker.

#### Scenario: Idle preview with an unchanged setting

- **WHEN** the preview is parked on a plugin-bearing clip and the Resolve setting is not touched for
  several TTL windows
- **THEN** the periodic queries return identical values, no forced re-render is triggered, and the
  preview does not flicker

#### Scenario: Cold start still renders once

- **WHEN** the first query of a session returns while the cache is empty
- **THEN** the value counts as changed and exactly one forced re-render is triggered

### Requirement: FillCrop write-back transposes to storage orientation

`compute_fillcrop_geometry_desqueezed` SHALL return a display-oriented crop rect. When applying it, the
plugin SHALL map that rect to storage orientation before writing any storage-oriented field. Under a
`video_rotation` of 90° or 270° the mapping SHALL transpose both the dimensions and the offset;
otherwise it SHALL be the identity.

Field orientations:

| field | orientation | value written |
|---|---|---|
| `params.output_size` | display | the returned crop dimensions |
| `params.size` | storage | the transposed crop dimensions |
| `lens.calib_dimension` | storage | the transposed crop dimensions |
| camera matrix `cx` / `cy` | storage | reduced by the transposed crop offset |

#### Scenario: Portrait anamorphic clip in a landscape timeline under scaleToCrop

- **WHEN** a clip stored 1920×1080 with `video_rotation=90` and a baked vertical stretch of 1.5 is placed
  in a 1920×1080 timeline set to `scaleToCrop`, and the computed display crop is `1620×608` at offset
  `(0, 656)`
- **THEN** `params.output_size` becomes `(1620, 608)`, while `params.size` and `calib_dimension` become
  `(608, 1620)` and the principal point is reduced by `(656, 0)`

#### Scenario: Unrotated clip is unaffected

- **WHEN** the same crop is applied to a clip with `video_rotation=0`
- **THEN** the storage-oriented values equal the display-oriented ones and the write-back is identical
  to the previous behavior

### Requirement: The periodic refresh publishes only host-input-sizing fields

The expiry-driven refresh SHALL update only `mismatch_mode`, `timeline_w`, `timeline_h`,
`use_custom_settings` and the query timestamp. It SHALL NOT publish clip-level fields
(`file_path`, `project_path`, `fps`, `frame_count`, dimensions, pixel aspect ratio) and SHALL NOT
raise the pending-file-info flag.

User-initiated queries (`LoadCurrent`, `ReloadProject`) SHALL keep publishing the full record and
raising the flag — adopting the newly resolved clip is the purpose of those actions.

The separation is load-bearing. The pending flag drives `check_pending_file_info`, which
unconditionally rewrites the `ProjectPath` parameter; `ProjectPath` is the first component of the
stabilization cache key and its change handler clears the stab cache regardless of whether the edit
was user-initiated. A refresh that raised the flag therefore forced a full project re-import every
TTL window. Additionally, the query resolves the clip under the **playhead**, not the clip of the
instance that armed it, so on a multi-clip timeline the rewritten path could belong to a different
clip and would be persisted into the project file.

#### Scenario: Periodic refresh does not re-import the project

- **WHEN** a plugin instance renders continuously for several TTL windows with the Resolve setting
  unchanged
- **THEN** no `ProjectPath` write, no stab-cache clear and no project re-import occur; the log shows
  no `new stab manager` / `[import_gyroflow]` entries attributable to the refresh

#### Scenario: Refresh does not adopt the playhead clip's project

- **WHEN** instance A (clip A) arms a refresh while the Resolve playhead is parked on clip B
- **THEN** instance A's `ProjectPath` is unchanged and it continues to stabilize with clip A's data

### Requirement: A wedged refresh query is reclaimed

The single-flight guard SHALL be reclaimable. When a refresh query has been in flight longer than a
fixed threshold well beyond any plausible query duration, the plugin SHALL release the guard, log a
warning, and permit a new query. The subprocess read has no deadline, so without reclamation a
fuscript that never returns would hold the guard for the remainder of the session and silently
disable the refresh.

#### Scenario: fuscript never returns

- **WHEN** a refresh query is armed and its subprocess never completes
- **THEN** after the threshold the guard is reclaimed with a warning and refresh resumes

### Requirement: Expiry kill-switch fully disables re-querying

When the TTL is configured to `0`, the plugin SHALL attempt at most one bootstrap query for the
process and SHALL NOT query again, regardless of whether that attempt succeeded. An empty cache
SHALL NOT keep arming queries.

#### Scenario: Kill-switch on a host where the query always fails

- **WHEN** the TTL is `0` and fuscript can never succeed (Resolve Free, scripting disabled)
- **THEN** exactly one attempt is made for the session, not one per pacing window

### Requirement: centerCrop uses a no-resizing crop model

`centerCrop` SHALL NOT share the `scaleToCrop` geometry. The plugin SHALL model it as Resolve
does — the source placed 1:1 at the timeline centre — so the visible source region is
`min(timeline, source)` on each axis, computed in physical (squeezed) pixels and scaled back to the
desqueezed space. When the source fits inside the timeline on both axes the plugin SHALL treat it as
no crop.

#### Scenario: Source larger than the timeline with matching aspect

- **WHEN** a 3840×2160 source is placed in a 1920×1080 timeline set to `centerCrop`
- **THEN** the visible region is 1920×1080 at offset (960, 540) — not "no crop", which is what the
  aspect-based model returned

#### Scenario: Source smaller than the timeline

- **WHEN** a 1280×720 source is placed in a 1920×1080 timeline set to `centerCrop`
- **THEN** no crop is applied and the stab is left untouched

### Requirement: Stretch write-back transposes to storage orientation

In `stretch` mode the plugin SHALL map the timeline dimensions to storage orientation before
assigning `params.size`, using the same display-to-storage mapping as the crop write-back.

#### Scenario: Portrait clip under stretch

- **WHEN** a clip with `video_rotation=90` is stretched into a 1920×1080 timeline
- **THEN** `params.size` becomes `(1080, 1920)`, not `(1920, 1080)`

## MODIFIED Requirements

### Requirement: Detect host input sizing mode via fuscript

The OpenFX plugin SHALL query the DaVinci Resolve scripting API via `fuscript` to read the host's
mismatched-resolution behavior setting (`timelineInputResMismatchBehavior`) and use it to drive
input-side handling. Every consumer of the mode — CreateInstance and the render path alike — SHALL
acquire it through the single freshness-checked path defined above. There SHALL NOT be a branch that
skips the freshness check based on `ProjectPath` being non-empty: a paste or `.drp`-restore instance
resolves from the shared cache when it is fresh and participates in the single-flight refresh when it is
not.

The plugin SHALL honor timeline-level override: when the current timeline's `useCustomSettings` is
`"1"`, the plugin SHALL read the setting from the timeline
(`tl:GetSetting('timelineInputResMismatchBehavior')`); otherwise it SHALL read from the project
(`proj:GetSetting('timelineInputResMismatchBehavior')`).

The single-key `GetSetting(key)` read is the correct read path and reflects the user's edit as soon as
the settings dialog is saved (verified live on Resolve 21.0.0.47 at both project and timeline level,
2026-07-26). The no-argument `GetSetting()` dump form SHALL NOT be used to read the timeline override:
on a timeline it returns the project-level value set, or once custom settings are enabled only the
overridden subset, and therefore cannot be searched for the effective value.

The plugin SHALL map the four observed string values to its internal `HostInputSizing` enum:

| fuscript value | `HostInputSizing` |
|---|---|
| `scaleToFit` | `Fit` |
| `scaleToCrop` | `FillCrop` |
| `centerCrop` | `CenterCrop` |
| `stretch` | `Stretch` |

#### Scenario: Restored instance participates in refresh

- **WHEN** a `.drp` restore creates an instance with a non-empty `ProjectPath` and the global cache is
  empty or stale
- **THEN** the instance participates in the single-flight refresh rather than skipping fuscript, and the
  queried value supersedes any persisted per-node value

### Requirement: CreateInstance reads the hidden field only when the global cache is empty

The per-node hidden `DetectedMismatchMode` param SHALL be a cold-start fallback only. It SHALL be
consulted when the global cache is empty and no fuscript query has yet returned in this session — the
genuine no-fuscript cases (Resolve Free, scripting disabled, compound clip). A returning query SHALL
always supersede the persisted value.

Resolution precedence, highest first: returning live query → fresh global cache → stale global cache
pending refresh → hidden per-node field → `Fit` fallback.

The param SHALL remain defined and secret so that paste and `.drp` round-trips continue to serialize it.

#### Scenario: Persisted stale value does not resurrect

- **WHEN** a node carries `DetectedMismatchMode = scaleToFit` persisted from an earlier session, the
  user changed the Resolve setting to `centerCrop` in the meantime, and the project is reopened
- **THEN** the hidden field supplies `Fit` only until the first query returns, after which `CenterCrop`
  applies; the persisted value never overrides the query result

## REMOVED Requirements

### Requirement: Fresh-drop invalidates stale plugin-global cache

**Reason**: superseded by the freshness window. The fresh-drop invalidation existed solely because the
cache had no expiry — dropping a new plugin node was the only user-reachable way to force a re-read.
With a TTL governing staleness the path is redundant, and keeping it would cost one extra fuscript
query per drop while adding a second, differently-shaped invalidation route to reason about.

**Migration**: the behavior it delivered (a changed Resolve setting reaching the plugin) is now covered
by "Plugin-global mismatch cache carries a freshness window" — within one TTL window, with no node
manipulation required. The `project_path_at_create.is_empty()` branch is deleted rather than kept as a
belt-and-braces trigger.
