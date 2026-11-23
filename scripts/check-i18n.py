"""校验 i18n：代码里 t()/tf() 用到的 key 都存在，且中英 key 集合一致。

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

    zh = json.loads((I18N / "zh-CN.json").read_text(encoding="utf-8"))
    en = json.loads((I18N / "en-US.json").read_text(encoding="utf-8"))
    quoted = quoted_keys()

    missing_zh = sorted(k for k in keys if k not in zh)
    missing_en = sorted(k for k in keys if k not in en)
    only_zh = sorted(set(zh) - set(en))
    only_en = sorted(set(en) - set(zh))
    unused = sorted(k for k in set(zh) | set(en) if k not in quoted)

    ok = True
    for label, items in [
        ("缺少 zh-CN key", missing_zh),
        ("缺少 en-US key", missing_en),
        ("仅 zh-CN 有", only_zh),
        ("仅 en-US 有", only_en),
        ("JSON 中未被代码引用", unused),
    ]:
        if items:
            ok = False
            print(f"{label}: {items}")

    if ok:
        print(f"i18n OK：{len(keys)} 个代码 key，zh/en 各 {len(zh)} 条")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
