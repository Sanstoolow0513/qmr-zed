use super::{Entry, Sidebar};
use gpui::{App, AppContext as _, AsyncApp, Context, EntityId, Global};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use terminal_view::workspace_terminal_panel::{TerminalId, WorkspaceTerminalPanel};
use ui::IconName;
use util::ResultExt as _;
use windows::{
    Win32::{
        Foundation::ERROR_NO_MORE_FILES,
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
    },
    core::{HRESULT, Owned},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalAgent {
    Codex,
    OpenCode,
}

#[derive(Default)]
pub(super) struct AgentMonitor {
    // A restored terminal keeps its TerminalId but gets a fresh GPUI entity and shell.
    sidebars: HashMap<EntityId, HashMap<TerminalId, (EntityId, u32)>>,
    agents: HashMap<EntityId, TerminalAgent>,
    polling: bool,
}

impl Global for AgentMonitor {}

impl AgentMonitor {
    pub(super) fn observe_sidebar(cx: &mut Context<Sidebar>) {
        cx.observe_global::<Self>(|_, cx| cx.notify()).detach();
        let sidebar_id = cx.entity_id();
        cx.on_release(move |_, cx| Self::remove_sidebar(sidebar_id, cx))
            .detach();
    }

    fn remove_sidebar(sidebar_id: EntityId, cx: &mut App) {
        if cx
            .try_global::<Self>()
            .is_some_and(|monitor| monitor.sidebars.contains_key(&sidebar_id))
        {
            cx.global_mut::<Self>().sidebars.remove(&sidebar_id);
        }
    }

    pub(super) fn watch_entries(entries: &[Entry], visible: bool, cx: &mut Context<Sidebar>) {
        let sidebar_id = cx.entity_id();
        if !visible {
            Self::remove_sidebar(sidebar_id, cx);
            return;
        }
        let terminals: HashMap<_, _> = entries
            .iter()
            .filter_map(|entry| {
                let Entry::Terminal {
                    workspace: Some(workspace),
                    metadata,
                    ..
                } = entry
                else {
                    return None;
                };
                let panel = workspace.read(cx).panel::<WorkspaceTerminalPanel>(cx)?;
                let process = panel
                    .read(cx)
                    .local_terminal_process(metadata.terminal_id, cx)?;
                Some((metadata.terminal_id, process))
            })
            .collect();
        if terminals.is_empty() {
            Self::remove_sidebar(sidebar_id, cx);
            return;
        }
        if cx
            .try_global::<Self>()
            .and_then(|monitor| monitor.sidebars.get(&sidebar_id))
            == Some(&terminals)
        {
            return;
        }
        let monitor = cx.default_global::<Self>();
        monitor.sidebars.insert(sidebar_id, terminals);
        if !monitor.polling {
            monitor.polling = true;
            App::spawn(cx, Self::poll).detach();
        }
    }

    async fn poll(cx: &mut AsyncApp) {
        loop {
            let processes: HashMap<EntityId, u32> = cx.update(|cx| {
                cx.global::<Self>()
                    .sidebars
                    .values()
                    .flat_map(|terminals| terminals.values().copied())
                    .collect()
            });
            if processes.is_empty() {
                cx.update(|cx| {
                    let monitor = cx.global_mut::<Self>();
                    monitor.polling = false;
                    monitor.agents.clear();
                });
                return;
            }
            let agents = cx
                .background_spawn(async move {
                    let roots = processes.values().copied().collect::<Vec<_>>();
                    let detected = detect_agents(&roots);
                    processes
                        .into_iter()
                        .filter_map(|(entity, pid)| Some((entity, *detected.get(&pid)?)))
                        .collect()
                })
                .await;
            cx.update(|cx| {
                if cx.global::<Self>().agents != agents {
                    cx.global_mut::<Self>().agents = agents;
                }
            });
            cx.background_executor().timer(Duration::from_secs(1)).await;
        }
    }

    pub(super) fn icon_for(terminal_id: TerminalId, cx: &Context<Sidebar>) -> Option<IconName> {
        let monitor = cx.try_global::<Self>()?;
        let (entity, _) = monitor.sidebars.get(&cx.entity_id())?.get(&terminal_id)?;
        Some(match monitor.agents.get(entity)? {
            TerminalAgent::Codex => IconName::AiOpenAi,
            TerminalAgent::OpenCode => IconName::AiOpenCode,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProcessKind {
    Launcher,
    Agent(TerminalAgent),
}

impl ProcessKind {
    fn for_executable(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case("codex.exe") {
            Some(Self::Agent(TerminalAgent::Codex))
        } else if name.eq_ignore_ascii_case("opencode.exe") {
            Some(Self::Agent(TerminalAgent::OpenCode))
        } else if [
            "pwsh.exe",
            "powershell.exe",
            "cmd.exe",
            "node.exe",
            "bun.exe",
        ]
        .iter()
        .any(|launcher| name.eq_ignore_ascii_case(launcher))
        {
            Some(Self::Launcher)
        } else {
            None
        }
    }
}

struct Process {
    parent: u32,
    kind: ProcessKind,
}

fn detect_agents(roots: &[u32]) -> HashMap<u32, TerminalAgent> {
    if roots.is_empty() {
        return HashMap::default();
    }
    process_snapshot()
        .log_err()
        .map(|processes| agents_for_roots(roots, &processes))
        .unwrap_or_default()
}

fn process_snapshot() -> windows::core::Result<HashMap<u32, Process>> {
    let snapshot = unsafe { Owned::new(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?) };
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut processes = HashMap::default();
    let mut next = unsafe { Process32FirstW(*snapshot, &mut entry) };
    loop {
        if let Err(error) = next {
            if error.code() == HRESULT::from_win32(ERROR_NO_MORE_FILES.0) {
                break;
            }
            return Err(error);
        }
        let name_end = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..name_end]);
        if let Some(kind) = ProcessKind::for_executable(&name) {
            processes.insert(
                entry.th32ProcessID,
                Process {
                    parent: entry.th32ParentProcessID,
                    kind,
                },
            );
        }
        next = unsafe { Process32NextW(*snapshot, &mut entry) };
    }
    Ok(processes)
}

fn agents_for_roots(
    roots: &[u32],
    processes: &HashMap<u32, Process>,
) -> HashMap<u32, TerminalAgent> {
    let roots: HashSet<_> = roots.iter().copied().collect();
    let mut candidates = HashMap::new();
    for (&pid, process) in processes {
        if let ProcessKind::Agent(agent) = process.kind
            && let Some(root) = owning_root(pid, &roots, processes)
        {
            // Sibling agent processes do not tell us which one owns the shell's input.
            candidates
                .entry(root)
                .and_modify(|candidate| *candidate = None)
                .or_insert(Some(agent));
        }
    }
    candidates
        .into_iter()
        .filter_map(|(root, agent)| Some((root, agent?)))
        .collect()
}

fn owning_root(
    mut pid: u32,
    roots: &HashSet<u32>,
    processes: &HashMap<u32, Process>,
) -> Option<u32> {
    // A terminated parent may leave a stale parent PID that has since been recycled.
    // Bound the walk so even an apparent cycle cannot stall a sidebar refresh.
    for _ in 0..processes.len() {
        if roots.contains(&pid) {
            return Some(pid);
        }
        let parent = processes.get(&pid)?.parent;
        if processes.get(&parent)?.kind != ProcessKind::Launcher {
            return None;
        }
        pid = parent;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(roots: &[u32], entries: &[(u32, u32, &str)]) -> HashMap<u32, TerminalAgent> {
        let processes = entries
            .iter()
            .filter_map(|&(pid, parent, name)| {
                Some((
                    pid,
                    Process {
                        parent,
                        kind: ProcessKind::for_executable(name)?,
                    },
                ))
            })
            .collect();
        agents_for_roots(roots, &processes)
    }

    #[test]
    fn matches_native_agents_and_shell_wrappers_to_the_nearest_terminal() {
        let agents = classify(
            &[1, 5, 6, 8],
            &[
                (1, 0, "pwsh.exe"),
                (2, 1, "cmd.exe"),
                (3, 2, "node.exe"),
                (4, 3, "CODEX.EXE"),
                (5, 1, "powershell.exe"),
                (6, 1, "opencode.exe"),
                (7, 5, "opencode.exe"),
                (8, 1, "pwsh.exe"),
                (9, 4, "opencode.exe"),
            ],
        );
        assert_eq!(
            agents,
            [
                (1, TerminalAgent::Codex),
                (5, TerminalAgent::OpenCode),
                (6, TerminalAgent::OpenCode)
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn skips_unrelated_processes_and_ambiguous_agents() {
        let agents = classify(
            &[1, 10, 20, 30],
            &[
                (1, 0, "pwsh.exe"),
                (2, 1, "codex.exe"),
                (3, 1, "opencode.exe"),
                (10, 0, "pwsh.exe"),
                (11, 10, "codex-helper.exe"),
                (12, 10, "application.exe"),
                (13, 12, "codex.exe"),
                (21, 20, "codex.exe"),
                (30, 0, "pwsh.exe"),
                (31, 30, "codex.exe"),
                (32, 30, "codex.exe"),
                (40, 41, "cmd.exe"),
                (41, 40, "node.exe"),
                (42, 40, "opencode.exe"),
            ],
        );
        assert!(agents.is_empty());
    }
}
