# SPDX-License-Identifier: GPL-3.0-or-later
"""Seal the actual formal OFX build inputs before and after compilation/signing."""
import argparse
import ctypes
import hashlib
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def sources(root=ROOT):
    paths = [root / "Cargo.toml", root / "Cargo.lock"]
    for name in ("common", "openfx"):
        paths.extend([root / name / "Cargo.toml", root / name / "build.rs"])
        paths.extend((root / name / "src").rglob("*.rs"))
    return {str(p.relative_to(root)): sha256(p) for p in sorted(set(paths))}


def formal_revision(root=ROOT):
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", "aarch64-apple-darwin"], cwd=root))
    core = next(p for p in metadata["packages"] if p["name"] == "gyroflow-core")
    source = core.get("source") or ""
    match = re.fullmatch(r"git\+https://github.com/NiYien/gyroflow\.git\?rev=([0-9a-f]{40})#\1", source)
    if not match:
        raise ValueError("Formal packaging requires the unchanged pinned Git core, without a path patch")
    ofx = next(p for p in metadata["packages"] if p["name"] == "gyroflow-ofx")
    node = next(n for n in metadata["resolve"]["nodes"] if n["id"] == ofx["id"])
    if "integer-fragment-sampling" in node["features"]:
        raise ValueError("Formal packaging requires core-default sampling")
    return match[1]


def declared_revision(root=ROOT):
    manifest = (root / "common/Cargo.toml").read_text()
    match = re.search(r'gyroflow-core\s*=\s*\{[^\n]*rev\s*=\s*"([0-9a-f]{40})"', manifest)
    if not match or '[patch."https://github.com/NiYien/gyroflow.git"]' in (root / "Cargo.toml").read_text():
        raise ValueError("Formal builds require the pinned Git core")
    return match[1]


def begin(root=ROOT):
    return {"schema_version": 1, "fragment_sampling_policy": "core-default",
            "formal_dependency_revision": declared_revision(root), "source_sha256": sources(root)}


def compiled_sampling(core):
    library = ctypes.CDLL(str(Path(core).resolve()))
    try:
        query = library.gyroflow_ofx_fragment_sampling_policy_v1
    except AttributeError as error:
        raise ValueError("The OFX binary does not declare its compiled sampling policy") from error
    query.argtypes = []
    query.restype = ctypes.c_uint32
    return query()


def finish(inputs, core, root=ROOT):
    if inputs != begin(root) or formal_revision(root) != inputs.get("formal_dependency_revision"):
        raise ValueError("Formal build source or sampling selection changed during the build")
    if compiled_sampling(core) != 0:
        raise ValueError("Formal delivery cannot contain the integer-sampling experiment")
    return {**inputs, "core_sha256": sha256(core)}


def validate(receipt, core, root=ROOT):
    if receipt.get("core_sha256") != sha256(core):
        raise ValueError("The formal OFX binary does not match its build receipt")
    expected = {k: v for k, v in receipt.items() if k != "core_sha256"}
    if expected != begin(root) or formal_revision(root) != expected.get("formal_dependency_revision"):
        raise ValueError("The formal OFX build receipt does not cover the current source")
    if compiled_sampling(core) != 0:
        raise ValueError("Formal delivery cannot contain the integer-sampling experiment")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["begin", "finish"])
    parser.add_argument("--inputs", type=Path, required=True)
    parser.add_argument("--core", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.action == "begin":
        args.inputs.parent.mkdir(parents=True, exist_ok=True)
        args.inputs.write_text(json.dumps(begin(), indent=2) + "\n")
    else:
        if args.core is None or args.output is None:
            parser.error("finish requires --core and --output")
        result = finish(json.loads(args.inputs.read_text()), args.core)
        args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
