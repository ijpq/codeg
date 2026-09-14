import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    "verify_setup", Path(__file__).with_name("verify-setup-binaries.py")
)
verify_setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify_setup)


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


if __name__ == "__main__":
    unittest.main()
