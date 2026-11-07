#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Verify forwarding performs no diagnostic parameter reads or file writes."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[4]
NATIVE = ROOT / "jianying/native/macos"
SCENARIOS = ("end", "sync", "other", "inner-failure", "missing", "type-mismatch", "render", "reentrant")


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def require(value, message):
    if not value:
        raise AssertionError(message)


def validate_log(path, scenario):
    require(not path.exists(), f"{scenario} created a diagnostic log")
    return {"diagnostic_file_absent": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--include-dir", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    headers = args.include_dir.resolve(strict=True)
    common = ["xcrun", "clang++", "-std=c++17", "-Wall", "-Wextra", "-Werror", "-arch", "arm64",
              "-mmacosx-version-min=11.0", "-I", str(headers)]
    commands = [common + ["-dynamiclib", str(NATIVE / "tests/mock_snapshot_inner.cpp"), "-o", str(output / "libgyroflow_ofx.dylib")],
                common + ["-dynamiclib", "-fobjc-arc", str(NATIVE / "jianying_macos.mm"), str(NATIVE / "metal_guard.mm"),
                          "-framework", "Foundation", "-framework", "Metal", "-o", str(output / "adapter.ofx")],
                common + [str(NATIVE / "tests/test_snapshot.cpp"), "-o", str(output / "test-snapshot")]]
    for command in commands:
        subprocess.run(command, check=True, cwd=ROOT)
    results = []
    for scenario in SCENARIOS:
        log = output / (scenario + ".log")
        result = subprocess.run([str(output / "test-snapshot"), str(output / "adapter.ofx"), str(log), scenario],
                                check=True, text=True, capture_output=True, cwd=ROOT)
        report = json.loads(result.stdout)
        report.update(validate_log(log, scenario), log=log.name, log_sha256=None)
        results.append(report)
    names = ("jianying_macos.mm", "metal_guard.mm", "metal_guard.h", "tests/mock_snapshot_inner.cpp",
             "tests/test_snapshot.cpp", "tests/run_snapshot_tests.py")
    summary = {"scope": "Quiet forwarding native mock ABI; no host, GPU or product acceptance", "passed": True,
               "groups_passed": len(results), "metal_device_created": False, "gpu_commands": 0,
               "production_inner_used": False, "permission_context": "workspace sandbox; no GPU device access",
               "sources_sha256": {str((NATIVE / name).relative_to(ROOT)): sha(NATIVE / name) for name in names},
               "binaries_sha256": {name: sha(output / name) for name in ("adapter.ofx", "libgyroflow_ofx.dylib", "test-snapshot")},
               "compile_commands": commands, "results": results}
    (output / "summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"groups_passed": len(results), "summary": str(output / "summary.json"),
                      "scope": summary["scope"]}, ensure_ascii=False))


if __name__ == "__main__":
    main()
