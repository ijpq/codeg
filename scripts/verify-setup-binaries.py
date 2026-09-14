"""Verify unsigned Tauri NSIS payloads, including its documented bundle marker.

Tauri CLI 2.10.0 patches UNK to NSS before packaging and restores the build
output afterwards. Only that exact replacement is allowed; hash all bytes.
https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.10.0/crates/tauri-bundler/src/bundle.rs
"""

import hashlib
from pathlib import Path
import sys


def verify_binary(built: bytes, installed: bytes, *, main: bool) -> str:
    expected = built
    if main:
        marker = b"__TAURI_BUNDLE_TYPE_VAR_UNK"
        if built.count(marker) != 1:
            raise ValueError("Expected exactly one Tauri UNK bundle marker")
        expected = built.replace(marker, b"__TAURI_BUNDLE_TYPE_VAR_NSS", 1)
    expected_hash = hashlib.sha256(expected).hexdigest()
    if hashlib.sha256(installed).hexdigest() != expected_hash:
        raise ValueError("Installed binary differs from expected NSIS payload")
    return expected_hash


if __name__ == "__main__":
    build_dir, install_dir = map(Path, sys.argv[1:])
    for name in ("codeg.exe", "codeg-mcp.exe"):
        digest = verify_binary(
            (build_dir / name).read_bytes(),
            (install_dir / name).read_bytes(),
            main=name == "codeg.exe",
        )
        print(f"Verified {name}: {digest}")
