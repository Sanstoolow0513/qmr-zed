use super::*;
use gpui::TestAppContext;
use util::path;

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
}
