//! Safety net for topbar: if the bar process is gone while the real taskbar is still hidden (crash, kill, bad boot),
//! the user would have no taskbar at all. reshell is already running from the Run key, so it watches and calls
//! `topbar.exe --restore`. shortcut: it restores the taskbar but does not restart the bar (a bar that crashes on
//! start would loop forever); upgrade if auto-restart is wanted.
use std::{os::windows::process::CommandExt, process::Command, thread, time::Duration};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, IsWindowVisible};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Broken = no bar, and a taskbar that exists but is hidden.
fn broken(bar_up: bool, tray_present: bool, tray_visible: bool) -> bool {
    !bar_up && tray_present && !tray_visible
}

pub fn run() {
    let mut strikes = 0;
    loop {
        thread::sleep(Duration::from_secs(5));
        let (bar, tray) = unsafe { (FindWindowW(wide("topbar").as_ptr(), std::ptr::null()), FindWindowW(wide("Shell_TrayWnd").as_ptr(), std::ptr::null())) };
        let b = broken(!bar.is_null(), !tray.is_null(), !tray.is_null() && unsafe { IsWindowVisible(tray) } != 0);
        strikes = if b { strikes + 1 } else { 0 };
        if strikes == 2 {
            // two checks in a row (~10 s): a bar that is only just starting must not be "restored" under its feet
            if let Ok(exe) = std::env::current_exe() {
                let _ = Command::new(exe.with_file_name("topbar.exe")).arg("--restore").creation_flags(0x0800_0000).status();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::broken;
    #[test]
    fn only_bar_gone_and_taskbar_hidden_is_broken() {
        assert!(broken(false, true, false));
        assert!(!broken(true, true, false)); // bar running: taskbar is meant to be hidden
        assert!(!broken(false, true, true)); // taskbar already back
        assert!(!broken(false, false, false)); // explorer restarting
    }
}
