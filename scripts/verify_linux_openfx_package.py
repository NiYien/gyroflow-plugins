from __future__ import annotations

import argparse
import stat
import zipfile
from pathlib import Path


BINARY_PATH = "GyroflowNiyien.ofx.bundle/Contents/Linux-x86-64/GyroflowNiyien.ofx"
VERSION_PATH = "GyroflowNiyien.ofx.bundle/Contents/version.txt"
REQUIRED_MEMBERS = (
    BINARY_PATH,
    "GyroflowNiyien.ofx.bundle/Contents/Info.plist",
    "GyroflowNiyien.ofx.bundle/Contents/LICENSE",
    VERSION_PATH,
    "ResolveScripts/Gyroflow NiYien Auto Cut Current Clip.lua",
    "ResolveScripts/Gyroflow NiYien Auto Cut Current Track.lua",
    "ResolveScripts/gyroflow_autocut_common.inc",
)


def normalized_members(archive: zipfile.ZipFile) -> dict[str, zipfile.ZipInfo]:
    return {info.filename.removeprefix("./"): info for info in archive.infolist()}


def verify_package(package: Path) -> dict[str, int | str]:
    package = Path(package)
    if not package.is_file() or package.stat().st_size == 0:
        raise ValueError(f"Linux OpenFX package is missing or empty: {package}")

    with zipfile.ZipFile(package) as archive:
        damaged_member = archive.testzip()
        if damaged_member:
            raise ValueError(f"Linux OpenFX package contains a damaged member: {damaged_member}")

        members = normalized_members(archive)
        for required in REQUIRED_MEMBERS:
            info = members.get(required)
            if info is None:
                raise ValueError(f"Linux OpenFX package is missing required member: {required}")
            if info.file_size == 0:
                raise ValueError(f"Linux OpenFX package contains an empty required member: {required}")

        binary_mode = members[BINARY_PATH].external_attr >> 16
        if binary_mode & stat.S_IXUSR == 0:
            raise ValueError(f"Linux OpenFX binary is not owner-executable: {BINARY_PATH}")

        version = archive.read(members[VERSION_PATH]).decode("utf-8").strip()
        if not version:
            raise ValueError(f"Linux OpenFX version file is empty: {VERSION_PATH}")

    return {"version": version, "binary_mode": binary_mode}


def main() -> int:
    parser = argparse.ArgumentParser(description="Validate the Linux x86_64 OpenFX release package.")
    parser.add_argument("package", type=Path)
    args = parser.parse_args()
    metadata = verify_package(args.package)
    print(f"Validated {args.package}: version={metadata['version']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
