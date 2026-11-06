# SPDX-License-Identifier: GPL-3.0-or-later
"""Validate an explicit local platform patch before packaging a Jianying candidate."""
from collections import Counter
import hashlib
import json
import math
from pathlib import Path
import re
import subprocess

BASE_REVISION = "e9c086ebff8a5f5ae05a11b845ef6aad29f5b0ff"
LOCAL_PACKAGE_NAMES = {"gyroflow-core", "stabilize_spirv", "neuflow-trace-shared"}
PROVENANCE_RESOURCE = "Contents/Resources/local-core-provenance.json"
GPU_VALIDATION_RESOURCE = "Contents/Resources/local-core-gpu-validation.json"
GPU_CHECKS = ("default_off_formal_e9_gpu_bit_exact", "optin_identity_integer_cpu_bit_exact",
              "roll_total_mae_u8_le_0p5", "roll_every_frame_mae_u8_le_0p5",
              "roll_at_least_99p99_percent_channels_abs_le_one",
              "roll_every_gt_one_channel_independent_phase_envelope", "roll_every_gt_one_point_at_phase_boundary",
              "all_gpu_calls_backend_wgpu", "cache_optin_and_dimension_separation", "time_matrix_direction_invariance")
GPU_EVIDENCE = ("build_provenance", "full_gpu_and_cache_results", "independent_phase_envelope", "independent_envelope_script")


def fail(message):
    raise ValueError("Local core provenance: " + message)


def required(mapping, key, kind):
    if not isinstance(mapping, dict) or key not in mapping or type(mapping[key]) is not kind:
        fail("missing or invalid " + key)
    return mapping[key]


def digest(data):
    return hashlib.sha256(data).hexdigest()


def hash_value(value):
    if not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value):
        fail("invalid SHA-256")
    return value


def absolute(value):
    if not isinstance(value, str) or not Path(value).is_absolute():
        fail("all evidence paths must be absolute")
    return Path(value).resolve(strict=True)


def relative(value):
    if (not isinstance(value, str) or not value or "\\" in value or value.startswith("/")
            or any(ord(char) < 32 for char in value)
            or any(part in ("", ".", "..") for part in value.split("/"))):
        fail("unsafe relative source path")
    return value


def checked_file(path, sha):
    path = absolute(path)
    if not path.is_file() or digest(path.read_bytes()) != hash_value(sha):
        fail("file checksum mismatch: " + str(path))
    return path


def git(repo, *arguments):
    return subprocess.check_output(["git", "-C", str(repo), *arguments])


def canonical_hash(inventory):
    return digest(json.dumps(inventory, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8"))


def inventory(root):
    if not root.is_dir():
        fail("source root is not a directory")
    result = {}
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            fail("source symlinks are not supported: " + str(path))
        if path.is_file():
            name = relative(path.relative_to(root).as_posix())
            data = path.read_bytes()
            result[name] = {"sha256": digest(data), "size": len(data)}
        elif not path.is_dir():
            fail("non-regular source entry: " + str(path))
    if not result:
        fail("empty source inventory")
    return result


def snapshot(record, live_root):
    snapshot_root = absolute(required(record, "root", str))
    manifest_path = checked_file(required(record, "manifest_path", str), required(record, "manifest_sha256", str))
    body = json.loads(manifest_path.read_bytes())
    if not isinstance(body, dict) or not body:
        fail("invalid snapshot inventory")
    for name, item in body.items():
        relative(name)
        hash_value(required(item, "sha256", str))
        if required(item, "size", int) < 0 or set(item) != {"sha256", "size"}:
            fail("invalid snapshot file metadata")
    if canonical_hash(body) != hash_value(required(record, "tree_sha256", str)):
        fail("snapshot tree digest mismatch")
    if inventory(snapshot_root) != body or inventory(live_root) != body:
        fail("live authority and snapshot inventories differ")
    return body, snapshot_root


def base_inventory(repo, revision, prefix):
    prefix = relative(prefix)
    paths = [name.decode("utf-8") for name in git(repo, "ls-tree", "-r", "-z", "--name-only", revision + ":" + prefix).split(b"\0") if name]
    for name in paths:
        relative(name)
    queries = "".join(revision + ":" + prefix + "/" + name + "\n" for name in paths).encode("utf-8")
    result = subprocess.run(["git", "-C", str(repo), "cat-file", "--batch"], input=queries,
                            capture_output=True, check=True).stdout
    offset = 0
    records = {}
    for name in paths:
        end = result.find(b"\n", offset)
        header = result[offset:end].split()
        if len(header) != 3 or header[1] != b"blob":
            fail("base source is not a Git blob: " + name)
        size = int(header[2])
        data = result[end + 1:end + 1 + size]
        if len(data) != size:
            fail("truncated Git blob")
        offset = end + size + 2
        records[name] = {"sha256": digest(data), "size": size}
    if offset != len(result) or not records:
        fail("invalid base source inventory")
    return records


def package_identities(lock):
    text = lock.read_text(encoding="utf-8")
    result = []
    for block in re.findall(r"(?ms)^\[\[package\]\]\s*\n(.*?)(?=^\[\[|\Z)", text):
        item = {}
        for key in ("name", "version", "source", "checksum"):
            values = re.findall(r"(?m)^" + key + r'\s*=\s*("[^\n]*")\s*$', block)
            if len(values) > 1:
                fail("duplicate package identity field in Cargo.lock")
            if values:
                item[key] = json.loads(values[0])
        if not isinstance(item.get("name"), str) or not isinstance(item.get("version"), str):
            fail("incomplete package identity in Cargo.lock")
        result.append(item)
    if not result:
        fail("Cargo.lock has no package identities")
    return result


def identity(item):
    if not isinstance(item, dict) or not {"name", "version"} <= set(item) or set(item) - {"name", "version", "source", "checksum"}:
        fail("invalid lock source identity record")
    for key in ("name", "version"):
        if not isinstance(item[key], str) or not item[key]:
            fail("invalid package name or version")
    for key in ("source", "checksum"):
        if item.get(key) is not None and not isinstance(item[key], str):
            fail("invalid optional package identity")
    return tuple(item.get(key) for key in ("name", "version", "source", "checksum"))


def compare_locks(formal, isolated, declared):
    original = Counter(identity(item) for item in package_identities(formal))
    patched = Counter(identity(item) for item in package_identities(isolated))
    if Counter((item[0], item[1]) for item in original.elements()) != Counter((item[0], item[1]) for item in patched.elements()):
        fail("isolated lock changes package names or versions")
    removed, added = original - patched, patched - original
    if (Counter(identity(item) for item in required(declared, "removed", list)) != removed
            or Counter(identity(item) for item in required(declared, "added", list)) != added):
        fail("declared lock source identity difference is incomplete")
    if {item[0] for item in removed.elements()} != LOCAL_PACKAGE_NAMES or {item[0] for item in added.elements()} != LOCAL_PACKAGE_NAMES:
        fail("unexpected local package source identity changes")


def validate_declaration(data):
    """Check JSON structure only; never dereference evidence paths from an archive."""
    if required(data, "schema", int) != 1 or required(data, "kind", str) != "local-platform-patch":
        fail("local-platform-patch schema 1 is required")
    if required(data, "base_core_revision", str) != BASE_REVISION:
        fail("base revision must be the approved formal core")
    opt_in = required(data, "opt_in", dict)
    if (required(opt_in, "bit", int) != 16384
            or required(opt_in, "name", str) != "INTEGER_FRAGMENT_COORDINATES"
            or required(opt_in, "default", bool) is not False
            or required(opt_in, "platform", str) != "macos"
            or required(opt_in, "required_sources", list) != ["MetalBuffer", "MetalBuffer"]):
        fail("bit 14 must be default-off and limited to two Mac MetalBuffer sources")
    if required(required(data, "gpu_validation", dict), "result", str) != "passed":
        fail("a passed GPU validation receipt is required")
    for key in ("base_core_tree", "repo_head"):
        if not re.fullmatch(r"[0-9a-f]{40}", required(data, key, str)):
            fail("invalid Git object identity")
    for key in ("authoritative_repo",):
        if not Path(required(data, key, str)).is_absolute():
            fail("authority path must be absolute")
    files = required(data, "files", list)
    if not files:
        fail("modified core files are required")
    for item in files:
        if not relative(required(item, "path", str)).startswith("src/core/"):
            fail("modified path is outside src/core")
        for key in ("sha256", "base_sha256"):
            hash_value(required(item, key, str))
    source = required(data, "source_snapshot", dict)
    for key in ("root", "manifest_path"):
        if not Path(required(source, key, str)).is_absolute():
            fail("snapshot evidence path must be absolute")
    for key in ("manifest_sha256", "tree_sha256"):
        hash_value(required(source, key, str))
    build = required(data, "isolated_build", dict)
    for key in ("workspace", "cargo_lock_path", "metadata_path", "resolved_core_manifest_path"):
        if not Path(required(build, key, str)).is_absolute():
            fail("build evidence path must be absolute")
    for key in ("cargo_lock_sha256", "workspace_cargo_toml_sha256", "formal_plugin_cargo_toml_sha256",
                "formal_plugin_lock_sha256", "metadata_sha256"):
        hash_value(required(build, key, str))
    if "source" not in build or build["source"] is not None:
        fail("resolved core source must be null")
    for side in ("removed", "added"):
        for item in required(required(build, "lock_source_identity_difference", dict), side, list):
            identity(item)
    packages = required(build, "resolved_local_packages", list)
    if len(packages) != 3 or {required(item, "name", str) for item in packages} != LOCAL_PACKAGE_NAMES:
        fail("resolved local package declarations are incomplete")
    for item in packages:
        required(item, "version", str)
        if not Path(required(item, "manifest_path", str)).is_absolute() or "source" not in item or item["source"] is not None:
            fail("local package declaration must identify an absolute path with source:null")
    dependencies = required(data, "local_dependency_sources", list)
    if len(dependencies) != 1 or required(dependencies[0], "name", str) != "neuflow-trace-shared":
        fail("external local dependency declaration is incomplete")
    for item in dependencies:
        relative(required(item, "base_git_path", str))
        for key in ("root", "snapshot_root", "manifest_path"):
            if not Path(required(item, key, str)).is_absolute():
                fail("external dependency evidence path must be absolute")
        for key in ("manifest_sha256", "tree_sha256"):
            hash_value(required(item, key, str))
        for entry in required(item, "files", list):
            relative(required(entry, "path", str))
            for key in ("sha256", "base_sha256"):
                hash_value(required(entry, key, str))
    for record, path_key in ((required(data, "inner_binary", dict), "path"),
                             (required(data, "gpu_validation", dict), "summary_path")):
        if not Path(required(record, path_key, str)).is_absolute():
            fail("binary or validation evidence path must be absolute")
        hash_value(required(record, "sha256" if path_key == "path" else "summary_sha256", str))


def validate_gpu_summary(summary, read_evidence=False):
    """Check the declared precision gate; archive callers never read external evidence."""
    if (required(summary, "schema", int) != 1
            or required(summary, "kind", str) != "macos-dual-metal-buffer-pixel-center-validation"
            or required(summary, "passed", bool) is not True or required(summary, "failures", int) != 0
            or required(summary, "default_off_equivalent", bool) is not True
            or required(summary, "identity_bit_exact", bool) is not True):
        fail("GPU validation has not passed the required baseline and opt-in checks")
    opt_in = required(summary, "opt_in", dict)
    if required(opt_in, "bit", int) != 16384 or required(opt_in, "default", bool) is not False:
        fail("GPU validation covers a different opt-in flag")
    tolerance = required(summary, "precision_tolerance", dict)
    expected = {"roll_total_mae_u8_limit": 0.5, "roll_every_frame_mae_u8_limit": 0.5,
                "roll_min_fraction_channels_abs_error_le_one": 0.9999,
                "coefficient_phases": 32, "max_neighbor_coefficient_phase_delta": 1,
                "phase_boundary_distance_limit_units": 0.01, "unorm_rounding_allowance_u8": 1,
                "identity_and_default_off_tolerance": "strict bit equality",
                "every_gt_one_channel_must_fit_independent_envelope": True}
    for key, value in expected.items():
        if key not in tolerance or type(tolerance[key]) is not type(value) or tolerance[key] != value:
            fail("GPU precision tolerance differs from the approved gate: " + key)
    checks = required(summary, "checks", dict)
    if any(required(checks, name, bool) is not True for name in GPU_CHECKS):
        fail("GPU validation has an unsuccessful required check")
    actual = required(summary, "roll_actual", dict)
    def number(key):
        value = actual.get(key)
        if type(value) not in (int, float) or not math.isfinite(value) or value < 0:
            fail("invalid GPU roll metric: " + key)
        return value
    if (number("total_mae_u8") > 0.5 or number("max_frame_mae_u8") > 0.5
            or not 0.9999 <= number("fraction_channels_abs_error_le_one") <= 1
            or number("max_nearest_axis_phase_boundary_distance_units") > 0.01
            or required(actual, "independent_value_envelope_failures", int) != 0):
        fail("GPU roll metrics exceed the approved precision gate")
    if (required(summary, "roll_bit_exact", bool) != required(actual, "bit_exact", bool)
            or (number("max_channel_error_u8") > 0 and summary["roll_bit_exact"])):
        fail("GPU roll must retain its actual non-bit-exact result")
    sources = required(summary, "source_sha256", dict)
    for name in ("src/core/stabilization/mod.rs", "src/core/gpu/wgpu_undistort.wgsl", "src/core/gpu/mod.rs"):
        hash_value(required(sources, name, str))
    evidence = required(summary, "evidence", dict)
    if set(evidence) != set(GPU_EVIDENCE):
        fail("GPU validation evidence collection is incomplete")
    for name in GPU_EVIDENCE:
        item = required(evidence, name, dict)
        path = required(item, "path", str)
        value = hash_value(required(item, "sha256", str))
        if not Path(path).is_absolute():
            fail("GPU evidence path must be absolute")
        if read_evidence:
            checked_file(path, value)


def validate_local_core_provenance(path, inner, plugin_root):
    """Validate an explicit build-time CLI receipt against live source and build files."""
    path = absolute(str(path))
    raw = path.read_bytes()
    data = json.loads(raw)
    validate_declaration(data)
    common = plugin_root / "common/Cargo.toml"
    formal_lock = plugin_root / "Cargo.lock"
    pin = re.search(r'gyroflow-core\s*=\s*\{[^\n]*\brev\s*=\s*"([0-9a-f]{40})"', common.read_text(encoding="utf-8"))
    if not pin or pin[1] != BASE_REVISION:
        fail("formal dependency pin changed")
    repo = absolute(required(data, "authoritative_repo", str))
    if git(repo, "rev-parse", "--show-toplevel").decode().strip() != str(repo):
        fail("authority must name the Git repository root")
    revision = required(data, "base_core_revision", str)
    if git(repo, "rev-parse", revision + ":src/core").decode().strip() != required(data, "base_core_tree", str):
        fail("base core Git tree mismatch")
    if git(repo, "rev-parse", "HEAD").decode().strip() != required(data, "repo_head", str):
        fail("authority Git HEAD mismatch")
    live_core = repo / "src/core"
    core_files, snapshot_root = snapshot(required(data, "source_snapshot", dict), live_core)
    baseline = base_inventory(repo, revision, "src/core")
    modified = set()
    for item in required(data, "files", list):
        name = relative(required(item, "path", str))
        if not name.startswith("src/core/") or name[9:] in modified:
            fail("invalid or duplicate modified core path")
        key = name[9:]
        if key not in baseline or key not in core_files:
            fail("modified file is outside the base core tree")
        if baseline[key]["sha256"] != hash_value(required(item, "base_sha256", str)) or core_files[key]["sha256"] != hash_value(required(item, "sha256", str)):
            fail("modified core file checksum mismatch: " + name)
        modified.add(key)
    actual_modified = {name for name, entry in baseline.items() if core_files.get(name) != entry}
    if not modified or actual_modified != modified:
        fail("modified core file declaration does not cover the tracked base differences")
    local_roots = {}
    for item in required(data, "local_dependency_sources", list):
        name = required(item, "name", str)
        if name in local_roots or name not in LOCAL_PACKAGE_NAMES - {"gyroflow-core", "stabilize_spirv"}:
            fail("unexpected or duplicate external local dependency")
        live = absolute(required(item, "root", str))
        base_path = relative(required(item, "base_git_path", str))
        if live != repo / base_path:
            fail("external local dependency is outside its authority path")
        record = {"root": required(item, "snapshot_root", str),
                  **{key: required(item, key, str) for key in ("manifest_path", "manifest_sha256", "tree_sha256")}}
        files, _ = snapshot(record, live)
        base_files = base_inventory(repo, revision, base_path)
        declared_files = {}
        for entry in required(item, "files", list):
            key = relative(required(entry, "path", str))
            if key in declared_files:
                fail("duplicate external dependency file")
            declared_files[key] = hash_value(required(entry, "sha256", str))
            if key not in base_files or base_files[key]["sha256"] != hash_value(required(entry, "base_sha256", str)):
                fail("external dependency baseline checksum mismatch")
        if files != base_files or set(declared_files) != set(files) or any(files[key]["sha256"] != value for key, value in declared_files.items()):
            fail("external local dependency must remain byte-identical to the approved base")
        local_roots[name] = live
    if set(local_roots) != {"neuflow-trace-shared"}:
        fail("external local dependency receipt is incomplete")
    build = required(data, "isolated_build", dict)
    workspace = absolute(required(build, "workspace", str))
    if workspace == plugin_root.resolve():
        fail("the build workspace must be isolated from the formal checkout")
    checked_file(str(common), required(build, "formal_plugin_cargo_toml_sha256", str))
    checked_file(str(formal_lock), required(build, "formal_plugin_lock_sha256", str))
    if "formal_workspace_cargo_toml_sha256" in build:
        checked_file(str(plugin_root / "Cargo.toml"), required(build, "formal_workspace_cargo_toml_sha256", str))
    checked_file(str(workspace / "Cargo.toml"), required(build, "workspace_cargo_toml_sha256", str))
    if "plugin_source_sha256" in data:
        for name, expected in required(data, "plugin_source_sha256", dict).items():
            name = relative(name)
            if not name.startswith("openfx/src/"):
                fail("unexpected plugin source receipt path")
            checked_file(str(plugin_root / name), expected)
            checked_file(str(workspace / name), expected)
    lock = checked_file(required(build, "cargo_lock_path", str), required(build, "cargo_lock_sha256", str))
    if lock != workspace / "Cargo.lock":
        fail("isolated lock path is not the workspace lock")
    compare_locks(formal_lock, lock, required(build, "lock_source_identity_difference", dict))
    metadata = json.loads(checked_file(required(build, "metadata_path", str), required(build, "metadata_sha256", str)).read_bytes())
    if absolute(required(metadata, "workspace_root", str)) != workspace:
        fail("Cargo metadata belongs to a different workspace")
    declared_packages = required(build, "resolved_local_packages", list)
    if {required(item, "name", str) for item in declared_packages} != LOCAL_PACKAGE_NAMES or len(declared_packages) != 3:
        fail("resolved local package list is incomplete")
    for item in declared_packages:
        name = required(item, "name", str)
        if "source" not in item or item["source"] is not None:
            fail("local package must declare source:null")
        manifest = absolute(required(item, "manifest_path", str))
        matches = [package for package in required(metadata, "packages", list)
                   if package.get("name") == name and package.get("version") == required(item, "version", str)]
        if len(matches) != 1 or matches[0].get("source", "missing") is not None or absolute(matches[0]["manifest_path"]) != manifest:
            fail("Cargo metadata does not resolve the declared local package")
        if name == "gyroflow-core" and manifest != live_core / "Cargo.toml":
            fail("core path patch does not resolve to the authority")
        if name == "stabilize_spirv" and live_core not in manifest.parents:
            fail("stabilize_spirv is outside the authority core tree")
        if name in local_roots and manifest != local_roots[name] / "Cargo.toml":
            fail("external local dependency resolved to a different source")
    if absolute(required(build, "resolved_core_manifest_path", str)) != live_core / "Cargo.toml" or "source" not in build or build["source"] is not None:
        fail("resolved core identity must be the local authority path with source:null")
    binary = required(data, "inner_binary", dict)
    if checked_file(required(binary, "path", str), required(binary, "sha256", str)) != inner.resolve(strict=True):
        fail("inner binary identity differs from --core")
    gpu = required(data, "gpu_validation", dict)
    receipt = checked_file(required(gpu, "summary_path", str), required(gpu, "summary_sha256", str))
    gpu_summary = json.loads(receipt.read_bytes())
    validate_gpu_summary(gpu_summary, read_evidence=True)
    if "precision_tolerance" in gpu and gpu["precision_tolerance"] != gpu_summary["precision_tolerance"]:
        fail("provenance and GPU precision tolerance disagree")
    gpu_sources = required(gpu_summary, "source_sha256", dict)
    for name in ("src/core/stabilization/mod.rs", "src/core/gpu/wgpu_undistort.wgsl", "src/core/gpu/mod.rs"):
        key = name[9:]
        if key not in core_files or core_files[key]["sha256"] != hash_value(required(gpu_sources, name, str)):
            fail("GPU validation source differs from the authority and snapshot: " + name)
    return {"formal_dependency_revision": revision,
            "actual_core_source": {"kind": "local-platform-patch", "base_revision": revision,
                                   "base_tree": data["base_core_tree"], "tree_sha256": data["source_snapshot"]["tree_sha256"],
                                   "authoritative_repo": str(repo), "snapshot_root": str(snapshot_root),
                                   "resolved_local_packages": declared_packages, "opt_in": data["opt_in"],
                                   "gpu_validation": {"summary": gpu_summary, "summary_sha256": gpu["summary_sha256"]}},
            "local_core_provenance": data, "local_core_provenance_sha256": digest(raw),
            "local_core_provenance_file": PROVENANCE_RESOURCE,
            "local_core_gpu_validation_file": GPU_VALIDATION_RESOURCE}
