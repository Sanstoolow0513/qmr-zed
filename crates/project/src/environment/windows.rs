use super::{
    Arc, DirenvSettings, EnvironmentOrigin, HashMap, Path, Shell, load_directory_shell_environment,
    set_origin_marker,
};
use futures::{FutureExt as _, channel::mpsc, future::Shared};
use gpui::{App, AppContext as _, Task};
use parking_lot::Mutex;

type Environment = HashMap<String, String>;
type DirectoryCapture = Shared<Task<Option<Arc<Environment>>>>;

#[derive(Clone, Default)]
pub(super) struct ProjectEnvironmentCache(Arc<Mutex<Cache>>);

#[derive(Default)]
struct Cache {
    baseline: Option<Arc<Environment>>,
    load_direnv: Option<DirenvSettings>,
    directories: HashMap<(Shell, Arc<Path>), DirectoryCapture>,
    refresh_failed: bool,
    #[cfg(any(test, feature = "test-support"))]
    test_baseline: Option<Environment>,
}

impl Cache {
    fn baseline(
        &mut self,
        read: impl FnOnce() -> anyhow::Result<Environment>,
        load_direnv: &DirenvSettings,
        errors: &mpsc::UnboundedSender<String>,
    ) -> Arc<Environment> {
        let mut value = match read() {
            Ok(env) => {
                self.refresh_failed = false;
                env
            }
            Err(error) => {
                if !self.refresh_failed {
                    let message = format!("Failed to refresh the Windows environment: {error:#}");
                    log::warn!("{message}");
                    errors.unbounded_send(message).ok();
                    self.refresh_failed = true;
                }
                self.baseline.as_deref().cloned().unwrap_or_else(|| {
                    let mut env = Environment::default();
                    let inherited = std::env::vars_os().filter_map(|(name, value)| {
                        Some((name.into_string().ok()?, value.into_string().ok()?))
                    });
                    util::extend_environment(&mut env, inherited);
                    env
                })
            }
        };
        set_origin_marker(&mut value, EnvironmentOrigin::Windows);
        self.update(value, load_direnv)
    }

    fn update(&mut self, env: Environment, load_direnv: &DirenvSettings) -> Arc<Environment> {
        if self.load_direnv.as_ref() == Some(load_direnv)
            && let Some(baseline) = &self.baseline
            && baseline.as_ref() == &env
        {
            return baseline.clone();
        }
        self.directories.clear();
        self.load_direnv = Some(load_direnv.clone());
        let baseline = Arc::new(env);
        self.baseline = Some(baseline.clone());
        baseline
    }
}

impl ProjectEnvironmentCache {
    pub(super) fn resolve(
        &self,
        shell: Shell,
        directory: Option<Arc<Path>>,
        load_direnv: DirenvSettings,
        errors: mpsc::UnboundedSender<String>,
        cx: &App,
    ) -> Shared<Task<Option<Environment>>> {
        #[cfg(not(any(test, feature = "test-support")))]
        let source = util::windows_environment::fresh;
        #[cfg(any(test, feature = "test-support"))]
        let (source, load_direnv) = {
            // GPUI fake filesystems have no real project directories or --printenv binary.
            let injected = self.0.lock().test_baseline.clone();
            let settings = if injected.is_some() {
                load_direnv
            } else {
                DirenvSettings::Disabled
            };
            let source = move || {
                if let Some(env) = injected {
                    return Ok(env);
                }
                let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
                let system32 = format!(r"{root}\System32");
                let temp = std::env::temp_dir().to_string_lossy().into_owned();
                Ok(Environment::from_iter([
                    ("SystemRoot".into(), root),
                    (
                        "PATH".into(),
                        format!(r"{system32};{system32}\WindowsPowerShell\v1.0"),
                    ),
                    ("TEMP".into(), temp.clone()),
                    ("TMP".into(), temp),
                ]))
            };
            (source, settings)
        };
        self.resolve_from(shell, directory, load_direnv, errors, source, cx)
    }

    fn resolve_from(
        &self,
        shell: Shell,
        directory: Option<Arc<Path>>,
        load_direnv: DirenvSettings,
        errors: mpsc::UnboundedSender<String>,
        source: impl FnOnce() -> anyhow::Result<Environment> + Send + 'static,
        cx: &App,
    ) -> Shared<Task<Option<Environment>>> {
        let cache = self.0.clone();
        let executor = cx.background_executor().clone();
        cx.background_spawn(async move {
            let (baseline, capture) = {
                // Serialize snapshot reads on the background executor so an older registry read
                // cannot replace a newer one. Shell execution happens outside this lock.
                let mut cache = cache.lock();
                let baseline = cache.baseline(source, &load_direnv, &errors);
                let Some(directory) = directory else {
                    return Some((*baseline).clone());
                };
                // Windows has no direct direnv export. Starting PowerShell here would apply its
                // startup PATH changes twice, first while sampling and again in the real shell.
                if load_direnv == DirenvSettings::Disabled
                    || (load_direnv == DirenvSettings::Direct && shell == Shell::System)
                {
                    return Some((*baseline).clone());
                }
                let key = (shell.clone(), directory.clone());
                if cache
                    .directories
                    .get(&key)
                    .is_some_and(|task| matches!(task.peek(), Some(None)))
                {
                    cache.directories.remove(&key);
                }
                let task = cache.directories.entry(key).or_insert_with(|| {
                    executor
                        .spawn(capture_directory(
                            shell,
                            directory,
                            load_direnv,
                            errors,
                            baseline.clone(),
                        ))
                        .shared()
                });
                (baseline, task.clone())
            };
            Some((*capture.await.unwrap_or(baseline)).clone())
        })
        .shared()
    }
}

async fn capture_directory(
    shell: Shell,
    directory: Arc<Path>,
    load_direnv: DirenvSettings,
    errors: mpsc::UnboundedSender<String>,
    baseline: Arc<Environment>,
) -> Option<Arc<Environment>> {
    match load_directory_shell_environment(shell, directory.clone(), load_direnv, errors, &baseline)
        .await
    {
        Ok(mut env) => {
            set_origin_marker(&mut env, EnvironmentOrigin::WorktreeShell);
            Some(Arc::new(env))
        }
        Err(error) => {
            log::warn!("Failed to capture the shell environment in {directory:?}: {error:#}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    async fn resolves_added_changed_and_deleted_variables_and_retries(cx: &mut TestAppContext) {
        let cache = ProjectEnvironmentCache::default();
        let (tx, mut errors) = mpsc::unbounded();
        let read = |cx: &mut TestAppContext, value: anyhow::Result<Environment>| {
            cx.update(|cx| {
                cache.resolve_from(
                    Shell::System,
                    Some(Arc::from(Path::new(r"C:\project"))),
                    DirenvSettings::Direct,
                    tx.clone(),
                    move || value,
                    cx,
                )
            })
        };
        let initial = Environment::from_iter([("SDK".into(), "v1".into())]);
        let installed = Environment::from_iter([
            ("SDK".into(), "v2".into()),
            ("PATH".into(), r"C:\new tool".into()),
        ]);
        let expected = |mut env: Environment| {
            env.insert("ZED_ENVIRONMENT".into(), "windows".into());
            Some(env)
        };
        assert_eq!(read(cx, Ok(initial.clone())).await, expected(initial));
        assert_eq!(
            read(cx, Ok(installed.clone())).await,
            expected(installed.clone())
        );
        assert_eq!(
            read(cx, Err(anyhow::anyhow!("registry unavailable"))).await,
            expected(installed)
        );
        assert!(errors.try_recv().unwrap().contains("registry unavailable"));
        assert_eq!(
            read(cx, Ok(Environment::default())).await,
            expected(Environment::default())
        );
    }

    #[gpui::test]
    async fn rerunning_resolved_task_reads_current_windows_snapshot(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            release_channel::init(semver::Version::new(0, 0, 0), cx);
            let mut settings = settings::SettingsStore::test(cx);
            settings
                .set_user_settings(r#"{"terminal":{"env":{"qmr_override":"terminal"}}}"#, cx)
                .unwrap();
            cx.set_global(settings);
        });
        let fs = fs::FakeFs::new(cx.executor());
        let project = crate::Project::test(fs, [], cx).await;
        let directory = tempfile::tempdir().unwrap();
        let root = std::env::var("SystemRoot").unwrap();
        let template = task::TaskTemplate {
            command: "echo %QMR_BASE%/%QMR_OVERRIDE%>result.txt".into(),
            label: "probe environment".into(),
            cwd: Some(directory.path().to_string_lossy().into_owned()),
            shell: Shell::Program("cmd.exe".into()),
            env: Environment::from_iter([("QMR_OVERRIDE".into(), "explicit".into())]),
            ..Default::default()
        };
        let mut resolved = template
            .resolve_task("probe", &Default::default())
            .unwrap()
            .resolved;
        resolved.command_mode = task::TaskCommandMode::Shell;
        for value in ["first", "second"] {
            let snapshot = Environment::from_iter([
                ("SystemRoot".into(), root.clone()),
                ("PATH".into(), format!(r"{root}\System32")),
                ("QMR_BASE".into(), value.into()),
                ("QMR_OVERRIDE".into(), "base".into()),
            ]);
            let terminal = project
                .update(cx, |project, cx| {
                    project
                        .environment()
                        .read(cx)
                        .windows_cache
                        .0
                        .lock()
                        .test_baseline = Some(snapshot);
                    project.create_terminal_task(resolved.clone(), cx)
                })
                .await
                .unwrap();
            let exit = terminal
                .read_with(cx, |terminal, cx| terminal.wait_for_completed_task(cx))
                .await;
            assert!(exit.is_some_and(|status| status.success()));
            assert_eq!(
                std::fs::read_to_string(directory.path().join("result.txt"))
                    .unwrap()
                    .trim(),
                format!("{value}/explicit")
            );
        }
    }

    #[test]
    fn caches_directories_only_for_the_same_complete_environment_and_settings() {
        let mut cache = Cache::default();
        let original = Environment::from_iter([
            ("PATH".into(), "same".into()),
            ("SDK_ROOT".into(), "old".into()),
        ]);
        let first = cache.update(original.clone(), &DirenvSettings::Direct);
        let key = (Shell::System, Arc::from(Path::new("C:\\project")));
        cache
            .directories
            .insert(key, Task::ready(Some(first.clone())).shared());
        assert!(Arc::ptr_eq(
            &first,
            &cache.update(original.clone(), &DirenvSettings::Direct)
        ));
        assert_eq!(cache.directories.len(), 1);
        let mut changed = original.clone();
        changed.remove("SDK_ROOT");
        assert!(!Arc::ptr_eq(
            &first,
            &cache.update(changed, &DirenvSettings::Direct)
        ));
        assert!(cache.directories.is_empty());
        let restored = cache.update(original.clone(), &DirenvSettings::Direct);
        assert!(!Arc::ptr_eq(
            &restored,
            &cache.update(original, &DirenvSettings::Disabled)
        ));
    }

    #[gpui::test]
    async fn cargo_locator_replays_the_build_environment(cx: &mut TestAppContext) {
        use gpui::BorrowAppContext as _;
        use settings::SettingsStore;

        cx.executor().allow_parking();
        cx.update(|cx| {
            cx.set_global(terminal::HeadlessTerminal(true));
            release_channel::init(semver::Version::new(0, 0, 0), cx);
            let settings = SettingsStore::test(cx);
            cx.set_global(settings);
            dap::DapRegistry::global(cx)
                .add_locator(Arc::new(crate::debugger::locators::cargo::CargoLocator));
        });
        let fs = fs::FakeFs::new(cx.executor());
        let project = crate::Project::test(fs, [], cx).await;
        let directory = tempfile::tempdir().unwrap();
        for sdk in ["a", "b"] {
            let bin = directory.path().join(sdk);
            std::fs::create_dir(&bin).unwrap();
            std::fs::write(bin.join("cargo.cmd"), format!(
                "@echo off\r\n>>calls.txt echo {sdk} %* %TASK_MARKER% %BUILD_ONLY_MARKER%\r\necho {{\"executable\":\"{sdk}.exe\"}}\r\n"
            )).unwrap();
        }
        let system32 = Path::new(&std::env::var_os("SystemRoot").unwrap()).join("System32");
        let set_sdk = |cx: &mut TestAppContext, sdk: &str| {
            let settings = serde_json::json!({"terminal": {"env": {
                "Path": directory.path().join(sdk), "PATHEXT": ".EXE;.CMD", "ComSpec": system32.join("cmd.exe"),
                "TASK_MARKER": "from-settings",
                "BUILD_ONLY_MARKER": sdk,
            }}});
            cx.update(|cx| {
                cx.update_global::<SettingsStore, _>(|store, cx| {
                    store.set_user_settings(&settings.to_string(), cx).unwrap();
                })
            });
        };
        let template = task::TaskTemplate {
            command: "cargo".into(),
            args: vec!["build".into()],
            label: "build with SDK".into(),
            cwd: Some(directory.path().display().to_string()),
            shell: Shell::WithArguments {
                program: system32
                    .join(r"WindowsPowerShell\v1.0\powershell.exe")
                    .display()
                    .to_string(),
                args: vec!["-NoProfile".into()],
                title_override: None,
            },
            env: Environment::from_iter([("TASK_MARKER".into(), "$ZED_SYMBOL".into())]),
            ..Default::default()
        };
        let context = task::TaskContext {
            task_variables: [(task::VariableName::Symbol, "from-task".into())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let mut task = template.resolve_task("build", &context).unwrap().resolved;
        let overrides = task::substitute_variables_in_map(&template.env, &context).unwrap();
        task.command_mode = task::TaskCommandMode::ShellWithQuotedArgs;
        set_sdk(cx, "a");
        for sdk in ["a", "b"] {
            let (terminal, locator) = project
                .update(cx, |project, cx| {
                    project.create_build_terminal(task.clone(), Some(overrides.clone()), cx)
                })
                .await
                .unwrap();
            set_sdk(cx, "b");
            let status = terminal
                .read_with(cx, |terminal, cx| terminal.wait_for_completed_task(cx))
                .await
                .unwrap();
            assert!(status.success());
            let dap = project.read_with(cx, |project, _| project.dap_store());
            let result = dap
                .update(cx, |dap, cx| {
                    dap.run_debug_locator("rust-cargo-locator", locator.unwrap(), cx)
                })
                .await
                .unwrap();
            let dap::DebugRequest::Launch(launch) = result else {
                panic!("expected a launch")
            };
            assert_eq!(launch.program, format!("{sdk}.exe"));
            assert!(!launch.env.contains_key("BUILD_ONLY_MARKER"));
            assert_eq!(
                launch.env.get("TASK_MARKER").map(String::as_str),
                Some("from-task")
            );
            assert_eq!(launch.env.len(), 1);
        }
        let calls = std::fs::read_to_string(directory.path().join("calls.txt")).unwrap();
        assert_eq!(
            calls.lines().collect::<Vec<_>>(),
            [
                "a build from-task a",
                "a build --message-format=json from-task a",
                "b build from-task b",
                "b build --message-format=json from-task b",
            ]
        );
    }
}
