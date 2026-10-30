"""Reject stale, partial and corrupted native build cache entries."""
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from unittest import mock
import zipfile

spec = importlib.util.spec_from_file_location('reuse', Path(__file__).with_name('reuse-tested-codex-build.py'))
reuse = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reuse)


class NativeBuildReuseTests(unittest.TestCase):
    def test_transient_github_read_retries_without_weakening_build_proof(self):
        args = ['gh', 'api', 'repos/fixture/actions/jobs']
        unavailable = subprocess.CalledProcessError(1, args, stderr=b'gh: Server Error (HTTP 502)')
        with mock.patch.object(reuse.subprocess, 'check_output', side_effect=[unavailable, b'{"jobs": []}']) as read, mock.patch.object(time, 'sleep'):
            self.assertEqual(reuse.json_command(args), {'jobs': []})
            self.assertEqual(read.call_count, 2)

    def test_permission_errors_and_invalid_proof_are_never_retried_as_network_failures(self):
        args = ['gh', 'api', 'repos/fixture/actions/jobs']
        denied = subprocess.CalledProcessError(1, args, stderr=b'gh: Forbidden (HTTP 403)')
        with mock.patch.object(reuse.subprocess, 'check_output', side_effect=denied) as read, mock.patch.object(time, 'sleep'):
            with self.assertRaises(subprocess.CalledProcessError):
                reuse.json_command(args)
            self.assertEqual(read.call_count, 1)
        with mock.patch.object(reuse.subprocess, 'check_output', return_value=b'invalid JSON') as read:
            with self.assertRaises(ValueError):
                reuse.json_command(args)
            self.assertEqual(read.call_count, 1)

    def test_persistent_github_failure_has_a_bounded_retry_budget(self):
        args = ['gh', 'api', 'repos/fixture/actions/jobs']
        unavailable = subprocess.CalledProcessError(1, args, stderr='gh: Server Error (HTTP 502)')
        with mock.patch.object(reuse.subprocess, 'check_output', side_effect=unavailable) as read, mock.patch.object(time, 'sleep') as sleep:
            with self.assertRaises(subprocess.CalledProcessError):
                reuse.json_command(args)
            self.assertEqual(read.call_count, 3)
            self.assertEqual(sleep.call_count, 2)

    def test_unavailable_candidate_is_never_accepted_and_later_candidates_are_checked(self):
        args = ['gh', 'api', 'repos/fixture/actions/jobs']
        unavailable = subprocess.CalledProcessError(1, args, stderr='gh: Server Error (HTTP 502)')
        runs = [{'databaseId': 1, 'headSha': 'a' * 40}, {'databaseId': 2, 'headSha': 'b' * 40}]
        details = {'head_sha': 'b' * 40, 'event': 'push', 'head_branch': 'main',
                   'head_repository': {'full_name': 'fixture/repo'},
                   'path': '.github/workflows/test.yml', 'conclusion': 'success'}
        with mock.patch.object(reuse, 'json_command', side_effect=[runs, unavailable, details, {'jobs': []}]) as read, mock.patch.object(reuse, 'matching_inputs', return_value=True), mock.patch.object(reuse.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)):
            self.assertFalse(reuse.reuse('fixture/repo', self.root / 'runtime'))
            self.assertEqual(read.call_count, 4)
            self.assertFalse((self.root / 'runtime').exists())
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='codeg-build-reuse-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.payload = self.root / 'payload'
        self.payload.mkdir()
        self.declared = {}
        for name in reuse.BINARIES:
            path = self.payload / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(('isolated binary fixture: ' + name).encode())
            self.declared[name] = hashlib.sha256(path.read_bytes()).hexdigest()
        for name in ('source.json', 'LICENSE', 'NOTICE'):
            (self.payload / name).write_bytes((reuse.ROOT / 'vendor/codex-runtime' / name).read_bytes())
        source = json.loads((self.payload / 'source.json').read_text())
        (self.payload / 'codex-package.json').write_text(json.dumps({
            'layoutVersion': 1, 'version': source['version'], 'target': 'x86_64-pc-windows-msvc',
            'variant': 'codex', 'entrypoint': 'codex.exe', 'resourcesDir': 'codex-resources',
            'pathDir': 'codex-path',
        }))
        (self.payload / 'runtime.sha256').write_text(''.join(value + '  ' + name + '\n' for name, value in self.declared.items()))
        (self.payload / 'codex.sha256').write_text(self.declared['codex.exe'] + '  codex.exe\n')
        (self.payload / 'migration-sources.json').write_text(json.dumps({'line_endings': 'CRLF', 'sha384': {'fixture.sql': 'a' * 96}}))

    def test_complete_group_and_archive_are_accepted(self):
        reuse.verify(self.payload)
        archive = self.root / 'runtime.zip'
        with zipfile.ZipFile(archive, 'w') as bundle:
            for path in self.payload.rglob('*'):
                if path.is_file():
                    bundle.write(path, path.relative_to(self.payload).as_posix())
        reuse.extract(archive, self.root / 'extracted')

    def test_missing_helper_and_changed_binary_are_rejected(self):
        path = self.payload / 'codex-resources/codex-command-runner.exe'
        data = path.read_bytes()
        path.unlink()
        with self.assertRaises(OSError):
            reuse.verify(self.payload)
        path.write_bytes(data + b'changed')
        with self.assertRaisesRegex(AssertionError, 'hash mismatch'):
            reuse.verify(self.payload)

    def test_mismatched_source_layout_and_migrations_are_rejected(self):
        for name, value in [('source.json', {}), ('codex-package.json', {}), ('migration-sources.json', {'line_endings': 'LF', 'sha384': {}})]:
            with self.subTest(name=name):
                path = self.payload / name
                original = path.read_bytes()
                path.write_text(json.dumps(value))
                with self.assertRaises(AssertionError):
                    reuse.verify(self.payload)
                path.write_bytes(original)

    def test_license_line_endings_are_portable_but_content_must_match(self):
        path = self.payload / 'LICENSE'
        original = path.read_text(encoding='utf-8')
        path.write_bytes(original.replace('\n', '\r\n').encode())
        reuse.verify(self.payload)
        path.write_text(original + '\nUnrelated license\n', encoding='utf-8')
        with self.assertRaisesRegex(AssertionError, 'License content differs'):
            reuse.verify(self.payload)

    def test_duplicate_and_undeclared_manifest_entries_are_rejected(self):
        path = self.payload / 'runtime.sha256'
        original = path.read_text()
        for extra in [original.splitlines()[0], 'a' * 64 + '  ../escape.exe']:
            with self.subTest(extra=extra), self.assertRaises(AssertionError):
                path.write_text(original + extra + '\n')
                reuse.verify(self.payload)

    def test_archive_cannot_escape_or_add_files(self):
        for name in ('../escape.exe', '/absolute.exe', 'auth.json'):
            archive = self.root / 'bad.zip'
            with zipfile.ZipFile(archive, 'w') as bundle:
                bundle.writestr(name, 'must not be written')
            with self.subTest(name=name), self.assertRaises(AssertionError):
                reuse.extract(archive, self.root / 'rejected')
        self.assertFalse((self.root / 'escape.exe').exists())

    def test_only_original_successful_native_build_is_proof(self):
        step = {'name': 'Test and build pinned recovery runtime', 'conclusion': 'success'}
        job = {'name': 'Codex recovery on windows-2022', 'conclusion': 'success', 'steps': [step]}
        self.assertTrue(reuse.build_proved([job]))
        step['conclusion'] = 'skipped'
        self.assertFalse(reuse.build_proved([job]))
        step['conclusion'] = 'success'
        job['conclusion'] = 'failure'
        self.assertFalse(reuse.build_proved([job]))
        job['name'] = 'Codex recovery on ubuntu-22.04'
        job['conclusion'] = 'success'
        self.assertFalse(reuse.build_proved([job]))

    def test_app_changes_reuse_but_builder_and_patch_changes_require_compilation(self):
        repository = self.root / 'repository'
        repository.mkdir()
        def git(*args):
            return subprocess.check_output(['git', '-c', 'user.name=Native build fixture',
                '-c', 'user.email=fixture@example.invalid', *args], cwd=repository,
                stderr=subprocess.DEVNULL, text=True).strip()
        git('init')
        patch = repository / 'vendor/codex-runtime/checkpoint-resume.patch'
        patch.parent.mkdir(parents=True)
        patch.write_text('original patch\n')
        builder = repository / 'scripts/build-codex-runtime.py'
        builder.parent.mkdir()
        builder.write_text('original builder\n')
        git('add', '.')
        git('commit', '-m', 'original native inputs')
        original = git('rev-parse', 'HEAD')
        (repository / 'app.txt').write_text('application-only change\n')
        git('add', '.')
        git('commit', '-m', 'application update')
        self.assertTrue(reuse.matching_inputs(original, repository))
        for path in [builder, patch]:
            path.write_text('changed native build input\n')
            git('add', '.')
            git('commit', '-m', 'native build update')
            self.assertFalse(reuse.matching_inputs(original, repository))


if __name__ == '__main__':
    unittest.main()
