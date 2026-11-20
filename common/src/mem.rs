//! The memory lists (free / standby) and purging the standby list, as ISLC does: when a game needs memory and the free list is
//! empty, Windows has to repurpose standby pages one by one, which shows as stutter. Needs administrator rights.
//! Not done on purpose: emptying working sets. Trimmed pages land on the standby list, which already counts as available
//! memory, and the trimmed program faults them back in: slower, not faster.
use std::{ffi::c_void, path::PathBuf, thread, time::Duration};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, LUID},
    Security::{AdjustTokenPrivileges, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES},
    System::{
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
    fn NtSetSystemInformation(class: u32, info: *const c_void, len: u32) -> i32;
}

const SYSTEM_MEMORY_LIST_INFORMATION: u32 = 80;
const MEMORY_PURGE_STANDBY_LIST: u32 = 4;
const PAGE: u64 = 4096;

#[repr(C)]
#[derive(Default)]
struct MemoryLists {
    zero: usize,
    free: usize,
    modified: usize,
    modified_no_write: usize,
    bad: usize,
    standby_by_priority: [usize; 8],
    repurposed_by_priority: [usize; 8],
    modified_pagefile: usize,
}

/// Free and standby memory in bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lists {
    pub free: u64,
    pub standby: u64,
}

fn enable_privilege(name: &str) -> bool {
    unsafe {
        let mut tok: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut tok) == 0 {
            return false;
        }
        let w: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let mut luid = LUID { LowPart: 0, HighPart: 0 };
        let mut ok = LookupPrivilegeValueW(std::ptr::null(), w.as_ptr(), &mut luid) != 0;
        if ok {
            let tp = TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [windows_sys::Win32::Security::LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }] };
            // succeeds without the privilege too; GetLastError would say ERROR_NOT_ALL_ASSIGNED
            ok = AdjustTokenPrivileges(tok, 0, &tp, 0, std::ptr::null_mut(), std::ptr::null_mut()) != 0 && windows_sys::Win32::Foundation::GetLastError() == 0;
        }
        CloseHandle(tok);
        ok
    }
}

/// None without administrator rights (the query needs SeProfileSingleProcessPrivilege).
pub fn lists() -> Option<Lists> {
    if !enable_privilege("SeProfileSingleProcessPrivilege") {
        return None;
    }
    let mut m = MemoryLists::default();
    let st = unsafe { NtQuerySystemInformation(SYSTEM_MEMORY_LIST_INFORMATION, &mut m as *mut _ as *mut c_void, size_of::<MemoryLists>() as u32, std::ptr::null_mut()) };
    (st >= 0).then(|| Lists { free: (m.zero + m.free) as u64 * PAGE, standby: m.standby_by_priority.iter().sum::<usize>() as u64 * PAGE })
}

/// Drops the standby list (cached file data nobody holds). Err(NTSTATUS) without administrator rights.
pub fn purge_standby() -> Result<(), i32> {
    if !enable_privilege("SeProfileSingleProcessPrivilege") {
        return Err(0xC000_0061u32 as i32); // STATUS_PRIVILEGE_NOT_HELD
    }
    let cmd = MEMORY_PURGE_STANDBY_LIST;
    let st = unsafe { NtSetSystemInformation(SYSTEM_MEMORY_LIST_INFORMATION, &cmd as *const u32 as *const c_void, 4) };
    if st >= 0 { Ok(()) } else { Err(st) }
}

pub fn total_ram() -> u64 {
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    unsafe { GlobalMemoryStatusEx(&mut m) };
    m.ullTotalPhys
}

/// When the auto purge acts: the free list below max(512 MB, 5% of RAM) while the standby list holds over 1 GB.
pub fn should_purge(l: Lists, total: u64) -> bool {
    l.free < (512 << 20).max(total / 20) && l.standby > 1 << 30
}

/// The switch file of the auto purge (rtaskmgr's Speed page writes it, topbar's thread acts on it).
pub fn auto_file() -> Option<PathBuf> {
    std::env::var_os("ProgramData").map(|p| PathBuf::from(p).join(r"WinRust\autopurge.txt"))
}

pub fn auto_on() -> bool {
    auto_file().is_some_and(|f| f.is_file())
}

/// Endless loop for a resident elevated program (topbar): every 10 s, while the switch file exists, purges when `should_purge`,
/// at most once a minute.
pub fn auto_loop() {
    let total = total_ram();
    let mut last: Option<std::time::Instant> = None;
    loop {
        thread::sleep(Duration::from_secs(10));
        if !auto_on() || last.is_some_and(|t| t.elapsed() < Duration::from_secs(60)) {
            continue;
        }
        if lists().is_some_and(|l| should_purge(l, total)) && purge_standby().is_ok() {
            last = Some(std::time::Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purge_rule() {
        let gb = 1u64 << 30;
        assert!(should_purge(Lists { free: 100 << 20, standby: 4 * gb }, 16 * gb));
        assert!(!should_purge(Lists { free: 2 * gb, standby: 4 * gb }, 16 * gb), "enough free memory");
        assert!(!should_purge(Lists { free: 100 << 20, standby: 500 << 20 }, 16 * gb), "nothing worth dropping");
        assert!(should_purge(Lists { free: 1 * gb, standby: 8 * gb }, 64 * gb), "5% of 64 GB is 3.2 GB");
    }

    #[test]
    fn struct_matches_the_kernel_layout() {
        assert_eq!(size_of::<MemoryLists>(), 22 * size_of::<usize>());
        assert!(total_ram() > 0);
    }
}
