#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Compare CPU/WGPU and D3D11/WGPU through the actual published OpenFX entry."""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import statistics
import shutil
import subprocess
import sys
import time


def process_environment(name):
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.GetEnvironmentVariableW.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_uint]
    kernel.GetEnvironmentVariableW.restype = ctypes.c_uint
    ctypes.set_last_error(0)
    size = kernel.GetEnvironmentVariableW(name, None, 0)
    if not size:
        return None if ctypes.get_last_error() == 203 else ""
    value = ctypes.create_unicode_buffer(size)
    kernel.GetEnvironmentVariableW(name, value, size)
    return value.value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--core", type=Path, required=True)
    parser.add_argument("--fixture", type=Path)
    parser.add_argument("--project", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    output = arguments.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    fixture = arguments.fixture
    if fixture is None:
        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
        import build_jianying_package as builder
        environment = builder.msvc_environment()
        compiler = shutil.which("cl", path=environment["PATH"])
        fixture = output / "ofx_jianying_d3d11_host.dll"
        source = Path(__file__).with_name("ofx_jianying_d3d11_host.cpp")
        subprocess.run([compiler, "/nologo", "/std:c++17", "/EHsc", "/O2", "/utf-8", "/LD",
                        str(source), "/I", str(builder.openfx_headers(environment)),
                        f"/Fo:{output / 'd3d11_host.obj'}", f"/Fe:{fixture}", "/link", "d3d11.lib", "dxgi.lib"],
                       env=environment, check=True)
    os.environ["GYROFLOW_DATA_DIR"] = str(output / "data")
    os.environ.pop("NO_OPENCL", None)
    os.environ.pop("NO_WGPU", None)
    library = ctypes.CDLL(str(fixture.resolve()))
    declarations = {
        "fixture_open": ([ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_int, ctypes.c_int], ctypes.c_int),
        "fixture_new_instance": ([], ctypes.c_void_p),
        "fixture_create": ([ctypes.c_void_p], ctypes.c_int),
        "fixture_import_value": ([ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int, ctypes.c_double, ctypes.c_char_p], ctypes.c_int),
        "fixture_action": ([ctypes.c_void_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_double], ctypes.c_int),
        "fixture_gpu_resize": ([ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_double], ctypes.c_int),
        "fixture_gpu_frame": ([ctypes.c_void_p, ctypes.c_uint], ctypes.c_int),
        "fixture_gpu_render": ([ctypes.c_void_p, ctypes.c_double, ctypes.c_int], ctypes.c_int),
        "fixture_gpu_poison_cpu_source": ([ctypes.c_void_p], None),
        "fixture_gpu_read": ([ctypes.c_void_p, ctypes.POINTER(ctypes.c_size_t)], ctypes.c_void_p),
        "fixture_output_data": ([ctypes.c_void_p, ctypes.POINTER(ctypes.c_size_t)], ctypes.c_void_p),
        "fixture_source_data": ([ctypes.c_void_p, ctypes.POINTER(ctypes.c_size_t)], ctypes.c_void_p),
        "fixture_snapshot": ([ctypes.c_void_p], ctypes.c_char_p),
        "fixture_error": ([], ctypes.c_char_p),
        "fixture_close": ([], None),
    }
    for name, (inputs, result) in declarations.items():
        function = getattr(library, name)
        function.argtypes, function.restype = inputs, result
    assert library.fixture_open(str(arguments.core.resolve()), str(output / "trace.jsonl"), 1, 1) == 0, library.fixture_error()
    project_data = json.loads(arguments.project.read_text(encoding="utf-8"))
    fps = float(project_data["video_info"]["fps"])
    instance = library.fixture_new_instance()
    assert instance
    assert library.fixture_import_value(instance, b"ProjectPath", 2, 0, str(arguments.project.resolve()).encode("utf-8")) == 0
    assert library.fixture_create(instance) == 0
    records = []
    try:
        for width, height, count in [(541, 305, 4), (1920, 1080, 4), (3840, 2160, 14)]:
            assert library.fixture_gpu_resize(instance, width, height, fps) == 0
            for frame in range(count):
                assert library.fixture_gpu_frame(instance, frame) == 0
                length = ctypes.c_size_t()
                pointer = library.fixture_source_data(instance, ctypes.byref(length))
                source_frame = ctypes.string_at(pointer, length.value)
                request_time = 90 + frame
                for action in [b"OfxActionBeginInstanceChanged", b"OfxActionEndInstanceChanged"]:
                    assert library.fixture_action(instance, action, b"OfxChangeUserEdited", request_time) == 0
                timings = []
                images = []
                for directx in [0, 1]:
                    start = time.perf_counter()
                    assert library.fixture_gpu_render(instance, request_time, directx) == 0
                    timings.append((time.perf_counter() - start) * 1000)
                    length = ctypes.c_size_t()
                    getter = library.fixture_gpu_read if directx else library.fixture_output_data
                    pointer = getter(instance, ctypes.byref(length))
                    assert pointer and length.value == width * height * 4
                    images.append(ctypes.string_at(pointer, length.value))
                    if not directx:
                        library.fixture_gpu_poison_cpu_source(instance)
                assert images[0] == images[1], (width, height, frame, "CPU/D3D11 mismatch")
                assert images[0] != source_frame, "Unexpected passthrough"
                records.append({"width": width, "height": height, "frame": frame,
                                "rgba_sha256": hashlib.sha256(images[0]).hexdigest(),
                                "cpu_wgpu_ms": timings[0], "d3d11_wgpu_ms": timings[1]})
        trimmed_project = output / "trim-reference.gyroflow"
        project_data["trim_ranges_ms"] = [[0.0, 100.0]]
        trimmed_project.write_text(json.dumps(project_data, ensure_ascii=False), encoding="utf-8")
        trimmed = library.fixture_new_instance()
        assert library.fixture_import_value(trimmed, b"ProjectPath", 2, 0, str(trimmed_project).encode("utf-8")) == 0
        assert library.fixture_create(trimmed) == 0
        trim_results = []
        for width, height in [(541, 305), (1920, 1080), (3840, 2160)]:
            assert library.fixture_gpu_resize(trimmed, width, height, fps) == 0
            assert library.fixture_gpu_frame(trimmed, 101) == 0
            for action in [b"OfxActionBeginInstanceChanged", b"OfxActionEndInstanceChanged"]:
                assert library.fixture_action(trimmed, action, b"OfxChangeUserEdited", 90.0) == 0
            length = ctypes.c_size_t()
            pointer = library.fixture_source_data(trimmed, ctypes.byref(length))
            source = ctypes.string_at(pointer, length.value)
            for directx in [0, 1]:
                assert library.fixture_gpu_render(trimmed, 90.0, directx) == 0
                getter = library.fixture_gpu_read if directx else library.fixture_output_data
                pointer = getter(trimmed, ctypes.byref(length))
                assert ctypes.string_at(pointer, length.value) == source, (width, height, directx, "trim copy mismatch")
                if not directx:
                    library.fixture_gpu_poison_cpu_source(trimmed)
            trim_results.append({"width": width, "height": height, "rgba_sha256": hashlib.sha256(source).hexdigest()})
        snapshot = json.loads(library.fixture_snapshot(instance).decode("utf-8"))
        assert snapshot["forbidden_writes"] == 0
        assert len({row["rgba_sha256"] for row in records}) == len(records)
        assert process_environment("NO_OPENCL") is None, "Plugin changed process NO_OPENCL"
        assert process_environment("NO_WGPU") is None, "Plugin changed process NO_WGPU"
        steady = [row for row in records if row["width"] == 3840 and row["frame"] >= 2]
        report = {"core_sha256": hashlib.sha256(arguments.core.read_bytes()).hexdigest(),
                  "project_sha256": hashlib.sha256(arguments.project.read_bytes()).hexdigest(),
                  "fixture_sha256": hashlib.sha256(fixture.read_bytes()).hexdigest(),
                  "exact_cpu_d3d_pixels": len(records), "trim_sizes": len(trim_results), "trim_results": trim_results,
                  "forbidden_parameter_writes": 0,
                  "launcher_environment_required": False,
                  "median_4k_ms": {name: statistics.median(row[name] for row in steady)
                                   for name in ["cpu_wgpu_ms", "d3d11_wgpu_ms"]},
                  "frames": records, "host_verified": False}
        (output / "result.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({key: value for key, value in report.items() if key != "frames"}, indent=2))
    finally:
        library.fixture_close()


if __name__ == "__main__":
    main()
