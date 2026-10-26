"""Execute a packaged server on loopback with disposable data and an unused port."""

import argparse
import json
import os
import importlib.util
from pathlib import Path
import socket
import secrets
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("version")
    parser.add_argument("--require-runtime", action="store_true")
    parser.add_argument("--require-pi-runtime", action="store_true")
    parser.add_argument("--log-dir", type=Path, required=True)
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    logs = args.log_dir.resolve()
    logs.mkdir(parents=True, exist_ok=True)
    spec = importlib.util.spec_from_file_location("release_resources", Path(__file__).with_name("release-resources.py"))
    resources = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(resources)
    inventory = resources.verify(root / "web", args.version)
    if args.require_pi_runtime or (root / 'pi-runtime').exists():
        subprocess.run([sys.executable, '-B', str(Path(__file__).with_name('build-pi-runtime.py')),
                        '--output-dir', str(root / 'pi-runtime'), '--verify'], check=True)
    if os.name == "nt":
        subprocess.run([sys.executable, "-B", str(Path(__file__).with_name("verify-windows-dependencies.py")),
                        str(root), "--report", str(logs / "windows-dependencies.json")], check=True)
    suffix = ".exe" if os.name == "nt" else ""
    server = root / ("codeg-server" + suffix)
    companion = root / ("codeg-mcp" + suffix)
    cli = root / ("codeg-cli" + suffix)
    for path in (server, companion, cli, root / "web" / "index.html"):
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

    cli_version = subprocess.check_output([str(cli), "--version"], text=True).strip()
    if cli_version != "codeg-cli " + args.version:
        raise RuntimeError(f"CLI version mismatch: {cli_version}")

    runtime = root / "runtime" / ("codex" + suffix)
    if args.require_runtime and not runtime.is_file():
        raise RuntimeError("The release is missing its pinned Codex recovery runtime")
    if runtime.is_file():
        manifest = json.loads((runtime.parent / "source.json").read_text())
        if args.require_runtime:
            pinned = Path(__file__).resolve().parents[1] / "vendor/codex-runtime/source.json"
            if manifest != json.loads(pinned.read_text()):
                raise RuntimeError("Private Codex runtime source differs from the release pin")
        version = subprocess.check_output([str(runtime), "--version"], text=True).strip()
        if version != "codex-cli " + manifest["version"]:
            raise RuntimeError(f"Private Codex runtime version mismatch: {version}")
        import hashlib
        actual = hashlib.sha256(runtime.read_bytes()).hexdigest()
        expected = (runtime.parent / "codex.sha256").read_text().split()[0]
        if actual != expected:
            raise RuntimeError("Private Codex runtime checksum mismatch")
        if args.require_runtime and os.name == "nt":
            required = {"codex.exe", "codex-code-mode-host.exe",
                        "codex-resources/codex-command-runner.exe",
                        "codex-resources/codex-windows-sandbox-setup.exe",
                        "codex-resources/codex-windows-sandbox-service.exe", "codex-path/rg.exe"}
            entries = {}
            for line in (runtime.parent / "runtime.sha256").read_text().splitlines():
                digest, name = line.split(maxsplit=1)
                if name in entries:
                    raise RuntimeError("Duplicate private runtime checksum entry")
                entries[name] = digest
            if set(entries) != required:
                raise RuntimeError("Private runtime helpers are missing or unexpected")
            for name, digest in entries.items():
                binary = (runtime.parent / name).resolve(strict=True)
                if not binary.is_relative_to(runtime.parent.resolve()):
                    raise RuntimeError("Private runtime checksum escaped its directory")
                if hashlib.sha256(binary.read_bytes()).hexdigest() != digest:
                    raise RuntimeError("Private runtime helper checksum mismatch: " + name)
            layout = json.loads((runtime.parent / "codex-package.json").read_text())
            if layout != {"layoutVersion": 1, "version": manifest["version"],
                          "target": "x86_64-pc-windows-msvc", "variant": "codex",
                          "entrypoint": "codex.exe", "resourcesDir": "codex-resources",
                          "pathDir": "codex-path"}:
                raise RuntimeError("Private runtime package layout mismatch")
            rg_version = subprocess.check_output([str(runtime.parent / "codex-path/rg.exe"), "--version"], text=True)
            if not rg_version.startswith("ripgrep "):
                raise RuntimeError("Packaged ripgrep could not run")

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
                inventory["database"] = resources.verify_db(Path(environment["CODEG_DATA_DIR"]))
                (logs / "resources-and-migrations.json").write_text(json.dumps(inventory, indent=2))
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
