#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
from __future__ import annotations

import argparse
import hashlib
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


def run(command: list[str], environment: dict[str, str] | None = None,
        log_path: Path | None = None) -> str:
    result = subprocess.run(
        command,
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
    )
    if log_path is not None:
        log_path.parent.mkdir(parents=True, exist_ok=True)
        log_path.write_text(f"Command: {' '.join(command)}\nExit code: {result.returncode}\n"
                            + result.stdout + result.stderr)
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


def compile_app(staging_root: Path, runtime_frameworks: Path,
                diagnostics_dir: Path | None = None) -> tuple[Path, list[Path]]:
    run([sys.executable, str(LOCALIZATION_GENERATOR)])
    run([sys.executable, str(SDK_PREFLIGHT), "--deployment-target", "13.0"],
        log_path=diagnostics_dir / "sdk.log" if diagnostics_dir else None)
    run([sys.executable, str(RUST_BUILDER)],
        log_path=diagnostics_dir / "rust.log" if diagnostics_dir else None)
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
        ],
        log_path=diagnostics_dir / "xcode.log" if diagnostics_dir else None,
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
    frameworks = copy_runtime_frameworks(runtime_frameworks, xpc_contents)
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
    run([sys.executable, str(PACKAGE_VERIFIER), "--app", str(staged_app)],
        log_path=diagnostics_dir / "verification.log" if diagnostics_dir else None)
    return staged_app, frameworks


def bundle_digest(app: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(app.rglob("*")):
        relative = str(path.relative_to(app)).encode()
        if path.is_symlink():
            digest.update(b"link\0" + relative + b"\0" + os.readlink(path).encode() + b"\0")
        elif path.is_file():
            digest.update(b"file\0" + relative + b"\0" + str(path.stat().st_mode & 0o777).encode()
                          + b"\0" + path.read_bytes() + b"\0")
    return digest.hexdigest()


def compilation_record(app: Path) -> dict:
    from finalcut_release import source_digest
    identity = json.loads(IDENTITY_PATH.read_text())
    return {
        "schema_version": 1, "compilation": "passed", "app_name": APP_NAME,
        "source_sha256": source_digest(ROOT),
        "sdk_sha256": json.loads((ROOT / "finalcut/config/sdk.json").read_text())["sha256"],
        "marketing_version": identity["marketing_version"], "build_version": identity["build_version"],
        "app_sha256": bundle_digest(app),
    }


def compile_only(arguments: argparse.Namespace) -> Path:
    output_dir = arguments.output_dir.resolve()
    output_app = output_dir / APP_NAME
    record_path = output_dir / "compile-result.json"
    if output_app.exists() or record_path.exists():
        raise FileExistsError("refusing to replace an existing Final Cut compilation")
    output_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="finalcut-compile-", dir=output_dir) as directory:
        app, _ = compile_app(Path(directory), arguments.runtime_frameworks, arguments.diagnostics_dir)
        record = compilation_record(app)
        app.rename(output_app)
    record_path.write_text(json.dumps(record, indent=2) + "\n")
    if arguments.diagnostics_dir:
        arguments.diagnostics_dir.mkdir(parents=True, exist_ok=True)
        (arguments.diagnostics_dir / "compile-result.json").write_text(record_path.read_text())
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as output:
            output.write("### NiYien FCP compilation\n\n"
                         "Rust, App and XPC compiled and passed universal bundle verification.\n"
                         "The next stage selects a host-validation candidate or an accepted release package.\n\n")
    return output_app


def validate_compiled_app(app: Path) -> None:
    record = json.loads((app.parent / "compile-result.json").read_text())
    if not app.is_dir() or record != compilation_record(app):
        raise ValueError("Compiled App does not match its source, SDK, version or bundle digest")


def build(arguments: argparse.Namespace) -> tuple[Path, Path]:
    testing = (arguments.unsigned_for_testing or arguments.allow_unvalidated_capacity_for_testing
               or arguments.allow_unvalidated_geometry_for_testing)
    candidate = arguments.candidate
    if candidate:
        if testing or not arguments.compiled_app:
            raise ValueError("A validation candidate requires a verified compiled App and Developer ID signing")
        if os.environ.get("GITHUB_ACTIONS") == "true" and os.environ.get("GITHUB_EVENT_NAME") != "workflow_dispatch":
            raise ValueError("CI validation candidates require a manual workflow_dispatch run")
        from finalcut_release import validate_core
        validate_core(ROOT, json.loads((ROOT / "finalcut/config/release-inputs.json").read_text()))
    if arguments.compiled_app and testing:
        raise ValueError("Reusing a compiled App requires production release acceptance")
    if not testing and not candidate:
        from finalcut_release import require_acceptance
        require_acceptance(ROOT)
    require_capacity_gate(arguments.allow_unvalidated_capacity_for_testing)
    if not candidate:
        require_geometry_gate(arguments.allow_unvalidated_geometry_for_testing)
    if not arguments.unsigned_for_testing and not arguments.signing_identity:
        raise RuntimeError("production package requires --signing-identity")
    from finalcut_release import acceptance_blockers
    blockers = acceptance_blockers(ROOT)
    output_dir = arguments.output_dir.resolve()
    output_app = output_dir / APP_NAME
    output_zip = output_dir / ZIP_NAME
    if output_app.exists() or output_zip.exists():
        raise FileExistsError("refusing to replace an existing Final Cut package")
    output_dir.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="finalcut-package-", dir=output_dir) as directory:
        staging_root = Path(directory)
        if arguments.compiled_app:
            validate_compiled_app(arguments.compiled_app)
            staged_app = staging_root / APP_NAME
            run(["ditto", str(arguments.compiled_app), str(staged_app)])
            frameworks = [staged_app / "Contents/PlugIns" / XPC_NAME / "Contents/Frameworks" / name
                          for name in RUNTIME_FRAMEWORK_NAMES]
        else:
            staged_app, frameworks = compile_app(staging_root, arguments.runtime_frameworks)
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
    distribution = {"channel": "candidate" if candidate else ("testing" if testing else "validated"),
                    "acceptance_ready": not blockers, "acceptance_blockers": blockers}
    (output_dir / "distribution-status.json").write_text(json.dumps(distribution, indent=2) + "\n")
    return output_app, output_zip


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--candidate", action="store_true",
                        help="Sign a manually requested validation candidate without declaring release acceptance")
    parser.add_argument("--compile-only", action="store_true",
                        help="Compile and verify locally without signing or creating a distribution ZIP")
    parser.add_argument("--compiled-app", type=Path,
                        help="Package the verified App from the compile-only stage on this runner")
    parser.add_argument("--diagnostics-dir", type=Path)
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
        if arguments.compile_only:
            if (arguments.candidate or arguments.compiled_app or arguments.unsigned_for_testing
                    or arguments.allow_unvalidated_capacity_for_testing
                    or arguments.allow_unvalidated_geometry_for_testing):
                raise ValueError("Compile-only cannot be combined with package or testing options")
            app = compile_only(arguments)
            print(f"Compiled and verified {app}; no distribution package was created")
            return
        app, zip_path = build(arguments)
    except (OSError, RuntimeError, ValueError) as error:
        print(f"Final Cut package build failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
    print(f"Built {app}")
    print(f"Built {zip_path}")


if __name__ == "__main__":
    main()
