# SPDX-License-Identifier: GPL-3.0-or-later
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/build_with_local_core.py"
spec = importlib.util.spec_from_file_location("local_core_builder", SCRIPT)
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


class LocalCoreBuildTests(unittest.TestCase):
    def test_local_lock_retains_every_other_dependency_and_its_checksum(self):
        text = '''version = 4
[[package]]
name = "gyroflow-core"
version = "1"
source = "git+https://example/core#abc"
dependencies = ["other"]
[[package]]
name = "other"
version = "2"
source = "registry+https://example/index"
checksum = "123456"
[[package]]
name = "stabilize_spirv"
version = "1"
source = "git+https://example/core#abc"
[[package]]
name = "neuflow-trace-shared"
version = "1"
source = "git+https://example/core#abc"
'''
        result = builder.local_lock(text)
        self.assertNotIn("git+https://example/core", result)
        self.assertIn('dependencies = ["other"]', result)
        self.assertIn('checksum = "123456"', result)
        self.assertEqual(builder.other_dependencies(text), builder.other_dependencies(result))

    def test_workspace_isolated_from_formal_manifests_and_private_documents(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "plugins"
            core = Path(temporary) / 'core with spaces and #'
            root.mkdir()
            (core / "src/core").mkdir(parents=True)
            (core / "src/core/Cargo.toml").write_text('[package]\nname="gyroflow-core"\n')
            manifest = '[workspace]\nmembers=["common"]\n'
            lock = 'version = 4\n[[package]]\nname = "gyroflow-core"\nversion = "1"\nsource = "git+core"\n'
            (root / "Cargo.toml").write_text(manifest)
            (root / "Cargo.lock").write_text(lock)
            for name in ["adobe", "common", "finalcut", "frei0r", "openfx"]:
                directory = root / name
                directory.mkdir()
                (directory / "Cargo.toml").write_text('[package]\nname="example"\n')
                (directory / "PRIVATE.MD").write_text("private")
                (directory / "build").mkdir()
                (directory / "build/stale.dll").write_bytes(b"stale")
            output = Path(temporary) / "test workspace"
            builder.prepare_workspace(root, core, output)
            self.assertEqual((root / "Cargo.toml").read_text(), manifest)
            self.assertEqual((root / "Cargo.lock").read_text(), lock)
            override = (output / "Cargo.toml").read_text().split('path = ', 1)[1].split(' }')[0]
            self.assertEqual(json.loads(override), (core / "src/core").as_posix())
            self.assertFalse((output / "common/PRIVATE.MD").exists())
            self.assertFalse((output / "common/build").exists())
            with self.assertRaises(FileExistsError):
                builder.prepare_workspace(root, core, output)


if __name__ == "__main__":
    unittest.main()
