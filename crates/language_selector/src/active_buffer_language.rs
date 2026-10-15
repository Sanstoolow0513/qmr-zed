use editor::Editor;
use gpui::{
    App, Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled, Subscription,
    WeakEntity, Window, div,
};
use language::LanguageName;
use settings::Settings as _;
use ui::{
    ButtonCommon, ButtonLike, ButtonSize, Clickable, FluentBuilder, Label, LabelCommon, LabelSize,
    Tooltip,
};
use workspace::{
    HideStatusItem, StatusBarSettings, StatusItemView, ToolbarItemEvent, ToolbarItemLocation,
    ToolbarItemView, Workspace, item::ItemHandle,
};

use crate::{LanguageSelector, Toggle};

pub struct ActiveBufferLanguage {
    active_language: Option<Option<LanguageName>>,
    workspace: WeakEntity<Workspace>,
    editor: Option<WeakEntity<Editor>>,
    _observe_active_editor: Option<Subscription>,
}

impl ActiveBufferLanguage {
    pub fn new(workspace: &Workspace) -> Self {
        Self {
            active_language: None,
            workspace: workspace.weak_handle(),
            editor: None,
            _observe_active_editor: None,
        }
    }

    fn update_language(&mut self, editor: Entity<Editor>, _: &mut Window, cx: &mut Context<Self>) {
        self.active_language = Some(None);

        let editor = editor.read(cx);
        if let Some(buffer) = editor.active_buffer(cx)
            && let Some(language) = buffer.read(cx).language()
        {
            self.active_language = Some(Some(language.name()));
        }

        cx.notify();
    }

    pub(crate) fn toggle_language(&self, window: &mut Window, cx: &mut App) {
        let Some(editor) = self.editor.as_ref().and_then(WeakEntity::upgrade) else {
            return;
        };
        let Some(buffer) = editor.read(cx).active_buffer(cx) else {
            return;
        };
        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                LanguageSelector::toggle_for_buffer(workspace, buffer, window, cx);
            });
        }
    }
}

impl Render for ActiveBufferLanguage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !StatusBarSettings::get_global(cx).active_language_button {
            return div().hidden();
        }

        div().when_some(self.active_language.as_ref(), |el, active_language| {
            let active_language_text = if let Some(active_language_text) = active_language {
                active_language_text.to_string()
            } else {
                "Unknown".to_string()
            };

            el.child(
                ButtonLike::new("change-language")
                    .child(
                        div().min_w_0().max_w_24().child(
                            Label::new(active_language_text.clone())
                                .size(LabelSize::Small)
                                .truncate(),
                        ),
                    )
                    .size(ButtonSize::Medium)
                    .tab_index(0isize)
                    .aria_label(format!("Language: {active_language_text}"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_language(window, cx);
                    }))
                    .tooltip(move |_window, cx| {
                        Tooltip::for_action(
                            i18n::format(
                                "Select Language: {language}",
                                &[("language", active_language_text.as_str())],
                                cx,
                            ),
                            &Toggle,
                            cx,
                        )
                    }),
            )
        })
    }
}

impl StatusItemView for ActiveBufferLanguage {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = active_pane_item.and_then(|item| item.downcast::<Editor>()) {
            self.editor = Some(editor.downgrade());
            self._observe_active_editor =
                Some(cx.observe_in(&editor, window, Self::update_language));
            self.update_language(editor, window, cx);
        } else {
            self.active_language = None;
            self.editor = None;
            self._observe_active_editor = None;
        }

        cx.notify();
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        Some(HideStatusItem::new(|settings| {
            settings
                .status_bar
                .get_or_insert_default()
                .active_language_button = Some(false);
        }))
    }
}

impl EventEmitter<ToolbarItemEvent> for ActiveBufferLanguage {}

impl ToolbarItemView for ActiveBufferLanguage {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        StatusItemView::set_active_pane_item(self, active_pane_item, window, cx);
        if self.editor.is_some() {
            ToolbarItemLocation::PrimaryRight
        } else {
            ToolbarItemLocation::Hidden
        }
    }
}
