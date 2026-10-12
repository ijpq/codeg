"""Check official Windows SQLx compatibility without changing runtime databases."""
import hashlib
import importlib.util
from pathlib import Path
import sqlite3
import subprocess
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


class WindowsCheckoutResourceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="codeg-windows-checkout-test-")
        self.addCleanup(self.temp.cleanup)
        self.source = Path(self.temp.name)
        def git(*args):
            return subprocess.check_output(["git", *args], cwd=self.source, stderr=subprocess.DEVNULL)
        self.git = git
        git("init", "--quiet")
        git("config", "core.autocrlf", "false")
        git("config", "user.name", "Fixture")
        git("config", "user.email", "fixture@example.invalid")
        (self.source / ".gitattributes").write_bytes(b"* text=auto\nkeep-lf.txt text eol=lf\nopaque.bin -text\nraw-fixture.txt -text\n")
        (self.source / "never.md").write_bytes(b"Approval policy is currently never.\n")
        (self.source / "keep-lf.txt").write_bytes(b"Preserve attribute-enforced LF.\n")
        (self.source / "opaque.bin").write_bytes(b"binary\0\n")
        (self.source / "raw-fixture.txt").write_bytes(b"Fixture LF must remain unchanged.\n")
        (self.source / "patched.rs").write_bytes(b"const ENABLED: bool = false;\n")
        git("add", ".")
        git("commit", "--quiet", "-m", "fixture")
        (self.source / "patched.rs").write_bytes(b"const ENABLED: bool = true;\n")

    def test_windows_matches_git_filters_and_preserves_private_patch_and_binary(self):
        expected = self.git("-c", "core.autocrlf=true", "cat-file", "--filters", "HEAD:never.md")
        selected = builder.prepare_windows_checkout_resources(self.source, {"patched.rs"})
        self.assertIn("never.md", selected)
        self.assertEqual((self.source / "never.md").read_bytes(), expected)
        self.assertIn(b"\r\n", expected)
        self.assertEqual((self.source / "keep-lf.txt").read_bytes(), b"Preserve attribute-enforced LF.\n")
        self.assertEqual((self.source / "opaque.bin").read_bytes(), b"binary\0\n")
        self.assertNotIn("raw-fixture.txt", selected)
        self.assertEqual((self.source / "raw-fixture.txt").read_bytes(), b"Fixture LF must remain unchanged.\n")
        self.assertEqual((self.source / "patched.rs").read_bytes(), b"const ENABLED: bool = true;\n")

    def test_preparation_is_idempotent(self):
        builder.prepare_windows_checkout_resources(self.source, {"patched.rs"})
        stamp = (self.source / "never.md").stat().st_mtime_ns
        self.assertEqual(builder.prepare_windows_checkout_resources(self.source, {"patched.rs"}), [])
        self.assertEqual((self.source / "never.md").stat().st_mtime_ns, stamp)

    def test_unexpected_resource_modification_is_not_overwritten(self):
        (self.source / "never.md").write_bytes(b"Uncommitted resource change.\n")
        with self.assertRaisesRegex(RuntimeError, "Unexpected resource source changes"):
            builder.prepare_windows_checkout_resources(self.source, {"patched.rs"})
        self.assertEqual((self.source / "never.md").read_bytes(), b"Uncommitted resource change.\n")


if __name__ == "__main__":
    unittest.main()
