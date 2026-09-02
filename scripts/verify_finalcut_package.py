#!/usr/bin/env python3
import argparse
import json
import plistlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from verify_finalcut_template import verify as verify_template


ROOT = Path(__file__).resolve().parents[1]
IDENTITY_PATH = ROOT / "finalcut" / "config" / "identity.json"
APP_NAME = "GyroflowNiYien Final Cut.app"
XPC_NAME = "GyroflowNiYienFinalCutEffect.pluginkit"
EFFECT_ENTITLEMENTS = {
    "com.apple.security.app-sandbox": True,
    "com.apple.security.files.bookmarks.app-scope": True,
    "com.apple.security.files.user-selected.read-only": True,
}


def run(command: list[str]) -> str:
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        raise ValueError(
            f"command failed ({result.returncode}): {' '.join(command)}\n"
            f"{result.stdout}{result.stderr}"
        )
    return result.stdout.strip()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def architectures(binary: Path) -> set[str]:
    return set(run(["xcrun", "lipo", "-archs", str(binary)]).split())


def verify_entitlement_policy(
    app_entitlements: dict[str, object],
    effect_entitlements: dict[str, object],
) -> None:
    require(
        app_entitlements == {},
        "wrapper App must have no sandbox or high-risk entitlements",
    )
    require(
        effect_entitlements == EFFECT_ENTITLEMENTS,
        "FxPlug XPC entitlements do not match the sandboxed read-only policy",
    )


def signed_entitlements(bundle: Path) -> dict[str, object]:
    result = subprocess.run(
        ["codesign", "-d", "--entitlements", ":-", str(bundle)],
        cwd=ROOT,
        capture_output=True,
    )
    if result.returncode != 0:
        raise ValueError(
            f"unable to read signed entitlements for {bundle}: "
            + result.stderr.decode("utf-8", errors="replace")
        )
    output = result.stdout
    plist_start = output.find(b"<?xml")
    if plist_start < 0:
        plist_start = output.find(b"<plist")
    require(plist_start >= 0, f"signed entitlements plist is missing for {bundle}")
    entitlements = plistlib.loads(output[plist_start:])
    require(isinstance(entitlements, dict), f"signed entitlements are invalid for {bundle}")
    return entitlements


def verify_app(app: Path, expect_signed: bool, expect_notarized: bool) -> None:
    identity = json.loads(IDENTITY_PATH.read_text(encoding="utf-8"))
    require(app.name == APP_NAME and app.is_dir(), f"expected {APP_NAME}")
    contents = app / "Contents"
    app_info_path = contents / "Info.plist"
    xpc = contents / "PlugIns" / XPC_NAME
    xpc_contents = xpc / "Contents"
    xpc_info_path = xpc_contents / "Info.plist"
    with app_info_path.open("rb") as source:
        app_info = plistlib.load(source)
    with xpc_info_path.open("rb") as source:
        xpc_info = plistlib.load(source)
    require(
        app_info["CFBundleIdentifier"] == identity["app_bundle_id"],
        "App bundle identifier mismatch",
    )
    require(
        xpc_info["CFBundleIdentifier"] == identity["effect_bundle_id"],
        "XPC bundle identifier mismatch",
    )
    require(
        app_info["CFBundleShortVersionString"] == identity["marketing_version"],
        "App version mismatch",
    )
    require(
        xpc_info["CFBundleShortVersionString"] == identity["marketing_version"],
        "XPC version mismatch",
    )
    require(
        app_info["CFBundleVersion"] == identity["build_version"],
        "App build version mismatch",
    )
    require(
        xpc_info["CFBundleVersion"] == identity["build_version"],
        "XPC build version mismatch",
    )
    require(not list(app.rglob("*.appex")), "Workflow extension is forbidden")

    app_binary = contents / "MacOS" / "GyroflowNiYien Final Cut"
    xpc_binary = xpc_contents / "MacOS" / "GyroflowNiYienFinalCutEffect"
    framework_binaries = [
        xpc_contents / "Frameworks" / "FxPlug.framework" / "Versions" / "A" / "FxPlug",
        xpc_contents
        / "Frameworks"
        / "PluginManager.framework"
        / "Versions"
        / "B"
        / "PluginManager",
    ]
    for binary in [app_binary, xpc_binary, *framework_binaries]:
        require(binary.is_file(), f"required binary is missing: {binary}")
        require(
            architectures(binary) == {"arm64", "x86_64"},
            f"binary is not universal: {binary}",
        )

    template_root = (
        contents
        / "Resources"
        / "Motion Templates"
        / "Effects.localized"
        / "NiYien"
        / "Gyroflow"
    )
    template = template_root / "Gyroflow NiYien.moef"
    require((template_root / "large.png").is_file(), "large preview is missing")
    require((template_root / "small.png").is_file(), "small preview is missing")
    require((template_root / "UPSTREAM.md").is_file(), "MIT attribution is missing")
    verify_template(template, xpc_info_path, IDENTITY_PATH)

    if expect_signed:
        run(["codesign", "--verify", "--deep", "--strict", "--verbose=2", str(app)])
        verify_entitlement_policy(
            signed_entitlements(app),
            signed_entitlements(xpc),
        )
        app_details = subprocess.run(
            ["codesign", "-dv", "--verbose=4", str(app)],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        require(
            f"TeamIdentifier={identity['team_id']}" in app_details.stderr,
            "App signing Team ID mismatch",
        )
        for bundle in [
            xpc_contents / "Frameworks" / "FxPlug.framework",
            xpc_contents / "Frameworks" / "PluginManager.framework",
            xpc,
            app,
        ]:
            run(["codesign", "--verify", "--strict", "--verbose=2", str(bundle)])
    if expect_notarized:
        run(["spctl", "--assess", "--type", "execute", "--verbose=4", str(app)])
        run(["xcrun", "stapler", "validate", str(app)])


def verify_zip(zip_path: Path, expect_signed: bool, expect_notarized: bool) -> None:
    with tempfile.TemporaryDirectory(prefix="finalcut-zip-verify-") as directory:
        destination = Path(directory)
        run(["ditto", "-x", "-k", str(zip_path), str(destination)])
        top_level = list(destination.iterdir())
        require(
            len(top_level) == 1 and top_level[0].name == APP_NAME,
            "zip must contain the App directly without an extra wrapper directory",
        )
        verify_app(top_level[0], expect_signed, expect_notarized)


def main() -> None:
    parser = argparse.ArgumentParser()
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument("--app", type=Path)
    target.add_argument("--zip", dest="zip_path", type=Path)
    parser.add_argument("--expect-signed", action="store_true")
    parser.add_argument("--expect-notarized", action="store_true")
    arguments = parser.parse_args()
    try:
        if arguments.app is not None:
            verify_app(
                arguments.app,
                arguments.expect_signed,
                arguments.expect_notarized,
            )
        else:
            verify_zip(
                arguments.zip_path,
                arguments.expect_signed,
                arguments.expect_notarized,
            )
    except (KeyError, OSError, ValueError) as error:
        print(f"Final Cut package verification failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error


if __name__ == "__main__":
    main()
