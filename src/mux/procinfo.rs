//! A process's working directory, command name and process group's members,
//! and every process's CPU time and memory, read from `/proc` on Linux and
//! from the kernel's process info (libc's `proc_pidinfo`, `proc_listpids`,
//! `sysctl`) on macOS, without starting a process: the server asks for every pane on every focus change, and the
//! tmux version's `ps` and `lsof` calls there were what made it sluggish
//! under a security agent that inspects every process started.
//!
//! What tmux gave as `pane_current_path` and `pane_current_command` came from
//! the same places (its `osdep-linux.c` and `osdep-darwin.c`), so a pane's
//! directory and program read as they did. A process that is gone, or one
//! we may not look at, gives `None` or nothing, never an error: the caller
//! falls back to the pane's own program.

use std::path::PathBuf;

/// The process's working directory.
pub fn cwd_of(pid: u32) -> Option<PathBuf> {
    if pid == 0 {
        return None;
    }
    imp::cwd_of(pid)
}

/// The process's command name (`zsh`, `claude`, `2.1.3`, `node`), as `ps -o
/// comm` shows it without its path.
pub fn command_of(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    imp::command_of(pid).filter(|name| !name.is_empty())
}

/// The process's arguments, its program first.
pub fn args_of(pid: u32) -> Option<Vec<String>> {
    if pid == 0 {
        return None;
    }
    imp::args_of(pid).filter(|args| !args.is_empty())
}

/// Every process in a process group.
pub fn group_members(pgid: u32) -> Vec<u32> {
    // Group 0 is no group: on Linux it is where the kernel's own threads
    // sit, which no pane's program is among.
    if pgid == 0 {
        return Vec::new();
    }
    let mut pids = imp::group_members(pgid);
    pids.sort_unstable();
    pids
}

/// The process's children, for summing a tab's CPU and memory.
pub fn children_of(pid: u32) -> Vec<u32> {
    if pid == 0 {
        return Vec::new();
    }
    let mut pids = imp::children_of(pid);
    pids.sort_unstable();
    pids
}

/// The process group a process is in.
pub fn group_of(pid: u32) -> Option<u32> {
    if pid == 0 {
        return None;
    }
    imp::group_of(pid).filter(|&g| g > 0)
}

/// One process as the Resources view counts it: its parent, its name, the
/// CPU time it has used so far and the memory it holds now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcUsage {
    pub pid: u32,
    pub ppid: u32,
    /// Its command's name, as `ps -o comm` shows it without its path.
    pub name: String,
    /// User and system time together, in nanoseconds since it started.
    pub cpu_ns: u64,
    /// Resident memory, in bytes: memory two processes share counts in both,
    /// as ps's `rss` did.
    pub memory: u64,
}

/// Every process we may look at, with its CPU time and memory: what the
/// sidebar daemon's `ps -A -o pid=,ppid=,rss=,%cpu=,comm=` gave, read from the
/// kernel instead. A few hundred small reads, so it runs off the server's
/// task. A process another user owns is listed with what can be read of it
/// (its parent, so the tree under a pane stays whole), and no time or memory.
pub fn all_usage() -> Vec<ProcUsage> {
    let mut list = imp::all_usage();
    list.sort_unstable_by_key(|p| p.pid);
    list
}

/// What `/proc/<pid>/stat` says of a process for the Resources view: its
/// name (between the first `(` and the last `)`), parent, and user and
/// system time in clock ticks (fields 14 and 15, counting from one).
#[cfg(any(test, target_os = "linux"))]
fn parse_stat_usage(stat: &str) -> Option<(String, u32, u64, u64)> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?.to_string();
    let fields: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    let ppid = fields.get(1)?.parse().ok()?;
    let utime = fields.get(11)?.parse().ok()?;
    let stime = fields.get(12)?.parse().ok()?;
    Some((name, ppid, utime, stime))
}

/// The resident pages in `/proc/<pid>/statm`, its second field.
#[cfg(any(test, target_os = "linux"))]
fn parse_statm_resident(statm: &str) -> Option<u64> {
    statm.split_whitespace().nth(1)?.parse().ok()
}

/// The parent and process group of a process, from the fields of
/// `/proc/<pid>/stat` after its command. The command is in parentheses and
/// may itself hold spaces and parentheses (a process can name itself
/// anything), so the fields are read after the last `)`, never by splitting
/// the whole line.
#[cfg(any(test, target_os = "linux"))]
fn parse_stat(stat: &str) -> Option<(u32, u32)> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut fields = rest.split_whitespace();
    let _state = fields.next()?;
    let ppid = fields.next()?.parse().ok()?;
    let pgrp = fields.next()?.parse().ok()?;
    Some((ppid, pgrp))
}

/// A deleted directory reads as its old path with " (deleted)" after it;
/// tmux cut that off too, so a tab named from it keeps its name.
#[cfg(any(test, target_os = "linux"))]
fn strip_deleted(path: PathBuf) -> PathBuf {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    const SUFFIX: &[u8] = b" (deleted)";
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() > SUFFIX.len() && bytes.ends_with(SUFFIX) {
        let mut bytes = path.into_os_string().into_vec();
        bytes.truncate(bytes.len() - SUFFIX.len());
        return PathBuf::from(std::ffi::OsString::from_vec(bytes));
    }
    path
}

/// The arguments in what `sysctl(KERN_PROCARGS2)` gives on macOS: the
/// argument count as a native int, the executable's path, NULs padding it
/// out, then the arguments one after another, each ending in a NUL, and the
/// environment after them, which is left alone.
#[cfg(any(test, target_os = "macos"))]
fn parse_procargs2(buf: &[u8]) -> Option<Vec<String>> {
    let count = buf.get(..4)?;
    let argc = i32::from_ne_bytes(count.try_into().ok()?);
    if argc <= 0 {
        return None;
    }
    let rest = &buf[4..];
    // Past the executable's path and the NULs after it.
    let path_end = rest.iter().position(|&b| b == 0)?;
    let start = rest[path_end..].iter().position(|&b| b != 0)? + path_end;
    let mut args = Vec::new();
    let mut fields = rest[start..].split(|&b| b == 0);
    for _ in 0..argc {
        match fields.next() {
            Some(arg) => args.push(String::from_utf8_lossy(arg).into_owned()),
            None => break,
        }
    }
    Some(args)
}

#[cfg(target_os = "linux")]
mod imp {
    use std::fs;
    use std::path::PathBuf;

    pub fn cwd_of(pid: u32) -> Option<PathBuf> {
        fs::read_link(format!("/proc/{pid}/cwd"))
            .ok()
            .map(super::strip_deleted)
    }

    /// `comm` is the name the kernel keeps, the executable's own (cut to
    /// fifteen bytes) unless the program renamed itself: Claude Code's
    /// executable is named after its version, so it reads `2.1.3` there.
    pub fn command_of(pid: u32) -> Option<String> {
        let comm = fs::read(format!("/proc/{pid}/comm")).ok()?;
        let comm = String::from_utf8_lossy(&comm);
        Some(comm.trim_end_matches('\n').to_string())
    }

    pub fn args_of(pid: u32) -> Option<Vec<String>> {
        let line = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let line = line.strip_suffix(b"\0").unwrap_or(&line);
        if line.is_empty() {
            return None;
        }
        Some(
            line.split(|&b| b == 0)
                .map(|arg| String::from_utf8_lossy(arg).into_owned())
                .collect(),
        )
    }

    pub fn group_members(pgid: u32) -> Vec<u32> {
        processes()
            .filter(|&(_, _, pgrp)| pgrp == pgid)
            .map(|(pid, _, _)| pid)
            .collect()
    }

    pub fn children_of(pid: u32) -> Vec<u32> {
        processes()
            .filter(|&(_, ppid, _)| ppid == pid)
            .map(|(child, _, _)| child)
            .collect()
    }

    pub fn group_of(pid: u32) -> Option<u32> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        super::parse_stat(&stat).map(|(_, pgrp)| pgrp)
    }

    pub fn all_usage() -> Vec<super::ProcUsage> {
        // SAFETY: sysconf only reads the system's configuration.
        let tick = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        // SAFETY: as above.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let ns_per_tick = if tick > 0 {
            1_000_000_000 / tick as u64
        } else {
            10_000_000
        };
        let page = if page > 0 { page as u64 } else { 4096 };
        pids()
            .filter_map(|pid| {
                let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
                let (name, ppid, utime, stime) = super::parse_stat_usage(&stat)?;
                let resident = fs::read_to_string(format!("/proc/{pid}/statm"))
                    .ok()
                    .and_then(|m| super::parse_statm_resident(&m))
                    .unwrap_or(0);
                Some(super::ProcUsage {
                    pid,
                    ppid,
                    name,
                    cpu_ns: (utime + stime) * ns_per_tick,
                    memory: resident * page,
                })
            })
            .collect()
    }

    fn pids() -> impl Iterator<Item = u32> {
        fs::read_dir("/proc")
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
    }

    /// Every process as (pid, parent, process group). Linux has no list of a
    /// group's members, so this reads each process's `stat`, a few hundred
    /// small reads, cheaper than the one process `ps` would have been. A
    /// process that ends while it is read is left out.
    fn processes() -> impl Iterator<Item = (u32, u32, u32)> {
        pids().filter_map(|pid| {
            let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            let (ppid, pgrp) = super::parse_stat(&stat)?;
            Some((pid, ppid, pgrp))
        })
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{CStr, OsStr, c_int, c_void};
    use std::mem::{size_of, size_of_val};
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::ptr;

    /// `proc_listpids`'s filters, from `sys/proc_info.h` (the libc crate
    /// doesn't carry them).
    const PROC_PGRP_ONLY: u32 = 2;
    const PROC_PPID_ONLY: u32 = 6;

    fn c_pid(pid: u32) -> Option<c_int> {
        c_int::try_from(pid).ok()
    }

    pub fn cwd_of(pid: u32) -> Option<PathBuf> {
        let pid = c_pid(pid)?;
        // SAFETY: a plain C struct, for which all zeroes is a valid value.
        let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
        let size = size_of::<libc::proc_vnodepathinfo>() as c_int;
        // SAFETY: the buffer is the struct the flavor fills, of the size given.
        let filled = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                (&raw mut info).cast::<c_void>(),
                size,
            )
        };
        if filled != size {
            return None;
        }
        let path = &info.pvi_cdir.vip_path;
        // SAFETY: `vip_path` is MAXPATHLEN chars laid out one after another
        // (the libc crate splits the array in two only for old compilers).
        let bytes =
            unsafe { std::slice::from_raw_parts(path.as_ptr().cast::<u8>(), size_of_val(path)) };
        let path = CStr::from_bytes_until_nul(bytes).ok()?;
        (!path.is_empty()).then(|| PathBuf::from(OsStr::from_bytes(path.to_bytes())))
    }

    /// The kernel's name for the process (`p_name`, else `p_comm`), which is
    /// the executable's name, as `ps -o comm` shows it without its path;
    /// else the executable's path's last part.
    pub fn command_of(pid: u32) -> Option<String> {
        let pid = c_pid(pid)?;
        let mut name = [0u8; 256];
        // SAFETY: the buffer is as long as the length given.
        let len = unsafe { libc::proc_name(pid, name.as_mut_ptr().cast(), name.len() as u32) };
        if len > 0 {
            let name = &name[..(len as usize).min(name.len())];
            return Some(String::from_utf8_lossy(name).into_owned());
        }
        let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: the buffer is as long as the length given.
        let len = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
        if len <= 0 {
            return None;
        }
        path.truncate(len as usize);
        let path = Path::new(OsStr::from_bytes(&path));
        Some(path.file_name()?.to_string_lossy().into_owned())
    }

    pub fn args_of(pid: u32) -> Option<Vec<String>> {
        let pid = c_pid(pid)?;
        let mut argmax: c_int = 0;
        let mut size = size_of::<c_int>();
        let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
        // SAFETY: the out-pointer is an int and the size says so.
        let ok = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as u32,
                (&raw mut argmax).cast::<c_void>(),
                &mut size,
                ptr::null_mut(),
                0,
            )
        };
        if ok != 0 || argmax <= 0 {
            return None;
        }
        let mut buf = vec![0u8; argmax as usize];
        let mut size = buf.len();
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        // SAFETY: the buffer is as long as the size given, which the kernel
        // sets to what it wrote.
        let ok = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as u32,
                buf.as_mut_ptr().cast::<c_void>(),
                &mut size,
                ptr::null_mut(),
                0,
            )
        };
        if ok != 0 {
            return None;
        }
        buf.truncate(size);
        super::parse_procargs2(&buf)
    }

    pub fn group_members(pgid: u32) -> Vec<u32> {
        list_pids(PROC_PGRP_ONLY, pgid)
    }

    pub fn children_of(pid: u32) -> Vec<u32> {
        list_pids(PROC_PPID_ONLY, pid)
    }

    /// The kernel's BSD info on a process: its parent, group and name.
    fn bsd_info(pid: u32) -> Option<libc::proc_bsdinfo> {
        let pid = c_pid(pid)?;
        // SAFETY: a plain C struct, for which all zeroes is a valid value.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = size_of::<libc::proc_bsdinfo>() as c_int;
        // SAFETY: the buffer is the struct the flavor fills, of the size given.
        let filled = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&raw mut info).cast::<c_void>(),
                size,
            )
        };
        (filled == size).then_some(info)
    }

    pub fn group_of(pid: u32) -> Option<u32> {
        bsd_info(pid).map(|info| info.pbi_pgid)
    }

    /// The task's resident memory and CPU time, which the kernel gives only
    /// for our own processes.
    fn task_info(pid: u32) -> Option<libc::proc_taskinfo> {
        let pid = c_pid(pid)?;
        // SAFETY: a plain C struct, for which all zeroes is a valid value.
        let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
        let size = size_of::<libc::proc_taskinfo>() as c_int;
        // SAFETY: the buffer is the struct the flavor fills, of the size given.
        let filled = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTASKINFO,
                0,
                (&raw mut info).cast::<c_void>(),
                size,
            )
        };
        (filled == size).then_some(info)
    }

    /// The task's times are in Mach's absolute time units, which are
    /// nanoseconds on an Intel Mac and 125/3 of one on Apple silicon; the
    /// timebase says which. (The libc crate marks the call deprecated in
    /// favor of the mach2 crate, for this one number.)
    #[allow(deprecated)]
    fn timebase() -> (u64, u64) {
        let mut info = libc::mach_timebase_info { numer: 0, denom: 0 };
        // SAFETY: the out-pointer is the struct the call fills.
        let ok = unsafe { libc::mach_timebase_info(&raw mut info) };
        if ok != 0 || info.numer == 0 || info.denom == 0 {
            return (1, 1);
        }
        (info.numer as u64, info.denom as u64)
    }

    /// A C string in a fixed array, up to its first NUL.
    fn c_text(chars: &[std::ffi::c_char]) -> String {
        // SAFETY: c_char and u8 have the same size and layout.
        let bytes = unsafe { std::slice::from_raw_parts(chars.as_ptr().cast::<u8>(), chars.len()) };
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    }

    pub fn all_usage() -> Vec<super::ProcUsage> {
        let (numer, denom) = timebase();
        all_pids()
            .into_iter()
            .filter_map(|pid| {
                let bsd = bsd_info(pid)?;
                let name = match c_text(&bsd.pbi_name) {
                    n if n.is_empty() => c_text(&bsd.pbi_comm),
                    n => n,
                };
                let task = task_info(pid);
                let ticks = task.map_or(0, |t| t.pti_total_user + t.pti_total_system);
                Some(super::ProcUsage {
                    pid,
                    ppid: bsd.pbi_ppid,
                    name,
                    cpu_ns: (ticks as u128 * numer as u128 / denom as u128) as u64,
                    memory: task.map_or(0, |t| t.pti_resident_size),
                })
            })
            .collect()
    }

    /// Every pid: asked once with no buffer for how many, then filled, with
    /// room for a few more started between the two calls.
    fn all_pids() -> Vec<u32> {
        // SAFETY: a null buffer asks only for the count.
        let need = unsafe { libc::proc_listallpids(ptr::null_mut(), 0) };
        if need <= 0 {
            return Vec::new();
        }
        let room = need as usize + 64;
        let mut pids: Vec<c_int> = vec![0; room];
        // SAFETY: the buffer holds `room` ints, the size given in bytes.
        let filled = unsafe {
            libc::proc_listallpids(
                pids.as_mut_ptr().cast::<c_void>(),
                (room * size_of::<c_int>()) as c_int,
            )
        };
        if filled <= 0 {
            return Vec::new();
        }
        pids.truncate((filled as usize).min(room));
        pids.into_iter()
            .filter(|&pid| pid > 0)
            .map(|pid| pid as u32)
            .collect()
    }

    /// The kernel's list of pids that match a filter: asked once with no
    /// buffer for how much room it needs, then filled. Room for a few more
    /// is added, since a process can start between the two calls.
    fn list_pids(filter: u32, arg: u32) -> Vec<u32> {
        // SAFETY: a null buffer asks only for the size.
        let need = unsafe { libc::proc_listpids(filter, arg, ptr::null_mut(), 0) };
        if need <= 0 {
            return Vec::new();
        }
        let room = need as usize / size_of::<c_int>() + 16;
        let mut pids: Vec<c_int> = vec![0; room];
        // SAFETY: the buffer holds `room` ints, the size given in bytes.
        let filled = unsafe {
            libc::proc_listpids(
                filter,
                arg,
                pids.as_mut_ptr().cast::<c_void>(),
                (room * size_of::<c_int>()) as c_int,
            )
        };
        if filled <= 0 {
            return Vec::new();
        }
        pids.truncate(filled as usize / size_of::<c_int>());
        pids.into_iter()
            .filter(|&pid| pid > 0)
            .map(|pid| pid as u32)
            .collect()
    }
}

/// Elsewhere there is nothing to read; the pane's own program stands in.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod imp {
    use std::path::PathBuf;

    pub fn cwd_of(_pid: u32) -> Option<PathBuf> {
        None
    }
    pub fn command_of(_pid: u32) -> Option<String> {
        None
    }
    pub fn args_of(_pid: u32) -> Option<Vec<String>> {
        None
    }
    pub fn group_members(_pgid: u32) -> Vec<u32> {
        Vec::new()
    }
    pub fn children_of(_pid: u32) -> Vec<u32> {
        Vec::new()
    }
    pub fn group_of(_pid: u32) -> Option<u32> {
        None
    }
    pub fn all_usage() -> Vec<super::ProcUsage> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_is_read_after_the_last_parenthesis() {
        let stat = "4242 (a (b) c) d) S 17 4240 17 34816 4240 4194304 0 0";
        assert_eq!(parse_stat(stat), Some((17, 4240)));
        assert_eq!(parse_stat("1 (init) S 0 1 1"), Some((0, 1)));
        assert_eq!(parse_stat("garbage"), None);
        assert_eq!(parse_stat("1 (x) S"), None);
    }

    #[test]
    fn usage_is_read_from_stat_and_statm() {
        let stat = "4242 (my (odd) name) S 17 4240 17 34816 4240 4194304 120 0 3 0 250 50 0 0 20 0";
        assert_eq!(
            parse_stat_usage(stat),
            Some(("my (odd) name".to_string(), 17, 250, 50))
        );
        assert_eq!(parse_stat_usage("1 (x) S 0"), None);
        assert_eq!(parse_statm_resident("1000 256 30 1 0 90 0\n"), Some(256));
        assert_eq!(parse_statm_resident("1000"), None);
    }

    #[test]
    fn a_deleted_directory_reads_as_its_old_path() {
        assert_eq!(
            strip_deleted(PathBuf::from("/tmp/gone (deleted)")),
            PathBuf::from("/tmp/gone")
        );
        assert_eq!(
            strip_deleted(PathBuf::from("/tmp/here")),
            PathBuf::from("/tmp/here")
        );
        assert_eq!(
            strip_deleted(PathBuf::from(" (deleted)")),
            PathBuf::from(" (deleted)")
        );
    }

    #[test]
    fn procargs2_gives_the_arguments_not_the_path_or_environment() {
        let mut buf = 3i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/usr/local/bin/node\0\0\0\0");
        buf.extend_from_slice(b"node\0/opt/claude/cli.js\0--resume\0HOME=/Users/x\0PATH=/bin\0");
        assert_eq!(
            parse_procargs2(&buf),
            Some(vec![
                "node".to_string(),
                "/opt/claude/cli.js".to_string(),
                "--resume".to_string()
            ])
        );
        assert_eq!(parse_procargs2(&[1, 0]), None);
        assert_eq!(parse_procargs2(&0i32.to_ne_bytes()), None);
        let mut short = 2i32.to_ne_bytes().to_vec();
        short.extend_from_slice(b"/bin/sh\0sh\0");
        assert_eq!(
            parse_procargs2(&short),
            Some(vec!["sh".to_string(), String::new()])
        );
    }

    #[test]
    fn nothing_for_pid_zero_or_a_process_that_is_not_there() {
        assert_eq!(cwd_of(0), None);
        assert_eq!(command_of(0), None);
        assert_eq!(args_of(0), None);
        assert!(group_members(0).is_empty());
        assert!(children_of(0).is_empty());
        assert_eq!(group_of(0), None);
        assert_eq!(group_of(u32::MAX - 1), None);
        // Past any pid the kernel hands out (Linux's limit is 2^22).
        assert_eq!(cwd_of(u32::MAX - 1), None);
        assert_eq!(command_of(u32::MAX - 1), None);
        assert!(group_members(u32::MAX - 1).is_empty());
    }

    #[cfg(target_os = "linux")]
    mod live {
        use super::super::*;
        use std::os::unix::process::CommandExt;
        use std::process::{Child, Command, Stdio};
        use std::time::{Duration, Instant};

        fn scratch(name: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "sb-procinfo-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("scratch dir");
            dir.canonicalize().expect("canonical scratch dir")
        }

        /// Ends the process (and its group, when it leads one) however the
        /// test ends.
        struct Reaped(Child);
        impl Drop for Reaped {
            fn drop(&mut self) {
                // SAFETY: signals only the test's own process group.
                unsafe { libc::killpg(self.0.id() as i32, libc::SIGKILL) };
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        fn until<T>(mut check: impl FnMut() -> Option<T>) -> T {
            let start = Instant::now();
            loop {
                if let Some(found) = check() {
                    return found;
                }
                assert!(start.elapsed() < Duration::from_secs(5), "timed out");
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        #[test]
        fn a_sleeps_directory_command_and_arguments() {
            let dir = scratch("sleep");
            let child = Command::new("sleep")
                .arg("30")
                .current_dir(&dir)
                .stdin(Stdio::null())
                .spawn()
                .expect("sleep");
            let child = Reaped(child);
            let pid = child.0.id();
            assert_eq!(cwd_of(pid), Some(dir.clone()));
            // The name and arguments change only once `sleep` has been
            // exec'd (the arguments a moment after the name), so wait for it.
            assert_eq!(until(|| command_of(pid).filter(|c| c == "sleep")), "sleep");
            assert_eq!(
                until(|| args_of(pid).filter(|a| a[0] == "sleep")),
                vec!["sleep".to_string(), "30".to_string()]
            );
            drop(child);
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_deleted_directory_is_still_named() {
            let dir = scratch("deleted");
            let child = Command::new("sleep")
                .arg("30")
                .current_dir(&dir)
                .stdin(Stdio::null())
                .spawn()
                .expect("sleep");
            let child = Reaped(child);
            std::fs::remove_dir(&dir).expect("remove dir");
            assert_eq!(cwd_of(child.0.id()), Some(dir));
        }

        #[test]
        fn a_groups_members_and_a_parents_children() {
            let child = Command::new("/bin/sh")
                .args(["-c", "sleep 30 & sleep 30 & wait"])
                .process_group(0)
                .stdin(Stdio::null())
                .spawn()
                .expect("sh");
            let child = Reaped(child);
            let pid = child.0.id();
            let children = until(|| {
                let kids = children_of(pid);
                (kids.len() == 2).then_some(kids)
            });
            let members = until(|| {
                let members = group_members(pid);
                (members.len() == 3).then_some(members)
            });
            assert!(members.contains(&pid));
            for kid in &children {
                assert!(members.contains(kid), "{kid} in {members:?}");
                assert_eq!(until(|| command_of(*kid).filter(|c| c == "sleep")), "sleep");
            }
            // SAFETY: getpgrp only reads the test's own process group.
            let ours = unsafe { libc::getpgrp() } as u32;
            assert!(!group_members(ours).contains(&pid));
            assert_eq!(group_of(pid), Some(pid));
            assert_eq!(group_of(std::process::id()), Some(ours));
        }

        #[test]
        fn every_process_is_listed_with_its_parent_and_memory() {
            let list = all_usage();
            let me = list
                .iter()
                .find(|p| p.pid == std::process::id())
                .expect("this test's own process");
            assert!(me.memory > 0, "{me:?}");
            // SAFETY: getppid only reads the test's own parent.
            assert_eq!(me.ppid, unsafe { libc::getppid() } as u32);
        }
    }
}
