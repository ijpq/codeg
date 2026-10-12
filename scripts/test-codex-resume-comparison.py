"""Regression checks for meaningful model-context comparisons on Windows/Linux."""
import importlib.util
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("resume_fixture", Path(__file__).with_name("test-codex-resume.py"))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class ModelContextComparisonTests(unittest.TestCase):
    def context(self, home):
        return [{"type": "message", "id": "persisted-message", "role": "user", "content": [
            {"type": "input_text", "text": "ESSENTIAL CONTEXT\nSkills root: " + home + "/skills"},
            {"type": "input_image", "image_url": fixture.IMAGE},
        ]}, {"type": "function_call", "call_id": "persisted-tool", "arguments": "{}"},
        {"type": "function_call_output", "call_id": "persisted-tool", "output": "TOOL RESULT"}]

    def test_windows_roots_compare_after_json_roundtrip(self):
        baseline = r"C:\fixtures\baseline\home"
        candidate = r"C:\fixtures\candidate\home"
        left, right = [json.loads(json.dumps(self.context(home))) for home in [baseline, candidate]]
        # Negative control: the old serialized replacement leaves paths different.
        old_left = json.loads(json.dumps(left).replace(baseline, "<ISOLATED_CODEX_HOME>"))
        old_right = json.loads(json.dumps(right).replace(candidate, "<ISOLATED_CODEX_HOME>"))
        self.assertNotEqual(old_left, old_right)
        self.assertEqual(fixture.normalize_fixture_home(left, baseline), fixture.normalize_fixture_home(right, candidate))

    def test_windows_forward_slash_skill_roots_match_without_hiding_real_changes(self):
        baseline = r"D:\fixtures\baseline\home"
        candidate = r"D:\fixtures\candidate\home"
        left = self.context(baseline.replace("\\", "/"))
        right = self.context(candidate.replace("\\", "/"))
        normalized = fixture.normalize_fixture_home(left, baseline)
        self.assertEqual(normalized, fixture.normalize_fixture_home(right, candidate))
        self.assertEqual(normalized[0]["content"][1]["image_url"], fixture.IMAGE)
        right[0]["content"][0]["text"] += " CHANGED INSTRUCTION"
        self.assertNotEqual(normalized, fixture.normalize_fixture_home(right, candidate))

    def test_linux_comparison_is_unchanged_and_preserves_images_and_ids(self):
        home = "/fixture/baseline/home"
        actual = fixture.normalize_fixture_home(self.context(home), home)
        old = json.loads(json.dumps(self.context(home)).replace(home, "<ISOLATED_CODEX_HOME>"))
        self.assertEqual(actual, old)
        self.assertEqual(actual[0]["id"], "persisted-message")
        self.assertEqual(actual[0]["content"][1]["image_url"], fixture.IMAGE)
        self.assertEqual(actual[1]["call_id"], actual[2]["call_id"])
        self.assertEqual(actual[2]["output"], "TOOL RESULT")

    def test_real_context_changes_still_fail_comparison(self):
        home = r"C:\fixtures\home"
        left, right = self.context(home), self.context(home)
        right[0]["content"][0]["text"] = right[0]["content"][0]["text"].replace("ESSENTIAL CONTEXT", "LOST CONTEXT")
        self.assertNotEqual(fixture.normalize_fixture_home(left, home), fixture.normalize_fixture_home(right, home))


if __name__ == "__main__":
    unittest.main()
