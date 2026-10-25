import importlib.util
from contextlib import closing
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import unittest
import zipfile
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "verify_setup", Path(__file__).with_name("verify-setup-binaries.py")
)
verify_setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify_setup)
dependency_spec = importlib.util.spec_from_file_location(
    "verify_dependencies", Path(__file__).with_name("verify-windows-dependencies.py")
)
dependencies = importlib.util.module_from_spec(dependency_spec)
dependency_spec.loader.exec_module(dependencies)
resource_spec = importlib.util.spec_from_file_location(
    "release_resources", Path(__file__).with_name("release-resources.py")
)
resources = importlib.util.module_from_spec(resource_spec)
resource_spec.loader.exec_module(resources)
pi_spec = importlib.util.spec_from_file_location(
    "pi_runtime", Path(__file__).with_name("build-pi-runtime.py")
)
pi_runtime = importlib.util.module_from_spec(pi_spec)
pi_spec.loader.exec_module(pi_runtime)


class PiRuntimeArchiveTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.destination = self.root / 'extracted'
        self.destination.mkdir()
        self.archive = self.root / 'node.zip'

    def directory_alias(self, alias, target):
        if os.name == 'nt':
            subprocess.run(['cmd', '/c', 'mklink', '/J', str(alias), str(target)],
                           capture_output=True, check=True)
            self.addCleanup(os.rmdir, alias)
        else:
            alias.symlink_to(target, target_is_directory=True)
            self.addCleanup(alias.unlink)

    def write_archive(self, name):
        with zipfile.ZipFile(self.archive, 'w') as bundle:
            bundle.writestr(name, b'pinned-node-bytes')

    def test_node_archive_extracts_inside_a_directory_alias(self):
        alias = self.root / 'runner-temp-alias'
        self.directory_alias(alias, self.destination)
        self.write_archive('node-version/node.exe')
        pi_runtime.extract_node_zip(self.archive, alias)
        self.assertEqual((self.destination / 'node-version/node.exe').read_bytes(),
                         b'pinned-node-bytes')

    def test_parent_traversal_is_rejected_before_extraction(self):
        self.write_archive('../outside.txt')
        with self.assertRaisesRegex(AssertionError, 'escapes extraction root'):
            pi_runtime.extract_node_zip(self.archive, self.destination)
        self.assertFalse((self.root / 'outside.txt').exists())

    def test_absolute_archive_path_is_rejected(self):
        outside = self.root / 'outside.txt'
        self.write_archive(str(outside))
        with self.assertRaisesRegex(AssertionError, 'escapes extraction root'):
            pi_runtime.extract_node_zip(self.archive, self.destination)
        self.assertFalse(outside.exists())

    def test_nested_directory_alias_cannot_escape_the_root(self):
        outside = self.root / 'outside'
        outside.mkdir()
        self.directory_alias(self.destination / 'link', outside)
        self.write_archive('link/node.exe')
        with self.assertRaisesRegex(AssertionError, 'escapes extraction root'):
            pi_runtime.extract_node_zip(self.archive, self.destination)
        self.assertEqual(list(outside.iterdir()), [])


class MigrationResourceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.path = self.root / 'codeg.db'
        with closing(sqlite3.connect(self.path)) as db:
            db.execute('CREATE TABLE seaql_migrations (version TEXT PRIMARY KEY)')
            db.executemany('INSERT INTO seaql_migrations VALUES (?)',
                           [(name,) for name in resources.migrations()])
            db.commit()
        self.probe = sqlite3.connect(self.path.as_uri() + '?mode=ro', uri=True)
        self.addCleanup(self.probe.close)

    def assert_database_released(self):
        # Retain the real connection so GC cannot hide a leaked Windows handle.
        with self.assertRaises(sqlite3.ProgrammingError):
            self.probe.execute('SELECT 1')
        self.path.unlink()

    def test_migration_check_releases_database_before_scratch_cleanup(self):
        with patch.object(resources.sqlite3, 'connect', return_value=self.probe):
            self.assertEqual(resources.verify_db(self.root)['integrity'], 'ok')
        self.assert_database_released()

    def test_failed_migration_check_also_releases_database(self):
        with patch.object(resources.sqlite3, 'connect', return_value=self.probe), \
                patch.object(resources, 'migrations', return_value=[]):
            with self.assertRaises(AssertionError):
                resources.verify_db(self.root)
        self.assert_database_released()


class VerifySetupTests(unittest.TestCase):
    built = b"prefix\x00__TAURI_BUNDLE_TYPE_VAR_UNK\x00suffix"
    installed = b"prefix\x00__TAURI_BUNDLE_TYPE_VAR_NSS\x00suffix"

    def test_expected_nsis_marker(self):
        verify_setup.verify_binary(self.built, self.installed, main=True)

    def test_unpatched_payload_rejected(self):
        with self.assertRaises(ValueError):
            verify_setup.verify_binary(self.built, self.built, main=True)

    def test_all_other_byte_changes_rejected(self):
        for offset in range(len(self.installed)):
            changed = bytearray(self.installed)
            changed[offset] ^= 1
            with self.subTest(offset=offset), self.assertRaises(ValueError):
                verify_setup.verify_binary(self.built, changed, main=True)

    def test_truncation_and_appended_bytes_rejected(self):
        for changed in (self.installed[:-1], self.installed + b"x"):
            with self.assertRaises(ValueError):
                verify_setup.verify_binary(self.built, changed, main=True)

    def test_missing_and_duplicate_markers_rejected(self):
        for built in (b"no marker", self.built * 2):
            with self.assertRaises(ValueError):
                verify_setup.verify_binary(built, self.installed, main=True)

    def test_companion_requires_exact_bytes(self):
        verify_setup.verify_binary(self.built, self.built, main=False)
        with self.assertRaises(ValueError):
            verify_setup.verify_binary(self.built, self.installed, main=False)


class WindowsDependencyTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / 'package'
        self.system = Path(self.temp.name) / 'System32'
        self.root.mkdir()
        self.system.mkdir()
        (self.root / 'codeg.exe').touch()

    def test_runner_installed_crt_does_not_hide_a_missing_package_dependency(self):
        (self.system / 'vcruntime140.dll').touch()
        with patch.object(dependencies, 'pe_imports', return_value=['vcruntime140.dll']):
            with self.assertRaisesRegex(ValueError, 'unpackaged redistributable'):
                dependencies.verify(self.root, self.system)

    def test_packaged_crt_and_system_api_are_accepted(self):
        (self.root / 'vcruntime140.dll').touch()
        (self.system / 'kernel32.dll').touch()
        def imports(path, **_):
            return ['vcruntime140.dll', 'kernel32.dll', 'api-ms-win-crt-runtime-l1-1-0.dll'] if path.suffix == '.exe' else []
        with patch.object(dependencies, 'pe_imports', side_effect=imports):
            result = dependencies.verify(self.root, self.system)
        self.assertTrue(result['passed'])
        self.assertEqual(result['binaries'][0]['imports']['vcruntime140.dll'], 'packaged')

    def test_unknown_missing_dll_is_rejected(self):
        with patch.object(dependencies, 'pe_imports', return_value=['missing-helper.dll']):
            with self.assertRaisesRegex(ValueError, 'unresolved import'):
                dependencies.verify(self.root, self.system)

    def test_native_node_module_dependencies_are_checked(self):
        module = self.root / 'addon.node'
        module.touch()
        with patch.object(dependencies, 'pe_imports', side_effect=lambda p, **_: ['missing-addon.dll'] if p == module else []):
            with self.assertRaisesRegex(ValueError, 'unresolved import'):
                dependencies.verify(self.root, self.system)

    def test_pi_foreign_prebuilds_are_inventoried_without_loading_them(self):
        native = self.root / 'pi-runtime/node_modules/@earendil-works/pi-tui/native'
        windows = native / 'win32/prebuilds/win32-x64/win32-platform.node'
        foreign = native / 'win32/prebuilds/win32-arm64/win32-platform.node'
        for path in [windows, foreign]:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.touch()
        with patch.object(dependencies, 'pe_imports', return_value=[]) as probe:
            result = dependencies.verify(self.root, self.system)
        self.assertEqual(result['inactive_native_modules'], [foreign.relative_to(self.root).as_posix()])
        self.assertIn(windows, [c.args[0] for c in probe.call_args_list])
        self.assertNotIn(foreign, [c.args[0] for c in probe.call_args_list])


if __name__ == "__main__":
    unittest.main()
