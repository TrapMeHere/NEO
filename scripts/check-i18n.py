"""Validate i18n tables.

`i18n/en-US.json` is the source/base language: every key used by the code must exist
there, and every key in it must be referenced by code. Every other `i18n/*.json` file is
a translation: it may temporarily miss keys (the runtime falls back to English, or to
Simplified Chinese for Traditional Chinese), but it must never contain keys that are
absent from the English base. Placeholders (`{name}`) must match the base for every
translated key.

Usage: python scripts/check-i18n.py
"""

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SOURCE_ROOTS = [
    ROOT / "crates" / "neo-ui" / "src" / "panels",
    ROOT / "apps" / "neo-desktop" / "src" / "app",
    ROOT / "crates" / "neo-tools" / "src",
]
SOURCE_FILES = [ROOT / "apps" / "neo-desktop" / "src" / "main.rs"]
I18N = ROOT / "i18n"

BASE_LOCALE = "en-US"
BASE_FILE = "en-US.json"
PLACEHOLDER = re.compile(r"\{[A-Za-z_][A-Za-z0-9_]*\}")


def source_files() -> list[pathlib.Path]:
    files = list(SOURCE_FILES)
    for root in SOURCE_ROOTS:
        files.extend(root.rglob("*.rs"))
    return files


# String literals under these prefixes are passed to t(), even without a literal t("...").
DYNAMIC_PREFIXES = (
    "app.",
    "menu.",
    "settings.",
    "validate.",
    "shortcut.",
    "export.",
    "status.",
    "sync.",
    "download.",
    "imagery.",
    "preset.",
    "plugins.",
    "notes.",
    "toolbar.",
    "search.",
    "history.",
    "tags.",
    "members.",
    "changeset.",
    "ecosystem.",
    "tool.",
    "panel.",
    "help.",
    "layer.",
    "layer_info.",
    "selection.",
)


def quoted_keys() -> set[str]:
    found = set()
    roots = [ROOT / "crates", ROOT / "apps"]
    for root in roots:
        for path in root.rglob("*.rs"):
            if "target" in path.parts:
                continue
            text = path.read_text(encoding="utf-8")
            found |= set(re.findall(r'"([a-z][a-z0-9_.]*)"', text))
    return found


def load_locales() -> dict[str, dict[str, str]]:
    tables: dict[str, dict[str, str]] = {}
    for path in sorted(I18N.glob("*.json")):
        locale = path.stem
        table = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(table, dict) or not table:
            raise SystemExit(f"locale file is empty or invalid: {path}")
        tables[locale] = table
    if BASE_LOCALE not in tables:
        raise SystemExit(f"missing base locale: {I18N / BASE_FILE}")
    return tables


def placeholders(text: str) -> set[str]:
    return set(PLACEHOLDER.findall(text))


def main() -> int:
    literal = set()
    dynamic = set()
    for path in source_files():
        text = path.read_text(encoding="utf-8")
        literal |= set(re.findall(r'\bt(?:f)?\(\s*"([a-z0-9_.]+)"', text))
        for key in re.findall(r'"([a-z][a-z0-9_.]*)"', text):
            if key.startswith(DYNAMIC_PREFIXES):
                dynamic.add(key)
    keys = literal | dynamic

    tables = load_locales()
    base = tables[BASE_LOCALE]
    base_keys = set(base)
    overlays = {
        locale: table for locale, table in tables.items() if locale != BASE_LOCALE
    }
    quoted = quoted_keys()

    ok = True
    problems: list[tuple[str, list[str]]] = []

    missing_base = sorted(k for k in keys if k not in base)
    problems.append((f"missing {BASE_LOCALE} keys", missing_base))

    for locale in overlays:
        extra = sorted(set(overlays[locale]) - base_keys)
        problems.append((f"{locale} has keys absent from {BASE_FILE}", extra))

    problems.append(
        ("keys in JSON but not referenced by code", sorted(k for k in base_keys if k not in quoted))
    )

    for locale, table in overlays.items():
        mismatched = [
            key
            for key, value in table.items()
            if key in base and placeholders(base[key]) != placeholders(value)
        ]
        problems.append((f"{locale} placeholder mismatch vs {BASE_LOCALE}", sorted(mismatched)))

    for label, items in problems:
        if items:
            ok = False
            print(f"{label}: {items}")

    for locale, table in overlays.items():
        covered = len(set(table) & base_keys)
        total = len(base_keys)
        percent = 100.0 * covered / total if total else 100.0
        print(f"{locale} coverage: {covered}/{total} ({percent:.1f}%; missing keys fall back)")

    if ok:
        parts = "、".join(f"{locale} {len(table)}" for locale, table in tables.items())
        print(f"i18n OK: {len(keys)} code keys; {parts}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
