#!/usr/bin/env python3
import argparse
import hashlib
import json
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
BASELINE = ROOT / "template" / "upstream" / "Gyroflow Toolbox.moef"
IDENTITY = ROOT / "config" / "identity.json"
EXPECTED_BASELINE_SHA256 = "42dcf66155aecfc9010af748071976f0868c4d403c5fb2f6495fbda380f3a70e"
UPSTREAM_EFFECT_UUID = "92ADB2F9-C649-48C2-B2D4-441CFC0633CB"
UPSTREAM_EFFECT_NAME = "Gyroflow Toolbox"


def apply_phase0_parameter_mapping(rendered: str) -> str:
    filter_match = re.search(
        r"(?P<open><filter\b[^>]*>)(?P<body>.*?)(?P<close>\n\s*</filter>)",
        rendered,
        flags=re.DOTALL,
    )
    if filter_match is None:
        raise ValueError("Motion template must contain one filter")

    body = filter_match.group("body")
    internal_start = body.index('\n\t\t\t\t<parameter name="" id="1"')
    import_start = body.index('\n\t\t\t\t<parameter name="Import"')
    internal_parameters = body[internal_start:import_start]

    drop_start = body.index('\n\t\t\t\t\t<parameter name="Drop Clip Here ➡" id="10"')
    next_import_parameter = body.index('\n\t\t\t\t\t<parameter name="" id="30"', drop_start)
    drop_parameter = body[drop_start:next_import_parameter]
    drop_parameter = drop_parameter.replace(
        'name="Drop Clip Here ➡" id="10"',
        'name="Drop One Clip or File" id="1101"',
    )

    prefix = body[:internal_start]
    phase0_parameters = (
        '\n\t\t\t\t<parameter name="Phase 0 Probe" id="1100" flags="8589938704">'
        + drop_parameter
        + '\n\t\t\t\t\t<parameter name="Observed Render Timestamp" id="1102" flags="12884901904" default="0" value="0"/>'
        + '\n\t\t\t\t\t<parameter name="Last Drop Evidence" id="1103" flags="12884967428"><text>Waiting for one item</text></parameter>'
        + '\n\t\t\t\t</parameter>'
        + '\n\t\t\t\t<parameter name="Instance Identity" id="1901" flags="12889161760"/>'
        + '\n\t\t\t\t<parameter name="Project Payload" id="1902" flags="12889161760"/>'
        + '\n\t\t\t\t<parameter name="Timing Payload" id="1903" flags="12889161760"/>'
        + '\n\t\t\t\t<parameter name="Mix" id="10001" flags="12884901888" default="1" value="1"/>'
        + '\n\t\t\t\t<parameter name="Flip" id="10002" flags="12889161760" default="0" value="0"/>'
        + '\n\t\t\t\t<parameter name="Input Points" id="10003" flags="12889161760" default="1" value="1"/>'
    )
    replacement_filter = (
        filter_match.group("open")
        + prefix
        + internal_parameters
        + phase0_parameters
        + filter_match.group("close")
    )
    rendered = rendered[: filter_match.start()] + replacement_filter + rendered[filter_match.end() :]

    publish_settings = (
        "<publishSettings>\n"
        "\t\t<version>2</version>\n"
        "\t\t<target object=\"10036\" channel=\"./1100\" name=\"Phase 0 Probe\"/>\n"
        "\t</publishSettings>"
    )
    rendered, replacements = re.subn(
        r"<publishSettings>.*?</publishSettings>",
        publish_settings,
        rendered,
        count=1,
        flags=re.DOTALL,
    )
    if replacements != 1:
        raise ValueError("Motion template must contain one publishSettings block")
    return rendered


def generate(output: Path) -> None:
    baseline_bytes = BASELINE.read_bytes()
    actual_hash = hashlib.sha256(baseline_bytes).hexdigest()
    if actual_hash != EXPECTED_BASELINE_SHA256:
        raise ValueError(
            f"opaque Motion template baseline drifted: expected {EXPECTED_BASELINE_SHA256}, got {actual_hash}"
        )

    identity = json.loads(IDENTITY.read_text(encoding="utf-8"))
    rendered = baseline_bytes.decode("utf-8")
    rendered = rendered.replace(UPSTREAM_EFFECT_UUID, identity["effect_uuid"])
    rendered = rendered.replace(UPSTREAM_EFFECT_NAME, identity["effect_name"])
    rendered = apply_phase0_parameter_mapping(rendered)

    if UPSTREAM_EFFECT_UUID in rendered or UPSTREAM_EFFECT_NAME in rendered:
        raise ValueError("upstream identity remains in generated Motion template")

    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(rendered, encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    generate(args.output)


if __name__ == "__main__":
    main()
