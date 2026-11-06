# SPDX-License-Identifier: GPL-3.0-or-later
"""A mismatched binary must be rejected before executing its policy query."""
import hashlib
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import jianying_macos_build_receipt as receipt


class ReceiptTests(unittest.TestCase):
    def test_hash_mismatch_never_loads_the_binary(self):
        with tempfile.TemporaryDirectory() as root:
            core = Path(root) / "owned.dylib"
            core.write_bytes(b"mismatched file")
            with mock.patch.object(receipt, "compiled_sampling") as query:
                with self.assertRaises(ValueError):
                    receipt.validate({"core_sha256": "0" * 64}, core)
                query.assert_not_called()

    def test_matching_integer_binary_is_rejected_after_source_validation(self):
        with tempfile.TemporaryDirectory() as root:
            core = Path(root) / "owned.dylib"
            core.write_bytes(b"owned compiled fixture")
            inputs = {"formal_dependency_revision": "e" * 40}
            record = {**inputs, "core_sha256": hashlib.sha256(core.read_bytes()).hexdigest()}
            with mock.patch.object(receipt, "begin", return_value=inputs), \
                 mock.patch.object(receipt, "formal_revision", return_value="e" * 40), \
                 mock.patch.object(receipt, "compiled_sampling", return_value=1) as query:
                with self.assertRaises(ValueError):
                    receipt.validate(record, core)
                query.assert_called_once()


if __name__ == "__main__":
    unittest.main()
