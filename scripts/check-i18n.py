"""校验 i18n：代码里 t()/tf() 用到的 key 都存在，语言表之间 key 集合一致。

- `zh-CN.json` 是基准表（也是 `zh` 的回退表），必须覆盖所有代码 key；
- `en-US.json` 必须与基准表 key 集合完全一致；
- `zh-TW.json` 是繁體覆盖表：允许缺 key（运行时回退到 `zh`/简体），但不允许出现
  基准表里没有的 key；
- 任何语言表里的 key 都必须被代码引用（避免死 key）。

用法：python scripts/check-i18n.py
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

BASE_LOCALE = "zh-CN"
# 完整对照语言（必须与基准表一致）与允许回退的覆盖语言。
FULL_LOCALES = {"zh-CN": "zh-CN.json", "en-US": "en-US.json"}
OVERLAY_LOCALES = {"zh-TW": "zh-TW.json"}


def source_files() -> list[pathlib.Path]:
    files = list(SOURCE_FILES)
    for root in SOURCE_ROOTS:
        files.extend(root.rglob("*.rs"))
    return files


# 这些前缀下的字符串字面量会交给 t()，即使没有写成 t("...")。
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
    "filter.",
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
    for locale, file_name in {**FULL_LOCALES, **OVERLAY_LOCALES}.items():
        path = I18N / file_name
        if not path.exists():
            raise SystemExit(f"缺少语言表：{path}")
        table = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(table, dict) or not table:
            raise SystemExit(f"语言表为空或格式错误：{path}")
        tables[locale] = table
    return tables


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
    quoted = quoted_keys()

    ok = True
    problems: list[tuple[str, list[str]]] = []

    for locale, file_name in FULL_LOCALES.items():
        missing = sorted(k for k in keys if k not in tables[locale])
        problems.append((f"缺少 {locale} key（{file_name}）", missing))

    base_keys = set(base)
    for locale, file_name in OVERLAY_LOCALES.items():
        extra = sorted(set(tables[locale]) - base_keys)
        problems.append((f"{locale} 有基准表之外的 key（{file_name}）", extra))

    en = tables["en-US"]
    problems.append(("仅 zh-CN 有（en-US 缺）", sorted(base_keys - set(en))))
    problems.append(("仅 en-US 有（zh-CN 缺）", sorted(set(en) - base_keys)))

    all_keys = set()
    for table in tables.values():
        all_keys |= set(table)
    problems.append(("JSON 中未被代码引用", sorted(k for k in all_keys if k not in quoted)))

    for label, items in problems:
        if items:
            ok = False
            print(f"{label}: {items}")

    for locale in OVERLAY_LOCALES:
        covered = len(set(tables[locale]) & base_keys)
        total = len(base_keys)
        percent = 100.0 * covered / total if total else 100.0
        print(f"{locale} 覆盖 {covered}/{total}（{percent:.1f}%，缺省回退 zh）")

    if ok:
        parts = "、".join(f"{locale} {len(table)}" for locale, table in tables.items())
        print(f"i18n OK：{len(keys)} 个代码 key；{parts}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
