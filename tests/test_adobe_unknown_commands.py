"""Check that the built Adobe plugin ignores commands outside the public SDK."""

import ctypes
import os
import pathlib
import subprocess
import sys
import unittest


class UnknownCommandsTest(unittest.TestCase):
    @unittest.skipUnless(
        sys.platform == "win32" and os.environ.get("GYROFLOW_ADOBE_TEST_PLUGIN"),
        "Set GYROFLOW_ADOBE_TEST_PLUGIN to the built Windows Adobe plugin",
    )
    def test_unknown_commands_return_success_without_host_data(self):
        plugin = pathlib.Path(os.environ["GYROFLOW_ADOBE_TEST_PLUGIN"]).resolve(strict=True)
        for command in (27, -1, 2147483647):
            with self.subTest(command=command):
                result = subprocess.run(
                    [sys.executable, __file__, "--invoke", str(plugin), str(command)],
                    capture_output=True, text=True, timeout=30,
                )
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--invoke":
        library = ctypes.CDLL(sys.argv[2])
        entry = library.EffectMain
        entry.argtypes = [ctypes.c_int, *([ctypes.c_void_p] * 5)]
        entry.restype = ctypes.c_int
        result = entry(int(sys.argv[3]), None, None, None, None, None)
        if result != 0:
            raise SystemExit(f"Unknown command returned error {result}")
    else:
        unittest.main(argv=[sys.argv[0]], verbosity=2)
