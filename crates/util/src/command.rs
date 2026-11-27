use std::ffi::OsStr;
use std::path::Path;

#[cfg(target_os = "macos")]
mod darwin;

#[cfg(target_os = "macos")]
pub use darwin::{Child, Command, Stdio};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000_u32;

pub use gpui_util::new_std_command;

pub fn new_command(program: impl AsRef<OsStr>) -> Command {
    Command::new(program)
}

#[cfg(windows)]
pub fn new_std_command_with_environment(
    program: &OsStr,
    directory: Option<&Path>,
    environment: &collections::HashMap<String, String>,
) -> anyhow::Result<std::process::Command> {
    let program =
        crate::windows_environment::resolve_native_program(program, directory, environment)?;
    let mut command = new_std_command(program);
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    command.env_clear().envs(environment);
    Ok(command)
}

/// Launch a project tool with the same environment used to locate its executable.
pub fn new_command_with_environment(
    program: impl AsRef<OsStr>,
    directory: &Path,
    environment: Option<&collections::HashMap<String, String>>,
) -> anyhow::Result<Command> {
    let program = program.as_ref();
    #[cfg(windows)]
    if let Some(env) = environment {
        let command = new_std_command_with_environment(program, Some(directory), env)?;
        return Ok(Command(smol::process::Command::from(command)));
    }
    let mut command = new_command(program);
    command.current_dir(directory);
    if let Some(env) = environment {
        command.envs(env);
    }
    Ok(command)
}

/// Resolve and spawn on an I/O worker, then read the child's pipes asynchronously.
pub async fn output_with_environment(
    program: impl AsRef<OsStr>,
    directory: &Path,
    environment: Option<collections::HashMap<String, String>>,
    arguments: impl IntoIterator<Item = impl Into<std::ffi::OsString>>,
) -> anyhow::Result<std::process::Output> {
    let program = program.as_ref().to_owned();
    let directory = directory.to_owned();
    let arguments: Vec<_> = arguments.into_iter().map(Into::into).collect();
    let child = smol::unblock(move || {
        let mut command = new_command_with_environment(program, &directory, environment.as_ref())?;
        anyhow::Ok(
            command
                .args(arguments)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?,
        )
    })
    .await?;
    Ok(child.output().await?)
}

#[cfg(all(test, windows))]
#[test]
fn project_commands_use_the_supplied_search_path_and_environment() -> anyhow::Result<()> {
    use tempfile::tempdir;

    let directory = tempdir()?;
    let system_root = std::env::var("SystemRoot")?;
    // Use an executable that is only on the project's PATH.
    let executable = directory.path().join("qmr-env-probe.exe");
    std::fs::copy(
        Path::new(&system_root).join(r"System32\cmd.exe"),
        &executable,
    )?;
    let env = collections::HashMap::from_iter([
        ("SystemRoot".into(), system_root),
        ("PATH".into(), directory.path().display().to_string()),
        ("QMR_SDK".into(), "active".into()),
    ]);
    let mut command = new_command_with_environment("qmr-env-probe", directory.path(), Some(&env))?;
    assert_eq!(
        Path::new(command.get_program()).canonicalize()?,
        executable.canonicalize()?
    );
    let output = smol::block_on(
        command
            .args([
                "/D",
                "/C",
                "echo %QMR_SDK%& if defined USERPROFILE exit /b 1",
            ])
            .output(),
    )?;
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout)?.trim(), "active");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub type Child = smol::process::Child;

#[cfg(not(target_os = "macos"))]
pub use std::process::Stdio;

#[cfg(not(target_os = "macos"))]
#[derive(Debug)]
pub struct Command(smol::process::Command);

#[cfg(not(target_os = "macos"))]
impl Command {
    #[inline]
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        #[cfg(target_os = "windows")]
        {
            use smol::process::windows::CommandExt;
            let mut cmd = smol::process::Command::new(program);
            cmd.creation_flags(CREATE_NO_WINDOW);
            Self(cmd)
        }
        #[cfg(not(target_os = "windows"))]
        Self(smol::process::Command::new(program))
    }

    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.0.arg(arg);
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.0.args(args);
        self
    }

    pub fn get_args(&self) -> impl Iterator<Item = &OsStr> {
        self.0.get_args()
    }

    pub fn env(&mut self, key: impl AsRef<OsStr>, val: impl AsRef<OsStr>) -> &mut Self {
        self.0.env(key, val);
        self
    }

    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.0.envs(vars);
        self
    }

    pub fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        self.0.env_remove(key);
        self
    }

    pub fn env_clear(&mut self) -> &mut Self {
        self.0.env_clear();
        self
    }

    pub fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
        self.0.current_dir(dir);
        self
    }

    pub fn stdin(&mut self, cfg: impl Into<Stdio>) -> &mut Self {
        self.0.stdin(cfg.into());
        self
    }

    pub fn stdout(&mut self, cfg: impl Into<Stdio>) -> &mut Self {
        self.0.stdout(cfg.into());
        self
    }

    pub fn stderr(&mut self, cfg: impl Into<Stdio>) -> &mut Self {
        self.0.stderr(cfg.into());
        self
    }

    pub fn kill_on_drop(&mut self, kill_on_drop: bool) -> &mut Self {
        self.0.kill_on_drop(kill_on_drop);
        self
    }

    pub fn spawn(&mut self) -> std::io::Result<Child> {
        self.0.spawn()
    }

    pub async fn output(&mut self) -> std::io::Result<std::process::Output> {
        self.0.output().await
    }

    pub async fn status(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.0.status().await
    }

    pub fn get_program(&self) -> &OsStr {
        self.0.get_program()
    }
}
