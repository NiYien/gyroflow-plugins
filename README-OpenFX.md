<span class="badge-patreon"><a href="https://www.patreon.com/smartislav" title="Donate to this project using Patreon"><img src="https://img.shields.io/badge/patreon-donate-yellow.svg" alt="Patreon donate button" /></a></span>
![example workflow](https://github.com/NiYien/gyroflow-plugins/actions/workflows/release.yml/badge.svg)

# Gyroflow(Niyien) OpenFX plugin

* Works with stabilization data exported with [gyroflow](http://gyroflow.xyz/)
* Allows you to apply the stabilization right in your OpenFX-capable video editor

# Installation

Grab the archive for your OS from the [releases page](https://github.com/NiYien/gyroflow-plugins/releases).

## Linux

    mkdir -p /usr/OFX/Plugins
    cd /usr/OFX/Plugins
    sudo unzip ${PATH_TO}/GyroflowNiyien-OpenFX-linux.zip

## MacOS

Copy the `GyroflowNiyien.ofx.bundle` from the archive into the `/Library/OFX/Plugins` directory.
Create the directory if it doesn't exist yet.
Then in Resolve, make sure to go to Preferences -> Video plugins and enable GyroflowNiyien.ofx.bundle.

## Windows

Copy the `GyroflowNiyien.ofx.bundle` from the archive into the `C:\Program Files\Common Files\OFX\Plugins` folder.
Create the folder if it doesn't exist yet.

## For more detailed instructions, see the [docs](https://docs.gyroflow.xyz/app/video-editor-plugins/davinci-resolve-openfx#installation)

# Usage

### Export `.gyroflow` file in the Gyroflow app

Click the `Export project file (including gyro data)` in the Gyroflow app. You can also use `Ctrl+S` or `Command+S` shortcut

### Basic plugin usage

First you need to apply the plugin to the clip.
In DaVinci Resolve you can do that by going to the Fusion tab and inserting the "Warp -> Gyroflow(Niyien)" after the media input node.
You can also apply the plugin on the Edit or Color page - it should work faster this way.

### Load the .gyroflow file

In DaVinci Resolve, go to the `Gyroflow(Niyien)` plugin settings. Select the `.gyroflow` file in the `Project file` entry.
If your video file is from GoPro 8+, DJI or Insta360, you can also select video file directly. If it's from Sony or it's BRAW - you can also select the video file directly, but you need to load lens profile or preset after that.

## For more detailed instructions, see the [docs](https://docs.gyroflow.xyz/app/video-editor-plugins/general-plugin-workflow)

### Host input sizing (DaVinci Resolve mismatched-resolution)

When the source clip's resolution does not match the timeline resolution, DaVinci Resolve scales the clip into the timeline buffer per the **Project Settings → Image Scaling → Mismatched Resolution Files** option. The OpenFX plugin needs to know which mode is active so the stabilization math is computed against the correct source pixels.

The `Host input sizing` dropdown (in the Adjust group) controls this:

- **Auto (fuscript)** — *default*. The plugin reads Resolve's `timelineInputResMismatchBehavior` setting via the `fuscript` scripting bridge and picks the matching mode automatically. Requires **DaVinci Resolve Studio** with `Preferences → General → External scripting using` set to `Local`. On the free edition / when scripting is disabled / on compound clips, the plugin silently falls back to `Fit`.
- **Fit** — assume Resolve letterboxed the clip into the timeline buffer (centered content band with black bars). This is the only legacy path; matches the plugin's pre-v2.2 behavior.
- **Fill+Crop** — assume Resolve 1:1 center-cropped the source pixels to fill the timeline buffer. The plugin offsets the lens principal point (`cx`/`cy`) and trims the calibration dimensions to the crop region; `fx`/`fy`/distortion are unchanged.
- **Center Crop** — same math as Fill+Crop for the common case (timeline ≤ source); kept as a distinct option for forward-compat with Resolve's `centerCrop` mode.
- **Stretch** — Resolve non-uniformly scaled the source to fit the timeline buffer. The plugin accepts the aspect distortion and logs a one-time warning; recommend switching Resolve to `scaleToFit` / `scaleToCrop` for accurate stabilization.

The Fusion page is always treated as `Fit` (it receives native-resolution clips). `Don't draw outside source clip` takes precedence over this dropdown.

**Runtime caveat**: changing Resolve's mismatched-resolution setting while the project is open requires pressing `Reload project` for the plugin to pick up the new value.


# License

This software is licensed under GNU General Public License version 3 ([LICENSE](LICENSE))

# Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the GNU General Public License version 3, shall be
licensed as above, without any additional terms or conditions.
