#!/usr/bin/env python3
import argparse
import hashlib
import json
import plistlib
import sys
import xml.etree.ElementTree as ET
from pathlib import Path


EXPECTED_OPAQUE_COUNT = 6
EXPECTED_OPAQUE_SHA256 = (
    "908253a2fba3d27016d0f216cdb529553c54c1431e955d49bdf69b11968e311c"
)
UPSTREAM_EFFECT_UUID = "92ADB2F9-C649-48C2-B2D4-441CFC0633CB"
UPSTREAM_GROUP_UUID = "67F9D1B5-88D7-4846-B08A-192BDD739992"


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def verify(template: Path, xpc_info: Path, identity_path: Path) -> None:
    template_bytes = template.read_bytes()
    root = ET.fromstring(template_bytes)
    with xpc_info.open("rb") as source:
        info = plistlib.load(source)
    identity = json.loads(identity_path.read_text(encoding="utf-8"))
    filters = root.findall(".//filter")
    require(len(filters) == 1, "template must contain exactly one filter")
    filter_element = filters[0]
    plugin_entries = info.get("ProPlugPlugInList", [])
    group_entries = info.get("ProPlugPlugInGroupList", [])
    require(len(plugin_entries) == 1, "XPC must declare exactly one effect")
    require(len(group_entries) == 1, "XPC must declare exactly one group")
    plugin = plugin_entries[0]
    group = group_entries[0]
    require(plugin["uuid"] == identity["effect_uuid"], "identity/XPC effect UUID mismatch")
    require(group["uuid"] == identity["group_uuid"], "identity/XPC group UUID mismatch")
    require(plugin["group"] == group["uuid"], "XPC effect/group UUID mismatch")
    require(
        plugin.get("version") in {"$(MARKETING_VERSION)", identity["marketing_version"]},
        "XPC effect version must follow the target marketing version",
    )
    require(filter_element.attrib.get("pluginUUID") == plugin["uuid"], "template/XPC UUID mismatch")
    require(filter_element.attrib.get("pluginName") == identity["effect_name"], "template effect name mismatch")
    require(
        filter_element.attrib.get("pluginVersion") == identity["marketing_version"],
        "template/XPC marketing version mismatch",
    )
    require(
        root.findall(".//scenenode[@name='Effect Source']/filter") == filters,
        "filter must be connected directly to Effect Source",
    )
    direct_ids = {
        int(parameter.attrib["id"])
        for parameter in filter_element.findall("./parameter")
    }
    expected_bank_names = {
        1904: "Project Payload Manifest A",
        1905: "Project Payload Manifest B",
        **{1910 + index: f"Project Payload A {index + 1:02d}" for index in range(10)},
        **{1930 + index: f"Project Payload B {index + 1:02d}" for index in range(10)},
    }
    require(
        direct_ids
        == {
            1,
            1000,
            1901,
            1902,
            1903,
            *expected_bank_names,
            2000,
            10001,
            10002,
            10003,
        },
        "production parameter mapping drifted",
    )
    direct_parameters = {
        int(parameter.attrib["id"]): parameter
        for parameter in filter_element.findall("./parameter")
    }
    require(
        direct_parameters[1901].attrib.get("name") == "Instance Identity"
        and direct_parameters[1901].attrib.get("flags") == "12889161760",
        "instance identity parameter mapping drifted",
    )
    require(
        all(
            direct_parameters[parameter_id].attrib.get("name") == expected_name
            and direct_parameters[parameter_id].attrib.get("flags") == "12889161760"
            for parameter_id, expected_name in expected_bank_names.items()
        ),
        "banked project payload parameter mapping drifted",
    )
    project_ids = {
        int(parameter.attrib["id"])
        for parameter in filter_element.findall("./parameter[@id='1000']/parameter")
    }
    adjustment_ids = {
        int(parameter.attrib["id"])
        for parameter in filter_element.findall("./parameter[@id='2000']/parameter")
    }
    require(project_ids == {1001}, "project parameter mapping drifted")
    require(
        adjustment_ids == {2001, 2002, 2003, 2004, 2005, 2006, 2007},
        "adjustment parameter mapping drifted",
    )
    published = {
        (target.attrib.get("object"), target.attrib.get("channel"), target.attrib.get("name"))
        for target in root.findall(".//publishSettings/target")
    }
    require(
        published
        == {
            ("10036", "./1000", "Gyroflow Project"),
            ("10036", "./2000", "Stabilization"),
        },
        "publishSettings mapping drifted",
    )
    opaque = [
        node.text or ""
        for node in root.findall(".//defaultVal") + root.findall(".//dataValue")
    ]
    require(len(opaque) == EXPECTED_OPAQUE_COUNT, "opaque field count drifted")
    require(
        all(
            hashlib.sha256(value.encode("utf-8")).hexdigest()
            == EXPECTED_OPAQUE_SHA256
            for value in opaque
        ),
        "opaque defaultVal/dataValue bytes drifted",
    )
    rendered = template_bytes.decode("utf-8")
    require(UPSTREAM_EFFECT_UUID not in rendered, "upstream effect UUID is forbidden")
    require(
        UPSTREAM_GROUP_UUID not in plistlib.dumps(info).decode("utf-8"),
        "upstream group UUID is forbidden",
    )
    require("Gyroflow Toolbox" not in rendered, "upstream product name is forbidden")
    require("Project Path" not in rendered, "project path parameter is forbidden")
    require("Bookmark" not in rendered, "bookmark parameter is forbidden")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("template", type=Path)
    parser.add_argument("xpc_info", type=Path)
    parser.add_argument(
        "--identity",
        type=Path,
        default=Path(__file__).resolve().parents[1]
        / "finalcut"
        / "config"
        / "identity.json",
    )
    arguments = parser.parse_args()
    try:
        verify(arguments.template, arguments.xpc_info, arguments.identity)
    except (KeyError, OSError, ET.ParseError, ValueError) as error:
        print(f"Final Cut template contract failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error


if __name__ == "__main__":
    main()
