#!/usr/bin/env python3
"""Enforce the NEO crate dependency DAG.

The architecture (docs/02) is only real if a machine checks it. This script reads
`cargo metadata --no-deps` and fails if a crate depends on an internal crate that the
allowed-edge table forbids (for example `neo-render -> neo-store`, which would break the
"renderer does not know the domain model" rule).

Usage:
    python scripts/check-deps.py            # check workspace
    python scripts/check-deps.py --json     # machine readable violations
"""

from __future__ import annotations

import json
import subprocess
import sys

# Maps crate -> set of internal crates it MAY depend on.
# Keep this table in sync with docs/02 §2.2. The set is the current Cargo.toml
# edge list, not a spare quota: a new internal edge fails until both are updated.
RULES: dict[str, set[str]] = {
    "neo-core": set(),
    "neo-osm": {"neo-core"},
    "neo-store": {"neo-core", "neo-osm"},
    "neo-io": {"neo-core", "neo-osm", "neo-store"},
    "neo-geo": {"neo-core", "neo-osm", "neo-store"},
    "neo-style": {"neo-core"},
    "neo-scene": {"neo-core", "neo-osm", "neo-store", "neo-geo", "neo-style"},
    "neo-render": {"neo-core", "neo-geo", "neo-scene"},
    "neo-editor": {"neo-core", "neo-osm", "neo-store", "neo-geo"},
    "neo-tools": {"neo-core", "neo-osm", "neo-store", "neo-geo", "neo-editor"},
    "neo-api": {"neo-core"},
    "neo-persist": set(),
    "neo-validate": {"neo-core", "neo-osm", "neo-store"},
    "neo-query": {"neo-core", "neo-osm", "neo-store", "neo-geo"},
    "neo-plugin": {"neo-core", "neo-query", "neo-editor"},
    "neo-i18n": set(),
    "neo-ipc": set(),
    "neo-ui": {"neo-core", "neo-osm", "neo-store", "neo-style", "neo-i18n"},
    "neo-desktop": {
        "neo-core",
        "neo-osm",
        "neo-store",
        "neo-io",
        "neo-geo",
        "neo-scene",
        "neo-render",
        "neo-editor",
        "neo-persist",
        "neo-tools",
        "neo-validate",
        "neo-api",
        "neo-plugin",
        "neo-ui",
        "neo-style",
        "neo-i18n",
        "neo-ipc",
    },
    "neo-prototype": {
        "neo-core",
        "neo-osm",
        "neo-store",
        "neo-io",
        "neo-geo",
        "neo-scene",
        "neo-render",
    },
    "neo-cli": {
        "neo-core",
        "neo-store",
        "neo-io",
        "neo-geo",
        "neo-scene",
        "neo-style",
        "neo-plugin",
    },
    "neo-imagery": {"neo-ipc"},
    "neo-sync": {"neo-api", "neo-core", "neo-io", "neo-ipc"},
    "neo": set(),
}

# depth(c) = 0 if c has no internal deps, else 1 + max(depth(dep)).
# Docs/02 §2.3. A new edge that changes a number must update this table in the same change.
DEPTH: dict[str, int] = {
    "neo-core": 0,
    "neo-i18n": 0,
    "neo-persist": 0,
    "neo-ipc": 0,
    "neo": 0,
    "neo-osm": 1,
    "neo-api": 1,
    "neo-style": 1,
    "neo-imagery": 1,
    "neo-store": 2,
    "neo-geo": 3,
    "neo-io": 3,
    "neo-validate": 3,
    "neo-ui": 3,
    "neo-scene": 4,
    "neo-editor": 4,
    "neo-query": 4,
    "neo-sync": 4,
    "neo-render": 5,
    "neo-tools": 5,
    "neo-plugin": 5,
    "neo-prototype": 6,
    "neo-cli": 6,
    "neo-desktop": 6,
}


def cargo_metadata() -> dict:
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    return json.loads(out.stdout)


def main() -> int:
    meta = cargo_metadata()
    internal = {pkg["name"] for pkg in meta["packages"] if pkg["name"].startswith("neo-")}

    violations: list[dict[str, str]] = []
    unknown: list[str] = []
    edges: dict[str, set[str]] = {name: set() for name in internal}
    for pkg in meta["packages"]:
        name = pkg["name"]
        if name not in RULES:
            unknown.append(name)
            continue
        # R6: 内部 crate 必须显式 `publish = false`（cargo metadata 里为空数组）。
        if pkg.get("publish") != []:
            violations.append({"rule": "R6", "package": name, "detail": "is publishable"})
        allowed = RULES[name]
        for dep in pkg["dependencies"]:
            dep_name = dep["name"]
            if dep_name not in internal:
                continue
            if dep["kind"] is not None and dep["kind"] != "normal":
                continue
            edges[name].add(dep_name)
            if dep_name not in allowed:
                violations.append(
                    {"rule": "R1", "package": name, "forbidden_dependency": dep_name}
                )

    # R2: 白名单/实际依赖图不得成环。
    color: dict[str, int] = {name: 0 for name in internal}
    stack: list[str] = []

    def visit(node: str) -> None:
        color[node] = 1
        stack.append(node)
        for dep in sorted(edges.get(node, ())):
            if color.get(dep) == 1:
                cycle = " -> ".join(stack[stack.index(dep):] + [dep])
                violations.append({"rule": "R2", "package": node, "detail": f"cycle {cycle}"})
            elif color.get(dep, 0) == 0:
                visit(dep)
        stack.pop()
        color[node] = 2

    for name in sorted(internal):
        if color.get(name, 0) == 0:
            visit(name)

    # R3: depth follows the declared edges and must match docs/02 §2.3.
    memo: dict[str, int] = {}
    visiting: set[str] = set()

    def depth_of(name: str) -> int:
        if name in memo:
            return memo[name]
        if name in visiting:
            return 0
        visiting.add(name)
        deps = edges.get(name, ())
        got = 0 if not deps else 1 + max(depth_of(dep) for dep in deps)
        visiting.remove(name)
        memo[name] = got
        return got

    if not any(item["rule"] == "R2" for item in violations):
        for name, expected in DEPTH.items():
            if name not in edges:
                continue
            got = depth_of(name)
            if got != expected:
                violations.append(
                    {
                        "rule": "R3",
                        "package": name,
                        "detail": f"depth {got}, table says {expected}",
                    }
                )

    if "--json" in sys.argv[1:]:
        print(json.dumps({"violations": violations, "unknown": unknown}, indent=2))
    else:
        if violations:
            print("dependency rule violations:")
            for v in violations:
                print(
                    f"  {v.get('rule', 'R1')} {v['package']} -> "
                    f"{v.get('forbidden_dependency', '')} {v.get('detail', '')}".rstrip()
                )
        if unknown:
            print("warning: crates missing from the rule table:", ", ".join(sorted(unknown)))
        if not violations:
            print(f"OK: {len(meta['packages'])} packages, DAG + depth + publish rules hold")

    return 2 if violations else 0


if __name__ == "__main__":
    raise SystemExit(main())
