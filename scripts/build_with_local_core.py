#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Build test plugins against an explicit sibling core without editing the release pin."""
import argparse
from collections import Counter
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
LOCAL_PACKAGES = {"gyroflow-core", "stabilize_spirv", "neuflow-trace-shared"}
PACKAGES = {"ofx": "gyroflow-ofx", "adobe": "gyroflow_adobe", "frei0r": "gyroflow-frei0r", "finalcut": "gyroflow-finalcut"}


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def local_lock(text):
    blocks = re.split(r"(?m)(?=^\[\[package\]\])", text)
    for i, block in enumerate(blocks):
        name = re.search(r'^name = "([^"\n]+)"', block, re.M)
        if name and name[1] in LOCAL_PACKAGES:
            blocks[i] = re.sub(r'^(source|checksum) = [^\n]+\n', '', block, flags=re.M)
    return ''.join(blocks)


def other_dependencies(text):
    result = []
    for block in re.split(r"(?m)(?=^\[\[package\]\])", text)[1:]:
        values = dict(re.findall(r'^(name|version|source|checksum) = "([^"\n]+)"', block, re.M))
        if values.get("name") not in LOCAL_PACKAGES:
            result.append(tuple(sorted(values.items())))
    return Counter(result)


def ignore_sources(directory, names):
    return [name for name in names if name in {".git", "target", "build", "__pycache__", "docs", "openspec"}
            or name.lower().endswith(".md")]


def core_sources(core):
    return {p.relative_to(core).as_posix(): sha(p) for p in sorted((core / "src/core").rglob("*"))
            if p.is_file() and not p.name.lower().endswith(".md")}


def prepare_workspace(root, core, output):
    if not (core / "src/core/Cargo.toml").is_file():
        raise ValueError("--core-root must contain src/core/Cargo.toml")
    manifest = (root / "Cargo.toml").read_text(encoding="utf-8")
    if '[patch."https://github.com/NiYien/gyroflow.git"]' in manifest:
        raise ValueError("The plugin workspace already overrides this core; use a workspace with the formal pin")
    output.mkdir(parents=True, exist_ok=False)
    # Cargo workspace members must remain available even when only one plugin is selected.
    for member in ["adobe", "common", "finalcut", "frei0r", "openfx"]:
        shutil.copytree(root / member, output / member, ignore=ignore_sources)
    path = json.dumps((core / "src/core").as_posix(), ensure_ascii=False)
    manifest += '\n[patch."https://github.com/NiYien/gyroflow.git"]\ngyroflow-core = { path = ' + path + ' }\n'
    (output / "Cargo.toml").write_text(manifest, encoding="utf-8")
    formal = (root / "Cargo.lock").read_text(encoding="utf-8")
    patched = local_lock(formal)
    if other_dependencies(formal) != other_dependencies(patched):
        raise ValueError("Non-core dependency identities changed")
    (output / "Cargo.lock").write_text(patched, encoding="utf-8")


def run_cargo(arguments, environment, capture=False):
    command = ["just", "--justfile", str(ROOT / "Justfile"), "--command", "cargo", *arguments]
    return subprocess.run(command, cwd=ROOT, env=environment, check=True,
                          stdout=subprocess.PIPE if capture else None, text=True, encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--core-root", type=Path, required=True)
    parser.add_argument("--plugins", nargs="+", choices=PACKAGES, default=["ofx", "adobe", "frei0r"])
    parser.add_argument("--target", default="x86_64-pc-windows-msvc" if sys.platform == "win32" else "aarch64-apple-darwin" if sys.platform == "darwin" else "x86_64-unknown-linux-gnu")
    parser.add_argument("--profile", choices=["release", "deploy"], default="release")
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--prepare-only", action="store_true")
    args = parser.parse_args()
    if "finalcut" in args.plugins and sys.platform != "darwin":
        parser.error("Final Cut is macOS-only")
    core = args.core_root.resolve(strict=True)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    output = (args.output_dir or ROOT / "target" / ("local-core-" + stamp)).resolve()
    workspace = output / "workspace"
    prepare_workspace(ROOT, core, workspace)
    environment = os.environ.copy()
    # A caller can override this explicitly when validating another resource snapshot.
    pin = core / "resources/.lens_data_pin"
    if pin.is_file():
        environment.setdefault("NIYIEN_LENS_DATA_TAG", pin.read_text(encoding="utf-8").strip())
    receipt = {"kind": "local-core-test-build", "core_root": str(core), "workspace": str(workspace),
               "plugins": args.plugins, "target": args.target, "profile": args.profile,
               "formal_plugin_lock_sha256": sha(ROOT / "Cargo.lock"),
               "core_sources": core_sources(core),
               "built": False, "host_tested": False}
    receipt_file = output / "local-core-build.json"
    receipt_file.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print("Workspace:", workspace, flush=True)
    if args.prepare_only:
        return
    common = ["--manifest-path", str(workspace / "Cargo.toml"), "--locked"]
    if args.offline:
        common += ["--offline"]
    metadata = json.loads(run_cargo(["metadata", *common, "--format-version", "1", "--filter-platform", args.target], environment, True).stdout)
    actual = [p for p in metadata["packages"] if p["name"] == "gyroflow-core"]
    if (len(actual) != 1 or actual[0]["source"] is not None
            or Path(actual[0]["manifest_path"]).resolve() != core / "src/core/Cargo.toml"):
        raise ValueError("Cargo did not resolve the requested local core")
    if other_dependencies((ROOT / "Cargo.lock").read_text(encoding="utf-8")) != other_dependencies((workspace / "Cargo.lock").read_text(encoding="utf-8")):
        raise ValueError("Cargo changed a non-core dependency")
    packages = [option for name in dict.fromkeys(args.plugins) for option in ["-p", PACKAGES[name]]]
    run_cargo(["build", *common, "--profile", args.profile, "--target", args.target, *packages], environment)
    if receipt["core_sources"] != core_sources(core):
        raise ValueError("Core source changed during the build; rerun from a stable checkout")
    target = Path(metadata["target_directory"]) / args.target / args.profile
    files = {}
    for name in dict.fromkeys(args.plugins):
        stem = PACKAGES[name].replace("-", "_")
        filename = stem + ".dll" if "windows" in args.target else "lib" + stem + (".a" if name == "finalcut" else ".dylib" if "darwin" in args.target else ".so")
        artifact = target / filename
        if not artifact.is_file():
            raise FileNotFoundError(artifact)
        destination = output / filename
        shutil.copy2(artifact, destination)
        files[name] = {"file": str(destination), "sha256": sha(destination)}
        if "windows" in args.target and name == "adobe":
            shutil.copy2(destination, output / "GyroflowNiyien-Adobe-windows.aex")
        if "windows" in args.target and name == "frei0r":
            shutil.copy2(destination, output / "libgyroflow_niyien_frei0r.dll")
        if "windows" in args.target and name == "ofx":
            bundle = output / "GyroflowNiyien.ofx.bundle"
            binary = bundle / "Contents/Win64/GyroflowNiyien.ofx"
            binary.parent.mkdir(parents=True)
            shutil.copy2(destination, binary)
            resources = bundle / "Contents/Resources"
            resources.mkdir()
            shutil.copy2(ROOT / "LICENSE", resources / "LICENSE")
    receipt.update(built=True, actual_core_manifest=actual[0]["manifest_path"],
                   local_lock_sha256=sha(workspace / "Cargo.lock"), artifacts=files)
    receipt_file.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    if sys.platform == "win32" and "ofx" in args.plugins:
        # Package the existing adapter around exactly the same local-core DLL.
        subprocess.run([sys.executable, str(ROOT / "scripts/build_jianying_package.py"),
                        "--core", files["ofx"]["file"], "--output-dir", str(output)], check=True)
    print("Build receipt:", receipt_file, flush=True)


if __name__ == "__main__":
    main()
