#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Exercise the shipped OpenFX binary through a strict Jianying-style host.

Set GYROFLOW_JIANYING_TEST_CORE to gyroflow_ofx.dll. Set
GYROFLOW_JIANYING_TEST_PROJECT to a real read-only project for rendering,
parameter packet, shared-cache and restored-instance checks.
"""
import ctypes
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import build_jianying_package as build

BEGIN = b"OfxActionBeginInstanceChanged"
END = b"OfxActionEndInstanceChanged"
RENDER = b"OfxImageEffectActionRender"
USER = b"OfxChangeUserEdited"
PLUGIN = b"OfxChangePluginEdited"
INTERACTIVE = {"Browse", "LoadLens", "LoadCurrent", "OpenGyroflow", "OpenPluginUpdater", "OpenRecentProject"}


def native_environment_value(name):
    getter = ctypes.windll.kernel32.GetEnvironmentVariableW
    getter.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_uint]
    getter.restype = ctypes.c_uint
    value = ctypes.create_unicode_buffer(32768)
    length = getter(name, value, len(value))
    return value.value if length else None


@unittest.skipUnless(sys.platform == "win32" and os.environ.get("GYROFLOW_JIANYING_TEST_CORE"),
                     "requires the built Windows OpenFX DLL")
class JianyingProtocolTests(unittest.TestCase):
    def test_real_entry_descriptors_packets_and_restored_instances(self):
        core = Path(os.environ["GYROFLOW_JIANYING_TEST_CORE"]).resolve()
        self.assertTrue(core.is_file())
        environment = build.msvc_environment()
        headers = build.openfx_headers(environment)
        import shutil
        compiler = shutil.which("cl", path=environment["PATH"])
        self.assertIsNotNone(compiler)
        with tempfile.TemporaryDirectory(prefix="jianying-protocol-") as temporary:
            temporary = Path(temporary)
            fixture = temporary / "host.dll"
            subprocess.run([compiler, "/nologo", "/std:c++17", "/EHsc", "/O2", "/utf-8", "/LD",
                            str(ROOT / "tests/fixtures/jianying_protocol_host.cpp"), "/I", str(headers),
                            f"/Fo:{temporary / 'host.obj'}", f"/Fe:{fixture}"],
                           env=environment, cwd=temporary, check=True, capture_output=True)
            library = ctypes.CDLL(str(fixture))
            declarations = {
                "fixture_open": ([ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_int, ctypes.c_int], ctypes.c_int),
                "fixture_new_instance": ([], ctypes.c_void_p),
                "fixture_create": ([ctypes.c_void_p], ctypes.c_int),
                "fixture_import_value": ([ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int, ctypes.c_double, ctypes.c_char_p], ctypes.c_int),
                "fixture_notify": ([ctypes.c_void_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_double], ctypes.c_int),
                "fixture_action": ([ctypes.c_void_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_double], ctypes.c_int),
                "fixture_snapshot": ([ctypes.c_void_p], ctypes.c_char_p),
                "fixture_browser_metadata": ([], ctypes.c_char_p),
                "fixture_error": ([], ctypes.c_char_p),
                "fixture_close": ([], None),
            }
            for name, (arguments, result) in declarations.items():
                getattr(library, name).argtypes = arguments
                getattr(library, name).restype = result
            # The production plugin must choose its GPU without launcher environment.
            saved = {key: os.environ.pop(key, None) for key in ["NO_OPENCL", "NO_WGPU", "GYROFLOW_DATA_DIR"]}
            os.environ["GYROFLOW_DATA_DIR"] = str(temporary / "data")
            opened = False
            try:
                status = library.fixture_open(str(core), str(temporary / "trace.jsonl"), 1, 1)
                self.assertEqual(status, 0, library.fixture_error().decode("utf-8"))
                opened = True
                if core.name == build.WRAPPER:
                    browser = json.loads(library.fixture_browser_metadata().decode("utf-8"))
                    self.assertEqual(browser, {"identifier": "xyz.NiYien.gyroflow", "label": "Gyroflow", "grouping": "NiYien"})
                for name in ["NO_OPENCL", "NO_WGPU"]:
                    self.assertIsNone(native_environment_value(name), f"Describe changed {name}")
                instance = library.fixture_new_instance()
                self.assertEqual(library.fixture_create(instance), 0)

                def snapshot(handle):
                    return json.loads(library.fixture_snapshot(handle).decode("utf-8"))

                def action(handle, name):
                    self.assertEqual(library.fixture_action(handle, name, USER, 30.53049), 0)

                def set_value(handle, name, value):
                    kind = snapshot(handle)["parameters"][name]["type"]
                    string = str(value).encode("utf-8") if kind == "OfxParamTypeString" else None
                    number = 0 if string is not None else float(value)
                    tag = 2 if string is not None else 1 if kind == "OfxParamTypeDouble" else 0
                    self.assertEqual(library.fixture_import_value(handle, name.encode(), tag, number, string), 0)

                def notify(handle, name, reason=USER):
                    self.assertEqual(library.fixture_notify(handle, name.encode(), reason, 30.53049), 0)

                parameters = snapshot(instance)["parameters"]
                for name in ["Status", "LoadedProject", "LoadedLens", "LoadedPreset", "TranslationStatus"]:
                    self.assertEqual(parameters[name]["string_mode"], "OfxParamStringIsLabel")
                leaves = {name for name, parameter in parameters.items()
                          if parameter["type"] not in {"OfxParamTypeGroup", "OfxParamTypePage"}}
                self.assertEqual(set(parameters["Main"]["page_children"]), leaves)
                self.assertFalse(parameters["Browse"]["secret"])
                supplied_project = os.environ.get("GYROFLOW_JIANYING_TEST_PROJECT")
                if supplied_project:
                    project = Path(supplied_project).resolve()
                    self.assertTrue(project.is_file())
                    action(instance, BEGIN)
                    set_value(instance, "ProjectPath", str(project))
                    notify(instance, "ProjectPath", PLUGIN)
                    set_value(instance, "ReloadProject", 1)
                    notify(instance, "ReloadProject")
                    action(instance, END)
                    before = snapshot(instance)
                    self.assertEqual(before["parameters"]["LoadedProject"]["value"], project.name)
                    action(instance, RENDER)
                    self.assertEqual(snapshot(instance)["forbidden_writes"], before["forbidden_writes"])
                    action(instance, BEGIN)
                    set_value(instance, "Smoothness", 48)
                    notify(instance, "Smoothness")
                    action(instance, END)
                    for mode, button in [(2, "UseStaticZoom"), (1, "UseDynamicZoom"), (2, "UseStaticZoom")]:
                        values = {name: item["value"] for name, item in snapshot(instance)["parameters"].items()
                                  if item["value"] is not None}
                        baseline = int(values[button])
                        # A sorted full packet still contains its old hidden ZoomMode.
                        action(instance, BEGIN)
                        for name in sorted(values):
                            set_value(instance, name, baseline + 1 if name == button else values[name])
                            if name not in INTERACTIVE:
                                notify(instance, name)
                        action(instance, END)
                        self.assertEqual(snapshot(instance)["parameters"]["ZoomMode"]["value"], mode)
                        self.assertEqual(snapshot(instance)["parameters"][button]["value"], baseline)
                        self.assertEqual(snapshot(instance)["parameters"]["Smoothness"]["value"], 48)
                    saved_values = snapshot(instance)["parameters"]
                    restored = library.fixture_new_instance()
                    for name, item in saved_values.items():
                        if item["value"] is not None:
                            set_value(restored, name, item["value"])
                    self.assertEqual(library.fixture_create(restored), 0)
                    action(restored, RENDER)
                    restored_values = snapshot(restored)["parameters"]
                    for name in ["ProjectPath", "Smoothness", "ZoomMode"]:
                        self.assertEqual(restored_values[name]["value"], saved_values[name]["value"])
                    self.assertTrue(restored_values["UseDynamicZoom"]["enabled"])
                    self.assertFalse(restored_values["UseStaticZoom"]["enabled"])
                for name in ["NO_OPENCL", "NO_WGPU"]:
                    self.assertIsNone(native_environment_value(name), f"Render changed {name}")
            finally:
                if opened:
                    library.fixture_close()
                import _ctypes
                _ctypes.FreeLibrary(library._handle)
                for key, value in saved.items():
                    if value is not None:
                        os.environ[key] = value
                    else:
                        os.environ.pop(key, None)


if __name__ == "__main__":
    unittest.main()
