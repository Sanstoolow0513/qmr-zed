---
title: Getting Started with Zed
description: Get started with Zed, the fast open-source code editor. Essential commands, environment setup, and navigation basics.
---

# Getting Started

qmr-zed is an open-source code editor with collaboration, Git, debugging and terminal tools. Built-in AI features are not included.

This guide covers the essential commands, environment setup, and navigation basics.

## Quick Start

### Welcome Page

When you open Zed without a folder, you see the welcome page in the main editor area. The welcome page offers quick actions to open a folder, clone a repository, or view documentation. Once you open a folder or file, the welcome page disappears. If you split the editor into multiple panes, the welcome page appears only in the center pane when empty—other panes show a standard empty state.

To reopen the welcome page, close all items in the center pane or use the command palette to search for "Welcome".

### 1. Open a Project

Open a folder from the command line:

```sh
zed ~/projects/my-app
```

Or use `Cmd+O` (macOS) / `Ctrl+O` (Linux/Windows) to open a folder from within Zed.

By default, new projects open in your current window's workspace sidebar. To open in a new window instead, use `zed -n ~/projects/my-app` or press `Cmd+Enter` when selecting from Open Recent. See [Windows & Projects](./windows-and-projects.md) for more details.

### 2. Learn the Essential Commands

| Action          | macOS         | Linux/Windows  |
| --------------- | ------------- | -------------- |
| Command palette | `Cmd+Shift+P` | `Ctrl+Shift+P` |
| Go to file      | `Cmd+P`       | `Ctrl+P`       |
| Go to symbol    | `Cmd+Shift+O` | `Ctrl+Shift+O` |
| Find in project | `Cmd+Shift+F` | `Ctrl+Shift+F` |
| Toggle terminal | `` Ctrl+` ``  | `` Ctrl+` ``   |
| Open settings   | `Cmd+,`       | `Ctrl+,`       |

The [command palette](./command-palette.md) (`Cmd+Shift+P`) is your gateway to every action in Zed. If you forget a shortcut, search for it there.

### Panel Layout

Use the [Workspace Terminals](./workspace-terminals.md) settings to place the workspace sidebar and terminal panel on either side of the editor.

### 3. Configure Your Editor

Open the Settings Editor with `Cmd+,` (macOS) or `Ctrl+,` (Linux/Windows). Search for any setting and change it directly.

Common first changes:

- **Theme**: Press `Cmd+K Cmd+T` (macOS) or `Ctrl+K Ctrl+T` (Linux/Windows) to open the theme selector
- **Font**: Search for [`buffer_font_family`](./reference/all-settings.md#buffer-font-family) in Settings
- **Format on save**: Search for `format_on_save` and set to `on`

### 4. Set Up Your Language

Zed includes built-in support for many languages. For others, install the extension:

1. Open Extensions with `Cmd+Shift+X` (macOS) or `Ctrl+Shift+X` (Linux/Windows)
2. Search for your language
3. Click Install

See [Languages](./languages.md) for language-specific setup instructions.

### 5. Open Workspace Terminals

Use the **+** beside a project in the workspace sidebar to create a terminal. Switch between projects and terminals without losing live shell sessions. See [Workspace Terminals](./workspace-terminals.md) for settings and shortcuts.

## Coming from Another Editor?

We have dedicated guides for switching from other editors:

- [VS Code](./migrate/vs-code.md) — Import settings, map keybindings, find equivalent features
- [IntelliJ IDEA](./migrate/intellij.md) — Adapt to Zed's approach to navigation and refactoring
- [PyCharm](./migrate/pycharm.md) — Set up Python development in Zed
- [WebStorm](./migrate/webstorm.md) — Configure JavaScript/TypeScript workflows
- [RustRover](./migrate/rustrover.md) — Rust development in Zed

You can also enable familiar keybindings:

- **Vim**: Enable `vim_mode` in settings. See [Vim Mode](./vim.md).
- **Helix**: Enable `helix_mode` in settings. See [Helix Mode](./helix.md).

## Join the Community

Zed is open source. Join us on GitHub or in Discord to contribute code, report bugs, or suggest features.

- [Discord](https://discord.com/invite/zedindustries)
- [GitHub Discussions](https://github.com/zed-industries/zed/discussions)
- [Zed Reddit](https://www.reddit.com/r/ZedEditor)
