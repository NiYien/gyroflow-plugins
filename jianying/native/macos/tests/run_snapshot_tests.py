#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Run the snapshot-only native ABI scenarios without creating any GPU resources."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[4]
NATIVE = ROOT / "jianying/native/macos"
SCENARIOS = ("end", "sync", "other", "inner-failure", "missing", "type-mismatch", "render", "reentrant")
TYPES = {"ProjectPath": "OfxParamTypeString", "Smoothness": "OfxParamTypeDouble",
         "ZoomMode": "OfxParamTypeChoice", "TranslationEnabled": "OfxParamTypeBoolean", "InstanceId": "OfxParamTypeString"}


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def require(value, message):
    if not value:
        raise AssertionError(message)


def validate_log(path, scenario):
    groups = {}
    values = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if " parameter_snapshot " in line:
            fields = dict(re.findall(r"([a-z_]+)=([^\s]+)", line.split(" parameter_snapshot ", 1)[1]))
            ident = fields["snapshot_id"]
            require(ident not in groups, "Duplicate snapshot group")
            groups[ident] = fields
        elif " parameter_snapshot_value " in line:
            before, raw_value = line.split(" value_json=", 1)
            fields = dict(re.findall(r"([a-z_]+)=([^\s]+)", before.split(" parameter_snapshot_value ", 1)[1]))
            fields["value"] = json.loads(raw_value)
            fields["actual_type"] = json.loads(fields["actual_type_json"])
            values.append(fields)
    if scenario in ("other", "inner-failure", "render"):
        require(not groups and not values, "An ineligible action produced snapshot reads or groups")
        return {"groups": 0, "value_records": 0}
    require(len(groups) == (2 if scenario == "reentrant" else 1), "Unexpected snapshot group count")
    active = [ident for ident, group in groups.items() if group["stage"] == "after_inner_success"]
    require(len(active) == 1 and len(values) == 5, "Successful action must have exactly one group and five records")
    if scenario == "reentrant":
        require(sum(group["stage"] == "skipped_reentrant_snapshot" for group in groups.values()) == 1,
                "Nested action must record one skipped reentrant group")
    group = groups[active[0]]
    require(group["action"] == ("OfxActionSyncPrivateData" if scenario == "sync" else "OfxActionEndInstanceChanged"), "Wrong snapshot stage action")
    require(group["param_set_status"] == "0", "Mock parameter set must be available")
    records = {item["parameter"]: item for item in values}
    require(set(records) == set(TYPES), "Snapshot omitted or duplicated a formal parameter")
    expected = {"ProjectPath": 'fixture folder/"quoted"\\file\nclip.gyroflow',
                "Smoothness": 48.5 if scenario == "sync" else 48.0,
                "ZoomMode": 1 if scenario == "sync" else 2,
                "TranslationEnabled": 1 if scenario == "sync" else 0,
                "InstanceId": 'instance-"one"\\tail'}
    for name, record in records.items():
        require(record["snapshot_id"] == active[0] and record["action"] == group["action"]
                and record["stage"] == group["stage"] and record["effect"] == group["effect"], "Record lost its action/effect group")
        require(record["declared_type"] == TYPES[name], "Wrong declared parameter type")
        if scenario == "missing" and name == "TranslationEnabled":
            require(record["get_handle_status"] == "3" and record["value_attempted"] == "0"
                    and record["value_status"] == "-1" and record["value"] is None, "Missing parameter status was not preserved")
        elif scenario == "missing" and name == "Smoothness":
            require(record["value_attempted"] == "1" and record["value_status"] == "1"
                    and record["value"] is None, "Failed getter status was not preserved")
        elif scenario == "type-mismatch" and name == "Smoothness":
            require(record["actual_type"] == "OfxParamTypeString" and record["value_attempted"] == "0"
                    and record["value_status"] == "-1" and record["value"] is None, "Type mismatch reached typed getter")
        else:
            require(record["actual_type"] == TYPES[name] and record["value_attempted"] == "1"
                    and record["value_status"] == "0" and record["value"] == expected[name], "Typed snapshot value changed")
            if name == "Smoothness":
                require(record["value_finite"] == "1", "Finite double was not identified")
    return {"groups": len(groups), "value_records": len(values), "json_strings_escaped": True}


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
        report.update(validate_log(log, scenario), log=log.name, log_sha256=sha(log))
        results.append(report)
    names = ("jianying_macos.mm", "metal_guard.mm", "metal_guard.h", "tests/mock_snapshot_inner.cpp",
             "tests/test_snapshot.cpp", "tests/run_snapshot_tests.py")
    summary = {"scope": "Snapshot-only native mock ABI; no host, GPU or product acceptance", "passed": True,
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
