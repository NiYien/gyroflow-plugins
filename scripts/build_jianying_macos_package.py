# SPDX-License-Identifier: GPL-3.0-or-later
"""Package the shared formal OFX core for native Apple Silicon Jianying."""
import argparse
import hashlib
import json
from pathlib import Path
import plistlib
import re
import shutil
import struct
import subprocess
import sys
import zipfile

import build_jianying_macos_candidate as candidate
import jianying_macos_build_receipt as receipt

ROOT = Path(__file__).resolve().parents[1]
BUNDLE = candidate.BUNDLE
WRAPPER = candidate.WRAPPER
CORE_BUNDLE = "Contents/PlugIns/GyroflowNiyien.ofx.bundle"
CORE = CORE_BUNDLE + "/Contents/MacOS/GyroflowNiyien.ofx"
PACKAGE = "GyroflowNiyien-Jianying-macos.zip"
LOCAL_PACKAGE = "GyroflowNiyien-Jianying-macos-local-validation.zip"
TEAM = "H59FJRN2AM"
GENERIC_CORE = "GyroflowNiyien.ofx.bundle/Contents/MacOS/GyroflowNiyien.ofx"
# Browser icons are named after the exported OfxPlugin identifier.
ICON_PNG = candidate.EFFECT_ID + ".png"
ICON_SVG = candidate.EFFECT_ID + ".svg"


def run(*args):
    result = subprocess.run([str(a) for a in args], text=True, capture_output=True)
    if result.returncode:
        sys.stderr.write(result.stdout)
        sys.stderr.write(result.stderr)
    result.check_returncode()
    return result


def metadata(version, local_validation=False):
    if not re.fullmatch(r"\d+(?:\.\d+){1,3}(?:[-+][A-Za-z0-9.-]+)?", version):
        raise ValueError("Invalid Jianying package version")
    return {"schema_version": 1, "plugin_id": candidate.PLUGIN_ID, "effect_id": candidate.EFFECT_ID, "version": version,
            "ofx_version": [1, 11], "platform": "macos", "architecture": "arm64",
            "candidate": local_validation, "fragment_sampling_policy": "core-default",
            "gpu": "Metal-only", "signature": "ad-hoc" if local_validation else "Developer ID",
            "notarization": "not-requested" if local_validation else "required-before-distribution",
            "autoload": True, "autoload_provider": candidate.AUTOLOAD_PROVIDER,
            "manual_saved_binding_priority": True, "icon": {"svg": ICON_SVG, "png": ICON_PNG}}


def validate_browser_png(data):
    if len(data) < 33 or data[:8] != b"\x89PNG\r\n\x1a\n" or data[12:16] != b"IHDR":
        raise ValueError("The effect browser icon must be a PNG image")
    width, height, depth, color = struct.unpack(">IIBB", data[16:26])
    if width < 128 or height < 128 or depth != 8 or color != 6:
        raise ValueError("The effect browser icon must be at least 128x128 with 8-bit RGBA")


def verify_shared_core(package, openfx_package):
    with zipfile.ZipFile(package) as archive, zipfile.ZipFile(openfx_package) as generic:
        if archive.read(BUNDLE + "/" + CORE) != generic.read(GENERIC_CORE):
            raise ValueError("Jianying and generic OFX packages must contain identical signed core bytes")


def verify(package, openfx_package=None, allow_local=False):
    with zipfile.ZipFile(package) as archive:
        names = archive.namelist()
        if len(names) != len(set(names)):
            raise ValueError("Duplicate archive member")
        declaration = json.loads(archive.read("delivery-manifest.json"))
        local = declaration.get("candidate")
        if type(local) is not bool or local and not allow_local:
            raise ValueError("A local validation archive is not a formal deliverable")
        expected_meta = metadata(declaration["version"], local)
        if any(declaration.get(k) != v for k, v in expected_meta.items()):
            raise ValueError("Unexpected formal Jianying identity or sampling policy")
        expected = {"delivery-manifest.json"}
        for item in declaration["files"]:
            name = item["path"]
            if not name.startswith(BUNDLE + "/") or "\\" in name or any(
                    p in ("", ".", "..") for p in name.split("/")) or name in expected:
                raise ValueError("Invalid formal Jianying inventory path")
            expected.add(name)
            data = archive.read(name)
            if len(data) != item["size"] or hashlib.sha256(data).hexdigest() != item["sha256"]:
                raise ValueError("Formal delivery checksum mismatch")
        if set(names) != expected:
            raise ValueError("Formal delivery inventory is incomplete")
        prefix = BUNDLE + "/Contents/"
        core_name = BUNDLE + "/" + CORE
        resources = prefix + "Resources/"
        inner = json.loads(archive.read(resources + "package-manifest.json"))
        if any(inner.get(k) != v for k, v in expected_meta.items()):
            raise ValueError("Inner and outer formal metadata differ")
        if inner.get("core_sha256") != declaration.get("core_sha256"):
            raise ValueError("Inner and outer core identities differ")
        if any(inner.get(key) != declaration.get(key) for key in (
                "formal_dependency_revision", "rust_source_sha256", "native_source_sha256")):
            raise ValueError("Inner and outer source identities differ")
        payload = {resources + "package-manifest.json", prefix + "MacOS/" + WRAPPER}
        payload.update(name for name in names if name.startswith(prefix + "_CodeSignature/"))
        for item in inner["files"]:
            name = item["path"]
            if "\\" in name or any(p in ("", ".", "..") for p in name.split("/")):
                raise ValueError("Invalid inner payload path")
            full_name = BUNDLE + "/" + name
            if full_name in payload:
                raise ValueError("Duplicate inner payload path")
            payload.add(full_name)
            data = archive.read(full_name)
            if len(data) != item["size"] or hashlib.sha256(data).hexdigest() != item["sha256"]:
                raise ValueError("Inner formal payload checksum mismatch")
        if payload != expected - {"delivery-manifest.json"}:
            raise ValueError("Inner formal inventory does not cover every payload file")
        if hashlib.sha256(archive.read(core_name)).hexdigest() != declaration.get("core_sha256"):
            raise ValueError("Declared shared core identity is incorrect")
        required = {prefix + "MacOS/" + WRAPPER, core_name,
                    BUNDLE + "/" + CORE_BUNDLE + "/Contents/Info.plist",
                    BUNDLE + "/" + CORE_BUNDLE + "/Contents/_CodeSignature/CodeResources",
                    prefix + "Info.plist", resources + "version.txt",
                    resources + "LICENSE", resources + "package-manifest.json",
                    prefix + "_CodeSignature/CodeResources"}
        required.update(resources + name for name in (ICON_SVG, ICON_PNG))
        if not required <= expected:
            raise ValueError("Formal package is missing a required file")
        validate_browser_png(archive.read(resources + ICON_PNG))
        info = plistlib.loads(archive.read(prefix + "Info.plist"))
        if info.get("CFBundleIdentifier") != candidate.PLUGIN_ID or info.get("CFBundleExecutable") != WRAPPER:
            raise ValueError("OFX draft identity changed")
        numeric = re.split(r"[-+]", declaration["version"])[0]
        if (info.get("NiYienPackageVersion") != declaration["version"]
                or info.get("CFBundleVersion") != numeric
                or info.get("CFBundleShortVersionString") != numeric):
            raise ValueError("Bundle version does not match the formal package")
        if archive.read(resources + "version.txt").decode().strip() != declaration["version"]:
            raise ValueError("Formal package version differs")
    if openfx_package is not None:
        verify_shared_core(package, openfx_package)
    return declaration


def verify_arm64(path):
    run("lipo", path, "-verify_arch", "arm64")


def verify_developer_id(path):
    requirement = ('=anchor apple generic and certificate leaf[subject.OU] = "' + TEAM
                   + '" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists')
    run("codesign", "--verify", "--strict", "--all-architectures", "-R", requirement, path)
    details = run("codesign", "-d", "--verbose=4", path).stderr
    if f"TeamIdentifier={TEAM}" not in details or "Authority=Developer ID Application:" not in details:
        raise ValueError("A valid NiYien Developer ID signature is required")


def sign(path, identity, local):
    args = ["--force", "--strict", "--options=runtime", "--sign", identity]
    if not local:
        args.append("--timestamp")
    run(sys.executable, ROOT / "scripts/codesign_with_retry.py", *args, path)


def copy_shared_core(core, openfx_package, bundle):
    source = core.parent.parent.parent
    if source.name != "GyroflowNiyien.ofx.bundle" or core != source / "Contents/MacOS/GyroflowNiyien.ofx":
        raise ValueError("The shared core must remain inside its original signed OFX bundle")
    run("codesign", "--verify", "--deep", "--strict", "--all-architectures", source)
    inventory = candidate.file_inventory(source)
    prefix = source.name + "/"
    with zipfile.ZipFile(openfx_package) as archive:
        files = {name[len(prefix):] for name in archive.namelist()
                 if name.startswith(prefix) and not name.endswith("/")}
        if files != {item["path"] for item in inventory}:
            raise ValueError("The input OFX bundle differs from the selected generic package")
        for item in inventory:
            data = archive.read(prefix + item["path"])
            if len(data) != item["size"] or hashlib.sha256(data).hexdigest() != item["sha256"]:
                raise ValueError("The input OFX bundle differs from the selected generic package")
    destination = bundle / CORE_BUNDLE
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(source, destination)


def write_package(package, output, delivery):
    with zipfile.ZipFile(package, "x", zipfile.ZIP_DEFLATED) as archive:
        for item in delivery["files"]:
            archive.write(output / item["path"], item["path"])
        archive.write(output / "delivery-manifest.json", "delivery-manifest.json")


def build(core, output, headers, version, build_receipt, openfx_package,
          identity, local_validation=False):
    if identity == "-" and not local_validation:
        raise ValueError("Formal deliverables require a Developer ID identity")
    meta = metadata(version, local_validation)
    core = core.resolve(strict=True)
    data = json.loads(build_receipt.read_text())
    native = ROOT / "jianying/native/macos"
    native_hashes = {name: receipt.sha256(native / name) for name in candidate.NATIVE_SOURCES}
    receipt.validate(data, core)
    verify_arm64(core)
    run("codesign", "--verify", "--strict", "--all-architectures", core)
    if not local_validation:
        verify_developer_id(core)
    output.mkdir(parents=True, exist_ok=False)
    bundle = output / BUNDLE
    binaries = bundle / "Contents/MacOS"
    resources = bundle / "Contents/Resources"
    binaries.mkdir(parents=True)
    resources.mkdir()
    native = ROOT / "jianying/native/macos"
    run("xcrun", "clang++", "-std=c++17", "-O2", "-dynamiclib", "-arch", "arm64",
        "-mmacosx-version-min=11.0", "-fobjc-arc", "-Wall", "-Wextra", "-Werror",
        "-I", headers, native / "jianying_macos.mm", native / "metal_guard.mm",
        "-framework", "Foundation", "-framework", "Metal", "-o", binaries / WRAPPER)
    copy_shared_core(core, openfx_package, bundle)
    shutil.copy2(ROOT / "LICENSE", resources / "LICENSE")
    for name, destination in (("icon.svg", ICON_SVG), ("icon.png", ICON_PNG)):
        shutil.copy2(ROOT / "jianying/resources" / name, resources / destination)
    # Older saved browser metadata can still reference the previous resource names.
    for suffix in (".png", ".svg"):
        shutil.copy2(resources / (candidate.EFFECT_ID + suffix), resources / (candidate.PLUGIN_ID + suffix))
    validate_browser_png((resources / ICON_PNG).read_bytes())
    numeric = re.split(r"[-+]", version)[0]
    info = {"CFBundleIdentifier": candidate.PLUGIN_ID, "CFBundleExecutable": WRAPPER,
            "CFBundleName": "Gyroflow", "CFBundlePackageType": "BNDL",
            "CFBundleVersion": numeric, "CFBundleShortVersionString": numeric,
            "NiYienPackageVersion": version, "LSMinimumSystemVersion": "11.0"}
    (bundle / "Contents/Info.plist").write_bytes(plistlib.dumps(info))
    (resources / "version.txt").write_text(version + "\n")
    meta.update(native_source_sha256=native_hashes, formal_dependency_revision=data["formal_dependency_revision"],
                core_sha256=receipt.sha256(core), rust_source_sha256=data["source_sha256"])
    payload = [item for item in candidate.file_inventory(bundle)
               if item["path"] != "Contents/MacOS/" + WRAPPER]
    inner = {**meta, "generated_signature_files": ["Contents/MacOS/" + WRAPPER,
             "Contents/_CodeSignature/CodeResources"], "files": payload}
    (resources / "package-manifest.json").write_text(json.dumps(inner, ensure_ascii=False, indent=2) + "\n")
    sign(bundle, identity, local_validation)
    run("codesign", "--verify", "--deep", "--strict", "--all-architectures", bundle)
    if not local_validation:
        verify_developer_id(bundle)
    if any(receipt.sha256(native / name) != digest for name, digest in native_hashes.items()):
        raise ValueError("Native adapter source changed during packaging")
    if receipt.sha256(bundle / CORE) != data["core_sha256"]:
        raise ValueError("Bundle signing changed the shared core")
    delivery = {**meta, "files": [{**item, "path": BUNDLE + "/" + item["path"]}
                                  for item in candidate.file_inventory(bundle)]}
    (output / "delivery-manifest.json").write_text(json.dumps(delivery, ensure_ascii=False, indent=2) + "\n")
    package = output / (LOCAL_PACKAGE if local_validation else PACKAGE)
    write_package(package, output, delivery)
    verify(package, openfx_package, allow_local=local_validation)
    receipt.validate(data, core)
    return {"package": str(package), "sha256": receipt.sha256(package),
            "candidate": local_validation, "installed": False, "published": False,
            "shared_core_byte_identical": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--core", type=Path)
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--include-dir", type=Path)
    parser.add_argument("--version")
    parser.add_argument("--build-receipt", type=Path)
    parser.add_argument("--openfx-package", type=Path, required=True)
    parser.add_argument("--signing-identity")
    parser.add_argument("--local-validation", action="store_true")
    parser.add_argument("--verify", type=Path)
    args = parser.parse_args()
    if args.verify:
        result = verify(args.verify, args.openfx_package, args.local_validation)
    else:
        if sys.platform != "darwin" or not all([args.core, args.output_dir, args.version,
                args.build_receipt, args.signing_identity]):
            parser.error("Build on macOS with --core, --output-dir, --version, --build-receipt and --signing-identity")
        result = build(args.core, args.output_dir, candidate.openfx_headers(args.include_dir),
                       args.version, args.build_receipt, args.openfx_package,
                       args.signing_identity, args.local_validation)
    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
