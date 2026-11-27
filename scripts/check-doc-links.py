#!/usr/bin/env python3
"""Check relative Markdown links inside the documentation (bilingual) and README files.

Fails on links that point to missing files (anchors are not resolved; only the target
file is checked). External http(s) links are ignored.
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
SKIP_PREFIXES = ("http://", "https://", "mailto:", "#")


def main() -> int:
    missing: list[str] = []
    checked = 0
    files = sorted(
        list(ROOT.glob("docs/**/*.md"))
        + list(ROOT.glob("docs-zh/**/*.md"))
        + [ROOT / "README.md"]
    )
    for path in files:
        text = path.read_text(encoding="utf-8")
        for match in LINK.finditer(text):
            target = match.group(1)
            if target.startswith(SKIP_PREFIXES):
                continue
            target = target.split("#", 1)[0]
            if not target:
                continue
            # `.research/` is in .gitignore (local evidence notes), so it is not a repo gate target.
            normalized = target.replace("\\", "/")
            if normalized.startswith("../.research/") or "/.research/" in normalized:
                continue
            checked += 1
            resolved = (path.parent / target).resolve()
            if not resolved.exists():
                missing.append(f"{path.relative_to(ROOT)} -> {target}")
    if missing:
        print("missing link targets:")
        for m in missing:
            print("  " + m)
        return 1
    print(f"OK: {checked} relative links resolved, 0 missing")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
