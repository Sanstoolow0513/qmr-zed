mod preview;
mod repl_menu;

use editor::actions::{
    AddSelectionAbove, AddSelectionBelow, CodeActionSource, DuplicateLineDown, GoToDiagnostic,
    GoToHunk, GoToPreviousDiagnostic, GoToPreviousHunk, MoveLineDown, MoveLineUp, SelectAll,
    SelectLargerSyntaxNode, SelectNext, SelectSmallerSyntaxNode, ToggleCodeActions,
    ToggleDiagnostics, ToggleGoToLine, ToggleInlineDiagnostics,
};
use editor::code_context_menus::{CodeContextMenu, ContextMenuOrigin};
use editor::{Editor, EditorSettings};
use gpui::{
    Action, Anchor, AnchoredPositionMode, AnyElement, ClickEvent, Context, ElementId, Entity,
    EventEmitter, FocusHandle, Focusable, InteractiveElement, ParentElement, Render, Styled,
    Subscription, WeakEntity, Window, anchored, deferred, point,
};
use project::project_settings::{DiagnosticSeverity, ProjectSettings};
use search::{BufferSearchBar, buffer_search};
use settings::{GitDiffBaseSetting, Settings, SettingsStore, update_settings_file};
use ui::{
    ButtonStyle, ContextMenu, ContextMenuEntry, DocumentationSide, IconButton, IconName, IconSize,
    PopoverMenu, PopoverMenuHandle, Tooltip, prelude::*,
};
use vim_mode_setting::{HelixModeSetting, VimModeSetting};
use workspace::item::ItemBufferKind;
use workspace::{
    ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView, Workspace, item::ItemHandle,
};
use zed_actions::outline::ToggleOutline;

const MAX_CODE_ACTION_MENU_LINES: u32 = 16;

pub struct QuickActionBar {
    _inlay_hints_enabled_subscription: Option<Subscription>,

    active_item: Option<Box<dyn ItemHandle>>,
    buffer_search_bar: Entity<BufferSearchBar>,
    show: bool,
    toggle_selections_handle: PopoverMenuHandle<ContextMenu>,
    toggle_settings_handle: PopoverMenuHandle<ContextMenu>,
    overflow_handle: PopoverMenuHandle<ContextMenu>,
    compact: bool,
    workspace: WeakEntity<Workspace>,
}

impl QuickActionBar {
    fn dispatch_editor_action(
        editor: &WeakEntity<Editor>,
        action: Box<dyn Action>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(editor) = editor.upgrade() else {
            return;
        };
        if let Some(workspace) = editor.read(cx).workspace() {
            workspace.update(cx, |workspace, cx| {
                workspace.activate_item(&editor, true, true, window, cx);
            });
        }
        editor
            .focus_handle(cx)
            .dispatch_action(action.as_ref(), window, cx);
    }

    pub fn new(
        buffer_search_bar: Entity<BufferSearchBar>,
        workspace: &Workspace,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            _inlay_hints_enabled_subscription: None,

            active_item: None,
            buffer_search_bar,
            show: true,
            toggle_selections_handle: Default::default(),
            toggle_settings_handle: Default::default(),
            overflow_handle: Default::default(),
            compact: false,
            workspace: workspace.weak_handle(),
        };
        this.apply_settings(cx);
        cx.observe_global::<SettingsStore>(|this, cx| this.apply_settings(cx))
            .detach();
        this
    }

    fn active_editor(&self) -> Option<Entity<Editor>> {
        self.active_item
            .as_ref()
            .and_then(|item| item.downcast::<Editor>())
    }

    fn apply_settings(&mut self, cx: &mut Context<Self>) {
        let new_show = EditorSettings::get_global(cx).toolbar.quick_actions;
        if new_show != self.show {
            self.show = new_show;
            cx.emit(ToolbarItemEvent::ChangeLocation(
                self.get_toolbar_item_location(),
            ));
        }
    }

    fn get_toolbar_item_location(&self) -> ToolbarItemLocation {
        if self.show && self.active_editor().is_some() {
            ToolbarItemLocation::PrimaryRight
        } else {
            ToolbarItemLocation::Hidden
        }
    }
}

impl QuickActionBar {
    fn render_editing_controls(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(editor) = self.active_editor() else {
            return div().into_any_element();
        };
        let editor_value = editor.read(cx);
        let selection_menu_enabled = editor_value.selection_menu_enabled(cx);
        let has_available_code_actions = editor_value.has_available_code_actions_for_selection();
        let code_action_enabled = editor_value.code_actions_enabled_for_toolbar(cx);
        let code_actions_dropdown = code_action_enabled.then(|| {
            let is_deployed = {
                let menu_ref = editor.read(cx).context_menu().borrow();
                let code_action_menu = menu_ref
                    .as_ref()
                    .filter(|menu| matches!(menu, CodeContextMenu::CodeActions(..)));
                code_action_menu
                    .as_ref()
                    .is_some_and(|menu| matches!(menu.origin(), ContextMenuOrigin::QuickActionBar))
            };
            let code_action_element = is_deployed
                .then(|| {
                    editor.update(cx, |editor, cx| {
                        editor.render_context_menu(MAX_CODE_ACTION_MENU_LINES, window, cx)
                    })
                })
                .flatten();
            v_flex()
                .child(
                    IconButton::new("toggle_code_actions_icon", IconName::BoltOutlined)
                        .icon_size(IconSize::Medium)
                        .size(ButtonSize::Medium)
                        .width(rems_from_px(28_f32))
                        .style(ButtonStyle::Subtle)
                        .disabled(!has_available_code_actions)
                        .toggle_state(is_deployed)
                        .when(!is_deployed, |this| {
                            this.when(has_available_code_actions, |this| {
                                this.tooltip(Tooltip::localized_for_action_title(
                                    "Code Actions",
                                    &ToggleCodeActions::default(),
                                ))
                            })
                            .when(
                                !has_available_code_actions,
                                |this| {
                                    this.tooltip(Tooltip::localized_for_action_title(
                                        "No Code Actions Available",
                                        &ToggleCodeActions::default(),
                                    ))
                                },
                            )
                        })
                        .on_click({
                            let editor = editor.clone();
                            move |_, window, cx| {
                                editor.update(cx, |editor, cx| {
                                    editor.toggle_code_actions(
                                        &ToggleCodeActions {
                                            deployed_from: Some(CodeActionSource::QuickActionBar),
                                            quick_launch: false,
                                        },
                                        window,
                                        cx,
                                    );
                                })
                            }
                        }),
                )
                .children(code_action_element.map(|menu| {
                    deferred(
                        anchored()
                            .position_mode(AnchoredPositionMode::Local)
                            .position(point(px(20.), px(20.)))
                            .anchor(Anchor::TopRight)
                            .child(menu),
                    )
                }))
        });

        let editor_selections_dropdown = selection_menu_enabled.then(|| {
            let has_diff_hunks = editor
                .read(cx)
                .buffer()
                .read(cx)
                .snapshot(cx)
                .has_diff_hunks();
            let _has_selection = editor.update(cx, |editor, cx| {
                editor.has_non_empty_selection(&editor.display_snapshot(cx))
            });

            let focus = editor.focus_handle(cx);

            PopoverMenu::new("editor-selections-dropdown")
                .trigger_with_tooltip(
                    IconButton::new("toggle_editor_selections_icon", IconName::CursorIBeam)
                        .icon_size(IconSize::Medium)
                        .size(ButtonSize::Medium)
                        .width(rems_from_px(28_f32))
                        .style(ButtonStyle::Subtle)
                        .toggle_state(self.toggle_selections_handle.is_deployed()),
                    Tooltip::localized_text("Selection Controls"),
                )
                .with_handle(self.toggle_selections_handle.clone())
                .anchor(Anchor::TopRight)
                .menu(move |window, cx| {
                    let focus = focus.clone();
                    let menu = ContextMenu::build(window, cx, move |menu, _, _| {
                        menu.context(focus.clone())
                            .action("Select All", Box::new(SelectAll))
                            .localize_last()
                            .action(
                                "Select Next Occurrence",
                                Box::new(SelectNext {
                                    replace_newest: false,
                                }),
                            )
                            .localize_last()
                            .action("Expand Selection", Box::new(SelectLargerSyntaxNode))
                            .localize_last()
                            .action("Shrink Selection", Box::new(SelectSmallerSyntaxNode))
                            .localize_last()
                            .action(
                                "Add Cursor Above",
                                Box::new(AddSelectionAbove {
                                    skip_soft_wrap: true,
                                }),
                            )
                            .localize_last()
                            .action(
                                "Add Cursor Below",
                                Box::new(AddSelectionBelow {
                                    skip_soft_wrap: true,
                                }),
                            )
                            .localize_last()
                            .separator()
                            .action("Go to Symbol", Box::new(ToggleOutline))
                            .localize_last()
                            .action("Go to Line/Column", Box::new(ToggleGoToLine))
                            .localize_last()
                            .separator()
                            .action("Next Problem", Box::new(GoToDiagnostic::default()))
                            .localize_last()
                            .action(
                                "Previous Problem",
                                Box::new(GoToPreviousDiagnostic::default()),
                            )
                            .localize_last()
                            .separator()
                            .action_disabled_when(!has_diff_hunks, "Next Hunk", Box::new(GoToHunk))
                            .action_disabled_when(
                                !has_diff_hunks,
                                "Previous Hunk",
                                Box::new(GoToPreviousHunk),
                            )
                            .separator()
                            .action("Move Line Up", Box::new(MoveLineUp))
                            .localize_last()
                            .action("Move Line Down", Box::new(MoveLineDown))
                            .localize_last()
                            .action("Duplicate Selection", Box::new(DuplicateLineDown))
                            .localize_last()
                    });
                    Some(menu)
                })
        });

        h_flex()
            .gap(DynamicSpacing::Base01.rems(cx))
            .children(code_actions_dropdown)
            .children(editor_selections_dropdown)
            .into_any_element()
    }
}

impl Render for QuickActionBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(editor) = self.active_editor() else {
            return div().id("empty quick action bar");
        };

        let supports_inlay_hints = editor.update(cx, |editor, cx| editor.supports_inlay_hints(cx));
        let supports_semantic_tokens =
            editor.update(cx, |editor, cx| editor.supports_semantic_tokens(cx));
        let supports_code_lens = editor.update(cx, |editor, cx| editor.supports_code_lens(cx));
        let editor_value = editor.read(cx);
        let selection_menu_enabled = editor_value.selection_menu_enabled(cx);
        let inlay_hints_enabled = editor_value.inlay_hints_enabled();
        let inline_values_enabled = editor_value.inline_values_enabled();
        let semantic_highlights_enabled = editor_value.semantic_highlights_enabled();
        let code_lens_enabled = editor_value.code_lens_enabled();
        let is_full = editor_value.mode().is_full();
        let diagnostics_enabled = editor_value.diagnostics_enabled()
            && editor_value.diagnostics_max_severity != DiagnosticSeverity::Off;
        let supports_inline_diagnostics = editor_value.inline_diagnostics_enabled();
        let inline_diagnostics_enabled = editor_value.show_inline_diagnostics();
        let git_blame_inline_enabled = editor_value.git_blame_inline_enabled();
        let show_git_blame_gutter = editor_value.show_git_blame_gutter();
        let auto_signature_help_enabled = editor_value.auto_signature_help_enabled(cx);
        let show_line_numbers = editor_value.line_numbers_enabled(cx);

        let supports_minimap = editor_value.supports_minimap(cx);
        let minimap_enabled = supports_minimap && editor_value.minimap().is_some();
        let focus_handle = editor_value.focus_handle(cx);

        let search_button = (editor.buffer_kind(cx) == ItemBufferKind::Singleton).then(|| {
            QuickActionBarButton::new(
                "toggle buffer search",
                search::SEARCH_ICON,
                !self.buffer_search_bar.read(cx).is_dismissed(),
                Box::new(buffer_search::Deploy::find()),
                focus_handle.clone(),
                "Buffer Search",
                {
                    let buffer_search_bar = self.buffer_search_bar.clone();
                    move |_, window, cx| {
                        buffer_search_bar.update(cx, |search_bar, cx| {
                            search_bar.toggle(&buffer_search::Deploy::find(), window, cx)
                        });
                    }
                },
            )
        });

        let compact = self.compact;
        let file_status = editor_value.active_buffer(cx).map(|buffer| {
            let buffer = buffer.read(cx);
            (buffer.encoding().name(), buffer.line_ending().label())
        });
        let editor_focus_handle = editor.focus_handle(cx);
        let editor = editor.downgrade();
        let tooltip_editor = editor.clone();
        let editor_settings_dropdown = {
            let vim_mode_enabled = VimModeSetting::get_global(cx).0;
            let helix_mode_enabled = HelixModeSetting::get_global(cx).0;
            let diff_against_default_branch =
                ProjectSettings::get_global(cx).git.diff_base == GitDiffBaseSetting::DefaultBranch;
            let fs = self
                .workspace
                .upgrade()
                .map(|workspace| workspace.read(cx).app_state().fs.clone());

            PopoverMenu::new("editor-settings")
                .trigger_with_tooltip(
                    IconButton::new("toggle_editor_settings_icon", IconName::Filter)
                        .icon_size(IconSize::Medium)
                        .size(ButtonSize::Medium)
                        .width(rems_from_px(28_f32))
                        .toggle_state(self.toggle_settings_handle.is_deployed()),
                    move |_, cx| {
                        let status = tooltip_editor.upgrade().and_then(|editor| {
                            vim::ModeIndicator::editor_status(editor.read(cx), cx)
                        });
                        Tooltip::simple(
                            status.map_or_else(
                                || i18n::static_text("Editor Controls", cx).to_string(),
                                |status| format!("{} · {status}", i18n::static_text("Editor Controls", cx)),
                            ),
                            cx,
                        )
                    },
                )
                .anchor(Anchor::TopRight)
                .with_handle(self.toggle_settings_handle.clone())
                .menu(move |window, cx| {
                    let menu = ContextMenu::build(window, cx, {
                        let focus_handle = editor_focus_handle.clone();
                        |mut menu, _, cx| {
                            menu = menu.context(focus_handle);

                            if let Some((encoding, line_ending)) = file_status {
                                for (label, action) in [
                                    (i18n::format("Encoding: {encoding}", &[("encoding", encoding)], cx).to_string(), Box::new(encoding_selector::Toggle) as Box<dyn Action>),
                                    (i18n::format("Line Endings: {line_ending}", &[("line_ending", line_ending)], cx).to_string(), Box::new(line_ending_selector::Toggle) as Box<dyn Action>),
                                    (i18n::static_text("Select Toolchain", cx).to_string(), Box::new(toolchain_selector::Select) as Box<dyn Action>),
                                ] {
                                    let editor = editor.clone();
                                    menu = menu.entry(label, Some(action.boxed_clone()), move |window, cx| {
                                        Self::dispatch_editor_action(&editor, action.boxed_clone(), window, cx);
                                    });
                                }
                                menu = menu.separator();
                            }

                            if let Some(status) = editor.upgrade().and_then(|editor| {
                                vim::ModeIndicator::editor_status(editor.read(cx), cx)
                            }) {
                                menu = menu.label(status).separator();
                            }

                            if supports_inlay_hints {
                                menu = menu.toggleable_entry(
                                    "Inlay Hints",
                                    inlay_hints_enabled,
                                    IconPosition::Start,
                                    Some(editor::actions::ToggleInlayHints.boxed_clone()),
                                    {
                                        let editor = editor.clone();
                                        move |window, cx| {
                                            editor
                                                .update(cx, |editor, cx| {
                                                    editor.toggle_inlay_hints(
                                                        &editor::actions::ToggleInlayHints,
                                                        window,
                                                        cx,
                                                    );
                                                })
                                                .ok();
                                        }
                                    },
                                ).localize_last();

                                menu = menu.toggleable_entry(
                                    "Inline Values",
                                    inline_values_enabled,
                                    IconPosition::Start,
                                    Some(editor::actions::ToggleInlineValues.boxed_clone()),
                                    {
                                        let editor = editor.clone();
                                        move |window, cx| {
                                            editor
                                                .update(cx, |editor, cx| {
                                                    editor.toggle_inline_values(
                                                        &editor::actions::ToggleInlineValues,
                                                        window,
                                                        cx,
                                                    );
                                                })
                                                .ok();
                                        }
                                    }
                                ).localize_last();
                            }

                            if supports_semantic_tokens {
                                menu = menu.toggleable_entry(
                                    "Semantic Highlights",
                                    semantic_highlights_enabled,
                                    IconPosition::Start,
                                    Some(editor::actions::ToggleSemanticHighlights.boxed_clone()),
                                    {
                                        let editor = editor.clone();
                                        move |window, cx| {
                                            editor
                                                .update(cx, |editor, cx| {
                                                    editor.toggle_semantic_highlights(
                                                        &editor::actions::ToggleSemanticHighlights,
                                                        window,
                                                        cx,
                                                    );
                                                })
                                                .ok();
                                        }
                                    },
                                ).localize_last();
                            }

                            if supports_code_lens {
                                menu = menu.toggleable_entry(
                                    "Code Lens",
                                    code_lens_enabled,
                                    IconPosition::Start,
                                    Some(editor::actions::ToggleCodeLens.boxed_clone()),
                                    {
                                        let editor = editor.clone();
                                        move |window, cx| {
                                            editor
                                                .update(cx, |editor, cx| {
                                                    editor.toggle_code_lens_action(
                                                        &editor::actions::ToggleCodeLens,
                                                        window,
                                                        cx,
                                                    );
                                                })
                                                .ok();
                                        }
                                    },
                                ).localize_last();
                            }

                            if supports_minimap {
                                menu = menu.toggleable_entry("Minimap", minimap_enabled, IconPosition::Start, Some(editor::actions::ToggleMinimap.boxed_clone()), {
                                    let editor = editor.clone();
                                    move |window, cx| {
                                        editor
                                            .update(cx, |editor, cx| {
                                                editor.toggle_minimap(
                                                    &editor::actions::ToggleMinimap,
                                                    window,
                                                    cx,
                                                );
                                            })
                                            .ok();
                                    }
                                },).localize_last()
                            }



                            menu = menu.separator();

                            if is_full {
                                menu = menu.toggleable_entry(
                                    "Diagnostics",
                                    diagnostics_enabled,
                                    IconPosition::Start,
                                    Some(ToggleDiagnostics.boxed_clone()),
                                    {
                                        let editor = editor.clone();
                                        move |window, cx| {
                                            editor
                                                .update(cx, |editor, cx| {
                                                    editor.toggle_diagnostics(
                                                        &ToggleDiagnostics,
                                                        window,
                                                        cx,
                                                    );
                                                })
                                                .ok();
                                        }
                                    },
                                ).localize_last();

                                if supports_inline_diagnostics {
                                    let mut inline_diagnostics_item = ContextMenuEntry::localized("Inline Diagnostics")
                                        .toggleable(IconPosition::Start, diagnostics_enabled && inline_diagnostics_enabled)
                                        .action(ToggleInlineDiagnostics.boxed_clone())
                                        .handler({
                                            let editor = editor.clone();
                                            move |window, cx| {
                                                editor
                                                    .update(cx, |editor, cx| {
                                                        editor.toggle_inline_diagnostics(
                                                            &ToggleInlineDiagnostics,
                                                            window,
                                                            cx,
                                                        );
                                                    })
                                                    .ok();
                                            }
                                        });
                                    if !diagnostics_enabled {
                                        inline_diagnostics_item = inline_diagnostics_item.disabled(true).documentation_aside(DocumentationSide::Left, |_|  Label::localized("Inline diagnostics are not available until regular diagnostics are enabled.").into_any_element());
                                    }
                                    menu = menu.item(inline_diagnostics_item)
                                }

                                menu = menu.separator();
                            }

                            menu = menu.toggleable_entry(
                                "Line Numbers",
                                show_line_numbers,
                                IconPosition::Start,
                                Some(editor::actions::ToggleLineNumbers.boxed_clone()),
                                {
                                    let editor = editor.clone();
                                    move |window, cx| {
                                        editor
                                            .update(cx, |editor, cx| {
                                                editor.toggle_line_numbers(
                                                    &editor::actions::ToggleLineNumbers,
                                                    window,
                                                    cx,
                                                );
                                            })
                                            .ok();
                                    }
                                },
                            ).localize_last();

                            menu = menu.toggleable_entry(
                                "Selection Menu",
                                selection_menu_enabled,
                                IconPosition::Start,
                                Some(editor::actions::ToggleSelectionMenu.boxed_clone()),
                                {
                                    let editor = editor.clone();
                                    move |window, cx| {
                                        editor
                                            .update(cx, |editor, cx| {
                                                editor.toggle_selection_menu(
                                                    &editor::actions::ToggleSelectionMenu,
                                                    window,
                                                    cx,
                                                )
                                            })
                                            .ok();
                                    }
                                },
                            ).localize_last();

                            menu = menu.toggleable_entry(
                                "Auto Signature Help",
                                auto_signature_help_enabled,
                                IconPosition::Start,
                                Some(editor::actions::ToggleAutoSignatureHelp.boxed_clone()),
                                {
                                    let editor = editor.clone();
                                    move |window, cx| {
                                        editor
                                            .update(cx, |editor, cx| {
                                                editor.toggle_auto_signature_help_menu(
                                                    &editor::actions::ToggleAutoSignatureHelp,
                                                    window,
                                                    cx,
                                                );
                                            })
                                            .ok();
                                    }
                                },
                            ).localize_last();

                            menu = menu.separator();

                            menu = menu.toggleable_entry(
                                "Inline Git Blame",
                                git_blame_inline_enabled,
                                IconPosition::Start,
                                Some(editor::actions::ToggleGitBlameInline.boxed_clone()),
                                {
                                    let editor = editor.clone();
                                    move |window, cx| {
                                        editor
                                            .update(cx, |editor, cx| {
                                                editor.toggle_git_blame_inline(
                                                    &editor::actions::ToggleGitBlameInline,
                                                    window,
                                                    cx,
                                                )
                                            })
                                            .ok();
                                    }
                                },
                            ).localize_last();

                            menu = menu.toggleable_entry(
                                "Column Git Blame",
                                show_git_blame_gutter,
                                IconPosition::Start,
                                Some(git::Blame.boxed_clone()),
                                {
                                    let editor = editor.clone();
                                    move |window, cx| {
                                        editor
                                            .update(cx, |editor, cx| {
                                                editor.toggle_git_blame(
                                                    &git::Blame,
                                                    window,
                                                    cx,
                                                )
                                            })
                                            .ok();
                                    }
                                },
                            ).localize_last();

                            if let Some(fs) = fs.clone() {
                                menu = menu.toggleable_entry(
                                    "Diff Against Default Branch",
                                    diff_against_default_branch,
                                    IconPosition::Start,
                                    None,
                                    {
                                        move |_window, cx| {
                                            let diff_base = if diff_against_default_branch {
                                                GitDiffBaseSetting::Head
                                            } else {
                                                GitDiffBaseSetting::DefaultBranch
                                            };
                                            update_settings_file(fs.clone(), cx, move |settings, _| {
                                                settings.git.get_or_insert_default().diff_base =
                                                    Some(diff_base);
                                            });
                                        }
                                    },
                                ).localize_last();
                            }

                            menu = menu.separator();

                            menu = menu.toggleable_entry(
                                "Vim Mode",
                                vim_mode_enabled,
                                IconPosition::Start,
                                None,
                                {
                                    move |window, cx| {
                                        let new_value = !vim_mode_enabled;
                                        VimModeSetting::override_global(VimModeSetting(new_value), cx);
                                        HelixModeSetting::override_global(HelixModeSetting(false), cx);
                                        window.refresh();
                                    }
                                },
                            ).localize_last();
                            menu = menu.toggleable_entry(
                                "Helix Mode",
                                helix_mode_enabled,
                                IconPosition::Start,
                                None,
                                {
                                    move |window, cx| {
                                        let new_value = !helix_mode_enabled;
                                        HelixModeSetting::override_global(HelixModeSetting(new_value), cx);
                                        VimModeSetting::override_global(VimModeSetting(false), cx);
                                        window.refresh();
                                    }
                                }
                            ).localize_last();

                            menu
                        }
                    });
                    Some(menu)
                })
        };

        h_flex()
            .id("quick action bar")
            .gap(DynamicSpacing::Base01.rems(cx))
            .border_l_1()
            .border_color(cx.theme().colors().border_variant)
            .ml_1()
            .pl_1()
            .when(!compact, |this| {
                this.children(self.render_repl_menu(cx))
                    .children(self.render_preview_button(cx))
            })
            .children(search_button)
            .when(!compact, |this| {
                this.child(self.render_editing_controls(window, cx))
            })
            .when(compact, |this| {
                let bar = cx.weak_entity();
                this.child(
                    PopoverMenu::new("editor-actions-overflow")
                        .trigger_with_tooltip(
                            IconButton::new("editor-actions-overflow-button", IconName::Ellipsis)
                                .icon_size(IconSize::Medium)
                                .size(ButtonSize::Medium)
                                .width(rems_from_px(28_f32))
                                .toggle_state(self.overflow_handle.is_deployed()),
                            Tooltip::localized_text("More Editor Actions"),
                        )
                        .with_handle(self.overflow_handle.clone())
                        .anchor(Anchor::TopRight)
                        .menu(move |window, cx| {
                            let bar = bar.clone();
                            Some(ContextMenu::build(window, cx, |menu, _, _| {
                                menu.custom_row(move |window, cx| {
                                    bar.update(cx, |bar, cx| {
                                        h_flex()
                                            .gap_1()
                                            .children(bar.render_repl_menu(cx))
                                            .children(bar.render_preview_button(cx))
                                            .child(bar.render_editing_controls(window, cx))
                                            .into_any_element()
                                    })
                                    .unwrap_or_else(|_| div().into_any_element())
                                })
                            }))
                        }),
                )
            })
            .child(editor_settings_dropdown)
    }
}

impl EventEmitter<ToolbarItemEvent> for QuickActionBar {}

#[derive(IntoElement)]
struct QuickActionBarButton {
    id: ElementId,
    icon: IconName,
    toggled: bool,
    action: Box<dyn Action>,
    focus_handle: FocusHandle,
    tooltip: SharedString,
    on_click: Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
}

impl QuickActionBarButton {
    fn new(
        id: impl Into<ElementId>,
        icon: IconName,
        toggled: bool,
        action: Box<dyn Action>,
        focus_handle: FocusHandle,
        tooltip: impl Into<SharedString>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            icon,
            toggled,
            action,
            focus_handle,
            tooltip: tooltip.into(),
            on_click: Box::new(on_click),
        }
    }
}

impl RenderOnce for QuickActionBarButton {
    fn render(self, _window: &mut Window, _: &mut App) -> impl IntoElement {
        let tooltip = self.tooltip.clone();
        let action = self.action.boxed_clone();

        IconButton::new(self.id.clone(), self.icon)
            .icon_size(IconSize::Medium)
            .size(ButtonSize::Medium)
            .width(rems_from_px(28_f32))
            .style(ButtonStyle::Subtle)
            .toggle_state(self.toggled)
            .tooltip(move |_window, cx| {
                Tooltip::for_action_in(tooltip.clone(), &*action, &self.focus_handle, cx)
            })
            .on_click(move |event, window, cx| (self.on_click)(event, window, cx))
    }
}

impl ToolbarItemView for QuickActionBar {
    fn available_width_changed(
        &mut self,
        width: gpui::Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let compact = width < rems(26.).to_pixels(window.rem_size());
        if compact != self.compact {
            self.compact = compact;
            cx.notify();
        }
    }

    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        self.active_item = active_pane_item.map(ItemHandle::boxed_clone);
        if let Some(active_item) = active_pane_item {
            self._inlay_hints_enabled_subscription.take();

            if let Some(editor) = active_item.downcast::<Editor>() {
                let (
                    mut inlay_hints_enabled,
                    mut supports_inlay_hints,
                    mut supports_semantic_tokens,
                ) = editor.update(cx, |editor, cx| {
                    (
                        editor.inlay_hints_enabled(),
                        editor.supports_inlay_hints(cx),
                        editor.supports_semantic_tokens(cx),
                    )
                });
                self._inlay_hints_enabled_subscription =
                    Some(cx.observe(&editor, move |_, editor, cx| {
                        let (
                            new_inlay_hints_enabled,
                            new_supports_inlay_hints,
                            new_supports_semantic_tokens,
                        ) = editor.update(cx, |editor, cx| {
                            (
                                editor.inlay_hints_enabled(),
                                editor.supports_inlay_hints(cx),
                                editor.supports_semantic_tokens(cx),
                            )
                        });
                        let should_notify = inlay_hints_enabled != new_inlay_hints_enabled
                            || supports_inlay_hints != new_supports_inlay_hints
                            || supports_semantic_tokens != new_supports_semantic_tokens;
                        inlay_hints_enabled = new_inlay_hints_enabled;
                        supports_inlay_hints = new_supports_inlay_hints;
                        supports_semantic_tokens = new_supports_semantic_tokens;
                        if should_notify {
                            cx.notify()
                        }
                    }));
            }
        }
        self.get_toolbar_item_location()
    }
}
