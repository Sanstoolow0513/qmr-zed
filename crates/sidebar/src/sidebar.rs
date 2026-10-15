#[cfg(test)]
mod sidebar_tests;
mod terminal_switcher;
use editor::Editor;
use gpui::{
    AnyElement, App, Context, Decorations, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    ListState, Render, Subscription, TaskExt, WeakEntity, Window, prelude::*,
};
use platform_title_bar::apply_title_bar_insets;
use recent_projects::sidebar_recent_projects::SidebarRecentProjects;
use remote::RemoteConnectionOptions;
use serde::{Deserialize, Serialize};
use settings::{Settings, SettingsStore};
use std::collections::{HashMap, HashSet};
use terminal_switcher::{TerminalSwitcher, TerminalSwitcherEvent};
use terminal_view::{
    terminal_metadata_store::{TerminalMetadata, TerminalMetadataStore},
    workspace_terminal_panel::{TerminalId, WorkspaceTerminalPanel},
};
use ui::{
    ContextMenu, Divider, KeyBinding, PopoverMenu, PopoverMenuHandle, TintColor, Tooltip,
    prelude::*, utils::platform_title_bar_height,
};
use util::ResultExt;
use workspace::{
    CloseWindow, FocusWorkspaceSidebar, MultiWorkspace, MultiWorkspaceEvent, OpenMode,
    ProjectGroupKey, Sidebar as WorkspaceSidebar, SidebarSide, ToggleWorkspaceSidebar, Workspace,
    WorkspaceSidebarSettings, sidebar_side_context_menu,
};
use zed_actions::OpenRecent;
use zed_actions::workspace_sidebar::FocusSidebarFilter;

gpui::actions!(
    workspace_sidebar,
    [
        /// Creates a terminal in the selected project.
        NewTerminalInGroup,
    ]
);

#[derive(Clone, PartialEq)]
enum Entry {
    Project {
        key: ProjectGroupKey,
        title: SharedString,
        expanded: bool,
    },
    Terminal {
        key: ProjectGroupKey,
        metadata: TerminalMetadata,
        workspace: Option<Entity<Workspace>>,
        notified: bool,
    },
}
impl Entry {
    fn key(&self) -> &ProjectGroupKey {
        match self {
            Self::Project { key, .. } | Self::Terminal { key, .. } => key,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct SerializedSidebar {
    width: Option<f32>,
    #[serde(default)]
    width_set_by_user: bool,
}

pub struct Sidebar {
    multi_workspace: WeakEntity<MultiWorkspace>,
    focus_handle: FocusHandle,
    filter_editor: Entity<Editor>,
    width: Pixels,
    width_set_by_user: bool,
    selection: Option<usize>,
    entries: Vec<Entry>,
    list_state: ListState,
    recent_projects_popover_handle: PopoverMenuHandle<SidebarRecentProjects>,
    observed: HashMap<EntityId, Subscription>,
    switcher: Option<Entity<TerminalSwitcher>>,
    switcher_subscription: Option<Subscription>,
    terminal_history: Vec<TerminalId>,
    active_workspace_terminal: Option<(EntityId, Option<TerminalId>)>,
}

impl Sidebar {
    fn schedule_update_entries(cx: &mut Context<Self>) {
        let sidebar = cx.weak_entity();
        cx.defer(move |cx| {
            sidebar
                .update(cx, |this, cx| this.update_entries(cx))
                .log_err();
        });
    }

    pub fn new(
        multi_workspace: Entity<MultiWorkspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter_editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Search projects and terminals…", window, cx);
            editor
        });
        cx.subscribe(&filter_editor, |this: &mut Self, _, event, cx| {
            if matches!(event, editor::EditorEvent::BufferEdited) {
                this.selection = None;
                this.update_entries(cx);
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(&multi_workspace, |_, _, _: &MultiWorkspaceEvent, cx| {
            Self::schedule_update_entries(cx);
        })
        .detach();
        cx.observe(&multi_workspace, |_, _, cx| {
            Self::schedule_update_entries(cx);
        })
        .detach();
        cx.observe(&TerminalMetadataStore::global(cx), |_, _, cx| {
            Self::schedule_update_entries(cx);
        })
        .detach();
        let mut previous_width = WorkspaceSidebarSettings::get_global(cx).default_width;
        cx.observe_global::<SettingsStore>(move |this, cx| {
            let width = WorkspaceSidebarSettings::get_global(cx).default_width;
            if width != previous_width {
                previous_width = width;
                this.set_width(None, cx);
            }
            cx.notify();
        })
        .detach();
        let list_state = ListState::new(0, gpui::ListAlignment::Top, px(500.0));
        let focus_handle = cx.focus_handle();
        cx.on_focus_in(&focus_handle, window, |this, window, cx| {
            if this.selection.is_none() && this.focus_handle.is_focused(window) {
                this.filter_editor.focus_handle(cx).focus(window, cx);
            }
        })
        .detach();
        let mut sidebar = Self {
            multi_workspace: multi_workspace.downgrade(),
            focus_handle,
            filter_editor,
            width: WorkspaceSidebarSettings::get_global(cx).default_width,
            width_set_by_user: false,
            selection: None,
            entries: Vec::new(),
            list_state,
            recent_projects_popover_handle: PopoverMenuHandle::default(),
            observed: HashMap::new(),
            switcher: None,
            switcher_subscription: None,
            terminal_history: Vec::new(),
            active_workspace_terminal: None,
        };
        sidebar.update_entries(cx);
        sidebar
    }

    fn update_entries(&mut self, cx: &mut Context<Self>) {
        let Some(multi_workspace) = self.multi_workspace.upgrade() else {
            return;
        };
        let groups = multi_workspace.read(cx).project_groups(cx);
        let current = multi_workspace.read(cx).workspace();
        let active_workspace_terminal = Some((
            current.entity_id(),
            current
                .read(cx)
                .panel::<WorkspaceTerminalPanel>(cx)
                .and_then(|panel| panel.read(cx).active_terminal_id()),
        ));
        if let Some(active) = current
            .read(cx)
            .panel::<WorkspaceTerminalPanel>(cx)
            .and_then(|panel| panel.read(cx).active_terminal_id())
            && self.terminal_history.first() != Some(&active)
        {
            self.terminal_history.retain(|terminal| *terminal != active);
            self.terminal_history.insert(0, active);
        }
        let query = self.filter_editor.read(cx).text(cx).to_lowercase();
        let store = TerminalMetadataStore::global(cx);
        let mut entries = Vec::new();
        let mut seen = HashSet::new();
        let mut observed = HashSet::new();
        for group in groups {
            let title = group.key.display_name(&HashMap::new());
            let host = group.key.host();
            let mut terminals: Vec<_> = store
                .read(cx)
                .entries_for_main_worktree_path(group.key.path_list(), host.as_ref())
                .cloned()
                .collect();
            let mut live = HashMap::new();
            for workspace in &group.workspaces {
                observed.insert(workspace.entity_id());
                self.observed
                    .entry(workspace.entity_id())
                    .or_insert_with(|| {
                        cx.observe(workspace, |_, _, cx| {
                            Self::schedule_update_entries(cx);
                        })
                    });
                if let Some(panel) = workspace.read(cx).panel::<WorkspaceTerminalPanel>(cx) {
                    observed.insert(panel.entity_id());
                    self.observed.entry(panel.entity_id()).or_insert_with(|| {
                        cx.observe(&panel, |_, _, cx| {
                            Self::schedule_update_entries(cx);
                        })
                    });
                    for terminal in panel.read(cx).terminals(cx) {
                        live.insert(terminal.id, (workspace.clone(), terminal.has_notification));
                    }
                    for metadata in &terminals {
                        if panel.read(cx).has_pending_terminal(metadata.terminal_id) {
                            live.insert(metadata.terminal_id, (workspace.clone(), false));
                        }
                    }
                }
                for terminal in store.read(cx).entries_for_path(
                    &workspace
                        .read(cx)
                        .project()
                        .read(cx)
                        .worktree_paths(cx)
                        .folder_path_list()
                        .clone(),
                    host.as_ref(),
                ) {
                    if !terminals
                        .iter()
                        .any(|entry| entry.terminal_id == terminal.terminal_id)
                    {
                        terminals.push(terminal.clone());
                    }
                }
            }
            terminals.sort_by_key(|terminal| std::cmp::Reverse(terminal.created_at));
            let project_matches = query.is_empty() || title.to_lowercase().contains(&query);
            terminals.retain(|terminal| {
                project_matches
                    || terminal.display_title().to_lowercase().contains(&query)
                    || terminal
                        .folder_paths()
                        .ordered_paths()
                        .any(|path| path.to_string_lossy().to_lowercase().contains(&query))
            });
            if !project_matches && terminals.is_empty() {
                continue;
            }
            entries.push(Entry::Project {
                key: group.key.clone(),
                title,
                expanded: group.expanded,
            });
            if group.expanded || !query.is_empty() {
                for metadata in terminals {
                    if !seen.insert(metadata.terminal_id) {
                        continue;
                    }
                    let (workspace, notified) = live
                        .get(&metadata.terminal_id)
                        .map_or((None, false), |(workspace, notified)| {
                            (Some(workspace.clone()), *notified)
                        });
                    entries.push(Entry::Terminal {
                        key: group.key.clone(),
                        metadata,
                        workspace,
                        notified,
                    });
                }
            }
        }
        self.observed.retain(|id, _| observed.contains(id));
        if self.entries == entries && self.active_workspace_terminal == active_workspace_terminal {
            return;
        }
        self.active_workspace_terminal = active_workspace_terminal;
        self.selection = self.selection.filter(|index| *index < entries.len());
        let count = entries.len();
        self.entries = entries;
        if self.list_state.item_count() != count {
            self.list_state.reset(count);
        }
        cx.notify();
    }

    fn activate_entry(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get(index).cloned() else {
            return;
        };
        self.selection = Some(index);
        match entry {
            Entry::Project { key, .. } => self.open_group(key, false, window, cx),
            Entry::Terminal {
                key,
                metadata,
                workspace,
                ..
            } => {
                if let Some(workspace) = workspace.or_else(|| {
                    self.multi_workspace.upgrade().and_then(|multi_workspace| {
                        multi_workspace.read(cx).workspace_for_paths(
                            metadata.folder_paths(),
                            metadata.remote_connection.as_ref(),
                            cx,
                        )
                    })
                }) {
                    self.activate_terminal(workspace, metadata, window, cx);
                } else {
                    self.open_terminal_workspace(key, metadata, window, cx);
                }
            }
        }
    }

    fn activate_terminal(
        &mut self,
        workspace: Entity<Workspace>,
        metadata: TerminalMetadata,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(multi_workspace) = self.multi_workspace.upgrade() {
            multi_workspace.update(cx, |multi_workspace, cx| {
                multi_workspace.activate(workspace.clone(), None, window, cx);
                multi_workspace.retain_active_workspace(cx);
            });
        }
        Self::with_terminal_panel(
            workspace,
            window,
            cx,
            move |panel, workspace, window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.restore_terminal(metadata, true, Some(workspace), window, cx)
                });
            },
        );
    }

    fn with_terminal_panel(
        workspace: Entity<Workspace>,
        window: &mut Window,
        cx: &mut App,
        callback: impl FnOnce(Entity<WorkspaceTerminalPanel>, &Workspace, &mut Window, &mut App)
        + 'static,
    ) {
        if let Some(panel) = workspace.read(cx).panel::<WorkspaceTerminalPanel>(cx) {
            workspace.update(cx, |workspace, cx| {
                callback(panel, workspace, window, cx);
                workspace.focus_panel::<WorkspaceTerminalPanel>(window, cx);
            });
            return;
        }
        let mut async_cx = window.to_async(cx);
        cx.spawn(async move |_cx| {
            let panel =
                WorkspaceTerminalPanel::load(workspace.downgrade(), async_cx.clone()).await?;
            workspace.update_in(&mut async_cx, |workspace, window, cx| {
                let panel = workspace
                    .panel::<WorkspaceTerminalPanel>(cx)
                    .unwrap_or_else(|| {
                        workspace.add_panel(panel.clone(), window, cx);
                        panel
                    });
                callback(panel, workspace, window, cx);
                workspace.focus_panel::<WorkspaceTerminalPanel>(window, cx);
            })?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn open_terminal_workspace(
        &mut self,
        key: ProjectGroupKey,
        metadata: TerminalMetadata,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(multi_workspace) = self.multi_workspace.upgrade() else {
            return;
        };
        let current = multi_workspace.read(cx).workspace().clone();
        let modal_workspace = current.clone();
        let task = multi_workspace.update(cx, |multi_workspace, cx| {
            multi_workspace.find_or_create_workspace(
                metadata.folder_paths().clone(),
                key.host(),
                Some(key),
                |options, window, cx| connect_remote(current, options, window, cx),
                None,
                OpenMode::Activate,
                None,
                window,
                cx,
            )
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            remote_connection::dismiss_connection_modal(&modal_workspace, cx);
            let workspace = result?;
            this.update_in(cx, |this, window, cx| {
                this.activate_terminal(workspace, metadata, window, cx)
            })?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn open_group(
        &mut self,
        key: ProjectGroupKey,
        create_terminal: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(multi_workspace) = self.multi_workspace.upgrade() else {
            return;
        };
        let existing = multi_workspace
            .read(cx)
            .last_active_workspace_for_group(&key, cx);
        if let Some(workspace) = existing {
            multi_workspace.update(cx, |multi_workspace, cx| {
                multi_workspace.activate(workspace.clone(), None, window, cx);
                multi_workspace.retain_active_workspace(cx);
            });
            if create_terminal {
                Self::create_terminal(workspace, window, cx);
            }
            return;
        }
        let current = multi_workspace.read(cx).workspace().clone();
        let modal_workspace = current.clone();
        let task = multi_workspace.update(cx, |multi_workspace, cx| {
            multi_workspace.find_or_create_workspace(
                key.path_list().clone(),
                key.host(),
                Some(key),
                |options, window, cx| connect_remote(current, options, window, cx),
                None,
                OpenMode::Activate,
                None,
                window,
                cx,
            )
        });
        cx.spawn_in(window, async move |_, cx| {
            let result = task.await;
            remote_connection::dismiss_connection_modal(&modal_workspace, cx);
            let workspace = result?;
            if create_terminal {
                cx.update(|window, cx| Self::create_terminal(workspace, window, cx))?;
            }
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn create_terminal(workspace: Entity<Workspace>, window: &mut Window, cx: &mut App) {
        Self::with_terminal_panel(workspace, window, cx, |panel, workspace, window, cx| {
            panel.update(cx, |panel, cx| {
                panel.new_terminal(Some(workspace), window, cx)
            })
        });
    }

    fn new_terminal_in_group(
        &mut self,
        _: &NewTerminalInGroup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = self
            .selection
            .and_then(|index| self.entries.get(index))
            .map(|entry| entry.key().clone())
            .or_else(|| {
                self.multi_workspace.upgrade().map(|multi_workspace| {
                    multi_workspace
                        .read(cx)
                        .workspace()
                        .read(cx)
                        .project_group_key(cx)
                })
            });
        if let Some(key) = key {
            self.open_group(key, true, window, cx);
        }
    }

    fn close_selected_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Entry::Terminal {
            metadata,
            workspace,
            ..
        }) = self
            .selection
            .and_then(|index| self.entries.get(index))
            .cloned()
        else {
            return;
        };
        if let Some(panel) =
            workspace.and_then(|workspace| workspace.read(cx).panel::<WorkspaceTerminalPanel>(cx))
        {
            panel.update(cx, |panel, cx| {
                panel.close_terminal(metadata.terminal_id, window, cx)
            });
        } else {
            TerminalMetadataStore::global(cx)
                .update(cx, |store, cx| store.delete(metadata.terminal_id, cx));
        }
        self.update_entries(cx);
    }

    fn expand_selected(&mut self, expanded: bool, cx: &mut Context<Self>) {
        if let Some(key) = self
            .selection
            .and_then(|index| self.entries.get(index))
            .map(|entry| entry.key().clone())
        {
            self.set_group_expanded(&key, expanded, cx);
            self.update_entries(cx);
        }
    }

    fn move_selection(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.entries.len();
        if count == 0 {
            return;
        }
        let index = match self.selection {
            Some(index) if forward => (index + 1) % count,
            Some(index) => (index + count - 1) % count,
            None if forward => 0,
            None => count - 1,
        };
        self.selection = Some(index);
        self.focus_handle.focus(window, cx);
        self.list_state.scroll_to_reveal_item(index);
        cx.notify();
    }

    fn cycle(
        &mut self,
        projects: bool,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(multi_workspace) = self.multi_workspace.upgrade() else {
            return;
        };
        let workspace = multi_workspace.read(cx).workspace();
        let active_key = workspace.read(cx).project_group_key(cx);
        let active_terminal = workspace
            .read(cx)
            .panel::<WorkspaceTerminalPanel>(cx)
            .and_then(|panel| panel.read(cx).active_terminal_id());
        let choices: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches!(entry, Entry::Project { .. }) == projects)
            .collect();
        if choices.is_empty() {
            return;
        }
        let current = choices.iter().position(|(_, entry)| match entry {
            Entry::Project { key, .. } => *key == active_key,
            Entry::Terminal { metadata, .. } => Some(metadata.terminal_id) == active_terminal,
        });
        let index = current.map_or(if forward { 0 } else { choices.len() - 1 }, |index| {
            if forward {
                (index + 1) % choices.len()
            } else {
                (index + choices.len() - 1) % choices.len()
            }
        });
        if let Some((index, _)) = choices.get(index) {
            self.activate_entry(*index, window, cx);
        }
    }

    fn render_entry(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(entry) = self.entries.get(index).cloned() else {
            return div().into_any_element();
        };
        let selected = self.selection == Some(index);
        match entry {
            Entry::Project { key, title, .. } => {
                let expanded = !self.is_group_collapsed(&key, cx);
                let toggle_key = key.clone();
                let new_key = key.clone();
                let menu_key = key.clone();
                h_flex()
                    .id(("project", index))
                    .px_1()
                    .py_1()
                    .gap_1()
                    .w_full()
                    .when(selected, |this| {
                        this.bg(cx.theme().colors().element_selected)
                    })
                    .hover(|this| this.bg(cx.theme().colors().ghost_element_hover))
                    .child(
                        IconButton::new(
                            ("expand-project", index),
                            if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            },
                        )
                        .icon_size(IconSize::Small)
                        .width(px(24.0))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_group_expanded(&toggle_key, !expanded, cx);
                            this.update_entries(cx);
                            cx.stop_propagation();
                        }))
                        .render(window, cx)
                        .height(px(24.0).into()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .child(Label::new(title).truncate()),
                    )
                    .child(
                        IconButton::new(("new-terminal", index), IconName::Plus)
                            .icon_size(IconSize::Small)
                            .width(px(24.0))
                            .tooltip(Tooltip::localized_text("New Terminal"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_group(new_key.clone(), true, window, cx);
                                cx.stop_propagation();
                            }))
                            .render(window, cx)
                            .height(px(24.0).into()),
                    )
                    .child(
                        PopoverMenu::new(("project-menu", index))
                            .menu({
                                let multi_workspace = self.multi_workspace.clone();
                                move |window, cx| {
                                    let multi_workspace = multi_workspace.clone();
                                    let key = menu_key.clone();
                                    Some(ContextMenu::build(window, cx, move |menu, _, _| {
                                        let up = multi_workspace.clone();
                                        let up_key = key.clone();
                                        let down = multi_workspace.clone();
                                        let down_key = key.clone();
                                        let new_window = multi_workspace.clone();
                                        let window_key = key.clone();
                                        menu.entry("Move Up", None, move |_, cx| {
                                            up.update(cx, |multi_workspace, cx| {
                                                multi_workspace.move_project_group_up(&up_key, cx)
                                            })
                                            .log_err();
                                        })
                                        .localize_last()
                                        .entry("Move Down", None, move |_, cx| {
                                            down.update(cx, |multi_workspace, cx| {
                                                multi_workspace
                                                    .move_project_group_down(&down_key, cx)
                                            })
                                            .log_err();
                                        })
                                        .localize_last()
                                        .entry("Open in New Window", None, move |window, cx| {
                                            new_window
                                                .update(cx, |multi_workspace, cx| {
                                                    multi_workspace
                                                        .open_project_group_in_new_window(
                                                            &window_key,
                                                            window,
                                                            cx,
                                                        )
                                                        .detach_and_log_err(cx)
                                                })
                                                .log_err();
                                        })
                                        .localize_last()
                                        .separator()
                                        .entry("Close Project", None, move |window, cx| {
                                            multi_workspace
                                                .update(cx, |multi_workspace, cx| {
                                                    multi_workspace
                                                        .remove_project_group(&key, window, cx)
                                                        .detach_and_log_err(cx)
                                                })
                                                .log_err();
                                        })
                                        .localize_last()
                                    }))
                                }
                            })
                            .trigger(
                                IconButton::new(("project-options", index), IconName::Ellipsis)
                                    .icon_size(IconSize::Small)
                                    .width(px(24.0))
                                    .render(window, cx)
                                    .height(px(24.0).into()),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.selection = Some(index);
                        this.open_group(key.clone(), false, window, cx);
                    }))
                    .into_any_element()
            }
            Entry::Terminal {
                metadata,
                workspace,
                notified,
                ..
            } => {
                let active = workspace.as_ref().is_some_and(|workspace| {
                    self.multi_workspace
                        .upgrade()
                        .is_some_and(|mw| mw.read(cx).workspace() == workspace)
                }) && workspace
                    .as_ref()
                    .and_then(|workspace| workspace.read(cx).panel::<WorkspaceTerminalPanel>(cx))
                    .is_some_and(|panel| {
                        panel.read(cx).active_terminal_id() == Some(metadata.terminal_id)
                    });
                let close_metadata = metadata.clone();
                let close_workspace = workspace;
                h_flex()
                    .id(("terminal", index))
                    .pl_5()
                    .pr_1()
                    .py_1()
                    .gap_2()
                    .w_full()
                    .when(selected || active, |this| {
                        this.bg(cx.theme().colors().ghost_element_selected)
                    })
                    .hover(|this| this.bg(cx.theme().colors().ghost_element_hover))
                    .child(
                        Icon::new(IconName::Terminal)
                            .size(IconSize::Small)
                            .color(if notified {
                                Color::Accent
                            } else {
                                Color::Muted
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .child(Label::new(metadata.display_title()).truncate()),
                    )
                    .child(
                        IconButton::new(("close-terminal", index), IconName::Close)
                            .icon_size(IconSize::Small)
                            .width(px(24.0))
                            .tooltip(Tooltip::localized_text("Close Terminal"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(panel) =
                                    close_workspace.as_ref().and_then(|workspace| {
                                        workspace.read(cx).panel::<WorkspaceTerminalPanel>(cx)
                                    })
                                {
                                    panel.update(cx, |panel, cx| {
                                        panel.close_terminal(close_metadata.terminal_id, window, cx)
                                    });
                                } else {
                                    TerminalMetadataStore::global(cx).update(cx, |store, cx| {
                                        store.delete(close_metadata.terminal_id, cx)
                                    });
                                }
                                this.update_entries(cx);
                                cx.stop_propagation();
                            }))
                            .render(window, cx)
                            .height(px(24.0).into()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_entry(index, window, cx)
                    }))
                    .into_any_element()
            }
        }
    }

    fn has_filter_query(&self, cx: &App) -> bool {
        !self.filter_editor.read(cx).text(cx).is_empty()
    }
    fn reset_filter_editor_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_editor
            .update(cx, |editor, cx| editor.set_text("", window, cx));
    }
    fn editor_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.entries.is_empty() {
            self.activate_entry(self.selection.unwrap_or(0), window, cx);
        }
    }
    fn is_group_collapsed(&self, key: &ProjectGroupKey, cx: &App) -> bool {
        self.multi_workspace
            .upgrade()
            .and_then(|mw| {
                mw.read(cx)
                    .group_state_by_key(key)
                    .map(|state| !state.expanded)
            })
            .unwrap_or(false)
    }
    fn set_group_expanded(&self, key: &ProjectGroupKey, expanded: bool, cx: &mut Context<Self>) {
        if let Some(mw) = self.multi_workspace.upgrade() {
            mw.update(cx, |mw, cx| {
                if let Some(state) = mw.group_state_by_key_mut(key) {
                    state.expanded = expanded;
                }
                mw.serialize(cx);
            });
        }
    }
    fn render_sidebar_header(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_query = self.has_filter_query(cx);
        let sidebar_on_left = self.side(cx) == SidebarSide::Left;
        let sidebar_on_right = self.side(cx) == SidebarSide::Right;
        let not_fullscreen = !window.is_fullscreen() && !window.is_simple_fullscreen();
        let traffic_lights = cfg!(target_os = "macos") && not_fullscreen && sidebar_on_left;
        let left_window_controls = !cfg!(target_os = "macos") && not_fullscreen && sidebar_on_left;
        let right_window_controls =
            !cfg!(target_os = "macos") && not_fullscreen && sidebar_on_right;
        let header_height = platform_title_bar_height(window);
        let left_controls = left_window_controls
            .then(|| Self::render_left_window_controls(window, cx))
            .flatten();
        let right_controls = right_window_controls
            .then(|| Self::render_right_window_controls(window, cx))
            .flatten();
        let controls_width = if traffic_lights {
            px(ui::utils::TRAFFIC_LIGHT_PADDING)
        } else if cfg!(target_os = "windows") && right_controls.is_some() {
            px(108.)
        } else if left_controls.is_some() || right_controls.is_some() {
            let count = cx.button_layout().map_or(0, |layout| {
                let buttons = if sidebar_on_left {
                    layout.left
                } else {
                    layout.right
                };
                buttons.iter().flatten().count()
            });
            window.rem_size() * (count as f32 * 2. + 0.75)
        } else {
            px(0.)
        };
        let separate_controls = controls_width > px(0.) && self.width - controls_width < px(160.);
        let mut search_controls = Some(
            h_flex()
                .debug_selector(|| "workspace-sidebar-header".to_string())
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(self.render_sidebar_toggle_button(cx))
                .child(self.render_filter_input(cx))
                .child(
                    h_flex()
                        .gap_1()
                        .when(
                            self.width - controls_width >= px(300.)
                                && self.selection.is_some()
                                && !self.filter_editor.focus_handle(cx).is_focused(window),
                            |this| this.child(KeyBinding::for_action(&FocusSidebarFilter, cx)),
                        )
                        .when(has_query, |this| {
                            this.child(
                                IconButton::new("clear_filter", IconName::Close)
                                    .icon_size(IconSize::Medium)
                                    .size(ButtonSize::Medium)
                                    .width(px(28.0))
                                    .tooltip(Tooltip::localized_text("Clear Search"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.reset_filter_editor_text(window, cx);
                                        this.update_entries(cx);
                                    })),
                            )
                        }),
                )
                .child(self.render_recent_projects_button(cx)),
        );

        let header = h_flex()
            .h(header_height)
            .flex_none()
            .map(|header| match window.window_decorations() {
                Decorations::Client { .. } => apply_title_bar_insets(
                    header,
                    left_window_controls,
                    right_window_controls,
                    separate_controls,
                ),
                Decorations::Server => header.mt_px().pb_px(),
            })
            .children(left_controls)
            .map(|this| {
                if traffic_lights {
                    this.pl(px(ui::utils::TRAFFIC_LIGHT_PADDING))
                } else if !left_window_controls {
                    this.pl_1p5()
                } else {
                    this
                }
            })
            .when(!right_window_controls, |this| this.pr_1p5())
            .gap_1()
            .when(!separate_controls, |this| {
                this.border_b_1()
                    .border_color(cx.theme().colors().border)
                    .when(traffic_lights, |this| {
                        this.child(Divider::vertical().color(ui::DividerColor::Border))
                    })
                    .children(search_controls.take())
            })
            .when(separate_controls, |this| this.child(div().flex_1()))
            .children(right_controls);
        v_flex()
            .flex_none()
            .child(header)
            .when_some(search_controls, |this, row| {
                this.child(
                    row.h(header_height)
                        .flex_none()
                        .w_full()
                        .px_1p5()
                        .border_b_1()
                        .border_color(cx.theme().colors().border),
                )
            })
    }
    fn render_left_window_controls(window: &Window, cx: &mut App) -> Option<AnyElement> {
        platform_title_bar::render_left_window_controls(
            cx.button_layout(),
            Box::new(CloseWindow),
            window,
        )
    }
    fn render_right_window_controls(window: &Window, cx: &mut App) -> Option<AnyElement> {
        platform_title_bar::render_right_window_controls(
            cx.button_layout(),
            Box::new(CloseWindow),
            window,
        )
    }
    fn render_sidebar_toggle_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let on_right = self.side(cx) == SidebarSide::Right;

        sidebar_side_context_menu("sidebar-toggle-menu", cx)
            .anchor(if on_right {
                gpui::Anchor::TopRight
            } else {
                gpui::Anchor::TopLeft
            })
            .attach(if on_right {
                gpui::Anchor::BottomRight
            } else {
                gpui::Anchor::BottomLeft
            })
            .trigger(move |_is_active, _window, _cx| {
                let icon = if on_right {
                    IconName::ThreadsSidebarRightOpen
                } else {
                    IconName::ThreadsSidebarLeftOpen
                };
                IconButton::new("sidebar-close-toggle", icon)
                    .icon_size(IconSize::Medium)
                    .size(ButtonSize::Medium)
                    .width(px(28.0))
                    .aria_label("Toggle Sidebar")
                    .tab_index(0isize)
                    .tooltip(Tooltip::element(move |_window, cx| {
                        v_flex()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_between()
                                    .child(Label::localized("Toggle Sidebar"))
                                    .child(KeyBinding::for_action(&ToggleWorkspaceSidebar, cx)),
                            )
                            .child(
                                h_flex()
                                    .pt_1()
                                    .gap_2()
                                    .border_t_1()
                                    .border_color(cx.theme().colors().border_variant)
                                    .justify_between()
                                    .child(Label::localized("Focus Sidebar"))
                                    .child(KeyBinding::for_action(&FocusWorkspaceSidebar, cx)),
                            )
                            .into_any_element()
                    }))
                    .on_click(|_, window, cx| {
                        if let Some(multi_workspace) = window.root::<MultiWorkspace>().flatten() {
                            multi_workspace.update(cx, |multi_workspace, cx| {
                                multi_workspace.close_sidebar(window, cx);
                            });
                        }
                    })
            })
    }
    fn render_filter_input(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .min_w_0()
            .flex_1()
            .capture_action(
                cx.listener(|this, _: &editor::actions::Newline, window, cx| {
                    this.editor_confirm(window, cx);
                }),
            )
            .child(self.filter_editor.clone())
    }
    fn render_recent_projects_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let multi_workspace = self.multi_workspace.upgrade();

        let workspace = multi_workspace
            .as_ref()
            .map(|mw| mw.read(cx).workspace().downgrade());

        let focus_handle = workspace
            .as_ref()
            .and_then(|ws| ws.upgrade())
            .map(|w| w.read(cx).focus_handle(cx))
            .unwrap_or_else(|| cx.focus_handle());

        let window_project_groups: Vec<ProjectGroupKey> = multi_workspace
            .as_ref()
            .map(|mw| mw.read(cx).project_group_keys())
            .unwrap_or_default();

        let popover_handle = self.recent_projects_popover_handle.clone();

        PopoverMenu::new("sidebar-recent-projects-menu")
            .with_handle(popover_handle)
            .menu(move |window, cx| {
                workspace.as_ref().map(|ws| {
                    SidebarRecentProjects::popover(
                        ws.clone(),
                        window_project_groups.clone(),
                        focus_handle.clone(),
                        window,
                        cx,
                    )
                })
            })
            .trigger_with_tooltip(
                IconButton::new("open-project", IconName::FolderAdd)
                    .icon_size(IconSize::Medium)
                    .size(ButtonSize::Medium)
                    .width(px(28.0))
                    .aria_label("Add Project")
                    .tab_index(0isize)
                    .selected_style(ButtonStyle::Tinted(TintColor::Accent)),
                |_window, cx| Tooltip::for_action("Add Project", &OpenRecent::default(), cx),
            )
            .offset(gpui::Point {
                x: px(-2.0),
                y: px(2.0),
            })
            .anchor(gpui::Anchor::TopRight)
            .attach(gpui::Anchor::BottomRight)
    }
}
impl WorkspaceSidebar for Sidebar {
    fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(multi_workspace) = self.multi_workspace.upgrade() {
            let workspace = multi_workspace.read(cx).workspace().clone();
            Self::create_terminal(workspace, window, cx);
        }
    }
    fn width(&self, _: &App) -> Pixels {
        self.width
    }
    fn set_width(&mut self, width: Option<Pixels>, cx: &mut Context<Self>) {
        self.width_set_by_user = width.is_some();
        self.width = width
            .unwrap_or_else(|| WorkspaceSidebarSettings::get_global(cx).default_width)
            .clamp(px(200.0), px(800.0));
        cx.emit(workspace::SidebarEvent::SerializeNeeded);
        cx.notify();
    }
    fn has_notifications(&self, _: &App) -> bool {
        self.entries
            .iter()
            .any(|entry| matches!(entry, Entry::Terminal { notified: true, .. }))
    }
    fn side(&self, cx: &App) -> SidebarSide {
        WorkspaceSidebarSettings::get_global(cx).side()
    }
    fn prepare_for_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selection = None;
        self.filter_editor.focus_handle(cx).focus(window, cx);
    }
    fn toggle_terminal_switcher(
        &mut self,
        select_last: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(switcher) = &self.switcher {
            switcher.update(cx, |switcher, cx| switcher.cycle(select_last, cx));
            return;
        }
        let Some(multi_workspace) = self.multi_workspace.upgrade() else {
            return;
        };
        let current = multi_workspace.read(cx).workspace();
        let active_terminal = current
            .read(cx)
            .panel::<WorkspaceTerminalPanel>(cx)
            .and_then(|panel| panel.read(cx).active_terminal_id());
        let groups = multi_workspace.read(cx).project_group_keys();
        let store = TerminalMetadataStore::global(cx);
        let mut entries: Vec<_> = store
            .read(cx)
            .entries()
            .filter(|metadata| {
                let key = ProjectGroupKey::from_worktree_paths(
                    &metadata.worktree_paths,
                    metadata.remote_connection.clone(),
                );
                groups.iter().any(|group| group.matches(&key))
            })
            .cloned()
            .collect();
        entries.sort_by_key(|metadata| {
            (
                Some(metadata.terminal_id) != active_terminal,
                self.terminal_history
                    .iter()
                    .position(|id| *id == metadata.terminal_id)
                    .unwrap_or(usize::MAX),
                std::cmp::Reverse(metadata.created_at),
            )
        });
        if entries.is_empty() {
            return;
        }
        let previous_focus = window.focused(cx);
        let switcher = cx.new(|cx| TerminalSwitcher::new(entries, select_last, window, cx));
        self.switcher_subscription =
            Some(
                cx.subscribe_in(&switcher, window, move |this, _, event, window, cx| {
                    this.switcher.take();
                    this.switcher_subscription.take();
                    if let Some(multi_workspace) = this.multi_workspace.upgrade() {
                        multi_workspace.update(cx, |multi_workspace, cx| {
                            multi_workspace.set_sidebar_overlay(None, cx)
                        });
                    }
                    match event {
                        TerminalSwitcherEvent::Confirmed(metadata) => {
                            let metadata = metadata.clone();
                            let key = ProjectGroupKey::from_worktree_paths(
                                &metadata.worktree_paths,
                                metadata.remote_connection.clone(),
                            );
                            this.open_terminal_workspace(key, metadata, window, cx);
                        }
                        TerminalSwitcherEvent::Dismissed => {
                            if let Some(focus) = &previous_focus {
                                focus.focus(window, cx);
                            }
                        }
                    }
                }),
            );
        multi_workspace.update(cx, |multi_workspace, cx| {
            multi_workspace.set_sidebar_overlay(Some(switcher.clone().into()), cx)
        });
        switcher.focus_handle(cx).focus(window, cx);
        self.switcher = Some(switcher);
    }
    fn cycle_project(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle(true, forward, window, cx);
    }
    fn cycle_terminal(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle(false, forward, window, cx);
    }
    fn serialized_state(&self, _: &App) -> Option<String> {
        serde_json::to_string(&SerializedSidebar {
            width: self.width_set_by_user.then(|| self.width.into()),
            width_set_by_user: self.width_set_by_user,
        })
        .log_err()
    }
    fn restore_serialized_state(&mut self, state: &str, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(saved) = serde_json::from_str::<SerializedSidebar>(state).log_err()
            && let Some(width) = saved
                .width
                .filter(|width| saved.width_set_by_user || *width != 300.0)
        {
            self.width = px(width).clamp(px(200.0), px(800.0));
            self.width_set_by_user = true;
        }
        cx.notify();
    }
}
impl EventEmitter<workspace::SidebarEvent> for Sidebar {}
impl Focusable for Sidebar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("workspace-sidebar")
            .key_context("WorkspaceSidebar")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .on_action(cx.listener(Self::new_terminal_in_group))
            .on_action(cx.listener(
                |this, _: &terminal_view::workspace_terminal_panel::CloseTerminal, window, cx| {
                    this.close_selected_terminal(window, cx)
                },
            ))
            .on_action(cx.listener(|this, _: &menu::SelectFirst, _, cx| {
                this.selection = (!this.entries.is_empty()).then_some(0);
                this.list_state.scroll_to_reveal_item(0);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &menu::SelectLast, _, cx| {
                this.selection = this.entries.len().checked_sub(1);
                if let Some(index) = this.selection {
                    this.list_state.scroll_to_reveal_item(index);
                }
                cx.notify();
            }))
            .on_action(
                cx.listener(|this, _: &menu::SelectChild, _, cx| this.expand_selected(true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &menu::SelectParent, _, cx| this.expand_selected(false, cx)),
            )
            .on_action(cx.listener(|this, _: &menu::Cancel, window, cx| {
                if this.has_filter_query(cx) {
                    this.reset_filter_editor_text(window, cx);
                } else if let Some(mw) = this.multi_workspace.upgrade() {
                    mw.read(cx).workspace().focus_handle(cx).focus(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FocusSidebarFilter, window, cx| {
                this.filter_editor.focus_handle(cx).focus(window, cx)
            }))
            .on_action(cx.listener(|this, _: &menu::SelectNext, window, cx| {
                this.move_selection(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &menu::SelectPrevious, window, cx| {
                this.move_selection(false, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &menu::Confirm, window, cx| this.editor_confirm(window, cx)),
            )
            .child(self.render_sidebar_header(window, cx))
            .child(
                div().flex_1().overflow_hidden().child(
                    gpui::list(
                        self.list_state.clone(),
                        cx.processor(|this, index, window, cx| {
                            this.render_entry(index, window, cx)
                        }),
                    )
                    .size_full(),
                ),
            )
    }
}
fn connect_remote(
    modal_workspace: Entity<Workspace>,
    connection_options: RemoteConnectionOptions,
    window: &mut Window,
    cx: &mut Context<MultiWorkspace>,
) -> gpui::Task<anyhow::Result<Option<Entity<remote::RemoteClient>>>> {
    remote_connection::connect_with_modal(&modal_workspace, connection_options, window, cx)
}
