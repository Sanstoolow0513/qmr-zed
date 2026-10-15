#!/usr/bin/env python3

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tomllib
import zipfile


REPOSITORY = "Sanstoolow0513/qmr-zed"
TARGETS = {
    "x86_64-unknown-linux-musl": ("linux", "x86_64", "gz"),
    "aarch64-unknown-linux-musl": ("linux", "aarch64", "gz"),
    "x86_64-apple-darwin": ("macos", "x86_64", "gz"),
    "aarch64-apple-darwin": ("macos", "aarch64", "gz"),
    "x86_64-pc-windows-msvc": ("windows", "x86_64", "zip"),
    "aarch64-pc-windows-msvc": ("windows", "aarch64", "zip"),
}


def run(*arguments):
    return subprocess.check_output(arguments, text=True).strip()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha256(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def release_identity(tag, commit=None):
    with Path("crates/zed/Cargo.toml").open("rb") as source:
        version = tomllib.load(source)["package"]["version"]
    require(tag == f"v{version}", f"Tag {tag!r} must equal v{version}")
    require("+" not in version, "Release versions must not contain build metadata")
    source_commit = run("git", "rev-parse", "HEAD")
    tag_commit = run("git", "rev-parse", f"refs/tags/{tag}^{{commit}}")
    require(source_commit == tag_commit, "Checkout must point to the release tag")
    if commit is not None:
        require(source_commit == commit, "Checkout does not match the release source commit")
    require(not run("git", "status", "--porcelain", "--untracked-files=no"),
            "Release builds require a clean tracked source tree")
    return version, source_commit


def asset_name(target):
    system, architecture, extension = TARGETS[target]
    return f"zed-remote-server-{system}-{architecture}.{extension}"


def package_server(arguments):
    version, commit = release_identity(arguments.tag, arguments.commit)
    system, _, extension = TARGETS[arguments.target]
    binary_name = "remote_server.exe" if system == "windows" else "remote_server"
    binary = Path("target") / arguments.target / "release" / binary_name
    require(binary.is_file(), f"Missing server binary: {binary}")
    binary_version = run(str(binary.resolve()), "version")
    require(binary_version == version,
            f"Server version {binary_version!r} does not match release {version}")
    if system == "linux":
        require("INTERP" not in run("readelf", "--program-headers", str(binary)),
                "Linux server must not require a dynamic loader")
        require("(NEEDED)" not in run("readelf", "--dynamic", str(binary)),
                "Linux server must be statically linked")

    destination = arguments.directory
    destination.mkdir(parents=True, exist_ok=True)
    archive_path = destination / asset_name(arguments.target)
    if extension == "zip":
        with zipfile.ZipFile(archive_path, "w", zipfile.ZIP_DEFLATED) as archive:
            archive.write(binary, "remote_server.exe")
    else:
        with binary.open("rb") as source, archive_path.open("wb") as output:
            with gzip.GzipFile(filename="", fileobj=output, mode="wb", mtime=0) as archive:
                shutil.copyfileobj(source, archive)
    metadata = {
        "repository": REPOSITORY,
        "version": version,
        "commit": commit,
        "target": arguments.target,
        "asset": archive_path.name,
        "sha256": sha256(archive_path),
    }
    archive_path.with_name(archive_path.name + ".json").write_text(
        json.dumps(metadata, indent=2) + "\n", encoding="utf-8"
    )
    print(archive_path)


def validate_assets(directory, version, commit):
    expected_files = []
    for target in TARGETS:
        archive_path = directory / asset_name(target)
        metadata_path = archive_path.with_name(archive_path.name + ".json")
        metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
        for key, expected in {
            "repository": REPOSITORY,
            "version": version,
            "commit": commit,
            "target": target,
            "asset": archive_path.name,
            "sha256": sha256(archive_path),
        }.items():
            require(metadata.get(key) == expected, f"{archive_path.name}: mismatched {key}")
        if archive_path.suffix == ".zip":
            with zipfile.ZipFile(archive_path) as archive:
                require(archive.namelist() == ["remote_server.exe"],
                        f"{archive_path.name}: expected remote_server.exe at ZIP root")
                require(archive.testzip() is None, f"Corrupt ZIP: {archive_path}")
                require(archive.getinfo("remote_server.exe").file_size > 0,
                        f"Empty binary: {archive_path}")
        else:
            with gzip.open(archive_path, "rb") as archive:
                require(bool(archive.read(1)), f"Empty binary: {archive_path}")
                while archive.read(1024 * 1024):
                    pass
        expected_files.extend([archive_path, metadata_path])

    package_name = f"qmr-zed-{version}-windows-x86_64"
    client_path = directory / f"{package_name}.zip"
    source_path = directory / f"qmr-zed-{version}-source.zip"
    with zipfile.ZipFile(client_path) as archive:
        for relative_path in ["QMRZed.exe", "bin/qmrzed.exe", "cli.exe", "conpty.dll", "x64/OpenConsole.exe"]:
            member = f"{package_name}/{relative_path}"
            require(member in archive.namelist(), f"Client package: missing {relative_path}")
            require(archive.getinfo(member).file_size > 0, f"Client package: empty {relative_path}")
        metadata = json.loads(archive.read(f"{package_name}/build-info.json"))
        for key, expected in {
            "repository": f"https://github.com/{REPOSITORY}",
            "version": version,
            "base_commit": commit,
            "platform": "x86_64-pc-windows-msvc",
            "working_tree_changes": [],
            "source_archive": source_path.name,
            "source_sha256": sha256(source_path),
            "remote_server_release": f"https://github.com/{REPOSITORY}/releases/tag/v{version}",
        }.items():
            require(metadata.get(key) == expected, f"Client package: mismatched {key}")
        require(archive.testzip() is None, "Corrupt client archive")
    with zipfile.ZipFile(source_path) as archive:
        require(archive.testzip() is None, "Corrupt source archive")
        manifest = tomllib.loads(archive.read("crates/zed/Cargo.toml").decode("utf-8"))
        require(manifest["package"]["version"] == version, "Source version does not match")
    expected_files.extend([client_path, source_path])
    (directory / "SHA256SUMS.txt").write_text(
        "".join(f"{sha256(path)}  {path.name}\n" for path in sorted(expected_files)),
        encoding="ascii",
    )
    return expected_files


def main():
    parser = argparse.ArgumentParser(description="Package and validate QMR Zed release artifacts")
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ["check-tag", "package-server", "verify-assets"]:
        command = commands.add_parser(name)
        command.add_argument("--tag", required=True)
        command.add_argument("--commit")
        if name != "check-tag":
            command.add_argument("--directory", type=Path, default=Path("target/qmr-release"))
        if name == "package-server":
            command.add_argument("--target", choices=TARGETS, required=True)
    arguments = parser.parse_args()
    if arguments.command == "package-server":
        package_server(arguments)
        return
    version, commit = release_identity(arguments.tag, arguments.commit)
    if arguments.command == "verify-assets":
        files = validate_assets(arguments.directory, version, commit)
        print(f"Verified {len(files)} artifacts for {arguments.tag} at {commit}")
    elif "GITHUB_OUTPUT" in os.environ:
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
            output.write(f"version={version}\ncommit={commit}\ntag={arguments.tag}\n")
    else:
        print(f"{arguments.tag} {commit}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError, zipfile.BadZipFile) as error:
        print(f"Release validation failed: {error}", file=sys.stderr)
        sys.exit(1)
