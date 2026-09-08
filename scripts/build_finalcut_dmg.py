#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Create a drag-to-Applications disk image from a verified NiYien FCP App."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

from notarize_finalcut_package import run
from verify_finalcut_package import APP_NAME, verify_app


def verify_image(path: Path, notarized: bool) -> None:
    run(["codesign", "--verify", "--strict", str(path)])
    with tempfile.TemporaryDirectory(prefix="niyien-fcp-dmg-check-") as directory:
        volume = Path(directory) / "volume"
        run(["hdiutil", "attach", str(path), "-readonly", "-nobrowse", "-mountpoint", str(volume)])
        try:
            visible = {entry.name for entry in volume.iterdir() if not entry.name.startswith(".")}
            if visible != {APP_NAME, "Applications"}:
                raise ValueError("Disk image must contain only NiYien FCP.app and Applications")
            applications = volume / "Applications"
            if not applications.is_symlink() or os.readlink(applications) != "/Applications":
                raise ValueError("Disk image Applications link must target /Applications")
            verify_app(volume / APP_NAME, True, notarized)
        finally:
            run(["hdiutil", "detach", str(volume)])


def build(app: Path, output: Path, identity: str, notarize: bool = False) -> dict:
    if output.exists() or output.is_symlink():
        raise FileExistsError("Refusing to overwrite an existing disk image")
    if not identity:
        raise ValueError("A Developer ID signing identity is required")
    verify_app(app, True, notarize)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="niyien-fcp-dmg-", dir=output.parent) as directory:
        temporary = Path(directory)
        stage = temporary / "contents"
        stage.mkdir()
        run(["ditto", str(app), str(stage / APP_NAME)])
        (stage / "Applications").symlink_to("/Applications", target_is_directory=True)
        image = temporary / "NiYien FCP.dmg"
        run(["hdiutil", "create", str(image), "-volname", "NiYien FCP", "-fs", "HFS+",
             "-srcfolder", str(stage), "-format", "UDZO", "-imagekey", "zlib-level=9"])
        run(["codesign", "--force", "--sign", identity, "--timestamp", "--options", "runtime", str(image)])
        submission = None
        if notarize:
            account, team, password = (os.environ.get(name, "") for name in
                                      ("NOTARY_APPLE_ID", "NOTARY_TEAM_ID", "NOTARY_PASSWORD"))
            if not all((account, team, password)):
                raise ValueError("DMG notarization requires the configured notary credentials")
            result = json.loads(run(["xcrun", "notarytool", "submit", str(image), "--wait",
                                     "--output-format", "json", "--apple-id", account,
                                     "--team-id", team, "--password", password], secrets=(account, password)))
            if result.get("status") != "Accepted" or not result.get("id"):
                raise ValueError("DMG notarization did not return Accepted")
            submission = result["id"]
            run(["xcrun", "stapler", "staple", str(image)])
            run(["xcrun", "stapler", "validate", str(image)])
        verify_image(image, notarize)
        image.replace(output)
    report = {"filename": output.name, "size": output.stat().st_size,
              "sha256": hashlib.sha256(output.read_bytes()).hexdigest(),
              "notarized": notarize, "notary_submission_id": submission}
    output.with_suffix(".json").write_text(json.dumps(report, indent=2) + "\n")
    return report


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--signing-identity", default=os.environ.get("SIGNING_FINGERPRINT", ""))
    parser.add_argument("--notarize", action="store_true")
    args = parser.parse_args()
    try:
        print(json.dumps(build(args.app.resolve(), args.output.absolute(), args.signing_identity, args.notarize), indent=2))
    except subprocess.TimeoutExpired:
        raise SystemExit("Final Cut DMG creation or notarization timed out") from None
    except (OSError, RuntimeError, ValueError) as error:
        print(f"Final Cut DMG build failed: {error}", file=sys.stderr)
        raise SystemExit(2) from None


if __name__ == "__main__":
    main()
