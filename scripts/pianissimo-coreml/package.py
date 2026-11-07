#!/usr/bin/env python3
"""Package a convert.py output directory into the reproducible archive Sagascript downloads.

The archive holds one top-level `<name>/` directory with every file of the conversion,
stored uncompressed (the weights do not compress) with fixed 1980-01-01 timestamps and
0644 permissions in sorted order, so the same input always yields the same SHA-256.

    package.py --model-dir out/pianissimo-da-coreml-own-15s --name pianissimo-da-coreml-r1 --out publish-da

writes `<name>.zip`, `SHA256SUMS`, the conversion's `manifest.json` and
`LICENSE-and-attribution.txt` (for the Hugging Face repository), and
`manifest-for-rust.json` (archive and per-file sizes and digests for `pianissimo_model.rs`).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import zipfile
from pathlib import Path


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--model-dir", type=Path, required=True)
    parser.add_argument("--name", required=True, help="archive stem and top-level directory, e.g. pianissimo-da-coreml-r1")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()

    model_dir = args.model_dir.resolve()
    files = sorted(p for p in model_dir.rglob("*") if p.is_file())
    if not files or not (model_dir / "manifest.json").is_file():
        raise SystemExit(f"Not a convert.py output: {model_dir}")
    out = args.out.resolve()
    if out.exists() and any(out.iterdir()):
        raise SystemExit(f"Refusing to overwrite non-empty output: {out}")
    out.mkdir(parents=True, exist_ok=True)

    archive = out / f"{args.name}.zip"
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_STORED) as zf:
        for path in files:
            info = zipfile.ZipInfo(f"{args.name}/{path.relative_to(model_dir).as_posix()}", date_time=(1980, 1, 1, 0, 0, 0))
            info.external_attr = 0o644 << 16
            info.compress_type = zipfile.ZIP_STORED
            # A known size lets zipfile add Zip64 fields only when a member needs them.
            info.file_size = path.stat().st_size
            with path.open("rb") as src, zf.open(info, "w") as dst:
                shutil.copyfileobj(src, dst, 1024 * 1024)

    digest = sha256(archive)
    (out / "SHA256SUMS").write_text(f"{digest}  {archive.name}\n")
    shutil.copyfile(model_dir / "manifest.json", out / "manifest.json")
    shutil.copyfile(model_dir / "LICENSE-and-attribution.txt", out / "LICENSE-and-attribution.txt")
    rust = {
        "revision": args.name.rsplit("-", 1)[-1],
        "archive": {"name": archive.name, "size": archive.stat().st_size, "sha256": digest},
        "files": [
            {"path": p.relative_to(model_dir).as_posix(), "size": p.stat().st_size, "sha256": sha256(p)}
            for p in files
        ],
    }
    (out / "manifest-for-rust.json").write_text(json.dumps(rust, indent=1) + "\n")
    print(f"{digest}  {archive}  {archive.stat().st_size} bytes, {len(files)} files")


if __name__ == "__main__":
    main()
