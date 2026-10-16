"""Generate/verify the shipped frontend inventory and compiled migration list."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import subprocess

REPO = Path(__file__).resolve().parents[1]
NAME = 'codeg-release-manifest.json'


def migrations():
    source = (REPO / 'src-tauri/src/db/migration/mod.rs').read_text()
    names = re.findall(r'Box::new\((m\w+)::Migration\)', source)
    assert names and len(names) == len(set(names))
    for name in names:
        assert (REPO / 'src-tauri/src/db/migration' / (name + '.rs')).is_file()
    return names


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify(root, version):
    root = root.resolve(strict=True)
    manifest = json.loads((root / NAME).read_text())
    assert manifest['version'] == version
    assert manifest['migrations'] == migrations(), 'Compiled migration inventory differs'
    expected_commit = os.environ.get('GITHUB_SHA')
    if expected_commit:
        assert manifest['commit'] == expected_commit
    files = manifest['files']
    for required in ('index.html', 'workspace.html', 'vs/loader.js'):
        assert required in files, 'Missing frontend entry: ' + required
    assert any(name.startswith('_next/static/') and name.endswith('.js') for name in files)
    actual_files = {p.relative_to(root).as_posix() for p in root.rglob('*') if p.is_file() and p.name != NAME}
    assert actual_files == set(files), 'Packaged frontend inventory differs'
    for name, expected in files.items():
        path = (root / name).resolve(strict=True)
        assert path.is_relative_to(root) and digest(path) == expected, 'Resource mismatch: ' + name
    return {'version': version, 'commit': manifest['commit'], 'frontend_files': len(files), 'compiled_migrations': len(manifest['migrations'])}


def verify_db(data):
    candidates = list(data.rglob('codeg.db'))
    assert len(candidates) == 1, 'Expected one isolated application database'
    with sqlite3.connect(candidates[0].as_uri() + '?mode=ro', uri=True) as db:
        applied = [row[0] for row in db.execute('SELECT version FROM seaql_migrations ORDER BY version')]
        expected = migrations()
        assert sorted(expected) == applied, {'missing': sorted(set(expected)-set(applied)), 'unexpected': sorted(set(applied)-set(expected))}
        assert db.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'
    return {'applied_migrations': applied, 'integrity': 'ok'}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('action', choices=['generate', 'verify', 'database'])
    p.add_argument('root', type=Path)
    p.add_argument('version', nargs='?')
    a = p.parse_args()
    if a.action == 'generate':
        version = json.loads((REPO / 'package.json').read_text())['version']
        commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip()
        files = {f.relative_to(a.root).as_posix(): digest(f) for f in sorted(a.root.rglob('*')) if f.is_file() and f.name != NAME}
        assert files, 'Export is empty'
        (a.root / NAME).write_text(json.dumps({'version': version, 'commit': commit, 'migrations': migrations(), 'files': files}, indent=2))
        result = verify(a.root, version)
    elif a.action == 'verify':
        result = verify(a.root, a.version)
    else:
        result = verify_db(a.root.resolve(strict=True))
    print(json.dumps(result))


if __name__ == '__main__':
    main()
