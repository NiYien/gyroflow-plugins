#!/usr/bin/env python3
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import sys
import xml.etree.ElementTree as ET
import uuid
import zlib
from fractions import Fraction
from pathlib import Path


PRODUCTION_EFFECT_UIDS = {
    "ABAD71F5-23F5-46F6-AB08-C11603168AA4",
    "~/Effects.localized/NiYien/Gyroflow/Gyroflow NiYien.moef",
}
PROJECT_BANK_CHUNK_BYTES = 416 * 1024
PROJECT_BANK_CHUNKS = 10
PROJECT_BANK_MAXIMUM_BYTES = 4 * 1024 * 1024
MAX_PROJECT_BYTES = 256 * 1024 * 1024


class VerificationError(ValueError):
    pass


def fail(message: str) -> None:
    raise VerificationError(message)


def exactly_one(items, context: str):
    items = list(items)
    if len(items) != 1:
        fail(f"{context}: expected exactly one, found {len(items)}")
    return items[0]


def strict_base64(value: str, context: str) -> bytes:
    try:
        return base64.b64decode(value, validate=True)
    except (ValueError, base64.binascii.Error) as error:
        fail(f"{context}: invalid Base64: {error}")


def base64_json(value: str, context: str) -> dict:
    try:
        decoded = json.loads(strict_base64(value, context))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"{context}: invalid JSON: {error}")
    if not isinstance(decoded, dict):
        fail(f"{context}: expected a JSON object")
    return decoded


def lowercase_sha256(value) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in "0123456789abcdef" for character in value)
    )


def positive_int(value) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def decode_project_payload(payload: str, context: str) -> bytes:
    decoded = strict_base64(payload, f"{context} payload")
    if decoded.startswith(b"GFPRJ2\0"):
        if len(decoded) <= 47:
            fail(f"{context}: truncated project payload v2 envelope")
        length = int.from_bytes(decoded[7:15], "big")
        digest = decoded[15:47].hex()
        compressed = decoded[47:]
    else:
        try:
            envelope = json.loads(decoded)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            fail(f"{context}: invalid project payload envelope: {error}")
        if not isinstance(envelope, dict):
            fail(f"{context}: expected a project payload object")
        if envelope.get("version") != 1 or envelope.get("codec") != "zlib":
            fail(f"{context}: unknown project payload version or codec")
        length = envelope.get("uncompressed_len")
        digest = envelope.get("content_sha256")
        encoded_project = envelope.get("project_base64")
        if not isinstance(encoded_project, str):
            fail(f"{context}: invalid project payload bounds")
        compressed = strict_base64(encoded_project, f"{context} compressed project")
    if (
        not positive_int(length)
        or length > MAX_PROJECT_BYTES
        or not lowercase_sha256(digest)
    ):
        fail(f"{context}: invalid project payload bounds")
    try:
        decompressor = zlib.decompressobj()
        project = decompressor.decompress(compressed, length + 1)
    except zlib.error as error:
        fail(f"{context}: invalid zlib project: {error}")
    if len(project) > length or not decompressor.eof:
        fail(f"{context}: project exceeds its declared length")
    try:
        project += decompressor.flush()
    except zlib.error as error:
        fail(f"{context}: invalid zlib project: {error}")
    if (
        not decompressor.eof
        or decompressor.unused_data
        or decompressor.unconsumed_tail
        or len(project) != length
        or hashlib.sha256(project).hexdigest() != digest
    ):
        fail(f"{context}: project length or SHA-256 mismatch")
    return project


def decode_bank(
    filter_video: ET.Element, bank: str, context: str
) -> tuple[int, str, str, bytes]:
    manifest_id, chunk_start = (1904, 1910) if bank == "A" else (1905, 1930)
    parameters = list(filter_video.findall("param"))
    manifest = exactly_one(
        (
            parameter
            for parameter in parameters
            if parameter.get("name") == f"Project Payload Manifest {bank}"
        ),
        f"{context} bank {bank} manifest",
    )
    encoded_manifest = manifest.get("value", "")
    decoded_manifest = base64_json(encoded_manifest, f"{context} bank {bank} manifest")
    generation = decoded_manifest.get("generation")
    chunk_count = decoded_manifest.get("chunk_count")
    encoded_length = decoded_manifest.get("encoded_length")
    payload_digest = decoded_manifest.get("payload_sha256")
    if (
        decoded_manifest.get("version") != 1
        or not positive_int(generation)
        or not positive_int(chunk_count)
        or chunk_count > PROJECT_BANK_CHUNKS
        or not positive_int(encoded_length)
        or encoded_length > PROJECT_BANK_MAXIMUM_BYTES
        or chunk_count
        != (encoded_length + PROJECT_BANK_CHUNK_BYTES - 1)
        // PROJECT_BANK_CHUNK_BYTES
        or not lowercase_sha256(payload_digest)
    ):
        fail(f"{context} bank {bank}: invalid manifest bounds")
    manifest_key = manifest.get("key", "")
    suffix = f"/{manifest_id}"
    if not manifest_key.endswith(suffix):
        fail(f"{context} bank {bank}: unknown manifest key")
    key_prefix = manifest_key[: -len(suffix)]

    chunks = []
    for index in range(chunk_count):
        chunk = exactly_one(
            (
                parameter
                for parameter in parameters
                if parameter.get("name") == f"Project Payload {bank} {index + 1:02}"
            ),
            f"{context} bank {bank} chunk {index + 1}",
        )
        if chunk.get("key") != f"{key_prefix}/{chunk_start + index}":
            fail(f"{context} bank {bank} chunk {index + 1}: unknown key")
        value = chunk.get("value")
        expected_length = (
            PROJECT_BANK_CHUNK_BYTES
            if index + 1 < chunk_count
            else encoded_length - PROJECT_BANK_CHUNK_BYTES * (chunk_count - 1)
        )
        if value is None or not value.isascii() or len(value) != expected_length:
            fail(f"{context} bank {bank} chunk {index + 1}: invalid length")
        chunks.append(value)
    payload = "".join(chunks)
    if (
        len(payload) != encoded_length
        or hashlib.sha256(payload.encode("ascii")).hexdigest() != payload_digest
    ):
        fail(f"{context} bank {bank}: payload SHA-256 mismatch")
    return (
        generation,
        key_prefix,
        payload_digest,
        decode_project_payload(payload, f"{context} bank {bank}"),
    )


def decode_instance_identity(
    filter_video: ET.Element, key_prefix: str, context: str
) -> str:
    parameter = exactly_one(
        (
            item
            for item in filter_video.findall("param")
            if item.get("name") == "Instance Identity"
        ),
        f"{context} Instance Identity",
    )
    if parameter.get("key") != f"{key_prefix}/1901":
        fail(f"{context}: Instance Identity key is unknown")
    value = parameter.get("value", "")
    try:
        parsed = uuid.UUID(value)
    except (ValueError, AttributeError) as error:
        fail(f"{context}: Instance Identity is invalid: {error}")
    if parsed.version != 4 or str(parsed).upper() != value.upper():
        fail(f"{context}: Instance Identity is not a canonical UUIDv4")
    return str(parsed).upper()


def parse_rational(value, context: str) -> Fraction:
    if not isinstance(value, str) or value.count("/") != 1:
        fail(f"{context}: invalid rational")
    try:
        return Fraction(value)
    except (ValueError, ZeroDivisionError) as error:
        fail(f"{context}: invalid rational: {error}")


def parse_fcpxml_time(value, context: str) -> Fraction:
    if not isinstance(value, str) or not value.endswith("s") or len(value) == 1:
        fail(f"{context}: invalid FCPXML time")
    try:
        return Fraction(value[:-1])
    except (ValueError, ZeroDivisionError) as error:
        fail(f"{context}: invalid FCPXML time: {error}")


def decode_timing(
    filter_video: ET.Element,
    asset_ref: str,
    fcpxml_version: str,
    expected_bounds_start: Fraction,
    context: str,
) -> dict:
    parameter = exactly_one(
        (
            item
            for item in filter_video.findall("param")
            if item.get("name") == "Timing Payload"
        ),
        f"{context} timing payload",
    )
    timing = base64_json(parameter.get("value", ""), f"{context} timing payload")
    if (
        timing.get("version") != 1
        or timing.get("fcpxml_version") != fcpxml_version
        or not positive_int(timing.get("occurrence"))
        or not lowercase_sha256(timing.get("structure_sha256"))
        or timing.get("asset_ref") != asset_ref
        or timing.get("render_ready_snapshot") is not True
    ):
        fail(f"{context}: invalid timing envelope")
    mapping = timing.get("mapping")
    if not isinstance(mapping, list) or len(mapping) < 2:
        fail(f"{context}: timing mapping requires at least two points")
    rational_mapping = []
    for index, point in enumerate(mapping):
        if not isinstance(point, dict):
            fail(f"{context}: timing mapping point {index + 1} is not an object")
        local = point.get("local")
        source = point.get("source")
        rational_mapping.append(
            (
                parse_rational(local, f"{context} local mapping point {index + 1}"),
                parse_rational(source, f"{context} source mapping point {index + 1}"),
            )
        )
    if rational_mapping[0][0] != 0 or any(
        first[0] >= second[0]
        for first, second in zip(rational_mapping, rational_mapping[1:])
    ):
        fail(f"{context}: timing local mapping is not strictly increasing from zero")
    effect_bounds = timing.get("effect_bounds")
    input_bounds = timing.get("input_bounds")
    if not isinstance(effect_bounds, dict) or input_bounds != effect_bounds:
        fail(f"{context}: timing effect/input bounds differ")
    if (
        parse_rational(effect_bounds.get("start"), f"{context} bounds start")
        != expected_bounds_start
        or parse_rational(effect_bounds.get("duration"), f"{context} bounds duration")
        != rational_mapping[-1][0]
    ):
        fail(f"{context}: timing bounds do not cover the mapping")
    return timing


def resolve_fcpxml(path: Path) -> Path:
    path = path.resolve()
    if path.is_dir():
        if path.suffix.lower() != ".fcpxmld":
            fail(f"input directory is not an .fcpxmld package: {path}")
        candidate = path / "Info.fcpxml"
        if not candidate.is_file() or candidate.is_symlink():
            fail(f"package must contain one regular root Info.fcpxml: {path}")
        return candidate
    if path.suffix.lower() != ".fcpxml" or not path.is_file() or path.is_symlink():
        fail(f"input is not one regular .fcpxml file: {path}")
    return path


def nearest_ancestor(
    element: ET.Element, parent_map: dict[ET.Element, ET.Element], tag: str
) -> ET.Element | None:
    current = parent_map.get(element)
    while current is not None:
        if current.tag == tag:
            return current
        current = parent_map.get(current)
    return None


def supported_clip_and_asset_ref(
    element: ET.Element, parent_map: dict[ET.Element, ET.Element], context: str
) -> tuple[ET.Element, str]:
    current = parent_map.get(element)
    while current is not None:
        if current.tag == "asset-clip":
            asset_ref = current.get("ref")
            if not asset_ref:
                fail(f"{context}: asset-clip has no asset ref")
            return current, asset_ref
        if current.tag == "clip":
            clip_item_tags = {
                "video",
                "audio",
                "asset-clip",
                "clip",
                "ref-clip",
                "sync-clip",
                "mc-clip",
                "gap",
                "title",
                "caption",
                "spine",
            }
            clip_items = [child for child in current if child.tag in clip_item_tags]
            if len(clip_items) != 1 or clip_items[0].tag != "video":
                fail(f"{context}: clip wrapper is not one exact direct video item")
            asset_ref = clip_items[0].get("ref")
            if not asset_ref:
                fail(f"{context}: clip wrapper video has no asset ref")
            return current, asset_ref
        current = parent_map.get(current)
    fail(f"{context}: filter has no supported clip ancestor")


def verify_output(
    input_path: Path,
    expected_projects: dict[str, Path],
    expected_mappings: dict[str, list[tuple[str, str]]],
) -> dict:
    xml_path = resolve_fcpxml(Path(input_path))
    try:
        xml_bytes = xml_path.read_bytes()
        root = ET.fromstring(xml_bytes)
    except (OSError, ET.ParseError) as error:
        fail(f"unable to parse FCPXML: {error}")
    if root.tag != "fcpxml" or not root.get("version"):
        fail("document root/version is not FCPXML")
    project = exactly_one(root.iter("project"), "project")
    resources = root.find("resources")
    effect_refs = {
        effect.get("id")
        for effect in ([] if resources is None else resources)
        if effect.tag == "effect" and effect.get("uid") in PRODUCTION_EFFECT_UIDS
    }
    if None in effect_refs or len(effect_refs) != 1:
        fail(f"production effect resource must be unique, found {len(effect_refs)}")
    parent_map = {child: parent for parent in root.iter() for child in parent}
    filters = [
        item
        for item in project.iter("filter-video")
        if item.get("ref") in effect_refs
    ]
    if not filters:
        fail("processed project has no production NiYien occurrences")

    occurrences = []
    found_names = []
    found_occurrence_ids = set()
    found_instance_identities = set()
    for index, filter_video in enumerate(filters, start=1):
        clip, asset_ref = supported_clip_and_asset_ref(
            filter_video, parent_map, f"occurrence {index}"
        )
        clip_name = clip.get("name") or clip.get("ref") or f"occurrence-{index}"
        if clip_name in found_names:
            fail(f"occurrence names are not unique: {clip_name}")
        found_names.append(clip_name)
        context = f"occurrence {index} ({clip_name})"
        generation_a, prefix_a, payload_digest_a, project_a = decode_bank(
            filter_video, "A", context
        )
        generation_b, prefix_b, payload_digest_b, project_b = decode_bank(
            filter_video, "B", context
        )
        if (
            prefix_a != prefix_b
            or payload_digest_a != payload_digest_b
            or project_a != project_b
        ):
            fail(f"{context}: project payload banks differ")
        instance_identity = decode_instance_identity(filter_video, prefix_a, context)
        if instance_identity in found_instance_identities:
            fail(f"{context}: duplicate Instance Identity {instance_identity}")
        found_instance_identities.add(instance_identity)
        expected_path = expected_projects.get(clip_name)
        if expected_path is None:
            fail(f"occurrence names differ from expectations: unexpected {clip_name}")
        expected_path = Path(expected_path)
        if not expected_path.is_file() or expected_path.is_symlink():
            fail(f"{context}: expected project is not one regular file: {expected_path}")
        expected_bytes = expected_path.read_bytes()
        if project_a != expected_bytes:
            fail(f"{context}: decoded project bytes differ from {expected_path}")
        bounds_start = parse_fcpxml_time(
            clip.get("start"), f"{context} clip start"
        )
        timing = decode_timing(
            filter_video,
            asset_ref,
            root.get("version"),
            bounds_start,
            context,
        )
        occurrence_id = timing["occurrence"]
        if occurrence_id in found_occurrence_ids:
            fail(f"{context}: duplicate timing occurrence {occurrence_id}")
        found_occurrence_ids.add(occurrence_id)
        expected_mapping = expected_mappings.get(clip_name)
        actual_mapping = [
            (point["local"], point["source"]) for point in timing["mapping"]
        ]
        if expected_mapping is not None and actual_mapping != expected_mapping:
            fail(
                f"{context}: mapping differs; expected {expected_mapping}, got {actual_mapping}"
            )
        occurrences.append(
            {
                "clip_name": clip_name,
                "asset_ref": asset_ref,
                "instance_identity": instance_identity,
                "bank_generations": {"A": generation_a, "B": generation_b},
                "encoded_payload_sha256": payload_digest_a,
                "project_bytes": len(project_a),
                "project_sha256": hashlib.sha256(project_a).hexdigest(),
                "timing": timing,
            }
        )

    if set(found_names) != set(expected_projects):
        fail(
            "occurrence names differ from expectations: "
            f"expected {sorted(expected_projects)}, found {sorted(found_names)}"
        )
    unknown_mapping_names = set(expected_mappings) - set(found_names)
    if unknown_mapping_names:
        fail(f"mapping expectations contain unknown occurrences: {sorted(unknown_mapping_names)}")
    return {
        "fcpxml": str(xml_path),
        "fcpxml_sha256": hashlib.sha256(xml_bytes).hexdigest(),
        "project_name": project.get("name"),
        "project_uid": project.get("uid"),
        "occurrence_count": len(occurrences),
        "occurrences": occurrences,
    }


def parse_named_path(value: str) -> tuple[str, Path]:
    if "=" not in value:
        fail(f"expected NAME=PATH, got {value}")
    name, path = value.split("=", 1)
    if not name or not path:
        fail(f"expected NAME=PATH, got {value}")
    return name, Path(path)


def parse_named_mapping(value: str) -> tuple[str, list[tuple[str, str]]]:
    if "=" not in value:
        fail(f"expected NAME=LOCAL:SOURCE[,LOCAL:SOURCE], got {value}")
    name, encoded_points = value.split("=", 1)
    points = []
    for encoded_point in encoded_points.split(","):
        if ":" not in encoded_point:
            fail(f"expected LOCAL:SOURCE mapping point, got {encoded_point}")
        local, source = encoded_point.split(":", 1)
        parse_rational(local, f"{name} expected local")
        parse_rational(source, f"{name} expected source")
        points.append((local, source))
    if not name or len(points) < 2:
        fail(f"expected NAME with at least two mapping points, got {value}")
    return name, points


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Verify Final Cut Route D project banks and timing byte-for-byte."
    )
    parser.add_argument("--fcpxml", required=True, type=Path)
    parser.add_argument(
        "--expect-project",
        action="append",
        default=[],
        metavar="NAME=PATH",
        help="Expected exact .gyroflow bytes for one named occurrence (repeatable).",
    )
    parser.add_argument(
        "--expect-mapping",
        action="append",
        default=[],
        metavar="NAME=LOCAL:SOURCE,...",
        help="Expected exact timing mapping for one named occurrence (repeatable).",
    )
    arguments = parser.parse_args()
    try:
        expected_projects = dict(parse_named_path(item) for item in arguments.expect_project)
        if not expected_projects or len(expected_projects) != len(arguments.expect_project):
            fail("provide one unique --expect-project for every occurrence")
        expected_mappings = dict(
            parse_named_mapping(item) for item in arguments.expect_mapping
        )
        if len(expected_mappings) != len(arguments.expect_mapping):
            fail("--expect-mapping occurrence names must be unique")
        result = verify_output(arguments.fcpxml, expected_projects, expected_mappings)
    except (OSError, VerificationError) as error:
        print(f"Final Cut Route D output verification failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
