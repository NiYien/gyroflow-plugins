#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
WORKSPACE = ROOT / "finalcut" / "GyroflowFinalCut.xcworkspace"
RUST_BUILDER = ROOT / "scripts" / "build_finalcut_rust.py"
SDK_PREFLIGHT = ROOT / "scripts" / "check_finalcut_sdk.py"
TEMPLATE_ASSEMBLER = ROOT / "scripts" / "assemble_finalcut_template_resources.py"
PACKAGE_VERIFIER = ROOT / "scripts" / "verify_finalcut_package.py"
LOCALIZATION_GENERATOR = ROOT / "scripts" / "generate_finalcut_localizations.py"
CAPACITY_GATE = ROOT / "finalcut" / "config" / "capacity-gate.json"
GEOMETRY_SUPPORT = ROOT / "finalcut" / "validation" / "geometry-support.json"
IDENTITY_PATH = ROOT / "finalcut" / "config" / "identity.json"
APP_NAME = json.loads(IDENTITY_PATH.read_text(encoding="utf-8"))["app_name"] + ".app"
ZIP_NAME = "GyroflowNiyien-FinalCut-macos.zip"
XPC_NAME = "GyroflowNiYienFinalCutEffect.pluginkit"
DEFAULT_RUNTIME_FRAMEWORKS = Path("/Library/Developer/Frameworks")
RUNTIME_FRAMEWORK_NAMES = ("FxPlug.framework", "PluginManager.framework")


def run(command: list[str], environment: dict[str, str] | None = None) -> str:
    result = subprocess.run(
        command,
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"command failed ({result.returncode}): {' '.join(command)}\n"
            f"{result.stdout}{result.stderr}"
        )
    return result.stdout.strip()


def require_capacity_gate(allow_unvalidated: bool, path: Path | None = None) -> None:
    gate = json.loads((path or CAPACITY_GATE).read_text(encoding="utf-8"))
    if (
        gate.get("release_blocked", True)
        or not gate.get("host_parameter_round_trip_validated", False)
    ) and not allow_unvalidated:
        raise RuntimeError(
            "Final Cut release is blocked until the representative large-project "
            "custom-parameter save/reopen gate passes"
        )
    if gate.get("fallback_path_allowed") is not False:
        raise RuntimeError("capacity gate must forbid path fallback")


def require_geometry_gate(allow_unvalidated: bool, path: Path | None = None) -> None:
    support = json.loads((path or GEOMETRY_SUPPORT).read_text(encoding="utf-8"))
    verified = support.get("verified_supported_entry_ids", [])
    if (support.get("release_blocked", True) or not verified) and not allow_unvalidated:
        raise RuntimeError(
            "Final Cut release is blocked until the real-host geometry manifest "
            "contains pixel-verified supported entries"
        )


def copy_runtime_frameworks(runtime_root: Path, xpc_contents: Path) -> list[Path]:
    destination = xpc_contents / "Frameworks"
    destination.mkdir(parents=True, exist_ok=True)
    copied: list[Path] = []
    for framework_name in RUNTIME_FRAMEWORK_NAMES:
        source = runtime_root / framework_name
        if not source.is_dir():
            raise FileNotFoundError(f"runtime framework is missing: {source}")
        target = destination / source.name
        run(["ditto", str(source), str(target)])
        copied.append(target)
    return copied


def sign_bundle(
    app: Path,
    frameworks: list[Path],
    identity: str,
) -> None:
    xpc = app / "Contents" / "PlugIns" / XPC_NAME
    for framework in frameworks:
        run(
            [
                "codesign",
                "--force",
                "--sign",
                identity,
                "--timestamp",
                "--options",
                "runtime",
                str(framework),
            ]
        )
    run(
        [
            "codesign",
            "--force",
            "--sign",
            identity,
            "--timestamp",
            "--options",
            "runtime",
            "--entitlements",
            str(ROOT / "finalcut" / "xcode" / "Effect" / "Effect.entitlements"),
            str(xpc),
        ]
    )
    run(
        [
            "codesign",
            "--force",
            "--sign",
            identity,
            "--timestamp",
            "--options",
            "runtime",
            "--entitlements",
            str(ROOT / "finalcut" / "xcode" / "App" / "App.entitlements"),
            str(app),
        ]
    )


def build(arguments: argparse.Namespace) -> tuple[Path, Path]:
    require_capacity_gate(arguments.allow_unvalidated_capacity_for_testing)
    require_geometry_gate(arguments.allow_unvalidated_geometry_for_testing)
    if not arguments.unsigned_for_testing and not arguments.signing_identity:
        raise RuntimeError("production package requires --signing-identity")
    output_dir = arguments.output_dir.resolve()
    output_app = output_dir / APP_NAME
    output_zip = output_dir / ZIP_NAME
    if output_app.exists() or output_zip.exists():
        raise FileExistsError("refusing to replace an existing Final Cut package")
    output_dir.mkdir(parents=True, exist_ok=True)

    run([sys.executable, str(LOCALIZATION_GENERATOR)])
    run([sys.executable, str(SDK_PREFLIGHT), "--deployment-target", "13.0"])
    run([sys.executable, str(RUST_BUILDER)])
    with tempfile.TemporaryDirectory(
        prefix="finalcut-package-",
        dir=output_dir,
    ) as directory:
        staging_root = Path(directory)
        archive_path = staging_root / "GyroflowFinalCut.xcarchive"
        run(
            [
                "xcodebuild",
                "-quiet",
                "-workspace",
                str(WORKSPACE),
                "-scheme",
                "GyroflowFinalCutApp",
                "-configuration",
                "Release",
                "-destination",
                "generic/platform=macOS",
                "-archivePath",
                str(archive_path),
                "CODE_SIGNING_ALLOWED=NO",
                "ONLY_ACTIVE_ARCH=NO",
                "ARCHS=arm64 x86_64",
                "SWIFT_STRICT_CONCURRENCY=complete",
                "SWIFT_TREAT_WARNINGS_AS_ERRORS=YES",
                "GCC_TREAT_WARNINGS_AS_ERRORS=YES",
                "archive",
            ]
        )
        built_app = archive_path / "Products" / "Applications" / APP_NAME
        if not built_app.is_dir():
            raise RuntimeError(f"Xcode did not produce {built_app}")
        staged_app = staging_root / APP_NAME
        run(["ditto", str(built_app), str(staged_app)])
        xpc_contents = (
            staged_app
            / "Contents"
            / "PlugIns"
            / XPC_NAME
            / "Contents"
        )
        frameworks = copy_runtime_frameworks(arguments.runtime_frameworks, xpc_contents)
        template_destination = (
            staged_app
            / "Contents"
            / "Resources"
            / "Motion Templates"
            / "Effects.localized"
            / "NiYien"
            / "Gyroflow"
        )
        run(
            [
                sys.executable,
                str(TEMPLATE_ASSEMBLER),
                "--output",
                str(template_destination),
            ]
        )
        if not arguments.unsigned_for_testing:
            sign_bundle(staged_app, frameworks, arguments.signing_identity)
        verifier = [
            sys.executable,
            str(PACKAGE_VERIFIER),
            "--app",
            str(staged_app),
        ]
        if not arguments.unsigned_for_testing:
            verifier.append("--expect-signed")
        run(verifier)
        staged_zip = staging_root / ZIP_NAME
        run(["ditto", "-c", "-k", "--keepParent", str(staged_app), str(staged_zip)])
        run(
            [
                sys.executable,
                str(PACKAGE_VERIFIER),
                "--zip",
                str(staged_zip),
                *(["--expect-signed"] if not arguments.unsigned_for_testing else []),
            ]
        )
        staged_app.rename(output_app)
        staged_zip.rename(output_zip)
    return output_app, output_zip


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output-dir", type=Path, default=ROOT / "target")
    parser.add_argument(
        "--runtime-frameworks",
        type=Path,
        default=DEFAULT_RUNTIME_FRAMEWORKS,
    )
    parser.add_argument(
        "--signing-identity",
        default=os.environ.get("SIGNING_FINGERPRINT", ""),
    )
    parser.add_argument("--unsigned-for-testing", action="store_true")
    parser.add_argument(
        "--allow-unvalidated-capacity-for-testing",
        action="store_true",
    )
    parser.add_argument(
        "--allow-unvalidated-geometry-for-testing",
        action="store_true",
    )
    arguments = parser.parse_args()
    try:
        app, zip_path = build(arguments)
    except (OSError, RuntimeError, ValueError) as error:
        print(f"Final Cut package build failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
    print(f"Built {app}")
    print(f"Built {zip_path}")


if __name__ == "__main__":
    main()
