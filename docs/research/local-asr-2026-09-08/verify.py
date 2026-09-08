#!/usr/bin/env python3
"""Offline checks for this dated research snapshot; no inference or network I/O.

Registry values are read at the recorded Git revision, so later product changes do
not silently rewrite a historical baseline. Metadata consistency is not publisher
claim validation, a local artifact hash check, or an accuracy benchmark.
"""

import json
import math
from pathlib import Path
import re
import subprocess


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read(name):
    return json.loads((HERE / name).read_text())


def digest(value, length):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{%d}" % length, value),
            f"Invalid {length}-character digest: {value!r}")


def artifact(row):
    digest(row["sha256"], 64)
    require(type(row["bytes"]) is int and row["bytes"] > 0, "Invalid artifact size")


def unique(rows, field):
    require(len({row[field] for row in rows}) == len(rows), f"Duplicate {field}")


def main():
    publisher = read("publisher-metadata.json")["models"]
    unique(publisher, "repo")
    by_repo = {row["repo"]: row for row in publisher}
    for model in publisher:
        digest(model["revision"], 40)
        unique(model["files"], "path")
        for file in model["files"]:
            artifact(file)
        tensors = model.get("serialized_tensor_metadata")
        if tensors:
            require(sum(tensors["parameters"].values()) == tensors["total"],
                    f"Tensor subtotal mismatch: {model['repo']}")

    baseline = read("current-baselines.json")
    digest(baseline["source_revision"], 40)
    source = subprocess.check_output(
        ["git", "show", f"{baseline['source_revision']}:{baseline['source_path']}"],
        cwd=ROOT, text=True,
    )
    url_section = source[source.index("pub fn download_url"):source.index("pub fn download_integrity")]
    urls = dict(re.findall(r'WhisperModel::(\w+) => "(https://[^\"]+)"', url_section))
    integrity = {
        name: (sha, int(size.replace("_", "")))
        for name, sha, size in re.findall(
            r'WhisperModel::(\w+) => DownloadIntegrity \{ sha256: "([a-f0-9]+)", size: ([\d_]+) \}', source
        )
    }
    encoders = {}
    for names, sha, size in re.findall(
        r'((?:WhisperModel::\w+)(?: \| WhisperModel::\w+)*) => Some\(DownloadIntegrity \{ sha256: "([a-f0-9]+)", size: ([\d_]+) \}', source
    ):
        for name in re.findall(r"WhisperModel::(\w+)", names):
            encoders[name] = (sha, int(size.replace("_", "")))
    unique(baseline["artifacts"], "id")
    require(set(urls) == {row["id"] for row in baseline["artifacts"]}, "Registry inventory mismatch")
    for row in baseline["artifacts"]:
        artifact(row)
        digest(row["revision"], 40)
        require(row["url"] == urls[row["id"]], f"Registry URL mismatch: {row['id']}")
        require((row["sha256"], row["bytes"]) == integrity[row["id"]], "Registry integrity mismatch")
        encoder = row["coreml_encoder"]
        require(bool(encoder) == (row["id"] in encoders), "Optional encoder mismatch")
        if encoder:
            artifact(encoder)
            require((encoder["sha256"], encoder["bytes"]) == encoders[row["id"]], "Encoder integrity mismatch")

    runtimes = read("runtime-assets.json")["assets"]
    unique(runtimes, "name")
    for row in runtimes:
        artifact(row)
    inventory = read("inventory.json")["items"]
    unique(inventory, "id")
    shortlist = [row for row in inventory if row["shortlisted"]]
    require(0 < len(shortlist) <= 5, "Shortlist must contain at most five families")
    for row in inventory:
        require(all(repo in by_repo for repo in row.get("repositories", [])), "Missing repository metadata")
        require(row.get("new_measurements", []) == [], "Unexpected new inference claim")
        if row["shortlisted"]:
            require(row["measurement_status"] == "blocked_not_run" and row["blockers"], "Missing per-candidate blockers")

    comparisons = read("comparisons.json")["rows"]
    unique(comparisons, "id")
    for row in comparisons:
        require(row["new_measurement"] is False, "Unexpected new comparison measurement")
        b, c = row["baseline_wer_percent"], row["candidate_wer_percent"]
        require(c >= 0, "Negative WER")
        if b is None:
            require(row.get("baseline_censor") and row["delta_wer_pp"] is None
                    and row["relative_wer_reduction_percent"] is None, "Censored baseline must not become a point estimate")
        else:
            require(b > 0, "Invalid baseline WER")
            require(math.isclose(row["delta_wer_pp"], c - b, abs_tol=0.0051), f"Absolute effect mismatch: {row['id']}")
            require(math.isclose(row["relative_wer_reduction_percent"], 100 * (b - c) / b, abs_tol=0.0051), f"Relative effect mismatch: {row['id']}")

    quality = read("quality-evidence.json")["rows"]
    unique(quality, "id")
    for row in quality:
        require(row["value"] >= 0 and row["new_measurement"] is False
                and row["cross_publisher_comparable"] is False, "Invalid quality evidence qualification")
        if row.get("publisher_metadata_revision"):
            digest(row["publisher_metadata_revision"], 40)

    gemma = by_repo["google/gemma-4-E2B-it-qat-q4_0-gguf"]["files"]
    require(sum(row["bytes"] for row in gemma) == 4_336_349_920, "Gemma two-file chain subtotal drift")
    for doc in HERE.glob("*.md"):
        for target in re.findall(r"\[[^\]]*\]\(([^)]+)\)", doc.read_text()):
            if "://" not in target and not target.startswith("#"):
                require((doc.parent / target.split("#")[0]).exists(), f"Broken local link in {doc.name}: {target}")

    print(f"PASS: {len(publisher)} repositories, {sum(len(m['files']) for m in publisher)} publisher artifacts, "
          f"{len(baseline['artifacts'])} source baselines, {len(runtimes)} runtime archives, "
          f"{len(inventory)} families ({len(shortlist)} shortlisted), {len(comparisons)} comparisons, "
          f"{len(quality)} quality rows; local Markdown links resolve.")
    print("Scope: offline consistency only; no new model measurements or downloaded-artifact verification.")


if __name__ == "__main__":
    main()
