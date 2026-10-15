use gpui::{EventEmitter, FocusHandle, Focusable, Modifiers, ModifiersChangedEvent, ScrollHandle};
use terminal_view::terminal_metadata_store::TerminalMetadata;
use ui::prelude::*;
use zed_actions::workspace_sidebar::ToggleTerminalSwitcher;

pub(crate) enum TerminalSwitcherEvent {
    Confirmed(TerminalMetadata),
    Dismissed,
}

pub(crate) struct TerminalSwitcher {
    focus_handle: FocusHandle,
    entries: Vec<TerminalMetadata>,
    selected: usize,
    initial_modifiers: Option<Modifiers>,
    scroll_handle: ScrollHandle,
}

impl TerminalSwitcher {
    #[cfg(test)]
    pub(crate) fn entries(&self) -> &[TerminalMetadata] {
        &self.entries
    }

    pub fn new(
        entries: Vec<TerminalMetadata>,
        reverse: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let selected = if reverse {
            entries.len().saturating_sub(1)
        } else {
            usize::from(entries.len() > 1)
        };
        let focus_handle = cx.focus_handle();
        cx.on_focus_out(&focus_handle, window, |_, _, _, cx| {
            cx.emit(TerminalSwitcherEvent::Dismissed)
        })
        .detach();
        let scroll_handle = ScrollHandle::new();
        scroll_handle.scroll_to_item(selected);
        Self {
            focus_handle,
            entries,
            selected,
            initial_modifiers: window.modifiers().modified().then_some(window.modifiers()),
            scroll_handle,
        }
    }

    pub fn cycle(&mut self, reverse: bool, cx: &mut Context<Self>) {
        if self.entries.is_empty() {
            return;
        }
        self.selected = if reverse {
            (self.selected + self.entries.len() - 1) % self.entries.len()
        } else {
            (self.selected + 1) % self.entries.len()
        };
        self.scroll_handle.scroll_to_item(self.selected);
        cx.notify();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.entries.get(self.selected) {
            cx.emit(TerminalSwitcherEvent::Confirmed(entry.clone()));
        } else {
            cx.emit(TerminalSwitcherEvent::Dismissed);
        }
    }

    fn modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(initial) = self.initial_modifiers
            && (!event.modified() || !initial.is_subset_of(event))
        {
            self.initial_modifiers = None;
            self.confirm(cx);
        }
    }
}

impl EventEmitter<TerminalSwitcherEvent> for TerminalSwitcher {}
impl Focusable for TerminalSwitcher {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
impl Render for TerminalSwitcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("TerminalSwitcher")
            .track_focus(&self.focus_handle)
            .p_1p5()
            .w(rems_from_px(440.0_f32))
            .elevation_3(cx)
            .on_modifiers_changed(cx.listener(Self::modifiers_changed))
            .on_action(cx.listener(|this, _: &menu::Confirm, _, cx| this.confirm(cx)))
            .on_action(
                cx.listener(|_, _: &menu::Cancel, _, cx| cx.emit(TerminalSwitcherEvent::Dismissed)),
            )
            .on_action(cx.listener(|this, action: &ToggleTerminalSwitcher, _, cx| {
                this.cycle(action.select_last, cx)
            }))
            .on_action(cx.listener(|this, _: &menu::SelectNext, _, cx| this.cycle(false, cx)))
            .on_action(cx.listener(|this, _: &menu::SelectPrevious, _, cx| this.cycle(true, cx)))
            .child(
                v_flex()
                    .id("terminal-switcher-list")
                    .max_h_128()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .children(self.entries.iter().enumerate().map(|(index, metadata)| {
                        let project = project::ProjectGroupKey::from_worktree_paths(
                            &metadata.worktree_paths,
                            metadata.remote_connection.clone(),
                        )
                        .display_name(&std::collections::HashMap::new());
                        h_flex()
                            .id(("terminal", index))
                            .p_2()
                            .gap_2()
                            .rounded_md()
                            .when(index == self.selected, |row| {
                                row.bg(cx.theme().colors().element_selected)
                            })
                            .child(Icon::new(IconName::Terminal))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .overflow_hidden()
                                    .child(Label::new(metadata.display_title()).truncate())
                                    .child(
                                        Label::new(project)
                                            .color(Color::Muted)
                                            .size(LabelSize::Small)
                                            .truncate(),
                                    ),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected = index;
                                this.confirm(cx);
                            }))
                    })),
            )
    }
}
