import importlib.util
from contextlib import closing
from pathlib import Path
import sqlite3
import tempfile
import unittest
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


if __name__ == "__main__":
    unittest.main()
