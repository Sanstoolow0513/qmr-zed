use anyhow::{Context as _, Result, bail};
use collections::HashMap;
use std::{
    env,
    ffi::OsStr,
    path::{Path, PathBuf},
};
use windows::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND},
    Globalization::{CSTR_EQUAL, CompareStringOrdinal},
    Storage::FileSystem::GetBinaryTypeW,
    System::{
        Registry::{HKEY, RegEnumValueW},
        RemoteDesktop::ProcessIdToSessionId,
    },
};
use windows::core::{HSTRING, PWSTR};
use windows_registry::{CURRENT_USER, Key, LOCAL_MACHINE, Type};

type Environment = HashMap<String, String>;

fn same_name(left: &str, right: &str) -> bool {
    if left.is_ascii() && right.is_ascii() {
        return left.eq_ignore_ascii_case(right);
    }
    let left: Vec<_> = left.encode_utf16().collect();
    let right: Vec<_> = right.encode_utf16().collect();
    unsafe { CompareStringOrdinal(&left, &right, true) == CSTR_EQUAL }
}

fn get<'a>(env: &'a Environment, name: &str) -> Option<&'a str> {
    env.get(name)
        .or_else(|| {
            env.iter()
                .find(|(key, _)| same_name(key, name))
                .map(|(_, v)| v)
        })
        .map(String::as_str)
}

fn insert(env: &mut Environment, mut name: String, value: String) {
    env.retain(|key, _| !same_name(key, &name));
    // Project tools also use this map directly for PATH lookups.
    if name.eq_ignore_ascii_case("PATH") || name.eq_ignore_ascii_case("PATHEXT") {
        name.make_ascii_uppercase();
    }
    env.insert(name, value);
}

pub(crate) fn extend(env: &mut Environment, overrides: impl IntoIterator<Item = (String, String)>) {
    let mut overrides: Vec<_> = overrides.into_iter().collect();
    // Settings maps can contain both "PATH" and "Path". Prefer the uppercase spelling
    // within the same layer, independently of the map's iteration order.
    overrides.sort_by(|(left, _), (right, _)| right.cmp(left));
    for (name, value) in overrides {
        insert(env, name, value);
    }
}

/// Read a fresh environment for a process in the current logon session.
/// This is synchronous registry I/O; callers should use a background executor.
pub fn fresh() -> Result<Environment> {
    let mut env = Environment::default();
    // These identity/profile values belong to the logon session. Importing all of
    // vars_os() here would also resurrect persistent variables the user deleted.
    for name in [
        "SystemRoot",
        "SystemDrive",
        "ALLUSERSPROFILE",
        "PUBLIC",
        "ProgramData",
        "COMPUTERNAME",
        "USERNAME",
        "USERDOMAIN",
        "USERDNSDOMAIN",
        "HOMEDRIVE",
        "HOMESHARE",
        "HOMEPATH",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
    ] {
        if let Ok(value) = env::var(name) {
            env.insert(name.to_owned(), value);
        }
    }

    let program_dirs = LOCAL_MACHINE.open(r"Software\Microsoft\Windows\CurrentVersion")?;
    for (value_name, name) in [
        ("ProgramFilesDir", "ProgramFiles"),
        ("CommonFilesDir", "CommonProgramFiles"),
        #[cfg(target_pointer_width = "64")]
        ("ProgramFilesDir (x86)", "ProgramFiles(x86)"),
        #[cfg(target_pointer_width = "64")]
        ("CommonFilesDir (x86)", "CommonProgramFiles(x86)"),
        #[cfg(target_pointer_width = "64")]
        ("ProgramW6432Dir", "ProgramW6432"),
        #[cfg(target_pointer_width = "64")]
        ("CommonW6432Dir", "CommonProgramW6432"),
        #[cfg(target_arch = "aarch64")]
        ("ProgramFilesDir (Arm)", "ProgramFiles(Arm)"),
        #[cfg(target_arch = "aarch64")]
        ("CommonFilesDir (Arm)", "CommonProgramFiles(Arm)"),
    ] {
        match program_dirs.get_string(value_name) {
            Ok(path) => {
                env.insert(name.into(), path);
            }
            Err(error) if error.code() == ERROR_FILE_NOT_FOUND.to_hresult() => {}
            Err(error) => return Err(error).context("reading Windows program directories"),
        }
    }

    let mut session = 0;
    unsafe { ProcessIdToSessionId(std::process::id(), &mut session)? };
    for (root, path) in [
        (
            LOCAL_MACHINE,
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
        ),
        (CURRENT_USER, "Environment"),
        (CURRENT_USER, "Volatile Environment"),
        (CURRENT_USER, &format!(r"Volatile Environment\{session}")),
    ] {
        read_layer(root, path, &mut env).with_context(|| format!("reading Windows {path}"))?;
    }
    Ok(env)
}

fn read_layer(root: &Key, path: &str, env: &mut Environment) -> Result<()> {
    let key = match root.open(path) {
        Ok(key) => key,
        Err(error)
            if [ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND]
                .iter()
                .any(|code| error.code() == code.to_hresult()) =>
        {
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    let mut values = Vec::new();
    // ValueIterator sizes its buffers once and treats a growing value as EOF.
    // Enumerate names (16,383 UTF-16 units maximum), then read each value so a
    // concurrent installer write produces a retryable error, not a truncated snapshot.
    let mut name_buf = vec![0u16; 16_384];
    for index in 0.. {
        let mut length = name_buf.len() as u32;
        let result = unsafe {
            RegEnumValueW(
                HKEY(key.as_raw()),
                index,
                Some(PWSTR(name_buf.as_mut_ptr())),
                &mut length,
                None,
                None,
                None,
                None,
            )
        };
        if result == ERROR_NO_MORE_ITEMS {
            break;
        }
        result.ok()?;
        let name = String::from_utf16(&name_buf[..length as usize])?;
        let value = key.get_value(&name)?;
        let ty = value.ty();
        if !name.is_empty()
            && !name.contains('=')
            && matches!(ty, Type::String | Type::ExpandString)
        {
            let value = String::try_from(value)?;
            anyhow::ensure!(!value.contains('\0'), "environment value contains a NUL");
            values.push((name, value, ty == Type::ExpandString));
        }
    }
    apply_layer(env, values)
}

fn apply_layer(env: &mut Environment, values: Vec<(String, String, bool)>) -> Result<()> {
    let mut expandable = Vec::new();
    for (name, value, expand) in values {
        if expand || name.eq_ignore_ascii_case("PATH") {
            expandable.push((name, value));
        } else {
            insert(env, name, value);
        }
    }
    expandable.sort_by(|(a, _), (b, _)| b.cmp(a));
    while let Some((name, value)) = expandable.pop() {
        install_expanded(name, &value, &mut expandable, env, 0)?;
    }
    Ok(())
}

fn install_expanded(
    name: String,
    mut input: &str,
    pending: &mut Vec<(String, String)>,
    env: &mut Environment,
    depth: usize,
) -> Result<()> {
    anyhow::ensure!(
        depth < 128,
        "environment variable references nested too deeply"
    );
    let mut result = String::with_capacity(input.len());
    while let Some((prefix, rest)) = input.split_once('%') {
        result.push_str(prefix);
        let Some((reference, tail)) = rest.split_once('%') else {
            result.push('%');
            input = rest;
            break;
        };
        // Resolve SDK/root variables independently of registry enumeration order.
        // Removing an entry before descending also terminates self/cyclic references.
        if let Some(index) = pending
            .iter()
            .position(|(key, _)| same_name(key, reference))
        {
            let (key, value) = pending.remove(index);
            install_expanded(key, &value, pending, env, depth + 1)?;
        }
        if let Some(value) = get(env, reference) {
            result.push_str(value);
        } else {
            result.push('%');
            result.push_str(reference);
            result.push('%');
        }
        // One UTF-16 code unit needs at most three UTF-8 bytes. Bound recursive
        // expansion of repeated references before allocating another copy.
        anyhow::ensure!(
            result.len() <= 32_767 * 3,
            "expanded environment variable too large"
        );
        input = tail;
    }
    result.push_str(input);
    if name.eq_ignore_ascii_case("PATH")
        && let Some(parent) = get(env, &name).filter(|p| !p.is_empty())
    {
        if result.is_empty() {
            return Ok(());
        }
        if !parent.ends_with(';') {
            result.insert(0, ';');
        }
        result.insert_str(0, parent);
    }
    insert(env, name, result);
    Ok(())
}

/// Find a named program or script using the child's PATH and PATHEXT.
pub fn resolve_program(program: &OsStr, cwd: Option<&Path>, env: &Environment) -> Result<PathBuf> {
    resolve_with_extensions(program, cwd, env, |_| true)
}

/// Select an executable or command script which can be launched without a file
/// association. PATHEXT can also include documents or interpreter-specific scripts.
pub fn resolve_native_program(
    program: &OsStr,
    cwd: Option<&Path>,
    env: &Environment,
) -> Result<PathBuf> {
    resolve_with_extensions(program, cwd, env, |extension| {
        ["exe", "com", "cmd", "bat"]
            .iter()
            .any(|native| extension.eq_ignore_ascii_case(native))
    })
}

fn resolve_with_extensions(
    program: &OsStr,
    cwd: Option<&Path>,
    env: &Environment,
    accepts: impl Fn(&str) -> bool,
) -> Result<PathBuf> {
    let path = Path::new(program);
    anyhow::ensure!(!program.is_empty(), "empty program name");
    let explicit_path = path.components().count() > 1 || path.has_root();
    let extensions = get(env, "PATHEXT").unwrap_or(".COM;.EXE;.BAT;.CMD");
    let find = |candidate: PathBuf| {
        if candidate.is_file()
            && (candidate
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(&accepts)
                || is_windows_binary(&candidate))
        {
            return Some(candidate);
        }
        extensions
            .split(';')
            .map(str::trim)
            .filter(|ext| ext.strip_prefix('.').is_some_and(&accepts))
            .find_map(|ext| {
                let mut name = candidate.as_os_str().to_owned();
                name.push(ext);
                let name = PathBuf::from(name);
                name.is_file().then_some(name)
            })
    };
    let cwd = match cwd {
        Some(cwd) => std::path::absolute(cwd)?,
        None => env::current_dir()?,
    };
    let found = if explicit_path {
        find(cwd.join(path))
    } else {
        get(env, "PATH")
            .into_iter()
            .flat_map(env::split_paths)
            .filter(|directory| !directory.as_os_str().is_empty())
            .find_map(|directory| find(cwd.join(directory).join(path)))
    };
    match found {
        Some(executable) => Ok(std::path::absolute(executable)?),
        None => bail!(
            "could not find {} in the launch environment",
            path.display()
        ),
    }
}

// A name without a recognized extension can be either a PE image or a Unix shim
// installed alongside a Windows batch file.
fn is_windows_binary(path: &Path) -> bool {
    let mut binary_type = 0;
    unsafe { GetBinaryTypeW(&HSTRING::from(path.as_os_str()), &mut binary_type).is_ok() }
}

pub fn system_shell(env: &Environment) -> String {
    gpui_util::find_powershell(
        |key| get(env, key).map(PathBuf::from),
        |name| resolve_native_program(OsStr::new(name), None, env).ok(),
    )
    .unwrap_or_else(|| {
        Path::new(get(env, "SystemRoot").unwrap_or(r"C:\Windows"))
            .join(r"System32\cmd.exe")
            .to_string_lossy()
            .into_owned()
    })
}

/// Resolve the launch program while retaining arguments, display name and shell semantics.
pub fn resolve_shell(
    shell: &crate::shell::Shell,
    cwd: Option<&Path>,
    env: &Environment,
) -> Result<crate::shell::Shell> {
    use crate::shell::Shell;
    let program = match shell {
        Shell::System => system_shell(env),
        Shell::Program(program) | Shell::WithArguments { program, .. } => program.clone(),
    };
    let program = resolve_native_program(OsStr::new(&program), cwd, env)?
        .into_os_string()
        .into_string()
        .map_err(|_| anyhow::anyhow!("shell program path is not Unicode"))?;
    Ok(match shell {
        Shell::WithArguments {
            args,
            title_override,
            ..
        } => Shell::WithArguments {
            program,
            args: args.clone(),
            title_override: title_override.clone(),
        },
        _ => Shell::Program(program),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_registry_layers_and_expands_against_the_child_environment() -> Result<()> {
        let mut env = Environment::default();
        apply_layer(
            &mut env,
            vec![
                ("Path".into(), r"%sdk%\bin".into(), false),
                ("SDK".into(), r"%toolchain%\sdk".into(), true),
                ("toolchain".into(), "%root%".into(), true),
                ("ROOT".into(), r"C:\Program Files".into(), false),
                ("Example".into(), "machine".into(), false),
            ],
        )?;
        apply_layer(
            &mut env,
            vec![
                ("PATH".into(), r"%TOOLS%;%MISSING%".into(), true),
                ("Tools".into(), r"D:\User Tools".into(), false),
                ("EXAMPLE".into(), String::new(), false),
            ],
        )?;
        assert_eq!(
            get(&env, "Path"),
            Some(r"C:\Program Files\sdk\bin;D:\User Tools;%MISSING%")
        );
        assert_eq!(get(&env, "example"), Some(""));
        assert!(!env.contains_key("Path"));
        assert!(!env.contains_key("Example"));
        Ok(())
    }

    #[test]
    fn explicit_overrides_replace_case_variants_instead_of_appending() {
        let mut env = Environment::default();
        extend(
            &mut env,
            [("Path".into(), "system".into()), ("Ä".into(), "old".into())],
        );
        extend(
            &mut env,
            [
                ("Path".into(), "mixed".into()),
                ("PATH".into(), "explicit".into()),
                ("ä".into(), "new".into()),
            ],
        );
        assert_eq!(env.len(), 2);
        assert_eq!(get(&env, "PATH"), Some("explicit"));
        assert_eq!(get(&env, "Ä"), Some("new"));
    }

    #[test]
    fn resolves_an_executable_using_the_supplied_directory_path_and_extensions() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let bin = directory.path().join("tool bin");
        std::fs::create_dir(&bin)?;
        let program = bin.join("probe.QMR");
        std::fs::write(&program, [])?;
        let executable = bin.join("probe.EXE");
        std::fs::write(&executable, [])?;
        let mut env = Environment::default();
        extend(
            &mut env,
            [
                ("Path".into(), "tool bin".into()),
                ("PATHEXT".into(), ".QMR;.EXE".into()),
            ],
        );
        assert_eq!(
            resolve_program(OsStr::new("probe"), Some(directory.path()), &env)?,
            program
        );
        let shell = crate::shell::Shell::WithArguments {
            program: "probe".into(),
            args: vec!["--login".into()],
            title_override: Some("SDK Shell".into()),
        };
        let resolved = resolve_shell(&shell, Some(directory.path()), &env)?;
        let crate::shell::Shell::WithArguments {
            program: shell_program,
            args,
            title_override,
        } = resolved
        else {
            panic!("configured shell arguments were lost")
        };
        assert_eq!(Path::new(&shell_program), executable);
        assert_eq!(args, ["--login"]);
        assert_eq!(title_override.as_deref(), Some("SDK Shell"));
        env.remove("PATH");
        assert!(resolve_program(OsStr::new("probe"), Some(directory.path()), &env).is_err());
        assert_eq!(
            resolve_program(
                OsStr::new(r".\tool bin\probe.QMR"),
                Some(directory.path()),
                &env
            )?,
            program
        );
        Ok(())
    }

    #[test]
    fn launches_windows_shims_and_extensionless_binaries() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let program = directory.path().join("npm");
        let batch = program.with_extension("cmd");
        let root = env::var("SystemRoot")?;
        let env = Environment::from_iter([
            ("SystemRoot".into(), root.clone()),
            ("PATH".into(), directory.path().display().to_string()),
            ("PATHEXT".into(), ".cmd;.exe".into()),
        ]);
        std::fs::write(&program, "#!/bin/sh\nexit 1\n")?;
        std::fs::write(&batch, "@exit /b 37\r\n")?;
        let launch = |name: &OsStr, args: &[&str]| -> Result<_> {
            Ok(crate::command::new_std_command_with_environment(
                name,
                Some(directory.path()),
                &env,
            )?
            .args(args)
            .status()?
            .code())
        };
        assert_eq!(resolve_program(program.as_os_str(), None, &env)?, batch);
        assert_eq!(launch(program.as_os_str(), &[])?, Some(37));

        std::fs::copy(Path::new(&root).join(r"System32\cmd.exe"), &program)?;
        for name in [program.as_os_str(), OsStr::new("npm")] {
            assert_eq!(resolve_native_program(name, None, &env)?, program);
            assert_eq!(launch(name, &["/d", "/c", "exit 42"])?, Some(42));
        }
        Ok(())
    }

    #[test]
    fn fresh_snapshot_can_start_a_windows_process() -> Result<()> {
        let env = fresh()?;
        let root = get(&env, "SystemRoot").context("missing SystemRoot")?;
        let powershell = Path::new(root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        let output = gpui_util::new_std_command(powershell)
            .env_clear()
            .envs(&env)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[bool]($env:Path -and $env:USERPROFILE -and $env:SystemRoot)",
            ])
            .output()?;
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout)?.trim(), "True");
        Ok(())
    }
}
