"""Release safety checks; no network, credentials or compiler required."""

import hashlib
from pathlib import Path
import stat
import tarfile
import tempfile
import unittest
import zipfile

import release


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "android/app").mkdir(parents=True)
        (self.root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.2.1"\n')
        self.gradle = self.root / "android/app/build.gradle.kts"
        self.gradle.write_text('versionName = "0.2.1"\nversionCode = 3\n')
        (self.root / "LICENSE").write_text("GPL-3.0-only test fixture")
        self.assets = self.root / "dist"

    def test_cargo_version_parser_reads_only_workspace_section(self):
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nresolver = "3"\n\n[workspace.package]\nversion = "0.2.1"\nlicense = "GPL-3.0-only"\n\n'
            '[workspace.dependencies]\nanyhow = "1"\nserde = { version = "1", features = ["derive"] }\n'
        )
        self.assertEqual(release.cargo_workspace_version((self.root / "Cargo.toml").read_text()), "0.2.1")
        with self.assertRaises(ValueError):
            release.cargo_workspace_version('[package]\nversion = "9.9.9"\n')

    def test_matching_versions(self):
        self.assertEqual(release.check_versions("v0.2.1", self.root), ("0.2.1", 3))

    def test_invalid_or_unstable_tag_is_rejected(self):
        for tag in ["0.2.1", "v00.2.1", "v0.2.1-rc1", "v0.2.1/../key", "v0.2.1\n"]:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                release.check_versions(tag, self.root)

    def test_mismatched_cargo_and_android_versions_are_rejected(self):
        with self.assertRaises(ValueError):
            release.check_versions("v0.2.2", self.root)
        self.gradle.write_text('versionName = "0.1.0"\nversionCode = 3\n')
        with self.assertRaises(ValueError):
            release.check_versions("v0.2.1", self.root)

    def test_invalid_or_ambiguous_android_version_is_rejected(self):
        for config in [
            'versionName = "0.2.1"\nversionCode = 0\n',
            'versionName = "0.2.1"\n',
            'versionName = "0.2.1"\nversionName = "0.2.1"\nversionCode = 3\n',
        ]:
            with self.subTest(config=config), self.assertRaises(ValueError):
                self.gradle.write_text(config)
                release.check_versions("v0.2.1", self.root)

    def binary(self, target):
        name = "anime-levisione.exe" if target.endswith("windows-msvc") else "anime-levisione"
        path = self.root / "target" / target / "release" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"synthetic executable")
        path.chmod(0o755)
        return name

    def test_unix_archive_contains_executable_and_license(self):
        target = release.TARGETS[0]
        name = self.binary(target)
        archive = release.package_tui("0.2.1", target, self.assets, self.root)
        with tarfile.open(archive) as bundle:
            members = {Path(item.name).name: item for item in bundle.getmembers() if item.isfile()}
            self.assertEqual(set(members), {name, "LICENSE", "README.txt"})
            self.assertTrue(members[name].mode & stat.S_IXUSR)
            self.assertEqual(bundle.extractfile(members[name]).read(), b"synthetic executable")

    def test_windows_archive_contains_exe_license_and_dependency_notice(self):
        target = release.TARGETS[-1]
        name = self.binary(target)
        archive = release.package_tui("0.2.1", target, self.assets, self.root)
        with zipfile.ZipFile(archive) as bundle:
            self.assertEqual({Path(item).name for item in bundle.namelist()}, {name, "LICENSE", "README.txt"})
            readme = next(item for item in bundle.namelist() if item.endswith("/README.txt"))
            instructions = bundle.read(readme).decode()
            self.assertIn("VLC and mpv are not bundled", instructions)
            self.assertIn("--player /path/to/player", instructions)

    def test_packager_rejects_missing_binary_unknown_target_and_overwrite(self):
        target = release.TARGETS[0]
        with self.assertRaises(ValueError):
            release.package_tui("0.2.1", target, self.assets, self.root)
        with self.assertRaises(ValueError):
            release.archive_name("../private", target)
        with self.assertRaises(ValueError):
            release.archive_name("0.2.1", "unexpected-platform")
        self.binary(target)
        release.package_tui("0.2.1", target, self.assets, self.root)
        with self.assertRaises(ValueError):
            release.package_tui("0.2.1", target, self.assets, self.root)

    def complete_assets(self):
        self.assets.mkdir()
        for target in release.TARGETS:
            (self.assets / release.archive_name("0.2.1", target)).write_bytes(b"archive")
        for extension in ["apk", "aab"]:
            (self.assets / f"anime-levisione-0.2.1-android.{extension}").write_bytes(b"android")
        (self.assets / "android-signing-certificate.txt").write_text("a" * 64 + "\n")

    def test_checksums_are_correct_and_repeatable(self):
        self.complete_assets()
        manifest = release.write_checksums("0.2.1", self.assets)
        first = manifest.read_bytes()
        for line in first.decode().splitlines():
            digest, name = line.split("  ")
            self.assertEqual(digest, hashlib.sha256((self.assets / name).read_bytes()).hexdigest())
        release.write_checksums("0.2.1", self.assets)
        self.assertEqual(manifest.read_bytes(), first)

    def test_missing_unexpected_or_invalid_assets_block_publication(self):
        self.complete_assets()
        secret = self.assets / "private-key.jks"
        secret.write_bytes(b"must never be published")
        with self.assertRaises(ValueError):
            release.write_checksums("0.2.1", self.assets)
        secret.unlink()
        certificate = self.assets / "android-signing-certificate.txt"
        certificate.write_text("not a certificate digest")
        with self.assertRaises(ValueError):
            release.write_checksums("0.2.1", self.assets)
        certificate.unlink()
        with self.assertRaises(ValueError):
            release.write_checksums("0.2.1", self.assets)


if __name__ == "__main__":
    unittest.main()
