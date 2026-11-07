#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Require an Accepted notary result and preserve rejection diagnostics."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys


def run(command, secrets):
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=1800)
    except subprocess.TimeoutExpired:
        raise RuntimeError("Notary command timed out") from None
    if result.returncode:
        details = result.stdout + result.stderr
        for value in secrets:
            details = details.replace(value, "<redacted>")
        raise RuntimeError(f"Notary command failed ({result.returncode}):\n{details}")
    return result.stdout


def notarize(archive, result_path, credentials, secrets):
    log_path = result_path.with_name(result_path.stem + "-log.json")
    result_path.unlink(missing_ok=True)
    log_path.unlink(missing_ok=True)
    output = run(["xcrun", "notarytool", "submit", "--wait", "--output-format", "json",
                  *credentials, str(archive)], secrets)
    result = json.loads(output)
    result_path.parent.mkdir(parents=True, exist_ok=True)
    result_path.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    if (not isinstance(result, dict) or not isinstance(result.get("id"), str)
            or not result["id"] or not isinstance(result.get("status"), str)):
        raise ValueError("Notarytool did not return a submission ID and status")
    if result["status"] != "Accepted":
        try:
            run(["xcrun", "notarytool", "log", result["id"], *credentials, str(log_path)], secrets)
            print(f"Apple rejection log: {log_path}", file=sys.stderr)
        except RuntimeError as error:
            print(f"Unable to download the Apple rejection log: {error}", file=sys.stderr)
        raise RuntimeError(f"Apple notarization returned {result['status']} for {result['id']}")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--zip", type=Path, required=True)
    parser.add_argument("--result", type=Path, required=True)
    parser.add_argument("--apple-id", default=os.environ.get("NOTARY_APPLE_ID"))
    parser.add_argument("--team-id", default=os.environ.get("NOTARY_TEAM_ID"))
    parser.add_argument("--password", default=os.environ.get("NOTARY_PASSWORD"))
    args = parser.parse_args()
    if not all((args.apple_id, args.team_id, args.password)):
        parser.error("Notary Apple ID, Team ID and password are required")
    credentials = ["--apple-id", args.apple_id, "--team-id", args.team_id, "--password", args.password]
    try:
        result = notarize(args.zip, args.result, credentials, (args.apple_id, args.password))
        print(f"Apple notarization Accepted: {result['id']}")
    except (OSError, RuntimeError, ValueError) as error:
        print(f"Jianying notarization failed: {error}", file=sys.stderr)
        raise SystemExit(2) from None


if __name__ == "__main__":
    main()
