//! A process's working directory, command name and process group's members,
//! read from `/proc` on Linux and from the kernel's process info (libc's
//! `proc_pidinfo`, `proc_listpids`) on macOS, without starting a process:
//! the server asks for every pane on every focus change.
//!
//! STUB: the signatures are the contract; step 2 fills them in.

use std::path::PathBuf;

/// The process's working directory.
pub fn cwd_of(pid: u32) -> Option<PathBuf> {
    let _ = pid;
    todo!("procinfo::cwd_of")
}

/// The process's command name (`zsh`, `claude`, `2.1.3`, `node`), as `ps -o
/// comm` shows it without its path.
pub fn command_of(pid: u32) -> Option<String> {
    let _ = pid;
    todo!("procinfo::command_of")
}

/// The process's arguments, its program first.
pub fn args_of(pid: u32) -> Option<Vec<String>> {
    let _ = pid;
    todo!("procinfo::args_of")
}

/// Every process in a process group.
pub fn group_members(pgid: u32) -> Vec<u32> {
    let _ = pgid;
    todo!("procinfo::group_members")
}

/// The process's children, for summing a tab's CPU and memory.
pub fn children_of(pid: u32) -> Vec<u32> {
    let _ = pid;
    todo!("procinfo::children_of")
}
