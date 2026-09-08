import importlib.util
import json
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
LOCALES = ("en", "zh-CN", "zh-TW", "ja", "ko", "ru")
XCODE_LOCALES = ("en", "zh-Hans", "zh-Hant", "ja", "ko", "ru")
GENERATOR = ROOT / "scripts" / "generate_finalcut_localizations.py"
CATALOG = ROOT / "finalcut" / "Xcode" / "Shared" / "Localizable.xcstrings"
INFO_CATALOG = ROOT / "finalcut" / "Xcode" / "App" / "InfoPlist.xcstrings"
FINAL_LOCALES = ROOT / "finalcut" / "locales"
APP_SOURCE = ROOT / "finalcut" / "Xcode" / "App"
EFFECT_SOURCE = ROOT / "finalcut" / "Xcode" / "Effect"

SHARED_KEYS = {
    "effect.group.project": "group.project",
    "effect.group.adjust": "group.adjust",
    "effect.label.status": "label.status",
    "effect.action.load_current": "label.load_current",
    "effect.action.browse": "label.browse",
    "effect.param.fov": "label.fov",
    "effect.param.smoothness": "label.smoothness",
    "effect.param.lens_correction": "label.lens_correction_strength",
    "effect.param.horizon_lock": "label.horizon_lock_amount",
    "effect.param.horizon_roll": "label.horizon_lock_roll",
    "effect.param.zoom_mode": "label.zoom_mode",
    "effect.param.overview": "label.toggle_overview",
}

REQUIRED_FINAL_KEYS = {
    "app.step.choose_fcpxml",
    "app.step.choose_media",
    "app.step.generate",
    "app.action.choose_fcpxml",
    "app.action.choose_media",
    "app.action.add_media",
    "app.action.cancel",
    "app.action.generate",
    "app.action.open_final_cut",
    "app.action.remove",
    "app.action.reselect_media",
    "app.action.retry_open",
    "app.action.reveal_output",
    "app.action.save_replacement",
    "app.panel.open_fcpxml",
    "app.panel.media_roots",
    "app.panel.save_replacement",
    "app.permission.related_projects",
    "app.replace.explanation",
    "app.replace.unconfirmed",
    "app.filter.all",
    "app.filter.issues",
    "app.label.expected_project",
    "app.label.processed_videos",
    "app.label.report",
    "app.label.report_total",
    "app.label.report_updated",
    "app.label.report_timing",
    "app.label.report_skipped",
    "app.label.success_count",
    "app.placeholder.search",
    "app.status.loading_input",
    "app.target.media_unknown",
    "app.target.project_unknown",
    "app.error.route.invalid_input",
    "app.error.route.unsafe_structure",
    "app.error.route.no_targets",
    "app.installation.footer",
    "effect.action.load_project",
    "effect.action.load_project_help",
    "effect.mode.direct",
    "effect.mode.none",
    "effect.mode.pending",
    "effect.mode.reprocess",
    "effect.mode.route_d",
    "effect.error.project_too_large",
    "effect.error.commit_pending",
    *(f"app.skip.{reason}" for reason in (
        "missing_project",
        "permission_denied",
        "invalid_project",
        "incompatible_project",
        "payload_too_large",
        "ambiguous_media",
        "unsupported_structure",
        "invalid_timing",
        "blocked_geometry",
    )),
}


def load_module(path: Path, name: str):
    sys.path.insert(0, str(path.parent))
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise AssertionError(f"unable to import {path}")
    module = importlib.util.module_from_spec(spec)
    try:
        spec.loader.exec_module(module)
    finally:
        sys.path.remove(str(path.parent))
    return module


class FinalCutLocalizationContractTests(unittest.TestCase):
    def test_route_d_processor_wraps_stable_codes_and_keeps_rust_detail_diagnostic_only(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            executable = root / "route-d-processor"
            build = subprocess.run(
                [
                    "xcrun",
                    "clang",
                    "-fobjc-arc",
                    "-fblocks",
                    "-fmodules",
                    f"-fmodules-cache-path={root / 'module-cache'}",
                    "-framework",
                    "Foundation",
                    "-I",
                    str(APP_SOURCE),
                    "-I",
                    str(EFFECT_SOURCE),
                    "-I",
                    str(ROOT / "finalcut" / "include"),
                    str(EFFECT_SOURCE / "GFLocalization.m"),
                    str(APP_SOURCE / "RouteDProcessor.m"),
                    str(ROOT / "tests" / "helpers" / "finalcut_route_d_processor_main.m"),
                    "-o",
                    str(executable),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            run = subprocess.run([str(executable)], capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
            result = json.loads(run.stdout)
            self.assertEqual(
                result["inputs"],
                [{"length": 0, "path": "/Media/A.gyroflow", "status": 1}],
            )
            self.assertTrue(result["directProjectRead"])
            descriptions = [item["description"] for item in result["results"]]
            self.assertEqual(len(set(descriptions[:3])), 3)
            self.assertNotIn("raw Rust diagnostic", " ".join(descriptions))
            self.assertTrue(all(
                item["diagnostic"] == "raw Rust diagnostic must not be user-visible"
                for item in result["results"]
            ))

    def test_counts_use_catalog_plural_variations_for_english_and_russian(self):
        catalog = json.loads(CATALOG.read_text())
        for key in (
            "app.label.effect_count",
            "app.label.processed_videos",
            "app.label.success_count",
        ):
            localizations = catalog["strings"][key]["localizations"]
            self.assertEqual(
                set(localizations["en"]["variations"]["plural"]),
                {"one", "other"},
            )
            self.assertEqual(
                set(localizations["ru"]["variations"]["plural"]),
                {"one", "few", "many", "other"},
            )

    def test_load_button_has_dedicated_action_and_help(self):
        source = (EFFECT_SOURCE / "GFProjectDropView.m").read_text()
        self.assertIn('GFLocalized(@"effect.action.load_project"', source)
        self.assertIn('GFLocalized(@"effect.action.load_project_help"', source)
        tooltip_assignment = source.split("self.loadButton.toolTip =", 1)[1].split(";", 1)[0]
        self.assertNotIn('effect.inspector.help', tooltip_assignment)

    def test_chinese_horizon_roll_uses_rotation_terminology(self):
        simplified = json.loads(
            (ROOT / "common" / "locales" / "zh-CN.json").read_text()
        )
        traditional = json.loads(
            (ROOT / "common" / "locales" / "zh-TW.json").read_text()
        )

        self.assertEqual(simplified["label.horizon_lock_roll"], "水平滚转")
        self.assertEqual(simplified["hint.horizon_lock_roll"], "水平锁定滚转调整")
        self.assertEqual(traditional["label.horizon_lock_roll"], "水平滾轉")
        self.assertEqual(traditional["hint.horizon_lock_roll"], "水平鎖定滾轉調整")

    def test_effect_host_errors_never_publish_raw_english_or_bridge_errors(self):
        effect = (EFFECT_SOURCE / "GyroflowFinalCutEffect.m").read_text()
        self.assertNotRegex(effect, r'GFFxError\(@"')
        for raw_error in (
            "*error = payloadError ?:",
            "*error = archiveError ?:",
            "*outError = commandBuffer.error ?:",
            "*outError = stateError ?:",
            "*outError = GFFxError(message)",
        ):
            self.assertNotIn(raw_error, effect)
        self.assertRegex(
            effect,
            r'GFLocalized\(\s*@"effect\.error\.render_detail"',
        )

    def test_swift_production_surfaces_use_the_explicit_bundle_wrapper(self):
        localized_files = (
            "FCPXMLDocumentInput.swift",
            "ReplacementProjectStore.swift",
            "SandboxedRouteDWorkflow.swift",
            "TemplateInstaller.swift",
        )
        source = "\n".join(
            (APP_SOURCE / name).read_text() for name in localized_files
        )
        self.assertNotIn("String(localized:", source)
        self.assertIn("FinalCutStrings.text(", source)

    def test_swift_wrapper_reads_the_bundle_passed_by_the_caller(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle = root / "Fixture.bundle"
            resources = bundle / "Contents" / "Resources" / "en.lproj"
            resources.mkdir(parents=True)
            with (bundle / "Contents" / "Info.plist").open("wb") as output:
                plistlib.dump(
                    {
                        "CFBundleDevelopmentRegion": "en",
                        "CFBundleIdentifier": "test.finalcut.localization",
                        "CFBundlePackageType": "BNDL",
                    },
                    output,
                )
            (resources / "Localizable.strings").write_text(
                '"probe.key" = "fixture translation";\n', encoding="utf-8"
            )
            executable = root / "finalcut-strings"
            build = subprocess.run(
                [
                    "xcrun",
                    "swiftc",
                    "-module-cache-path",
                    str(root / "module-cache"),
                    str(APP_SOURCE / "FinalCutStrings.swift"),
                    str(ROOT / "tests" / "helpers" / "finalcut_strings_main.swift"),
                    "-o",
                    str(executable),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(build.returncode, 0, build.stdout + build.stderr)
            run = subprocess.run(
                [str(executable), str(bundle)],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
            self.assertEqual(run.stdout.strip(), "fixture translation")

    def test_inspector_has_no_embedded_project_export_path(self):
        header = (EFFECT_SOURCE / "GFProjectStore.h").read_text()
        drop_view = (EFFECT_SOURCE / "GFProjectDropView.m").read_text()
        self.assertIn("currentProjectFilename", header)
        self.assertNotIn("embeddedProjectOpenSnapshot", header + drop_view)
        self.assertNotIn("filenameStem", drop_view)
        self.assertNotIn("GFEmbeddedProjectOpener", drop_view)

    def test_locale_sources_and_generator_exist(self):
        self.assertTrue(GENERATOR.is_file(), "localization generator is missing")
        self.assertTrue(CATALOG.is_file(), "xcstrings catalog is missing")
        self.assertTrue(INFO_CATALOG.is_file(), "Info.plist catalog is missing")
        self.assertEqual(
            {path.name for path in FINAL_LOCALES.glob("*.json")},
            {f"{locale}.json" for locale in LOCALES},
        )

    def test_final_cut_only_locales_have_exact_key_and_placeholder_parity(self):
        if not (FINAL_LOCALES / "en.json").is_file():
            self.skipTest("locale sources are not implemented yet")
        locale_data = {
            locale: json.loads((FINAL_LOCALES / f"{locale}.json").read_text())
            for locale in LOCALES
        }
        expected_keys = set(locale_data["en"])
        self.assertTrue(REQUIRED_FINAL_KEYS <= expected_keys)
        self.assertFalse(expected_keys & set(SHARED_KEYS))
        placeholder = re.compile(r"%(?:\d+\$)?(?:ld|lu|[@df])")
        def placeholders(value):
            values = value.values() if isinstance(value, dict) else [value]
            return {tuple(placeholder.findall(item)) for item in values}
        for locale, values in locale_data.items():
            with self.subTest(locale=locale):
                self.assertEqual(set(values), expected_keys)
                self.assertTrue(all(
                    (isinstance(value, str) and value)
                    or (
                        isinstance(value, dict)
                        and value
                        and "other" in value
                        and all(isinstance(item, str) and item for item in value.values())
                    )
                    for value in values.values()
                ))
                self.assertEqual(
                    {key: placeholders(value) for key, value in values.items()},
                    {
                        key: placeholders(value)
                        for key, value in locale_data["en"].items()
                    },
                )
                if locale != "en":
                    for key in REQUIRED_FINAL_KEYS - {"effect.mode.route_d"}:
                        if isinstance(values[key], str):
                            self.assertNotEqual(
                                values[key],
                                locale_data["en"][key],
                                f"{locale} falls back to English for {key}",
                            )

    def test_catalog_reuses_shared_values_and_maps_only_supported_languages(self):
        if not CATALOG.is_file():
            self.skipTest("catalog is not implemented yet")
        catalog = json.loads(CATALOG.read_text())
        self.assertEqual(catalog["sourceLanguage"], "en")
        for catalog_key, common_key in SHARED_KEYS.items():
            localizations = catalog["strings"][catalog_key]["localizations"]
            self.assertEqual(set(localizations), set(XCODE_LOCALES))
            for source_locale, xcode_locale in zip(LOCALES, XCODE_LOCALES):
                common = json.loads(
                    (ROOT / "common" / "locales" / f"{source_locale}.json").read_text()
                )
                self.assertEqual(
                    localizations[xcode_locale]["stringUnit"]["value"],
                    common[common_key],
                )

    def test_info_plist_catalog_localizes_all_file_access_purpose_strings(self):
        catalog = json.loads(INFO_CATALOG.read_text())
        self.assertEqual(
            set(catalog["strings"]),
            {
                "NSDesktopFolderUsageDescription",
                "NSDocumentsFolderUsageDescription",
                "NSDownloadsFolderUsageDescription",
                "NSNetworkVolumesUsageDescription",
                "NSRemovableVolumesUsageDescription",
            },
        )
        for entry in catalog["strings"].values():
            localizations = entry["localizations"]
            self.assertEqual(set(localizations), set(XCODE_LOCALES))
            self.assertTrue(all(
                localizations[locale]["stringUnit"]["value"]
                for locale in XCODE_LOCALES
            ))

    def test_generator_is_reproducible_and_check_detects_all_contract_drift(self):
        if not GENERATOR.is_file() or not CATALOG.is_file():
            self.skipTest("generator is not implemented yet")
        first = CATALOG.read_bytes()
        first_info = INFO_CATALOG.read_bytes()
        subprocess.run([sys.executable, str(GENERATOR)], cwd=ROOT, check=True)
        self.assertEqual(CATALOG.read_bytes(), first)
        self.assertEqual(INFO_CATALOG.read_bytes(), first_info)
        subprocess.run(
            [sys.executable, str(GENERATOR), "--check"], cwd=ROOT, check=True
        )

        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory)
            shutil.copytree(ROOT / "common" / "locales", fixture / "common" / "locales")
            shutil.copytree(FINAL_LOCALES, fixture / "finalcut" / "locales")
            (fixture / "finalcut" / "Xcode" / "Shared").mkdir(parents=True)
            command = [sys.executable, str(GENERATOR), "--root", str(fixture)]
            subprocess.run(command, check=True, capture_output=True, text=True)

            def rejected(mutator):
                pristine = json.loads(
                    (fixture / "finalcut" / "locales" / "ja.json").read_text()
                )
                mutated = dict(pristine)
                mutator(mutated)
                locale_path = fixture / "finalcut" / "locales" / "ja.json"
                locale_path.write_text(json.dumps(mutated), encoding="utf-8")
                result = subprocess.run(command, capture_output=True, text=True)
                locale_path.write_text(json.dumps(pristine), encoding="utf-8")
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)

            rejected(lambda values: values.pop(next(iter(values))))
            rejected(lambda values: values.__setitem__("unexpected.key", "unexpected"))
            format_key = next(
                key for key, value in json.loads(
                    (fixture / "finalcut" / "locales" / "en.json").read_text()
                ).items() if isinstance(value, str) and "%@" in value
            )
            rejected(lambda values: values.__setitem__(format_key, values[format_key].replace("%@", "")))

            common_path = fixture / "common" / "locales" / "ru.json"
            common = json.loads(common_path.read_text())
            common[SHARED_KEYS["effect.label.status"]] += " drift"
            common_path.write_text(json.dumps(common), encoding="utf-8")
            check = subprocess.run(
                command + ["--check"], capture_output=True, text=True
            )
            self.assertNotEqual(check.returncode, 0, check.stdout + check.stderr)

    def test_runtime_user_text_uses_stable_localization_keys(self):
        swift_files = [
            "BatchProcessView.swift",
            "FinalCutAppModel.swift",
            "FCPXMLDocumentInput.swift",
            "ReplacementProjectStore.swift",
            "SandboxedRouteDWorkflow.swift",
            "TemplateInstaller.swift",
        ]
        swift = "\n".join(
            (ROOT / "finalcut" / "Xcode" / "App" / name).read_text()
            for name in swift_files
        )
        self.assertIn("FinalCutStrings.text(", swift)
        forbidden_swift = [
            r'\b(?:Text|Button|Label|DisclosureGroup|LabeledContent)\("[A-Za-z]',
            r'\.accessibility(?:Label|Hint)\(\s*"[A-Za-z]',
            r'panel\.message\s*=\s*"[A-Za-z]',
            r'return\s+"(?:Choose|The |Select|Preview|Bundled|Template |Refusing)',
        ]
        for pattern in forbidden_swift:
            self.assertIsNone(re.search(pattern, swift), pattern)

        objc_files = [
            "GFProjectDropView.m",
            "GFProjectStore.m",
            "GFSourceResolver.m",
            "GyroflowFinalCutEffect.m",
        ]
        objc = "\n".join(
            (ROOT / "finalcut" / "Xcode" / "Effect" / name).read_text()
            for name in objc_files
        )
        self.assertIn("GFLocalized(@\"effect.", objc)
        for literal in (
            'startParameterSubGroup:@"Gyroflow Project"',
            'startParameterSubGroup:@"Stabilization"',
            'addCustomParameterWithName:@"Project Import"',
            'menuEntries:@[@"No zoom", @"Dynamic zoom", @"Static zoom"]',
            'stringWithFormat:@"Could not locate a project:',
            'stringWithFormat:@"No sibling %@ found;',
        ):
            self.assertNotIn(literal, objc)

        used_keys = set(
            re.findall(r'FinalCutStrings\.(?:text|format)\(\s*"([^"]+)"', swift)
        )
        used_keys.update(
            re.findall(r'localized:\s*"([^"]+)"', swift)
        )
        used_keys.update(re.findall(r'GFLocalized\(@"([^"]+)"', objc))
        catalog_keys = set(json.loads(CATALOG.read_text())["strings"])
        self.assertTrue(used_keys <= catalog_keys, sorted(used_keys - catalog_keys))

    def test_package_verifier_requires_every_localization_in_both_bundles(self):
        verifier = load_module(
            ROOT / "scripts" / "verify_finalcut_package.py",
            "verify_finalcut_package_localizations",
        )
        self.assertTrue(
            hasattr(verifier, "verify_localizations"),
            "package verifier does not validate localization resources",
        )
        with tempfile.TemporaryDirectory() as directory:
            app_resources = Path(directory) / "AppResources"
            effect_resources = Path(directory) / "EffectResources"
            for root in (app_resources, effect_resources):
                for locale in XCODE_LOCALES:
                    strings = root / f"{locale}.lproj" / "Localizable.strings"
                    strings.parent.mkdir(parents=True, exist_ok=True)
                    strings.write_text("", encoding="utf-8")
            for locale in XCODE_LOCALES:
                (app_resources / f"{locale}.lproj" / "InfoPlist.strings").write_text(
                    "", encoding="utf-8"
                )
            verifier.verify_localizations(app_resources, effect_resources)
            (effect_resources / "ko.lproj" / "Localizable.strings").unlink()
            with self.assertRaisesRegex(ValueError, "ko"):
                verifier.verify_localizations(app_resources, effect_resources)


if __name__ == "__main__":
    unittest.main()
