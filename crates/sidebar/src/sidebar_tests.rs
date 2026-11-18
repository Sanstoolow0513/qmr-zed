use super::*;
use gpui::TestAppContext;
use util::path;

#[gpui::test]
async fn saved_terminals_remain_visible_after_project_path_changes(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let settings = SettingsStore::test(cx);
        cx.set_global(settings);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        editor::init(cx);
        terminal_view::init(cx);
        terminal_view::workspace_terminal_panel::init(cx);
    });
    let fs = fs::FakeFs::new(cx.executor());
    fs.insert_tree(path!("/project"), serde_json::json!({"main.rs":""}))
        .await;
    fs.insert_tree(path!("/extra"), serde_json::json!({"other.rs":""}))
        .await;
    let project = project::Project::test(fs, [path!("/project").as_ref()], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
    let workspace =
        multi_workspace.read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone());
    workspace.update_in(cx, |workspace, window, cx| {
        let panel = cx.new(|cx| WorkspaceTerminalPanel::new(workspace, window, cx));
        workspace.add_panel(panel, window, cx);
    });
    let sidebar =
        cx.new_window_entity(|window, cx| Sidebar::new(multi_workspace.clone(), window, cx));
    multi_workspace.update(cx, |workspace, cx| {
        workspace.register_sidebar(sidebar.clone(), cx);
        workspace.open_sidebar(cx);
    });
    cx.run_until_parked();
    let terminal_ids: HashSet<_> = cx.update(|_, cx| {
        let paths = project.read(cx).worktree_paths(cx);
        TerminalMetadataStore::global(cx).update(cx, |store, cx| {
            [
                ("8e8ab7c0-09df-47db-bbc9-666d38165931", "Build"),
                ("8e8ab7c0-09df-47db-bbc9-666d38165932", "Server"),
            ]
            .into_iter()
            .map(|(terminal_id, title)| {
                let terminal_id = TerminalId::from_key_string(terminal_id).unwrap();
                store.save(
                    TerminalMetadata {
                        terminal_id,
                        title: title.into(),
                        custom_title: None,
                        created_at: chrono::Utc::now(),
                        worktree_paths: paths.clone(),
                        remote_connection: None,
                        working_directory: None,
                    },
                    cx,
                );
                terminal_id
            })
            .collect()
        })
    });
    let (added, _) = project
        .update(cx, |project, cx| {
            project.find_or_create_worktree(path!("/extra"), true, cx)
        })
        .await
        .unwrap();
    let added_id = added.read_with(cx, |worktree, _| worktree.id());

    for remove in [false, true] {
        if remove {
            project.update(cx, |project, cx| project.remove_worktree(added_id, cx));
        }
        cx.run_until_parked();
        multi_workspace.update(cx, |workspace, _| workspace.test_expand_all_groups());
        sidebar.update_in(cx, |sidebar, window, cx| {
            sidebar.update_entries(cx);
            let visible_ids: HashSet<_> = sidebar
                .entries
                .iter()
                .filter_map(|entry| match entry {
                    Entry::Terminal { metadata, .. } => Some(metadata.terminal_id),
                    Entry::Project { .. } => None,
                })
                .collect();
            assert_eq!(visible_ids, terminal_ids);
            sidebar.toggle_terminal_switcher(false, window, cx);
            let switcher = sidebar.switcher.as_ref().unwrap();
            let switcher_ids: HashSet<_> = switcher
                .read(cx)
                .entries()
                .iter()
                .map(|metadata| metadata.terminal_id)
                .collect();
            assert_eq!(switcher_ids, terminal_ids);
            switcher.update(cx, |_, cx| cx.emit(TerminalSwitcherEvent::Dismissed));
        });
        cx.run_until_parked();
    }
}

#[gpui::test]
async fn terminal_rows_reflow_after_resizing_and_renaming(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let settings = SettingsStore::test(cx);
        cx.set_global(settings);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        editor::init(cx);
        terminal_view::init(cx);
        terminal_view::workspace_terminal_panel::init(cx);
    });
    let fs = fs::FakeFs::new(cx.executor());
    fs.insert_tree(path!("/project"), serde_json::json!({"main.rs": ""}))
        .await;
    let project = project::Project::test(fs, [path!("/project").as_ref()], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
    let sidebar =
        cx.new_window_entity(|window, cx| Sidebar::new(multi_workspace.clone(), window, cx));
    multi_workspace.update(cx, |workspace, cx| {
        workspace.register_sidebar(sidebar.clone(), cx);
        workspace.open_sidebar(cx);
    });
    sidebar.update(cx, |sidebar, cx| sidebar.set_width(Some(px(200.)), cx));
    let terminal_id = TerminalId::from_key_string("8e8ab7c0-09df-47db-bbc9-666d38165931").unwrap();
    cx.update(|_, cx| {
        let metadata = TerminalMetadata {
            terminal_id,
            title: "Shell".into(),
            custom_title: None,
            created_at: chrono::Utc::now(),
            worktree_paths: project.read(cx).worktree_paths(cx),
            remote_connection: None,
            working_directory: None,
        };
        TerminalMetadataStore::global(cx).update(cx, |store, cx| store.save(metadata, cx));
    });

    let height = |cx: &mut gpui::VisualTestContext| {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        sidebar.read_with(cx, |sidebar, _| {
            assert!(matches!(sidebar.entries[1], Entry::Terminal { .. }));
            sidebar.list_state.bounds_for_item(1).unwrap().size.height
        })
    };
    let rename = |title: SharedString, cx: &mut gpui::VisualTestContext| {
        cx.update(|_, cx| {
            TerminalMetadataStore::global(cx).update(cx, |store, cx| {
                store.rename_terminal(terminal_id, title, cx)
            });
        });
    };
    let one_line = height(cx);
    let icon_width = |cx: &mut gpui::VisualTestContext| {
        cx.debug_bounds("sidebar-terminal-icon-1")
            .unwrap()
            .size
            .width
    };
    let original_icon_width = icon_width(cx);
    let title = "Compile the server tests";
    rename(title.into(), cx);
    let two_lines = height(cx);
    assert!(two_lines > one_line);
    rename(format!("{} 验收Marker", title.repeat(12)).into(), cx);
    assert_eq!(
        height(cx),
        two_lines,
        "longer titles are limited to two lines"
    );
    let filter = |query: &'static str, cx: &mut gpui::VisualTestContext| {
        sidebar.update_in(cx, |sidebar, window, cx| {
            sidebar
                .filter_editor
                .update(cx, |editor, cx| editor.set_text(query, window, cx));
        });
        cx.run_until_parked();
    };
    filter("no matching terminal", cx);
    sidebar.read_with(cx, |sidebar, _| assert!(sidebar.entries.is_empty()));
    filter("验收marker", cx);
    assert_eq!(
        height(cx),
        two_lines,
        "the hidden suffix is still searchable"
    );
    filter("", cx);
    assert_eq!(height(cx), two_lines);

    rename(title.into(), cx);
    sidebar.update(cx, |sidebar, cx| sidebar.set_width(Some(px(600.)), cx));
    assert_eq!(height(cx), one_line);
    sidebar.update(cx, |sidebar, cx| sidebar.set_width(Some(px(200.)), cx));
    assert_eq!(height(cx), two_lines);
    cx.update(|_, cx| theme_settings::adjust_ui_font_size(cx, |size| size + px(6.)));
    assert!(height(cx) > two_lines);
    assert!(icon_width(cx) > original_icon_width);
    rename("Docs".into(), cx);
    let short_with_larger_font = height(cx);
    rename(title.into(), cx);
    assert!(height(cx) > short_with_larger_font);
    let row = sidebar.read_with(cx, |sidebar, _| {
        sidebar.list_state.bounds_for_item(1).unwrap()
    });
    cx.simulate_click(
        gpui::point(row.right() - px(15.), row.center().y),
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();
    sidebar.read_with(cx, |sidebar, cx| {
        assert!(
            TerminalMetadataStore::global(cx)
                .read(cx)
                .entry(terminal_id)
                .is_none()
        );
        assert!(matches!(
            sidebar.entries.as_slice(),
            [Entry::Project { .. }]
        ));
    });
}

#[gpui::test]
async fn terminal_entries_filter_collapse_and_close(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let settings = SettingsStore::test(cx);
        cx.set_global(settings);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        editor::init(cx);
        terminal_view::init(cx);
        terminal_view::workspace_terminal_panel::init(cx);
    });
    let fs = fs::FakeFs::new(cx.executor());
    fs.insert_tree(path!("/project"), serde_json::json!({"main.rs":""}))
        .await;
    let project = project::Project::test(fs, [path!("/project").as_ref()], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
    multi_workspace.update(cx, |workspace, cx| workspace.retain_active_workspace(cx));
    multi_workspace.update(cx, |workspace, _| workspace.test_expand_all_groups());
    let sidebar =
        cx.new_window_entity(|window, cx| Sidebar::new(multi_workspace.clone(), window, cx));
    multi_workspace.update(cx, |workspace, cx| {
        workspace.register_sidebar(sidebar.clone(), cx);
        workspace.open_sidebar(cx);
    });
    cx.run_until_parked();
    let terminal_id = TerminalId::from_key_string("8e8ab7c0-09df-47db-bbc9-666d38165931").unwrap();
    cx.update(|_, cx| {
        let metadata = TerminalMetadata {
            terminal_id,
            title: "Shell".into(),
            custom_title: Some("Build".into()),
            created_at: chrono::Utc::now(),
            worktree_paths: project.read(cx).worktree_paths(cx),
            remote_connection: None,
            working_directory: None,
        };
        TerminalMetadataStore::global(cx).update(cx, |store, cx| store.save(metadata, cx));
    });
    cx.run_until_parked();
    sidebar.update_in(cx, |sidebar, window, cx| {
        sidebar.update_entries(cx);
        assert_eq!(sidebar.entries.len(), 2);
        let key = sidebar.entries[0].key().clone();
        sidebar.set_group_expanded(&key, false, cx);
        sidebar.update_entries(cx);
        assert_eq!(sidebar.entries.len(), 1);
        sidebar
            .filter_editor
            .update(cx, |editor, cx| editor.set_text("build", window, cx));
        sidebar.update_entries(cx);
        assert_eq!(
            sidebar.entries.len(),
            2,
            "search reveals terminals in collapsed projects"
        );
        sidebar.selection = Some(1);
        sidebar.close_selected_terminal(window, cx);
        assert!(
            TerminalMetadataStore::global(cx)
                .read(cx)
                .entry(terminal_id)
                .is_none()
        );
        sidebar.reset_filter_editor_text(window, cx);
        sidebar.update_entries(cx);
        assert_eq!(
            sidebar.entries.len(),
            1,
            "closing a terminal preserves its project"
        );
    });

    sidebar.update_in(cx, |sidebar, window, cx| {
        sidebar.filter_editor.update(cx, |editor, cx| {
            editor.set_text("no matching projects or terminals", window, cx)
        });
        sidebar.update_entries(cx);
        assert!(sidebar.entries.is_empty());
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let header = cx
        .debug_bounds("workspace-sidebar-header")
        .expect("the header remains visible when filtering returns no entries");
    let add_project = cx
        .debug_bounds("ICON-FolderAdd")
        .expect("add project remains reachable without visible entries");
    assert!(header.contains(&add_project.center()));
    let clear_filter = cx
        .debug_bounds("ICON-Close")
        .expect("an unmatched search can still be cleared");
    assert!(header.contains(&clear_filter.center()));
    cx.simulate_click(clear_filter.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    sidebar.read_with(cx, |sidebar, cx| {
        assert!(!sidebar.has_filter_query(cx));
        assert_eq!(sidebar.entries.len(), 1);
    });

    let close_sidebar = cx
        .debug_bounds("ICON-ThreadsSidebarLeftOpen")
        .expect("the sidebar toggle is available in the header");
    assert!(header.contains(&close_sidebar.center()));
    cx.simulate_click(close_sidebar.center(), gpui::Modifiers::none());
    multi_workspace.read_with(cx, |workspace, _| assert!(!workspace.sidebar_open()));

    cx.update(|_, cx| {
        let mut settings = WorkspaceSidebarSettings::get_global(cx).clone();
        settings.position = settings::SidebarDockPosition::Right;
        WorkspaceSidebarSettings::override_global(settings, cx);
    });
    sidebar.update_in(cx, |sidebar, window, cx| {
        sidebar.set_width(Some(px(200.)), cx);
        sidebar.filter_editor.update(cx, |editor, cx| {
            editor.set_text("no matching projects or terminals", window, cx)
        });
        sidebar.update_entries(cx);
    });
    multi_workspace.update(cx, |workspace, cx| workspace.open_sidebar(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let header = cx
        .debug_bounds("workspace-sidebar-header")
        .expect("the narrow right sidebar retains its search header");
    assert!(header.size.width <= px(200.));
    for selector in [
        "ICON-FolderAdd",
        "ICON-Close",
        "ICON-ThreadsSidebarRightOpen",
    ] {
        let button = cx.debug_bounds(selector).expect("header action is visible");
        assert!(header.contains(&button.center()));
    }
}

#[test]
fn sidebar_shortens_only_standalone_windows_shell_titles() {
    let mut metadata = TerminalMetadata {
        terminal_id: TerminalId::from_key_string("eb96ef6a-50de-4d48-a4ba-a3c2baf8bae4").unwrap(),
        title: "".into(),
        custom_title: None,
        created_at: chrono::Utc::now(),
        worktree_paths: project::WorktreePaths::from_folder_paths(&Default::default()),
        remote_connection: None,
        working_directory: None,
    };
    for (raw, expected) in [
        (r"C:\Program Files\PowerShell\7\pwsh.exe", "pwsh"),
        (r#""C:\Program Files\PowerShell\7\pwsh.exe""#, "pwsh"),
        (r"\\?\C:\WINDOWS\System32\CMD.EXE", "cmd"),
        (r"\\server\share\cmd.exe", "cmd"),
        ("C:/Windows/System32/cmd.exe", "cmd"),
        ("PWSH.EXE", "pwsh"),
        ("cmd.exe", "cmd"),
    ] {
        metadata.title = raw.into();
        assert_eq!(
            sidebar_terminal_title(&metadata).as_ref(),
            expected,
            "{raw}"
        );
        assert_eq!(metadata.title.as_ref(), raw);
    }
    for raw in [
        "Fix pwsh.exe",
        "⠋ Reviewing changes",
        "repo — pwsh.exe",
        r"C:\Windows\powershell.exe",
        r"C:\tools\my-pwsh.exe",
        r"C:\tools\cmd.exe /c build",
        r"C:\tools\cmd.exe /c scripts\pwsh.exe",
        r"C:\tools\other --run scripts\cmd.exe",
        r#"C:\dir"withquote\cmd.exe"#,
        r"Switch to C:\Windows\cmd.exe",
        r"relative\cmd.exe",
        r"\cmd.exe",
        r"\\server\cmd.exe",
        r"\\.\pipe\cmd.exe",
        r"C:\tools\pwsh.exe\",
    ] {
        metadata.title = raw.into();
        assert_eq!(sidebar_terminal_title(&metadata).as_ref(), raw);
    }
    metadata.title = r"C:\Windows\System32\cmd.exe".into();
    metadata.custom_title = Some(r"C:\tools\pwsh.exe".into());
    assert_eq!(
        sidebar_terminal_title(&metadata).as_ref(),
        r"C:\tools\pwsh.exe"
    );
    metadata.title = "⠋ Reviewing changes".into();
    metadata.custom_title = Some("Release".into());
    assert_eq!(sidebar_terminal_title(&metadata).as_ref(), "⠋ Release");
}
