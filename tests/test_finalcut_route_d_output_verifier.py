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
PRODUCTION_KEY_PREFIX = "9999/10013/10016/3/10036"
EXPECTED_PARAMETERS = ["1", "15", "100", "0", "0", "1", "0"]


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


def visible_parameters(values=EXPECTED_PARAMETERS) -> str:
    names = [
        "FOV",
        "Smoothness",
        "Lens Correction",
        "Horizon Lock",
        "Horizon Roll",
        "Zoom Mode",
        "Stabilization Overview",
    ]
    return "".join(
        f'<param name="{name}" key="{PRODUCTION_KEY_PREFIX}/{identifier}" value="{value}"/>'
        for name, identifier, value in zip(names, range(2001, 2008), values)
    )


def fixture(
    project: bytes,
    *,
    corrupt_chunk: bool = False,
    clip_start: str = "0s",
    timing_start: str = "0/1",
    occurrence: int = 1,
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
                "occurrence": occurrence,
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
    visible = visible_parameters()
    return f'''<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE fcpxml>
<fcpxml version="1.14"><resources>
<format id="r1" frameDuration="1/30s"/>
<asset id="r2" start="0s" duration="2s" format="r1"><media-rep kind="original-media" src="file:///tmp/Clip.mov"/></asset>
<effect id="fx" uid="~/Effects.localized/NiYien/Gyroflow/Gyroflow NiYien.moef"/>
</resources><project name="Processed" uid="11111111-1111-4111-8111-111111111111"><sequence format="r1" duration="2s"><spine>
<asset-clip name="Clip" ref="r2" offset="0s" start="{clip_start}" duration="2s"><filter-video ref="fx"><param name="Instance Identity" key="9999/10013/10016/3/10036/1901" value="11111111-2222-4333-8444-555555555555"/>{bank_a}{bank_b}<param name="Project Display Name" key="9999/10013/10016/3/10036/1906" value="Clip.gyroflow"/>{visible}<param name="Timing Payload" key="9999/10013/10016/3/10036/1903" value="{timing}"/></filter-video></asset-clip>
</spine></sequence></project></fcpxml>'''


class FinalCutRouteDOutputVerifierTests(unittest.TestCase):
    def test_different_hash_requires_exact_static_manager_parameters(self):
        module = load_verifier()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sibling = root / "Clip.gyroflow"
            sibling.write_bytes(b'{"title":"new project"}')
            original = root / "Original.fcpxml"
            output = root / "Replacement.fcpxml"
            original.write_text(fixture(b'{"title":"old project"}'), encoding="utf-8")
            output.write_text(
                fixture(sibling.read_bytes()).replace(
                    f'key="{PRODUCTION_KEY_PREFIX}/2001" value="1"/>',
                    f'key="{PRODUCTION_KEY_PREFIX}/2001" value="9">'
                    '<keyframeAnimation><keyframe time="0s" value="9"/>'
                    '</keyframeAnimation></param>',
                ),
                encoding="utf-8",
            )

            with self.assertRaisesRegex(
                module.VerificationError, "different-hash visible parameter"
            ):
                module.verify_output(
                    output,
                    {"Clip": sibling},
                    {},
                    {"Clip": EXPECTED_PARAMETERS},
                    original_path=original,
                )

    def test_different_hash_rejects_display_value_keyframes_and_missing_bank(self):
        module = load_verifier()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sibling = root / "Clip.gyroflow"
            sibling.write_bytes(b'{"title":"new project"}')
            original = root / "Original.fcpxml"
            original.write_text(fixture(b'{"title":"old project"}'), encoding="utf-8")
            valid = fixture(sibling.read_bytes())
            payload = encoded_project(sibling.read_bytes())
            mutations = {
                "Display.fcpxml": valid.replace(
                    'value="Clip.gyroflow"', 'value="Wrong.gyroflow"'
                ),
                "Value.fcpxml": valid.replace(
                    f'key="{PRODUCTION_KEY_PREFIX}/2002" value="15"',
                    f'key="{PRODUCTION_KEY_PREFIX}/2002" value="16"',
                ),
                "Keyframes.fcpxml": valid.replace(
                    f'<param name="FOV" key="{PRODUCTION_KEY_PREFIX}/2001" value="1"/>',
                    f'<param name="FOV" key="{PRODUCTION_KEY_PREFIX}/2001" value="1">'
                    '<keyframeAnimation><keyframe time="0s" value="1"/>'
                    '</keyframeAnimation></param>',
                ),
                "MissingBank.fcpxml": valid.replace(
                    bank_parameters("B", 1, payload), ""
                ),
            }
            for filename, document in mutations.items():
                output = root / filename
                output.write_text(document, encoding="utf-8")
                with self.subTest(filename=filename), self.assertRaises(
                    module.VerificationError
                ):
                    module.verify_output(
                        output,
                        {"Clip": sibling},
                        {},
                        {"Clip": EXPECTED_PARAMETERS},
                        original_path=original,
                    )

    def test_equal_hash_preserves_identity_display_banks_and_visible_subtrees(self):
        module = load_verifier()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sibling = root / "Clip.gyroflow"
            sibling.write_bytes(b'{"title":"same project"}')
            original = root / "Original.fcpxml"
            original.write_text(fixture(sibling.read_bytes()), encoding="utf-8")
            payload = encoded_project(sibling.read_bytes())

            mutations = {
                "Identity.fcpxml": (
                    "11111111-2222-4333-8444-555555555555",
                    "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                ),
                "Display.fcpxml": ("Clip.gyroflow", "Changed.gyroflow"),
                "Bank.fcpxml": (
                    bank_parameters("A", 2, payload),
                    bank_parameters("A", 3, payload),
                ),
            }
            for filename, (before, after) in mutations.items():
                output = root / filename
                output.write_text(
                    original.read_text(encoding="utf-8").replace(before, after),
                    encoding="utf-8",
                )
                with self.subTest(filename=filename), self.assertRaisesRegex(
                    module.VerificationError, "equal-hash"
                ):
                    module.verify_output(
                        output,
                        {"Clip": sibling},
                        {},
                        {},
                        original_path=original,
                    )

    def test_equal_hash_allows_only_missing_or_empty_display_value_to_be_filled(self):
        module = load_verifier()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sibling = root / "Clip.gyroflow"
            sibling.write_bytes(b'{"title":"same project"}')
            canonical = (
                f'<param name="Project Display Name" '
                f'key="{PRODUCTION_KEY_PREFIX}/1906" value="Clip.gyroflow"/>'
            )
            for label, original_display in {"missing": "", "empty": canonical.replace(
                'value="Clip.gyroflow"', 'value=""'
            )}.items():
                original = root / f"Original-{label}.fcpxml"
                output = root / f"Output-{label}.fcpxml"
                original.write_text(
                    fixture(sibling.read_bytes()).replace(canonical, original_display),
                    encoding="utf-8",
                )
                output.write_text(fixture(sibling.read_bytes()), encoding="utf-8")

                result = module.verify_output(
                    output,
                    {"Clip": sibling},
                    {},
                    {},
                    original_path=original,
                )
                self.assertEqual(result["occurrences"][0]["action"], "timing_only")


    def test_wrong_prefix_and_suffix_cannot_impersonate_reserved_parameters(self):
        module = load_verifier()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "Clip.gyroflow"
            project.write_bytes(b'{"title":"exact project bytes"}')
            original = root / "Original.fcpxml"
            original.write_text(fixture(project.read_bytes()), encoding="utf-8")

            for filename, replacement in {
                "WrongPrefix.fcpxml": "untrusted/prefix",
                "WrongSuffix.fcpxml": f"{PRODUCTION_KEY_PREFIX}/shadow",
            }.items():
                output = root / filename
                output.write_text(
                    original.read_text(encoding="utf-8").replace(
                        PRODUCTION_KEY_PREFIX, replacement
                    ),
                    encoding="utf-8",
                )
                with self.subTest(filename=filename), self.assertRaises(
                    module.VerificationError
                ):
                    module.verify_output(
                        output,
                        {"Clip": project},
                        {},
                        {},
                        original_path=original,
                    )

    def test_rejects_nonproduction_resource_and_story_filter_changes(self):
        module = load_verifier()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sibling = root / "Clip.gyroflow"
            sibling.write_bytes(b'{"title":"exact sibling"}')
            original = root / "Original.fcpxml"
            output = root / "Replacement.fcpxml"
            original.write_text(fixture(b'{"title":"old embedded"}'), encoding="utf-8")
            output.write_text(fixture(sibling.read_bytes()), encoding="utf-8")

            extra_resource = root / "ExtraResource.fcpxml"
            extra_resource.write_text(
                output.read_text(encoding="utf-8").replace(
                    "</resources>",
                    '<asset id="r-extra" start="0s" duration="1s" format="r1"/>'
                    "</resources>",
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(module.VerificationError, "resources"):
                module.verify_output(
                    extra_resource,
                    {"Clip": sibling},
                    {},
                    original_path=original,
                )

            extra_filter = root / "ExtraFilter.fcpxml"
            extra_filter.write_text(
                output.read_text(encoding="utf-8").replace(
                    "</spine></sequence>",
                    '<asset-clip name="Untargeted" ref="r2" offset="3s" start="0s" duration="2s">'
                    '<filter-video ref="nonproduction"><param name="Sentinel" value="new"/>'
                    "</filter-video></asset-clip></spine></sequence>",
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(module.VerificationError, "project structure"):
                module.verify_output(
                    extra_filter,
                    {"Clip": sibling},
                    {},
                    original_path=original,
                )

    def test_route_expectations_support_same_named_occurrences(self):
        module = load_verifier()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first_sibling = root / "First.gyroflow"
            second_sibling = root / "Second.gyroflow"
            first_sibling.write_bytes(b'{"title":"first sibling"}')
            second_sibling.write_bytes(b'{"title":"second sibling"}')

            def with_second_occurrence(
                first: bytes,
                second: bytes,
                first_display: str = "Clip.gyroflow",
                second_display: str = "Clip.gyroflow",
            ) -> str:
                document = fixture(first).replace("Clip.gyroflow", first_display)
                second_document = fixture(second, occurrence=2).replace(
                    "Clip.gyroflow", second_display
                )
                start = second_document.index("<asset-clip")
                end = second_document.index("</asset-clip>", start) + len("</asset-clip>")
                second_clip = second_document[start:end].replace(
                    'offset="0s"', 'offset="3s"'
                )
                return document.replace("</spine>", second_clip + "</spine>")

            original = root / "Original.fcpxml"
            output = root / "Replacement.fcpxml"
            original.write_text(
                with_second_occurrence(b'{"title":"old first"}', b'{"title":"old second"}'),
                encoding="utf-8",
            )
            output.write_text(
                with_second_occurrence(
                    first_sibling.read_bytes(),
                    second_sibling.read_bytes(),
                    first_sibling.name,
                    second_sibling.name,
                ),
                encoding="utf-8",
            )
            routes = [
                "/fcpxml[1]/project[1]/sequence[1]/spine[1]/asset-clip[1]/filter-video[1]",
                "/fcpxml[1]/project[1]/sequence[1]/spine[1]/asset-clip[2]/filter-video[1]",
            ]

            result = module.verify_output(
                output,
                {routes[0]: first_sibling, routes[1]: second_sibling},
                {},
                {
                    routes[0]: EXPECTED_PARAMETERS,
                    routes[1]: EXPECTED_PARAMETERS,
                },
                original_path=original,
            )

            self.assertEqual(result["occurrence_count"], 2)
            self.assertEqual([item["clip_name"] for item in result["occurrences"]], ["Clip", "Clip"])

    def test_compares_output_with_original_name_and_occurrence_set(self):
        module = load_verifier()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sibling = root / "Clip.gyroflow"
            sibling.write_bytes(b'{"title":"exact sibling"}')
            original = root / "Original.fcpxml"
            output = root / "Replacement.fcpxml"
            original.write_text(
                fixture(b'{"title":"old embedded"}').replace(
                    'project name="Processed" uid="11111111-1111-4111-8111-111111111111"',
                    'project name="Original" uid="11111111-1111-4111-8111-111111111111"',
                ),
                encoding="utf-8",
            )
            output.write_text(
                fixture(sibling.read_bytes()).replace(
                    'project name="Processed" uid="11111111-1111-4111-8111-111111111111"',
                    'project name="Original" uid="aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"',
                ),
                encoding="utf-8",
            )

            result = module.verify_output(
                output,
                {"Clip": sibling},
                {},
                {"Clip": EXPECTED_PARAMETERS},
                original_path=original,
            )

            self.assertEqual(result["project_name"], "Original")
            self.assertEqual(result["project_uid"], "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")
            self.assertEqual(result["original_project_uid"], "11111111-1111-4111-8111-111111111111")

    def test_rejects_inserted_effect_and_mutated_skipped_filter(self):
        module = load_verifier()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sibling = root / "Clip.gyroflow"
            sibling.write_bytes(b'{"title":"exact sibling"}')
            original = root / "Original.fcpxml"
            output = root / "Replacement.fcpxml"
            source = fixture(b'{"title":"old embedded"}').replace(
                'project name="Processed"', 'project name="Original"'
            )
            source = source.replace(
                "</spine></sequence></project></fcpxml>",
                '<asset-clip name="Skipped" ref="r2" offset="3s" start="0s" duration="2s">'
                '<filter-video ref="fx"><param name="Sentinel" value="keep"/></filter-video>'
                "</asset-clip></spine></sequence></project></fcpxml>",
            )
            original.write_text(source, encoding="utf-8")
            output.write_text(
                fixture(sibling.read_bytes())
                .replace('project name="Processed"', 'project name="Original"')
                .replace(
                    "</spine></sequence></project></fcpxml>",
                    '<asset-clip name="Skipped" ref="r2" offset="3s" start="0s" duration="2s">'
                    '<filter-video ref="fx"><param name="Sentinel" value="changed"/></filter-video>'
                    "</asset-clip></spine></sequence></project></fcpxml>",
                ),
                encoding="utf-8",
            )

            inserted = root / "Inserted.fcpxml"
            inserted.write_text(
                output.read_text(encoding="utf-8").replace(
                    "</spine></sequence></project></fcpxml>",
                    '<asset-clip name="Inserted" ref="r2" offset="6s" start="0s" duration="2s">'
                    '<filter-video ref="fx"/></asset-clip>'
                    "</spine></sequence></project></fcpxml>",
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(module.VerificationError, "occurrence set"):
                module.verify_output(
                    inserted,
                    {"Clip": sibling},
                    {},
                    original_path=original,
                )
            with self.assertRaisesRegex(module.VerificationError, "non-allowed filter"):
                module.verify_output(
                    output,
                    {"Clip": sibling},
                    {},
                    {"Clip": EXPECTED_PARAMETERS},
                    original_path=original,
                )

    def test_equal_hash_requires_host_parameters_to_remain_literal(self):
        module = load_verifier()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sibling = root / "Clip.gyroflow"
            sibling.write_bytes(b'{"title":"same embedded bytes"}')
            original = root / "Original.fcpxml"
            output = root / "Replacement.fcpxml"
            original.write_text(
                fixture(sibling.read_bytes()).replace(
                    f'<param name="FOV" key="{PRODUCTION_KEY_PREFIX}/2001" value="1"/>',
                    f'<param name="FOV" key="{PRODUCTION_KEY_PREFIX}/2001" value="1.5">'
                    '<keyframeAnimation><keyframe time="0s" value="1.5"/>'
                    "</keyframeAnimation></param>",
                ),
                encoding="utf-8",
            )
            output.write_text(
                original.read_text(encoding="utf-8").replace('value="1.5"', 'value="2.5"'),
                encoding="utf-8",
            )

            with self.assertRaisesRegex(module.VerificationError, "equal-hash"):
                module.verify_output(
                    output,
                    {"Clip": sibling},
                    {},
                    original_path=original,
                )

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
                original_path=fcpxml,
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

            result = module.verify_output(
                fcpxml, {"Clip": project}, {}, original_path=fcpxml
            )

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
                module.verify_output(
                    fcpxml, {"Clip": project}, {}, original_path=fcpxml
                )

    def test_missing_instance_identity_fails(self):
        module = load_verifier()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "Clip.gyroflow"
            project.write_bytes(b'{"title":"exact project bytes"}')
            fcpxml = root / "Processed.fcpxml"
            original = root / "Original.fcpxml"
            original.write_text(fixture(b'{"title":"old project"}'), encoding="utf-8")
            fcpxml.write_text(
                fixture(project.read_bytes()).replace(
                    '<param name="Instance Identity" '
                    'key="9999/10013/10016/3/10036/1901" '
                    'value="11111111-2222-4333-8444-555555555555"/>',
                    "",
                ),
                encoding="utf-8",
            )

            with self.assertRaisesRegex(module.VerificationError, "1901"):
                module.verify_output(
                    fcpxml,
                    {"Clip": project},
                    {},
                    {"Clip": EXPECTED_PARAMETERS},
                    original_path=original,
                )

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
                original_path=fcpxml,
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
                module.verify_output(
                    fcpxml, {"Different": project}, {}, original_path=fcpxml
                )
            with self.assertRaisesRegex(module.VerificationError, "mapping"):
                module.verify_output(
                    fcpxml,
                    {"Clip": project},
                    {"Clip": [("0/1", "0/1"), ("2/1", "2/1")]},
                    original_path=fcpxml,
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
