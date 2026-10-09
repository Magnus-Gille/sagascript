"""Verify archived analyzer arithmetic and explicit scope, without running inference."""
import json
import math
from pathlib import Path
root = Path(__file__).resolve().parent
measurements = json.loads((root / "measured-baseline.json").read_text())
for name in ("rust-core", "rust-cli-mixed-attempts", "swift"):
    receipt = json.loads((root / "receipts" / f"{name}-coverage.json").read_text())
    assert receipt["source_revision"] == measurements["measurement_provenance"]["source_revision"]
    assert receipt["files"] and len({f["path"] for f in receipt["files"]}) == len(receipt["files"])
    assert all(not Path(f["path"]).is_absolute() for f in receipt["files"])
    for metric in ("lines", "functions", "regions"):
        total = receipt["original_totals"][metric]
        assert sum(f["summary"][metric]["count"] for f in receipt["files"]) == total["count"]
        assert sum(f["summary"][metric]["covered"] for f in receipt["files"]) == total["covered"]
        assert 0 <= total["covered"] <= total["count"]
        assert math.isclose(100 * total["covered"] / total["count"], total["percent"])
    if name == "swift":
        source = [f for f in receipt["files"] if "/Sources/EngineHostCore/" in f["path"]]
        for metric, total in receipt["source_only_totals"].items():
            assert sum(f["summary"][metric]["count"] for f in source) == total["count"]
            assert sum(f["summary"][metric]["covered"] for f in source) == total["covered"]
        assert receipt["source_only_totals"] == measurements["coverage"]["swift"]["source_only_totals"]
    elif name == "rust-core":
        for metric in ("lines", "functions", "regions"):
            assert math.isclose(receipt["original_totals"][metric]["percent"], measurements["coverage"]["cargo_llvm_cov"]["core"][metric])
    else:
        assert receipt["status"] == "mixed-attempt-profiles-not-baseline"
assert measurements["coverage"]["frontend"]["status"] == "not_measured"
assert measurements["coverage"]["cargo_llvm_cov"]["cli"]["status"] == "mixed-attempt-profiles-not-baseline"
functions = json.loads((root / "receipts/rust-functions.json").read_text())
f = next(x for x in functions if x["name"] == "transcribe_file")
assert f["cyclomatic"]["max"] == measurements["static_metrics"]["rust_complexity"]["top_function"]["cyclomatic"]
assert f["cognitive"]["max"] == measurements["static_metrics"]["rust_complexity"]["top_function"]["cognitive"]
print("PASS: coverage numerator/denominator arithmetic, scope exclusions, source revision and complexity receipt")
