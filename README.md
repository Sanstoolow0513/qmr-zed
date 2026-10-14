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

All 16 faces from `MapleMono-NF-CN-unhinted.zip` are bundled as **Maple Mono NF CN**.
The interface and editor use this family by default, and terminals inherit the
editor font unless overridden. Custom font settings remain supported. The
original [font license](assets/fonts/maple-mono-nf-cn/LICENSE.txt) is included.

### QMR Zed Windows distribution

Build the independent Windows x64 release with PowerShell 7, the MSVC Rust
toolchain, and Visual Studio C++ and CMake tools:

```powershell
pwsh -File script/bundle-qmr-windows.ps1
```

The script builds optimized Release binaries and writes an extract-and-run ZIP,
the corresponding source ZIP, and SHA-256 checksums to `target/qmr-dist`. Run
`QMRZed.exe`, or use `bin/qmrzed.exe` from a terminal. The package is unsigned.
Settings live in `%APPDATA%\QMRZed`, and data/logs in `%LOCALAPPDATA%\QMRZed`,
separate from official Zed. Updates are manual from this fork's
[releases](https://github.com/Sanstoolow0513/qmr-zed/releases); official Zed
automatic updates are disabled. The build manifest records the base commit and
any uncommitted source changes. macOS and Linux packaging still uses upstream
templates and has not been adapted for this distribution.

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
