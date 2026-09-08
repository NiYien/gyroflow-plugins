#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Validate release inputs and record the final notarized deliverable."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
INPUTS = ROOT / "finalcut/config/release-inputs.json"
IDENTITY = ROOT / "finalcut/config/identity.json"
ZIP_NAME = "GyroflowNiyien-FinalCut-macos.zip"


def source_digest(root: Path) -> str:
    digest = hashlib.sha256()
    paths = [path for directory in ("finalcut/src", "finalcut/include", "finalcut/Xcode", "common/src",
                                   "finalcut/locales", "common/locales", "finalcut/template")
             for path in (root / directory).rglob("*") if path.is_file()
             and path.suffix in (".rs", ".c", ".cpp", ".m", ".mm", ".h", ".hpp", ".swift", ".plist",
                                 ".entitlements", ".metal", ".xcstrings", ".icns", ".pbxproj", ".json", ".moef", ".png")]
    paths.extend(root / name for name in ("finalcut/config/identity.json", "finalcut/config/release-inputs.json",
                                         "Cargo.toml", "Cargo.lock", "common/Cargo.toml", "common/build.rs", "finalcut/Cargo.toml"))
    for path in sorted(paths):
        data = path.read_bytes()
        if path.suffix == ".pbxproj":
            data = re.sub(rb"(?:CURRENT_PROJECT_VERSION|MARKETING_VERSION) = [^;]+;", b"VERSION_NORMALIZED;", data)
        elif path.name == "identity.json":
            identity = json.loads(data)
            identity.pop("build_version", None)
            identity.pop("marketing_version", None)
            data = json.dumps(identity, sort_keys=True).encode()
        digest.update(str(path.relative_to(root)).encode() + b"\0" + data + b"\0")
    return digest.hexdigest()


def version(base: str, event: str, ref: str, run_number: str) -> tuple[str, str]:
    if not re.fullmatch(r"\d+\.\d+\.\d+", base):
        raise ValueError("Plugin base version must contain three numeric components")
    if event == "push":
        if ref != f"refs/tags/v{base}":
            raise ValueError("Release tag must be v<plugin base version>")
    elif event != "workflow_dispatch":
        raise ValueError("Only a version tag or workflow_dispatch can create a release candidate")
    if not run_number.isdigit() or int(run_number) < 1:
        raise ValueError("A positive GitHub run number is required")
    return base, str(int(run_number))


def validate_core(root: Path, inputs: dict) -> None:
    manifest = (root / "common/Cargo.toml").read_text()
    active = "\n".join(line for line in manifest.splitlines() if not line.lstrip().startswith("#"))
    match = re.search(r"^gyroflow-core\s*=\s*\{([^}]+)\}", active, re.MULTILINE)
    if not match or re.search(r"\bpath\s*=", match[1]):
        raise ValueError("Production gyroflow-core must use the published pinned Git revision, not a local path")
    if not re.search(r'\brev\s*=\s*"' + re.escape(inputs["core_revision"]) + '"', match[1]):
        raise ValueError("gyroflow-core revision does not match release-inputs.json")
    if not re.search(r'\bgit\s*=\s*"https://github.com/NiYien/gyroflow.git"', match[1]):
        raise ValueError("gyroflow-core must use the NiYien Git repository")
    lock = (root / "Cargo.lock").read_text()
    blocks = re.split(r"\[\[package\]\]", lock)
    core = [block for block in blocks if re.search(r'^name = "gyroflow-core"$', block, re.MULTILINE)]
    expected = f'git+https://github.com/NiYien/gyroflow.git?rev={inputs["core_revision"]}#{inputs["core_revision"]}'
    if len(core) != 1 or f'source = "{expected}"' not in core[0]:
        raise ValueError("Cargo.lock must resolve gyroflow-core to the pinned Git revision")
    for name in (".cargo/config", ".cargo/config.toml"):
        config = root / name
        if config.exists() and "gyroflow-core" in config.read_text():
            raise ValueError("Production checkout must not override gyroflow-core locally")


def require_performance(root: Path) -> None:
    performance = json.loads((root / "finalcut/validation/performance-baseline.json").read_text())
    if performance.get("release_blocked", True) or performance.get("capture_status") != "captured":
        raise ValueError("Final Cut performance acceptance is not captured or release-ready")


def require_runtime_evidence(root: Path) -> None:
    acceptance = json.loads((root / "finalcut/validation/release-acceptance.json").read_text())
    needed = ("branding", "localization", "upgrade", "file_access")
    if acceptance.get("release_blocked", True) or not all(acceptance.get(key) for key in needed):
        raise ValueError("Final Cut branding, language, upgrade and file-access acceptance must pass")
    sdk = json.loads((root / "finalcut/config/sdk.json").read_text())
    if acceptance.get("source_sha256") != source_digest(root) or acceptance.get("sdk_sha256") != sdk["sha256"]:
        raise ValueError("Final Cut acceptance evidence does not match the current runtime or SDK")
    for key in needed:
        evidence = root / acceptance[key]
        if not evidence.resolve().is_relative_to(root.resolve()) or not evidence.is_file():
            raise ValueError(f"Final Cut {key} acceptance evidence is missing")


def acceptance_blockers(root: Path) -> list[str]:
    from build_finalcut_package import require_capacity_gate, require_geometry_gate
    checks = (
        lambda: require_capacity_gate(False, root / "finalcut/config/capacity-gate.json"),
        lambda: require_geometry_gate(False, root / "finalcut/validation/geometry-support.json"),
        lambda: require_performance(root),
        lambda: require_runtime_evidence(root),
    )
    blockers = []
    for check in checks:
        try:
            check()
        except json.JSONDecodeError:
            raise
        except (ValueError, RuntimeError) as error:
            blockers.append(str(error))
    return blockers


def require_acceptance(root: Path) -> None:
    blockers = acceptance_blockers(root)
    if blockers:
        raise ValueError("; ".join(blockers))


def check_acceptance(directory: Path, root: Path = ROOT) -> dict:
    blockers = acceptance_blockers(root)
    candidate = bool(blockers) and os.environ.get("GITHUB_EVENT_NAME") == "workflow_dispatch"
    result = {"ready": not blockers, "blockers": blockers,
              "delivery": "candidate" if candidate else ("validated" if not blockers else "blocked"),
              "package": not blockers or candidate}
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "release-readiness.json").write_text(json.dumps(result, indent=2) + "\n")
    output_path = os.environ.get("GITHUB_OUTPUT")
    if output_path:
        with open(output_path, "a") as output:
            output.write(f"ready={str(result['ready']).lower()}\n")
            output.write(f"package={str(result['package']).lower()}\n")
            output.write(f"delivery={result['delivery']}\n")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as output:
            output.write("### NiYien FCP release acceptance\n\n")
            if blockers:
                output.write("**Release pending acceptance.** ")
                if candidate:
                    output.write("This manual run will sign and notarize installable candidate ZIP/DMG artifacts "
                                 "for host validation. The standard update artifact and GitHub Release remain blocked.\n\n")
                else:
                    output.write("Signing, notarization and installable artifacts are skipped.\n\n")
                output.writelines(f"- {reason}\n" for reason in blockers)
                output.write("\n")
            else:
                output.write("Acceptance passed; signing and notarization may proceed.\n\n")
    if blockers and os.environ.get("GITHUB_ACTIONS") == "true":
        message = "; ".join(blockers).replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
        print(f"::warning title=Final Cut release pending acceptance::{message}")
    return result


def prepare(root: Path = ROOT) -> dict:
    inputs = json.loads((root / "finalcut/config/release-inputs.json").read_text())
    validate_core(root, inputs)
    cargo = (root / "Cargo.toml").read_text()
    base = re.search(r'\[workspace.package\]\s*version\s*=\s*"([^"]+)"', cargo)[1]
    marketing, build = version(base, os.environ.get("GITHUB_EVENT_NAME", ""),
                               os.environ.get("GITHUB_REF", ""), os.environ.get("GITHUB_RUN_NUMBER", ""))
    identity_path = root / "finalcut/config/identity.json"
    identity = json.loads(identity_path.read_text())
    identity.update(marketing_version=marketing, build_version=build)
    identity_path.write_text(json.dumps(identity, indent=2) + "\n")
    project = root / "finalcut/Xcode/GyroflowFinalCut.xcodeproj/project.pbxproj"
    source = project.read_text()
    source = re.sub(r"CURRENT_PROJECT_VERSION = [^;]+;", f"CURRENT_PROJECT_VERSION = {build};", source)
    source = re.sub(r"MARKETING_VERSION = [^;]+;", f"MARKETING_VERSION = {marketing};", source)
    project.write_text(source)
    return {**inputs, "marketing_version": marketing, "build_version": build}


def check_toolchain() -> dict:
    from check_finalcut_sdk import detected_xcode_version
    inputs = json.loads(INPUTS.read_text())
    rust = subprocess.check_output(["rustc", "--version"], text=True).split()[1]
    actual = {"xcode_version": detected_xcode_version(), "rust_toolchain": rust,
              "xcode_path": os.environ.get("DEVELOPER_DIR"),
              "lens_data_tag": os.environ.get("NIYIEN_LENS_DATA_TAG")}
    if any(value != inputs[key] for key, value in actual.items()):
        raise ValueError("Selected Xcode, Rust or lens data does not match release-inputs.json")
    return actual


def check_signing() -> dict:
    identity = json.loads(IDENTITY.read_text())
    fingerprint = os.environ.get("SIGNING_FINGERPRINT", "").upper()
    if not re.fullmatch(r"[A-F0-9]{40}", fingerprint):
        raise ValueError("MACOS_CERTIFICATE_FINGERPRINT must be a SHA-1 certificate fingerprint")
    if os.environ.get("NOTARY_TEAM_ID") != identity["team_id"]:
        raise ValueError("MACOS_TEAM does not match the Final Cut identity")
    valid = subprocess.check_output(["security", "find-identity", "-v", "-p", "codesigning"], text=True)
    pattern = r'\b' + fingerprint + r' "Developer ID Application: [^"\n]+ \(' + re.escape(identity["team_id"]) + r'\)"'
    if not re.search(pattern, valid):
        raise ValueError("Configured Developer ID private key/certificate is not valid for the expected Team")
    certificates = subprocess.check_output(["security", "find-certificate", "-a", "-p"], text=True)
    for pem in re.findall(r"-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----", certificates, re.DOTALL):
        result = subprocess.run(["openssl", "x509", "-noout", "-sha1", "-fingerprint", "-enddate"],
                                input=pem, capture_output=True, text=True, check=True)
        lines = result.stdout.splitlines()
        if lines[0].split("=", 1)[1].replace(":", "").upper() != fingerprint:
            continue
        expiry = subprocess.run(["openssl", "x509", "-noout", "-checkend", "0"],
                                input=pem, capture_output=True, text=True)
        if expiry.returncode != 0:
            raise ValueError("Configured Developer ID certificate is expired")
        return {"certificate_type": "Developer ID Application", "team_id": identity["team_id"],
                "certificate_valid_until": lines[1].split("=", 1)[1]}
    raise ValueError("Configured Developer ID certificate could not be inspected")


def summarize(directory: Path) -> dict:
    artifact = directory / ZIP_NAME
    inputs = json.loads(INPUTS.read_text())
    identity = json.loads(IDENTITY.read_text())
    sdk = json.loads((ROOT / "finalcut/config/sdk.json").read_text())
    notary = json.loads((directory / "notary-result.json").read_text())
    if notary.get("status") != "Accepted":
        raise ValueError("Summary requires an Accepted notarization result")
    distribution = json.loads((directory / "distribution-status.json").read_text())
    report = {
        "distribution": distribution,
        "plugin_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        **inputs, "sdk_sha256": sdk["sha256"], "sdk_package_version": sdk["package_version"],
        "version": identity["marketing_version"], "build_version": identity["build_version"],
        "filename": artifact.name, "size": artifact.stat().st_size,
        "sha256": hashlib.sha256(artifact.read_bytes()).hexdigest(),
        "notary_submission_id": notary["id"],
    }
    (directory / "finalcut-release-summary.json").write_text(json.dumps(report, indent=2) + "\n")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as output:
            output.write("### NiYien FCP\n\n```json\n" + json.dumps(report, indent=2) + "\n```\n")
    return report


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "summary", "check-toolchain", "check-signing",
                                          "check-acceptance", "require-acceptance"))
    parser.add_argument("--directory", type=Path, default=ROOT / "release-finalcut")
    args = parser.parse_args()
    try:
        actions = {"prepare": prepare, "summary": lambda: summarize(args.directory),
                   "check-toolchain": check_toolchain, "check-signing": check_signing,
                   "check-acceptance": lambda: check_acceptance(args.directory),
                   "require-acceptance": lambda: require_acceptance(ROOT) or {"ready": True}}
        print(json.dumps(actions[args.action](), indent=2))
    except (ValueError, OSError, KeyError, RuntimeError) as error:
        if os.environ.get("GITHUB_ACTIONS") == "true":
            message = str(error).replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
            print(f"::error title=Final Cut release preflight failed::{message}")
        raise SystemExit(f"Final Cut release preflight failed: {error}") from None


if __name__ == "__main__":
    main()
