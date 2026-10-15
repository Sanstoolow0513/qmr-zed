# Workspace Terminals

Use the workspace sidebar to keep projects and terminal sessions in one window.
Open it with {#action multi_workspace::ToggleWorkspaceSidebar}. Select a project
or terminal to switch to it. The **+** button beside a project creates a terminal
in that project. **Add Project**, on the right of the sidebar's search field,
opens the recent-project picker. The toggle on the left of the search field
hides the sidebar. When it is hidden, the toggle stays in the window's title bar
on the same side.

The workspace terminal panel fills the central area when no files or other
content are open. Opening a file restores the terminal's saved dock width and
the editor beside it. The hide button in the terminal header keeps its process
running; selecting the terminal in the sidebar shows it again. The close button
ends the session.

The regular [Terminal Panel](./terminal.md) remains available for tasks and its
existing terminal sessions through the tools menu. Click a workspace terminal's
title to rename it; use
{#action workspace_terminal::ToggleSearch} to search its output.

{#action workspace_sidebar::ToggleTerminalSwitcher} switches between terminal
sessions. Closing a terminal does not delete its Git worktree or project files.
Switching projects keeps open terminal sessions alive. After restarting the
application, restoring a saved terminal starts a new shell using its recorded
working directory and title; it does not resume the old process.

## Settings

Open the Settings Editor and select **Workspace Terminals**. You can change the
sidebar position, width and automatic opening, and the terminal panel's position,
size, startup command and bell notifications.

Or add this to your settings.json:

```json
{
  "workspace_sidebar": {
    "auto_open": true,
    "position": "left",
    "default_width": 300
  },
  "workspace_terminal": {
    "dock": "left",
    "flexible": true,
    "default_width": 640,
    "init_command": "",
    "notify_on_bell": "primary_screen",
    "play_sound_on_bell": "never"
  }
}
```

The sidebar width is limited to 200–800 pixels. The panel's fixed width is at
least 300 pixels. An empty startup command starts only the configured shell.
Remote terminal sessions continue to use their project's remote connection.

## Upgrading from the Agent Panel

qmr-zed migrates the former `agent.threads_sidebar` settings and terminal-related
panel settings to `workspace_sidebar` and `workspace_terminal`. Explicit new
settings take precedence. Saved terminal identifiers, titles, paths and panel
selection remain compatible with the previous version.

The window no longer has a bottom status bar. Project tools appear above the
right-hand panel, or in the title bar's tools menu when that panel is hidden.
The editor toolbar shows the current line and column and the file's language
beside the existing editor controls. Click the position to jump to a line, or
the language to change the file's language mode. Encoding and line endings are
available in the editor settings menu. Existing `status_bar` settings for the
position and language buttons still control those buttons; the old global
status bar visibility setting no longer creates a bottom bar.

qmr-zed does not include Agents, inline assistants, AI edit predictions, model
providers or MCP servers. Old AI settings cannot enable them. Existing Agent
conversation files are left on disk and are not loaded. Programs you choose to
run in a terminal are unaffected.

See [Windows & Projects](./windows-and-projects.md) for project management and
[Terminal](./terminal.md) for shell configuration.
