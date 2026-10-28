> [!IMPORTANT]
> Remove this line to confirm you've reviewed this PR before submitting.
# QMR Zed

This qmr-zed fork focuses on software development without built-in AI. Projects
and terminal sessions use the independent [workspace sidebar](docs/src/workspace-terminals.md).
Editing, language servers, Git, debugging, collaboration and remote development
remain available.

QMR Zed defaults to Simplified Chinese. Switch between **简体中文** and **English**
in **Settings → General → Display Language** (设置 → 通用 → 显示语言), then restart
to apply the change. The JSON setting is `"display_language": "zh-CN"` or `"en"`.
Changing it in `settings.json` also shows a restart notification.

The regular, italic, bold, and bold italic faces of **Maple Mono NF CN** are bundled.
Other requested weights use the nearest available face. Release builds compress
these four faces together without removing Chinese glyphs, Nerd Font icons, or ligatures.
The interface and editor use this family by default, and terminals inherit the
editor font unless overridden. Custom font settings remain supported. The
original [font license](assets/fonts/maple-mono-nf-cn/LICENSE.txt) is included.

### QMR Zed releases

Release assets use these names and package types:

- Windows: `QMRZed-x86_64.exe` and `QMRZed-aarch64.exe` (Inno Setup installers)
- macOS: `Zed-x86_64.dmg` and `Zed-aarch64.dmg`
- Linux: `zed-linux-x86_64.tar.gz` and `zed-linux-aarch64.tar.gz`
- Linux sandbox: `bwrap-linux-x86_64.gz` and `bwrap-linux-aarch64.gz`
- Remote servers for Linux, macOS, and Windows on x86_64 and aarch64

The Windows editor is installed as `QMRZed.exe`, with the `qmrzed` command in
its `bin` directory. The installed application uses its own install location, file
associations, and `%APPDATA%\QMRZed` / `%LOCALAPPDATA%\QMRZed` data directories,
separate from official Zed. Automatic updates to official Zed builds are
disabled. Download updates from this fork's
[releases](https://github.com/Sanstoolow0513/qmr-zed/releases).

These packages are unsigned: the fork does not have Apple or Azure signing
certificates. Windows SmartScreen and macOS Gatekeeper show a warning. The
Windows installer enables "Open with QMR Zed" for files, folders, folder
backgrounds, and drives by default. On Windows 11, these entries are available
under **Show more options** even when the unsigned Explorer package cannot
register. File and folder context menus can be disabled separately during setup.

Build one platform locally with the upstream bundlers. Set
`ZED_RELEASE_CHANNEL` to `stable`, `preview`, `nightly`, or `dev` first; when
it is unset, the scripts use `crates/zed/RELEASE_CHANNEL`.

```powershell
$env:ZED_RELEASE_CHANNEL = 'stable'
pwsh -File script/bundle-windows.ps1
```

```bash
export ZED_RELEASE_CHANNEL=stable
./script/bundle-mac
./script/bundle-linux --skip-sentry
```

Windows needs PowerShell 7, the MSVC Rust toolchain, Visual Studio C++ and
CMake, the Windows SDK, and Inno Setup 6. macOS needs Xcode. Linux needs the
packages installed by `script/linux`, including LLD 12 or newer.

SSH, WSL, and dev-container connections download matching remote servers from
this fork's GitHub Release. Release tags use `v` followed by the version in
`crates/zed/Cargo.toml` (for example, `v2026.10.15`). The **QMR Zed release**
workflow builds every client package and remote server from the same commit,
then the static `bwrap` binaries. It publishes only after all of those assets
pass validation. Linux servers are static musl binaries. A release version and
its tag must never be reused for different source code.

To release, commit the reviewed changes, create the matching tag on this fork,
and push that tag to `origin`. The workflow also supports a manual run against
an existing tag. It requires no upstream credentials or private runners. A
locally built client needs the matching fork Release before a first remote
connection can download its server. The client reports a missing release
instead of falling back to an unrelated official server.

[![Zed](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/zed-industries/zed/main/assets/badge/v0.json)](https://zed.dev)
[![CI](https://github.com/zed-industries/zed/actions/workflows/run_tests.yml/badge.svg)](https://github.com/zed-industries/zed/actions/workflows/run_tests.yml)

QMR Zed is based on Zed, a high-performance, multiplayer code editor from the creators of [Atom](https://github.com/atom/atom) and [Tree-sitter](https://github.com/tree-sitter/tree-sitter).

---

### Upstream Zed installation

On macOS, Linux, and Windows you can [download Zed directly](https://zed.dev/download) or install Zed via your local package manager ([macOS](https://zed.dev/docs/installation#macos)/[Linux](https://zed.dev/docs/linux#installing-via-a-package-manager)/[Windows](https://zed.dev/docs/windows#package-managers)).

Other platforms are not yet available:

- Web ([tracking discussion](https://github.com/zed-industries/zed/discussions/26195))

### Developing Zed

- [Building Zed for macOS](./docs/src/development/macos.md)
- [Building Zed for Linux](./docs/src/development/linux.md)
- [Building Zed for Windows](./docs/src/development/windows.md)

### Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md) for ways you can contribute to Zed.

Also... we're hiring! Check out our [jobs](https://zed.dev/jobs) page for open roles.

### Licensing

Zed source code is licensed primarily under GPL-3.0-or-later, with Apache-2.0 components where marked.

License information for third party dependencies must be correctly provided for CI to pass.

We use [`cargo-about`](https://github.com/EmbarkStudios/cargo-about) to automatically comply with open source licenses. If CI is failing, check the following:

- Is it showing a `no license specified` error for a crate you've created? If so, add `publish = false` under `[package]` in your crate's Cargo.toml.
- Is the error `failed to satisfy license requirements` for a dependency? If so, first determine what license the project has and whether this system is sufficient to comply with this license's requirements. If you're unsure, ask a lawyer. Once you've verified that this system is acceptable add the license's SPDX identifier to the `accepted` array in `script/licenses/zed-licenses.toml`.
- Is `cargo-about` unable to find the license for a dependency? If so, add a clarification field at the end of `script/licenses/zed-licenses.toml`, as specified in the [cargo-about book](https://embarkstudios.github.io/cargo-about/cli/generate/config.html#crate-configuration).

## Sponsorship

Zed is developed by **Zed Industries, Inc.**, a for-profit company.

If you’d like to financially support the project, you can do so via GitHub Sponsors.
Sponsorships go directly to Zed Industries and are used as general company revenue.
There are no perks or entitlements associated with sponsorship.
