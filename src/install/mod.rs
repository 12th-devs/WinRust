//! Install logic, shared by the window and the command line so both do exactly the same thing:
//! process/registry helpers, the embedded payload, and one file per plugin with its install/uninstall pair.
use crate::*;
mod explorer;
mod search;
mod taskmgr;
mod tools;
mod topbar;
pub(crate) use {explorer::*, search::*, taskmgr::*, tools::*, topbar::*};

pub(crate) fn out(prog: &str, args: &[&str]) -> Result<String, String> {
    let o = Command::new(prog).args(args).creation_flags(0x0800_0000).output().map_err(|e| format!("{prog}: {e}"))?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    } else {
        Err(format!("{prog} {}: {}{}", args.join(" "), String::from_utf8_lossy(&o.stdout).trim(), String::from_utf8_lossy(&o.stderr).trim()))
    }
}

pub(crate) fn run(prog: &str, args: &[&str]) -> Result<(), String> {
    out(prog, args).map(|_| ())
}

// Non-fatal failures of one install: shown as a count in the status line, spelled out in install.log.
pub(crate) static WARN: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub(crate) fn soft(what: &str, r: Result<(), String>) {
    if let Err(e) = r {
        WARN.lock().unwrap().push(format!("{what}: {e}"));
    }
}

pub(crate) fn log(s: &str) {
    let d = PathBuf::from(env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into())).join("plugin-launcher");
    let _ = fs::create_dir_all(&d);
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(d.join("install.log")) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = writeln!(f, "[{t}] {s}"); // shortcut: unix seconds, no date formatting; upgrade if the log gets read by humans a lot
    }
}

/// Copies over an exe that was just killed: taskkill returns before Windows (or an antivirus scan) releases the file, so retry ~6 s.
pub(crate) fn put(src: &Path, dst: &Path) -> Result<(), String> {
    let mut r = fs::copy(src, dst).map(|_| ());
    for _ in 0..30 {
        if !matches!(&r, Err(e) if e.raw_os_error() == Some(32) || e.raw_os_error() == Some(5)) {
            break;
        }
        thread::sleep(std::time::Duration::from_millis(200));
        r = fs::copy(src, dst).map(|_| ());
    }
    r.map_err(|e| format!("{}: {e}", dst.display()))
}

pub(crate) fn reg_dword(key: &str, name: &str) -> Option<u32> {
    let o = out("reg", &["query", key, "/v", name]).ok()?;
    let t = o.lines().find(|l| l.contains("REG_DWORD"))?.split_whitespace().last()?;
    u32::from_str_radix(t.trim_start_matches("0x"), 16).ok()
}

include!(concat!(env!("OUT_DIR"), "/payload.rs")); // PAYLOAD: the exes listed in payload.txt, embedded by build.rs

/// Update without rebuilding the launcher: an exe in `payload\` next to this one (or two levels up, the cargo
/// target\release layout) wins over the embedded copy. Trust level = the launcher exe's own folder.
pub(crate) fn disk_payload() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let d = exe.parent()?;
    [d.join("payload"), d.join(r"..\..\payload")].into_iter().find(|p| p.is_dir())
}

/// Unpacks the `need`ed bundled plugins, runs `f` on them, deletes them. The folder is under Program Files (admin-only writable),
/// not %TEMP%: a user-level process must not be able to swap an exe that is about to be installed as a SYSTEM task.
pub(crate) fn with_payload<T>(need: &[&str], f: impl FnOnce(&Path) -> Result<T, String>) -> Result<T, String> {
    let d = program_files().join(format!("plugin-launcher-tmp-{}", std::process::id()));
    fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    let disk = disk_payload();
    let unpack = |n: &&str| -> Result<(), String> {
        let bytes = PAYLOAD.iter().find(|(m, _)| m == n).ok_or(tf!("нет {n} в сборке", "{n} is missing from the build"))?.1;
        match disk.as_ref().map(|p| p.join(n)).filter(|p| p.is_file()) {
            Some(src) => {
                log(&tf!("{n}: из {}", "{n}: from {}", src.display()));
                fs::copy(src, d.join(n)).map(|_| ())
            }
            None => fs::write(d.join(n), bytes),
        }
        .map_err(|e| tf!("распаковка {n}: {e}", "unpacking {n}: {e}"))
    };
    let r = need.iter().try_for_each(unpack).and_then(|_| f(&d));
    let _ = fs::remove_dir_all(&d);
    r
}

pub(crate) fn program_files() -> PathBuf {
    PathBuf::from(env::var("ProgramFiles").unwrap_or(r"C:\Program Files".into()))
}

// shortcut: Windows makes at most one restore point per 24 h and it needs System Protection on; failures are ignored.
pub(crate) fn checkpoint() -> Result<(), String> {
    run("powershell", &["-NoProfile", "-Command", "Checkpoint-Computer -Description 'Plugin Launcher' -RestorePointType MODIFY_SETTINGS"])
}

// Shared by the window and the command line, so both do exactly the same thing.
pub(crate) fn install_plugin(i: usize) -> Result<String, String> {
    let p = &PLUGINS[i];
    let (inst, uninst) = p.actions.ok_or(t!("установка недоступна", "installation is not available"))?;
    let was = (p.installed)(); // already installed = an update: a failure must not remove the working copy
    WARN.lock().unwrap().clear();
    soft(t!("точка восстановления не создана", "restore point was not created"), checkpoint());
    if let Err(e) = inst() {
        log(&tf!("{}: ОШИБКА {e}", "{}: ERROR {e}", p.name));
        if was {
            return Err(e);
        }
        let back = uninst().err().map_or(String::new(), |u| tf!("; откат: {u}", "; rollback: {u}"));
        return Err(tf!("{e} (установка отменена, изменения возвращены{back})", "{e} (installation cancelled, changes reverted{back})"));
    }
    let w = WARN.lock().unwrap().clone();
    log(&format!("{}: {} {}", p.name, if was { t!("обновлён", "updated") } else { t!("установлен", "installed") }, w.join(" | ")));
    let note = if w.is_empty() { String::new() } else { tf!(" Предупреждений: {} (см. %ProgramData%\\plugin-launcher\\install.log).", " Warnings: {} (see %ProgramData%\\plugin-launcher\\install.log).", w.len()) };
    Ok(format!("{} {}.{note}", p.name, if was { t!("обновлён", "updated") } else { t!("установлен", "installed") }))
}

pub(crate) fn uninstall_plugin(i: usize) -> Result<String, String> {
    let p = &PLUGINS[i];
    let (_, uninst) = p.actions.ok_or(t!("удаление недоступно", "uninstall is not available"))?;
    uninst().inspect_err(|e| log(&tf!("{}: ОШИБКА удаления {e}", "{}: uninstall ERROR {e}", p.name)))?;
    log(&tf!("{}: удалён", "{}: removed", p.name));
    Ok(tf!("{} удалён, система восстановлена.", "{} removed, system restored.", p.name))
}

pub(crate) fn rollback() -> Result<String, String> {
    // reverse install order: rust-explorer (which switched Explorer off) goes before topbar, else the session is left with no panel and no shell
    let errs: Vec<String> = PLUGINS.iter().rev().filter(|p| (p.installed)()).filter_map(|p| p.actions.and_then(|a| a.1().err().map(|e| format!("{}: {e}", p.name)))).collect();
    if errs.is_empty() { Ok(t!("Все плагины удалены, система восстановлена.", "All plugins removed, system restored.").into()) } else { Err(errs.join("; ")) }
}
