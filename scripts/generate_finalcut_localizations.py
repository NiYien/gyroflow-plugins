#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Dict, Union


LOCALE_MAP = {
    "en": "en",
    "zh-CN": "zh-Hans",
    "zh-TW": "zh-Hant",
    "ja": "ja",
    "ko": "ko",
    "ru": "ru",
}
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
FORMAT_PLACEHOLDER = re.compile(r"%(?:\d+\$)?(?:ld|lu|[@df])")
Translation = Union[str, Dict[str, str]]
PLURAL_CATEGORIES = {"zero", "one", "two", "few", "many", "other"}
PLURAL_KEYS = {
    "app.label.effect_count",
    "app.label.processed_videos",
    "app.label.success_count",
}
INFO_PLIST_KEYS = {
    "NSDesktopFolderUsageDescription": "app.permission.related_projects",
    "NSDocumentsFolderUsageDescription": "app.permission.related_projects",
    "NSDownloadsFolderUsageDescription": "app.permission.related_projects",
    "NSNetworkVolumesUsageDescription": "app.permission.related_projects",
    "NSRemovableVolumesUsageDescription": "app.permission.related_projects",
}


def load_object(path: Path) -> dict[str, str]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict) or not all(
        isinstance(key, str) and isinstance(item, str) and item
        for key, item in value.items()
    ):
        raise ValueError(f"{path} must contain a non-empty string dictionary")
    return value


def load_final_object(path: Path) -> dict[str, Translation]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path} must contain a translation dictionary")
    for key, item in value.items():
        if not isinstance(key, str):
            raise ValueError(f"{path} contains a non-string key")
        if isinstance(item, str) and item:
            continue
        if (
            key in PLURAL_KEYS
            and isinstance(item, dict)
            and item
            and set(item) <= PLURAL_CATEGORIES
            and "other" in item
            and all(isinstance(text, str) and text for text in item.values())
        ):
            continue
        raise ValueError(f"{path} contains an invalid translation for {key}")
    return value


def validate_locale_files(directory: Path) -> None:
    actual = {path.stem for path in directory.glob("*.json")}
    expected = set(LOCALE_MAP)
    if actual != expected:
        raise ValueError(
            f"locale set mismatch: missing={sorted(expected - actual)} "
            f"extra={sorted(actual - expected)}"
        )


def load_translations(root: Path) -> dict[str, dict[str, Translation]]:
    final_directory = root / "finalcut" / "locales"
    common_directory = root / "common" / "locales"
    validate_locale_files(final_directory)
    final = {
        locale: load_final_object(final_directory / f"{locale}.json")
        for locale in LOCALE_MAP
    }
    expected_keys = set(final["en"])
    shared_overlap = expected_keys & set(SHARED_KEYS)
    if shared_overlap:
        raise ValueError(
            f"Final Cut-only locale source duplicates shared keys: {sorted(shared_overlap)}"
        )
    def placeholders(value: Translation) -> set[tuple[str, ...]]:
        values = value.values() if isinstance(value, dict) else [value]
        return {tuple(FORMAT_PLACEHOLDER.findall(item)) for item in values}

    expected_placeholders = {
        key: placeholders(value) for key, value in final["en"].items()
    }
    for locale, values in final.items():
        keys = set(values)
        if keys != expected_keys:
            raise ValueError(
                f"{locale} key mismatch: missing={sorted(expected_keys - keys)} "
                f"extra={sorted(keys - expected_keys)}"
            )
        actual_placeholders = {
            key: placeholders(value) for key, value in values.items()
        }
        if actual_placeholders != expected_placeholders:
            mismatched = sorted(
                key
                for key in expected_keys
                if actual_placeholders[key] != expected_placeholders[key]
            )
            raise ValueError(f"{locale} placeholder mismatch: {mismatched}")

    merged: dict[str, dict[str, Translation]] = {}
    for locale in LOCALE_MAP:
        common = load_object(common_directory / f"{locale}.json")
        missing_shared = set(SHARED_KEYS.values()) - set(common)
        if missing_shared:
            raise ValueError(f"{locale} common locale missing shared keys: {sorted(missing_shared)}")
        merged[locale] = {
            **final[locale],
            **{
                catalog_key: common[common_key]
                for catalog_key, common_key in SHARED_KEYS.items()
            },
        }
    return merged


def build_catalog(root: Path) -> dict[str, object]:
    translations = load_translations(root)
    all_keys = sorted(translations["en"])
    strings = {}
    for key in all_keys:
        localizations = {}
        for source_locale, xcode_locale in LOCALE_MAP.items():
            value = translations[source_locale][key]
            if isinstance(value, dict):
                localizations[xcode_locale] = {
                    "variations": {
                        "plural": {
                            category: {
                                "stringUnit": {
                                    "state": "translated",
                                    "value": text,
                                }
                            }
                            for category, text in sorted(value.items())
                        }
                    }
                }
            else:
                localizations[xcode_locale] = {
                    "stringUnit": {
                        "state": "translated",
                        "value": value,
                    }
                }
        strings[key] = {
            "extractionState": "manual",
            "localizations": localizations,
        }
    return {
        "sourceLanguage": "en",
        "strings": strings,
        "version": "1.0",
    }


def encoded_catalog(root: Path) -> bytes:
    return (
        json.dumps(
            build_catalog(root),
            ensure_ascii=False,
            indent=2,
            sort_keys=True,
        )
        + "\n"
    ).encode("utf-8")


def build_info_plist_catalog(root: Path) -> dict[str, object]:
    translations = load_translations(root)
    strings = {}
    for plist_key, translation_key in sorted(INFO_PLIST_KEYS.items()):
        strings[plist_key] = {
            "extractionState": "manual",
            "localizations": {
                xcode_locale: {
                    "stringUnit": {
                        "state": "translated",
                        "value": translations[source_locale][translation_key],
                    }
                }
                for source_locale, xcode_locale in LOCALE_MAP.items()
            },
        }
    return {
        "sourceLanguage": "en",
        "strings": strings,
        "version": "1.0",
    }


def encoded_info_plist_catalog(root: Path) -> bytes:
    return (
        json.dumps(
            build_info_plist_catalog(root),
            ensure_ascii=False,
            indent=2,
            sort_keys=True,
        )
        + "\n"
    ).encode("utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    arguments = parser.parse_args()
    root = arguments.root.resolve()
    outputs = {
        root / "finalcut" / "Xcode" / "Shared" / "Localizable.xcstrings":
            encoded_catalog,
        root / "finalcut" / "Xcode" / "App" / "InfoPlist.xcstrings":
            encoded_info_plist_catalog,
    }
    try:
        for output, encoder in outputs.items():
            generated = encoder(root)
            if arguments.check:
                if not output.is_file() or output.read_bytes() != generated:
                    raise ValueError(f"{output} is not reproducible; run the generator")
            else:
                output.parent.mkdir(parents=True, exist_ok=True)
                output.write_bytes(generated)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"Final Cut localization generation failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error


if __name__ == "__main__":
    main()
