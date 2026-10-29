"""Build the private, locked Node + Pi + CodeG ACP runtime group; never install globally."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request
import zipfile

REPO = Path(__file__).resolve().parents[1]
VENDOR = REPO / 'vendor/pi-runtime'
ARCHIVES = {
    ('Windows', 'AMD64'): ('node-v24.16.0-win-x64.zip', 'edaca9bd58ec8e92037dac4e877d52f6b8f430b81c18b57e264b4e2fb111cd56'),
    ('Linux', 'x86_64'): ('node-v24.16.0-linux-x64.tar.xz', 'd804845d34eddc21dc1092b519d643ef40b1f58ec5dec5c22b1f4bd8fabde6c9'),
    ('Linux', 'aarch64'): ('node-v24.16.0-linux-arm64.tar.xz', '524659219d6a207a7400f2bde15d19ba060ffbe0d32a8643319ad67e3bb64c78'),
    ('Darwin', 'arm64'): ('node-v24.16.0-darwin-arm64.tar.xz', 'e28ad5531b2aafe0ea555a51b2412c42fdc0f91a6a53fbd03ac93e3847e91389'),
    ('Darwin', 'x86_64'): ('node-v24.16.0-darwin-x64.tar.xz', '6b144acbcfdbca75a1366100ff96e6bf6a4fe666b88a4bda7bfbd0299c82cca2'),
}


def sha(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def verify(root):
    assert json.loads((root / 'source.json').read_text()) == json.loads((VENDOR / 'source.json').read_text()), 'Pi runtime source mismatch'
    source = json.loads((root / 'source.json').read_text())
    assert sha(root / ('pi-acp-' + source['adapterVersion'] + '.tgz')) == source['adapterTarballSha256'], 'Pi adapter source archive checksum mismatch'
    expected = json.loads((root / 'runtime-files.json').read_text())
    actual = {p.relative_to(root).as_posix() for p in root.rglob('*') if p.is_file() and p.name != 'runtime-files.json'}
    assert actual == set(expected), 'Pi runtime file inventory mismatch'
    for name, digest in expected.items():
        path = (root / name).resolve(strict=True)
        assert path.is_relative_to(root.resolve()) and sha(path) == digest, 'Pi runtime checksum mismatch: ' + name
    assert json.loads((root / 'node_modules/pi-acp/package.json').read_text())['version'] == json.loads((VENDOR / 'source.json').read_text())['adapterVersion']
    assert json.loads((root / 'node_modules/@earendil-works/pi-coding-agent/package.json').read_text())['version'] == '1.4.0'
    for name in ('node_modules/pi-acp/dist/pi-mcp-extension.js', 'NODE-LICENSE', 'node_modules/pi-acp/LICENSE'):
        assert (root / name).is_file(), 'Pi runtime missing ' + name
    return {'passed': True, 'files': len(expected), 'source': json.loads((root / 'source.json').read_text())}


def extract_node_zip(archive, destination):
    # RUNNER_TEMP can be a Windows junction (and /var a macOS alias). Compare
    # resolved members against that same physical root, retaining containment
    # checks for traversal and nested links before writing any archive member.
    destination = destination.resolve(strict=True)
    with zipfile.ZipFile(archive) as bundle:
        assert all((destination / name).resolve().is_relative_to(destination) for name in bundle.namelist()), 'Node archive path escapes extraction root'
        bundle.extractall(destination)


def build(root):
    if root.exists():
        return verify(root)
    archive_name, digest = ARCHIVES[(platform.system(), platform.machine())]
    work = Path(tempfile.mkdtemp(prefix='codeg-pi-runtime-'))
    archive = work / archive_name
    urllib.request.urlretrieve('https://nodejs.org/dist/v24.16.0/' + archive_name, archive)
    assert sha(archive) == digest, 'Official Node archive checksum mismatch'
    extracted = work / 'node'
    extracted.mkdir()
    if archive_name.endswith('.zip'):
        extract_node_zip(archive, extracted)
    else:
        with tarfile.open(archive) as bundle:
            # Invoke npm-cli.js with the pinned Node directly; the distribution's
            # bin/npm/npx/corepack links are unnecessary in a private runtime.
            bundle.extractall(extracted, members=[m for m in bundle.getmembers() if m.isfile() or m.isdir()], filter='data')
    source = next(extracted.iterdir())
    windows = platform.system() == 'Windows'
    node = source / ('node.exe' if windows else 'bin/node')
    npm = source / ('node_modules/npm/bin/npm-cli.js' if windows else 'lib/node_modules/npm/bin/npm-cli.js')
    assert subprocess.check_output([str(node), '--version'], text=True).strip() == 'v24.16.0'
    root.mkdir(parents=True)
    target_node = root / ('node.exe' if windows else 'bin/node')
    target_node.parent.mkdir(exist_ok=True)
    shutil.copy2(node, target_node)
    shutil.copy2(source / 'LICENSE', root / 'NODE-LICENSE')
    for name in ('package.json', 'package-lock.json', 'source.json', 'pi-acp-' + json.loads((VENDOR / 'source.json').read_text())['adapterVersion'] + '.tgz'):
        shutil.copy2(VENDOR / name, root / name)
    env = {**os.environ, 'PATH': str(node.parent) + os.pathsep + os.environ.get('PATH', '')}
    subprocess.run([str(node), str(npm), 'ci', '--omit=dev', '--include=optional', '--ignore-scripts', '--no-audit', '--no-fund'], cwd=root, env=env, check=True)
    entries = {'pi': 'node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js', 'pi-acp': 'node_modules/pi-acp/dist/index.js'}
    for name, entry in entries.items():
        if windows:
            (root / (name + '.cmd')).write_text('@echo off\r\nsetlocal\r\nset "PATH=%~dp0;%PATH%"\r\n"%~dp0node.exe" "%~dp0' + entry.replace('/', '\\') + '" %*\r\nexit /b %errorlevel%\r\n', newline='')
        else:
            launcher = root / name
            launcher.write_text('#!/bin/sh\nexec "$(dirname "$0")/bin/node" "$(dirname "$0")/' + entry + '" "$@"\n')
            launcher.chmod(0o755)
    assert subprocess.check_output([str(target_node), str(root / entries['pi']), '--version'], text=True, env=env).strip() == '1.4.0'
    expected = {p.relative_to(root).as_posix(): sha(p) for p in sorted(root.rglob('*')) if p.is_file()}
    (root / 'runtime-files.json').write_text(json.dumps(expected, indent=2))
    result = verify(root)
    result['build_temporary_path'] = str(work)
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--verify', action='store_true')
    args = parser.parse_args()
    root = args.output_dir.resolve()
    print(json.dumps(verify(root) if args.verify else build(root)))
