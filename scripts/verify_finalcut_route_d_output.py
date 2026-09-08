#!/usr/bin/env python3
from __future__ import annotations

import argparse
import base64
import copy
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
PROJECT_PARAMETER_KEY_PREFIX = "9999/10013/10016/3/10036"
VISIBLE_PARAMETER_IDS = tuple(range(2001, 2008))
RESERVED_PARAMETER_IDS = {
    *range(1901, 1907),
    *range(1910, 1920),
    *range(1930, 1940),
    *VISIBLE_PARAMETER_IDS,
}


class VerificationError(ValueError):
    pass


def fail(message: str) -> None:
    raise VerificationError(message)


def exactly_one(items, context: str):
    items = list(items)
    if len(items) != 1:
        fail(f"{context}: expected exactly one, found {len(items)}")
    return items[0]


def production_parameter_key(identifier: int) -> str:
    return f"{PROJECT_PARAMETER_KEY_PREFIX}/{identifier}"


def parameters_with_id(filter_video: ET.Element, identifier: int) -> list[ET.Element]:
    key = production_parameter_key(identifier)
    return [
        parameter
        for parameter in filter_video.findall("param")
        if parameter.get("key") == key
    ]


def exactly_one_parameter(
    filter_video: ET.Element, identifier: int, context: str
) -> ET.Element:
    return exactly_one(
        parameters_with_id(filter_video, identifier),
        f"{context} production parameter /{identifier}",
    )


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
) -> tuple[int, str, str, bytes, int]:
    manifest_id, chunk_start = (1904, 1910) if bank == "A" else (1905, 1930)
    manifest = exactly_one_parameter(filter_video, manifest_id, f"{context} bank {bank}")
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
    chunks = []
    for index in range(chunk_count):
        chunk = exactly_one_parameter(
            filter_video,
            chunk_start + index,
            f"{context} bank {bank} chunk {index + 1}",
        )
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
        PROJECT_PARAMETER_KEY_PREFIX,
        payload_digest,
        decode_project_payload(payload, f"{context} bank {bank}"),
        chunk_count,
    )


def decode_bank_optional(filter_video: ET.Element, bank: str, context: str):
    manifest_id = 1904 if bank == "A" else 1905
    manifests = parameters_with_id(filter_video, manifest_id)
    if not manifests or not manifests[0].get("value"):
        return None
    return decode_bank(filter_video, bank, context)


def decode_instance_identity(
    filter_video: ET.Element, context: str
) -> str:
    parameter = exactly_one_parameter(filter_video, 1901, context)
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
    parameter = exactly_one_parameter(filter_video, 1903, context)
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


def read_regular_file_bounded(path: Path, maximum_bytes: int, context: str) -> bytes:
    try:
        if path.is_symlink() or not path.is_file():
            fail(f"{context} is not one regular file: {path}")
        if path.stat().st_size > maximum_bytes:
            fail(f"{context} exceeds the {maximum_bytes}-byte limit: {path}")
        with path.open("rb") as handle:
            value = handle.read(maximum_bytes + 1)
    except OSError as error:
        fail(f"unable to read {context}: {error}")
    if len(value) > maximum_bytes:
        fail(f"{context} exceeds the {maximum_bytes}-byte limit: {path}")
    return value


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


def parse_document(path: Path, label: str) -> tuple[Path, bytes, ET.Element, ET.Element, dict]:
    xml_path = resolve_fcpxml(Path(path))
    try:
        xml_bytes = read_regular_file_bounded(
            xml_path, MAX_PROJECT_BYTES, f"{label} FCPXML"
        )
        root = ET.fromstring(xml_bytes)
    except ET.ParseError as error:
        fail(f"unable to parse {label} FCPXML: {error}")
    if root.tag != "fcpxml" or not root.get("version"):
        fail(f"{label} document root/version is not FCPXML")
    project = exactly_one(root.iter("project"), f"{label} project")
    return (
        xml_path,
        xml_bytes,
        root,
        project,
        {child: parent for parent in root.iter() for child in parent},
    )


def production_effect_refs(root: ET.Element, label: str) -> set[str]:
    resources = root.find("resources")
    effect_refs = {
        effect.get("id")
        for effect in ([] if resources is None else resources)
        if effect.tag == "effect" and effect.get("uid") in PRODUCTION_EFFECT_UIDS
    }
    if None in effect_refs or len(effect_refs) != 1:
        fail(f"{label} production effect resource must be unique, found {len(effect_refs)}")
    return effect_refs


def validate_global_resource_ids(root: ET.Element, label: str) -> None:
    resources = root.find("resources")
    if resources is None:
        fail(f"{label} document has no resources element")
    seen = {}
    for resource in resources:
        identifier = resource.get("id")
        if identifier is None:
            continue
        if identifier in seen:
            fail(
                f"{label} resources contain duplicate id {identifier!r} "
                f"on {seen[identifier]} and {resource.tag}"
            )
        seen[identifier] = resource.tag


def element_route(element: ET.Element, parent_map: dict[ET.Element, ET.Element]) -> tuple[tuple[str, int], ...]:
    route = []
    current = element
    while current in parent_map:
        parent = parent_map[current]
        matching_siblings = [sibling for sibling in parent if sibling.tag == current.tag]
        route.append(
            (current.tag, matching_siblings.index(current) + 1)
        )
        current = parent
    route.append((current.tag, 1))
    return tuple(reversed(route))


def route_key(route: tuple[tuple[str, int], ...]) -> str:
    return "/" + "/".join(f"{tag}[{index}]" for tag, index in route)


def occurrence_records(
    root: ET.Element,
    project: ET.Element,
    effect_refs: set[str],
    parent_map: dict[ET.Element, ET.Element],
    label: str,
) -> dict[tuple[tuple[str, int], ...], dict]:
    records = {}
    candidates = []
    for filter_video in root.iter("filter-video"):
        if filter_video.get("ref") not in effect_refs:
            continue
        current = parent_map.get(filter_video)
        in_supported_scope = False
        while current is not None:
            if current is project or current.tag == "media":
                in_supported_scope = True
                break
            current = parent_map.get(current)
        if in_supported_scope:
            candidates.append(filter_video)
    for index, filter_video in enumerate(candidates, start=1):
        context = f"{label} occurrence {index}"
        clip, asset_ref = supported_clip_and_asset_ref(filter_video, parent_map, context)
        route = element_route(filter_video, parent_map)
        if route in records:
            fail(f"{context}: duplicate structural occurrence route")
        records[route] = {
            "route": route_key(route),
            "filter": filter_video,
            "clip": clip,
            "clip_name": clip.get("name") or clip.get("ref") or f"occurrence-{index}",
            "asset_ref": asset_ref,
            "context": context,
        }
    if not records:
        fail(f"{label} project has no production NiYien occurrences")
    return records


def parameter_id(parameter: ET.Element) -> int | None:
    if parameter.tag != "param":
        return None
    key = parameter.get("key", "")
    prefix = f"{PROJECT_PARAMETER_KEY_PREFIX}/"
    if not key.startswith(prefix):
        return None
    suffix = key[len(prefix) :]
    if "/" in suffix:
        return None
    try:
        identifier = int(suffix)
    except ValueError:
        return None
    if suffix != str(identifier) or identifier not in RESERVED_PARAMETER_IDS:
        return None
    return identifier


def is_allowed_target_parameter(parameter: ET.Element) -> bool:
    return parameter_id(parameter) in RESERVED_PARAMETER_IDS


def frozen_filter(filter_video: ET.Element) -> bytes:
    clone = ET.Element(filter_video.tag, filter_video.attrib)
    clone.text = filter_video.text
    clone.tail = filter_video.tail
    for child in filter_video:
        if not is_allowed_target_parameter(child):
            clone.append(copy.deepcopy(child))
    return ET.tostring(clone, encoding="utf-8")


def frozen_project(
    project: ET.Element,
    effect_refs: set[str],
    parent_map: dict[ET.Element, ET.Element],
) -> bytes:
    def clone(element: ET.Element) -> ET.Element:
        if element.tag == "filter-video" and element.get("ref") in effect_refs:
            placeholder = ET.Element("production-occurrence")
            placeholder.set("route", route_key(element_route(element, parent_map)))
            placeholder.tail = element.tail
            return placeholder
        attributes = dict(element.attrib)
        if element is project:
            attributes.pop("uid", None)
        result = ET.Element(element.tag, attributes)
        result.text = element.text
        result.tail = element.tail
        for child in element:
            result.append(clone(child))
        return result

    return ET.tostring(clone(project), encoding="utf-8")


def frozen_resources(
    root: ET.Element,
    effect_refs: set[str],
    parent_map: dict[ET.Element, ET.Element],
) -> bytes:
    resources = root.find("resources")
    if resources is None:
        fail("document has no resources element")

    def clone(element: ET.Element) -> ET.Element:
        if element.tag == "filter-video" and element.get("ref") in effect_refs:
            placeholder = ET.Element("production-occurrence")
            placeholder.set("route", route_key(element_route(element, parent_map)))
            placeholder.tail = element.tail
            return placeholder
        result = ET.Element(element.tag, dict(element.attrib))
        result.text = element.text
        result.tail = element.tail
        for child in element:
            result.append(clone(child))
        return result

    return ET.tostring(clone(resources), encoding="utf-8")


def validate_reserved_parameter_keys(occurrences: dict, label: str) -> None:
    for record in occurrences.values():
        seen = set()
        for parameter in record["filter"].findall("param"):
            identifier = parameter_id(parameter)
            if identifier is None:
                continue
            if identifier in seen:
                fail(
                    f"{label} {record['route']} has duplicate reserved "
                    f"parameter /{identifier}"
                )
            seen.add(identifier)


def resolve_occurrence_expectations(
    expected_projects: dict[str, Path],
    expected_mappings: dict[str, list[tuple[str, str]]],
    expected_parameters: dict[str, list[str]],
    occurrences: dict[tuple[tuple[str, int], ...], dict],
) -> tuple[
    dict[tuple[tuple[str, int], ...], Path],
    dict[tuple[tuple[str, int], ...], list[tuple[str, str]]],
    dict[tuple[tuple[str, int], ...], list[str]],
]:
    routes = {record["route"]: route for route, record in occurrences.items()}
    names: dict[str, list[tuple[tuple[str, int], ...]]] = {}
    for route, record in occurrences.items():
        names.setdefault(record["clip_name"], []).append(route)

    def resolve(identity: str, expectation_kind: str) -> tuple[tuple[str, int], ...]:
        if identity in routes:
            return routes[identity]
        matches = names.get(identity, [])
        if len(matches) == 1:
            return matches[0]
        if len(matches) > 1:
            fail(
                f"{expectation_kind} {identity!r} is ambiguous; use its structural route"
            )
        fail(f"occurrence names differ from expectations: unexpected {identity}")

    resolved_projects = {}
    for identity, path in expected_projects.items():
        route = resolve(identity, "project expectation")
        if route in resolved_projects:
            fail(f"multiple project expectations resolve to {route_key(route)}")
        resolved_projects[route] = path
    resolved_mappings = {}
    for identity, mapping in expected_mappings.items():
        route = resolve(identity, "mapping expectation")
        if route in resolved_mappings:
            fail(f"multiple mapping expectations resolve to {route_key(route)}")
        if route not in resolved_projects:
            fail(f"mapping expectation has no project expectation: {identity}")
        resolved_mappings[route] = mapping
    resolved_parameters = {}
    for identity, values in expected_parameters.items():
        route = resolve(identity, "parameter expectation")
        if route in resolved_parameters:
            fail(f"multiple parameter expectations resolve to {route_key(route)}")
        if route not in resolved_projects:
            fail(f"parameter expectation has no project expectation: {identity}")
        if (
            not isinstance(values, (list, tuple))
            or len(values) != len(VISIBLE_PARAMETER_IDS)
            or any(not isinstance(value, str) for value in values)
        ):
            fail(f"{identity}: expected exactly seven string parameter values")
        resolved_parameters[route] = list(values)
    return resolved_projects, resolved_mappings, resolved_parameters


def selected_project_payload(filter_video: ET.Element, context: str):
    candidates = []
    for bank in ("A", "B"):
        try:
            candidate = decode_bank_optional(filter_video, bank, context)
        except VerificationError:
            candidate = None
        if candidate is not None:
            candidates.append((bank, candidate))
    if (
        len(candidates) == 2
        and candidates[0][1][0] == candidates[1][1][0]
        and candidates[0][1][3] != candidates[1][1][3]
    ):
        fail(f"{context}: project payload banks conflict at the same generation")
    if candidates:
        return max(candidates, key=lambda item: item[1][0])

    legacy = parameters_with_id(filter_video, 1902)
    if not legacy or not legacy[0].get("value"):
        return None
    try:
        project = decode_project_payload(legacy[0].get("value", ""), f"{context} legacy")
    except VerificationError:
        return None
    return "legacy", (0, PROJECT_PARAMETER_KEY_PREFIX, "", project, 0)


def parameter_subtrees(filter_video: ET.Element, identifiers) -> dict[int, list[bytes]]:
    return {
        identifier: [
            ET.tostring(parameter, encoding="utf-8")
            for parameter in parameters_with_id(filter_video, identifier)
        ]
        for identifier in identifiers
    }


def require_static_parameter(parameter: ET.Element, context: str) -> None:
    if list(parameter) or (parameter.text is not None and parameter.text.strip()):
        fail(f"{context} must be a static parameter without keyframes or child nodes")


def verify_same_hash_action(
    original_filter: ET.Element,
    output_filter: ET.Element,
    expected_path: Path,
    expected_bytes: bytes,
    context: str,
):
    preserved_ids = RESERVED_PARAMETER_IDS - {1903, 1906}
    if parameter_subtrees(original_filter, preserved_ids) != parameter_subtrees(
        output_filter, preserved_ids
    ):
        fail(f"{context}: equal-hash bank, identity, legacy, or visible subtrees changed")

    original_display = parameters_with_id(original_filter, 1906)
    output_display = parameters_with_id(output_filter, 1906)
    if not original_display:
        display = exactly_one(output_display, f"{context} equal-hash inserted display")
        require_static_parameter(display, f"{context} equal-hash inserted display")
        if display.get("value") != expected_path.name:
            fail(f"{context}: equal-hash inserted display filename is incorrect")
    elif original_display[0].get("value", ""):
        if parameter_subtrees(original_filter, {1906}) != parameter_subtrees(
            output_filter, {1906}
        ):
            fail(f"{context}: equal-hash display subtree changed")
    else:
        display = exactly_one(output_display, f"{context} equal-hash filled display")
        if display.get("value") != expected_path.name:
            fail(f"{context}: equal-hash filled display filename is incorrect")
        normalized = copy.deepcopy(display)
        normalized.set("value", "")
        if ET.tostring(normalized, encoding="utf-8") != ET.tostring(
            original_display[0], encoding="utf-8"
        ):
            fail(f"{context}: equal-hash display changed beyond its empty value")

    selected = selected_project_payload(output_filter, context)
    if selected is None or selected[1][3] != expected_bytes:
        fail(f"{context}: equal-hash selected project payload changed")
    banks = {}
    payload_digest = ""
    for bank in ("A", "B"):
        candidate = decode_bank_optional(output_filter, bank, context)
        if candidate is not None:
            banks[bank] = candidate[0]
            if not payload_digest:
                payload_digest = candidate[2]
    identity = None
    if parameters_with_id(output_filter, 1901):
        identity = decode_instance_identity(output_filter, context)
    return banks, payload_digest, identity


def verify_different_hash_action(
    output_filter: ET.Element,
    expected_path: Path,
    expected_bytes: bytes,
    expected_parameters: list[str] | None,
    context: str,
):
    if expected_parameters is None:
        fail(f"{context}: different-hash update has no manager parameter expectations")
    bank_a = decode_bank(output_filter, "A", context)
    bank_b = decode_bank(output_filter, "B", context)
    if (
        bank_a[0] != 2
        or bank_b[0] != 1
        or bank_a[1] != PROJECT_PARAMETER_KEY_PREFIX
        or bank_b[1] != PROJECT_PARAMETER_KEY_PREFIX
        or bank_a[2] != bank_b[2]
        or bank_a[3] != expected_bytes
        or bank_b[3] != expected_bytes
    ):
        fail(f"{context}: different-hash output does not contain canonical two-bank data")

    required_ids = {
        1901,
        1903,
        1904,
        1905,
        1906,
        *range(1910, 1910 + bank_a[4]),
        *range(1930, 1930 + bank_b[4]),
        *VISIBLE_PARAMETER_IDS,
    }
    present_ids = {
        identifier
        for parameter in output_filter.findall("param")
        if (identifier := parameter_id(parameter)) is not None
    }
    if present_ids != required_ids:
        fail(
            f"{context}: different-hash reserved parameter set differs; "
            f"expected {sorted(required_ids)}, got {sorted(present_ids)}"
        )
    for identifier in required_ids - {1903, *VISIBLE_PARAMETER_IDS}:
        require_static_parameter(
            exactly_one_parameter(output_filter, identifier, context),
            f"{context} different-hash parameter /{identifier}",
        )

    display = exactly_one_parameter(output_filter, 1906, context)
    if display.get("value") != expected_path.name:
        fail(f"{context}: different-hash display filename is incorrect")
    for identifier, expected_value in zip(VISIBLE_PARAMETER_IDS, expected_parameters):
        parameter = exactly_one_parameter(output_filter, identifier, context)
        if parameter.get("value") != expected_value:
            fail(
                f"{context}: different-hash visible parameter /{identifier} differs; "
                f"expected {expected_value!r}, got {parameter.get('value')!r}"
            )
        require_static_parameter(
            parameter, f"{context} different-hash visible parameter /{identifier}"
        )
    return {"A": bank_a[0], "B": bank_b[0]}, bank_a[2], decode_instance_identity(
        output_filter, context
    )


def verify_output(
    input_path: Path,
    expected_projects: dict[str, Path],
    expected_mappings: dict[str, list[tuple[str, str]]],
    expected_parameters: dict[str, list[str]] | None = None,
    *,
    original_path: Path,
) -> dict:
    expected_parameters = {} if expected_parameters is None else expected_parameters
    original_xml_path, _, original_root, original_project, original_parents = parse_document(
        original_path, "original"
    )
    xml_path, xml_bytes, root, project, parent_map = parse_document(input_path, "output")
    if original_root.get("version") != root.get("version"):
        fail("output FCPXML version differs from the original")
    if project.get("name") != original_project.get("name"):
        fail("output project name differs from the original")
    validate_global_resource_ids(original_root, "original")
    validate_global_resource_ids(root, "output")
    original_effect_refs = production_effect_refs(original_root, "original")
    effect_refs = production_effect_refs(root, "output")
    if effect_refs != original_effect_refs:
        fail("output adds or changes a production effect resource")
    original_occurrences = occurrence_records(
        original_root,
        original_project,
        original_effect_refs,
        original_parents,
        "original",
    )
    output_occurrences = occurrence_records(
        root, project, effect_refs, parent_map, "output"
    )
    if set(output_occurrences) != set(original_occurrences):
        fail("output production occurrence set differs from the original")
    validate_reserved_parameter_keys(original_occurrences, "original")
    validate_reserved_parameter_keys(output_occurrences, "output")
    if frozen_resources(
        original_root, original_effect_refs, original_parents
    ) != frozen_resources(root, effect_refs, parent_map):
        fail("output resources differ outside production occurrences")
    if frozen_project(
        original_project, original_effect_refs, original_parents
    ) != frozen_project(project, effect_refs, parent_map):
        fail("output project structure differs outside production occurrences")
    expected_by_route, mappings_by_route, parameters_by_route = (
        resolve_occurrence_expectations(
            expected_projects,
            expected_mappings,
            expected_parameters,
            original_occurrences,
        )
    )

    occurrences = []
    found_occurrence_ids = set()
    for route, original_record in original_occurrences.items():
        output_record = output_occurrences[route]
        clip_name = original_record["clip_name"]
        if output_record["clip_name"] != clip_name:
            fail(f"output occurrence at {route} changed clip name")
        context = f"output occurrence ({clip_name}, {original_record['route']})"
        filter_video = output_record["filter"]
        if frozen_filter(filter_video) != frozen_filter(original_record["filter"]):
            fail(f"{context}: non-allowed filter content changed")
        expected_path = expected_by_route.get(route)
        if expected_path is None:
            if ET.tostring(filter_video, encoding="utf-8") != ET.tostring(
                original_record["filter"], encoding="utf-8"
            ):
                fail(f"{context}: skipped filter is not preserved literally")
            continue
        expected_path = Path(expected_path)
        expected_bytes = read_regular_file_bounded(
            expected_path, MAX_PROJECT_BYTES, f"{context} expected project"
        )
        original_selected = selected_project_payload(
            original_record["filter"], f"original occurrence ({clip_name})"
        )
        same_hash = (
            original_selected is not None and original_selected[1][3] == expected_bytes
        )
        if same_hash:
            action = "timing_only"
            bank_generations, payload_digest, instance_identity = verify_same_hash_action(
                original_record["filter"],
                filter_video,
                expected_path,
                expected_bytes,
                context,
            )
        else:
            action = "updated_project"
            bank_generations, payload_digest, instance_identity = (
                verify_different_hash_action(
                    filter_video,
                    expected_path,
                    expected_bytes,
                    parameters_by_route.get(route),
                    context,
                )
            )
        bounds_start = parse_fcpxml_time(output_record["clip"].get("start"), f"{context} clip start")
        timing = decode_timing(
            filter_video,
            output_record["asset_ref"],
            root.get("version"),
            bounds_start,
            context,
        )
        occurrence_id = timing["occurrence"]
        if occurrence_id in found_occurrence_ids:
            fail(f"{context}: duplicate timing occurrence {occurrence_id}")
        found_occurrence_ids.add(occurrence_id)
        expected_mapping = mappings_by_route.get(route)
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
                "route": original_record["route"],
                "asset_ref": output_record["asset_ref"],
                "instance_identity": instance_identity,
                "action": action,
                "bank_generations": bank_generations,
                "encoded_payload_sha256": payload_digest,
                "project_bytes": len(expected_bytes),
                "project_sha256": hashlib.sha256(expected_bytes).hexdigest(),
                "timing": timing,
            }
        )

    return {
        "fcpxml": str(xml_path),
        "original_fcpxml": str(original_xml_path),
        "fcpxml_sha256": hashlib.sha256(xml_bytes).hexdigest(),
        "project_name": project.get("name"),
        "project_uid": project.get("uid"),
        "original_project_uid": original_project.get("uid"),
        "occurrence_count": len(occurrences),
        "skipped_occurrence_count": len(original_occurrences) - len(occurrences),
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


def parse_named_parameters(value: str) -> tuple[str, list[str]]:
    if "=" not in value:
        fail(f"expected NAME=FOV,SMOOTHNESS,LENS,HORIZON,ROLL,ZOOM,OVERVIEW, got {value}")
    name, encoded_values = value.split("=", 1)
    values = encoded_values.split(",")
    if not name or len(values) != len(VISIBLE_PARAMETER_IDS):
        fail(f"expected NAME with exactly seven parameter values, got {value}")
    return name, values


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Verify a Final Cut Route D replacement against its original export."
    )
    parser.add_argument(
        "--original-fcpxml",
        required=True,
        type=Path,
        help="The complete FCPXML exported from Final Cut before replacement.",
    )
    parser.add_argument("--fcpxml", required=True, type=Path)
    parser.add_argument(
        "--expect-project",
        action="append",
        default=[],
        metavar="NAME=PATH",
        help="Legacy display-name expectation; accepted only when the original name is unique.",
    )
    parser.add_argument(
        "--expect-occurrence",
        action="append",
        default=[],
        metavar="ROUTE=PATH",
        help="Expected exact sibling bytes for one structural occurrence route (repeatable).",
    )
    parser.add_argument(
        "--expect-mapping",
        action="append",
        default=[],
        metavar="OCCURRENCE=LOCAL:SOURCE,...",
        help="Expected timing mapping for a structural route or unique legacy display name.",
    )
    parser.add_argument(
        "--expect-parameters",
        action="append",
        default=[],
        metavar="OCCURRENCE=FOV,SMOOTHNESS,LENS,HORIZON,ROLL,ZOOM,OVERVIEW",
        help="Expected manager-derived static values for a different-hash target.",
    )
    arguments = parser.parse_args()
    try:
        project_items = [
            *(parse_named_path(item) for item in arguments.expect_project),
            *(parse_named_path(item) for item in arguments.expect_occurrence),
        ]
        expected_projects = dict(project_items)
        if not expected_projects or len(expected_projects) != len(project_items):
            fail("provide one unique occurrence expectation for every updated target")
        expected_mappings = dict(
            parse_named_mapping(item) for item in arguments.expect_mapping
        )
        if len(expected_mappings) != len(arguments.expect_mapping):
            fail("--expect-mapping occurrence names must be unique")
        expected_parameters = dict(
            parse_named_parameters(item) for item in arguments.expect_parameters
        )
        if len(expected_parameters) != len(arguments.expect_parameters):
            fail("--expect-parameters occurrence names must be unique")
        result = verify_output(
            arguments.fcpxml,
            expected_projects,
            expected_mappings,
            expected_parameters,
            original_path=arguments.original_fcpxml,
        )
    except (OSError, VerificationError) as error:
        print(f"Final Cut Route D output verification failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
