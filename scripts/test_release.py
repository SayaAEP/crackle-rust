import unittest

from release import bump_version, parse_commit, release_notes


class ReleaseTests(unittest.TestCase):
    def test_auto_bump_selects_major_for_breaking_changes(self):
        version, _ = bump_version(
            "0.4.2",
            "auto",
            ["fix: correct output\n\nBREAKING CHANGE: output format changed"],
        )
        self.assertEqual(version, "1.0.0")

    def test_auto_bump_selects_minor_for_features(self):
        version, _ = bump_version("0.4.2", "auto", ["feat(cli): add status"])
        self.assertEqual(version, "0.5.0")

    def test_auto_bump_selects_patch_otherwise_and_manual_levels_override(self):
        self.assertEqual(bump_version("0.4.2", "auto", ["fix: correct output"])[0], "0.4.3")
        self.assertEqual(bump_version("0.4.2", "major", ["fix: correct output"])[0], "1.0.0")

    def test_rejects_non_conventional_subjects(self):
        with self.assertRaisesRegex(ValueError, "not a Conventional Commit"):
            parse_commit("added a thing")

    def test_release_notes_include_categories_and_breaking_marker(self):
        notes = release_notes(
            "0.5.0",
            [
                ("abcdef012345", ("feat", "feat: add status", False)),
                ("012345abcdef", ("fix", "fix: change format", True)),
            ],
        )
        self.assertIn("# v0.5.0", notes)
        self.assertIn("## Features", notes)
        self.assertIn("add status (abcdef0)", notes)
        self.assertIn("change format (breaking)", notes)


if __name__ == "__main__":
    unittest.main()
