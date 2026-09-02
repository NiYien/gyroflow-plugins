#!/usr/bin/env python3
import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Optional


ROOT = Path(__file__).resolve().parents[1]
REPOSITORY_ROOT = ROOT.parents[1]
FXPLUG_SDK_FRAMEWORKS = Path("/Library/Developer/SDKs/FxPlug.sdk/Library/Frameworks")
WORKFLOW_EXTENSION_SDK = Path("/Library/Developer/SDKs/WorkflowExtensionSDK.sdk")


def run(command: list[str]) -> None:
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(
            f"command failed ({result.returncode}): {' '.join(command)}\n{result.stdout}{result.stderr}"
        )


def compile_universal(
    output: Path,
    sources: list[Path],
    frameworks: list[str],
    module_cache: Path,
    extra_compiler_flags: Optional[list[str]] = None,
) -> None:
    command = [
        "xcrun",
        "clang",
        "-fobjc-arc",
        "-fmodules",
        f"-fmodules-cache-path={module_cache}",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-arch",
        "arm64",
        "-arch",
        "x86_64",
        "-mmacosx-version-min=13.5",
        "-F",
        str(FXPLUG_SDK_FRAMEWORKS),
        "-Wl,-rpath,@loader_path/../Frameworks",
        "-o",
        str(output),
    ]
    command.extend(extra_compiler_flags or [])
    command.extend(str(source) for source in sources)
    for framework in frameworks:
        command.extend(["-framework", framework])
    run(command)


def compile_workflow_extension(output: Path, source: Path, module_cache: Path) -> None:
    command = [
        "xcrun",
        "clang",
        "-fobjc-arc",
        "-fmodules",
        f"-fmodules-cache-path={module_cache}",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-arch",
        "arm64",
        "-arch",
        "x86_64",
        "-mmacosx-version-min=13.5",
        "-I",
        str(WORKFLOW_EXTENSION_SDK / "usr" / "include"),
        "-F",
        str(WORKFLOW_EXTENSION_SDK / "Library" / "Frameworks"),
        "-L",
        str(WORKFLOW_EXTENSION_SDK / "usr" / "lib"),
        "-Wl,-e,_ProExtensionMain",
        "-o",
        str(output),
        str(source),
        "-framework",
        "Cocoa",
        "-framework",
        "CoreMedia",
        "-lProExtension",
    ]
    run(command)


def copy_framework(source_root: Path, destination_root: Path, name: str) -> Path:
    source = source_root / f"{name}.framework"
    if not source.is_dir():
        raise FileNotFoundError(f"required runtime framework is missing: {source}")
    destination = destination_root / source.name
    shutil.copytree(source, destination, symlinks=True)
    return destination


def build_probe(
    output: Path,
    framework_root: Path,
    signing_identity: str,
    processing_color_info: str,
) -> None:
    if output.exists():
        raise FileExistsError(f"refusing to replace existing output: {output}")
    if not FXPLUG_SDK_FRAMEWORKS.is_dir():
        raise FileNotFoundError(f"FxPlug SDK frameworks are missing: {FXPLUG_SDK_FRAMEWORKS}")
    if not WORKFLOW_EXTENSION_SDK.is_dir():
        raise FileNotFoundError(
            f"Workflow Extension SDK is missing: {WORKFLOW_EXTENSION_SDK}"
        )

    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="finalcut-phase0-build-", dir=output.parent) as directory:
        app = Path(directory) / output.name
        app_contents = app / "Contents"
        app_macos = app_contents / "MacOS"
        app_plugins = app_contents / "PlugIns"
        app_resources = app_contents / "Resources"
        xpc = app_plugins / "GyroflowNiYienPhase0Renderer.pluginkit"
        xpc_contents = xpc / "Contents"
        xpc_macos = xpc_contents / "MacOS"
        xpc_frameworks = xpc_contents / "Frameworks"
        workflow = app_plugins / "GyroflowNiYienPhase0Workflow.appex"
        workflow_contents = workflow / "Contents"
        workflow_macos = workflow_contents / "MacOS"
        template = (
            app_resources
            / "Motion Templates"
            / "Effects.localized"
            / "NiYien"
            / "Gyroflow"
        )
        for path in (app_macos, xpc_macos, xpc_frameworks, workflow_macos, template):
            path.mkdir(parents=True)

        shutil.copy2(ROOT / "app" / "Info.plist", app_contents / "Info.plist")
        shutil.copy2(ROOT / "xpc" / "Info.plist", xpc_contents / "Info.plist")
        shutil.copy2(ROOT / "workflow" / "Info.plist", workflow_contents / "Info.plist")

        app_executable = app_macos / "GyroflowNiYienFinalCutPhase0"
        compile_universal(
            app_executable,
            [ROOT / "app" / "main.m"],
            ["Cocoa"],
            Path(directory) / ".clang-module-cache",
        )

        xpc_executable = xpc_macos / "GyroflowNiYienPhase0Renderer"
        compile_universal(
            xpc_executable,
            [
                ROOT / "xpc" / "main.m",
                ROOT / "xpc" / "GFPhase0Effect.m",
                ROOT / "xpc" / "GFPhase0DropZoneView.m",
                ROOT / "xpc" / "GFPhase0ParameterCommitter.m",
                ROOT / "xpc" / "GFPhase0MediaResolver.m",
                ROOT / "xpc" / "GFPhase0ProjectStore.m",
            ],
            [
                "Cocoa",
                "CoreMedia",
                "IOSurface",
                "Metal",
                "UniformTypeIdentifiers",
                "FxPlug",
                "PluginManager",
            ],
            Path(directory) / ".clang-module-cache",
            [
                f"-DGF_PHASE0_PROCESSING_COLOR_INFO={ {'auto': -1, 'linear': 0, 'gamma': 2}[processing_color_info] }"
            ],
        )

        workflow_executable = workflow_macos / "GyroflowNiYienPhase0Workflow"
        compile_workflow_extension(
            workflow_executable,
            ROOT / "workflow" / "GFPhase0WorkflowViewController.m",
            Path(directory) / ".clang-module-cache",
        )

        fxplug_framework = copy_framework(framework_root, xpc_frameworks, "FxPlug")
        plugin_manager_framework = copy_framework(framework_root, xpc_frameworks, "PluginManager")

        generated_template = template / "Gyroflow NiYien Phase 0.moef"
        run(
            [
                sys.executable,
                str(ROOT / "scripts" / "generate_template.py"),
                "--output",
                str(generated_template),
            ]
        )
        run(
            [
                sys.executable,
                str(ROOT / "scripts" / "verify_template.py"),
                str(generated_template),
                str(xpc_contents / "Info.plist"),
            ]
        )
        preview_source = REPOSITORY_ROOT / "adobe" / "logo_white.png"
        run(["sips", "-z", "300", "400", str(preview_source), "--out", str(template / "large.png")])
        run(["sips", "-z", "60", "80", str(preview_source), "--out", str(template / "small.png")])

        if signing_identity != "-":
            for framework in (fxplug_framework, plugin_manager_framework):
                run(
                    [
                        "codesign",
                        "--force",
                        "--sign",
                        signing_identity,
                        "--timestamp=none",
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
                signing_identity,
                "--timestamp=none",
                "--options",
                "runtime",
                "--entitlements",
                str(ROOT / "xpc" / "SandboxEntitlements.entitlements"),
                str(xpc),
            ]
        )
        run(
            [
                "codesign",
                "--force",
                "--sign",
                signing_identity,
                "--timestamp=none",
                "--options",
                "runtime",
                "--entitlements",
                str(ROOT / "workflow" / "SandboxEntitlements.entitlements"),
                str(workflow),
            ]
        )
        run(
            [
                "codesign",
                "--force",
                "--sign",
                signing_identity,
                "--timestamp=none",
                "--options",
                "runtime",
                str(app),
            ]
        )
        os.replace(app, output)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--framework-root", required=True, type=Path)
    parser.add_argument("--signing-identity", default="-")
    parser.add_argument(
        "--processing-color-info",
        choices=("auto", "linear", "gamma"),
        default="auto",
    )
    args = parser.parse_args()
    build_probe(
        args.output.resolve(),
        args.framework_root.resolve(),
        args.signing_identity,
        args.processing_color_info,
    )


if __name__ == "__main__":
    main()
