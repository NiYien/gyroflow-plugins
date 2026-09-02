import base64
import hashlib
import importlib.util
import json
import tempfile
import unittest
import zlib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
VERIFIER = ROOT / "scripts" / "verify_finalcut_route_d_output.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location(
        "verify_finalcut_route_d_output", VERIFIER
    )
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def encoded_project_v1(project: bytes) -> str:
    envelope = {
        "version": 1,
        "codec": "zlib",
        "uncompressed_len": len(project),
        "content_sha256": hashlib.sha256(project).hexdigest(),
        "project_base64": base64.b64encode(zlib.compress(project)).decode("ascii"),
    }
    return base64.b64encode(
        json.dumps(envelope, separators=(",", ":")).encode("utf-8")
    ).decode("ascii")


def encoded_project(project: bytes) -> str:
    envelope = (
        b"GFPRJ2\0"
        + len(project).to_bytes(8, "big")
        + hashlib.sha256(project).digest()
        + zlib.compress(project)
    )
    return base64.b64encode(envelope).decode("ascii")


def bank_parameters(bank: str, generation: int, payload: str) -> str:
    manifest_id, chunk_id = (1904, 1910) if bank == "A" else (1905, 1930)
    manifest = base64.b64encode(
        json.dumps(
            {
                "version": 1,
                "generation": generation,
                "chunk_count": 1,
                "encoded_length": len(payload),
                "payload_sha256": hashlib.sha256(payload.encode("ascii")).hexdigest(),
            },
            separators=(",", ":"),
        ).encode("utf-8")
    ).decode("ascii")
    return (
        f'<param name="Project Payload Manifest {bank}" '
        f'key="9999/10013/10016/3/10036/{manifest_id}" value="{manifest}"/>'
        f'<param name="Project Payload {bank} 01" '
        f'key="9999/10013/10016/3/10036/{chunk_id}" value="{payload}"/>'
    )


def fixture(
    project: bytes,
    *,
    corrupt_chunk: bool = False,
    clip_start: str = "0s",
    timing_start: str = "0/1",
) -> str:
    payload = encoded_project(project)
    bank_a = bank_parameters("A", 2, payload)
    bank_b = bank_parameters("B", 1, payload)
    if corrupt_chunk:
        marker = 'name="Project Payload A 01"'
        start = bank_a.index(marker)
        value = bank_a.index(' value="', start) + len(' value="')
        replacement = "A" if bank_a[value] != "A" else "B"
        bank_a = bank_a[:value] + replacement + bank_a[value + 1 :]
    timing = base64.b64encode(
        json.dumps(
            {
                "version": 1,
                "fcpxml_version": "1.14",
                "occurrence": 1,
                "structure_sha256": "1" * 64,
                "asset_ref": "r2",
                "mapping": [
                    {"local": "0/1", "source": "1/1"},
                    {"local": "2/1", "source": "3/1"},
                ],
                "effect_bounds": {"start": timing_start, "duration": "2/1"},
                "input_bounds": {"start": timing_start, "duration": "2/1"},
                "render_ready_snapshot": True,
            },
            separators=(",", ":"),
        ).encode("utf-8")
    ).decode("ascii")
    return f'''<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE fcpxml>
<fcpxml version="1.14"><resources>
<format id="r1" frameDuration="1/30s"/>
<asset id="r2" start="0s" duration="2s" format="r1"><media-rep kind="original-media" src="file:///tmp/Clip.mov"/></asset>
<effect id="fx" uid="~/Effects.localized/NiYien/Gyroflow/Gyroflow NiYien.moef"/>
</resources><project name="Processed" uid="11111111-1111-4111-8111-111111111111"><sequence format="r1" duration="2s"><spine>
<asset-clip name="Clip" ref="r2" offset="0s" start="{clip_start}" duration="2s"><filter-video ref="fx"><param name="Instance Identity" key="9999/10013/10016/3/10036/1901" value="11111111-2222-4333-8444-555555555555"/>{bank_a}{bank_b}<param name="Timing Payload" key="9999/10013/10016/3/10036/1903" value="{timing}"/></filter-video></asset-clip>
</spine></sequence></project></fcpxml>'''


class FinalCutRouteDOutputVerifierTests(unittest.TestCase):
    def test_bank_geometry_matches_fcpxml_safe_template_budget(self):
        module = load_verifier()

        self.assertEqual(module.PROJECT_BANK_CHUNK_BYTES, 416 * 1024)
        self.assertEqual(module.PROJECT_BANK_CHUNKS, 10)

    def test_recovers_both_banks_and_exact_timing(self):
        module = load_verifier()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "Clip.gyroflow"
            project.write_bytes(b'{"title":"exact project bytes"}')
            fcpxml = root / "Processed.fcpxml"
            fcpxml.write_text(fixture(project.read_bytes()), encoding="utf-8")

            result = module.verify_output(
                fcpxml,
                {"Clip": project},
                {"Clip": [("0/1", "1/1"), ("2/1", "3/1")]},
            )

            self.assertEqual(result["occurrence_count"], 1)
            self.assertEqual(
                result["occurrences"][0]["instance_identity"],
                "11111111-2222-4333-8444-555555555555",
            )
            self.assertEqual(result["occurrences"][0]["bank_generations"], {"A": 2, "B": 1})
            self.assertEqual(
                result["occurrences"][0]["encoded_payload_sha256"],
                hashlib.sha256(encoded_project(project.read_bytes()).encode("ascii")).hexdigest(),
            )
            self.assertEqual(
                result["occurrences"][0]["project_sha256"],
                hashlib.sha256(project.read_bytes()).hexdigest(),
            )

    def test_decodes_legacy_v1_project_payload(self):
        module = load_verifier()
        project = b'{"title":"legacy exact project bytes"}'

        self.assertEqual(
            module.decode_project_payload(encoded_project_v1(project), "legacy"),
            project,
        )

    def test_accepts_exact_nonzero_fxplug_bounds_start(self):
        module = load_verifier()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "Clip.gyroflow"
            project.write_bytes(b'{"title":"absolute FxPlug bounds"}')
            fcpxml = root / "Processed.fcpxml"
            fcpxml.write_text(
                fixture(
                    project.read_bytes(),
                    clip_start="3600s",
                    timing_start="3600/1",
                ),
                encoding="utf-8",
            )

            result = module.verify_output(fcpxml, {"Clip": project}, {})

            self.assertEqual(
                result["occurrences"][0]["timing"]["effect_bounds"]["start"],
                "3600/1",
            )

    def test_corrupt_bank_fails_even_when_other_bank_is_valid(self):
        module = load_verifier()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "Clip.gyroflow"
            project.write_bytes(b'{"title":"exact project bytes"}')
            fcpxml = root / "Processed.fcpxml"
            fcpxml.write_text(
                fixture(project.read_bytes(), corrupt_chunk=True), encoding="utf-8"
            )

            with self.assertRaisesRegex(module.VerificationError, "bank A"):
                module.verify_output(fcpxml, {"Clip": project}, {})

    def test_missing_instance_identity_fails(self):
        module = load_verifier()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "Clip.gyroflow"
            project.write_bytes(b'{"title":"exact project bytes"}')
            fcpxml = root / "Processed.fcpxml"
            fcpxml.write_text(
                fixture(project.read_bytes()).replace(
                    '<param name="Instance Identity" '
                    'key="9999/10013/10016/3/10036/1901" '
                    'value="11111111-2222-4333-8444-555555555555"/>',
                    "",
                ),
                encoding="utf-8",
            )

            with self.assertRaisesRegex(module.VerificationError, "Instance Identity"):
                module.verify_output(fcpxml, {"Clip": project}, {})

    def test_verifies_exact_single_video_clip_wrapper(self):
        module = load_verifier()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "Clip.gyroflow"
            project.write_bytes(b'{"title":"exact wrapped project bytes"}')
            wrapped = fixture(project.read_bytes()).replace(
                '<asset-clip name="Clip" ref="r2" offset="0s" start="0s" duration="2s">',
                '<clip name="Clip" offset="0s" start="0s" duration="2s">'
                '<video ref="r2" offset="0s" start="0s" duration="2s"/>',
            ).replace("</filter-video></asset-clip>", "</filter-video></clip>")
            fcpxml = root / "Wrapped.fcpxml"
            fcpxml.write_text(wrapped, encoding="utf-8")

            result = module.verify_output(
                fcpxml,
                {"Clip": project},
                {"Clip": [("0/1", "1/1"), ("2/1", "3/1")]},
            )
            self.assertEqual(result["occurrences"][0]["asset_ref"], "r2")

    def test_requires_exact_expected_occurrence_set_and_mapping(self):
        module = load_verifier()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "Clip.gyroflow"
            project.write_bytes(b'{"title":"exact project bytes"}')
            fcpxml = root / "Processed.fcpxml"
            fcpxml.write_text(fixture(project.read_bytes()), encoding="utf-8")

            with self.assertRaisesRegex(module.VerificationError, "occurrence names"):
                module.verify_output(fcpxml, {"Different": project}, {})
            with self.assertRaisesRegex(module.VerificationError, "mapping"):
                module.verify_output(
                    fcpxml,
                    {"Clip": project},
                    {"Clip": [("0/1", "0/1"), ("2/1", "2/1")]},
                )

    def test_declared_project_length_bounds_decompression(self):
        module = load_verifier()
        compressed = zlib.compress(b"x" * 1_000_000)
        payload = base64.b64encode(
            json.dumps(
                {
                    "version": 1,
                    "codec": "zlib",
                    "uncompressed_len": 1,
                    "content_sha256": hashlib.sha256(b"x").hexdigest(),
                    "project_base64": base64.b64encode(compressed).decode("ascii"),
                },
                separators=(",", ":"),
            ).encode("utf-8")
        ).decode("ascii")

        with self.assertRaisesRegex(module.VerificationError, "declared length"):
            module.decode_project_payload(payload, "bomb")


if __name__ == "__main__":
    unittest.main()
