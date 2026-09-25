#!/usr/bin/env python3
"""Check the pinned package inventory in a generated Pianissimo runtime."""

import json
import re
import sys
from importlib import metadata
from pathlib import Path


def canonical(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: verify-pianissimo-runtime.py RUNTIME_DIR", file=sys.stderr)
        return 2
    root = Path(sys.argv[1]).resolve(strict=True)
    repo = Path(__file__).resolve().parent.parent
    expected = {
        canonical(item["name"]): item
        for item in json.loads((repo / "scripts/pianissimo-runtime-inventory.json").read_text())
    }
    actual = {}
    errors = []
    site = root / "site-packages"
    for distribution in metadata.distributions(path=[str(site)]):
        name = canonical(distribution.metadata["Name"])
        if name in actual:
            errors.append(f"duplicate package: {name}")
        actual[name] = distribution.version
        for declared in distribution.metadata.get_all("License-File", []):
            dist_info = Path(distribution._path)  # importlib's installed distribution directory
            if not (dist_info / declared).is_file() and not (dist_info / "licenses" / declared).is_file():
                errors.append(f"missing declared license file: {name}: {declared}")
    for name, item in expected.items():
        if actual.get(name) != item["version"]:
            errors.append(f"package mismatch: {name} expected {item['version']}, found {actual.get(name)}")
    for name in actual.keys() - expected.keys():
        errors.append(f"unexpected package: {name}")
    if not (root / "CPYTHON_LICENSE").is_file():
        errors.append("CPython license missing")
    if not (root / "python/bin/python3.12").is_file():
        errors.append("Python executable missing")
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"Verified Pianissimo runtime: {len(actual)} pinned Python packages and license declarations")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
