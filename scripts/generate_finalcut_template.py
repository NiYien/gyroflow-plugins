#!/usr/bin/env python3
import argparse
import hashlib
import json
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
BASELINE = (
    ROOT
    / "probes"
    / "finalcut_phase0"
    / "template"
    / "upstream"
    / "Gyroflow Toolbox.moef"
)
IDENTITY = ROOT / "finalcut" / "config" / "identity.json"
EXPECTED_BASELINE_SHA256 = (
    "42dcf66155aecfc9010af748071976f0868c4d403c5fb2f6495fbda380f3a70e"
)
UPSTREAM_EFFECT_UUID = "92ADB2F9-C649-48C2-B2D4-441CFC0633CB"
UPSTREAM_EFFECT_NAME = "Gyroflow Toolbox"


def production_parameters(body: str) -> str:
    internal_start = body.index('\n\t\t\t\t<parameter name="" id="1"')
    import_start = body.index('\n\t\t\t\t<parameter name="Import"')
    internal = body[internal_start:import_start]
    drop_start = body.index(
        '\n\t\t\t\t\t<parameter name="Drop Clip Here ➡" id="10"'
    )
    drop_end = body.index('\n\t\t\t\t\t<parameter name="" id="30"', drop_start)
    project_control = body[drop_start:drop_end].replace(
        'name="Drop Clip Here ➡" id="10"',
        'name="" id="1001"',
    )
    prefix = body[:internal_start]
    hidden_fov = (
        '\n\t\t\t\t<parameter name="FOV" id="2001" flags="12884901904" default="1" value="1"/>'
    )
    adjustment_group = (
        '\n\t\t\t\t<parameter name="Stabilization" id="2000" flags="8589938704">'
        '\n\t\t\t\t\t<parameter name="Smoothness" id="2002" flags="12884901904" default="15" value="15"/>'
        '\n\t\t\t\t\t<parameter name="Lens Correction" id="2003" flags="12884901904" default="100" value="100"/>'
        '\n\t\t\t\t\t<parameter name="Horizon Lock" id="2004" flags="12884901904" default="0" value="0"/>'
        '\n\t\t\t\t\t<parameter name="Horizon Roll" id="2005" flags="12884901904" default="0" value="0"/>'
        '\n\t\t\t\t\t<parameter name="Zoom Mode" id="2006" flags="12884901904" default="1" value="1"/>'
        '\n\t\t\t\t\t<parameter name="Stabilization Overview" id="2007" flags="12884901904" default="0" value="0"/>'
        '\n\t\t\t\t</parameter>'
    )
    bank_parameters = [
        (1904, "Project Payload Manifest A"),
        (1905, "Project Payload Manifest B"),
        *((1910 + index, f"Project Payload A {index + 1:02d}") for index in range(10)),
        *((1930 + index, f"Project Payload B {index + 1:02d}") for index in range(10)),
    ]
    hidden_payload_parameters = "".join(
        f'\n\t\t\t\t<parameter name="{name}" id="{parameter_id}" flags="12889161760"/>'
        for parameter_id, name in bank_parameters
    )
    hidden_and_builtin = (
        '\n\t\t\t\t<parameter name="Instance Identity" id="1901" flags="12889161760"/>'
        '\n\t\t\t\t<parameter name="Project Payload" id="1902" flags="12889161760"/>'
        '\n\t\t\t\t<parameter name="Timing Payload" id="1903" flags="12889161760"/>'
        '\n\t\t\t\t<parameter name="Project Display Name" id="1906" flags="12889161760"/>'
        + hidden_payload_parameters
        + '\n\t\t\t\t<parameter name="Mix" id="10001" flags="12884901888" default="1" value="1"/>'
        + '\n\t\t\t\t<parameter name="Flip" id="10002" flags="12889161760" default="0" value="0"/>'
        + '\n\t\t\t\t<parameter name="Input Points" id="10003" flags="12889161760" default="1" value="1"/>'
    )
    return (
        prefix
        + internal
        + project_control
        + hidden_fov
        + adjustment_group
        + hidden_and_builtin
    )


def render_template() -> str:
    baseline_bytes = BASELINE.read_bytes()
    actual_hash = hashlib.sha256(baseline_bytes).hexdigest()
    if actual_hash != EXPECTED_BASELINE_SHA256:
        raise ValueError(
            "opaque Motion template baseline drifted: "
            f"expected {EXPECTED_BASELINE_SHA256}, got {actual_hash}"
        )
    identity = json.loads(IDENTITY.read_text(encoding="utf-8"))
    rendered = baseline_bytes.decode("utf-8")
    rendered = rendered.replace(UPSTREAM_EFFECT_UUID, identity["effect_uuid"])
    rendered = rendered.replace(UPSTREAM_EFFECT_NAME, identity["effect_name"])
    filter_match = re.search(
        r"(?P<open><filter\b[^>]*>)(?P<body>.*?)(?P<close>\n\s*</filter>)",
        rendered,
        flags=re.DOTALL,
    )
    if filter_match is None:
        raise ValueError("Motion template must contain one filter")
    opening = re.sub(
        r'pluginVersion="[^"]*"',
        f'pluginVersion="{identity["marketing_version"]}"',
        filter_match.group("open"),
        count=1,
    )
    replacement = (
        opening
        + production_parameters(filter_match.group("body"))
        + filter_match.group("close")
    )
    rendered = rendered[: filter_match.start()] + replacement + rendered[filter_match.end() :]
    publish_settings = (
        "<publishSettings>\n"
        "\t\t<version>2</version>\n"
        '\t\t<target object="10036" channel="./1001" name=""/>\n'
        '\t\t<target object="10036" channel="./2000" name="Stabilization"/>\n'
        "\t</publishSettings>"
    )
    rendered, count = re.subn(
        r"<publishSettings>.*?</publishSettings>",
        publish_settings,
        rendered,
        count=1,
        flags=re.DOTALL,
    )
    if count != 1:
        raise ValueError("Motion template must contain one publishSettings block")
    if UPSTREAM_EFFECT_UUID in rendered or UPSTREAM_EFFECT_NAME in rendered:
        raise ValueError("upstream identity remains in generated Motion template")
    return rendered


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()
    rendered = render_template()
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = arguments.output.with_name(f".{arguments.output.name}.tmp")
    temporary.write_text(rendered, encoding="utf-8")
    temporary.replace(arguments.output)


if __name__ == "__main__":
    main()
