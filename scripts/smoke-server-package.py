"""Execute a packaged server on loopback with disposable data and an unused port."""

import argparse
import json
import os
from pathlib import Path
import socket
import secrets
import subprocess
import tempfile
import time
import urllib.error
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("version")
    parser.add_argument("--log-dir", type=Path, required=True)
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    logs = args.log_dir.resolve()
    logs.mkdir(parents=True, exist_ok=True)
    suffix = ".exe" if os.name == "nt" else ""
    server = root / ("codeg-server" + suffix)
    companion = root / ("codeg-mcp" + suffix)
    for path in (server, companion, root / "web" / "index.html"):
        if not path.is_file():
            raise RuntimeError(f"Required package file is missing: {path.name}")
    forbidden_names = {"auth.json", "tokens.json", "config.toml", ".env"}
    forbidden_suffixes = {".db", ".sqlite", ".sqlite3", ".pdb", ".key"}
    for path in root.rglob("*"):
        if path.is_file() and (path.name in forbidden_names or path.suffix in forbidden_suffixes):
            raise RuntimeError(f"Unexpected data or build file in package: {path.name}")
    for binary in (server, companion):
        version = subprocess.check_output([str(binary), "--version"], text=True).strip()
        if version != args.version:
            raise RuntimeError(f"{binary.name} version {version!r} differs from {args.version!r}")

    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix="codeg-package-smoke-") as scratch:
        environment = dict(os.environ)
        environment.update({
            "CODEG_HOST": "127.0.0.1", "CODEG_PORT": str(port),
            "CODEG_TOKEN": secrets.token_hex(32),
            "CODEG_DATA_DIR": str(Path(scratch) / "data"),
            "CODEG_HOME": str(Path(scratch) / "data"),
            "CODEX_HOME": str(Path(scratch) / "codex"),
            "CODEG_STATIC_DIR": str(root / "web"),
            "XDG_CACHE_HOME": str(Path(scratch) / "cache"),
            "XDG_CONFIG_HOME": str(Path(scratch) / "config"),
            "XDG_DATA_HOME": str(Path(scratch) / "share"),
        })
        with (logs / "server.stdout.log").open("w") as stdout, (logs / "server.stderr.log").open("w") as stderr:
            child = subprocess.Popen([str(server)], cwd=root, env=environment, stdout=stdout, stderr=stderr)
            started = time.monotonic()
            try:
                while True:
                    if child.poll() is not None:
                        raise RuntimeError(f"Isolated server exited with {child.returncode}; inspect smoke logs")
                    try:
                        request = urllib.request.Request(f"http://127.0.0.1:{port}/api/health", data=b"{}", headers={"Content-Type": "application/json", "Authorization": "Bearer " + environment["CODEG_TOKEN"]})
                        with urllib.request.urlopen(request, timeout=1) as response:
                            health = json.load(response)
                        break
                    except (OSError, urllib.error.URLError):
                        if time.monotonic() - started > 30:
                            raise RuntimeError("Isolated package did not become healthy within 30 seconds") from None
                        time.sleep(0.1)
                if health != {"status": "ok", "version": args.version}:
                    raise RuntimeError(f"Unexpected package health: {health}")
                with urllib.request.urlopen(f"http://127.0.0.1:{port}/", timeout=2) as response:
                    if response.status != 200 or b"<html" not in response.read().lower():
                        raise RuntimeError("Packaged frontend is not served")
                print(json.dumps({"version": args.version, "server": "passed", "mcp": "passed", "static": "passed", "startup_ms": round((time.monotonic() - started) * 1000)}))
            finally:
                # Only the child created here; no service lookup or shared-port shutdown.
                child.terminate()
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)


if __name__ == "__main__":
    main()
