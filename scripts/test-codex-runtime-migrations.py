"""Check official Windows SQLx compatibility without changing runtime databases."""
import hashlib
import importlib.util
from pathlib import Path
import sqlite3
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("runtime_builder", Path(__file__).with_name("build-codex-runtime.py"))
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


class MigrationSourceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="codeg-migration-source-test-")
        self.addCleanup(self.temp.cleanup)
        self.source = Path(self.temp.name)
        self.sql = b"CREATE TABLE fixture (id TEXT PRIMARY KEY);\n-- Preserve exact migration bytes.\n"
        self.paths = []
        for name in builder.MIGRATION_DIRECTORIES:
            path = self.source / "codex-rs" / "state" / name / "0001_fixture.sql"
            path.parent.mkdir(parents=True)
            path.write_bytes(self.sql)
            self.paths.append(path)

    def test_windows_matches_official_crlf_checksum_without_changing_database(self):
        expected_sql = self.sql.replace(b"\n", b"\r\n")
        official_checksum = hashlib.sha384(expected_sql).digest()
        self.assertNotEqual(hashlib.sha384(self.sql).digest(), official_checksum)
        database = self.source / "state_5.sqlite"
        with sqlite3.connect(database) as connection:
            connection.execute("CREATE TABLE _sqlx_migrations (version INTEGER, checksum BLOB)")
            connection.execute("INSERT INTO _sqlx_migrations VALUES (1, ?)", (official_checksum,))
        database_before = database.read_bytes()
        checksums = builder.prepare_migration_sources(self.source, windows=True)
        with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
            stored = connection.execute("SELECT checksum FROM _sqlx_migrations WHERE version = 1").fetchone()[0]
        for path in self.paths:
            self.assertEqual(path.read_bytes(), expected_sql)
            self.assertEqual(checksums[path.relative_to(self.source).as_posix()], stored.hex())
        self.assertEqual(database.read_bytes(), database_before)

    def test_windows_preparation_is_idempotent(self):
        first = builder.prepare_migration_sources(self.source, windows=True)
        mtimes = {path: path.stat().st_mtime_ns for path in self.paths}
        self.assertEqual(builder.prepare_migration_sources(self.source, windows=True), first)
        self.assertEqual({path: path.stat().st_mtime_ns for path in self.paths}, mtimes)

    def test_linux_preserves_source_bytes(self):
        before = {path: path.read_bytes() for path in self.paths}
        builder.prepare_migration_sources(self.source, windows=False)
        self.assertEqual({path: path.read_bytes() for path in self.paths}, before)

    def test_missing_pinned_sources_fail(self):
        self.paths[-1].unlink()
        with self.assertRaisesRegex(RuntimeError, "Missing pinned migration sources"):
            builder.prepare_migration_sources(self.source, windows=True)

    def test_non_newline_carriage_return_is_rejected(self):
        self.paths[0].write_bytes(b"SELECT 'unexpected\rcontent';\n")
        with self.assertRaisesRegex(RuntimeError, "Unexpected migration line endings"):
            builder.prepare_migration_sources(self.source, windows=True)


if __name__ == "__main__":
    unittest.main()
