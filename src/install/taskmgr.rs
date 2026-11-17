//! rtaskmgr-win32: replaces Task Manager through an IFEO Debugger; ships rs-du (disk cleaner) next to it.
use crate::*;

pub(crate) const TM_KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe";

pub(crate) fn tm_dir() -> PathBuf {
    program_files().join("rtaskmgr")
}

// Image File Execution Options "Debugger": Windows starts our exe instead of taskmgr.exe (the system file stays untouched, so this is fully reversible).
pub(crate) fn tm_install() -> Result<(), String> {
    with_payload(&["Task manager.exe", "rs-du.exe"], |d| {
        let dst = tm_dir().join("rtaskmgr-win32.exe");
        let _ = run("taskkill", &["/f", "/im", "rtaskmgr-win32.exe"]); // an update: a running exe cannot be overwritten
        fs::create_dir_all(tm_dir()).map_err(|e| e.to_string())?;
        put(&d.join("Task manager.exe"), &dst)?;
        put(&d.join("rs-du.exe"), &tm_dir().join("rs-du.exe"))?;
        // the search (and the bar's Start panel) lists Start Menu shortcuts only, so without one the task manager is invisible to them
        soft(t!("ярлык в меню Пуск не создан, поиск не увидит диспетчер", "Start menu shortcut was not created, search will not find the task manager"), run("powershell", &["-NoProfile", "-Command", &format!("$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}'); $s.TargetPath='{}'; $s.Save()", tm_lnk().display(), dst.display())]));
        // elevated start without a UAC prompt (the exe re-launches itself through this task), so it can end protected processes
        let task = format!("$u=[Security.Principal.WindowsIdentity]::GetCurrent().Name; Register-ScheduledTask -TaskName rtaskmgr -Force -Action (New-ScheduledTaskAction -Execute '{}') -Principal (New-ScheduledTaskPrincipal -UserId $u -LogonType Interactive -RunLevel Highest) -Settings (New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances Parallel) | Out-Null", dst.display());
        soft(t!("задача rtaskmgr не создана, запуск от администратора будет через UAC", "rtaskmgr task was not created, launching as administrator will go through UAC"), run("powershell", &["-NoProfile", "-Command", &task]));
        run("reg", &["add", TM_KEY, "/v", "Debugger", "/t", "REG_SZ", "/d", &format!("\"{}\"", dst.display()), "/f"])
    })
}

pub(crate) fn tm_lnk() -> PathBuf {
    PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into())).join(r"Microsoft\Windows\Start Menu\Programs\Task manager.lnk")
}

pub(crate) fn tm_uninstall() -> Result<(), String> {
    let _ = fs::remove_file(tm_lnk());
    let _ = run("schtasks", &["/delete", "/tn", "rtaskmgr", "/f"]);
    // registry first: with the exe gone, taskmgr.exe would not start at all. Only our value: the key may hold
    // other IFEO settings (e.g. mitigation policies) that are not ours to remove.
    run("reg", &["delete", TM_KEY, "/v", "Debugger", "/f"])?;
    let _ = run("taskkill", &["/f", "/im", "rtaskmgr-win32.exe"]);
    let _ = fs::remove_file(tm_dir().join("rtaskmgr-win32.exe"));
    cleaner_release(bar_dir().join("topbar.exe").is_file());
    Ok(())
}

/// rs-du (disk cleaner) lives in the task manager folder and is shared by rtaskmgr-win32 and the topbar menu:
/// it goes away only when the other plugin is gone too.
pub(crate) fn cleaner_release(other_installed: bool) {
    if !other_installed {
        let _ = fs::remove_file(tm_dir().join("rs-du.exe"));
    }
    let _ = fs::remove_dir(tm_dir()); // only succeeds when empty
}
