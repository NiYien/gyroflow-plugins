# SPDX-License-Identifier: GPL-3.0-or-later
import json
import plistlib
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
HOST = "com.apple.FinalCutApp"
LOCALES = ("en", "zh-Hans", "zh-Hant", "ja", "ko", "ru")


class FinalCutHostLocalizationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory()
        cls.root = Path(cls.temporary.name)
        contents = cls.root / "LanguageHarness.app" / "Contents"
        cls.executable = contents / "MacOS" / "LanguageHarness"
        cls.executable.parent.mkdir(parents=True)
        (contents / "Info.plist").write_bytes(plistlib.dumps({
            "CFBundleIdentifier": "test.niyien.finalcut.host-localization",
            "CFBundleExecutable": "LanguageHarness",
            "CFBundlePackageType": "APPL",
            "CFBundleDevelopmentRegion": "en",
        }))
        for locale in LOCALES:
            resources = contents / "Resources" / f"{locale}.lproj"
            resources.mkdir(parents=True)
            (resources / "Localizable.strings").write_text(
                f'"probe.key" = "{locale}";\n', encoding="utf-8"
            )
        subprocess.run([
            "xcrun", "clang", "-fobjc-arc", "-fblocks", "-Wall", "-Wextra", "-Werror",
            "-framework", "Foundation", "-I", str(ROOT / "finalcut/Xcode/Effect"),
            str(ROOT / "finalcut/Xcode/Effect/GFLocalization.m"),
            str(ROOT / "tests/helpers/finalcut_host_localization_main.m"),
            "-o", str(cls.executable),
        ], check=True, capture_output=True, text=True)

    @classmethod
    def tearDownClass(cls):
        cls.temporary.cleanup()

    def fixture(self, name, preferences, system=None, host=HOST):
        home = self.root / name
        path = home / "Library/Containers" / host / "Data/Library/Preferences" / f"{host}.plist"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(preferences if isinstance(preferences, bytes) else plistlib.dumps(preferences))
        return {"home": str(home), "host": host, "system": system or ["zh-Hans-CN", "en-CN"]}

    def run_cases(self, cases):
        files = list(self.root.glob("*/Library/Containers/*/Data/Library/Preferences/*.plist"))
        before = {path: path.read_bytes() for path in files}
        result = subprocess.run([
            str(self.executable), json.dumps(cases), "-AppleLanguages", "(zh-Hans)",
        ], check=True, capture_output=True, text=True)
        report = json.loads(result.stdout)
        self.assertEqual(report["before"], report["after"])
        self.assertEqual(before, {path: path.read_bytes() for path in files})
        return report["results"]

    def test_host_override_switches_both_directions_without_changing_app_preferences(self):
        cases = [
            self.fixture("english", {"AppleLanguages": ["en-CN", "zh-Hans"]}),
            self.fixture("chinese", {"AppleLanguages": ["zh-Hans-CN"]}),
            self.fixture("english-again", {"AppleLanguages": ["en-US"]}),
            self.fixture("removed", {"UnrelatedSetting": "preserved"}),
        ]
        self.assertEqual([item["text"] for item in self.run_cases(cases)],
                         ["en", "zh-Hans", "en", "zh-Hans"])

    def test_all_six_host_languages_and_unsupported_language_fallback(self):
        identifiers = ["en-CN", "zh-Hans-CN", "zh-Hant-TW", "ja-JP", "ko-KR", "ru-RU"]
        cases = [self.fixture(f"locale-{locale}", {"AppleLanguages": [identifier]})
                 for locale, identifier in zip(LOCALES, identifiers)]
        cases += [self.fixture("unsupported", {"AppleLanguages": ["fr-FR"]}),
                  self.fixture("ordered", {"AppleLanguages": ["fr-FR", "ru-RU"]})]
        self.assertEqual([item["text"] for item in self.run_cases(cases)],
                         [*LOCALES, "en", "ru"])

    def test_invalid_or_missing_override_uses_system_language(self):
        preferences = [{}, {"AppleLanguages": []}, {"AppleLanguages": "en"},
                       {"AppleLanguages": ["en", 1]}, b"not a plist"]
        cases = [self.fixture(f"invalid-{index}", value, ["ja-JP"])
                 for index, value in enumerate(preferences)]
        self.assertEqual([item["text"] for item in self.run_cases(cases)], ["ja"] * len(cases))

    def test_only_known_final_cut_hosts_are_read(self):
        known = self.fixture("classic", {"AppleLanguages": ["ko-KR"]}, host="com.apple.FinalCut")
        unknown = self.fixture("unknown", {"AppleLanguages": ["en"]}, host="example.other-app")
        self.assertEqual([item["text"] for item in self.run_cases([known, unknown])], ["ko", "zh-Hans"])

    def test_linked_preference_file_is_not_followed(self):
        case = self.fixture("linked", {}, ["ru-RU"])
        path = Path(case["home"]) / "Library/Containers" / HOST / "Data/Library/Preferences" / f"{HOST}.plist"
        destination = self.root / "unrelated.plist"
        destination.write_bytes(plistlib.dumps({"AppleLanguages": ["en"]}))
        path.unlink()
        path.symlink_to(destination)
        self.assertEqual(self.run_cases([case])[0]["text"], "ru")


if __name__ == "__main__":
    unittest.main()
