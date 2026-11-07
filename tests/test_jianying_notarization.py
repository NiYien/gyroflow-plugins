# SPDX-License-Identifier: GPL-3.0-or-later
import contextlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import notarize_jianying_package as notary


class NotarizationTests(unittest.TestCase):
    credentials = ["--apple-id", "private-account", "--team-id", "TEAM", "--password", "private-password"]
    secrets = ("private-account", "private-password")

    def test_accepted_submission_records_the_actual_result(self):
        reply = {"id": "submission", "status": "Accepted"}
        with tempfile.TemporaryDirectory() as directory:
            result_path = Path(directory) / "evidence/result.json"
            with patch.object(notary.subprocess, "run", return_value=
                              subprocess.CompletedProcess([], 0, json.dumps(reply), "")) as command:
                result = notary.notarize(Path("plugin.zip"), result_path, self.credentials, self.secrets)
            self.assertEqual(result, reply)
            self.assertEqual(json.loads(result_path.read_text()), reply)
            self.assertEqual(command.call_args.args[0], ["xcrun", "notarytool", "submit", "--wait",
                             "--output-format", "json", *self.credentials, "plugin.zip"])
            self.assertEqual(command.call_count, 1)

    def test_rejection_with_zero_exit_status_fails_and_keeps_the_apple_log(self):
        reply = {"id": "submission", "status": "Invalid"}
        with tempfile.TemporaryDirectory() as directory:
            result_path = Path(directory) / "result.json"
            def service(command, **kwargs):
                if command[2] == "submit":
                    return subprocess.CompletedProcess(command, 0, json.dumps(reply), "")
                self.assertEqual(command[:4], ["xcrun", "notarytool", "log", "submission"])
                Path(command[-1]).write_text('{"issues":[{"message":"missing timestamp"}]}')
                return subprocess.CompletedProcess(command, 0, "", "")
            with patch.object(notary.subprocess, "run", side_effect=service), \
                 contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaisesRegex(RuntimeError, "returned Invalid"):
                    notary.notarize(Path("plugin.zip"), result_path, self.credentials, self.secrets)
            self.assertEqual(json.loads(result_path.read_text()), reply)
            self.assertEqual(json.loads((Path(directory) / "result-log.json").read_text())
                             ["issues"][0]["message"], "missing timestamp")

    def test_failure_to_fetch_rejection_log_preserves_the_original_rejection(self):
        responses = [subprocess.CompletedProcess([], 0, '{"id":"submission","status":"Rejected"}', ""),
                     subprocess.CompletedProcess([], 1, "", "private-password log failure")]
        with tempfile.TemporaryDirectory() as directory:
            diagnostics = io.StringIO()
            with patch.object(notary.subprocess, "run", side_effect=responses), \
                 contextlib.redirect_stderr(diagnostics):
                with self.assertRaisesRegex(RuntimeError, "returned Rejected"):
                    notary.notarize(Path("plugin.zip"), Path(directory) / "result.json",
                                    self.credentials, self.secrets)
            self.assertIn("log failure", diagnostics.getvalue())
            self.assertNotIn("private-password", diagnostics.getvalue())

    def test_submission_error_redacts_credentials_and_creates_no_success_record(self):
        response = subprocess.CompletedProcess([], 1, "private-account", "private-password failed")
        with tempfile.TemporaryDirectory() as directory:
            result_path = Path(directory) / "result.json"
            with patch.object(notary.subprocess, "run", return_value=response):
                with self.assertRaises(RuntimeError) as raised:
                    notary.notarize(Path("plugin.zip"), result_path, self.credentials, self.secrets)
            self.assertNotIn("private-account", str(raised.exception))
            self.assertNotIn("private-password", str(raised.exception))
            self.assertFalse(result_path.exists())

    def test_timeout_does_not_expose_the_command_credentials(self):
        command = ["xcrun", "notarytool", "submit", *self.credentials, "plugin.zip"]
        with patch.object(notary.subprocess, "run", side_effect=subprocess.TimeoutExpired(command, 1800)):
            with self.assertRaisesRegex(RuntimeError, "timed out") as raised:
                notary.run(command, self.secrets)
        self.assertNotIn("private-password", str(raised.exception))


if __name__ == "__main__":
    unittest.main()
