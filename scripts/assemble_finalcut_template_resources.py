#!/usr/bin/env python3
import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
GENERATOR = ROOT / "scripts" / "generate_finalcut_template.py"
VERIFIER = ROOT / "scripts" / "verify_finalcut_template.py"
XPC_INFO = ROOT / "finalcut" / "xcode" / "Effect" / "Info.plist"
PREVIEW_SOURCE = ROOT / "adobe" / "logo_white.png"
ATTRIBUTION = ROOT / "finalcut" / "template" / "UPSTREAM.md"


def run(command: list[str]) -> None:
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(
            f"command failed ({result.returncode}): {' '.join(command)}\n"
            f"{result.stdout}{result.stderr}"
        )


def preview(source: Path, destination: Path, maximum: int, height: int, width: int) -> None:
    resized = destination.with_name(f".{destination.name}.resized.png")
    try:
        run(["sips", "-Z", str(maximum), str(source), "--out", str(resized)])
        run(
            [
                "sips",
                "-p",
                str(height),
                str(width),
                "--padColor",
                "1D1D1D",
                str(resized),
                "--out",
                str(destination),
            ]
        )
    finally:
        resized.unlink(missing_ok=True)


def assemble(output: Path) -> None:
    if output.exists():
        raise FileExistsError(f"refusing to replace existing output: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix="finalcut-template-",
        dir=output.parent,
    ) as directory:
        staging = Path(directory) / "Gyroflow"
        staging.mkdir()
        template = staging / "Gyroflow NiYien.moef"
        run([sys.executable, str(GENERATOR), "--output", str(template)])
        run([sys.executable, str(VERIFIER), str(template), str(XPC_INFO)])
        preview(PREVIEW_SOURCE, staging / "large.png", 240, 300, 400)
        preview(PREVIEW_SOURCE, staging / "small.png", 48, 60, 80)
        shutil.copy2(ATTRIBUTION, staging / "UPSTREAM.md")
        staging.rename(output)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()
    try:
        assemble(arguments.output)
    except (OSError, RuntimeError) as error:
        print(f"Final Cut template assembly failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error


if __name__ == "__main__":
    main()
