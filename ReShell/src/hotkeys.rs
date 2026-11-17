//! Replacement for desktop-hotkeys.ps1: Win+1/2/3 -> desktop 1/2/3, Win+` -> previous desktop, Win+E -> our file manager.
//! No undocumented COM: the current desktop comes from the registry, switching sends Ctrl+Win+Left/Right
//! (Win is already held by the user). shortcut: relies on the system shortcuts being enabled.
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering::Relaxed};
use windows_sys::Win32::{Foundation::*, System::{LibraryLoader::GetModuleHandleW, Registry::*}, UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*}};

static PREV: AtomicUsize = AtomicUsize::new(usize::MAX);
static HOOK: AtomicIsize = AtomicIsize::new(0);

/// (current index, desktop count) from the registry.
fn desktops() -> Option<(usize, usize)> {
    let key: Vec<u16> = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Explorer\\VirtualDesktops\0".encode_utf16().collect();
    let get = |name: &str| -> Option<Vec<u8>> {
        let n: Vec<u16> = format!("{name}\0").encode_utf16().collect();
        let mut buf = vec![0u8; 1024];
        let mut len = buf.len() as u32;
        let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), n.as_ptr(), RRF_RT_REG_BINARY, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len) };
        (r == 0).then(|| { buf.truncate(len as usize); buf })
    };
    index_in(&get("VirtualDesktopIDs")?, &get("CurrentVirtualDesktop")?)
}

fn index_in(ids: &[u8], cur: &[u8]) -> Option<(usize, usize)> {
    let i = ids.chunks_exact(16).position(|c| c == cur)?;
    Some((i, ids.len() / 16))
}

fn tap(vk: u16, up: bool) -> INPUT {
    let mut i: INPUT = unsafe { std::mem::zeroed() };
    i.r#type = INPUT_KEYBOARD;
    i.Anonymous.ki = KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, dwExtraInfo: 0 };
    i
}

fn send(keys: &[INPUT]) {
    unsafe { SendInput(keys.len() as u32, keys.as_ptr(), size_of::<INPUT>() as i32) };
}

/// Win+E: rust-explorer from the user's own folder (where updates land without admin), else next to us, else Explorer.
/// Null std handles: this process has no console (FreeConsole) and a child that inherits the invalid ones fails to start.
fn open_files() {
    use std::{path::PathBuf, process::{Command, Stdio}};
    let mine = std::env::var_os("LOCALAPPDATA").map(|b| PathBuf::from(b).join(r"plugin-launcher\bin\rust-explorer.exe"));
    let beside = std::env::current_exe().ok().map(|e| e.with_file_name("rust-explorer.exe"));
    let exe = [mine, beside].into_iter().flatten().find(|p| p.exists()).unwrap_or_else(|| "explorer.exe".into());
    let _ = Command::new(exe).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

fn go(target: usize) {
    let Some((cur, count)) = desktops() else { return };
    if target == cur || target >= count { return }
    PREV.store(cur, Relaxed);
    let key = if target > cur { VK_RIGHT } else { VK_LEFT };
    for _ in 0..cur.abs_diff(target) {
        send(&[tap(VK_CONTROL, false), tap(key, false), tap(key, true), tap(VK_CONTROL, true)]);
    }
}

unsafe extern "system" fn hook(n: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if n >= 0 {
        let k = unsafe { &*(l as *const KBDLLHOOKSTRUCT) };
        let win = unsafe { (GetAsyncKeyState(VK_LWIN as i32) | GetAsyncKeyState(VK_RWIN as i32)) as u16 & 0x8000 != 0 };
        if k.vkCode == 0x45 && win && k.flags & LLKHF_INJECTED == 0 {
            // Win+E: swallow it so Explorer's own handler does not open a window as well
            if w == WM_KEYDOWN as usize {
                send(&[tap(0xE8, false), tap(0xE8, true)]); // Win release must not open Start
                std::thread::spawn(open_files);
            }
            return 1;
        }
        let id = match k.vkCode { 0x31..=0x33 => Some(k.vkCode as usize - 0x31), 0xC0 => Some(usize::MAX), _ => None };
        if let (Some(id), true, false) = (id, win, k.flags & LLKHF_INJECTED != 0) {
            if w == WM_KEYDOWN as usize {
                // unassigned key tap stops Win release from opening Start; switching runs off the hook thread
                send(&[tap(0xE8, false), tap(0xE8, true)]);
                std::thread::spawn(move || go(if id == usize::MAX { PREV.load(Relaxed) } else { id }));
            }
            return 1;
        }
    }
    unsafe { CallNextHookEx(HOOK.load(Relaxed) as _, n, w, l) }
}

pub fn run() -> i32 {
    let h = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), GetModuleHandleW(std::ptr::null()), 0) };
    if h.is_null() { eprintln!("hook failed"); return 1 }
    HOOK.store(h as isize, Relaxed);
    let mut m: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut m, std::ptr::null_mut(), 0, 0) } > 0 {}
    0
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_index() {
        let ids = [[1u8; 16], [2; 16], [3; 16]].concat();
        assert_eq!(super::index_in(&ids, &[2; 16]), Some((1, 3)));
        assert_eq!(super::index_in(&ids, &[9; 16]), None);
    }
}
