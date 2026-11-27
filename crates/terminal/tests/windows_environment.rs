#![cfg(windows)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::tty::{self, ChildEvent, Options, Shell};

fn run_pty(options: Options) -> Option<i32> {
    let size = WindowSize {
        num_lines: 24,
        num_cols: 80,
        cell_width: 8,
        cell_height: 16,
    };
    let pty = tty::new(&options, size, 0).unwrap();
    let ChildEvent::Exited(status) = pty
        .child_watcher()
        .event_rx()
        .recv_timeout(Duration::from_secs(30))
        .unwrap();
    status.and_then(|status| status.code())
}

#[test]
fn batch_shell_launches_with_spaces_and_literal_arguments() {
    use std::io::Write as _;
    let path = std::env::temp_dir().join(format!("qmr & %path% {}.cmd", std::process::id()));
    let mut script = std::fs::File::create_new(&path).unwrap();
    script.write_all(b"@echo off\r\nif \"%~1\"==\"two words\" if \"%~2\"==\"a&b\" if \"%~3\"==\"%%PATH%%\" exit /b 37\r\nexit /b 43\r\n").unwrap();
    drop(script);
    let code = run_pty(Options {
        shell: Some(Shell::new(
            path.display().to_string(),
            ["two words", "a&b", "%PATH%"].map(str::to_owned).to_vec(),
        )),
        env: std::env::vars().collect(),
        clear_env: true,
        ..Default::default()
    });
    std::fs::remove_file(path).unwrap();
    assert_eq!(code, Some(37));
}

#[test]
fn new_powershell_uses_only_the_supplied_environment() {
    const PARENT_ONLY: &str = "QMR_PTY_PARENT_ONLY";
    let executable = std::env::current_exe().unwrap();
    if std::env::var_os(PARENT_ONLY).is_none() {
        // Give the test harness a stale variable without mutating the runner's environment.
        let result = Command::new(&executable)
            .env(PARENT_ONLY, "obsolete")
            .args([
                "--exact",
                "new_powershell_uses_only_the_supplied_environment",
            ])
            .status()
            .unwrap();
        assert!(result.success());
        return;
    }

    let system_root = std::env::var("SystemRoot").unwrap();
    let powershell = std::env::var_os("ProgramFiles")
        .map(|root| PathBuf::from(root).join(r"PowerShell\7\pwsh.exe"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| {
            PathBuf::from(&system_root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe")
        });
    let mut env: HashMap<_, _> = std::env::vars().collect();
    env.retain(|key, _| {
        !key.eq_ignore_ascii_case(PARENT_ONLY) && !key.eq_ignore_ascii_case("PATH")
    });
    env.insert(
        "PATH".into(),
        executable.parent().unwrap().display().to_string(),
    );
    env.insert(
        "QMR_PTY_COMMAND".into(),
        executable.file_name().unwrap().to_str().unwrap().into(),
    );
    let script = r#"
        if (Test-Path Env:QMR_PTY_PARENT_ONLY) { exit 21 }
        if (-not (Get-Command -CommandType Application $env:QMR_PTY_COMMAND -ErrorAction SilentlyContinue)) { exit 22 }
        exit 0
    "#;
    let options = Options {
        shell: Some(Shell::new(
            powershell.display().to_string(),
            vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                script.into(),
            ],
        )),
        env,
        clear_env: true,
        escape_args: true,
        ..Default::default()
    };
    assert_eq!(run_pty(options), Some(0));
}
