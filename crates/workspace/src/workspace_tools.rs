use crate::{Dock, Workspace, dock::PanelTool};
use gpui::{
    Action, Anchor, AnyView, App, Context, Entity, Render, Subscription, WeakEntity, Window, canvas,
};
use settings::SettingsStore;
use std::sync::Arc;
use ui::{ContextMenu, PopoverMenu, PopoverMenuHandle, Tooltip, prelude::*};
use util::ResultExt;

struct ProjectAction {
    key: &'static str,
    label: SharedString,
    view: AnyView,
    action: Box<dyn Action>,
    enabled: Arc<dyn Fn(&App) -> bool>,
}

struct ProjectStatus {
    view: AnyView,
    visible: Box<dyn Fn(&mut App) -> bool>,
}

pub struct WorkspaceTools {
    workspace: WeakEntity<Workspace>,
    docks: [Entity<Dock>; 3],
    project_actions: Vec<ProjectAction>,
    status_items: Vec<ProjectStatus>,
    menu: PopoverMenuHandle<ContextMenu>,
    width: Pixels,
    _subscriptions: Vec<Subscription>,
}

impl WorkspaceTools {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        docks: [Entity<Dock>; 3],
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions: Vec<_> = docks
            .iter()
            .map(|dock| cx.observe(dock, |_, _, cx| cx.notify()))
            .collect();
        subscriptions.push(cx.observe_global::<SettingsStore>(|_, cx| cx.notify()));
        Self {
            workspace,
            docks,
            project_actions: Vec::new(),
            status_items: Vec::new(),
            menu: Default::default(),
            width: px(300.),
            _subscriptions: subscriptions,
        }
    }

    pub fn add_project_action(
        &mut self,
        key: &'static str,
        label: impl Into<SharedString>,
        view: AnyView,
        action: Box<dyn Action>,
        enabled: Arc<dyn Fn(&App) -> bool>,
        cx: &mut Context<Self>,
    ) {
        self.project_actions.push(ProjectAction {
            key,
            label: label.into(),
            view,
            action,
            enabled,
        });
        cx.notify();
    }

    pub fn add_status_item(
        &mut self,
        view: AnyView,
        visible: impl Fn(&mut App) -> bool + 'static,
        cx: &mut Context<Self>,
    ) {
        self.status_items.push(ProjectStatus {
            view,
            visible: Box::new(visible),
        });
        cx.notify();
    }

    pub fn show_menu(&self, window: &mut Window, cx: &mut App) {
        self.menu.show(window, cx);
    }

    fn panel_tools(&self, window: &Window, cx: &App) -> Vec<PanelTool> {
        self.docks
            .iter()
            .flat_map(|dock| PanelTool::collect(dock, window, cx))
            .collect()
    }

    fn build_menu(
        &self,
        visible_keys: &[&str],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ContextMenu> {
        let tools = self.panel_tools(window, cx);
        let actions: Vec<_> = self
            .project_actions
            .iter()
            .filter(|entry| (entry.enabled)(cx) && !visible_keys.contains(&entry.key))
            .map(|entry| (entry.label.clone(), entry.action.boxed_clone()))
            .collect();
        let statuses: Vec<_> = self
            .status_items
            .iter()
            .filter(|item| (item.visible)(cx))
            .map(|item| item.view.clone())
            .collect();
        ContextMenu::build(window, cx, |mut menu, _, cx| {
            for tool in tools
                .into_iter()
                .filter(|tool| !visible_keys.contains(&tool.key))
            {
                let row_tool = tool.clone();
                menu = menu.custom_entry(
                    move |window, cx| row_tool.render_menu_row(window, cx),
                    move |window, cx| tool.activate(window, cx),
                );
            }
            for (label, action) in actions {
                menu = menu.action(label, action).localize_last();
            }
            if !statuses.is_empty() {
                menu = menu.separator().header(i18n::text("Project Status", cx));
                for view in statuses {
                    menu = menu.custom_row(move |_, _| {
                        div().w_full().child(view.clone()).into_any_element()
                    });
                }
            }
            menu
        })
    }
}

impl Render for WorkspaceTools {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let compact = self
            .workspace
            .upgrade()
            .is_none_or(|workspace| workspace.read(cx).tools_visible_in_titlebar(cx));
        let tools = self.panel_tools(window, cx);
        let available_keys: Vec<_> = tools
            .iter()
            .map(|tool| tool.key)
            .chain(
                self.project_actions
                    .iter()
                    .filter(|action| (action.enabled)(cx))
                    .map(|action| action.key),
            )
            .collect();
        let scale = f32::from(window.rem_size()) / 16.;
        let keys = primary_keys(self.width / scale, compact, &available_keys);
        let mut children = Vec::new();
        for key in &keys {
            if let Some(tool) = tools.iter().find(|tool| tool.key == *key) {
                children.push(tool.render(true, window, cx));
            } else if let Some(action) = self
                .project_actions
                .iter()
                .find(|action| action.key == *key)
            {
                children.push(
                    div()
                        .flex_none()
                        .child(action.view.clone())
                        .into_any_element(),
                );
            }
        }
        let weak = cx.weak_entity();
        let menu_keys = keys.clone();
        let menu = PopoverMenu::new("workspace-tools-menu")
            .with_handle(self.menu.clone())
            .anchor(Anchor::TopRight)
            .menu(move |window, cx| {
                weak.update(cx, |this, cx| this.build_menu(&menu_keys, window, cx))
                    .log_err()
            })
            .trigger_with_tooltip(
                IconButton::new(
                    "workspace-tools-toggle",
                    if compact {
                        IconName::Menu
                    } else {
                        IconName::Ellipsis
                    },
                )
                .icon_size(IconSize::Medium)
                .size(ButtonSize::Medium)
                .width(rems_from_px(28_f32))
                .tab_index(0isize)
                .aria_label(i18n::text(if compact { "Tools" } else { "More" }, cx)),
                Tooltip::localized_text(if compact { "Tools" } else { "More" }),
            );
        let measured = cx.entity().downgrade();
        h_flex()
            .id("workspace-tools")
            .role(gpui::Role::Toolbar)
            .aria_label(i18n::text("Tools", cx))
            .tab_group()
            .relative()
            .gap_1()
            .flex_none()
            .when(!compact, |this| {
                this.w_full()
                    .h_8()
                    .px_1()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
            })
            .children(children)
            .child(menu)
            .when(!compact, |this| {
                this.child(
                    canvas(
                        move |bounds, _, cx| {
                            measured
                                .update(cx, |this, cx| {
                                    if this.width != bounds.size.width {
                                        this.width = bounds.size.width;
                                        cx.notify();
                                    }
                                })
                                .log_err();
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
            })
    }
}

fn primary_keys(
    width: Pixels,
    compact: bool,
    available_keys: &[&'static str],
) -> Vec<&'static str> {
    if compact {
        return Vec::new();
    }
    let count = ((f32::from(width) - 40.) / 32.).floor().max(1.) as usize;
    [
        "ProjectPanel",
        "project-search",
        "project-diagnostics",
        "GitPanel",
        "OutlinePanel",
    ]
    .into_iter()
    .filter(|key| available_keys.contains(key))
    .take(count.min(5))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn navigation_overflow_keeps_file_entry_and_moves_secondary_tools_first() {
        let available_keys = [
            "ProjectPanel",
            "project-search",
            "project-diagnostics",
            "GitPanel",
            "OutlinePanel",
        ];
        assert_eq!(
            primary_keys(px(300.), false, &available_keys),
            available_keys
        );
        assert_eq!(
            primary_keys(px(170.), false, &available_keys),
            vec![
                "ProjectPanel",
                "project-search",
                "project-diagnostics",
                "GitPanel"
            ]
        );
        assert_eq!(
            primary_keys(px(80.), false, &available_keys),
            vec!["ProjectPanel"]
        );
        assert!(primary_keys(px(300.), true, &available_keys).is_empty());
    }

    #[test]
    fn hidden_tools_do_not_consume_navigation_space() {
        let available_keys = ["ProjectPanel", "GitPanel", "OutlinePanel"];
        assert_eq!(
            primary_keys(px(140.), false, &available_keys),
            available_keys
        );
        assert_eq!(
            primary_keys(px(80.), false, &["GitPanel"]),
            vec!["GitPanel"]
        );
        assert!(primary_keys(px(300.), false, &[]).is_empty());
    }
}
