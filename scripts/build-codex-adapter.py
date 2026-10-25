"""Stage the self-contained, pinned ACP bundle beside the private Codex CLI.

Node is provided by the matching pi-runtime group; no global npm installation,
user prefix, profile, credentials or model configuration is touched.
"""
import argparse
import hashlib
import io
import json
from pathlib import Path
import platform
import tarfile
import urllib.request

REPO = Path(__file__).resolve().parents[1]
SOURCE = REPO / 'vendor/codex-adapter/source.json'
FILES = ('dist/index.js', 'package.json', 'LICENSE', 'README.md')


def verify(root):
    assert json.loads((root / 'adapter-source.json').read_text()) == json.loads(SOURCE.read_text())
    inventory = json.loads((root / 'adapter-files.json').read_text())
    expected = {'node_modules/@agentclientprotocol/codex-acp/' + name for name in FILES}
    expected.add('codex-acp.cmd' if platform.system() == 'Windows' else 'codex-acp')
    assert set(inventory) == expected, 'Incomplete or unexpected ACP inventory'
    for name, digest in inventory.items():
        path = (root / name).resolve(strict=True)
        assert path.is_relative_to(root.resolve())
        assert hashlib.sha256(path.read_bytes()).hexdigest() == digest, name
    package = root / 'node_modules/@agentclientprotocol/codex-acp/package.json'
    assert json.loads(package.read_text())['version'] == '2.2.2'
    return {'passed': True, 'files': len(inventory), 'source': json.loads(SOURCE.read_text())}


def build(root):
    if (root / 'adapter-files.json').exists():
        return verify(root)
    source = json.loads(SOURCE.read_text())
    raw = urllib.request.urlopen(source['tarball'], timeout=60).read()
    assert hashlib.sha256(raw).hexdigest() == source['sha256'], 'ACP tarball hash mismatch'
    root.mkdir(parents=True, exist_ok=True)
    inventory = {}
    with tarfile.open(fileobj=io.BytesIO(raw)) as archive:
        for name in FILES:
            member = archive.getmember('package/' + name)
            assert member.isfile() and not member.issym()
            data = archive.extractfile(member).read()
            target = root / 'node_modules/@agentclientprotocol/codex-acp' / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            inventory[target.relative_to(root).as_posix()] = hashlib.sha256(data).hexdigest()
    windows = platform.system() == 'Windows'
    launcher = root / ('codex-acp.cmd' if windows else 'codex-acp')
    if windows:
        launcher.write_text('@echo off\r\n"%~dp0..\\pi-runtime\\node.exe" "%~dp0node_modules\\@agentclientprotocol\\codex-acp\\dist\\index.js" %*\r\nexit /b %errorlevel%\r\n', newline='')
    else:
        launcher.write_text('#!/bin/sh\nexec "$(dirname "$0")/../pi-runtime/bin/node" "$(dirname "$0")/node_modules/@agentclientprotocol/codex-acp/dist/index.js" "$@"\n')
        launcher.chmod(0o755)
    inventory[launcher.name] = hashlib.sha256(launcher.read_bytes()).hexdigest()
    (root / 'adapter-source.json').write_bytes(SOURCE.read_bytes())
    (root / 'adapter-files.json').write_text(json.dumps(inventory, indent=2) + '\n')
    return verify(root)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--verify', action='store_true')
    args = parser.parse_args()
    print(json.dumps(verify(args.output_dir.resolve()) if args.verify else build(args.output_dir.resolve())))
