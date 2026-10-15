#!/usr/bin/env python3

import gzip
import importlib.util
import io
import json
from pathlib import Path
import tarfile
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
        self.write_clients()

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

    def write_clients(self, missing=None):
        for name in release.CLIENT_ASSETS:
            path = self.directory / name
            if name == missing:
                path.unlink(missing_ok=True)
                continue
            if name.endswith(".exe"):
                path.write_bytes(b"MZ" + b"installer" * 64)
            elif name.endswith(".dmg"):
                path.write_bytes(b"dmg" * 200)
            elif name.startswith("zed-linux-"):
                buffer = io.BytesIO()
                with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
                    payload = b"editor"
                    info = tarfile.TarInfo("zed.app/libexec/zed-editor")
                    info.size = len(payload)
                    archive.addfile(info, io.BytesIO(payload))
                path.write_bytes(buffer.getvalue())
            else:
                path.write_bytes(gzip.compress(b"bwrap"))

    def validate(self):
        return release.validate_assets(self.directory, self.version, self.commit)

    def test_complete_matching_release(self):
        files = self.validate()
        self.assertEqual(len(files), 20)
        checksums = (self.directory / "SHA256SUMS.txt").read_text().splitlines()
        self.assertEqual(len(checksums), 20)
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

    def test_missing_client_asset_prevents_release(self):
        self.write_clients(missing="Zed-x86_64.exe")
        with self.assertRaisesRegex(ValueError, "Missing client asset: Zed-x86_64.exe"):
            self.validate()

    def test_invalid_windows_installer_prevents_release(self):
        (self.directory / "Zed-aarch64.exe").write_bytes(b"not an executable")
        with self.assertRaisesRegex(ValueError, "not a Windows executable"):
            self.validate()

    def test_linux_archive_requires_editor(self):
        path = self.directory / "zed-linux-x86_64.tar.gz"
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
            payload = b"cli"
            info = tarfile.TarInfo("zed.app/bin/zed")
            info.size = len(payload)
            archive.addfile(info, io.BytesIO(payload))
        path.write_bytes(buffer.getvalue())
        with self.assertRaisesRegex(ValueError, "missing libexec/zed-editor"):
            self.validate()


if __name__ == "__main__":
    unittest.main()
