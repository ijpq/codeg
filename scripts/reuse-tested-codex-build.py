"""Reuse a fully tested Windows CLI only when every native build input matches.

This caches the unchanged component, not the application checks. The current CI
still exercises resume, model context, tools, images and native Plan/Goal, and
exports its own artifact. A changed builder/source/patch, missing original build
proof or unavailable artifact falls back to a complete native build.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time
import zipfile

ROOT = Path(__file__).resolve().parents[1]
INPUTS = ('vendor/codex-runtime', 'scripts/build-codex-runtime.py')
BINARIES = {
    'codex.exe', 'codex-code-mode-host.exe', 'codex-path/rg.exe',
    'codex-resources/codex-command-runner.exe',
    'codex-resources/codex-windows-sandbox-setup.exe',
    'codex-resources/codex-windows-sandbox-service.exe',
}
METADATA = {
    'source.json', 'LICENSE', 'NOTICE', 'codex-package.json', 'codex.sha256',
    'runtime.sha256', 'migration-sources.json',
    'codex-resources/ripgrep-LICENSE-MIT', 'codex-resources/ripgrep-UNLICENSE',
}


def transient_read_error(cause):
    error = cause.stderr or ''
    if isinstance(error, bytes):
        error = error.decode('utf-8', errors='replace')
    return re.search(r'\bHTTP (?:500|502|503|504)\b', error) is not None


def json_command(args):
    for attempt in range(3):
        try:
            return json.loads(subprocess.check_output(args, text=True, cwd=ROOT,
                                                     stderr=subprocess.PIPE))
        except subprocess.CalledProcessError as cause:
            if not transient_read_error(cause) or attempt == 2:
                raise
            print('Temporary GitHub read failure; retrying build proof lookup', flush=True)
            time.sleep(attempt + 1)


def digest(path):
    checksum = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            checksum.update(chunk)
    return checksum.hexdigest()


def verify(root):
    source = json.loads((ROOT / 'vendor/codex-runtime/source.json').read_text())
    assert json.loads((root / 'source.json').read_text()) == source, 'Source pin differs'
    declared = {}
    for line in (root / 'runtime.sha256').read_text().splitlines():
        match = re.fullmatch(r'([a-f0-9]{64})  ([a-zA-Z0-9_./-]+)', line)
        assert match and match[2] in BINARIES, 'Unexpected runtime manifest entry'
        assert match[2] not in declared, 'Duplicate runtime entry'
        declared[match[2]] = match[1]
    assert set(declared) == BINARIES, 'Incomplete native runtime group'
    for name, expected in declared.items():
        assert digest(root / name) == expected, 'Runtime hash mismatch: ' + name
    assert (root / 'codex.sha256').read_text().strip() == declared['codex.exe'] + '  codex.exe'
    layout = json.loads((root / 'codex-package.json').read_text())
    assert layout == {
        'layoutVersion': 1, 'version': source['version'],
        'target': 'x86_64-pc-windows-msvc', 'variant': 'codex',
        'entrypoint': 'codex.exe', 'resourcesDir': 'codex-resources',
        'pathDir': 'codex-path',
    }, 'Runtime layout differs'
    for name in ('LICENSE', 'NOTICE'):
        assert (root / name).read_text(encoding='utf-8') == (ROOT / 'vendor/codex-runtime' / name).read_text(encoding='utf-8'), 'License content differs: ' + name
    migrations = json.loads((root / 'migration-sources.json').read_text())
    assert migrations['line_endings'] == 'CRLF' and migrations['sha384']
    assert all(re.fullmatch(r'[a-f0-9]{96}', value) for value in migrations['sha384'].values())


def extract(archive, root):
    with zipfile.ZipFile(archive) as package:
        files = [info for info in package.infolist() if not info.is_dir()]
        names = [info.filename for info in files]
        assert len(names) == len(set(names)), 'Duplicate archive entry'
        assert set(names) <= BINARIES | METADATA, 'Unexpected archive contents: ' + str(sorted(set(names) - BINARIES - METADATA))
        for info in files:
            assert not ((info.external_attr >> 16) & 0o170000) == 0o120000, 'Archive symlink'
            target = root / info.filename
            target.parent.mkdir(parents=True, exist_ok=True)
            with package.open(info) as stream, target.open('wb') as output:
                shutil.copyfileobj(stream, output)
    verify(root)


def build_proved(jobs):
    return any(
        job['name'] == 'Codex recovery on windows-2022'
        and job['conclusion'] == 'success'
        and any(step['name'] == 'Test and build pinned recovery runtime'
                and step['conclusion'] == 'success' for step in job['steps'])
        for job in jobs
    )


def matching_inputs(sha, repository=ROOT):
    return subprocess.run(['git', 'diff', '--quiet', sha, 'HEAD', '--', *INPUTS],
                          cwd=repository).returncode == 0


def reuse(repo, output):
    runs = json_command(['gh', 'run', 'list', '--repo', repo, '--workflow', 'test.yml',
                         '--branch', 'main', '--event', 'push', '--status', 'success',
                         '--limit', '30', '--json', 'databaseId,headSha'])
    for run in runs:
        sha = run['headSha']
        known = subprocess.run(['git', 'cat-file', '-e', sha + '^{commit}'], cwd=ROOT,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if known.returncode:
            subprocess.run(['git', 'fetch', '--no-tags', '--depth=1', 'origin', sha], cwd=ROOT, check=True)
        if not matching_inputs(sha):
            continue
        try:
            details = json_command(['gh', 'api', f"repos/{repo}/actions/runs/{run['databaseId']}"])
            assert details['head_sha'] == sha and details['event'] == 'push' and details['head_branch'] == 'main'
            assert details['head_repository']['full_name'].lower() == repo.lower()
            assert details['path'] == '.github/workflows/test.yml' and details['conclusion'] == 'success'
            jobs = json_command(['gh', 'api', f"repos/{repo}/actions/runs/{run['databaseId']}/jobs?per_page=100"])
            if not build_proved(jobs['jobs']):
                continue  # Never substitute an unverified/chained reuse for original build proof.
            artifacts = json_command(['gh', 'api', f"repos/{repo}/actions/runs/{run['databaseId']}/artifacts?per_page=100"])
        except subprocess.CalledProcessError as cause:
            if not transient_read_error(cause):
                raise
            print('Skipping unavailable build proof:', run['databaseId'], flush=True)
            continue  # No readable proof means this candidate can never be reused.
        artifact = next((a for a in artifacts['artifacts'] if not a['expired']
                         and a['name'] == 'tested-codex-runtime-Windows-X64'), None)
        if not artifact:
            continue
        assert artifact['workflow_run']['head_sha'] == sha
        assert re.fullmatch(r'sha256:[a-f0-9]{64}', artifact['digest']), 'Missing artifact digest'
        with tempfile.TemporaryDirectory(prefix='codex-build-reuse-', dir=output.parent) as scratch:
            scratch = Path(scratch)
            archive = scratch / 'native.zip'
            with archive.open('wb') as stream:
                subprocess.run(['gh', 'api', f"repos/{repo}/actions/artifacts/{artifact['id']}/zip"],
                               stdout=stream, check=True)
            assert 'sha256:' + digest(archive) == artifact['digest'], 'Archive digest mismatch'
            staged = scratch / 'verified'
            staged.mkdir()
            extract(archive, staged)
            assert not output.exists() or not any(output.iterdir()), 'Output is not empty'
            shutil.copytree(staged, output, dirs_exist_ok=True)
        proof = {'originalBuildRun': run['databaseId'], 'originalBuildCommit': sha,
                 'artifactId': artifact['id'], 'artifactDigest': artifact['digest'],
                 'unchangedInputs': list(INPUTS), 'currentProtocolChecksRequired': True}
        (output / 'build-reuse.json').write_text(json.dumps(proof, indent=2) + '\n')
        print(json.dumps(proof))
        return True
    return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--repo', default=os.environ.get('GITHUB_REPOSITORY'))
    args = parser.parse_args()
    args.output_dir.parent.mkdir(parents=True, exist_ok=True)
    reused = False
    try:
        assert args.repo
        reused = reuse(args.repo, args.output_dir.resolve())
    except (AssertionError, OSError, ValueError, KeyError, zipfile.BadZipFile,
            subprocess.CalledProcessError) as cause:
        print('No reusable native build; full compilation is required:', type(cause).__name__, str(cause))
    if os.environ.get('GITHUB_OUTPUT'):
        with open(os.environ['GITHUB_OUTPUT'], 'a') as stream:
            stream.write('reused=' + str(reused).lower() + '\n')
    print('Reused original fully tested build:', reused)


if __name__ == '__main__':
    main()
