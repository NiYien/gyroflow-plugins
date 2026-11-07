#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Build an arm64-only local Jianying candidate without installing or publishing it."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[1]
BUNDLE = "GyroflowJianyingBatch11.ofx.bundle"
WRAPPER = "GyroflowJianyingBatch11.ofx"
CORE = "libgyroflow_ofx.dylib"
PLUGIN_ID = "xyz.niyien.gyroflow.jianying.batch11"
EFFECT_ID = "xyz.niyien.jianying.gyroflow"
PACKAGE = "GyroflowNiyien-Jianying-macos-arm64-candidate.zip"
DELIVERY = "delivery-manifest.json"
NATIVE_SOURCES = ("jianying_macos.mm", "metal_guard.mm", "metal_guard.h", "source_path.h", "../source_path_contract.h")
AUTOLOAD_PROVIDER = {"id": "macos-effect-processor-source-path", "version": 2,
                     "compatibility": "runtime-type-and-layout-contract",
                     "host": {"bundle_id": "com.lemon.lvpro", "architecture": "arm64",
                              "module": "libcccreator.dylib"}}
AUTOLOAD_REQUIRED_RUST_SOURCES = ("openfx/src/gyroflow.rs", "openfx/src/jianying_macos.rs",
                                  "openfx/src/jianying_autoload.rs", "openfx/src/lib.rs")
RUST_SOURCES = ("openfx/src/gyroflow.rs", "openfx/src/jianying_macos.rs", "openfx/src/lib.rs",
                "openfx/src/jianying_autoload.rs", "openfx/src/jianying_protocol.rs",
                "openfx/src/pending_host_values.rs",
                "openfx/Cargo.toml", "common/src/lib.rs", "common/src/loaded_manager.rs",
                "common/Cargo.toml", "Cargo.lock")


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(*arguments, capture=False):
    return subprocess.run(arguments, cwd=ROOT, check=True, text=True,
                          capture_output=capture)


def architectures(path):
    return run("lipo", "-archs", str(path), capture=True).stdout.split()


def require_arm64(path):
    if architectures(path) != ["arm64"]:
        raise ValueError("The local candidate requires an exact arm64 binary: " + str(path))


def openfx_headers(explicit=None):
    if explicit is not None:
        headers = explicit.resolve(strict=True)
    else:
        result = run("cargo", "metadata", "--locked", "--format-version", "1",
                     "--filter-platform", "aarch64-apple-darwin", capture=True)
        metadata = json.loads(result.stdout)
        package = next(item for item in metadata["packages"] if item["name"] == "ofx_sys")
        headers = Path(package["manifest_path"]).parent / "native/openfx/include"
    for header in ("ofxCore.h", "ofxImageEffect.h", "ofxProperty.h", "ofxParam.h"):
        if not (headers / header).is_file():
            raise ValueError("Missing pinned OpenFX header: " + str(headers / header))
    return headers


def version_number(version):
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.-]+)?", version):
        raise ValueError("Use a three-part candidate version with an optional suffix")
    return version.split("-", 1)[0].split("+", 1)[0]


def reserve_output(output):
    # Never overwrite a prior candidate or remove a previous build directory.
    output = output.resolve()
    protected = (Path("/Applications"), Path("/Library"), Path("/System"),
                 Path("/usr"), Path("/bin"), Path("/sbin"), Path("/private/etc"))
    if (any(output == path or path in output.parents for path in protected) or
            any(part.lower() in ("ofx", "plugins") or part.lower().endswith(".app")
                for part in output.parts)):
        raise ValueError("Use a build output directory, not an application or plugin installation directory")
    for name in (BUNDLE, PACKAGE, DELIVERY, "jianying-macos-build-summary.json"):
        if (output / name).exists():
            raise FileExistsError("Candidate output already exists: " + str(output / name))
    output.mkdir(parents=True, exist_ok=True)


def file_inventory(directory):
    return [{"path": path.relative_to(directory).as_posix(), "size": path.stat().st_size,
             "sha256": sha256(path)}
            for path in sorted(directory.rglob("*")) if path.is_file()]


def write_json(path, value):
    with path.open("x", encoding="utf-8") as stream:
        stream.write(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def candidate_metadata(version, autoload=True):
    metadata = {"schema_version": 1, "plugin_id": PLUGIN_ID, "effect_id": EFFECT_ID, "version": version,
            "ofx_version": [1, 11], "platform": "macos", "architecture": "arm64",
            "candidate": True, "host_verified": False, "notarized": False,
            "signature": "ad-hoc", "gpu": "Metal-only", "autoload": autoload,
            "metadata_layout": {"version": "Contents/Resources/version.txt",
                                "package_manifest": "Contents/Resources/package-manifest.json",
                                "license": "Contents/Resources/LICENSE"},
            "version_scope": "OFX identity stays 1.11; CF numeric version and package candidate version are separate",
            "diagnostic_mode": {"name": "DiagnosticMode", "type": "OfxParamTypeChoice", "default": 0,
                                "animates": False, "secret": True,
                                "options": ["0 - Stabilize", "1 - Source copy", "2 - Color pattern",
                                            "3 - Source opaque", "4 - Batch sweep (2.5s each)"],
                                "definition": "after_inner_describe_in_context", "added_to_Main_page": True}}
    if autoload:
        metadata.update(autoload_provider=json.loads(json.dumps(AUTOLOAD_PROVIDER)),
                        manual_saved_binding_priority=True)
    return metadata


def validate_autoload_metadata(manifest):
    autoload = manifest.get("autoload")
    if type(autoload) is not bool:
        raise ValueError("Candidate autoload must be an explicit boolean")
    if autoload and (manifest.get("autoload_provider") != AUTOLOAD_PROVIDER
                     or manifest.get("manual_saved_binding_priority") is not True):
        raise ValueError("Enabled autoload requires the verified provider identity and manual/saved binding priority")
    return autoload


def source_provenance():
    common = (ROOT / "common/Cargo.toml").read_text(encoding="utf-8")
    pin = re.search(r'gyroflow-core\s*=\s*\{[^\n]*\brev\s*=\s*"([0-9a-f]{40})"', common)
    if not pin:
        raise ValueError("Cannot establish the shared core source revision")
    status = run("git", "status", "--porcelain=v1", "--untracked-files=all", capture=True).stdout.splitlines()
    public_status = [line for line in status if not line[3:].lower().endswith(".md")
                     and not any(part.lower() in ("docs", "openspec") for part in Path(line[3:]).parts)]
    return {"git_head": run("git", "rev-parse", "HEAD", capture=True).stdout.strip(),
            "source_dirty": bool(status), "git_status": public_status,
            "shared_core_source_revision": pin[1],
            "formal_dependency_revision": pin[1],
            "actual_core_source": {"kind": "formal-git-dependency", "revision": pin[1]},
            "rust_source_sha256": {name: sha256(ROOT / name) for name in RUST_SOURCES}}


def local_validation_module():
    path = Path(__file__).with_name("jianying_local_core_provenance.py")
    spec = importlib.util.spec_from_file_location("jianying_local_core_provenance", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def validate_local_core_provenance(path, inner):
    return local_validation_module().validate_local_core_provenance(path, inner, ROOT)


def validate_autoload_build_provenance(path, inner, rust_source_sha256):
    if path is None:
        raise ValueError("Autoload builds require a fresh --local-core-provenance")
    validated = validate_local_core_provenance(path.resolve(strict=True), inner)
    recorded = validated["local_core_provenance"].get("plugin_source_sha256", {})
    for name in AUTOLOAD_REQUIRED_RUST_SOURCES:
        if (not isinstance(recorded, dict) or name not in recorded
                or name not in rust_source_sha256 or recorded[name] != rust_source_sha256[name]):
            raise ValueError("Autoload provenance must cover current Rust source: " + name)
    return validated


def verify_delivery(package):
    """Verify only archive bytes and declarations, without reading its evidence paths."""
    with zipfile.ZipFile(package) as archive:
        names = archive.namelist()
        if len(names) != len(set(names)):
            raise ValueError("Duplicate ZIP member")
        manifest = json.loads(archive.read(DELIVERY))
        if manifest["plugin_id"] != PLUGIN_ID or manifest.get("effect_id") != EFFECT_ID or manifest["ofx_version"] != [1, 11]:
            raise ValueError("Candidate identity changed")
        if (manifest.get("platform"), manifest.get("architecture"), manifest.get("candidate"),
                manifest.get("host_verified"), manifest.get("notarized")) != (
                    "macos", "arm64", True, False, False):
            raise ValueError("A local candidate must not claim production acceptance")
        autoload = validate_autoload_metadata(manifest)
        expected = {DELIVERY}
        for entry in manifest["files"]:
            name = entry["path"]
            if ("\\" in name or name.startswith("/") or
                    any(part in ("", ".", "..") for part in name.split("/")) or
                    not name.startswith(BUNDLE + "/")):
                raise ValueError("Invalid delivery path: " + name)
            if name in expected:
                raise ValueError("Duplicate delivery path: " + name)
            expected.add(name)
            data = archive.read(name)
            if len(data) != entry["size"] or hashlib.sha256(data).hexdigest() != entry["sha256"]:
                raise ValueError("Delivery checksum mismatch: " + name)
        if set(names) != expected:
            raise ValueError("Delivery inventory does not cover every ZIP file")
        prefix = BUNDLE + "/Contents/"
        resources = prefix + "Resources/"
        required = {prefix + "MacOS/" + WRAPPER, prefix + "MacOS/" + CORE,
                    prefix + "Info.plist", resources + "version.txt", resources + "package-manifest.json",
                    resources + "LICENSE", prefix + "_CodeSignature/CodeResources"}
        if not required <= expected:
            raise ValueError("Candidate is incomplete")
        inner = json.loads(archive.read(resources + "package-manifest.json"))
        validate_autoload_metadata(inner)
        for key, value in candidate_metadata(manifest["version"], autoload=autoload).items():
            if inner.get(key) != value:
                raise ValueError("Inner and outer candidate metadata disagree: " + key)
        for key in ("formal_dependency_revision", "actual_core_source", "local_core_provenance",
                    "local_core_provenance_sha256", "local_core_provenance_file", "local_core_gpu_validation_file"):
            if inner.get(key) != manifest.get(key):
                raise ValueError("Inner and outer source provenance disagree: " + key)
        actual_source = manifest.get("actual_core_source", {})
        if actual_source.get("kind") == "local-platform-patch":
            if "shared_core_source_revision" in manifest or "shared_core_source_revision" in inner:
                raise ValueError("A local platform patch must not claim an unmodified shared core revision")
            declaration = manifest.get("local_core_provenance")
            local_validation_module().validate_declaration(declaration)
            provenance_name = BUNDLE + "/" + manifest.get("local_core_provenance_file", "")
            raw = archive.read(provenance_name)
            if (hashlib.sha256(raw).hexdigest() != manifest.get("local_core_provenance_sha256")
                    or json.loads(raw) != declaration
                    or declaration["inner_binary"]["sha256"] != manifest.get("core_sha256")
                    or declaration["base_core_revision"] != manifest.get("formal_dependency_revision")
                    or actual_source.get("base_revision") != declaration["base_core_revision"]
                    or actual_source.get("base_tree") != declaration["base_core_tree"]
                    or actual_source.get("tree_sha256") != declaration["source_snapshot"]["tree_sha256"]):
                raise ValueError("Local platform patch identity or original provenance bytes changed")
            gpu_raw = archive.read(BUNDLE + "/" + manifest.get("local_core_gpu_validation_file", ""))
            gpu_summary = json.loads(gpu_raw)
            gpu_identity = actual_source.get("gpu_validation", {})
            if (hashlib.sha256(gpu_raw).hexdigest() != declaration["gpu_validation"]["summary_sha256"]
                    or gpu_summary != gpu_identity.get("summary")
                    or gpu_identity.get("summary_sha256") != declaration["gpu_validation"]["summary_sha256"]):
                raise ValueError("Original GPU validation receipt bytes changed")
            local_validation_module().validate_gpu_summary(gpu_summary, read_evidence=False)
        inner_expected = {resources + "package-manifest.json"}
        inner_expected.update(name for name in names if name.startswith(prefix + "_CodeSignature/"))
        inner_expected.add(prefix + "MacOS/" + WRAPPER)
        for entry in inner["files"]:
            name = entry["path"]
            if ("\\" in name or name.startswith("/") or
                    any(part in ("", ".", "..") for part in name.split("/"))):
                raise ValueError("Invalid inner payload path")
            filename = BUNDLE + "/" + name
            if filename in inner_expected:
                raise ValueError("Duplicate inner payload path")
            inner_expected.add(filename)
            data = archive.read(filename)
            if len(data) != entry["size"] or hashlib.sha256(data).hexdigest() != entry["sha256"]:
                raise ValueError("Inner payload checksum mismatch: " + name)
        if inner_expected != expected - {DELIVERY}:
            raise ValueError("Inner payload inventory does not cover the signed payload")
        if archive.read(resources + "version.txt").decode().strip() != manifest["version"]:
            raise ValueError("Candidate version mismatch")
        info = plistlib.loads(archive.read(prefix + "Info.plist"))
        if info["CFBundleIdentifier"] != PLUGIN_ID or info["CFBundleExecutable"] != WRAPPER:
            raise ValueError("Bundle identity changed")
        if (info["CFBundleShortVersionString"] != version_number(manifest["version"]) or
                info.get("NiYienPackageVersion") != manifest["version"]):
            raise ValueError("Bundle version mismatch")
    return manifest


def build(core, output, headers, version, local_core_provenance=None):
    if local_core_provenance is None:
        raise ValueError("Autoload builds require a fresh --local-core-provenance")
    short_version = version_number(version)
    core = core.resolve(strict=True)
    output = output.resolve()
    require_arm64(core)
    run("codesign", "--verify", "--strict", str(core))
    signature = run("codesign", "-dvvv", str(core), capture=True)
    if "Signature=adhoc" not in signature.stderr:
        raise ValueError("The shared core must already have a valid ad-hoc signature for byte-identical packaging")
    sources = ROOT / "jianying/native/macos"
    source_hashes = {str((sources / name).relative_to(ROOT)): sha256(sources / name)
                     for name in NATIVE_SOURCES}
    provenance = source_provenance()
    local_core_provenance = local_core_provenance.resolve(strict=True)
    provenance.update(validate_autoload_build_provenance(
        local_core_provenance, core, provenance["rust_source_sha256"]))
    provenance.pop("shared_core_source_revision")
    reserve_output(output)
    bundle = output / BUNDLE
    binaries = bundle / "Contents/MacOS"
    binaries.mkdir(parents=True)
    resources = bundle / "Contents/Resources"
    resources.mkdir()
    wrapper = binaries / WRAPPER
    run("xcrun", "clang++", "-std=c++17", "-O2", "-dynamiclib", "-arch", "arm64",
        "-mmacosx-version-min=11.0", "-fobjc-arc", "-Wall", "-Wextra", "-Werror",
        "-I", str(headers), str(sources / "jianying_macos.mm"), str(sources / "metal_guard.mm"),
        "-framework", "Foundation", "-framework", "Metal", "-o", str(wrapper))
    require_arm64(wrapper)
    input_core_sha = sha256(core)
    packaged_core = binaries / CORE
    shutil.copy2(core, packaged_core)
    if sha256(packaged_core) != input_core_sha:
        raise ValueError("Shared core copy changed bytes")
    # Preserve the compiler's valid linker signature so both packages can use identical bytes.
    run("codesign", "--verify", "--strict", str(packaged_core))
    run("codesign", "--force", "--sign", "-", str(wrapper))
    shutil.copy2(ROOT / "LICENSE", resources / "LICENSE")
    info = {"CFBundleIdentifier": PLUGIN_ID, "CFBundleExecutable": WRAPPER,
            "CFBundleName": "Gyroflow", "CFBundlePackageType": "BNDL",
            "CFBundleVersion": short_version, "CFBundleShortVersionString": short_version,
            "NiYienPackageVersion": version,
            "LSMinimumSystemVersion": "11.0"}
    with (bundle / "Contents/Info.plist").open("xb") as stream:
        plistlib.dump(info, stream, sort_keys=False)
    (resources / "version.txt").write_text(version + "\n", encoding="utf-8")
    if local_core_provenance is not None:
        raw = local_core_provenance.read_bytes()
        if hashlib.sha256(raw).hexdigest() != provenance["local_core_provenance_sha256"]:
            raise ValueError("Local core provenance changed during candidate packaging")
        (resources / "local-core-provenance.json").write_bytes(raw)
        gpu_record = provenance["local_core_provenance"]["gpu_validation"]
        gpu_raw = Path(gpu_record["summary_path"]).read_bytes()
        if hashlib.sha256(gpu_raw).hexdigest() != gpu_record["summary_sha256"]:
            raise ValueError("GPU validation receipt changed during candidate packaging")
        (resources / "local-core-gpu-validation.json").write_bytes(gpu_raw)
    metadata = candidate_metadata(version)
    metadata.update(**provenance, core_sha256=input_core_sha, source_sha256=source_hashes,
                    inventory_scope="Payload excluding this manifest, the bundle-signed main executable and generated signature seal",
                    generated_signature_files=["Contents/MacOS/" + WRAPPER, "Contents/_CodeSignature/CodeResources"],
                    files=[item for item in file_inventory(bundle)
                           if not item["path"].startswith("Contents/_CodeSignature/")
                           and item["path"] != "Contents/MacOS/" + WRAPPER])
    write_json(resources / "package-manifest.json", metadata)
    run("codesign", "--force", "--sign", "-", str(bundle))
    run("codesign", "--verify", "--deep", "--strict", "--all-architectures", str(bundle))
    if sha256(packaged_core) != input_core_sha or sha256(core) != input_core_sha:
        raise ValueError("Signing changed the shared core bytes")
    delivery = candidate_metadata(version)
    delivery.update(**provenance, core_sha256=input_core_sha, source_sha256=source_hashes,
                    inventory_scope="Every bundle file including the generated signature seal",
                    files=[{**item, "path": BUNDLE + "/" + item["path"]} for item in file_inventory(bundle)])
    write_json(output / DELIVERY, delivery)
    package = output / PACKAGE
    with zipfile.ZipFile(package, "x", zipfile.ZIP_DEFLATED) as archive:
        for item in delivery["files"]:
            archive.write(output / item["path"], item["path"])
        archive.write(output / DELIVERY, DELIVERY)
    verify_delivery(package)
    summary = candidate_metadata(version)
    if source_provenance()["rust_source_sha256"] != provenance["rust_source_sha256"]:
        raise ValueError("Rust source changed during candidate packaging")
    if any(sha256(ROOT / name) != digest for name, digest in source_hashes.items()):
        raise ValueError("Native source changed during candidate packaging")
    if local_core_provenance is not None:
        if validate_local_core_provenance(local_core_provenance, core) != {
                key: provenance[key] for key in ("formal_dependency_revision", "actual_core_source",
                                               "local_core_provenance", "local_core_provenance_sha256",
                                               "local_core_provenance_file", "local_core_gpu_validation_file")}:
            raise ValueError("Local core evidence changed during candidate packaging")
    summary.update(**provenance, package=str(package), package_sha256=sha256(package), bundle=str(bundle),
                   input_core=str(core), core_sha256=input_core_sha, wrapper_sha256=sha256(wrapper),
                   source_sha256=source_hashes, include_dir=str(headers),
                   shared_core_byte_identical=True, installed=False, published=False)
    write_json(output / "jianying-macos-build-summary.json", summary)
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--core", type=Path)
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--include-dir", type=Path)
    parser.add_argument("--version", default="2.1.2-macos-candidate")
    parser.add_argument("--local-core-provenance", type=Path,
                        help="Required for builds: validate the current autoload Rust sources, inner binary and passed GPU receipt")
    parser.add_argument("--verify", type=Path)
    args = parser.parse_args()
    if args.verify:
        print(json.dumps(verify_delivery(args.verify.resolve(strict=True)), ensure_ascii=False, indent=2))
        return
    if sys.platform != "darwin":
        parser.error("Build this local arm64 candidate on macOS")
    if args.core is None or args.output_dir is None or args.local_core_provenance is None:
        parser.error("--core, --output-dir and --local-core-provenance are required for an autoload candidate")
    summary = build(args.core, args.output_dir, openfx_headers(args.include_dir), args.version,
                    args.local_core_provenance)
    print(json.dumps(summary, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
