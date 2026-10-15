#!/usr/bin/env python3

import gzip
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import zipfile


spec = importlib.util.spec_from_file_location("qmr_release", Path(__file__).with_name("qmr-release.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseValidationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.version = "2026.10.13"
        self.commit = "a" * 40
        for target in release.TARGETS:
            archive_path = self.directory / release.asset_name(target)
            if archive_path.suffix == ".zip":
                with zipfile.ZipFile(archive_path, "w") as archive:
                    archive.writestr("remote_server.exe", b"server")
            else:
                with gzip.open(archive_path, "wb") as archive:
                    archive.write(b"server")
            self.write_server_metadata(target)
        self.source_path = self.directory / f"qmr-zed-{self.version}-source.zip"
        with zipfile.ZipFile(self.source_path, "w") as archive:
            archive.writestr("crates/zed/Cargo.toml", f'[package]\nversion = "{self.version}"\n')
        self.client_metadata = {
            "repository": f"https://github.com/{release.REPOSITORY}",
            "version": self.version,
            "base_commit": self.commit,
            "platform": "x86_64-pc-windows-msvc",
            "working_tree_changes": [],
            "source_archive": self.source_path.name,
            "source_sha256": release.sha256(self.source_path),
            "remote_server_release": f"https://github.com/{release.REPOSITORY}/releases/tag/v{self.version}",
        }
        self.write_client()

    def write_server_metadata(self, target, **overrides):
        archive_path = self.directory / release.asset_name(target)
        metadata = {
            "repository": release.REPOSITORY,
            "version": self.version,
            "commit": self.commit,
            "target": target,
            "asset": archive_path.name,
            "sha256": release.sha256(archive_path),
        }
        metadata.update(overrides)
        archive_path.with_name(archive_path.name + ".json").write_text(json.dumps(metadata))

    def write_client(self, missing_runtime=None):
        name = f"qmr-zed-{self.version}-windows-x86_64"
        with zipfile.ZipFile(self.directory / f"{name}.zip", "w") as archive:
            archive.writestr(f"{name}/build-info.json", json.dumps(self.client_metadata))
            for relative_path in ["QMRZed.exe", "bin/qmrzed.exe", "cli.exe", "conpty.dll", "x64/OpenConsole.exe"]:
                if relative_path != missing_runtime:
                    archive.writestr(f"{name}/{relative_path}", b"runtime")

    def validate(self):
        return release.validate_assets(self.directory, self.version, self.commit)

    def test_complete_matching_release(self):
        files = self.validate()
        self.assertEqual(len(files), 14)
        checksums = (self.directory / "SHA256SUMS.txt").read_text().splitlines()
        self.assertEqual(len(checksums), 14)
        for path in files:
            self.assertIn(f"{release.sha256(path)}  {path.name}", checksums)

    def test_missing_server_prevents_release(self):
        (self.directory / release.asset_name("aarch64-unknown-linux-musl")).unlink()
        with self.assertRaises(FileNotFoundError):
            self.validate()
        self.assertFalse((self.directory / "SHA256SUMS.txt").exists())

    def test_mixed_source_commits_prevent_release(self):
        self.write_server_metadata("x86_64-apple-darwin", commit="b" * 40)
        with self.assertRaisesRegex(ValueError, "mismatched commit"):
            self.validate()

    def test_tampered_server_prevents_release(self):
        archive = self.directory / release.asset_name("x86_64-unknown-linux-musl")
        archive.write_bytes(b"corrupt download")
        with self.assertRaisesRegex(ValueError, "mismatched sha256"):
            self.validate()

    def test_windows_archive_requires_root_executable(self):
        target = "aarch64-pc-windows-msvc"
        with zipfile.ZipFile(self.directory / release.asset_name(target), "w") as archive:
            archive.writestr("nested/remote_server.exe", b"server")
        self.write_server_metadata(target)
        with self.assertRaisesRegex(ValueError, "at ZIP root"):
            self.validate()

    def test_stale_or_dirty_client_prevents_release(self):
        for key, value in [("base_commit", "b" * 40), ("working_tree_changes", [" M source.rs"])]:
            with self.subTest(key=key):
                original = self.client_metadata[key]
                self.client_metadata[key] = value
                self.write_client()
                with self.assertRaisesRegex(ValueError, f"mismatched {key}"):
                    self.validate()
                self.client_metadata[key] = original

    def test_wrong_source_archive_prevents_release(self):
        self.source_path.write_bytes(b"wrong source")
        with self.assertRaisesRegex(ValueError, "mismatched source_sha256"):
            self.validate()

    def test_missing_client_runtime_prevents_release(self):
        self.write_client(missing_runtime="conpty.dll")
        with self.assertRaisesRegex(ValueError, "missing conpty.dll"):
            self.validate()


if __name__ == "__main__":
    unittest.main()
