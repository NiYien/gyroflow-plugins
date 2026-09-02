#!/usr/bin/env python3
import argparse
import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin")
LIBRARY_NAME = "libgyroflow_finalcut.a"
MACOS_DEPLOYMENT_TARGET = "13.0"


def run(command: list[str], environment: dict[str, str]) -> str:
    result = subprocess.run(
        command,
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"command failed ({result.returncode}): {' '.join(command)}\n"
            f"{result.stdout}{result.stderr}"
        )
    return result.stdout.strip()


def detected_lipo() -> str:
    result = subprocess.run(
        ["xcrun", "--find", "lipo"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0 or not result.stdout.strip():
        raise RuntimeError(f"unable to locate lipo with xcrun: {result.stderr}")
    return result.stdout.strip()


def build(
    cargo: str,
    lipo: str,
    target_dir: Path,
    output: Path,
) -> tuple[str, ...]:
    environment = os.environ.copy()
    environment["CARGO_TARGET_DIR"] = str(target_dir.resolve())
    environment["MACOSX_DEPLOYMENT_TARGET"] = MACOS_DEPLOYMENT_TARGET

    libraries: list[Path] = []
    for target in TARGETS:
        run(
            [
                cargo,
                "build",
                "-p",
                "gyroflow-finalcut",
                "--release",
                "--target",
                target,
            ],
            environment,
        )
        library = target_dir / target / "release" / LIBRARY_NAME
        if not library.is_file():
            raise RuntimeError(f"Rust build did not produce {library}")
        libraries.append(library)

    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_name(f".{output.name}.{os.getpid()}.tmp")
    try:
        run(
            [
                lipo,
                "-create",
                *(str(library) for library in libraries),
                "-output",
                str(temporary),
            ],
            environment,
        )
        architectures = tuple(run([lipo, "-archs", str(temporary)], environment).split())
        if set(architectures) != {"arm64", "x86_64"}:
            raise RuntimeError(
                f"universal Rust library has unexpected architectures: {architectures}"
            )
        os.replace(temporary, output)
    finally:
        if temporary.exists():
            temporary.unlink()
    return architectures


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Build the universal Rust static library for Final Cut."
    )
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--lipo")
    parser.add_argument("--target-dir", type=Path, default=ROOT / "target")
    parser.add_argument(
        "--output",
        type=Path,
        default=ROOT / "finalcut" / "build" / LIBRARY_NAME,
    )
    args = parser.parse_args()

    try:
        architectures = build(
            args.cargo,
            args.lipo or detected_lipo(),
            args.target_dir,
            args.output,
        )
    except (OSError, RuntimeError) as error:
        print(f"Final Cut Rust build failed: {error}", file=sys.stderr)
        return 2

    print(
        f"Built {args.output.resolve()} architectures: {' '.join(architectures)}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
