"""Reproduce a restored Cargo cache missing rusty_v8's external native library."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "runtime_builder", Path(__file__).with_name("build-codex-runtime.py")
)
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


class WindowsV8CacheTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="codeg-v8-cache-test-")
        self.addCleanup(self.temp.cleanup)
        self.workspace = Path(self.temp.name) / "source"
        self.workspace.mkdir()
        self.target = Path(self.temp.name) / "target"
        self.output = self.target / "release/build/v8-fixture/output"
        self.library = self.target / "release/gn_out/obj/rusty_v8.lib"
        self.cargo = ["cargo"]

    def cached_output(self):
        self.output.parent.mkdir(parents=True)
        self.output.write_text("cargo:rustc-link-search=release/gn_out/obj\n")

    def test_fresh_target_and_complete_cache_need_no_invalidation(self):
        with patch.object(builder, "run") as run:
            self.assertFalse(builder.repair_windows_v8_cache(self.cargo, self.workspace, self.target))
            self.cached_output()
            self.library.parent.mkdir(parents=True)
            self.library.write_bytes(b"native library fixture")
            self.assertFalse(builder.repair_windows_v8_cache(self.cargo, self.workspace, self.target))
            run.assert_not_called()

    def test_missing_or_empty_library_invalidates_only_v8(self):
        self.cached_output()
        for empty in [False, True]:
            with self.subTest(empty=empty), patch.object(builder, "run") as run:
                if empty:
                    self.library.parent.mkdir(parents=True)
                    self.library.write_bytes(b"")
                self.assertTrue(builder.repair_windows_v8_cache(self.cargo, self.workspace, self.target))
                run.assert_called_once_with(["cargo", "clean", "--release", "--package", "v8"], self.workspace)

    def test_invalidation_failure_is_not_hidden(self):
        self.cached_output()
        with patch.object(builder, "run", side_effect=subprocess.CalledProcessError(101, "cargo")):
            with self.assertRaises(subprocess.CalledProcessError):
                builder.repair_windows_v8_cache(self.cargo, self.workspace, self.target)

    def test_real_cargo_cache_repair_restores_build_script_output(self):
        # Use a dependency-free package named v8 to exercise Cargo's real
        # build-script caching. No downloaded V8 or installed CLI is modified.
        (self.workspace / "src").mkdir()
        (self.workspace / "Cargo.toml").write_text(
            '[package]\nname = "v8"\nversion = "0.0.1"\nedition = "2021"\n'
        )
        (self.workspace / "src/lib.rs").write_text("pub const FIXTURE: bool = true;\n")
        (self.workspace / "build.rs").write_text(r'''
use std::{env, fs, path::PathBuf};
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let profile = out.ancestors().nth(3).unwrap();
    let library = profile.join("gn_out/obj/rusty_v8.lib");
    fs::create_dir_all(library.parent().unwrap()).unwrap();
    fs::write(library, b"verified fixture library").unwrap();
}
''')
        environment = dict(os.environ, CARGO_TARGET_DIR=str(self.target))
        for name in ["CARGO_BUILD_TARGET", "CARGO_ENCODED_RUSTFLAGS", "RUSTFLAGS"]:
            environment.pop(name, None)
        with patch.dict(os.environ, environment, clear=True):
            builder.run(self.cargo + ["generate-lockfile", "--offline"], self.workspace)
            command = self.cargo + ["build", "--release", "--locked", "--offline"]
            builder.run(command, self.workspace)
            self.assertEqual(self.library.read_bytes(), b"verified fixture library")
            self.library.unlink()
            builder.run(command, self.workspace)
            self.assertFalse(self.library.exists(), "Cargo must reproduce the stale build-script cache")
            sentinel = self.target / "unrelated-cache-fixture"
            sentinel.write_bytes(b"keep other packages")
            self.assertTrue(builder.repair_windows_v8_cache(self.cargo, self.workspace, self.target))
            builder.run(command, self.workspace)
            self.assertEqual(self.library.read_bytes(), b"verified fixture library")
            self.assertEqual(sentinel.read_bytes(), b"keep other packages")
            self.assertFalse(builder.repair_windows_v8_cache(self.cargo, self.workspace, self.target))


if __name__ == "__main__":
    unittest.main()
