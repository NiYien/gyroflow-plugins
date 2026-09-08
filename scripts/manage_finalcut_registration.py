#!/usr/bin/env python3
from __future__ import annotations

import argparse
import plistlib
import subprocess
from pathlib import Path


PRODUCTION_APP = Path("/Applications/NiYien FCP.app")
XPC_NAME = "GyroflowNiYienFinalCutEffect.pluginkit"
XPC_BUNDLE_ID = "com.niyien.gyroflow.finalcut.effect"


def xpc_for_app(app: Path) -> Path:
    return app.resolve() / "Contents" / "PlugIns" / XPC_NAME


def validate_xpc(xpc: Path) -> None:
    info = xpc / "Contents" / "Info.plist"
    if not xpc.is_dir() or not info.is_file():
        raise ValueError(f"FxPlug XPC is missing: {xpc}")
    with info.open("rb") as source:
        metadata = plistlib.load(source)
    if metadata.get("CFBundleIdentifier") != XPC_BUNDLE_ID:
        raise ValueError(f"FxPlug XPC identity mismatch: {xpc}")


def run_pluginkit(arguments: list[str]) -> None:
    subprocess.run(["/usr/bin/pluginkit", *arguments], check=True)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Explicitly inspect or change the local Final Cut FxPlug registration."
    )
    parser.add_argument("action", choices=("status", "register", "unregister"))
    parser.add_argument("--app", type=Path, default=PRODUCTION_APP)
    arguments = parser.parse_args()

    if arguments.action == "status":
        print(f"Expected production App: {arguments.app.resolve()}")
        run_pluginkit(["-m", "-A", "-D", "-v", "-i", XPC_BUNDLE_ID])
        return

    xpc = xpc_for_app(arguments.app)
    validate_xpc(xpc)
    operation = "-a" if arguments.action == "register" else "-r"
    print(f"{arguments.action.title()}ing exact FxPlug path: {xpc}")
    run_pluginkit([operation, str(xpc)])
    print("Current registrations:")
    run_pluginkit(["-m", "-A", "-D", "-v", "-i", XPC_BUNDLE_ID])


if __name__ == "__main__":
    main()
