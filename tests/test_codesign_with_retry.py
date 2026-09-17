# SPDX-License-Identifier: GPL-3.0-or-later
from __future__ import annotations

import importlib.util
import io
import subprocess
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from unittest.mock import call, patch


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "codesign_with_retry.py"
SPEC = importlib.util.spec_from_file_location("codesign_with_retry", SCRIPT)
signing = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(signing)


class CodesignRetryTests(unittest.TestCase):
    arguments = [
        "-vvvv", "--deep", "--strict", "--options=runtime", "--timestamp",
        "--force", "-s", "Developer ID Application: Test (TEAM)",
        "output with spaces/GyroflowNiyien.ofx.bundle",
    ]
    timestamp_error = subprocess.CompletedProcess(
        [], 1, "GyroflowNiyien.dylib: A timestamp was expected but was not found.\n"
    )
    success = subprocess.CompletedProcess([], 0, "signed successfully\n")

    def run_signing(self, results):
        output = io.StringIO()
        with patch.object(signing.subprocess, "run", side_effect=results) as run, \
                patch.object(signing.time, "sleep") as sleep, redirect_stderr(output):
            status = signing.codesign_with_retry(self.arguments)
        for invocation in run.call_args_list:
            self.assertEqual(invocation.args[0], ["codesign", *self.arguments])
            self.assertEqual(invocation.kwargs["env"]["LC_ALL"], "C")
            self.assertEqual(invocation.kwargs["stderr"], subprocess.STDOUT)
        return status, run.call_count, sleep.call_args_list, output.getvalue()

    def test_success_does_not_retry(self):
        status, attempts, sleeps, output = self.run_signing([self.success])
        self.assertEqual((status, attempts, sleeps), (0, 1, []))
        self.assertEqual(output, "signed successfully\n")

    def test_missing_timestamp_retries_without_changing_signing_arguments(self):
        status, attempts, sleeps, output = self.run_signing(
            [self.timestamp_error, self.timestamp_error, self.success]
        )
        self.assertEqual((status, attempts, sleeps), (0, 3, [call(5), call(15)]))
        self.assertEqual(output.count("A timestamp was expected"), 2)
        self.assertIn("attempt 3/4", output)
        self.assertIn("signed successfully", output)

    def test_persistent_timestamp_failure_stops_after_four_attempts(self):
        status, attempts, sleeps, output = self.run_signing([self.timestamp_error] * 4)
        self.assertEqual((status, attempts), (1, 4))
        self.assertEqual(sleeps, [call(5), call(15), call(30)])
        self.assertEqual(output.count("A timestamp was expected"), 4)

    def test_other_signing_failures_return_immediately(self):
        for message in ("The specified item could not be found in the keychain.",
                        "errSecInternalComponent", "code object is not signed at all"):
            with self.subTest(message=message):
                failure = subprocess.CompletedProcess([], 2, message)
                status, attempts, sleeps, output = self.run_signing([failure])
                self.assertEqual((status, attempts, sleeps), (2, 1, []))
                self.assertEqual(output, message)

    def test_permanent_error_after_timestamp_failure_stops_retries(self):
        failure = subprocess.CompletedProcess([], 3, "invalid signing identity\n")
        status, attempts, sleeps, output = self.run_signing([self.timestamp_error, failure])
        self.assertEqual((status, attempts, sleeps), (3, 2, [call(5)]))
        self.assertIn("invalid signing identity", output)


if __name__ == "__main__":
    unittest.main()
