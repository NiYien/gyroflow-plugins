#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Build the Windows Jianying adapter around the shared OpenFX library."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
BUNDLE = "GyroflowJianyingBatch11.ofx.bundle"
WRAPPER = "GyroflowJianyingBatch11.ofx"
PLUGIN_ID = "xyz.niyien.gyroflow.jianying.batch11"
PACKAGE = "GyroflowNiyien-Jianying-windows.zip"


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def package_version(environment=None):
    environment = os.environ if environment is None else environment
    workspace = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    base = re.search(r'\[workspace\.package\]\s*version\s*=\s*"([^"\n]+)"', workspace)[1]
    reference = environment.get("GITHUB_REF", "")
    if reference.startswith("refs/tags/"):
        return reference.removeprefix("refs/tags/")
    number = environment.get("GITHUB_RUN_NUMBER")
    return f"{base}.{number}" if number else f"{base}-dev"


def msvc_environment():
    if shutil.which("cl"):
        return os.environ.copy()
    vswhere = Path(os.environ.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Microsoft Visual Studio/Installer/vswhere.exe"
    installation = subprocess.check_output([str(vswhere), "-utf8", "-latest", "-products", "*", "-requires",
                                            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                                            "-property", "installationPath"], text=True, encoding="utf-8").strip()
    if not installation:
        raise RuntimeError("Visual C++ x64 build tools are required")
    vcvars = Path(installation) / "VC/Auxiliary/Build/vcvars64.bat"
    result = subprocess.run(f'cmd /u /d /s /c ""{vcvars}" >nul && set"',
                            check=True, capture_output=True, text=True, encoding="utf-16le")
    environment = os.environ.copy()
    for line in result.stdout.splitlines():
        key, separator, value = line.partition("=")
        if separator and key:
            environment[key.upper()] = value
    return environment


def openfx_headers(environment):
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--offline",
         "--filter-platform", "x86_64-pc-windows-msvc"],
        cwd=ROOT, env=environment, text=True, encoding="utf-8"))
    package = next(p for p in metadata["packages"] if p["name"] == "ofx_sys")
    headers = Path(package["manifest_path"]).parent / "native/openfx/include"
    if not (headers / "ofxImageEffect.h").is_file():
        raise RuntimeError("The pinned ofx_sys dependency has no OpenFX headers")
    return headers


def compile_wrapper(output, environment):
    output.mkdir(parents=True, exist_ok=True)
    native = ROOT / "jianying/native"
    binary = output / WRAPPER
    compiler = shutil.which("cl", path=environment["PATH"])
    if not compiler:
        raise RuntimeError("Visual C++ x64 compiler was not found after vcvars64")
    subprocess.run([compiler, "/nologo", "/std:c++17", "/EHsc", "/O2", "/utf-8", "/LD",
                    str(native / "jianying.cpp"), "/I", str(openfx_headers(environment)),
                    f"/Fo:{output / 'jianying.obj'}", f"/Fe:{binary}",
                    "/link", "d3d11.lib", "dxgi.lib", "shell32.lib", "ole32.lib"],
                   cwd=ROOT, env=environment, check=True)
    return binary


def create_package(wrapper, core, output, version):
    output.mkdir(parents=True, exist_ok=True)
    package = output / PACKAGE
    with tempfile.TemporaryDirectory(prefix="jianying-package-", dir=output) as temporary:
        bundle = Path(temporary) / BUNDLE
        binaries = bundle / "Contents/Win64"
        binaries.mkdir(parents=True)
        shutil.copy2(wrapper, binaries / WRAPPER)
        shutil.copy2(core, binaries / "gyroflow_ofx.dll")
        shutil.copy2(ROOT / "LICENSE", bundle / "Contents/LICENSE")
        shutil.copy2(ROOT / "jianying/Info.plist", bundle / "Contents/Info.plist")
        (bundle / "Contents/version.txt").write_text(version + "\n", encoding="utf-8")
        files = [{"path": path.relative_to(bundle).as_posix(), "sha256": sha256(path)}
                 for path in sorted(bundle.rglob("*")) if path.is_file()]
        manifest = {"schema_version": 1, "plugin_id": PLUGIN_ID, "version": version, "files": files}
        (bundle / "Contents/package-manifest.json").write_text(
            json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        with zipfile.ZipFile(package, "w", zipfile.ZIP_DEFLATED) as archive:
            for path in sorted(bundle.rglob("*")):
                if path.is_file():
                    archive.write(path, path.relative_to(bundle.parent).as_posix())
    verify_package(package)
    return package, manifest


def verify_package(package):
    with zipfile.ZipFile(package) as archive:
        prefix = BUNDLE + "/"
        manifest_name = prefix + "Contents/package-manifest.json"
        manifest = json.loads(archive.read(manifest_name))
        if manifest["schema_version"] != 1 or manifest["plugin_id"] != PLUGIN_ID:
            raise ValueError("Unexpected Jianying package identity")
        expected = {manifest_name}
        for entry in manifest["files"]:
            name = entry["path"]
            if "\\" in name or name.startswith("/") or any(part in ("", ".", "..") for part in name.split("/")):
                raise ValueError("Invalid package manifest path")
            filename = prefix + name
            if filename in expected:
                raise ValueError("Duplicate package file")
            expected.add(filename)
            if hashlib.sha256(archive.read(filename)).hexdigest() != entry["sha256"]:
                raise ValueError("Package hash mismatch: " + name)
        required = {prefix + "Contents/Win64/" + WRAPPER, prefix + "Contents/Win64/gyroflow_ofx.dll"}
        if not required <= expected or set(archive.namelist()) != expected:
            raise ValueError("Unexpected Jianying package contents")
        if archive.read(prefix + "Contents/version.txt").decode("utf-8").strip() != manifest["version"]:
            raise ValueError("Inconsistent package version")
    return manifest


def verify_wrapper(wrapper):
    import ctypes

    class Plugin(ctypes.Structure):
        _fields_ = [("api", ctypes.c_char_p), ("api_version", ctypes.c_int),
                    ("identifier", ctypes.c_char_p), ("major", ctypes.c_uint),
                    ("minor", ctypes.c_uint), ("set_host", ctypes.c_void_p), ("entry", ctypes.c_void_p)]

    library = ctypes.CDLL(str(wrapper))
    library.OfxGetPlugin.argtypes = [ctypes.c_int]
    library.OfxGetPlugin.restype = ctypes.POINTER(Plugin)
    plugin = library.OfxGetPlugin(0)
    if library.OfxGetNumberOfPlugins() != 1 or not plugin or library.OfxGetPlugin(1):
        raise RuntimeError("Invalid Jianying plugin exports")
    if plugin.contents.identifier != PLUGIN_ID.encode() or (plugin.contents.major, plugin.contents.minor) != (1, 11):
        raise RuntimeError("Jianying draft identity changed")


def verify_shared_core(jianying_package, openfx_package):
    with zipfile.ZipFile(jianying_package) as jianying, zipfile.ZipFile(openfx_package) as openfx:
        inner = jianying.read(BUNDLE + "/Contents/Win64/gyroflow_ofx.dll")
        generic = openfx.read("GyroflowNiyien.ofx.bundle/Contents/Win64/GyroflowNiyien.ofx")
        if inner != generic:
            raise ValueError("Jianying and generic OpenFX packages contain different core binaries")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, default=Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")))
    parser.add_argument("--core", type=Path, help="Package an already-built shared OpenFX DLL")
    parser.add_argument("--verify", type=Path, help="Verify an existing package without building")
    parser.add_argument("--openfx-package", type=Path, help="Also verify the identical core in a generic OpenFX ZIP")
    parser.add_argument("--verify-wrapper", type=Path, help=argparse.SUPPRESS)
    arguments = parser.parse_args()
    if arguments.verify_wrapper:
        verify_wrapper(arguments.verify_wrapper.resolve(strict=True))
        return
    if arguments.verify:
        if arguments.openfx_package:
            verify_shared_core(arguments.verify, arguments.openfx_package)
        print(json.dumps(verify_package(arguments.verify), indent=2))
        return
    if sys.platform != "win32":
        parser.error("Jianying packaging is currently validated only on Windows x64")
    environment = msvc_environment()
    output = arguments.output_dir.resolve()
    if arguments.core:
        core = arguments.core.resolve(strict=True)
    else:
        subprocess.run(["cargo", "build", "--profile", "deploy", "--locked", "-p", "gyroflow-ofx"],
                       cwd=ROOT, env=environment, check=True)
        core = Path(environment.get("CARGO_TARGET_DIR", ROOT / "target")) / "deploy/gyroflow_ofx.dll"
    wrapper = compile_wrapper(output / "jianying-native", environment)
    shutil.copy2(core, wrapper.parent / "gyroflow_ofx.dll")
    subprocess.run([sys.executable, str(Path(__file__).resolve()), "--verify-wrapper", str(wrapper)], check=True)
    package, manifest = create_package(wrapper, core, output, package_version())
    summary = {"package": package.name, "sha256": sha256(package), "plugin_id": PLUGIN_ID,
               "version": manifest["version"], "ofx_version": [1, 11],
               "core_sha256": sha256(core), "wrapper_sha256": sha256(wrapper),
               "native_sources": {path.relative_to(ROOT).as_posix(): sha256(path)
                                  for path in sorted((ROOT / "jianying/native").glob("*"))},
               "host_verified": False}
    (output / "jianying-build-summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
