#!/usr/bin/env python3
"""Sign every native file in the generated macOS Pianissimo runtime.

Run after installing the Developer ID certificate and before `tauri build`.
Tauri signs the containing app after copying this already-signed runtime.
"""

import os
import stat
import subprocess
import sys
from pathlib import Path


MACH_O_MAGIC = {
    b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf",  # thin 64-bit
    b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca",  # fat
}


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: sign-pianissimo-runtime.py RUNTIME_DIR", file=sys.stderr)
        return 2
    root = Path(sys.argv[1]).resolve(strict=True)
    identity = os.environ.get("APPLE_SIGNING_IDENTITY")
    if not identity:
        print("APPLE_SIGNING_IDENTITY is required", file=sys.stderr)
        return 2
    if not (root / "share/licenses/nemo-speech/LICENSE").is_file():
        print("NeMo-Speech.cpp license is missing from runtime", file=sys.stderr)
        return 1

    native = []
    for path in root.rglob("*"):
        if path.is_symlink():
            if not path.resolve().is_relative_to(root):
                print(f"Runtime symlink escapes bundle: {path.relative_to(root)}", file=sys.stderr)
                return 1
            continue
        if not path.is_file():
            continue
        with path.open("rb") as stream:
            is_native = stream.read(4) in MACH_O_MAGIC
        if is_native:
            native.append(path)
        elif path.stat().st_mode & 0o111:
            # Scripts are imported as data and do not need execute permission.
            path.chmod(path.stat().st_mode & ~0o111)

    if not native or not (root / "bin/nemo-speech") in native:
        print("Pianissimo runtime contains no native executable", file=sys.stderr)
        return 1
    # Sign inner binaries before their containing library and the outer app.
    native.sort(key=lambda path: (-len(path.parts), str(path)))
    for path in native:
        command = ["codesign", "--force", "--sign", identity]
        if identity != "-":
            command.extend(["--options", "runtime", "--timestamp"])
        command.append(str(path))
        result = subprocess.run(command, capture_output=True, text=True)
        if result.returncode:
            raise RuntimeError(f"Signing failed for {path.relative_to(root)}: {result.stderr.strip()}")
    for path in native:
        result = subprocess.run(["codesign", "--verify", "--strict", str(path)],
                                capture_output=True, text=True)
        if result.returncode:
            raise RuntimeError(f"Signature invalid for {path.relative_to(root)}: {result.stderr.strip()}")
    print(f"Signed and verified {len(native)} Pianissimo native files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
