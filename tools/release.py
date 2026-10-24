#!/usr/bin/env python3
"""Version checks, portable TUI archives and the GitHub Release checksum manifest."""

import argparse
import hashlib
from pathlib import Path
import re
import shutil
import tarfile
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = "https://github.com/k33wee/anime-levisione"
VERSION = r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
)


def check_versions(tag, root=ROOT):
    if not re.fullmatch("v" + VERSION, tag):
        raise ValueError("A stable release tag must have the form vMAJOR.MINOR.PATCH")
    version = tag[1:]
    with (root / "Cargo.toml").open("rb") as source:
        cargo_version = tomllib.load(source)["workspace"]["package"]["version"]
    gradle = (root / "android/app/build.gradle.kts").read_text(encoding="utf-8")
    names = re.findall(r'^\s*versionName\s*=\s*"([^"\r\n]+)"\s*$', gradle, re.MULTILINE)
    codes = re.findall(r"^\s*versionCode\s*=\s*([0-9]+)\s*$", gradle, re.MULTILINE)
    if cargo_version != version or names != [version]:
        raise ValueError(f"Tag {tag}, Cargo version and Android versionName must agree")
    if len(codes) != 1 or int(codes[0]) <= 0:
        raise ValueError("Android must have a positive integer versionCode")
    return version, int(codes[0])


def archive_name(version, target):
    if not re.fullmatch(VERSION, version) or target not in TARGETS:
        raise ValueError("Invalid release version or unsupported target")
    extension = ".zip" if target.endswith("windows-msvc") else ".tar.gz"
    return f"anime-levisione-{version}-{target}{extension}"


def package_tui(version, target, output, root=ROOT):
    check_versions("v" + version, root)
    filename = archive_name(version, target)
    executable = "anime-levisione.exe" if target.endswith("windows-msvc") else "anime-levisione"
    binary = root / "target" / target / "release" / executable
    if not binary.is_file():
        raise ValueError(f"Build the {target} TUI binary before packaging")
    output.mkdir(parents=True, exist_ok=True)
    archive = output / filename
    if archive.exists():
        raise ValueError(f"Refusing to overwrite {archive}")
    with tempfile.TemporaryDirectory(prefix="anime-levisione-package-") as temporary:
        package = Path(temporary) / f"anime-levisione-{version}-{target}"
        package.mkdir()
        shutil.copy2(binary, package / executable)
        shutil.copy2(root / "LICENSE", package / "LICENSE")
        (package / "README.txt").write_text(
            f"anime-levisione {version} — {target}\n\n"
            "Extract this directory and put the executable on your PATH.\n"
            "Install VLC or mpv for playback; ffmpeg is optional for downloads.\n"
            "The Linux build requires glibc 2.35 or newer.\n\n"
            f"Documentation: {REPOSITORY}/tree/v{version}#tui\n"
            f"Source code: {REPOSITORY}/archive/refs/tags/v{version}.tar.gz\n"
            "License: GPL-3.0-only (included in LICENSE).\n",
            encoding="utf-8",
        )
        if filename.endswith(".zip"):
            with zipfile.ZipFile(archive, "x", compression=zipfile.ZIP_DEFLATED) as bundle:
                for file in sorted(package.iterdir()):
                    bundle.write(file, arcname=f"{package.name}/{file.name}")
        else:
            with tarfile.open(archive, "x:gz") as bundle:
                bundle.add(package, arcname=package.name)
    return archive


def write_checksums(version, directory):
    expected = {archive_name(version, target) for target in TARGETS}
    expected.update({
        f"anime-levisione-{version}-android.apk",
        f"anime-levisione-{version}-android.aab",
        "android-signing-certificate.txt",
    })
    files = {path.name for path in directory.iterdir() if path.name != "SHA256SUMS"}
    if files != expected or any(not (directory / name).is_file() for name in files):
        raise ValueError(f"Release assets differ: missing={sorted(expected - files)}, unexpected={sorted(files - expected)}")
    certificate = (directory / "android-signing-certificate.txt").read_text().strip()
    if not re.fullmatch(r"[0-9a-f]{64}", certificate):
        raise ValueError("Expected the verified Android certificate SHA-256 fingerprint")
    manifest = directory / "SHA256SUMS"
    with manifest.open("w", encoding="utf-8", newline="\n") as output:
        for name in sorted(expected):
            with (directory / name).open("rb") as source:
                digest = hashlib.file_digest(source, "sha256").hexdigest()
            output.write(f"{digest}  {name}\n")
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    check = commands.add_parser("check")
    check.add_argument("tag")
    package = commands.add_parser("package")
    package.add_argument("version")
    package.add_argument("target", choices=TARGETS)
    package.add_argument("--output", type=Path, default=Path("dist"))
    checksums = commands.add_parser("checksums")
    checksums.add_argument("version")
    checksums.add_argument("--directory", type=Path, default=Path("dist"))
    args = parser.parse_args()
    try:
        if args.command == "check":
            version, code = check_versions(args.tag)
            print(f"version={version}\nversion_code={code}")
        elif args.command == "package":
            print(package_tui(args.version, args.target, args.output))
        else:
            print(write_checksums(args.version, args.directory))
    except (ValueError, OSError) as error:
        parser.exit(1, f"Release validation failed: {error}\n")


if __name__ == "__main__":
    main()
