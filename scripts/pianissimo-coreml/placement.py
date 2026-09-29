#!/usr/bin/env python3
"""Report which compute device Core ML plans to run each op of an .mlpackage on.

Uses ``MLComputePlan`` (macOS 14.4+, coremltools >= 8). ``const`` and ``constexpr_*`` (weight decompression) ops are skipped.
"""

from __future__ import annotations

import argparse
import collections
import json
import tempfile
from pathlib import Path

import coremltools as ct
from coremltools.models.compute_plan import MLComputePlan


def device_name(usage) -> str:
    if usage is None:
        return "none"
    name = type(usage.preferred_compute_device).__name__
    for key, label in (("NeuralEngine", "ANE"), ("GPU", "GPU"), ("CPU", "CPU")):
        if key in name:
            return label
    return name


def placement(path: Path, units: ct.ComputeUnit) -> dict:
    compiled = str(path)
    if path.suffix != ".mlmodelc":
        # MLComputePlan only accepts compiled models.
        tmp = tempfile.TemporaryDirectory(prefix="placement-")
        compiled = ct.utils.compile_model(str(path), destination_path=str(Path(tmp.name) / "m.mlmodelc"))
    plan = MLComputePlan.load_from_path(compiled, compute_units=units)
    program = plan.model_structure.program
    if program is None:
        raise SystemExit(f"{path} is not an ML Program")
    counts: collections.Counter[str] = collections.Counter()
    by_op: dict[str, collections.Counter[str]] = collections.defaultdict(collections.Counter)
    for function in program.functions.values():
        stack = [function.block]
        while stack:
            block = stack.pop()
            for op in block.operations:
                for inner in op.blocks:
                    stack.append(inner)
                if op.operator_name == "const" or "constexpr" in op.operator_name:
                    continue
                device = device_name(plan.get_compute_device_usage_for_mlprogram_operation(op))
                counts[device] += 1
                by_op[op.operator_name][device] += 1
    total = sum(counts.values())
    return {
        "model": str(path),
        "compute_units": str(units),
        "ops": total,
        "counts": dict(counts),
        "percent": {k: round(100 * v / total, 1) for k, v in counts.items()} if total else {},
        "non_ane_ops": {
            op: dict(c) for op, c in sorted(by_op.items()) if any(d != "ANE" for d in c)
        },
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("packages", nargs="+", type=Path)
    parser.add_argument("--compute-units", choices=("CPU_AND_NE", "ALL"), default="CPU_AND_NE")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    units = getattr(ct.ComputeUnit, args.compute_units)
    for package in args.packages:
        report = placement(package.resolve(), units)
        if args.json:
            print(json.dumps(report, indent=2))
        else:
            print(f"{package}: {report['ops']} ops {report['counts']} {report['percent']}")


if __name__ == "__main__":
    main()
