#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Install a hash-pinned FxPlug SDK on a clean hosted macOS runner."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import plistlib
import shutil
import subprocess
import sys
import tempfile
import urllib.parse
import urllib.request
from pathlib import Path

from check_finalcut_sdk import detected_xcode_version, validate_sdk

ROOT = Path(__file__).resolve().parents[1]
PIN = ROOT / "finalcut/config/sdk.json"


def verify_download(path: Path, pin: dict) -> None:
    if path.stat().st_size != pin["size"]:
        raise ValueError("FxPlug SDK size mismatch; the response may be an HTML login page")
    if hashlib.sha256(path.read_bytes()).hexdigest() != pin["sha256"]:
        raise ValueError("FxPlug SDK SHA-256 mismatch")


def download_url(explicit: str, base: str, pin: dict) -> str:
    url = explicit or (base.rstrip("/") + "/" + pin["filename"] if base else "")
    parsed = urllib.parse.urlsplit(url)
    if parsed.scheme != "https" or not parsed.netloc or parsed.username or parsed.password:
        raise ValueError("Set FINALCUT_SDK_URL or SDK_BASE to a non-interactive HTTPS SDK source")
    return url


def prepare_download(cache: Path, pin: dict, url: str = "", local: Path | None = None) -> Path:
    cache.mkdir(parents=True, exist_ok=True)
    target = cache / pin["filename"]
    if target.is_file() and not target.is_symlink():
        try:
            verify_download(target, pin)
            return target
        except ValueError:
            pass
    if local is None and not url:
        raise ValueError("No verified SDK cache; configure FINALCUT_SDK_URL or SDK_BASE")
    with tempfile.NamedTemporaryFile(dir=cache, suffix=".download", delete=False) as output:
        temporary = Path(output.name)
        try:
            if local is not None:
                with local.open("rb") as source:
                    shutil.copyfileobj(source, output)
            else:
                request = urllib.request.Request(url, headers={
                    "User-Agent": "NiYien-FCP-SDK/1.0 (+https://www.niyien.com)",
                })
                with urllib.request.urlopen(request, timeout=120) as source:
                    if urllib.parse.urlsplit(source.url).scheme != "https":
                        raise ValueError("SDK redirect must remain HTTPS")
                    remaining = pin["size"] + 1
                    while remaining:
                        chunk = source.read(min(65536, remaining))
                        if not chunk:
                            break
                        output.write(chunk)
                        remaining -= len(chunk)
            output.flush()
            verify_download(temporary, pin)
            os.replace(temporary, target)
        finally:
            temporary.unlink(missing_ok=True)
    return target


def verify_installation(pin: dict, framework_root: Path = Path("/Library/Developer/Frameworks")) -> dict:
    sdk = validate_sdk(Path("/Library/Developer/SDKs/FxPlug.sdk"), detected_xcode_version(), "13.0")
    receipt = subprocess.run(["pkgutil", "--pkg-info-plist", pin["receipt_id"]],
                             check=True, capture_output=True).stdout
    if plistlib.loads(receipt).get("pkg-version") != pin["package_version"]:
        raise ValueError("Installed FxPlug package version does not match the pin")
    for name, version in pin["runtime_versions"].items():
        root = framework_root / f"{name}.framework" / "Versions" / version["directory"]
        info = plistlib.loads((root / "Resources/Info.plist").read_bytes())
        if (info.get("CFBundleShortVersionString"), info.get("CFBundleVersion")) != (version["version"], version["build"]):
            raise ValueError(f"Installed {name} runtime version does not match the pin")
        slices = subprocess.run(["xcrun", "lipo", "-archs", str(root / name)],
                                check=True, capture_output=True, text=True).stdout.split()
        if set(slices) != {"arm64", "x86_64"}:
            raise ValueError(f"{name} runtime is not universal")
    return {**sdk, "package_version": pin["package_version"], "sha256": pin["sha256"]}


def install(path: Path, pin: dict) -> dict:
    verify_download(path, pin)
    with tempfile.TemporaryDirectory(prefix="niyien-fxplug-") as directory:
        mount = Path(directory) / "volume"
        subprocess.run(["hdiutil", "attach", str(path), "-readonly", "-nobrowse",
                        "-mountpoint", str(mount)], check=True, capture_output=True)
        try:
            package = mount / pin["package_filename"]
            if not package.is_file() or package.is_symlink():
                raise ValueError("Pinned SDK installer package is missing")
            subprocess.run(["sudo", "installer", "-pkg", str(package), "-target", "/"], check=True)
        finally:
            subprocess.run(["hdiutil", "detach", str(mount)], check=True, capture_output=True)
    return verify_installation(pin)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache-dir", type=Path, default=ROOT / "ext/fxplug")
    parser.add_argument("--file", type=Path)
    parser.add_argument("--install", action="store_true")
    parser.add_argument("--verify-installed", action="store_true")
    args = parser.parse_args()
    pin = json.loads(PIN.read_text())
    try:
        if args.verify_installed:
            report = verify_installation(pin)
        else:
            explicit, base = os.environ.get("FINALCUT_SDK_URL", ""), os.environ.get("SDK_BASE", "")
            url = download_url(explicit, base, pin) if explicit or base else ""
            path = prepare_download(args.cache_dir, pin, url, args.file)
            report = install(path, pin) if args.install else {"path": str(path), "sha256": pin["sha256"]}
        print(json.dumps(report, indent=2))
    except Exception as error:
        # Download URLs can contain credentials in query parameters.
        print(f"FxPlug SDK preparation failed ({type(error).__name__}). "
              "Check the SDK source, pinned hash, package layout and toolchain.", file=sys.stderr)
        if isinstance(error, ValueError):
            print(str(error), file=sys.stderr)
        raise SystemExit(2) from None


if __name__ == "__main__":
    main()
