#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Compile both source ABI profiles and the native compiler/loader boundary."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class JianyingSourceContractTests(unittest.TestCase):
    def test_source_contract_and_native_module_updates(self):
        windows = sys.platform == "win32"
        if windows:
            sys.path.insert(0, str(ROOT / "scripts"))
            import build_jianying_package
            environment = build_jianying_package.msvc_environment()
            compiler = shutil.which("cl", path=environment["PATH"])
            self.assertIsNotNone(compiler, "MSVC is required for the native Windows ABI")
        else:
            environment = os.environ.copy()
            compiler = shutil.which("clang++") or shutil.which("c++")
            self.assertIsNotNone(compiler, "A C++17 compiler is required")
        helpers = ROOT / "tests/helpers"
        native = ROOT / "jianying/native"
        with tempfile.TemporaryDirectory(prefix="jianying-source-contract-") as temporary:
            output = Path(temporary)
            suffix = ".exe" if windows else ""

            def compile_file(source, destination, shared=False, revision=1):
                if windows:
                    args = [compiler, "/nologo", "/std:c++17", "/EHsc", "/O2", "/W4", "/WX", "/utf-8",
                            "/I", str(native), f"/DFIXTURE_REVISION={revision}", str(helpers / source),
                            f"/Fo:{output / (destination.stem + str(revision) + '.obj')}", f"/Fe:{destination}"]
                    if shared:
                        args.append("/LD")
                else:
                    args = [compiler, "-std=c++17", "-O2", "-Wall", "-Wextra", "-Werror", "-I", str(native),
                            f"-DFIXTURE_REVISION={revision}", str(helpers / source), "-o", str(destination)]
                    if shared:
                        args.append("-dynamiclib" if sys.platform == "darwin" else "-shared")
                        args.append("-fPIC")
                result = subprocess.run(args, cwd=output, env=environment, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

            contract = output / ("source-contract" + suffix)
            compile_file("jianying_source_contract_main.cpp", contract)
            result = subprocess.run([str(contract)], check=True, capture_output=True, text=True, env=environment)
            self.assertTrue(json.loads(result.stdout)["passed"])
            if not windows and sys.platform != "darwin":
                return
            runner = output / ("native-source" + suffix)
            compile_file("jianying_source_native_main.cpp", runner)
            digests = []
            for revision in (1, 2):
                folder = output / str(revision)
                folder.mkdir()
                library = folder / ("cccreator.dll" if windows else "libcccreator.dylib")
                compile_file("jianying_source_native_fixture.cpp", library, shared=True, revision=revision)
                digests.append(hashlib.sha256(library.read_bytes()).hexdigest())
                result = subprocess.run([str(runner), str(library)], capture_output=True, text=True, env=environment)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertTrue(json.loads(result.stdout)["compiler_rtti"])
            self.assertNotEqual(*digests, "The two native library builds must have different whole-file hashes")


if __name__ == "__main__":
    unittest.main()
