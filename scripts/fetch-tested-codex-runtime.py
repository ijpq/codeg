"""Download only the exact commit's successful ordinary-CI Windows runtime."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess


def get_json(args):
    return json.loads(subprocess.check_output(['gh', *args], text=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--repo', default=os.environ.get('GITHUB_REPOSITORY'))
    args = parser.parse_args()
    sha = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    assert args.repo, 'Repository is required'
    runs = get_json(['run', 'list', '--repo', args.repo, '--workflow', 'test.yml', '--commit', sha, '--status', 'success', '--limit', '20', '--json', 'databaseId,headSha,event,conclusion'])
    run = next(r for r in runs if r['headSha'] == sha and r['event'] == 'push' and r['conclusion'] == 'success')
    listing = get_json(['api', f"repos/{args.repo}/actions/runs/{run['databaseId']}/artifacts?per_page=100"])
    artifact = next(a for a in listing['artifacts'] if a['name'] == 'tested-codex-runtime-Windows-X64' and not a['expired'])
    assert artifact['workflow_run']['head_sha'] == sha
    root = args.output_dir.resolve()
    root.mkdir(parents=True, exist_ok=True)
    subprocess.run(['gh', 'run', 'download', str(run['databaseId']), '--repo', args.repo, '--name', artifact['name'], '--dir', str(root)], check=True)
    repo = Path(__file__).resolve().parents[1]
    assert json.loads((root / 'source.json').read_text()) == json.loads((repo / 'vendor/codex-runtime/source.json').read_text())
    for line in (root / 'runtime.sha256').read_text().splitlines():
        match = re.fullmatch(r'([a-f0-9]{64})  ([a-zA-Z0-9_./-]+)', line)
        assert match, 'Malformed runtime manifest'
        path = (root / match[2]).resolve(strict=True)
        assert path.is_relative_to(root)
        assert hashlib.sha256(path.read_bytes()).hexdigest() == match[1], match[2]
    assert (root / 'codex.exe').is_file()
    provenance = {'commit': sha, 'run_id': run['databaseId'], 'artifact_id': artifact['id'], 'artifact_digest': artifact['digest']}
    (root / 'ci-provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
    print(json.dumps(provenance))


if __name__ == '__main__':
    main()
