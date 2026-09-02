#!/usr/bin/env python3
import argparse
import hashlib
import plistlib
import sys
import xml.etree.ElementTree as ET
from pathlib import Path


EXPECTED_OPAQUE_COUNT = 6
EXPECTED_OPAQUE_SHA256 = "908253a2fba3d27016d0f216cdb529553c54c1431e955d49bdf69b11968e311c"
UPSTREAM_EFFECT_UUID = "92ADB2F9-C649-48C2-B2D4-441CFC0633CB"
UPSTREAM_GROUP_UUID = "67F9D1B5-88D7-4846-B08A-192BDD739992"


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def verify(template_path: Path, xpc_info_path: Path) -> None:
    template_bytes = template_path.read_bytes()
    tree = ET.fromstring(template_bytes)
    with xpc_info_path.open("rb") as plist_file:
        xpc_info = plistlib.load(plist_file)

    filters = tree.findall(".//filter")
    require(len(filters) == 1, "template must contain exactly one filter")
    filter_element = filters[0]
    plugin_entry = xpc_info["ProPlugPlugInList"][0]
    group_entry = xpc_info["ProPlugPlugInGroupList"][0]
    require(filter_element.attrib.get("pluginUUID") == plugin_entry["uuid"], "template/XPC effect UUID mismatch")
    require(plugin_entry["group"] == group_entry["uuid"], "XPC effect/group UUID mismatch")

    effect_source_filters = tree.findall(".//scenenode[@name='Effect Source']/filter")
    require(effect_source_filters == filters, "filter must be connected directly to Effect Source")
    published_targets = {
        (target.attrib.get("object"), target.attrib.get("channel"), target.attrib.get("name"))
        for target in tree.findall(".//publishSettings/target")
    }
    require(
        published_targets == {("10036", "./1100", "Phase 0 Probe")},
        "unexpected publishSettings mapping",
    )

    opaque_values = [
        node.text or ""
        for node in tree.findall(".//defaultVal") + tree.findall(".//dataValue")
    ]
    require(len(opaque_values) == EXPECTED_OPAQUE_COUNT, "opaque field count drifted")
    require(
        all(
            hashlib.sha256(value.encode("utf-8")).hexdigest() == EXPECTED_OPAQUE_SHA256
            for value in opaque_values
        ),
        "opaque defaultVal/dataValue bytes drifted",
    )

    rendered = template_bytes.decode("utf-8")
    require(UPSTREAM_EFFECT_UUID not in rendered, "upstream effect UUID is forbidden")
    require(UPSTREAM_GROUP_UUID not in plistlib.dumps(xpc_info).decode("utf-8"), "upstream group UUID is forbidden")
    require("Gyroflow Toolbox" not in rendered, "upstream product identity is forbidden")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("template", type=Path)
    parser.add_argument("xpc_info", type=Path)
    args = parser.parse_args()
    try:
        verify(args.template, args.xpc_info)
    except (KeyError, OSError, ET.ParseError, ValueError) as error:
        print(f"template contract failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error


if __name__ == "__main__":
    main()
