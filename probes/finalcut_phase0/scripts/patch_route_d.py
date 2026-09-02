#!/usr/bin/env python3

import argparse
import json
import os
import sys
import tempfile
import uuid
import xml.etree.ElementTree as ET
from pathlib import Path


EFFECT_NAME = "Gyroflow NiYien Phase 0"
TIMING_PARAMETER_NAME = "Timing Payload"


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--target-project-name", required=True)
    return parser.parse_args()


def fail(message: str) -> None:
    raise ValueError(message)


def patch_route_d(source: Path, output: Path, target_project_name: str) -> dict[str, object]:
    if source.resolve() == output.resolve():
        fail("Route D output must not replace the source FCPXML")
    if output.exists():
        fail(f"refusing to replace existing output: {output}")
    if not output.parent.is_dir():
        fail(f"output directory does not exist: {output.parent}")

    tree = ET.parse(source)
    root = tree.getroot()
    if root.tag != "fcpxml":
        fail("input root must be fcpxml")
    projects = root.findall("./project")
    if len(projects) != 1:
        fail(f"Route D probe requires exactly one project, found {len(projects)}")

    project = projects[0]
    source_project_name = project.attrib.get("name", "")
    source_project_uid = project.attrib.get("uid", "")
    if not source_project_uid:
        fail("source project UID is missing")
    if not target_project_name or target_project_name == source_project_name:
        fail("target project name must be non-empty and differ from the source")

    target_project_uid = str(uuid.uuid4()).upper()
    project.set("name", target_project_name)
    project.set("uid", target_project_uid)

    assets = {
        asset.attrib.get("id", ""): asset
        for asset in root.findall("./resources/asset")
        if asset.attrib.get("id")
    }
    filters_patched = 0

    def visit(element: ET.Element, context: dict[str, str]) -> None:
        nonlocal filters_patched
        current = context
        if element.tag == "asset-clip":
            asset = assets.get(element.attrib.get("ref", ""))
            current = {
                "assetStart": asset.attrib.get("start", "missing") if asset is not None else "missing",
                "clipStart": element.attrib.get("start", "missing"),
                "offset": element.attrib.get("offset", "missing"),
            }
        elif element.tag == "ref-clip":
            current = {
                "assetStart": context.get("assetStart", "nested"),
                "clipStart": element.attrib.get("start", context.get("clipStart", "nested")),
                "offset": element.attrib.get("offset", "missing"),
            }

        if element.tag == "filter-video" and element.attrib.get("name") == EFFECT_NAME:
            timing_parameter = element.find(f"./param[@name='{TIMING_PARAMETER_NAME}']")
            if timing_parameter is None:
                fail("NiYien filter is missing Timing Payload")
            filters_patched += 1
            timing_parameter.set(
                "value",
                "|".join(
                    [
                        "route-d-v1",
                        f"project={target_project_uid}",
                        f"occurrence={filters_patched:04d}",
                        f"assetStart={current.get('assetStart', 'missing')}",
                        f"clipStart={current.get('clipStart', 'missing')}",
                        f"offset={current.get('offset', 'missing')}",
                    ]
                ),
            )

        for child in element:
            visit(child, current)

    visit(project, {})
    if filters_patched == 0:
        fail(f"project contains no {EFFECT_NAME} filters")

    ET.indent(tree, space="  ")
    body = ET.tostring(root, encoding="unicode", short_empty_elements=True)
    rendered = '<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE fcpxml>\n' + body

    temporary_path = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            dir=output.parent,
            prefix=f".{output.name}.",
            delete=False,
        ) as temporary:
            temporary.write(rendered)
            temporary.flush()
            os.fsync(temporary.fileno())
            temporary_path = Path(temporary.name)
        os.replace(temporary_path, output)
    finally:
        if temporary_path is not None and temporary_path.exists():
            temporary_path.unlink()

    return {
        "filtersPatched": filters_patched,
        "sourceProjectName": source_project_name,
        "sourceProjectUID": source_project_uid,
        "targetProjectName": target_project_name,
        "targetProjectUID": target_project_uid,
    }


def main() -> int:
    arguments = parse_arguments()
    try:
        summary = patch_route_d(
            arguments.input,
            arguments.output,
            arguments.target_project_name,
        )
    except (OSError, ET.ParseError, ValueError) as error:
        print(str(error), file=sys.stderr)
        return 2
    print(json.dumps(summary, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
