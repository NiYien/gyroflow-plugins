#!/usr/bin/env python3
import argparse
import json
import plistlib
import subprocess
import sys
from pathlib import Path


DEFAULT_SDK_ROOT = Path("/Library/Developer/SDKs/FxPlug.sdk")
DEFAULT_DEPLOYMENT_TARGET = "13.0"
REQUIRED_PATHS = (
    Path("Library/Frameworks/FxPlug.framework"),
    Path("Library/Frameworks/FxPlug.framework/Headers/FxPlugSDK.h"),
    Path("Library/Frameworks/PluginManager.framework"),
    Path("Library/Frameworks/PluginManager.framework/Headers/PluginManager.h"),
)


class PreflightError(RuntimeError):
    pass


def version_tuple(value: str) -> tuple[int, ...]:
    parts = value.strip().split(".")
    if not parts or any(not part.isdigit() for part in parts):
        raise PreflightError(f"invalid version: {value!r}")
    return tuple(int(part) for part in parts)


def detected_xcode_version() -> str:
    result = subprocess.run(
        ["xcodebuild", "-version"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise PreflightError(
            "unable to run xcodebuild -version; install and select Xcode 15 or newer"
        )
    first_line = result.stdout.splitlines()[0] if result.stdout else ""
    prefix = "Xcode "
    if not first_line.startswith(prefix):
        raise PreflightError(f"unexpected xcodebuild version output: {first_line!r}")
    return first_line[len(prefix) :].strip()


def framework_deployment_floor(sdk_root: Path) -> str:
    floors: list[str] = []
    for framework_name in ("FxPlug", "PluginManager"):
        tbd_path = (
            sdk_root
            / "Library"
            / "Frameworks"
            / f"{framework_name}.framework"
            / f"{framework_name}.tbd"
        )
        try:
            tbd = json.loads(tbd_path.read_text(encoding="utf-8"))
            target_info = tbd["main_library"]["target_info"]
        except (OSError, KeyError, TypeError, json.JSONDecodeError) as error:
            raise PreflightError(
                f"{framework_name} deployment metadata is invalid: {tbd_path}: {error}"
            ) from error
        macos_floors = [
            str(target["min_deployment"])
            for target in target_info
            if str(target.get("target", "")).endswith("-macos")
        ]
        if not macos_floors:
            raise PreflightError(
                f"{framework_name} deployment metadata has no macOS targets: {tbd_path}"
            )
        floors.extend(macos_floors)
    return max(floors, key=version_tuple)


def validate_sdk(
    sdk_root: Path,
    xcode_version: str,
    deployment_target: str,
) -> dict[str, str]:
    if not sdk_root.is_dir():
        raise PreflightError(f"FxPlug SDK not found at {sdk_root}")

    settings_path = sdk_root / "SDKSettings.plist"
    if not settings_path.is_file():
        raise PreflightError(f"FxPlug SDK metadata is missing: {settings_path}")
    try:
        with settings_path.open("rb") as settings_file:
            settings = plistlib.load(settings_file)
    except (OSError, plistlib.InvalidFileException) as error:
        raise PreflightError(f"FxPlug SDK metadata is invalid: {error}") from error

    if settings.get("CanonicalName") != "FxPlugSDK":
        raise PreflightError(
            f"unexpected SDK canonical name: {settings.get('CanonicalName')!r}"
        )

    for required_path in REQUIRED_PATHS:
        candidate = sdk_root / required_path
        if not candidate.exists():
            raise PreflightError(f"required FxPlug SDK component is missing: {candidate}")

    minimum_tools = str(settings.get("MinimumSupportedToolsVersion", "15.0"))
    if version_tuple(xcode_version) < version_tuple(minimum_tools):
        raise PreflightError(
            f"FxPlug SDK requires Xcode {minimum_tools} or newer; found {xcode_version}"
        )
    minimum_macos = framework_deployment_floor(sdk_root)
    if version_tuple(deployment_target) < version_tuple(minimum_macos):
        raise PreflightError(
            f"installed FxPlug frameworks require macOS {minimum_macos} or newer; "
            f"configured deployment target is {deployment_target}"
        )

    return {
        "canonical_name": "FxPlugSDK",
        "sdk_root": str(sdk_root.resolve()),
        "sdk_version": str(settings.get("Version", "unknown")),
        "minimum_tools_version": minimum_tools,
        "minimum_macos_deployment": minimum_macos,
        "configured_deployment_target": deployment_target,
        "xcode_version": xcode_version,
    }


def write_stamp(stamp: Path, payload: dict[str, str]) -> None:
    stamp.parent.mkdir(parents=True, exist_ok=True)
    temporary = stamp.with_name(f".{stamp.name}.tmp")
    temporary.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    temporary.replace(stamp)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Fail-fast preflight for the Final Cut FxPlug production build."
    )
    parser.add_argument("--sdk-root", type=Path, default=DEFAULT_SDK_ROOT)
    parser.add_argument("--xcode-version")
    parser.add_argument(
        "--deployment-target",
        default=DEFAULT_DEPLOYMENT_TARGET,
    )
    parser.add_argument("--stamp", type=Path)
    args = parser.parse_args()

    try:
        xcode_version = args.xcode_version or detected_xcode_version()
        payload = validate_sdk(
            args.sdk_root,
            xcode_version,
            args.deployment_target,
        )
        if args.stamp is not None:
            write_stamp(args.stamp, payload)
    except PreflightError as error:
        print(f"Final Cut SDK preflight failed: {error}", file=sys.stderr)
        return 2

    print(
        "Final Cut SDK preflight passed: "
        f"{payload['sdk_root']} (Xcode {payload['xcode_version']})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
