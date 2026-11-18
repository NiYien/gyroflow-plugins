#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Load the production DLL through Describe, then release the host's module reference."""
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import build_jianying_package as build


def probe(core, fixture, temporary):
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    for name, arguments, result in [
        ("GetModuleHandleW", [wintypes.LPCWSTR], wintypes.HMODULE),
        ("LoadLibraryW", [wintypes.LPCWSTR], wintypes.HMODULE),
        ("FreeLibrary", [wintypes.HMODULE], wintypes.BOOL),
    ]:
        function = getattr(kernel, name)
        function.argtypes, function.restype = arguments, result
    os.environ["GYROFLOW_DATA_DIR"] = str(temporary / "data")
    host = ctypes.CDLL(str(fixture))
    host.fixture_set_host_name.argtypes = [ctypes.c_char_p]
    host.fixture_open.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_int, ctypes.c_int]
    host.fixture_open.restype = ctypes.c_int
    host.fixture_error.restype = ctypes.c_char_p
    host.fixture_close.restype = None
    host.fixture_reset_after_unload.restype = None
    host.fixture_new_instance.restype = ctypes.c_void_p
    host.fixture_create.argtypes = [ctypes.c_void_p]
    host.fixture_create.restype = ctypes.c_int
    host.fixture_import_value.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int, ctypes.c_double, ctypes.c_char_p]
    host.fixture_import_value.restype = ctypes.c_int
    host.fixture_notify.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_double]
    host.fixture_notify.restype = ctypes.c_int
    host.fixture_action.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_char_p, ctypes.c_double]
    host.fixture_action.restype = ctypes.c_int
    host.fixture_snapshot.argtypes = [ctypes.c_void_p]
    host.fixture_snapshot.restype = ctypes.c_char_p
    host.fixture_set_host_name(b"DaVinciResolve")
    if host.fixture_open(str(core), str(temporary / "actions.jsonl"), 0, 1) != 0:
        raise RuntimeError(host.fixture_error().decode("utf8"))
    module = kernel.GetModuleHandleW(str(core))
    if not module:
        raise RuntimeError("Loaded plugin is missing from the module table")
    host.fixture_close()
    if not kernel.FreeLibrary(module):
        raise ctypes.WinError(ctypes.get_last_error())
    retained = kernel.GetModuleHandleW(str(core))
    if retained != module:
        print(json.dumps({"retained_after_unload": False}), flush=True)
        # Old builds have live workers whose code has just been unmapped.
        os._exit(1)
    reloaded = kernel.LoadLibraryW(str(core))
    if reloaded != module:
        raise RuntimeError("Reload did not reuse the retained module")
    if not kernel.FreeLibrary(reloaded):
        raise ctypes.WinError(ctypes.get_last_error())
    host.fixture_reset_after_unload()
    if host.fixture_open(str(core), str(temporary / "reloaded-actions.jsonl"), 0, 1) != 0:
        raise RuntimeError(host.fixture_error().decode("utf8"))
    effect = host.fixture_new_instance()
    if host.fixture_create(effect) != 0:
        raise RuntimeError("CreateInstance failed after logical plugin reload")
    project = os.environ.get("GYROFLOW_JIANYING_TEST_PROJECT")
    if project:
        user, edited = b"OfxChangeUserEdited", b"OfxChangePluginEdited"
        calls = [
            host.fixture_action(effect, b"OfxActionBeginInstanceChanged", user, 30.53049),
            host.fixture_import_value(effect, b"ProjectPath", 2, 0, str(Path(project).resolve()).encode("utf8")),
            host.fixture_notify(effect, b"ProjectPath", edited, 30.53049),
            host.fixture_import_value(effect, b"ReloadProject", 0, 1, None),
            host.fixture_notify(effect, b"ReloadProject", user, 30.53049),
            host.fixture_action(effect, b"OfxActionEndInstanceChanged", user, 30.53049),
            host.fixture_action(effect, b"OfxImageEffectActionRender", user, 30.53049),
        ]
        # Resolve's Begin/End notifications are optional and return ReplyDefault.
        if calls != [14, 0, 0, 0, 0, 14, 0]:
            raise RuntimeError(f"Real-project actions failed after reload: {calls}")
        loaded = json.loads(host.fixture_snapshot(effect).decode("utf8"))["parameters"]["LoadedProject"]["value"]
        if loaded != Path(project).name:
            raise RuntimeError(f"Reload did not load the supplied real project: {loaded}")
    host.fixture_close()
    if not kernel.FreeLibrary(kernel.GetModuleHandleW(str(core))):
        raise ctypes.WinError(ctypes.get_last_error())
    print(json.dumps({"retained_after_unload": True, "same_module_on_reload": True,
                      "instance_after_reload": True, "real_project_render": bool(project)}), flush=True)


@unittest.skipUnless(sys.platform == "win32" and os.environ.get("GYROFLOW_JIANYING_TEST_CORE"),
                     "requires the built Windows OpenFX DLL")
class OpenFXModuleLifetimeTests(unittest.TestCase):
    def test_describe_workers_keep_their_module_after_host_unload(self):
        core = Path(os.environ["GYROFLOW_JIANYING_TEST_CORE"]).resolve()
        self.assertTrue(core.is_file())
        environment = build.msvc_environment()
        compiler = shutil.which("cl", path=environment["PATH"])
        self.assertIsNotNone(compiler)
        with tempfile.TemporaryDirectory(prefix="ofx-module-lifetime-") as directory:
            temporary = Path(directory)
            fixture = temporary / "host.dll"
            subprocess.run([compiler, "/nologo", "/std:c++17", "/EHsc", "/O2", "/utf-8", "/LD",
                            str(ROOT / "tests/fixtures/jianying_protocol_host.cpp"),
                            "/I", str(build.openfx_headers(environment)),
                            f"/Fo:{temporary / 'host.obj'}", f"/Fe:{fixture}"],
                           env=environment, cwd=temporary, check=True, capture_output=True)
            result = subprocess.run([sys.executable, str(Path(__file__).resolve()), "--probe",
                                     str(core), str(fixture), str(temporary)],
                                    env=environment, cwd=temporary, capture_output=True, text=True,
                                    encoding="utf8", errors="replace", timeout=60)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(json.loads(result.stdout.strip().splitlines()[-1]),
                             {"retained_after_unload": True, "same_module_on_reload": True,
                              "instance_after_reload": True,
                              "real_project_render": bool(os.environ.get("GYROFLOW_JIANYING_TEST_PROJECT"))})


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--probe":
        probe(*(Path(value).resolve() for value in sys.argv[2:5]))
    else:
        unittest.main()
