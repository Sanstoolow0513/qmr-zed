use crate::{
    TerminalView,
    terminal_metadata_store::{
        TerminalMetadata, TerminalMetadataStore, compose_terminal_thread_title,
        normalize_terminal_custom_title, terminal_title_without_prefix,
    },
    terminal_notification::{TerminalNotification, TerminalNotificationEvent},
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use collections::HashMap;
use db::kvp::KeyValueStore;
use editor::Editor;
use fs::Fs;
use futures::FutureExt as _;
use gpui::{
    Action, App, AsyncWindowContext, Entity, EventEmitter, FocusHandle, Focusable, PlatformDisplay,
    Subscription, Task, TaskExt, WeakEntity, WindowHandle, prelude::*,
};
use project::Project;
use search::{BufferSearchBar, buffer_search::Deploy as DeployBufferSearch};
use serde::{Deserialize, Serialize};
use settings::{Settings, TerminalNotificationDisplay};
use std::{fmt, path::PathBuf, rc::Rc, sync::Arc, time::Duration};
use terminal::Event as TerminalEvent;
use ui::{Tooltip, prelude::*};
use util::ResultExt as _;
use workspace::{
    MultiWorkspace, ToolbarItemView, Workspace, WorkspaceId, WorkspaceTerminalSettings,
    dock::{DockPosition, Panel, PanelEvent},
    item::{ItemEvent, ItemHandle},
};

const PANEL_KEY: &str = "workspace_terminal";
const TERMINAL_INIT_COMMAND_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);

gpui::actions!(
    workspace_terminal,
    [
        /// Creates a terminal in the active workspace.
        NewTerminal,
        /// Toggles focus on the workspace terminal panel.
        ToggleFocus,
        /// Searches the active workspace terminal.
        ToggleSearch,
        /// Renames the active workspace terminal.
        RenameTerminal,
        /// Closes the active workspace terminal.
        CloseTerminal,
    ]
);

#[derive(Default, Serialize, Deserialize)]
struct SerializedPanel {
    #[serde(default)]
    last_active_terminal_id: Option<String>,
}

pub enum WorkspaceTerminalEvent {
    EntryChanged,
    ActiveViewChanged,
    ActiveViewFocused,
    TerminalCloseRequested { metadata: TerminalMetadata },
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TerminalId(uuid::Uuid);
impl TerminalId {
    pub(crate) fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    pub(crate) fn to_key_string(self) -> String {
        self.0.hyphenated().to_string()
    }

    pub fn from_key_string(key: &str) -> anyhow::Result<Self> {
        Ok(Self(uuid::Uuid::parse_str(key)?))
    }
}
impl fmt::Display for TerminalId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
#[derive(Clone, Debug)]
pub struct WorkspaceTerminalInfo {
    pub id: TerminalId,
    pub title: SharedString,
    pub created_at: DateTime<Utc>,
    pub has_notification: bool,
    pub custom_title: Option<SharedString>,
    pub working_directory: Option<PathBuf>,
}
struct WorkspaceTerminal {
    view: Entity<TerminalView>,
    title_editor: Option<Entity<Editor>>,
    title_editor_initial_title: Option<String>,
    title_editor_subscription: Option<Subscription>,
    last_known_title: String,
    last_known_terminal_title: String,
    working_directory: Option<PathBuf>,
    created_at: DateTime<Utc>,
    has_notification: bool,
    search_bar: Option<Entity<BufferSearchBar>>,
    notification_windows: Vec<WindowHandle<TerminalNotification>>,
    notification_subscriptions: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}
impl WorkspaceTerminal {
    fn terminal_title_for_view(view: &TerminalView, cx: &App) -> SharedString {
        let terminal = view.terminal().read(cx);
        if terminal.breadcrumb_text.is_empty() {
            let title = terminal.title(true);
            if title == "Terminal" {
                SharedString::from("")
            } else {
                title.into()
            }
        } else {
            terminal.breadcrumb_text.clone().into()
        }
    }
    fn current_terminal_title(&self, cx: &App) -> SharedString {
        let view = self.view.read(cx);
        Self::terminal_title_for_view(view, cx)
    }
    fn terminal_title(&self, cx: &App) -> SharedString {
        let title = self.current_terminal_title(cx);
        if title.is_empty() && !self.last_known_terminal_title.is_empty() {
            SharedString::from(self.last_known_terminal_title.clone())
        } else {
            title
        }
    }
    fn title(&self, cx: &App) -> SharedString {
        let terminal_title = self.terminal_title(cx);
        let custom_title = self.custom_title(cx);
        compose_terminal_thread_title(
            terminal_title.as_ref(),
            custom_title.as_ref().map(|title| title.as_ref()),
        )
    }
    fn editable_title(&self, cx: &App) -> SharedString {
        if let Some(custom_title) = self.custom_title(cx) {
            custom_title
        } else {
            let terminal_title = self.terminal_title(cx);
            SharedString::from(terminal_title_without_prefix(terminal_title.as_ref()).to_string())
        }
    }
    fn refresh_title(&mut self, cx: &mut App) -> bool {
        let terminal_title = self.current_terminal_title(cx);
        if !terminal_title.is_empty() {
            self.last_known_terminal_title = terminal_title.to_string();
        }

        let title = self.title(cx);
        let changed = self.last_known_title != title.as_ref();
        if changed {
            self.last_known_title = title.to_string();
        }
        changed
    }
    fn refresh_metadata(&mut self, cx: &mut App) -> bool {
        let title_changed = self.refresh_title(cx);
        let current_working_directory = self.view.read(cx).terminal().read(cx).working_directory();
        let working_directory_changed = current_working_directory
            .as_ref()
            .is_some_and(|current| self.working_directory.as_ref() != Some(current));
        if working_directory_changed {
            self.working_directory = current_working_directory;
        }
        title_changed || working_directory_changed
    }
    fn custom_title(&self, cx: &App) -> Option<SharedString> {
        self.view.read(cx).custom_title().map(SharedString::from)
    }
}
pub struct WorkspaceTerminalPanel {
    workspace: WeakEntity<Workspace>,
    workspace_id: Option<WorkspaceId>,
    project: Entity<Project>,
    fs: Arc<dyn Fs>,
    focus_handle: FocusHandle,
    active_terminal: Option<TerminalId>,
    terminals: HashMap<TerminalId, WorkspaceTerminal>,
    pending_terminal_spawns: std::collections::HashSet<TerminalId>,
    requested_terminal: Option<TerminalId>,
    zoomed: bool,
    pending_serialization: Option<Task<Result<()>>>,
    _subscriptions: Vec<Subscription>,
}

pub fn init(cx: &mut App) {
    crate::terminal_metadata_store::init(cx);
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &NewTerminal, window, cx| {
            if let Some(panel) = workspace.panel::<WorkspaceTerminalPanel>(cx) {
                panel.update(cx, |panel, cx| {
                    panel.new_terminal(Some(workspace), window, cx)
                });
                workspace.focus_panel::<WorkspaceTerminalPanel>(window, cx);
            }
        });
        workspace.register_action(
            |workspace, _: &workspace::NewWorkspaceTerminal, window, cx| {
                if let Some(panel) = workspace.panel::<WorkspaceTerminalPanel>(cx) {
                    panel.update(cx, |panel, cx| {
                        panel.new_terminal(Some(workspace), window, cx)
                    });
                    workspace.focus_panel::<WorkspaceTerminalPanel>(window, cx);
                }
            },
        );
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<WorkspaceTerminalPanel>(window, cx);
        });
    })
    .detach();
}

impl WorkspaceTerminalPanel {
    pub fn new(workspace: &Workspace, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let project = workspace.project().clone();
        let subscription = cx.subscribe(&project, |this, _, event, cx| {
            if matches!(
                event,
                project::Event::WorktreeAdded(_)
                    | project::Event::WorktreeRemoved(_)
                    | project::Event::WorktreeOrderChanged
                    | project::Event::WorktreePathsChanged { .. }
            ) {
                this.persist_all_terminal_metadata(cx);
            }
        });
        cx.on_release(|this, cx| {
            this.dismiss_all_terminal_notifications(cx);
            if let Some(task) = this.pending_serialization.take() {
                task.detach_and_log_err(cx);
            }
        })
        .detach();
        Self {
            workspace: workspace.weak_handle(),
            workspace_id: workspace.database_id(),
            project,
            fs: workspace.app_state().fs.clone(),
            focus_handle: cx.focus_handle(),
            active_terminal: None,
            terminals: HashMap::default(),
            pending_terminal_spawns: Default::default(),
            requested_terminal: None,
            zoomed: false,
            pending_serialization: None,
            _subscriptions: vec![subscription],
        }
    }

    pub fn load(
        workspace: WeakEntity<Workspace>,
        cx: AsyncWindowContext,
    ) -> Task<Result<Entity<Self>>> {
        cx.spawn(async move |cx| {
            let workspace_id = workspace.read_with(cx, |workspace, _| workspace.database_id())?;
            let kvp = cx.update(|_, cx| KeyValueStore::global(cx))?;
            let saved = cx
                .background_spawn(async move {
                    let key = workspace_id.map(|id| i64::from(id).to_string());
                    let current = key
                        .as_deref()
                        .and_then(|key| kvp.scoped(PANEL_KEY).read(key).log_err().flatten());
                    let json = current
                        .or_else(|| {
                            key.as_deref().and_then(|key| {
                                kvp.scoped("agent_panel").read(key).log_err().flatten()
                            })
                        })
                        .or_else(|| kvp.read_kvp("agent_panel").log_err().flatten());
                    json.and_then(|json| serde_json::from_str::<SerializedPanel>(&json).log_err())
                        .unwrap_or_default()
                })
                .await;
            let (store, reload) = cx.update(|_, cx| {
                let store = TerminalMetadataStore::global(cx);
                let reload = store.read(cx).reload_task();
                (store, reload)
            })?;
            reload.await;
            workspace.update_in(cx, |workspace, window, cx| {
                if let Some(panel) = workspace.panel::<Self>(cx) {
                    return panel;
                }
                let panel = cx.new(|cx| Self::new(workspace, window, cx));
                workspace.add_panel(panel.clone(), window, cx);
                let metadata = saved
                    .last_active_terminal_id
                    .as_deref()
                    .and_then(|id| TerminalId::from_key_string(id).log_err())
                    .and_then(|id| store.read(cx).entry(id).cloned());
                if let Some(metadata) = metadata.filter(|metadata| {
                    let project = workspace.project().read(cx);
                    metadata.folder_paths() == project.worktree_paths(cx).folder_path_list()
                        && metadata.remote_connection == project.remote_connection_options(cx)
                }) {
                    panel.update(cx, |panel, cx| {
                        panel.restore_terminal(metadata, false, Some(workspace), window, cx)
                    });
                }
                panel
            })
        })
    }

    fn serialize(&mut self, cx: &mut App) {
        let Some(workspace_id) = self.workspace_id else {
            return;
        };
        let saved = SerializedPanel {
            last_active_terminal_id: self.active_terminal.map(TerminalId::to_key_string),
        };
        let kvp = KeyValueStore::global(cx);
        self.pending_serialization = Some(cx.background_spawn(async move {
            kvp.scoped(PANEL_KEY)
                .write(
                    i64::from(workspace_id).to_string(),
                    serde_json::to_string(&saved)?,
                )
                .await
        }));
    }

    fn set_active_terminal(
        &mut self,
        terminal_id: TerminalId,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_terminal = Some(terminal_id);
        if focus && let Some(terminal) = self.terminals.get(&terminal_id) {
            terminal.view.focus_handle(cx).focus(window, cx);
        }
        self.serialize(cx);
        cx.emit(WorkspaceTerminalEvent::ActiveViewChanged);
        cx.notify();
    }

    pub fn active_terminal_id(&self) -> Option<TerminalId> {
        self.active_terminal
    }

    pub fn visible_terminal_view(&self) -> Option<&Entity<TerminalView>> {
        self.active_terminal
            .and_then(|id| self.terminals.get(&id))
            .map(|terminal| &terminal.view)
    }

    fn terminal_surface_visible(&self, terminal_id: TerminalId) -> bool {
        self.active_terminal == Some(terminal_id)
    }

    pub fn close_terminal(
        &mut self,
        terminal_id: TerminalId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_terminal_spawns.remove(&terminal_id);
        if self.requested_terminal == Some(terminal_id) {
            self.requested_terminal = None;
        }
        self.dismiss_terminal_notifications(terminal_id, cx);
        self.terminals.remove(&terminal_id);
        TerminalMetadataStore::global(cx).update(cx, |store, cx| store.delete(terminal_id, cx));
        if self.active_terminal == Some(terminal_id) {
            self.active_terminal = None;
            let next = self
                .terminals
                .iter()
                .max_by_key(|(_, terminal)| terminal.created_at)
                .map(|(id, _)| *id);
            if let Some(next) = next {
                self.activate_terminal(next, true, window, cx);
            }
        }
        self.serialize(cx);
        cx.emit(WorkspaceTerminalEvent::EntryChanged);
        cx.notify();
    }

    fn create_terminal_shell(
        &mut self,
        directory: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<terminal::Terminal>>> {
        self.project.update(cx, |project, cx| {
            project.create_terminal_shell(directory, cx)
        })
    }

    fn play_terminal_notification_sound(&self, visible: bool, cx: &mut App) {
        if WorkspaceTerminalSettings::get_global(cx)
            .play_sound_on_bell
            .should_play(visible)
        {
            audio::Audio::play_sound(audio::Sound::AgentDone, cx);
        }
    }
    pub fn new_terminal(
        &mut self,
        workspace: Option<&Workspace>,

        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.supports_terminal(cx) {
            return;
        }
        let working_directory = self.terminal_working_directory(workspace, cx);
        if !self.pending_terminal_spawns.is_empty() {
            return;
        }
        self.spawn_terminal(
            TerminalId::new(),
            working_directory,
            None,
            None,
            None,
            true,
            true,
            true,
            window,
            cx,
        );
    }
    fn terminal_working_directory(
        &self,
        workspace: Option<&Workspace>,
        cx: &App,
    ) -> Option<PathBuf> {
        workspace
            .map(|workspace| crate::default_working_directory(workspace, cx))
            .unwrap_or_else(|| self.default_terminal_working_directory(cx))
    }
    pub fn supports_terminal(&self, cx: &App) -> bool {
        self.has_open_project(cx) && self.project.read(cx).supports_terminal(cx)
    }
    fn spawn_terminal(
        &mut self,
        terminal_id: TerminalId,
        working_directory: Option<PathBuf>,
        custom_title: Option<SharedString>,
        initial_title: Option<SharedString>,
        created_at: Option<DateTime<Utc>>,
        select: bool,
        focus: bool,
        run_init_command: bool,

        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_terminal_spawns.insert(terminal_id);
        if select {
            self.requested_terminal = Some(terminal_id);
        }
        let terminal_working_directory = working_directory.clone();
        let init_command = Self::terminal_init_command(run_init_command, cx);
        let terminal_task = self.create_terminal_shell(working_directory, cx);
        let workspace = self.workspace.clone();
        let workspace_id = self.workspace_id;
        let project = self.project.downgrade();

        cx.spawn_in(window, async move |this, cx| {
            let terminal = match terminal_task.await {
                Ok(terminal) => terminal,
                Err(error) => {
                    log::error!("failed to spawn workspace terminal: {error:#}");
                    workspace
                        .update(cx, |workspace, cx| workspace.show_error(error, cx))
                        .log_err();
                    this.update(cx, |this, cx| {
                        if this.pending_terminal_spawns.remove(&terminal_id) {
                            cx.notify();
                        }
                    })
                    .log_err();
                    return anyhow::Ok(());
                }
            };
            this.update_in(cx, |this, window, cx| {
                if !this.pending_terminal_spawns.contains(&terminal_id) {
                    return;
                }
                let terminal_for_init_command = terminal.clone();
                let terminal_view = cx.new(|cx| {
                    let mut view =
                        TerminalView::new(terminal, workspace, workspace_id, project, window, cx);
                    view.set_show_workspace_actions(false, cx);
                    view
                });
                let select = select && this.requested_terminal == Some(terminal_id);
                this.insert_terminal(
                    terminal_id,
                    terminal_view,
                    terminal_working_directory,
                    custom_title,
                    initial_title,
                    created_at,
                    select,
                    focus,
                    window,
                    cx,
                );
                Self::write_terminal_init_command(&terminal_for_init_command, init_command, cx);
            })?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }
    fn terminal_init_command(run_init_command: bool, cx: &App) -> Option<String> {
        run_init_command
            .then(|| {
                WorkspaceTerminalSettings::get_global(cx)
                    .init_command
                    .clone()
            })
            .flatten()
            .filter(|command| !command.trim().is_empty())
    }
    fn write_terminal_init_command(
        terminal: &Entity<terminal::Terminal>,
        init_command: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(command) = init_command else {
            return;
        };

        if !terminal.read(cx).is_pty() {
            terminal.update(cx, |terminal, _| {
                terminal.write_init_command(Self::terminal_init_command_input(command))
            });
            return;
        }

        let startup = terminal.update(cx, |terminal, _| {
            terminal.start_init_command_startup_handshake()
        });

        let terminal = terminal.downgrade();
        cx.spawn(async move |_this, cx| {
            // Fall back to the timeout so the init command is still delivered if
            // the shell never echoes the marker.
            let timeout = cx
                .background_executor()
                .timer(TERMINAL_INIT_COMMAND_STARTUP_TIMEOUT);
            futures::select_biased! {
                _ = startup.fuse() => {}
                _ = timeout.fuse() => {}
            }

            let input = Self::terminal_init_command_input(command);
            if let Err(error) = terminal.update(cx, move |terminal, cx| {
                if !terminal.write_init_command_after_startup(input, cx) {
                    log::debug!(
                        "skipping terminal init command because the terminal is no longer eligible"
                    );
                }
            }) {
                log::debug!("skipping terminal init command because the terminal closed: {error}");
            }
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }
    fn terminal_init_command_input(command: String) -> Vec<u8> {
        let mut input = command.into_bytes();
        // CR, not "\r\n": "\r\n" puts PowerShell into continuation
        // mode (same convention as the activation-script writes in
        // `TerminalBuilder::new`).
        input.push(b'\x0d');
        input
    }
    fn insert_terminal(
        &mut self,
        terminal_id: TerminalId,
        terminal_view: Entity<TerminalView>,
        working_directory: Option<PathBuf>,
        custom_title: Option<SharedString>,
        initial_title: Option<SharedString>,
        created_at: Option<DateTime<Utc>>,
        select: bool,
        focus: bool,

        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(custom_title) = custom_title {
            terminal_view.update(cx, |terminal_view, cx| {
                terminal_view.set_custom_title(Some(custom_title.to_string()), cx);
            });
        }
        let terminal_entity = terminal_view.read(cx).terminal().clone();
        let view_subscription = cx.subscribe(
            &terminal_view,
            move |this, _terminal_view, event: &ItemEvent, cx| match event {
                ItemEvent::UpdateTab | ItemEvent::UpdateBreadcrumbs => {
                    this.refresh_terminal_metadata(terminal_id, cx);
                }
                ItemEvent::CloseItem | ItemEvent::Edit => {}
            },
        );
        // Listen on the underlying `Terminal` entity for shell-driven metadata
        // changes and bell.
        let terminal_subscription = cx.subscribe_in(
            &terminal_entity,
            window,
            move |this, _terminal, event: &TerminalEvent, window, cx| match event {
                TerminalEvent::TitleChanged
                | TerminalEvent::Wakeup
                | TerminalEvent::BreadcrumbsChanged => {
                    this.refresh_terminal_metadata(terminal_id, cx);
                }
                TerminalEvent::Bell => this.mark_terminal_notification(terminal_id, window, cx),
                TerminalEvent::CloseTerminal => {
                    this.close_terminal(terminal_id, window, cx);
                }
                TerminalEvent::BlinkChanged(_)
                | TerminalEvent::OutputReplaced
                | TerminalEvent::SelectionsChanged
                | TerminalEvent::NewNavigationTarget(_)
                | TerminalEvent::Open(_) => {}
            },
        );

        let last_known_terminal_title = initial_title
            .map(|title| title.to_string())
            .unwrap_or_default();
        let mut terminal = WorkspaceTerminal {
            view: terminal_view,
            title_editor: None,
            title_editor_initial_title: None,
            title_editor_subscription: None,
            last_known_title: last_known_terminal_title.clone(),
            last_known_terminal_title,
            working_directory,
            created_at: created_at.unwrap_or_else(Utc::now),
            has_notification: false,
            search_bar: None,
            notification_windows: Vec::new(),
            notification_subscriptions: Vec::new(),
            _subscriptions: vec![view_subscription, terminal_subscription],
        };
        self.pending_terminal_spawns.remove(&terminal_id);
        terminal.refresh_metadata(cx);
        self.terminals.insert(terminal_id, terminal);
        self.persist_terminal_metadata(terminal_id, cx);
        if select {
            self.set_active_terminal(terminal_id, focus, window, cx);
        }
        cx.emit(WorkspaceTerminalEvent::EntryChanged);
        cx.notify();
    }
    pub fn activate_terminal(
        &mut self,
        terminal_id: TerminalId,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.requested_terminal = Some(terminal_id);
        let Some(terminal) = self.terminals.get_mut(&terminal_id) else {
            return;
        };
        let had_notification = terminal.has_notification;
        terminal.has_notification = false;
        if had_notification {
            self.dismiss_terminal_notifications(terminal_id, cx);
        }
        self.set_active_terminal(terminal_id, focus, window, cx);
        if had_notification {
            cx.emit(WorkspaceTerminalEvent::EntryChanged);
            cx.notify();
        }
    }

    fn refresh_terminal_metadata(&mut self, terminal_id: TerminalId, cx: &mut Context<Self>) {
        if let Some(terminal) = self.terminals.get_mut(&terminal_id)
            && terminal.refresh_metadata(cx)
        {
            self.persist_terminal_metadata(terminal_id, cx);
            cx.emit(WorkspaceTerminalEvent::EntryChanged);
            cx.notify();
        }
    }
    fn persist_all_terminal_metadata(&self, cx: &mut Context<Self>) {
        let terminal_ids = self.terminals.keys().copied().collect::<Vec<_>>();
        for terminal_id in terminal_ids {
            self.persist_terminal_metadata(terminal_id, cx);
        }
    }
    fn persist_terminal_metadata(&self, terminal_id: TerminalId, cx: &mut Context<Self>) {
        let Some(store) = TerminalMetadataStore::try_global(cx) else {
            return;
        };
        let Some(metadata) = self.terminal_metadata(terminal_id, cx) else {
            return;
        };
        store.update(cx, |store, cx| {
            store.save(metadata, cx);
        });
    }
    fn terminal_metadata(&self, terminal_id: TerminalId, cx: &App) -> Option<TerminalMetadata> {
        let terminal = self.terminals.get(&terminal_id)?;
        let project = self.project.read(cx);
        Some(TerminalMetadata {
            terminal_id,
            title: terminal.terminal_title(cx),
            custom_title: terminal.custom_title(cx),
            created_at: terminal.created_at,
            worktree_paths: project.worktree_paths(cx),
            remote_connection: project.remote_connection_options(cx),
            working_directory: terminal.working_directory.clone(),
        })
    }
    pub fn restore_terminal(
        &mut self,
        metadata: TerminalMetadata,
        focus: bool,

        workspace: Option<&Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pending_terminal_spawns.contains(&metadata.terminal_id) {
            self.requested_terminal = Some(metadata.terminal_id);
            return;
        }
        if self.has_terminal(metadata.terminal_id) {
            self.activate_terminal(metadata.terminal_id, focus, window, cx);
            return;
        }

        if !self.supports_terminal(cx) {
            return;
        }

        let working_directory = self.terminal_restore_working_directory(&metadata, workspace, cx);
        let initial_title = Self::terminal_restore_initial_title(&metadata);
        self.spawn_terminal(
            metadata.terminal_id,
            working_directory,
            metadata.custom_title.clone(),
            initial_title,
            Some(metadata.created_at),
            true,
            focus,
            true,
            window,
            cx,
        );
    }
    fn terminal_restore_working_directory(
        &self,
        metadata: &TerminalMetadata,
        workspace: Option<&Workspace>,
        cx: &App,
    ) -> Option<PathBuf> {
        if let Some(working_directory) = metadata.working_directory.clone() {
            return Some(working_directory);
        }

        if let Some(workspace) = workspace {
            return crate::default_working_directory(workspace, cx);
        }

        self.default_terminal_working_directory(cx)
    }
    fn terminal_restore_initial_title(metadata: &TerminalMetadata) -> Option<SharedString> {
        (!metadata.title.is_empty()).then(|| metadata.title.clone())
    }
    pub fn edit_terminal_title(
        &mut self,
        terminal_id: TerminalId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(terminal) = self.terminals.get_mut(&terminal_id) else {
            return;
        };

        if let Some(title_editor) = terminal.title_editor.as_ref() {
            title_editor.focus_handle(cx).focus(window, cx);
            return;
        }

        let title = terminal.editable_title(cx).to_string();
        let title_editor_initial_title = title.clone();
        let title_editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_text(title, window, cx);
            editor
        });
        let title_editor_subscription = cx.subscribe_in(
            &title_editor,
            window,
            move |this, title_editor, event: &editor::EditorEvent, window, cx| {
                this.handle_terminal_title_editor_event(
                    terminal_id,
                    title_editor,
                    event,
                    window,
                    cx,
                );
            },
        );
        title_editor.update(cx, |editor, cx| {
            editor.select_all(&editor::actions::SelectAll, window, cx);
            editor.focus_handle(cx).focus(window, cx);
        });
        terminal.title_editor = Some(title_editor);
        terminal.title_editor_initial_title = Some(title_editor_initial_title);
        terminal.title_editor_subscription = Some(title_editor_subscription);
        cx.notify();
    }
    fn stop_editing_terminal_title(
        &mut self,
        terminal_id: TerminalId,
        focus_terminal: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(terminal) = self.terminals.get_mut(&terminal_id) else {
            return;
        };
        let terminal_view = terminal.view.clone();
        terminal.title_editor = None;
        terminal.title_editor_initial_title = None;
        terminal.title_editor_subscription = None;
        let title_changed = terminal.refresh_title(cx);

        if focus_terminal {
            terminal_view.focus_handle(cx).focus(window, cx);
        }
        if title_changed {
            cx.emit(WorkspaceTerminalEvent::EntryChanged);
        }
        cx.notify();
    }
    fn handle_terminal_title_editor_event(
        &mut self,
        terminal_id: TerminalId,
        title_editor: &Entity<Editor>,
        event: &editor::EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            editor::EditorEvent::BufferEdited => {
                if !title_editor.read(cx).is_focused(window) {
                    return;
                }
                let Some((terminal_view, initial_title, terminal_title)) =
                    self.terminals.get(&terminal_id).and_then(|terminal| {
                        terminal
                            .title_editor
                            .as_ref()
                            .is_some_and(|current_editor| current_editor == title_editor)
                            .then(|| {
                                (
                                    terminal.view.clone(),
                                    terminal.title_editor_initial_title.clone(),
                                    terminal.terminal_title(cx),
                                )
                            })
                    })
                else {
                    return;
                };
                let new_title = title_editor.read(cx).text(cx);
                if initial_title.as_deref() == Some(new_title.as_str()) {
                    return;
                }
                let custom_title = normalize_terminal_custom_title(
                    terminal_title.as_ref(),
                    SharedString::from(new_title),
                );

                cx.defer(move |cx| {
                    terminal_view.update(cx, |terminal_view, cx| {
                        terminal_view
                            .set_custom_title(custom_title.map(|title| title.to_string()), cx);
                    });
                });
            }
            editor::EditorEvent::Blurred => {
                if self
                    .terminals
                    .get(&terminal_id)
                    .and_then(|terminal| terminal.title_editor.as_ref())
                    .is_some_and(|current_editor| current_editor == title_editor)
                {
                    self.stop_editing_terminal_title(terminal_id, false, window, cx);
                }
            }
            _ => {}
        }
    }
    fn mark_terminal_notification(
        &mut self,
        terminal_id: TerminalId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_terminal_visible(terminal_id, window, cx) {
            return;
        }
        let newly_notified = {
            let Some(terminal) = self.terminals.get_mut(&terminal_id) else {
                return;
            };
            if terminal.has_notification {
                false
            } else {
                terminal.has_notification = true;
                true
            }
        };
        if newly_notified {
            cx.emit(WorkspaceTerminalEvent::EntryChanged);
            cx.notify();

            self.play_terminal_notification_sound(
                self.terminal_status_visible(terminal_id, window, cx),
                cx,
            );
            self.show_terminal_notification(terminal_id, window, cx);
        }
    }
    fn show_terminal_notification(
        &mut self,
        terminal_id: TerminalId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(terminal) = self.terminals.get(&terminal_id) else {
            return;
        };
        if !terminal.notification_windows.is_empty() {
            return;
        }
        let title = terminal.title(cx);
        if self.terminal_status_visible(terminal_id, window, cx) {
            return;
        }
        let settings = WorkspaceTerminalSettings::get_global(cx);
        match settings.notify_on_bell {
            TerminalNotificationDisplay::PrimaryScreen => {
                window.request_attention();
                if let Some(primary) = cx.primary_display() {
                    self.pop_up_terminal_notification(terminal_id, &title, primary, window, cx);
                }
            }
            TerminalNotificationDisplay::AllScreens => {
                window.request_attention();
                for screen in cx.displays() {
                    self.pop_up_terminal_notification(terminal_id, &title, screen, window, cx);
                }
            }
            TerminalNotificationDisplay::Never => {}
        }
    }
    fn pop_up_terminal_notification(
        &mut self,
        terminal_id: TerminalId,
        title: &SharedString,
        screen: Rc<dyn PlatformDisplay>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let options = TerminalNotification::window_options(screen, cx);
        let project_name = self.workspace.upgrade().and_then(|workspace| {
            workspace
                .read(cx)
                .project()
                .read(cx)
                .visible_worktrees(cx)
                .next()
                .map(|worktree| worktree.read(cx).root_name_str().to_string())
        });
        let title = title.clone();
        let Ok(screen_window) = cx.open_window(options, |_window, cx| {
            cx.new(|_cx| TerminalNotification::new(title, None, IconName::Terminal, project_name))
        }) else {
            return;
        };
        let Ok(pop_up) = screen_window.entity(cx) else {
            return;
        };

        let event_subscription = cx.subscribe_in(&pop_up, window, {
            move |this, _, event: &TerminalNotificationEvent, window, cx| match event {
                TerminalNotificationEvent::Accepted => {
                    let Some(handle) = window.window_handle().downcast::<MultiWorkspace>() else {
                        log::error!("root view should be a MultiWorkspace");
                        return;
                    };
                    cx.activate(true);

                    let workspace = this.workspace.clone();
                    cx.defer(move |cx| {
                        handle
                            .update(cx, |multi_workspace, window, cx| {
                                window.activate_window();

                                let Some(workspace) = workspace.upgrade() else {
                                    return;
                                };
                                multi_workspace.activate(workspace.clone(), None, window, cx);

                                workspace.update(cx, |workspace, cx| {
                                    workspace.reveal_panel::<WorkspaceTerminalPanel>(window, cx);
                                    if let Some(panel) =
                                        workspace.panel::<WorkspaceTerminalPanel>(cx)
                                    {
                                        panel.update(cx, |panel, cx| {
                                            panel.activate_terminal(terminal_id, true, window, cx);
                                        });
                                    }
                                    workspace.focus_panel::<WorkspaceTerminalPanel>(window, cx);
                                });
                            })
                            .log_err();
                    });

                    this.dismiss_terminal_notifications(terminal_id, cx);
                }
                TerminalNotificationEvent::Dismissed => {
                    this.dismiss_terminal_notifications(terminal_id, cx);
                }
            }
        });

        let pop_up_weak = pop_up.downgrade();
        let window_activation_subscription = cx.observe_window_activation(window, {
            let pop_up_weak = pop_up_weak.clone();
            move |this, window, cx| {
                this.dismiss_terminal_pop_up_if_visible(terminal_id, &pop_up_weak, window, cx);
            }
        });

        let multi_workspace_subscription = {
            let pop_up_weak = pop_up_weak.clone();
            window.root::<MultiWorkspace>().flatten().map(|mw| {
                cx.observe_in(&mw, window, move |this, _, window, cx| {
                    this.dismiss_terminal_pop_up_if_visible(terminal_id, &pop_up_weak, window, cx);
                })
            })
        };

        let this_panel = cx.entity();
        let panel_subscription = cx.subscribe_in(&this_panel, window, {
            move |this, _, event: &WorkspaceTerminalEvent, window, cx| match event {
                WorkspaceTerminalEvent::ActiveViewChanged
                | WorkspaceTerminalEvent::ActiveViewFocused => {
                    this.dismiss_terminal_pop_up_if_visible(terminal_id, &pop_up_weak, window, cx);
                }
                WorkspaceTerminalEvent::EntryChanged
                | WorkspaceTerminalEvent::TerminalCloseRequested { .. } => {}
            }
        });

        let Some(terminal) = self.terminals.get_mut(&terminal_id) else {
            screen_window
                .update(cx, |_, window, _| window.remove_window())
                .ok();
            return;
        };
        terminal.notification_windows.push(screen_window);
        terminal.notification_subscriptions.push(event_subscription);
        terminal
            .notification_subscriptions
            .push(window_activation_subscription);
        terminal.notification_subscriptions.push(panel_subscription);
        if let Some(subscription) = multi_workspace_subscription {
            terminal.notification_subscriptions.push(subscription);
        }
    }
    fn dismiss_terminal_notifications(&mut self, terminal_id: TerminalId, cx: &mut App) {
        let Some(terminal) = self.terminals.get_mut(&terminal_id) else {
            return;
        };
        let windows = std::mem::take(&mut terminal.notification_windows);
        terminal.notification_subscriptions.clear();
        for window in windows {
            window
                .update(cx, |_, window, _| {
                    window.remove_window();
                })
                .ok();
        }
    }
    fn dismiss_all_terminal_notifications(&mut self, cx: &mut App) {
        let terminal_ids = self.terminals.keys().copied().collect::<Vec<_>>();
        for terminal_id in terminal_ids {
            self.dismiss_terminal_notifications(terminal_id, cx);
        }
    }
    fn active_terminal_visible(&self, terminal_id: TerminalId, window: &Window, cx: &App) -> bool {
        if !window.is_window_active() {
            return false;
        }
        if !self.terminal_surface_visible(terminal_id) {
            return false;
        }
        let Some(workspace) = self.workspace.upgrade() else {
            return false;
        };
        if let Some(multi_workspace) = window.root::<MultiWorkspace>().flatten() {
            let multi_workspace = multi_workspace.read(cx);
            if multi_workspace.workspace() != &workspace {
                return false;
            }
        }
        WorkspaceTerminalPanel::is_visible(&workspace, cx)
    }
    fn terminal_status_visible(&self, terminal_id: TerminalId, window: &Window, cx: &App) -> bool {
        if !window.is_window_active() {
            return false;
        }

        if let Some(multi_workspace) = window.root::<MultiWorkspace>().flatten() {
            let multi_workspace = multi_workspace.read(cx);
            if multi_workspace.sidebar_open() && multi_workspace.is_terminal_list_view_active(cx) {
                return true;
            }

            let Some(workspace) = self.workspace.upgrade() else {
                return false;
            };

            return multi_workspace.workspace() == &workspace
                && self.terminal_surface_visible(terminal_id)
                && WorkspaceTerminalPanel::is_visible(&workspace, cx);
        }

        self.workspace.upgrade().is_some_and(|workspace| {
            self.terminal_surface_visible(terminal_id)
                && WorkspaceTerminalPanel::is_visible(&workspace, cx)
        })
    }
    fn dismiss_terminal_pop_up_if_visible(
        &mut self,
        terminal_id: TerminalId,
        pop_up: &WeakEntity<TerminalNotification>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal_status_visible(terminal_id, window, cx) {
            return;
        }
        if self.active_terminal_visible(terminal_id, window, cx)
            && let Some(terminal) = self.terminals.get_mut(&terminal_id)
            && terminal.has_notification
        {
            terminal.has_notification = false;
            cx.emit(WorkspaceTerminalEvent::EntryChanged);
            cx.notify();
        }
        if let Some(pop_up) = pop_up.upgrade() {
            pop_up.update(cx, |notification, cx| {
                notification.dismiss(cx);
            });
        }
    }
    fn default_terminal_working_directory(&self, cx: &App) -> Option<PathBuf> {
        // Reuse the workspace-based helper so behavior matches the regular
        // terminal panel (e.g. `WorkingDirectory::FirstProjectDirectory` falling
        // back to a file's parent directory when the worktree root is a file).
        self.workspace
            .upgrade()
            .and_then(|workspace| crate::default_working_directory(workspace.read(cx), cx))
    }
    fn has_open_project(&self, cx: &App) -> bool {
        self.project.read(cx).visible_worktrees(cx).next().is_some()
    }
    pub fn has_terminal(&self, terminal_id: TerminalId) -> bool {
        self.terminals.contains_key(&terminal_id)
    }
    pub fn has_pending_terminal(&self, terminal_id: TerminalId) -> bool {
        self.pending_terminal_spawns.contains(&terminal_id)
    }
    pub fn rename_terminal(
        &mut self,
        terminal_id: TerminalId,
        title: SharedString,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(terminal) = self.terminals.get(&terminal_id) else {
            return false;
        };
        let terminal_title = terminal.terminal_title(cx);
        let custom_title = normalize_terminal_custom_title(terminal_title.as_ref(), title);
        let terminal_view = terminal.view.clone();
        cx.defer(move |cx| {
            terminal_view.update(cx, |terminal_view, cx| {
                terminal_view.set_custom_title(custom_title.map(|title| title.to_string()), cx);
            });
        });
        true
    }
    pub fn terminals(&self, cx: &App) -> Vec<WorkspaceTerminalInfo> {
        self.terminals
            .iter()
            .map(|(id, terminal)| WorkspaceTerminalInfo {
                id: *id,
                title: terminal.title(cx),
                created_at: terminal.created_at,
                has_notification: terminal.has_notification,
                custom_title: terminal.custom_title(cx),
                working_directory: terminal.working_directory.clone(),
            })
            .collect()
    }
    pub fn is_visible(workspace: &Entity<Workspace>, cx: &App) -> bool {
        let workspace_read = workspace.read(cx);

        workspace_read
            .panel::<WorkspaceTerminalPanel>(cx)
            .map(|panel| {
                let panel_id = Entity::entity_id(&panel);

                workspace_read.all_docks().iter().any(|dock| {
                    dock.read(cx)
                        .visible_panel()
                        .is_some_and(|visible_panel| visible_panel.panel_id() == panel_id)
                })
            })
            .unwrap_or(false)
    }
    fn toggle_terminal_thread_search(
        &mut self,
        _: &ToggleSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(terminal) = self
            .active_terminal_id()
            .and_then(|terminal_id| self.terminals.get_mut(&terminal_id))
        else {
            cx.propagate();
            return;
        };

        let terminal_view = terminal.view.clone();
        let search_bar = terminal
            .search_bar
            .get_or_insert_with(|| cx.new(|cx| BufferSearchBar::new(None, window, cx)))
            .clone();
        let deployed = search_bar.update(cx, |search_bar, cx| {
            let terminal_item: &dyn ItemHandle = &terminal_view;
            search_bar.set_active_pane_item(Some(terminal_item), window, cx);
            search_bar.deploy(&DeployBufferSearch::find(), None, window, cx)
        });
        if deployed {
            cx.stop_propagation();
        } else {
            cx.propagate();
        }
    }
}
impl EventEmitter<WorkspaceTerminalEvent> for WorkspaceTerminalPanel {}
impl EventEmitter<PanelEvent> for WorkspaceTerminalPanel {}
impl Focusable for WorkspaceTerminalPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
impl Panel for WorkspaceTerminalPanel {
    fn activation_priority(&self) -> u32 {
        0
    }
    fn persistent_name() -> &'static str {
        "WorkspaceTerminalPanel"
    }
    fn panel_key() -> &'static str {
        PANEL_KEY
    }
    fn activation_focus_handle(&self, cx: &App) -> FocusHandle {
        self.visible_terminal_view()
            .map_or_else(|| self.focus_handle.clone(), |view| view.focus_handle(cx))
    }
    fn position(&self, _: &Window, cx: &App) -> DockPosition {
        match WorkspaceTerminalSettings::get_global(cx).dock {
            settings::DockPosition::Right => DockPosition::Right,
            _ => DockPosition::Left,
        }
    }
    fn position_is_valid(&self, position: DockPosition) -> bool {
        position != DockPosition::Bottom
    }
    fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
        settings::update_settings_file(self.fs.clone(), cx, move |settings, _| {
            settings.workspace_terminal.get_or_insert_default().dock = Some(position.into())
        });
    }
    fn default_size(&self, _: &Window, cx: &App) -> Pixels {
        WorkspaceTerminalSettings::get_global(cx).default_width
    }
    fn min_size(&self, _: &Window, _: &App) -> Option<Pixels> {
        Some(px(300.0))
    }
    fn supports_flexible_size(&self) -> bool {
        true
    }
    fn has_flexible_size(&self, _: &Window, cx: &App) -> bool {
        WorkspaceTerminalSettings::get_global(cx).flexible
    }
    fn set_flexible_size(&mut self, flexible: bool, _: &mut Window, cx: &mut Context<Self>) {
        settings::update_settings_file(self.fs.clone(), cx, move |settings, _| {
            settings.workspace_terminal.get_or_insert_default().flexible = Some(flexible)
        });
    }
    fn icon(&self, _: &Window, cx: &App) -> Option<IconName> {
        WorkspaceTerminalSettings::get_global(cx)
            .button
            .then_some(IconName::Terminal)
    }
    fn icon_tooltip(&self, _: &Window, cx: &App) -> Option<&'static str> {
        Some(i18n::static_text("Workspace Terminals", cx))
    }
    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }
    fn is_zoomed(&self, _: &Window, _: &App) -> bool {
        self.zoomed
    }
    fn set_zoomed(&mut self, zoomed: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.zoomed = zoomed;
        cx.notify();
    }
}

impl Render for WorkspaceTerminalPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let terminal_id = self.active_terminal;
        let active = terminal_id.and_then(|id| self.terminals.get(&id));
        let title_editor = active.and_then(|terminal| terminal.title_editor.clone());
        let title = active.map_or_else(
            || SharedString::from("Terminal"),
            |terminal| terminal.title(cx),
        );
        let view = active.map(|terminal| terminal.view.clone());
        let search_bar = active.and_then(|terminal| terminal.search_bar.clone());
        v_flex()
            .key_context("WorkspaceTerminal")
            .track_focus(&self.focus_handle)
            .size_full()
            .on_action(cx.listener(Self::toggle_terminal_thread_search))
            .on_action(cx.listener(|this, _: &RenameTerminal, window, cx| {
                if let Some(id) = this.active_terminal {
                    this.edit_terminal_title(id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CloseTerminal, window, cx| {
                if let Some(id) = this.active_terminal {
                    this.close_terminal(id, window, cx);
                }
            }))
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(Icon::new(IconName::Terminal))
                    .child(
                        div()
                            .id("workspace-terminal-title")
                            .flex_1()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(id) = terminal_id {
                                    this.edit_terminal_title(id, window, cx);
                                }
                            }))
                            .on_action(cx.listener(move |this, _: &menu::Confirm, window, cx| {
                                if let Some(id) = terminal_id {
                                    this.stop_editing_terminal_title(id, true, window, cx);
                                }
                            }))
                            .on_action(cx.listener(
                                move |this, _: &editor::actions::Cancel, window, cx| {
                                    if let Some(id) = terminal_id {
                                        this.stop_editing_terminal_title(id, false, window, cx);
                                    }
                                },
                            ))
                            .map(|this| {
                                if let Some(editor) = title_editor {
                                    this.child(editor)
                                } else {
                                    this.child(Label::new(title).truncate())
                                }
                            }),
                    )
                    .child(
                        IconButton::new("new-workspace-terminal", IconName::Plus)
                            .tooltip(Tooltip::localized_text("New Terminal"))
                            .disabled(
                                !self.supports_terminal(cx)
                                    || !self.pending_terminal_spawns.is_empty(),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_terminal(None, window, cx)
                            })),
                    )
                    .child(
                        IconButton::new("close-workspace-terminal", IconName::Close)
                            .tooltip(Tooltip::localized_text("Close Terminal"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(id) = terminal_id {
                                    this.close_terminal(id, window, cx);
                                }
                            })),
                    ),
            )
            .when_some(search_bar, |this, search_bar| {
                this.when(!search_bar.read(cx).is_dismissed(), |this| {
                    this.child(search_bar)
                })
            })
            .map(|this| {
                if let Some(view) = view {
                    this.child(view)
                } else {
                    this.child(
                        div().p_4().child(
                            Label::localized(if !self.pending_terminal_spawns.is_empty() {
                                "Starting terminal…"
                            } else if self.supports_terminal(cx) {
                                "Create a terminal to get started"
                            } else {
                                "Open a project that supports terminals"
                            })
                            .color(Color::Muted),
                        ),
                    )
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use settings::SettingsStore;
    use util::{path, paths::PathStyle};

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings = SettingsStore::test(cx);
            cx.set_global(settings);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
            search::init(cx);
            crate::init(cx);
            init(cx);
        });
    }

    fn add_terminal(
        panel: &Entity<WorkspaceTerminalPanel>,
        window: &mut Window,
        cx: &mut App,
    ) -> TerminalId {
        panel.update(cx, |panel, cx| {
            let terminal_id = TerminalId::new();
            let terminal = cx.new(|cx| {
                terminal::TerminalBuilder::new_display_only(
                    terminal::terminal_settings::CursorShape::default(),
                    terminal::terminal_settings::AlternateScroll::On,
                    None,
                    0,
                    cx.background_executor(),
                    PathStyle::local(),
                )
                .subscribe(cx)
            });
            let view = cx.new(|cx| {
                TerminalView::new(
                    terminal,
                    panel.workspace.clone(),
                    panel.workspace_id,
                    panel.project.downgrade(),
                    window,
                    cx,
                )
            });
            panel.insert_terminal(
                terminal_id,
                view,
                None,
                None,
                Some("Shell".into()),
                None,
                true,
                true,
                window,
                cx,
            );
            terminal_id
        })
    }

    #[gpui::test]
    async fn terminal_rename_and_close_preserve_other_sessions(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(path!("/project"), serde_json::json!({"main.rs":""}))
            .await;
        let project = Project::test(fs, [path!("/project").as_ref()], cx).await;
        let window = cx.add_window(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let (panel, first, second) = window
            .update(cx, |multi_workspace, window, cx| {
                let panel = multi_workspace.workspace().update(cx, |workspace, cx| {
                    let panel = cx.new(|cx| WorkspaceTerminalPanel::new(workspace, window, cx));
                    workspace.add_panel(panel.clone(), window, cx);
                    panel
                });
                let first = add_terminal(&panel, window, cx);
                let second = add_terminal(&panel, window, cx);
                panel.update(cx, |panel, cx| {
                    assert!(panel.rename_terminal(first, "Build".into(), cx))
                });
                (panel, first, second)
            })
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let store = TerminalMetadataStore::global(cx);
            assert_eq!(
                store.read(cx).entry(first).unwrap().custom_title.as_deref(),
                Some("Build")
            );
        });
        window
            .update(cx, |_, window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.close_terminal(second, window, cx);
                    assert_eq!(panel.active_terminal_id(), Some(first));
                    assert!(panel.has_terminal(first));
                    assert!(!panel.has_terminal(second));
                    panel.close_terminal(first, window, cx);
                    assert_eq!(panel.active_terminal_id(), None);
                    assert!(panel.terminals(cx).is_empty());
                })
            })
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(
                TerminalMetadataStore::global(cx)
                    .read(cx)
                    .entries()
                    .next()
                    .is_none()
            )
        });
    }

    #[gpui::test]
    async fn switching_workspaces_retains_terminal_entities(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = fs::FakeFs::new(cx.executor());
        fs.insert_tree(path!("/first"), serde_json::json!({"main.rs":""}))
            .await;
        fs.insert_tree(path!("/second"), serde_json::json!({"main.rs":""}))
            .await;
        let first_project = Project::test(fs.clone(), [path!("/first").as_ref()], cx).await;
        let second_project = Project::test(fs, [path!("/second").as_ref()], cx).await;
        let window =
            cx.add_window(|window, cx| MultiWorkspace::test_new(first_project, window, cx));
        window
            .update(cx, |multi_workspace, window, cx| {
                let first_workspace = multi_workspace.workspace().clone();
                let panel = first_workspace.update(cx, |workspace, cx| {
                    let panel = cx.new(|cx| WorkspaceTerminalPanel::new(workspace, window, cx));
                    workspace.add_panel(panel.clone(), window, cx);
                    panel
                });
                let terminal_id = add_terminal(&panel, window, cx);
                let view = panel.read(cx).visible_terminal_view().unwrap().clone();
                multi_workspace.test_add_workspace(second_project, window, cx);
                multi_workspace.activate(first_workspace, None, window, cx);
                assert_eq!(panel.read(cx).active_terminal_id(), Some(terminal_id));
                assert_eq!(panel.read(cx).visible_terminal_view(), Some(&view));
                assert_eq!(panel.read(cx).terminals(cx).len(), 1);
            })
            .unwrap();
    }

    #[test]
    fn legacy_panel_state_ignores_agent_session_fields() {
        let terminal_id = TerminalId::new();
        let serialized = serde_json::json!({
            "selected_agent": {"unknown_old_agent":{}},
            "last_active_thread":{"session_id":"old"},
            "last_active_terminal_id": terminal_id.to_key_string(),
        });
        let state: SerializedPanel = serde_json::from_value(serialized).unwrap();
        assert_eq!(
            state.last_active_terminal_id,
            Some(terminal_id.to_key_string())
        );
    }

    #[gpui::test]
    async fn concurrent_panel_loads_share_one_panel(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = fs::FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let window = cx.add_window(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let (first, second) = window
            .update(cx, |multi_workspace, window, cx| {
                let workspace = multi_workspace.workspace().downgrade();
                (
                    WorkspaceTerminalPanel::load(workspace.clone(), window.to_async(cx)),
                    WorkspaceTerminalPanel::load(workspace, window.to_async(cx)),
                )
            })
            .unwrap();
        let (first, second) = futures::join!(first, second);
        assert_eq!(first.unwrap(), second.unwrap());
    }

    #[gpui::test]
    async fn terminal_search_and_shell_exit(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = fs::FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace =
            multi_workspace.read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone());
        let panel = workspace.update_in(cx, |workspace, window, cx| {
            cx.new(|cx| WorkspaceTerminalPanel::new(workspace, window, cx))
        });
        let (terminal_id, terminal) = cx.update(|window, cx| {
            let id = add_terminal(&panel, window, cx);
            panel.update(cx, |panel, cx| {
                panel.toggle_terminal_thread_search(&ToggleSearch, window, cx);
                let terminal = panel.terminals.get(&id).unwrap();
                assert!(
                    !terminal
                        .search_bar
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .is_dismissed()
                );
                (id, terminal.view.read(cx).terminal().clone())
            })
        });
        terminal.update(cx, |_, cx| cx.emit(TerminalEvent::CloseTerminal));
        cx.run_until_parked();
        panel.read_with(cx, |panel, cx| {
            assert!(!panel.has_terminal(terminal_id));
            assert!(
                TerminalMetadataStore::global(cx)
                    .read(cx)
                    .entry(terminal_id)
                    .is_none()
            );
        });
    }
}
