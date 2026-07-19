# OFX Host PAR-Composited Band Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix Resolve Edit/Color `Fit` cropping when an anamorphic source buffer already contains clip-PAR composition, while preserving source-native physical matches, preview/subscale, and unrelated paths.

**Architecture:** Keep the existing logical and physical aspect calculations, then pass them through a pure three-space selector. A physical-aspect main buffer is source-native and stays `Physical`; otherwise a main Resolve Edit/Color `Fit` render defaults to `HostParComposited` under the user-applied-PAR contract. Preview/subscale and unrelated paths stay `Physical`. The selector returns data only and does not mutate plugin/global/stabilization state.

**Tech Stack:** Rust, OpenFX, DaVinci Resolve fuscript metadata, OpenSpec, Cargo unit tests.

## Global Constraints

- No new UI or persisted OFX parameter.
- No changes to gyroflow-core, Adobe, frei0r, Fusion, FillCrop, CenterCrop, Stretch, `DontDrawOutside`, Vegas, or global-cache semantics.
- Source-native physical-match, preview/subscale, and non-Resolve buffers must preserve `6b8cb14` physical behavior.
- Users are responsible for applying the anamorphic PAR corresponding to the loaded `.gyroflow` raw lens stretch in Resolve.
- New and modified code comments must be English.
- Use TDD: observe the focused regression test fail before production code is changed.

---

### Task 1: Pure aspect-space selector

**Files:**
- Modify: `openfx/src/gyroflow.rs`
- Test: `openfx/src/gyroflow.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: host name, logical/physical `(org_ratio, output_aspect)`, raw stretch, Fit/Fusion/DontDrawOutside gates, preview/subscale state, and actual source-buffer size.
- Produces: `BandAspectSelection { space: BandAspectSpace, org_ratio: f64, output_aspect: f64 }`.

- [x] **Step 1: Write the failing PAR-composited regression test**

Simulate the deployed restored instance: host `DaVinciResolve`, no fuscript evidence, OFX PAR `1.0`, physical aspect `0.5625`, logical aspect `0.9`, and main source buffer `1920×1080`. Assert `HostParComposited` and selected ratios `0.9`.

- [x] **Step 2: Write conservative gate tests**

Cover source-native physical match, preview/subscale, Fusion, non-Fit, `DontDrawOutside`, non-anamorphic, Vegas/other host, and kill-switch legacy selection.

- [x] **Step 3: Run the focused test and observe RED**

Run: `cargo test -p gyroflow-ofx host_par_composited -- --nocapture`

Observed: compile failure `E0061` because the D7 selector does not yet accept the preview/subscale gate required by D8.

- [x] **Step 4: Implement the minimum pure selector**

Use 1% relative agreement to give a source-buffer/physical match precedence. Require logical-vs-physical separation greater than `0.1`, accept observed host `DaVinciResolve` plus canonical alias, and return `Physical` on every failed/preview gate. A remaining main Resolve Fit anamorphic buffer selects logical.

- [x] **Step 5: Run focused and existing geometry tests**

Run: `cargo test -p gyroflow-ofx band_ -- --nocapture`

Expected: all band selector and existing physical-band tests pass.

### Task 2: Render-path wiring and diagnostics

**Files:**
- Modify: `openfx/src/gyroflow.rs`
- Test: `openfx/src/gyroflow.rs`

**Interfaces:**
- Consumes: Task 1 selector, existing host name, and the actual source image dimensions.
- Produces: selected `org_ratio` and `output_aspect` used by the existing `src_rect`, output aspect-fit, and proxy branches.

- [x] **Step 1: Preserve both logical and physical calculations**

Read lens live/raw stretch regardless of the kill-switch, calculate both pairs before releasing `stab.params`, and leave `lens_baked_stretch`/FillCrop behavior untouched.

- [x] **Step 2: Classify after the source image is available**

Pass the Resolve host gate, existing preview/subscale detection, and actual `src_size` to the selector. Do not read clip fields, add fields to `HostInputSizingCacheEntry`, or add persisted parameters.

- [x] **Step 3: Select once and reuse both ratios**

Feed the selected input ratio to `get_center_rect(src_size, org_ratio)` and the selected output ratio to the existing output/proxy aspect-fit code. Do not alter their gates or rect math.

- [x] **Step 4: Replace the anamorphic diagnostic line**

Log selected space, logical/physical/selected ratios, source-buffer dimensions, diagnostic OFX clip/image PAR, and existing stretch/rotation values. Remain silent for non-anamorphic sources.

- [x] **Step 5: Run full OpenFX unit tests**

Run: `cargo test -p gyroflow-ofx`

Expected: zero failures.

### Task 3: Static and live verification

**Files:**
- Modify after verification: `openspec/changes/ofx-anamorphic-band-guess/tasks.md`

**Interfaces:**
- Consumes: completed implementation.
- Produces: verified release plugin and completed OpenSpec checklist.

- [x] **Step 1: Check formatting and compilation**

Run: `cargo fmt --all -- --check`

Run: `cargo check -p gyroflow-ofx`

Observed: `cargo check -p gyroflow-ofx` and scoped `git diff --check` exit 0. The repository-wide `cargo fmt --all -- --check` still reports the documented pre-existing formatting baseline; no formatting command was applied.

- [x] **Step 2: Build release OFX**

Run the repository's established OFX release command and confirm exit 0.

Observed: release build exit 0; output is 25,503,744 bytes with SHA-256 `4492AF63B886E97C96AC79BD30C0965C5A4B653932790DD75E3806292D984BFE`, identical to the deployed bundle.

- [x] **Step 3: Deploy and live-test the original clip**

With `GYROFLOW_OFX_ANAMORPHIC_BAND` unset, reopen `New Project 1 / CDC_1155.MOV`. Confirm the restored path logs host `DaVinciResolve`, main buffer `1920×1080`, selected logical `0.9`, and `HostParComposited`; the resulting 972×1080 content band must match the successful kill-switch A/B framing.

Observed: main render selected `HostParComposited` with `buffer=(1920, 1080)` and `selected=(0.9000, 0.9000)`; user confirmed the side bars were gone.

- [x] **Step 4: Regression spot-check**

Confirm a source-native squeezed anamorphic Fit clip matching physical stays `Physical`. Unit gates prove non-anamorphic, Fusion, FillCrop/CenterCrop/Stretch, `DontDrawOutside`, Vegas/other-host, thumbnail/proxy, and kill-switch behavior stay unchanged.

Observed: fresh OpenFX suite 41/41. Live source-native and kill-switch checks remain valid; D8 preview/subscale probes stayed `Physical`. After a normal Resolve restart, `DSC_3172.MP4` read `scaleToCrop` and its main 1920x1920 render used full-buffer `in_rect=None` / `out_rect=None`; the user confirmed no black bars.

- [x] **Step 5: Update OpenSpec tasks and inspect diff**

Run: `git diff --check`

Run: `git status --short`

Expected: only the scoped OFX source/test, OpenSpec artifacts, and this plan are changed.

Execution choice: inline execution in this session, authorized by the user on 2026-07-19.
