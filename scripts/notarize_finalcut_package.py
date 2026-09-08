#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
import argparse
import json
import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
VERIFIER = ROOT / "scripts" / "verify_finalcut_package.py"


def run(command: list[str], secrets: tuple[str, ...] = ()) -> str:
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=1800)
    if result.returncode != 0:
        details = f"{result.stdout}{result.stderr}"
        for value in secrets:
            if value:
                details = details.replace(value, "<redacted>")
        raise RuntimeError(
            f"{Path(command[0]).name} failed ({result.returncode})\n{details}"
        )
    return result.stdout


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--app", required=True, type=Path)
    parser.add_argument("--zip", dest="zip_path", required=True, type=Path)
    parser.add_argument("--apple-id", default=os.environ.get("NOTARY_APPLE_ID"))
    parser.add_argument("--team-id", default=os.environ.get("NOTARY_TEAM_ID"))
    parser.add_argument("--password", default=os.environ.get("NOTARY_PASSWORD"))
    parser.add_argument("--keychain-profile")
    parser.add_argument("--result", type=Path)
    arguments = parser.parse_args()
    try:
        run(
            [
                sys.executable,
                str(VERIFIER),
                "--app",
                str(arguments.app),
                "--expect-signed",
            ]
        )
        credentials: list[str]
        if arguments.keychain_profile:
            credentials = ["--keychain-profile", arguments.keychain_profile]
        elif arguments.apple_id and arguments.team_id and arguments.password:
            credentials = [
                "--apple-id",
                arguments.apple_id,
                "--team-id",
                arguments.team_id,
                "--password",
                arguments.password,
            ]
        else:
            raise RuntimeError("notary credentials or --keychain-profile are required")
        notary_output = run(
            [
                "xcrun",
                "notarytool",
                "submit",
                "--wait",
                "--output-format",
                "json",
                *credentials,
                str(arguments.zip_path),
            ],
            secrets=tuple(value for value in (arguments.password, arguments.apple_id) if value),
        )
        result = json.loads(notary_output)
        if result.get("status") != "Accepted" or not result.get("id"):
            raise RuntimeError("Apple notarization did not return Accepted")
        run(["xcrun", "stapler", "staple", "--verbose", str(arguments.app)])
        temporary = arguments.zip_path.with_name(f".{arguments.zip_path.name}.notarized")
        try:
            run(
                [
                    "ditto",
                    "-c",
                    "-k",
                    "--keepParent",
                    str(arguments.app),
                    str(temporary),
                ]
            )
            run([sys.executable, str(VERIFIER), "--zip", str(temporary),
                 "--expect-signed", "--expect-notarized"])
            temporary.replace(arguments.zip_path)
        finally:
            temporary.unlink(missing_ok=True)
        if arguments.result:
            arguments.result.write_text(json.dumps({"id": result["id"], "status": "Accepted"}) + "\n")
    except subprocess.TimeoutExpired:
        print("Final Cut notarization timed out; no deliverable was published", file=sys.stderr)
        raise SystemExit(2) from None
    except (OSError, RuntimeError, ValueError) as error:
        print(f"Final Cut notarization failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error


if __name__ == "__main__":
    main()
