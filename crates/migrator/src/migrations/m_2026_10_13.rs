use anyhow::Result;
use gpui::KeyBindingContextPredicate;
use serde_json::{Map, Value};

pub fn migrate_workspace_terminals(value: &mut Value) -> Result<()> {
    super::migrate_settings(value, &mut |settings| {
        if let Some(agent) = settings.get("agent").and_then(Value::as_object).cloned() {
            if let Some(sidebar) = agent.get("threads_sidebar").and_then(Value::as_object) {
                merge_missing(settings, "workspace_sidebar", sidebar.clone());
            }
            let mut terminal = Map::new();
            for (old, new) in [
                ("button", "button"),
                ("dock", "dock"),
                ("flexible", "flexible"),
                ("default_width", "default_width"),
                ("terminal_init_command", "init_command"),
                ("notify_when_agent_waiting", "notify_on_bell"),
                ("play_sound_when_agent_done", "play_sound_on_bell"),
            ] {
                if let Some(value) = agent.get(old) {
                    terminal.insert(new.into(), value.clone());
                }
            }
            merge_missing(settings, "workspace_terminal", terminal);
        }
        for key in [
            "agent",
            "agent_servers",
            "language_models",
            "copilot",
            "context_servers",
            "context_server_timeout",
            "disable_ai",
            "edit_predictions",
        ] {
            settings.remove(key);
        }
        for key in [
            "agent_ui_font_size",
            "agent_ui_font_family",
            "agent_buffer_font_size",
            "agent_buffer_font_family",
            "show_edit_predictions",
            "edit_predictions_disabled_in",
        ] {
            settings.remove(key);
        }
        if let Some(languages) = settings.get_mut("languages").and_then(Value::as_object_mut) {
            for language in languages.values_mut().filter_map(Value::as_object_mut) {
                language.remove("show_edit_predictions");
                language.remove("edit_predictions_disabled_in");
            }
        }
        if let Some(telemetry) = settings.get_mut("telemetry").and_then(Value::as_object_mut) {
            telemetry.remove("anthropic_retention");
        }
        if let Some(vim) = settings.get_mut("vim").and_then(Value::as_object_mut) {
            vim.remove("show_edit_predictions_in_normal_mode");
        }
        if let Some(toolbar) = settings.get_mut("toolbar").and_then(Value::as_object_mut) {
            toolbar.remove("agent_review");
        }
        Ok(())
    })
}

fn merge_missing(settings: &mut Map<String, Value>, key: &str, old: Map<String, Value>) {
    if old.is_empty() {
        return;
    }
    let new = settings
        .entry(key)
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(new) = new.as_object_mut() {
        for (key, value) in old {
            new.entry(key).or_insert(value);
        }
    }
}

enum MigratedContext {
    Always,
    Never,
    Predicate(KeyBindingContextPredicate),
}

fn is_removed_context(name: &str) -> bool {
    matches!(
        name,
        "AcpThread"
            | "AcpThreadSearchBar"
            | "AgentDiff"
            | "AgentFeedback"
            | "AgentFeedbackMessageEditor"
            | "MessageEditor"
            | "Conversation"
            | "InlineAssist"
            | "InlineAssistant"
            | "Assistant"
            | "EditPredictionContext"
            | "ThreadHistory"
            | "SkillCreator"
            | "RulesLibrary"
            | "PromptLibrary"
            | "edit_prediction"
            | "edit_prediction_mode"
    )
}

fn renamed_context(name: &str) -> Option<&'static str> {
    match name {
        "AgentPanel" => Some("WorkspaceTerminal"),
        "AgentsSidebar" | "ThreadsSidebar" => Some("WorkspaceSidebar"),
        "ThreadSwitcher" => Some("TerminalSwitcher"),
        _ => None,
    }
}

fn migrate_context_predicate(
    predicate: &KeyBindingContextPredicate,
    is_root: bool,
) -> Option<MigratedContext> {
    use KeyBindingContextPredicate as Predicate;
    use MigratedContext::{Always, Never};

    Some(match predicate {
        Predicate::Identifier(name) if is_removed_context(name) => Never,
        Predicate::Identifier(name) => MigratedContext::Predicate(Predicate::Identifier(
            renamed_context(name)
                .map(Into::into)
                .unwrap_or_else(|| name.clone()),
        )),
        Predicate::Equal(name, _) if is_removed_context(name) => Never,
        Predicate::NotEqual(name, _) if is_removed_context(name) => Always,
        Predicate::Equal(name, value) => MigratedContext::Predicate(Predicate::Equal(
            renamed_context(name)
                .map(Into::into)
                .unwrap_or_else(|| name.clone()),
            value.clone(),
        )),
        Predicate::NotEqual(name, value) => MigratedContext::Predicate(Predicate::NotEqual(
            renamed_context(name)
                .map(Into::into)
                .unwrap_or_else(|| name.clone()),
            value.clone(),
        )),
        Predicate::Not(inner) => match migrate_context_predicate(inner, false)? {
            Always => Never,
            Never => Always,
            MigratedContext::Predicate(inner) => {
                MigratedContext::Predicate(Predicate::Not(Box::new(inner)))
            }
        },
        Predicate::And(left, right) => match (
            migrate_context_predicate(left, false)?,
            migrate_context_predicate(right, false)?,
        ) {
            (Never, _) | (_, Never) => Never,
            (Always, other) | (other, Always) => other,
            (MigratedContext::Predicate(left), MigratedContext::Predicate(right)) => {
                MigratedContext::Predicate(Predicate::And(Box::new(left), Box::new(right)))
            }
        },
        Predicate::Or(left, right) => match (
            migrate_context_predicate(left, false)?,
            migrate_context_predicate(right, false)?,
        ) {
            (Always, _) | (_, Always) => Always,
            (Never, other) | (other, Never) => other,
            (MigratedContext::Predicate(left), MigratedContext::Predicate(right)) => {
                MigratedContext::Predicate(Predicate::Or(Box::new(left), Box::new(right)))
            }
        },
        Predicate::Descendant(parent, child) => match (
            migrate_context_predicate(parent, false)?,
            migrate_context_predicate(child, false)?,
        ) {
            (Never, _) | (_, Never) => Never,
            (Always, MigratedContext::Predicate(child))
                if is_root
                    && context_has_no_ancestry_constraints(&child)
                    && matches!(parent.as_ref(), Predicate::Not(inner)
                        if matches!(inner.as_ref(), Predicate::Identifier(name) if is_removed_context(name))) =>
            {
                MigratedContext::Predicate(child)
            }
            (MigratedContext::Predicate(parent), MigratedContext::Predicate(child)) => {
                MigratedContext::Predicate(Predicate::Descendant(Box::new(parent), Box::new(child)))
            }
            // A constant child still imposes a depth constraint. Keep unfamiliar ancestry
            // expressions rather than moving their bindings to a different context.
            _ => return None,
        },
    })
}

fn context_has_no_ancestry_constraints(predicate: &KeyBindingContextPredicate) -> bool {
    use KeyBindingContextPredicate as Predicate;

    match predicate {
        Predicate::Identifier(_) | Predicate::Equal(..) | Predicate::NotEqual(..) => true,
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            context_has_no_ancestry_constraints(left) && context_has_no_ancestry_constraints(right)
        }
        // Negation scans the entire context stack, which a descendant expression narrows.
        Predicate::Not(_) | Predicate::Descendant(..) => false,
    }
}

fn serialize_context(predicate: &KeyBindingContextPredicate) -> String {
    use KeyBindingContextPredicate as Predicate;

    match predicate {
        Predicate::Identifier(name) => name.to_string(),
        Predicate::Equal(name, value) => format!("{name} == {value}"),
        Predicate::NotEqual(name, value) => format!("{name} != {value}"),
        Predicate::Not(inner) => match inner.as_ref() {
            Predicate::Identifier(name) => format!("!{name}"),
            _ => format!("!({})", serialize_context(inner)),
        },
        Predicate::And(left, right) => {
            format!(
                "{} && {}",
                serialize_context_operand(left),
                serialize_context_operand(right)
            )
        }
        Predicate::Or(left, right) => {
            format!(
                "{} || {}",
                serialize_context_operand(left),
                serialize_context_operand(right)
            )
        }
        // GPUI's Display omits parentheses around nested descendant expressions.
        Predicate::Descendant(parent, child) => {
            format!(
                "{} > {}",
                serialize_context_operand(parent),
                serialize_context_operand(child)
            )
        }
    }
}

fn serialize_context_operand(predicate: &KeyBindingContextPredicate) -> String {
    use KeyBindingContextPredicate as Predicate;

    match predicate {
        Predicate::And(..) | Predicate::Or(..) | Predicate::Descendant(..) => {
            format!("({})", serialize_context(predicate))
        }
        _ => serialize_context(predicate),
    }
}

fn migrate_keymap_context(context: &mut Value) -> bool {
    let Some(name) = context.get("context").and_then(Value::as_str) else {
        return true;
    };
    let Ok(original) = KeyBindingContextPredicate::parse(name) else {
        return true;
    };
    let migrated = match migrate_context_predicate(&original, true) {
        Some(MigratedContext::Never) => return false,
        Some(MigratedContext::Always) => String::new(),
        Some(MigratedContext::Predicate(predicate)) if predicate != original => {
            let migrated = serialize_context(&predicate);
            if !matches!(KeyBindingContextPredicate::parse(&migrated), Ok(parsed) if parsed == predicate)
            {
                log::error!("Failed to preserve keymap context structure during migration");
                return true;
            }
            migrated
        }
        _ => return true,
    };
    if let Some(object) = context.as_object_mut() {
        object.insert("context".into(), Value::String(migrated));
    }
    true
}

pub fn migrate_terminal_keymap(value: &mut Value) -> Result<()> {
    let Some(contexts) = value.as_array_mut() else {
        return Ok(());
    };
    contexts.retain_mut(migrate_keymap_context);
    for context in contexts {
        let Some(bindings) = context.get_mut("bindings").and_then(Value::as_object_mut) else {
            continue;
        };
        bindings.retain(|_, action| {
            if let Some(args) = action.as_array_mut()
                && args.first().and_then(Value::as_str) == Some("zed::Extensions")
                && let Some(options) = args.get_mut(1).and_then(Value::as_object_mut)
                && options.get("category_filter").and_then(Value::as_str) == Some("context_servers")
            {
                options.remove("category_filter");
            }
            let name = action.as_str().or_else(|| {
                action
                    .as_array()
                    .and_then(|args| args.first())
                    .and_then(Value::as_str)
            });
            let Some(name) = name else {
                return true;
            };
            let replacement = match name {
                "agent::ToggleFocus" => Some("workspace_terminal::ToggleFocus"),
                "agent::ToggleSearch" => Some("workspace_terminal::ToggleSearch"),
                "agent::RenameThread" => Some("workspace_terminal::RenameTerminal"),
                "agent::CloseThread" => Some("workspace_terminal::CloseTerminal"),
                "agent::NewThread" | "agent::NewTerminalThread" => {
                    Some("workspace_terminal::NewTerminal")
                }
                "agents_sidebar::NewThreadInGroup" => Some("workspace_sidebar::NewTerminalInGroup"),
                "agents_sidebar::FocusSidebarFilter" => {
                    Some("workspace_sidebar::FocusSidebarFilter")
                }
                "agents_sidebar::ToggleThreadSwitcher" => {
                    Some("workspace_sidebar::ToggleTerminalSwitcher")
                }
                "multi_workspace::NextThread" => Some("multi_workspace::NextWorkspaceTerminal"),
                "multi_workspace::PreviousThread" => {
                    Some("multi_workspace::PreviousWorkspaceTerminal")
                }
                "multi_workspace::NewThread" => Some("multi_workspace::NewWorkspaceTerminal"),
                _ => None,
            };
            if let Some(replacement) = replacement {
                if replacement == "workspace_sidebar::ToggleTerminalSwitcher"
                    && let Some(args) = action.as_array_mut()
                {
                    if let Some(name) = args.first_mut() {
                        *name = Value::String(replacement.into());
                    }
                } else {
                    *action = Value::String(replacement.into());
                }
                true
            } else {
                !name.contains("EditPrediction")
                    && !matches!(
                        name,
                        "git::GenerateCommitMessage"
                            | "git::ReviewDiff"
                            | "workspace::UseAgenticLayout"
                            | "workspace::UseClassicLayout"
                            | "editor::SendReviewToAgent"
                    )
                    && ![
                        "agent::",
                        "assistant::",
                        "inline_assistant::",
                        "agents_sidebar::",
                        "copilot::",
                        "edit_prediction::",
                        "copilot_edit_predictions::",
                        "acp::",
                        "zeta::",
                        "zed_predict_onboarding::",
                        "context_server::",
                        "skill_creator::",
                        "bedrock::",
                        "language_model::",
                        "language_models::",
                        "prompt_store::",
                    ]
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
            }
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn complete_settings_migration_preserves_comments_and_terminal_layout() {
        let input = r#"{
            // Keep the project layout.
            "agent": { "dock": "right", "default_width": 700, "threads_sidebar": {"position":"right"} },
            "show_edit_predictions": true,
            "languages": {"Rust": {"show_edit_predictions": true, "tab_size": 4}},
            "vim_mode": true
        }"#;
        let migrated = crate::migrate_settings(input).unwrap().unwrap();
        assert!(migrated.contains("// Keep the project layout."));
        let value: Value = settings_json::parse_json_with_comments(&migrated).unwrap();
        assert_eq!(value["workspace_terminal"]["dock"], "right");
        assert_eq!(value["workspace_terminal"]["default_width"], 700);
        assert_eq!(value["workspace_sidebar"]["position"], "right");
        assert_eq!(value["languages"]["Rust"]["tab_size"], 4);
        assert_eq!(value["vim_mode"], true);
        assert!(value.get("agent").is_none());
        assert!(
            value["languages"]["Rust"]
                .get("show_edit_predictions")
                .is_none()
        );
        assert_eq!(crate::migrate_settings(&migrated).unwrap(), None);
    }

    #[test]
    fn complete_keymap_migration_removes_ai_contexts_without_losing_editor_bindings() {
        let input = r#"[
            {"context":"AgentPanel > Terminal","bindings":{"ctrl-n":"agent::NewThread"}},
            {"context":"Editor && edit_prediction","bindings":{"tab":"editor::AcceptEditPrediction"}},
            {"context":"Editor && !edit_prediction","bindings":{"ctrl-s":"workspace::Save","ctrl-e":"editor::ToggleEditPrediction"}}
        ]"#;
        let migrated = crate::migrate_keymap(input).unwrap().unwrap();
        let value: Value = settings_json::parse_json_with_comments(&migrated).unwrap();
        assert_eq!(value.as_array().unwrap().len(), 2);
        assert_eq!(value[0]["context"], "WorkspaceTerminal > Terminal");
        assert_eq!(
            value[0]["bindings"]["ctrl-n"],
            "workspace_terminal::NewTerminal"
        );
        assert_eq!(value[1]["context"], "Editor");
        assert_eq!(value[1]["bindings"]["ctrl-s"], "workspace::Save");
        assert!(value[1]["bindings"].get("ctrl-e").is_none());
        assert_eq!(crate::migrate_keymap(&migrated).unwrap(), None);
    }

    #[test]
    fn complete_keymap_migration_preserves_negated_ai_context_customizations() {
        let input = r#"[
            // Keep my editor customizations.
            {
                "context": "!AcpThread > Editor && mode == full",
                "use_key_equivalents": true,
                "bindings": {
                    "ctrl-s": "workspace::Save",
                    "alt-enter": "editor::OpenExcerpts",
                    "ctrl-l": ["editor::SelectToEndOfLine", {"stop_at_soft_wraps": true}],
                    "ctrl-k": null,
                    "ctrl-enter": "assistant::InlineAssist"
                }
            }
        ]"#;
        let migrated = crate::migrate_keymap(input).unwrap().unwrap();
        let value: Value = settings_json::parse_json_with_comments(&migrated).unwrap();
        assert_eq!(value.as_array().unwrap().len(), 1);
        assert_eq!(value[0]["context"], "Editor && mode == full");
        assert_eq!(value[0]["use_key_equivalents"], true);
        assert_eq!(
            value[0]["bindings"],
            json!({
                "ctrl-s": "workspace::Save",
                "alt-enter": "editor::OpenExcerpts",
                "ctrl-l": ["editor::SelectToEndOfLine", {"stop_at_soft_wraps": true}],
                "ctrl-k": null,
            })
        );
        assert_eq!(crate::migrate_keymap(&migrated).unwrap(), None);
    }

    #[test]
    fn migrate_contexts_fold_only_removed_identifiers() {
        for (context, expected) in [
            (
                " !( AcpThread ) > (Editor && mode == full) ",
                Some("Editor && mode == full"),
            ),
            ("Editor && ! ( edit_prediction )", Some("Editor")),
            ("Editor && !(AcpThread || InlineAssistant)", Some("Editor")),
            ("Editor || AcpThreadSearchBar", Some("Editor")),
            (
                "Editor && (AcpThread || focused)",
                Some("Editor && focused"),
            ),
            ("AcpThread > Editor", None),
            ("AgentFeedbackMessageEditor > Editor", None),
            ("InlineAssistant > Editor", None),
            ("Editor && !!AcpThread", None),
            ("Editor && edit_prediction_mode == eager", None),
            ("Editor && edit_prediction_mode != eager", Some("Editor")),
            ("!AcpThread", Some("")),
            ("!AcpThread || Editor", Some("")),
            ("Editor && !(AcpThread && focused)", Some("Editor")),
            (
                "Editor && !(AcpThread || focused)",
                Some("Editor && !focused"),
            ),
            ("CustomAcpThread > Editor", Some("CustomAcpThread > Editor")),
            ("AgentPanelExtra", Some("AgentPanelExtra")),
            ("mode == AcpThread", Some("mode == AcpThread")),
            ("mode != AgentPanel", Some("mode != AgentPanel")),
            ("Editor && !!focused", Some("Editor && !!focused")),
            ("", Some("")),
        ] {
            let mut value =
                json!([{"context": context, "bindings": {"ctrl-s": "workspace::Save"}}]);
            migrate_terminal_keymap(&mut value).unwrap();
            if let Some(expected) = expected {
                assert_eq!(value.as_array().unwrap().len(), 1, "{context}");
                let actual = value[0]["context"].as_str().unwrap();
                if expected.is_empty() {
                    assert_eq!(actual, expected, "{context}");
                } else {
                    assert_eq!(
                        KeyBindingContextPredicate::parse(actual).unwrap(),
                        KeyBindingContextPredicate::parse(expected).unwrap(),
                        "{context}"
                    );
                }
                assert_eq!(value[0]["bindings"]["ctrl-s"], "workspace::Save");
            } else {
                assert!(value.as_array().unwrap().is_empty(), "{context}");
            }
            let migrated = value.clone();
            migrate_terminal_keymap(&mut value).unwrap();
            assert_eq!(value, migrated, "{context}");
        }
    }

    #[test]
    fn migrate_contexts_preserves_unfamiliar_or_invalid_expressions() {
        for context in [
            "Editor > !AcpThread",
            "!AcpThread > !InlineAssistant",
            "(Editor || !AcpThread) > Terminal",
            "Workspace > (!AcpThread > Editor)",
            "!(!AcpThread > Editor)",
            "Editor && (AcpThread ||)",
            "!(AgentPanel",
            "  Editor && !CustomAcpThread  ",
            "Editor && !!focused",
        ] {
            let mut value = json!([{"context": context, "bindings": {
                "ctrl-s": "workspace::Save",
                "ctrl-enter": "assistant::InlineAssist"
            }}]);
            migrate_terminal_keymap(&mut value).unwrap();
            assert_eq!(value[0]["context"], context);
            assert_eq!(value[0]["bindings"], json!({"ctrl-s": "workspace::Save"}));
        }
        let mut value = json!([
            {"bindings": {"ctrl-s": "workspace::Save"}},
            {"context": null, "bindings": {"ctrl-s": "workspace::Save"}},
        ]);
        let original = value.clone();
        migrate_terminal_keymap(&mut value).unwrap();
        assert_eq!(value, original);
    }

    #[test]
    fn migrate_contexts_preserves_scoped_negation_and_ancestry() {
        let contexts = [
            gpui::KeyContext::parse("Workspace").unwrap(),
            gpui::KeyContext::parse("Editor").unwrap(),
        ];
        for (context, expected_match) in [
            ("!AcpThread > (Editor && !Workspace)", true),
            ("!AcpThread > (Workspace > Editor)", false),
        ] {
            let original = KeyBindingContextPredicate::parse(context).unwrap();
            assert_eq!(original.eval(&contexts), expected_match);
            let mut value =
                json!([{"context": context, "bindings": {"ctrl-s": "workspace::Save"}}]);
            migrate_terminal_keymap(&mut value).unwrap();
            assert_eq!(value[0]["context"], context);
            let migrated =
                KeyBindingContextPredicate::parse(value[0]["context"].as_str().unwrap()).unwrap();
            assert_eq!(migrated.eval(&contexts), expected_match);
            assert_eq!(value[0]["bindings"]["ctrl-s"], "workspace::Save");
        }
    }

    #[test]
    fn migrate_context_serialization_preserves_ancestry_and_boolean_grouping() {
        for (context, expected) in [
            (
                "(AgentPanel > Terminal) && focused",
                "(WorkspaceTerminal > Terminal) && focused",
            ),
            (
                "Workspace > (AgentPanel > Terminal)",
                "Workspace > (WorkspaceTerminal > Terminal)",
            ),
            (
                "(AgentPanel > Terminal) > Editor",
                "(WorkspaceTerminal > Terminal) > Editor",
            ),
            (
                "(AgentPanel > Terminal) || Editor",
                "(WorkspaceTerminal > Terminal) || Editor",
            ),
            (
                "!(AgentPanel > Terminal)",
                "!(WorkspaceTerminal > Terminal)",
            ),
            (
                "ThreadsSidebar && (focused || !AcpThread)",
                "WorkspaceSidebar",
            ),
        ] {
            let mut value =
                json!([{"context": context, "bindings": {"ctrl-s": "workspace::Save"}}]);
            migrate_terminal_keymap(&mut value).unwrap();
            assert_eq!(
                KeyBindingContextPredicate::parse(value[0]["context"].as_str().unwrap()).unwrap(),
                KeyBindingContextPredicate::parse(expected).unwrap(),
                "{context}",
            );
            let migrated = value.clone();
            migrate_terminal_keymap(&mut value).unwrap();
            assert_eq!(value, migrated);
        }
    }

    #[test]
    fn migrate_preserves_terminal_preferences_and_new_values() {
        let legacy = json!({"agent": {"threads_sidebar": {"position":"right", "default_width":420}, "terminal_init_command":"pwsh", "dock":"right"}});
        let mut value = legacy.clone();
        value["workspace_sidebar"] = json!({"default_width": 360});
        value["windows"] = legacy.clone();
        value["profiles"] = json!({"work": {"settings": legacy}});
        migrate_workspace_terminals(&mut value).unwrap();
        assert_eq!(
            value["workspace_sidebar"],
            json!({"default_width":360, "position":"right"})
        );
        assert_eq!(value["workspace_terminal"]["init_command"], "pwsh");
        assert_eq!(value["windows"]["workspace_terminal"]["dock"], "right");
        assert_eq!(
            value["profiles"]["work"]["settings"]["workspace_sidebar"]["default_width"],
            420
        );
        assert!(value.get("agent").is_none());
        let migrated = value.clone();
        migrate_workspace_terminals(&mut value).unwrap();
        assert_eq!(value, migrated);
    }

    #[test]
    fn migrate_keymap_preserves_switcher_arguments_and_editor_bindings() {
        let mut value = json!([{"context":"AgentsSidebar", "bindings":{
            "ctrl-n":"agents_sidebar::NewThreadInGroup",
            "ctrl-shift-tab":["agents_sidebar::ToggleThreadSwitcher", {"select_last":true}],
            "ctrl-enter":"assistant::Assist", "ctrl-s":"workspace::Save"
        }}]);
        migrate_terminal_keymap(&mut value).unwrap();
        assert_eq!(value[0]["context"], "WorkspaceSidebar");
        assert_eq!(
            value[0]["bindings"]["ctrl-shift-tab"],
            json!(["workspace_sidebar::ToggleTerminalSwitcher", {"select_last":true}])
        );
        assert!(value[0]["bindings"].get("ctrl-enter").is_none());
        assert_eq!(value[0]["bindings"]["ctrl-s"], "workspace::Save");
    }
}
