"""Check the offline documentation inputs without image libraries or network."""

import json
from pathlib import Path
import re
import struct
import unittest

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "docs/screenshots/fixtures"


class ScreenshotFixturesTests(unittest.TestCase):
    def test_catalog_has_eight_unique_local_records(self):
        catalog = json.loads((FIXTURES / "catalog.json").read_text())
        self.assertEqual(len(catalog), 8)
        self.assertEqual(len({row["id"] for row in catalog}), 8)
        self.assertEqual(len({row["title"] for row in catalog}), 8)
        self.assertEqual(
            {path.name for path in FIXTURES.glob("*.png")},
            {f"{row['id']}.png" for row in catalog},
        )
        for row in catalog:
            with self.subTest(title=row["title"]):
                self.assertRegex(row["id"], r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
                self.assertTrue(row["title"] and row["synopsis"] and row["genres"])
                self.assertGreaterEqual(row["episodes"], 5)
                self.assertIsInstance(row["dub"], bool)
                self.assertEqual(len(row["palette"]), 3)
                for color in row["palette"]:
                    self.assertIsNotNone(re.fullmatch(r"#[0-9A-F]{6}", color))
                self.assertIn(row["motif"], ["orbit", "kite", "tower"])
                self.assertNotIn("http", json.dumps(row).lower())
                self.assert_png(FIXTURES / f"{row['id']}.png", (480, 706))

    def assert_png(self, path, size):
        data = path.read_bytes()
        self.assertEqual(data[:8], b"\x89PNG\r\n\x1a\n")
        self.assertEqual(struct.unpack(">II", data[16:24]), size)

    def test_all_documentation_images_have_been_regenerated(self):
        directory = ROOT / "docs/screenshots"
        names = {"library.png", "audio-picker.png"} | {
            f"android-{name}.png" for name in [
                "home", "detail", "player-audio", "player-controls", "fullscreen", "settings"
            ]
        }
        self.assertEqual({path.name for path in directory.glob("*.png")}, names)
        for name in names:
            expected = (1200, 675)
            if name == "android-fullscreen.png":
                expected = (960, 432)
            elif name.startswith("android-"):
                expected = (540, 1200)
            self.assert_png(directory / name, expected)


if __name__ == "__main__":
    unittest.main()
