#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
import argparse
import hashlib
import json
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from verify_finalcut_template import verify as verify_template


ROOT = Path(__file__).resolve().parents[1]
IDENTITY_PATH = ROOT / "finalcut" / "config" / "identity.json"
APP_NAME = json.loads(IDENTITY_PATH.read_text(encoding="utf-8"))["app_name"] + ".app"
XPC_NAME = "GyroflowNiYienFinalCutEffect.pluginkit"
APP_ENTITLEMENTS = {}
EFFECT_ENTITLEMENTS = {
    "com.apple.security.app-sandbox": True,
    "com.apple.security.files.bookmarks.app-scope": True,
    "com.apple.security.files.user-selected.read-only": True,
    "com.apple.security.temporary-exception.files.home-relative-path.read-only": [
        "/Library/Containers/com.apple.FinalCut/Data/Library/Preferences/com.apple.FinalCut.plist",
        "/Library/Containers/com.apple.FinalCutApp/Data/Library/Preferences/com.apple.FinalCutApp.plist",
    ],
    "com.apple.security.temporary-exception.shared-preference.read-only": [
        "com.apple.FinalCut",
        "com.apple.FinalCutApp",
    ],
}
LOCALIZATION_REGIONS = ("en", "zh-Hans", "zh-Hant", "ja", "ko", "ru")
FILE_ACCESS_USAGE_KEYS = (
    "NSDesktopFolderUsageDescription",
    "NSDocumentsFolderUsageDescription",
    "NSDownloadsFolderUsageDescription",
    "NSNetworkVolumesUsageDescription",
    "NSRemovableVolumesUsageDescription",
)


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


def png_dimensions(path: Path) -> tuple[int, int]:
    header = path.read_bytes()[:24]
    require(
        header[:8] == b"\x89PNG\r\n\x1a\n" and header[12:16] == b"IHDR",
        f"{path.name} is not a valid PNG preview",
    )
    return (
        int.from_bytes(header[16:20], "big"),
        int.from_bytes(header[20:24], "big"),
    )


def architectures(binary: Path) -> set[str]:
    return set(run(["xcrun", "lipo", "-archs", str(binary)]).split())


def verify_entitlement_policy(
    app_entitlements: dict[str, object],
    effect_entitlements: dict[str, object],
) -> None:
    require(
        app_entitlements == APP_ENTITLEMENTS,
        "wrapper App must not contain sandbox or file-access entitlements",
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
    if plist_start < 0:
        require(not output.strip(), f"invalid signed entitlements output for {bundle}")
        return {}
    entitlements = plistlib.loads(output[plist_start:])
    require(isinstance(entitlements, dict), f"signed entitlements are invalid for {bundle}")
    return entitlements


def verify_signing_identity(bundle: Path, team: str) -> None:
    details = subprocess.run(["codesign", "-dv", "--verbose=4", str(bundle)],
                             cwd=ROOT, capture_output=True, text=True)
    require(details.returncode == 0, f"Unable to read signing identity: {bundle.name}")
    lines = details.stderr.splitlines()
    require(f"TeamIdentifier={team}" in lines, f"Signing Team ID mismatch: {bundle.name}")
    require(any(line.startswith("Authority=Developer ID Application:") for line in lines),
            f"Developer ID Application required: {bundle.name}")
    require(any(line.startswith("CodeDirectory ") and re.search(r"flags=0x[0-9a-f]+\([^)]*\bruntime\b", line)
                for line in lines), f"Hardened Runtime is missing: {bundle.name}")


def verify_branding(app: Path, info: dict[str, object], identity: dict[str, str]) -> None:
    for key in ("CFBundleName", "CFBundleDisplayName"):
        require(info.get(key) == identity["app_name"], f"App {key} mismatch")
    executable = identity["app_executable"]
    require(
        executable not in ("", ".", "..") and "/" not in executable and "\\" not in executable,
        "App executable must be a filename",
    )
    require(info.get("CFBundleExecutable") == executable, "App executable mismatch")
    icon_name = identity["app_icon"]
    require(Path(icon_name).name == icon_name, "App icon must be a filename")
    require(info.get("CFBundleIconFile") == icon_name, "App icon declaration mismatch")
    icon = app / "Contents" / "Resources" / icon_name
    require(icon.is_file() and not icon.is_symlink(), "App icon is missing or linked")
    data = icon.read_bytes()
    require(len(data) >= 8 and data[:4] == b"icns", "App icon is not ICNS")
    require(int.from_bytes(data[4:8], "big") == len(data), "App icon length is invalid")
    require(hashlib.sha256(data).hexdigest() == identity["app_icon_sha256"], "App icon hash mismatch")


def verify_translation_content(app_resources: Path, effect_resources: Path) -> None:
    catalog = json.loads((ROOT / "finalcut/Xcode/Shared/Localizable.xcstrings").read_text())
    for resources in (app_resources, effect_resources):
        for region in LOCALIZATION_REGIONS:
            strings = json.loads(run([
                "plutil", "-convert", "json", "-o", "-",
                str(resources / f"{region}.lproj" / "Localizable.strings"),
            ]))
            for key, entry in catalog["strings"].items():
                localization = entry["localizations"][region]
                unit = localization.get("stringUnit")
                if unit is not None:
                    require(strings.get(key) == unit["value"], f"translation mismatch: {region}/{key}")
            plurals = json.loads(run([
                "plutil", "-convert", "json", "-o", "-",
                str(resources / f"{region}.lproj" / "Localizable.stringsdict"),
            ]))
            for key, entry in catalog["strings"].items():
                forms = entry["localizations"][region].get("variations", {}).get("plural")
                if forms:
                    require(key in plurals, f"plural dictionary missing: {region}/{key}")
                    actual = plurals[key]
                    variables = [v for v in actual.values() if isinstance(v, dict)]
                    require(len(variables) == 1, f"invalid plural dictionary: {region}/{key}")
                    for category, form in forms.items():
                        require(variables[0].get(category) == form["stringUnit"]["value"],
                                f"plural mismatch: {region}/{key}/{category}")
    for region in LOCALIZATION_REGIONS:
        privacy = json.loads(run([
            "plutil", "-convert", "json", "-o", "-",
            str(app_resources / f"{region}.lproj" / "InfoPlist.strings"),
        ]))
        require(all(privacy.get(key) for key in FILE_ACCESS_USAGE_KEYS),
                f"privacy translations incomplete: {region}")


def verify_localizations(app_resources: Path, effect_resources: Path) -> None:
    for bundle_name, resources in (
        ("wrapper App", app_resources),
        ("FxPlug XPC", effect_resources),
    ):
        for region in LOCALIZATION_REGIONS:
            strings = resources / f"{region}.lproj" / "Localizable.strings"
            require(
                strings.is_file(),
                f"{bundle_name} localization is missing for {region}: {strings}",
            )
        if bundle_name == "wrapper App":
            for region in LOCALIZATION_REGIONS:
                info_strings = resources / f"{region}.lproj" / "InfoPlist.strings"
                require(
                    info_strings.is_file(),
                    f"wrapper App privacy localization is missing for {region}: "
                    f"{info_strings}",
                )


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
    verify_branding(app, app_info, identity)
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
    require(
        all(isinstance(app_info.get(key), str) and app_info[key] for key in FILE_ACCESS_USAGE_KEYS),
        "wrapper App file-access usage descriptions are incomplete",
    )
    require(
        "CFBundleDocumentTypes" not in app_info and "UTImportedTypeDeclarations" not in app_info,
        "wrapper App must not claim the .gyroflow document type",
    )
    require(not list(app.rglob("*.appex")), "Workflow extension is forbidden")
    with (ROOT / "finalcut" / "Xcode" / "App" / "App.entitlements").open(
        "rb"
    ) as source:
        source_app_entitlements = plistlib.load(source)
    with (ROOT / "finalcut" / "Xcode" / "Effect" / "Effect.entitlements").open(
        "rb"
    ) as source:
        source_effect_entitlements = plistlib.load(source)
    verify_entitlement_policy(source_app_entitlements, source_effect_entitlements)
    verify_localizations(
        contents / "Resources",
        xpc_contents / "Resources",
    )
    verify_translation_content(contents / "Resources", xpc_contents / "Resources")

    app_binary = contents / "MacOS" / identity["app_executable"]
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
    large_preview = template_root / "large.png"
    small_preview = template_root / "small.png"
    require(large_preview.is_file(), "large preview is missing")
    require(small_preview.is_file(), "small preview is missing")
    require(
        png_dimensions(large_preview) == (640, 360),
        "large preview must be 640x360",
    )
    require(
        png_dimensions(small_preview) == (192, 108),
        "small preview must be 192x108",
    )
    require((template_root / "UPSTREAM.txt").is_file(), "MIT attribution is missing")
    verify_template(template, xpc_info_path, IDENTITY_PATH)

    if expect_signed:
        run(["codesign", "--verify", "--deep", "--strict", "--verbose=2", str(app)])
        verify_entitlement_policy(
            signed_entitlements(app),
            signed_entitlements(xpc),
        )
        for bundle in [
            xpc_contents / "Frameworks" / "FxPlug.framework",
            xpc_contents / "Frameworks" / "PluginManager.framework",
            xpc,
            app,
        ]:
            run(["codesign", "--verify", "--strict", "--verbose=2", str(bundle)])
            verify_signing_identity(bundle, identity["team_id"])
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
