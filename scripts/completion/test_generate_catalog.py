import unittest
from scripts.completion.generate_catalog import apply, flag, node

class CatalogImportTests(unittest.TestCase):
    def test_flag_modifiers_and_dynamic_values(self):
        stats = dict(skipped_commands=0, skipped_flags=0, skipped_values=0)
        root = node("tool")
        apply(root, {"flags": {"-o, --output=": "format", "--hidden&": "hidden"},
                     "completion": {"flag": {"output": ["json", "$carapace.ActionFiles", "unsafe;command"]}},
                     "run": "never execute"}, stats)
        self.assertEqual(root["flags"][0]["values"], ["json"])
        self.assertTrue(root["flags"][0]["takes_value"])
        self.assertEqual(stats["skipped_values"], 2)
        self.assertIsNone(flag("--hidden&", "hidden"))
        self.assertIsNone(flag("--bad;command", "bad"))

if __name__ == "__main__":
    unittest.main()
