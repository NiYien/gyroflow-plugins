#!/usr/bin/env python3
import argparse
import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
VERIFIER = ROOT / "scripts" / "verify_finalcut_package.py"


def run(command: list[str]) -> None:
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(
            f"command failed ({result.returncode}): {' '.join(command)}\n"
            f"{result.stdout}{result.stderr}"
        )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--app", required=True, type=Path)
    parser.add_argument("--zip", dest="zip_path", required=True, type=Path)
    parser.add_argument("--apple-id", default=os.environ.get("NOTARY_APPLE_ID"))
    parser.add_argument("--team-id", default=os.environ.get("NOTARY_TEAM_ID"))
    parser.add_argument("--password", default=os.environ.get("NOTARY_PASSWORD"))
    parser.add_argument("--keychain-profile")
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
        run(
            [
                "xcrun",
                "notarytool",
                "submit",
                "--wait",
                *credentials,
                str(arguments.zip_path),
            ]
        )
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
            temporary.replace(arguments.zip_path)
        finally:
            temporary.unlink(missing_ok=True)
        run(
            [
                sys.executable,
                str(VERIFIER),
                "--zip",
                str(arguments.zip_path),
                "--expect-signed",
                "--expect-notarized",
            ]
        )
    except (OSError, RuntimeError) as error:
        print(f"Final Cut notarization failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error


if __name__ == "__main__":
    main()
