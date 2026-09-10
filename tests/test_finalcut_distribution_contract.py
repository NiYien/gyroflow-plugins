# SPDX-License-Identifier: GPL-3.0-or-later
import hashlib
import importlib.util
import json
import io
import os
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock
from contextlib import redirect_stderr, redirect_stdout

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import finalcut_release as release
import notarize_finalcut_package as notary
import prepare_finalcut_sdk as sdk
import verify_finalcut_package as verifier


class BrandingContractTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.identity = json.loads((ROOT / "finalcut/config/identity.json").read_text())
        self.app = Path(self.temporary.name) / "NiYien FCP.app"
        resources = self.app / "Contents/Resources"
        resources.mkdir(parents=True)
        shutil.copyfile(ROOT / "finalcut/Xcode/App/Resources/Icon.icns", resources / "Icon.icns")
        self.info = {
            "CFBundleName": "NiYien FCP", "CFBundleDisplayName": "NiYien FCP",
            "CFBundleExecutable": "GyroflowNiYien Final Cut", "CFBundleIconFile": "Icon.icns",
        }

    def test_real_icon_and_short_name_are_accepted(self):
        verifier.verify_branding(self.app, self.info, self.identity)

    def test_missing_or_modified_icon_is_rejected(self):
        icon = self.app / "Contents/Resources/Icon.icns"
        data = icon.read_bytes()
        icon.write_bytes(data[:-1] + bytes([data[-1] ^ 1]))
        with self.assertRaisesRegex(ValueError, "hash mismatch"):
            verifier.verify_branding(self.app, self.info, self.identity)
        icon.unlink()
        with self.assertRaisesRegex(ValueError, "missing"):
            verifier.verify_branding(self.app, self.info, self.identity)

    def test_wrong_executable_and_icon_reference_are_rejected(self):
        for key, value in (("CFBundleExecutable", "../../bin/sh"), ("CFBundleIconFile", "Absent.icns")):
            with self.subTest(key=key), self.assertRaises(ValueError):
                verifier.verify_branding(self.app, {**self.info, key: value}, self.identity)

    def test_no_entitlement_is_valid_but_malformed_output_is_not(self):
        with mock.patch.object(verifier.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, b"", b"")):
            self.assertEqual(verifier.signed_entitlements(self.app), {})
        with mock.patch.object(verifier.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, b"not plist", b"")):
            with self.assertRaises(ValueError):
                verifier.signed_entitlements(self.app)

    def test_preview_dimensions_are_read_from_the_png_header(self):
        preview = Path(self.temporary.name) / "small.png"
        preview.write_bytes(
            b"\x89PNG\r\n\x1a\n"
            + b"\x00\x00\x00\x0dIHDR"
            + (192).to_bytes(4, "big")
            + (108).to_bytes(4, "big")
        )
        self.assertEqual(verifier.png_dimensions(preview), (192, 108))
        preview.write_bytes(b"not a png")
        with self.assertRaisesRegex(ValueError, "valid PNG preview"):
            verifier.png_dimensions(preview)


class SdkDownloadContractTests(unittest.TestCase):
    def test_html_and_same_size_corruption_are_rejected(self):
        data = b"valid sdk image"
        pin = {"size": len(data), "sha256": hashlib.sha256(data).hexdigest(), "filename": "sdk.dmg"}
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.dmg"
            for body in (b"<html>Login</html>", b"x" * len(data)):
                source.write_bytes(body)
                with self.assertRaises(ValueError):
                    sdk.prepare_download(Path(directory) / "cache", pin, local=source)
                self.assertFalse((Path(directory) / "cache/sdk.dmg").exists())

    def test_bad_cache_is_replaced_only_by_verified_input(self):
        data = b"valid sdk image"
        pin = {"size": len(data), "sha256": hashlib.sha256(data).hexdigest(), "filename": "sdk.dmg"}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "sdk.dmg").write_bytes(b"bad")
            (root / "source").write_bytes(data)
            target = sdk.prepare_download(root, pin, local=root / "source")
            self.assertEqual(target.read_bytes(), data)
            self.assertEqual(sdk.prepare_download(root, pin), target)

    def test_source_requires_https_and_does_not_assume_apple_login(self):
        pin = {"filename": "FxPlug_SDK_4.3.5.dmg"}
        for url in ("", "http://example.com/sdk", "https://user:password@example.com/sdk"):
            with self.assertRaises(ValueError):
                sdk.download_url(url, "", pin)
        self.assertEqual(sdk.download_url("", "https://example.com/sdk/", pin),
                         "https://example.com/sdk/FxPlug_SDK_4.3.5.dmg")

    def test_installer_failure_detaches_the_verified_image(self):
        commands = []
        def run(command, **kwargs):
            commands.append(command)
            if "attach" in command:
                mount = Path(command[-1])
                mount.mkdir()
                (mount / "FxPlugSDK.pkg").write_bytes(b"package")
            if "installer" in command:
                raise subprocess.CalledProcessError(1, command)
            return subprocess.CompletedProcess(command, 0)
        with mock.patch.object(sdk, "verify_download"), mock.patch.object(sdk, "verify_installation") as verify, \
             mock.patch.object(sdk.subprocess, "run", side_effect=run):
            with self.assertRaises(subprocess.CalledProcessError):
                sdk.install(Path("sdk.dmg"), {"package_filename": "FxPlugSDK.pkg"})
        self.assertEqual(commands[-1][:2], ["hdiutil", "detach"])
        verify.assert_not_called()


class ReleaseContractTests(unittest.TestCase):
    def test_version_tag_mismatch_and_untrusted_events_are_rejected(self):
        self.assertEqual(release.version("2.1.2", "push", "refs/tags/v2.1.2", "42"), ("2.1.2", "42"))
        self.assertEqual(release.version("2.1.2", "workflow_dispatch", "refs/heads/main", "43"), ("2.1.2", "43"))
        for event, ref, run in (("push", "refs/tags/v2.1.3", "42"), ("pull_request", "", "42"),
                                ("workflow_dispatch", "", "0")):
            with self.assertRaises(ValueError):
                release.version("2.1.2", event, ref, run)

    def test_preflight_failure_is_visible_in_actions_and_still_blocks_the_run(self):
        output = io.StringIO()
        with mock.patch.dict(os.environ, {"GITHUB_ACTIONS": "true"}), \
             mock.patch.object(sys, "argv", ["finalcut_release.py", "prepare"]), \
             mock.patch.object(release, "prepare", side_effect=RuntimeError("Geometry is 0% verified\nRelease blocked")), \
             redirect_stdout(output):
            with self.assertRaisesRegex(SystemExit, "Release blocked"):
                release.main()
        self.assertIn("::error title=Final Cut release preflight failed::", output.getvalue())
        self.assertIn("Geometry is 0%25 verified%0ARelease blocked", output.getvalue())

    def test_local_core_dependency_cannot_enter_production(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "common").mkdir()
            (root / "common/Cargo.toml").write_text('gyroflow-core = { path = "../core" }')
            with self.assertRaisesRegex(ValueError, "local path"):
                release.validate_core(root, {"core_revision": "a" * 40})

    def test_notary_failure_does_not_reveal_password(self):
        password = "private-notary-password"
        result = subprocess.CompletedProcess([], 1, "", "failure: " + password)
        with mock.patch.object(notary.subprocess, "run", return_value=result):
            with self.assertRaises(RuntimeError) as caught:
                notary.run(["xcrun", "notarytool", "--password", password], secrets=(password,))
        self.assertNotIn(password, str(caught.exception))
        self.assertIn("<redacted>", str(caught.exception))

    def test_notary_rejected_status_prevents_staple(self):
        commands = []
        def run(command, **kwargs):
            commands.append(command)
            return json.dumps({"id": "submission", "status": "Invalid"}) if "notarytool" in command else ""
        with mock.patch.object(sys, "argv", ["notarizer", "--app", "A.app", "--zip", "a.zip", "--keychain-profile", "ci"]), \
             mock.patch.object(notary, "run", side_effect=run):
            with self.assertRaises(SystemExit):
                notary.main()
        self.assertFalse(any("stapler" in command for command in commands))

    def test_cloud_job_preserves_artifact_protocol_and_no_latest_rollout(self):
        source = (ROOT / ".github/workflows/release.yml").read_text()
        finalcut = source.split("  build_finalcut:\n", 1)[1]
        self.assertNotIn("self-hosted", finalcut)
        self.assertNotIn("schedule:", source)
        self.assertNotIn("--allow-unvalidated", finalcut)
        self.assertIn("runs-on: macos-26", finalcut)
        self.assertIn("name: GyroflowNiyien-FCP-macos-zip\n", finalcut)
        self.assertIn("path: release-finalcut/GyroflowNiyien-FinalCut-macos.zip", finalcut)
        self.assertIn("make_latest: false", source)
        self.assertNotIn("files: ./**/*", source)

    def test_notary_publishes_result_only_after_the_rebuilt_zip_passes(self):
        for failure in (None, "timeout", "staple", "verification"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                archive, result_file = root / "delivery.zip", root / "notary.json"
                archive.write_bytes(b"original")
                phases = []
                def run(command, **kwargs):
                    if "notarytool" in command:
                        phases.append("accepted")
                        if failure == "timeout":
                            raise subprocess.TimeoutExpired(["notarytool", "--password", "private-password"], 10)
                        return json.dumps({"id": "submission", "status": "Accepted"})
                    if "stapler" in command:
                        phases.append("staple")
                        if failure == "staple":
                            raise RuntimeError("staple failed")
                    if "ditto" in command:
                        phases.append("rebuild")
                        Path(command[-1]).write_bytes(b"stapled")
                    if "--expect-notarized" in command:
                        phases.append("verify")
                        self.assertEqual(archive.read_bytes(), b"original")
                        self.assertFalse(result_file.exists())
                        if failure == "verification":
                            raise RuntimeError("modified signed resource")
                    return ""
                argv = ["notarizer", "--app", str(root / "A.app"), "--zip", str(archive),
                        "--keychain-profile", "ci", "--result", str(result_file)]
                errors = io.StringIO()
                with mock.patch.object(sys, "argv", argv), mock.patch.object(notary, "run", side_effect=run), \
                     redirect_stderr(errors):
                    if failure:
                        with self.assertRaises(SystemExit):
                            notary.main()
                    else:
                        notary.main()
                self.assertNotIn("private-password", errors.getvalue())
                self.assertEqual(archive.read_bytes(), b"original" if failure else b"stapled")
                self.assertEqual(result_file.exists(), failure is None)
                if failure is None:
                    self.assertEqual(phases, ["accepted", "staple", "rebuild", "verify"])

    def test_signing_identity_requires_the_valid_certificate_and_team(self):
        fingerprint = "A" * 40
        env = {"SIGNING_FINGERPRINT": fingerprint, "NOTARY_TEAM_ID": "H59FJRN2AM"}
        identity_output = f'1) {fingerprint} "Developer ID Application: Owner (H59FJRN2AM)"'
        certificate = "-----BEGIN CERTIFICATE-----\nfixture\n-----END CERTIFICATE-----"
        info = f"SHA1 Fingerprint={fingerprint}\nnotAfter=Dec 31 00:00:00 2030 GMT\n"
        for expired in (False, True):
            with self.subTest(expired=expired), mock.patch.dict(os.environ, env), \
                 mock.patch.object(release.subprocess, "check_output", side_effect=[identity_output, certificate]), \
                 mock.patch.object(release.subprocess, "run", side_effect=[
                     subprocess.CompletedProcess([], 0, info, ""),
                     subprocess.CompletedProcess([], int(expired), "", "")]):
                if expired:
                    with self.assertRaisesRegex(ValueError, "expired"):
                        release.check_signing()
                else:
                    self.assertEqual(release.check_signing()["team_id"], env["NOTARY_TEAM_ID"])
        with mock.patch.dict(os.environ, {**env, "NOTARY_TEAM_ID": "OTHER"}):
            with self.assertRaisesRegex(ValueError, "MACOS_TEAM"):
                release.check_signing()


class SourceDigestTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        for name in ("Cargo.toml", "Cargo.lock", "common/Cargo.toml", "common/build.rs", "finalcut/Cargo.toml"):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("fixture")
        self.write("finalcut/config/identity.json", {"marketing_version": "2.1.2", "build_version": "1"})
        self.write("finalcut/config/release-inputs.json", {})

    def write(self, name, value):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value))

    def test_runtime_changes_invalidate_the_compilation_digest(self):
        original = release.source_digest(self.root)
        source = self.root / "finalcut/Xcode/Effect/GFRenderPolicy.c"
        source.parent.mkdir(parents=True)
        source.write_text("changed C runtime")
        self.assertNotEqual(release.source_digest(self.root), original)

    def test_build_number_change_preserves_the_runtime_digest(self):
        original = release.source_digest(self.root)
        self.write("finalcut/config/identity.json", {"marketing_version": "2.1.3", "build_version": "50"})
        self.assertEqual(release.source_digest(self.root), original)


if __name__ == "__main__":
    unittest.main()
