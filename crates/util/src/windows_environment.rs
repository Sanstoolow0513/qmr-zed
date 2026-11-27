use anyhow::{Context as _, Result, bail};
use collections::HashMap;
use std::{
    env,
    ffi::OsStr,
    os::windows::{
        ffi::OsStringExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
};
use windows::Win32::{
    Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND,
        HANDLE,
    },
    Globalization::{CSTR_EQUAL, CompareStringOrdinal},
    Security::TOKEN_QUERY,
    Storage::FileSystem::GetBinaryTypeW,
    System::{
        Registry::{HKEY, RegEnumValueW},
        RemoteDesktop::ProcessIdToSessionId,
        SystemInformation::GetSystemDirectoryW,
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
    UI::Shell::GetUserProfileDirectoryW,
};
use windows::core::{HSTRING, PWSTR};
use windows_registry::{CURRENT_USER, Key, LOCAL_MACHINE, Type};

type Environment = HashMap<String, String>;
type RegistryValue = (String, String, bool);

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
    let read = |root: &Key, path: &str| {
        read_layer(root, path).with_context(|| format!("reading Windows {path}"))
    };
    let machine = read(
        LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
    )?;
    let user = read(CURRENT_USER, "Environment")?;
    let logon = read(CURRENT_USER, "Volatile Environment")?;
    let session = read(CURRENT_USER, &format!(r"Volatile Environment\{session}"))?;
    let profile = match session
        .iter()
        .chain(&logon)
        .find(|(name, _, _)| same_name(name, "USERPROFILE"))
    {
        Some((_, value, false)) => value.clone(),
        _ => token_profile_directory()?,
    };
    insert(&mut env, "USERPROFILE".into(), profile);
    apply_layer(&mut env, machine)?;
    apply_account_layers(&mut env, user, logon, session)?;
    Ok(env)
}

fn token_profile_directory() -> Result<String> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)? };
    let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
    let handle = HANDLE(token.as_raw_handle());
    let mut length = 0;
    if let Err(error) = unsafe { GetUserProfileDirectoryW(handle, None, &mut length) }
        && error.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult()
    {
        return Err(error.into());
    }
    let mut buffer = vec![0; length as usize];
    unsafe { GetUserProfileDirectoryW(handle, Some(PWSTR(buffer.as_mut_ptr())), &mut length)? };
    Ok(String::from_utf16(
        buffer.strip_suffix(&[0]).unwrap_or(&buffer),
    )?)
}

fn read_layer(root: &Key, path: &str) -> Result<Vec<RegistryValue>> {
    let key = match root.open(path) {
        Ok(key) => key,
        Err(error)
            if [ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND]
                .iter()
                .any(|code| error.code() == code.to_hresult()) =>
        {
            return Ok(Vec::new());
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
    Ok(values)
}

fn apply_account_layers(
    env: &mut Environment,
    user: Vec<RegistryValue>,
    logon: Vec<RegistryValue>,
    session: Vec<RegistryValue>,
) -> Result<()> {
    // An earlier REG_EXPAND_SZ can depend on an identity supplied at sign-in.
    // Resolve those references without installing a later layer's values or PATH twice.
    let account = [&user, &logon, &session];
    for (index, values) in account.iter().enumerate() {
        let mut upcoming: Vec<&RegistryValue> = Vec::new();
        for entry in account[index + 1..]
            .iter()
            .rev()
            .flat_map(|layer| layer.iter())
        {
            if !same_name(&entry.0, "PATH")
                && !upcoming.iter().any(|other| same_name(&other.0, &entry.0))
            {
                upcoming.push(entry);
            }
        }
        if upcoming.is_empty() {
            apply_layer(env, values.to_vec())?;
            continue;
        }
        let mut projected = env.clone();
        apply_layer(env, values.to_vec())?;

        let mut definitions = Vec::new();
        let mut resolved_names = Vec::new();
        for entry in values.iter() {
            if upcoming.iter().any(|other| same_name(&other.0, &entry.0)) {
                // A sign-in variable can itself reference the previous value of its name.
                if let Some(value) = get(env, &entry.0) {
                    insert(&mut projected, entry.0.clone(), value.into());
                }
            } else {
                resolved_names.push(&entry.0);
                definitions.push(entry.clone());
            }
        }
        definitions.extend(upcoming.into_iter().cloned());
        apply_layer(&mut projected, definitions)?;
        for name in resolved_names {
            if let Some(value) = get(&projected, name) {
                insert(env, name.clone(), value.into());
            }
        }
    }
    Ok(())
}

fn apply_layer(env: &mut Environment, values: Vec<RegistryValue>) -> Result<()> {
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

/// Locate a program that an adapter can launch directly in its project environment.
pub async fn which(
    program: &OsStr,
    directory: PathBuf,
    environment: Environment,
) -> Option<PathBuf> {
    let program = program.to_owned();
    smol::unblock(move || resolve_native_program(&program, Some(&directory), &environment).ok())
        .await
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
        find(system_directory()?.join(path)).or_else(|| {
            get(env, "PATH")
                .into_iter()
                .flat_map(env::split_paths)
                .filter(|directory| !directory.as_os_str().is_empty())
                .find_map(|directory| find(cwd.join(directory).join(path)))
        })
    };
    match found {
        Some(executable) => Ok(std::path::absolute(executable)?),
        None => bail!(
            "could not find {} in the launch environment",
            path.display()
        ),
    }
}

fn system_directory() -> Result<PathBuf> {
    let mut buffer = vec![0; 260];
    loop {
        let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
        if length == 0 {
            return Err(windows::core::Error::from_thread().into());
        }
        if length < buffer.len() {
            return Ok(std::ffi::OsString::from_wide(&buffer[..length]).into());
        }
        buffer.resize(length + 1, 0);
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
    fn expands_user_values_with_final_sign_in_identity() -> Result<()> {
        let mut env = Environment::from_iter([
            ("USERPROFILE".into(), r"C:\launcher".into()),
            ("PATH".into(), "machine".into()),
        ]);
        apply_account_layers(
            &mut env,
            vec![
                ("TEMP".into(), r"%UserProfile%\Temp".into(), true),
                ("Path".into(), r"%SDK%\bin".into(), true),
            ],
            vec![
                ("USERPROFILE".into(), r"C:\profile".into(), false),
                ("SDK".into(), r"%USERPROFILE%\sdk".into(), true),
            ],
            vec![
                ("userprofile".into(), r"D:\session".into(), false),
                ("Path".into(), "session-bin".into(), false),
            ],
        )?;
        assert_eq!(get(&env, "TEMP"), Some(r"D:\session\Temp"));
        assert_eq!(
            get(&env, "PATH"),
            Some(r"machine;D:\session\sdk\bin;session-bin")
        );
        assert!(Path::new(&token_profile_directory()?).is_absolute());
        Ok(())
    }

    #[test]
    fn system_directory_precedes_project_path_for_bare_programs() -> Result<()> {
        let sdk = tempfile::tempdir()?;
        let system_cmd = system_directory()?.join("cmd.exe");
        let sdk_cmd = sdk.path().join("cmd.exe");
        std::fs::copy(&system_cmd, &sdk_cmd)?;
        let env = Environment::from_iter([("PATH".into(), sdk.path().display().to_string())]);
        assert_eq!(
            resolve_native_program("cmd.exe".as_ref(), Some(sdk.path()), &env)?,
            system_cmd
        );
        assert_eq!(resolve_program("cmd.exe".as_ref(), None, &env)?, system_cmd);
        assert_eq!(
            resolve_native_program(r".\cmd.exe".as_ref(), Some(sdk.path()), &env)?,
            sdk_cmd
        );
        assert_eq!(
            resolve_native_program("cmd.exe".as_ref(), None, &Environment::default())?,
            system_cmd
        );
        Ok(())
    }

    #[test]
    fn sign_in_expansion_preserves_each_layers_path_and_previous_value() -> Result<()> {
        let mut env = Environment::from_iter([("PATH".into(), "machine".into())]);
        apply_account_layers(
            &mut env,
            vec![
                ("Path".into(), "user".into(), false),
                ("SDK".into(), "base".into(), false),
            ],
            vec![
                ("Path".into(), "logon".into(), false),
                ("SDK".into(), "%SDK%-login".into(), true),
                ("AT_LOGON".into(), "%PATH%".into(), true),
            ],
            vec![("Path".into(), "session".into(), false)],
        )?;
        assert_eq!(get(&env, "SDK"), Some("base-login"));
        assert_eq!(get(&env, "AT_LOGON"), Some("machine;user;logon"));
        assert_eq!(get(&env, "PATH"), Some("machine;user;logon;session"));
        Ok(())
    }

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
        let powershell = program.with_extension("ps1");
        let root = env::var("SystemRoot")?;
        let env = Environment::from_iter([
            ("SystemRoot".into(), root.clone()),
            ("PATH".into(), directory.path().display().to_string()),
            ("PATHEXT".into(), ".ps1;.cmd;.exe".into()),
        ]);
        std::fs::write(&program, "#!/bin/sh\nexit 1\n")?;
        std::fs::write(&powershell, "exit 1\n")?;
        let discover =
            || smol::block_on(which("npm".as_ref(), directory.path().into(), env.clone()));
        assert!(discover().is_none());
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
        assert_eq!(
            resolve_program(program.as_os_str(), None, &env)?,
            powershell
        );
        let discovered = discover().context("expected a launchable npm shim")?;
        assert_eq!(discovered, batch);
        assert_eq!(launch(discovered.as_os_str(), &[])?, Some(37));
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
