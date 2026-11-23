#!/usr/bin/env python3
"""Guard the "only the editor/IO path may mutate the dataset" rule.

docs/01 INV-1 says: `&mut DataSet` is owned by a single writer, and all structural
mutation goes through the editor commands (plus the import path). This script is a
cheap textual guard used in CI: it scans Rust sources for calls to the mutating
`DataSet` methods and reports files that are not in the allow-list.

It is intentionally conservative (it can be extended, but a false positive should be
resolved by moving code, not by silencing the check silently).

Usage:
    python scripts/check-write-api.py
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

# Methods that structurally mutate the dataset.
MUTATORS = re.compile(
    r"\.(push_node|push_way|push_relation|set_node_position|set_way_nodes|set_change_state|remove_node|remove_way)\("
)

# Directories/files allowed to call mutators.
ALLOWED_PREFIXES = (
    "crates/neo-store/",      # implementation itself (+ its unit tests)
    "crates/neo-io/",         # import pipeline is an explicit bulk-write path
    "crates/neo-editor/",     # command implementations (the only interactive writer)
    "apps/",                  # application wiring + benchmarks (explicit, reviewed)
)

SKIP_DIRS = {"target", ".research", ".git"}
TEST_MODULE = re.compile(r"^\s*(#\[cfg\(test\)\]|mod tests\b)")


def main() -> int:
    violations: list[str] = []
    scanned = 0
    for path in sorted(ROOT.rglob("*.rs")):
        rel = path.relative_to(ROOT).as_posix()
        if any(part in SKIP_DIRS for part in path.parts):
            continue
        if rel.startswith(ALLOWED_PREFIXES):
            continue
        scanned += 1
        text = path.read_text(encoding="utf-8", errors="replace")
        in_tests = False
        for lineno, line in enumerate(text.splitlines(), start=1):
            if TEST_MODULE.match(line):
                # 约定：测试模块放在文件末尾；其后的代码允许构造测试数据集。
                in_tests = True
            if in_tests or line.lstrip().startswith("//"):
                continue
            if MUTATORS.search(line):
                violations.append(f"{rel}:{lineno}: {line.strip()}")

    if violations:
        print("write-API violations (dataset mutated outside editor/io path):")
        for v in violations:
            print("  " + v)
    else:
        print(f"OK: scanned {scanned} files, no out-of-band dataset mutations")
    return 1 if violations else 0


if __name__ == "__main__":
    raise SystemExit(main())
