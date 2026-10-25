"""Fetch one checksum-verified OfficeCLI into an isolated test tools directory.

This is an optional preview converter for CI, never a user installation or a
runtime silently bundled into CodeG. Does not run vendor installers or set PATH.
"""
import argparse
import hashlib
import json
import platform
import shutil
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--output', required=True)
args = parser.parse_args()
root = Path(args.output).resolve()
root.mkdir(parents=True, exist_ok=True)
version = 'v1.0.158'
if platform.system() == 'Windows':
    asset, executable = 'officecli-win-x64.exe', 'officecli.exe'
elif platform.system() == 'Linux':
    asset = 'officecli-linux-' + ('arm64' if platform.machine() in ['aarch64', 'arm64'] else 'x64')
    executable = 'officecli'
else:
    raise SystemExit('This verification supports isolated Windows/Linux runners')
source = root / 'officecli-source'
source.mkdir(exist_ok=True)
for name in [asset, 'SHA256SUMS']:
    if not (source / name).exists():
        subprocess.run(['gh', 'release', 'download', version, '--repo', 'iOfficeAI/OfficeCLI', '--pattern', name, '--dir', str(source)], check=True)
expected = next(line.split()[0] for line in (source / 'SHA256SUMS').read_text().splitlines() if line.rstrip().endswith(asset))
digest = hashlib.sha256((source / asset).read_bytes()).hexdigest()
assert digest == expected
shutil.copy2(source / asset, root / executable)
(root / executable).chmod(0o700)
info = {'version': version, 'asset': asset, 'sha256': digest, 'isolated_test_converter': True}
(root / 'officecli-test-provenance.json').write_text(json.dumps(info, indent=2) + '\n')
print(json.dumps(info))
