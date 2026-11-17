// Plugin Launcher: a small dark Win32 window that installs / removes plugins (rust-search; chrome-rs is a stub).
// Binaries come from next to this exe, else ..\rust-search\target\release. Needs admin (see app.manifest).
#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]
use std::{
    cell::Cell,
    env, fs,
    io::Write,
    mem::{size_of, zeroed},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    ptr::{null, null_mut},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
    },
    thread,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{
        Dwm::DwmSetWindowAttribute,
        Gdi::*,
        GdiPlus::{
            FillModeAlternate, GdipAddPathArc, GdipClosePathFigure, GdipCreateFromHDC, GdipCreatePath, GdipCreateSolidFill, GdipDeleteBrush, GdipDeleteGraphics, GdipDeletePath, GdipFillPath,
            GdipSetSmoothingMode, GdiplusStartup, GdiplusStartupInput, GpBrush, GpGraphics, GpPath, GpSolidFill, SmoothingModeAntiAlias,
        },
    },
    System::{Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE}, LibraryLoader::GetModuleHandleW},
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

const BG: u32 = rgb(22, 19, 18);
const PANEL: u32 = rgb(36, 31, 29);
const BORDER: u32 = rgb(60, 51, 47);
const TEXT: u32 = rgb(255, 255, 255);
const DIM: u32 = rgb(166, 152, 145);
const ACCENT: u32 = rgb(222, 98, 42); // rust orange
const GOOD: u32 = rgb(110, 200, 120);
const BAD: u32 = rgb(255, 99, 97);
const WM_DONE: u32 = WM_APP + 1;
const WM_MOUSELEAVE: u32 = 0x02A3; // not exported by windows-sys under the enabled features

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

struct Plugin {
    name: &'static str,
    desc: &'static str,
    warn: &'static str, // shown in a confirmation box before install
    installed: fn() -> bool,
    actions: Option<(fn() -> Result<(), String>, fn() -> Result<(), String>)>, // (install, uninstall); None = stub
    settings: bool, // shows a gear button that opens the plugin's settings panel
    bulk: bool, // part of the one-click "install everything" (the window lists only these); false = command line only
    glyph: &'static str, // Segoe MDL2 Assets icon
}
const PLUGINS: &[Plugin] = &[
    Plugin {
        name: "rust-search",
        desc: "Быстрый поиск файлов по NTFS (Alt+Space). Заменяет поиск Windows: служба WSearch отключается, SearchHost блокируется.",
        warn: "Служба поиска Windows (WSearch) будет ОТКЛЮЧЕНА, SearchHost.exe заблокирован, строка поиска на панели задач скрыта: поиск в Пуске, Проводнике и Outlook перестанет работать.",
        installed: || prog_dir().join("rs.exe").is_file(),
        actions: Some((install, uninstall)),
        settings: false,
        bulk: true,
        glyph: "\u{E721}", // search
    },
    Plugin {
        name: "rtaskmgr-win32",
        desc: "Диспетчер задач на Rust. Заменяет стандартный: Ctrl+Shift+Esc и запуск taskmgr.exe открывают его.",
        warn: "Стандартный диспетчер задач будет ЗАМЕНЁН: любой запуск taskmgr.exe откроет rtaskmgr-win32.",
        installed: || run("reg", &["query", TM_KEY, "/v", "Debugger"]).is_ok(),
        actions: Some((tm_install, tm_uninstall)),
        settings: false,
        bulk: true,
        glyph: "\u{E9D9}", // diagnostic
    },
    Plugin {
        name: "topbar",
        desc: "Нижняя панель в стиле Windows 11 вместо панели задач: окна, часы, меню. Стандартная панель скрывается. Вместе с ней ставятся хоткеи рабочих столов (Win+1/2/3, Win+`).",
        warn: "Панель задач Windows будет СКРЫТА и заменена панелью внизу экрана. Удаление плагина возвращает её.",
        installed: || bar_dir().join("topbar.exe").is_file(),
        actions: Some((bar_install, bar_uninstall)),
        settings: true,
        bulk: true,
        glyph: "\u{E700}", // menu
    },
    Plugin {
        name: "rust-explorer",
        desc: "Файловый менеджер на Rust вместо Проводника: папки и диски открываются в нём (Win+E тоже), рабочий стол рисует он сам. Обычный explorer.exe как оболочка ОТКЛЮЧАЕТСЯ. Нужен topbar.",
        warn: "ОБЫЧНЫЙ Explorer будет ОТКЛЮЧЁН: после установки панель задач, рабочий стол и окна папок дают topbar и rust-explorer. Сначала установите topbar, без него не будет панели. Ещё для экономии RAM отключаются службы DiagTrack, PcaSvc, InventorySvc, lfsvc (геолокация), cbdhsvc (журнал буфера Win+V), DoSvc ставится вручную, службы svchost объединяются (полностью после перезагрузки); удаление плагина вернёт их. Если что-то пойдёт не так: Ctrl+Alt+Del, Диспетчер задач, Запустить новую задачу, explorer.exe; затем удалите плагин.",
        installed: || rx_dir().join("rust-explorer.exe").is_file(),
        actions: Some((rx_install, rx_uninstall)),
        settings: false,
        bulk: true,
        glyph: "\u{E8B7}", // folder
    },
    Plugin {
        name: "wiztree",
        desc: "Анализ диска: карта занятого места (treemap) по MFT за секунды. Ярлык «WinRust Disk» в меню Пуск, запускается от администратора.",
        warn: "Ничего системного не меняется: файл в Program Files и ярлык в меню Пуск.",
        installed: || wz_dir().join("wiztree.exe").is_file(),
        actions: Some((wz_install, wz_uninstall)),
        settings: false,
        bulk: true,
        glyph: "\u{EDA2}", // hard drive
    },
    Plugin {
        name: "photoview",
        desc: "Просмотр фото: открывается мгновенно, листает папку стрелками, учитывает поворот из EXIF. Появляется в «Открыть с помощью» (по умолчанию не назначается).",
        warn: "Программа добавляется в «Открыть с помощью» для jpg, png, bmp, gif, tiff, webp, heic, avif. Приложением по умолчанию её делаете вы сами в Параметрах.",
        installed: || pv_dir().join("photoview.exe").is_file(),
        actions: Some((pv_install, pv_uninstall)),
        settings: false,
        bulk: true,
        glyph: "\u{E91B}", // picture
    },
    Plugin {
        name: "debloat",
        desc: "Отключает мусор Windows: телеметрию, Copilot, Recall, рекламу в Пуске, фон Edge. Только обратимое, старые значения запоминаются.",
        warn: "Будут ОТКЛЮЧЕНЫ несколько служб (телеметрия, карты, отчёты об ошибках) и выставлены политики реестра (Copilot, Recall, реклама, фон Edge; Edge покажет «управляется организацией»). Старые значения сохраняются, uninstall debloat возвращает всё. Приложения не удаляются.",
        installed: || db_dir().join("debloat.exe").is_file(),
        actions: Some((db_install, db_uninstall)),
        settings: false,
        bulk: false, // changes system behaviour: only on request, never as part of "install everything"
        glyph: "\u{E74D}", // delete
    },
    Plugin { name: "chrome-rs", desc: "Браузер на Rust. Установка пока недоступна (заглушка).", warn: "", installed: || false, actions: None, settings: false, bulk: true, glyph: "\u{E774}" }, // globe
];

static BUSY: AtomicBool = AtomicBool::new(false);
static JOB: AtomicUsize = AtomicUsize::new(usize::MAX); // what BUSY is working on: plugin index, or PLUGINS.len() = rollback
static INSTALLED: [AtomicBool; PLUGINS.len()] = [const { AtomicBool::new(false) }; PLUGINS.len()];
fn refresh() {
    for (p, f) in PLUGINS.iter().zip(&INSTALLED) {
        f.store((p.installed)(), Relaxed);
    }
}
static MSG: Mutex<(String, bool)> = Mutex::new((String::new(), true));
thread_local! { static SETTINGS: Cell<Option<usize>> = const { Cell::new(None) }; } // plugin whose settings panel is open
thread_local! { static HOVER: Cell<Option<usize>> = const { Cell::new(None) }; }

// ---------- install logic ----------

fn out(prog: &str, args: &[&str]) -> Result<String, String> {
    let o = Command::new(prog).args(args).creation_flags(0x0800_0000).output().map_err(|e| format!("{prog}: {e}"))?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    } else {
        Err(format!("{prog} {}: {}{}", args.join(" "), String::from_utf8_lossy(&o.stdout).trim(), String::from_utf8_lossy(&o.stderr).trim()))
    }
}

fn run(prog: &str, args: &[&str]) -> Result<(), String> {
    out(prog, args).map(|_| ())
}

// Non-fatal failures of one install: shown as a count in the status line, spelled out in install.log.
static WARN: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn soft(what: &str, r: Result<(), String>) {
    if let Err(e) = r {
        WARN.lock().unwrap().push(format!("{what}: {e}"));
    }
}

fn log(s: &str) {
    let d = PathBuf::from(env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into())).join("plugin-launcher");
    let _ = fs::create_dir_all(&d);
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(d.join("install.log")) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = writeln!(f, "[{t}] {s}"); // shortcut: unix seconds, no date formatting; upgrade if the log gets read by humans a lot
    }
}

/// Copies over an exe that was just killed: taskkill returns before Windows (or an antivirus scan) releases the file, so retry ~6 s.
fn put(src: &Path, dst: &Path) -> Result<(), String> {
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

fn reg_dword(key: &str, name: &str) -> Option<u32> {
    let o = out("reg", &["query", key, "/v", name]).ok()?;
    let t = o.lines().find(|l| l.contains("REG_DWORD"))?.split_whitespace().last()?;
    u32::from_str_radix(t.trim_start_matches("0x"), 16).ok()
}

/// What the system looked like before the plugin touched it: HKLM\SOFTWARE\plugin-launcher\<plugin>. Written once
/// (an update must not overwrite the originals with our own values), read and deleted on uninstall.
const STATE: &str = r"HKLM\SOFTWARE\plugin-launcher\rust-search";
const WS_KEY: &str = r"HKLM\SYSTEM\CurrentControlSet\Services\WSearch";
const SB_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Search";

fn save_dword(name: &str, v: Option<u32>) {
    if let (Some(v), None) = (v, reg_dword(STATE, name)) {
        let _ = run("reg", &["add", STATE, "/v", name, "/t", "REG_DWORD", "/d", &v.to_string(), "/f"]);
    }
}

const SH_KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\SearchHost.exe";
const TM_KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe";

const PAYLOAD: [(&str, &[u8]); 11] = [
    ("wiztree.exe", include_bytes!("../payload/wiztree.exe")),
    ("photoview.exe", include_bytes!("../payload/photoview.exe")),
    ("debloat.exe", include_bytes!("../payload/debloat.exe")),
    ("rust-explorer.exe", include_bytes!("../payload/rust-explorer.exe")),
    ("rs-shell.exe", include_bytes!("../payload/rs-shell.exe")),
    ("reshell.exe", include_bytes!("../payload/reshell.exe")),
    ("topbar.exe", include_bytes!("../payload/topbar.exe")),
    ("rs.exe", include_bytes!("../payload/rs.exe")),
    ("rs-launcher.exe", include_bytes!("../payload/rs-launcher.exe")),
    ("Task manager.exe", include_bytes!("../payload/Task manager.exe")),
    ("rs-du.exe", include_bytes!("../payload/rs-du.exe")),
];

/// Update without rebuilding the launcher: an exe in `payload\` next to this one (or two levels up, the cargo
/// target\release layout) wins over the embedded copy. Trust level = the launcher exe's own folder.
fn disk_payload() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let d = exe.parent()?;
    [d.join("payload"), d.join(r"..\..\payload")].into_iter().find(|p| p.is_dir())
}

/// Unpacks the `need`ed bundled plugins, runs `f` on them, deletes them. The folder is under Program Files (admin-only writable),
/// not %TEMP%: a user-level process must not be able to swap an exe that is about to be installed as a SYSTEM task.
fn with_payload<T>(need: &[&str], f: impl FnOnce(&Path) -> Result<T, String>) -> Result<T, String> {
    let d = program_files().join(format!("plugin-launcher-tmp-{}", std::process::id()));
    fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    let disk = disk_payload();
    let unpack = |n: &&str| -> Result<(), String> {
        let bytes = PAYLOAD.iter().find(|(m, _)| m == n).ok_or(format!("нет {n} в сборке"))?.1;
        match disk.as_ref().map(|p| p.join(n)).filter(|p| p.is_file()) {
            Some(src) => {
                log(&format!("{n}: из {}", src.display()));
                fs::copy(src, d.join(n)).map(|_| ())
            }
            None => fs::write(d.join(n), bytes),
        }
        .map_err(|e| format!("распаковка {n}: {e}"))
    };
    let r = need.iter().try_for_each(unpack).and_then(|_| f(&d));
    let _ = fs::remove_dir_all(&d);
    r
}

fn program_files() -> PathBuf {
    PathBuf::from(env::var("ProgramFiles").unwrap_or(r"C:\Program Files".into()))
}

fn tm_dir() -> PathBuf {
    program_files().join("rtaskmgr")
}

fn bar_dir() -> PathBuf {
    program_files().join("topbar")
}

fn rx_dir() -> PathBuf {
    program_files().join("rust-explorer")
}

fn prog_dir() -> PathBuf {
    program_files().join("rs-search")
}

// shortcut: Windows makes at most one restore point per 24 h and it needs System Protection on; failures are ignored.
fn checkpoint() -> Result<(), String> {
    run("powershell", &["-NoProfile", "-Command", "Checkpoint-Computer -Description 'Plugin Launcher' -RestorePointType MODIFY_SETTINGS"])
}

// Image File Execution Options "Debugger": Windows starts our exe instead of taskmgr.exe (the system file stays untouched, so this is fully reversible).
fn tm_install() -> Result<(), String> {
    with_payload(&["Task manager.exe", "rs-du.exe"], |d| {
        let dst = tm_dir().join("rtaskmgr-win32.exe");
        let _ = run("taskkill", &["/f", "/im", "rtaskmgr-win32.exe"]); // an update: a running exe cannot be overwritten
        fs::create_dir_all(tm_dir()).map_err(|e| e.to_string())?;
        put(&d.join("Task manager.exe"), &dst)?;
        put(&d.join("rs-du.exe"), &tm_dir().join("rs-du.exe"))?;
        // the search (and the bar's Start panel) lists Start Menu shortcuts only, so without one the task manager is invisible to them
        soft("ярлык в меню Пуск не создан, поиск не увидит диспетчер", run("powershell", &["-NoProfile", "-Command", &format!("$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}'); $s.TargetPath='{}'; $s.Save()", tm_lnk().display(), dst.display())]));
        // elevated start without a UAC prompt (the exe re-launches itself through this task), so it can end protected processes
        let task = format!("$u=[Security.Principal.WindowsIdentity]::GetCurrent().Name; Register-ScheduledTask -TaskName rtaskmgr -Force -Action (New-ScheduledTaskAction -Execute '{}') -Principal (New-ScheduledTaskPrincipal -UserId $u -LogonType Interactive -RunLevel Highest) -Settings (New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances Parallel) | Out-Null", dst.display());
        soft("задача rtaskmgr не создана, запуск от администратора будет через UAC", run("powershell", &["-NoProfile", "-Command", &task]));
        run("reg", &["add", TM_KEY, "/v", "Debugger", "/t", "REG_SZ", "/d", &format!("\"{}\"", dst.display()), "/f"])
    })
}

fn tm_lnk() -> PathBuf {
    PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into())).join(r"Microsoft\Windows\Start Menu\Programs\Task manager.lnk")
}

fn tm_uninstall() -> Result<(), String> {
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
fn cleaner_release(other_installed: bool) {
    if !other_installed {
        let _ = fs::remove_file(tm_dir().join("rs-du.exe"));
    }
    let _ = fs::remove_dir(tm_dir()); // only succeeds when empty
}

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

// topbar.cfg next to topbar.exe (the bar re-reads it every few seconds). Missing file = OneDrive hidden.
fn cfg_path() -> PathBuf {
    bar_dir().join("topbar.cfg")
}

fn od_hidden() -> bool {
    fs::read_to_string(cfg_path()).map_or(true, |c| !c.contains("hide_onedrive=0"))
}

fn set_od_hidden(v: bool) -> Result<(), String> {
    fs::create_dir_all(bar_dir()).and_then(|_| fs::write(cfg_path(), format!("hide_onedrive={}\n", v as u8))).map_err(|e| e.to_string())
}

// PowerShell profile block (src/profile.ps1: zoxide, grep->rg, ll->eza, cat->bat) that lives exactly as long as topbar is installed.
// Written to Windows PowerShell 5.1 and, if installed, pwsh 7 (each has its own $PROFILE).
// shortcut: the elevated user's profile, which is the same account unless UAC asks for a different admin; upgrade if that matters.
const PS_STRIP: &str = r"$p=$PROFILE; $t=[string]$(if(Test-Path $p){Get-Content $p -Raw}); $t=[regex]::Replace($t,'(?s)# >>> topbar >>>.*?# <<< topbar <<<\r?\n?','');";
const PS_BLOCK: &str = include_str!("profile.ps1");
const SHELLS: [&str; 2] = ["powershell", "pwsh"];

fn profile_install() -> Result<(), String> {
    shells_run(&format!("{PS_STRIP}$b=@'\n{PS_BLOCK}'@; New-Item -Force -ItemType Directory (Split-Path $p) | Out-Null; Set-Content $p ($t.TrimEnd() + [Environment]::NewLine + $b) -Encoding utf8"))
}

fn profile_uninstall() -> Result<(), String> {
    // delete the file if nothing but our block was in it
    shells_run(&format!("{PS_STRIP} if($t.Trim()){{Set-Content $p $t -Encoding utf8}}elseif(Test-Path $p){{Remove-Item $p}}"))
}

/// Runs `script` in every shell. Only the first (always present) can fail the call: a missing pwsh is not an error.
fn shells_run(script: &str) -> Result<(), String> {
    let r = run(SHELLS[0], &["-NoProfile", "-Command", script]);
    let _ = run(SHELLS[1], &["-NoProfile", "-Command", script]);
    r
}

fn bar_install() -> Result<(), String> {
    with_payload(&["topbar.exe", "rs-du.exe", "reshell.exe"], |d| {
        let dst = bar_dir().join("topbar.exe");
        let _ = run("taskkill", &["/f", "/im", "topbar.exe"]);
        fs::create_dir_all(bar_dir()).map_err(|e| e.to_string())?;
        put(&d.join("topbar.exe"), &dst)?;
        fs::create_dir_all(tm_dir()).map_err(|e| e.to_string())?;
        put(&d.join("rs-du.exe"), &tm_dir().join("rs-du.exe"))?; // topbar menu "Чистильщик диска"
        let rsh = bar_dir().join("reshell.exe"); // Win+1/2/3 desktop hotkeys, no args = hotkeys
        let _ = run("taskkill", &["/f", "/im", "reshell.exe"]);
        put(&d.join("reshell.exe"), &rsh)?;
        run("reg", &["add", RUN_KEY, "/v", "reshell", "/t", "REG_SZ", "/d", &format!("\"{}\"", rsh.display()), "/f"])?;
        let _ = Command::new("explorer.exe").arg(&rsh).spawn();
        run(dst.to_str().unwrap(), &["--restore"])?; // clean state in case a killed instance left the taskbar hidden
        // The bar runs elevated (a task with the highest run level starts it at logon without a UAC prompt): that lets it close
        // elevated windows and click elevated apps' tray icons. It starts everything else through explorer, i.e. unelevated.
        // Fallback when the task cannot be made: the plain Run key and a normal-rights bar.
        let task = format!("$u=[Security.Principal.WindowsIdentity]::GetCurrent().Name; Register-ScheduledTask -TaskName topbar -Force -Action (New-ScheduledTaskAction -Execute '{}') -Trigger (New-ScheduledTaskTrigger -AtLogOn -User $u) -Principal (New-ScheduledTaskPrincipal -UserId $u -LogonType Interactive -RunLevel Highest) -Settings (New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero)) | Out-Null", dst.display());
        if run("powershell", &["-NoProfile", "-Command", &task]).is_ok() && run("schtasks", &["/run", "/tn", "topbar"]).is_ok() {
            let _ = run("reg", &["delete", RUN_KEY, "/v", "topbar", "/f"]);
        } else {
            WARN.lock().unwrap().push("задача topbar не создана: запуск через Run, без прав администратора".into());
            run("reg", &["add", RUN_KEY, "/v", "topbar", "/t", "REG_SZ", "/d", &format!("\"{}\"", dst.display()), "/f"])?;
            let _ = Command::new("explorer.exe").arg(&dst).spawn(); // normal user token, like rs-launcher
        }
        soft("профиль PowerShell не записан", profile_install()); // a shell nicety, must not fail the bar install
        Ok(())
    })
}

fn bar_uninstall() -> Result<(), String> {
    let _ = run("reg", &["delete", RUN_KEY, "/v", "topbar", "/f"]);
    let _ = run("schtasks", &["/delete", "/tn", "topbar", "/f"]);
    let _ = run("reg", &["delete", RUN_KEY, "/v", "reshell", "/f"]);
    let _ = run("taskkill", &["/f", "/im", "reshell.exe"]);
    let _ = run("taskkill", &["/f", "/im", "topbar.exe"]);
    let _ = profile_uninstall();
    let r = run(bar_dir().join("topbar.exe").to_str().unwrap(), &["--restore"]); // a killed bar cannot restore the taskbar itself
    let _ = fs::remove_dir_all(bar_dir());
    cleaner_release(tm_dir().join("rtaskmgr-win32.exe").is_file());
    r
}

// ---------- rust-explorer: file manager + Explorer replacement ----------
//
// Install: our files in Program Files, folders / drives opened from any program go to rust-explorer (HKCU class verbs of the
// installing user), rs-shell --overlay starts at sign-in and, with the ReplaceExplorer switch, ends the shell Explorer once topbar
// and the rust-explorer desktop are on screen. AutoRestartShell is set to 0 so Windows does not bring Explorer back.
// Uninstall puts everything back and starts Explorer again. rs-shell itself hands the session back to Explorer if topbar dies.
const RX_STATE: &str = r"HKLM\SOFTWARE\plugin-launcher\rust-explorer";
const WINLOGON_KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon";
const RX_SWITCH: &str = r"HKCU\Software\rs-shell";
const RX_FOLDERS: [&str; 3] = [r"HKCU\Software\Classes\Directory\shell\open", r"HKCU\Software\Classes\Folder\shell\open", r"HKCU\Software\Classes\Drive\shell\open"];

// svchost RAM trim, done with rust-explorer: (service, Start we set, Windows default). Uninstall puts back the Windows defaults,
// not the values found at install: they may already be our own (the same tweak was first run by hand).
// DoSvc's key is ACL-protected on some builds, so every step is soft: a refused one is only a warning.
const RX_SVCS: [(&str, u32, u32); 6] = [("DiagTrack", 4, 2), ("PcaSvc", 4, 2), ("InventorySvc", 4, 2), ("lfsvc", 4, 3), ("cbdhsvc", 4, 2), ("DoSvc", 3, 2)];
const SPLIT_KEY: &str = r"HKLM\SYSTEM\CurrentControlSet\Control";
const SPLIT_DEFAULT: u32 = 3670016; // 3.5 GB: above it Windows gives every service its own svchost

fn rx_trim(on: bool) {
    for (s, off, def) in RX_SVCS {
        let v = if on { off } else { def };
        soft(&format!("служба {s}"), run("reg", &["add", &format!(r"HKLM\SYSTEM\CurrentControlSet\Services\{s}"), "/v", "Start", "/t", "REG_DWORD", "/d", &v.to_string(), "/f"]));
        if on && off == 4 {
            let _ = run("sc.exe", &["stop", s]); // the per-user cbdhsvc_xxxx instance stops at the next sign-in
        }
    }
    let t = if on { 0x4000000 } else { SPLIT_DEFAULT };
    soft("SvcHostSplitThresholdInKB", run("reg", &["add", SPLIT_KEY, "/v", "SvcHostSplitThresholdInKB", "/t", "REG_DWORD", "/d", &t.to_string(), "/f"]));
}

fn rx_lnk() -> PathBuf {
    PathBuf::from(env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into())).join(r"plugin-launcher\rs-overlay.lnk")
}

fn rx_install() -> Result<(), String> {
    if !bar_dir().join("topbar.exe").is_file() {
        return Err("сначала установите topbar: без него после отключения Explorer не будет панели".into());
    }
    with_payload(&["rust-explorer.exe", "rs-shell.exe"], |d| {
        for n in ["rs-shell.exe", "rust-explorer.exe"] {
            let _ = run("taskkill", &["/f", "/im", n]); // an update: a running exe cannot be overwritten
        }
        fs::create_dir_all(rx_dir()).map_err(|e| e.to_string())?;
        let (fm, sh) = (rx_dir().join("rust-explorer.exe"), rx_dir().join("rs-shell.exe"));
        put(&d.join("rust-explorer.exe"), &fm)?;
        put(&d.join("rs-shell.exe"), &sh)?;
        // what Windows had, saved once (an update must not overwrite it with our own 0)
        if reg_dword(RX_STATE, "AutoRestartShell").is_none() {
            let v = reg_dword(WINLOGON_KEY, "AutoRestartShell").unwrap_or(1);
            run("reg", &["add", RX_STATE, "/v", "AutoRestartShell", "/t", "REG_DWORD", "/d", &v.to_string(), "/f"])?;
        }
        // folders and drives opened by other programs, the desktop, Win+R: rust-explorer. DelegateExecute is emptied on purpose:
        // while it is set, Windows ignores the command and uses Explorer's built-in handler.
        for k in RX_FOLDERS {
            let c = format!(r"{k}\command");
            run("reg", &["add", &c, "/ve", "/t", "REG_SZ", "/d", &format!("\"{}\" \"%1\"", fm.display()), "/f"])?;
            run("reg", &["add", &c, "/v", "DelegateExecute", "/t", "REG_SZ", "/d", "", "/f"])?;
        }
        // A logon task starts the overlay right at sign-in; Explorer runs the Run key only after its own startup delay (~20 s of
        // plain Windows on screen). Limited run level: the shell side must never run elevated. Fallback: the Run key.
        let task = format!("$u=[Security.Principal.WindowsIdentity]::GetCurrent().Name; Register-ScheduledTask -TaskName rs-overlay -Force -Action (New-ScheduledTaskAction -Execute '{}' -Argument '--overlay') -Trigger (New-ScheduledTaskTrigger -AtLogOn -User $u) -Principal (New-ScheduledTaskPrincipal -UserId $u -LogonType Interactive -RunLevel Limited) -Settings (New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero)) | Out-Null", sh.display());
        if run("powershell", &["-NoProfile", "-Command", &task]).is_ok() {
            let _ = run("reg", &["delete", RUN_KEY, "/v", "rs-overlay", "/f"]);
        } else {
            WARN.lock().unwrap().push("задача rs-overlay не создана: запуск через Run, с задержкой Windows".into());
            run("reg", &["add", RUN_KEY, "/v", "rs-overlay", "/t", "REG_SZ", "/d", &format!("\"{}\" --overlay", sh.display()), "/f"])?;
        }
        run("reg", &["add", RX_SWITCH, "/v", "ReplaceExplorer", "/t", "REG_DWORD", "/d", "1", "/f"])?;
        run("reg", &["add", WINLOGON_KEY, "/v", "AutoRestartShell", "/t", "REG_DWORD", "/d", "0", "/f"])?;
        rx_trim(true);
        // Start it now, with the user's normal token: explorer.exe opens a shortcut unelevated, and the shortcut carries --overlay.
        let lnk = rx_lnk();
        let _ = fs::create_dir_all(lnk.parent().unwrap());
        soft("overlay не запущен сейчас: он стартует при следующем входе", run("powershell", &["-NoProfile", "-Command", &format!("$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}'); $s.TargetPath='{}'; $s.Arguments='--overlay'; $s.Save()", lnk.display(), sh.display())]));
        let _ = Command::new("explorer.exe").arg(&lnk).spawn();
        Ok(())
    })
}

fn rx_uninstall() -> Result<(), String> {
    // Windows may restart its shell again; our hooks off; then the shell back, unelevated, only if none is running
    let back = reg_dword(RX_STATE, "AutoRestartShell").unwrap_or(1);
    let r = run("reg", &["add", WINLOGON_KEY, "/v", "AutoRestartShell", "/t", "REG_DWORD", "/d", &back.to_string(), "/f"]);
    let _ = run("reg", &["delete", RUN_KEY, "/v", "rs-overlay", "/f"]);
    let _ = run("schtasks", &["/delete", "/tn", "rs-overlay", "/f"]);
    let _ = run("reg", &["delete", RX_SWITCH, "/f"]);
    rx_trim(false);
    for k in RX_FOLDERS {
        let _ = run("reg", &["delete", k, "/f"]); // only our HKCU override; the system's verbs under HKLM stay
    }
    for n in ["rs-shell.exe", "rust-explorer.exe"] {
        let _ = run("taskkill", &["/f", "/im", n]);
    }
    let _ = run("powershell", &["-NoProfile", "-Command", r#"if (-not (Get-CimInstance Win32_Process -Filter "Name='explorer.exe'" | Where-Object { $_.CommandLine -match '(?i)explorer\.exe"?\s*$' })) { runas /trustlevel:0x20000 C:\Windows\explorer.exe }"#]);
    let _ = fs::remove_file(rx_lnk());
    let _ = fs::remove_dir_all(rx_dir());
    if r.is_ok() {
        let _ = run("reg", &["delete", RX_STATE, "/f"]);
    }
    r
}

// ---------- wiztree, photoview, debloat: small stand-alone tools, one exe each in its own Program Files folder ----------

fn wz_dir() -> PathBuf {
    program_files().join("wiztree")
}

fn pv_dir() -> PathBuf {
    program_files().join("photoview")
}

fn db_dir() -> PathBuf {
    program_files().join("debloat")
}

/// Start Menu shortcut: search and the bar's Start panel list Start Menu shortcuts only.
fn start_lnk(name: &str) -> PathBuf {
    PathBuf::from(env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into())).join(format!(r"Microsoft\Windows\Start Menu\Programs\{name}.lnk"))
}

/// Copies `exe` out of the payload into `dir` (an update: the running copy is killed first) and adds the Start Menu shortcut `lnk`.
fn tool_install(exe: &str, dir: &Path, lnk: Option<&str>) -> Result<(), String> {
    with_payload(&[exe], |d| {
        let _ = run("taskkill", &["/f", "/im", exe]);
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let dst = dir.join(exe);
        put(&d.join(exe), &dst)?;
        if let Some(n) = lnk {
            soft(&format!("ярлык «{n}» в меню Пуск не создан, поиск его не увидит"), run("powershell", &["-NoProfile", "-Command", &format!("$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}'); $s.TargetPath='{}'; $s.Save()", start_lnk(n).display(), dst.display())]));
        }
        Ok(())
    })
}

fn tool_remove(exe: &str, dir: &Path, lnk: Option<&str>) {
    if let Some(n) = lnk {
        let _ = fs::remove_file(start_lnk(n));
    }
    let _ = run("taskkill", &["/f", "/im", exe]);
    let _ = fs::remove_dir_all(dir);
}

fn wz_install() -> Result<(), String> {
    tool_install("wiztree.exe", &wz_dir(), Some("WinRust Disk"))
}

fn wz_uninstall() -> Result<(), String> {
    tool_remove("wiztree.exe", &wz_dir(), Some("WinRust Disk"));
    Ok(())
}

const PV_EXTS: [&str; 11] = ["jpg", "jpeg", "jfif", "png", "bmp", "gif", "tif", "tiff", "webp", "heic", "avif"]; // keep in step with photoview's EXTS
const PV_PROG: &str = "WinRustPhoto";
const PV_APP: &str = "WinRust Photo Viewer";

/// "Open with" entry and a Default apps entry; the default itself stays the user's choice (Windows does not allow setting it from code).
fn pv_install() -> Result<(), String> {
    tool_install("photoview.exe", &pv_dir(), Some("Photo viewer"))?;
    let exe = pv_dir().join("photoview.exe").display().to_string().replace('\\', r"\\");
    let mut reg = format!(
        "Windows Registry Editor Version 5.00\r\n\r\n[HKEY_LOCAL_MACHINE\\SOFTWARE\\Classes\\{PV_PROG}]\r\n@=\"Picture\"\r\n\r\n[HKEY_LOCAL_MACHINE\\SOFTWARE\\Classes\\{PV_PROG}\\shell\\open\\command]\r\n@=\"\\\"{exe}\\\" \\\"%1\\\"\"\r\n\r\n\
         [HKEY_LOCAL_MACHINE\\SOFTWARE\\WinRust\\PhotoView\\Capabilities]\r\n\"ApplicationName\"=\"{PV_APP}\"\r\n\"ApplicationDescription\"=\"Fast picture viewer\"\r\n\r\n[HKEY_LOCAL_MACHINE\\SOFTWARE\\WinRust\\PhotoView\\Capabilities\\FileAssociations]\r\n"
    );
    for e in PV_EXTS {
        reg += &format!("\".{e}\"=\"{PV_PROG}\"\r\n");
    }
    reg += &format!("\r\n[HKEY_LOCAL_MACHINE\\SOFTWARE\\RegisteredApplications]\r\n\"{PV_APP}\"=\"SOFTWARE\\\\WinRust\\\\PhotoView\\\\Capabilities\"\r\n");
    for e in PV_EXTS {
        reg += &format!("\r\n[HKEY_LOCAL_MACHINE\\SOFTWARE\\Classes\\.{e}\\OpenWithProgids]\r\n\"{PV_PROG}\"=hex(0):\r\n");
    }
    // the .reg lives in Program Files (admin-only), not %TEMP%
    let f = pv_dir().join("register.reg");
    fs::write(&f, reg).map_err(|e| format!("{}: {e}", f.display()))?;
    let r = run("reg", &["import", &f.display().to_string()]);
    let _ = fs::remove_file(&f);
    r
}

fn pv_uninstall() -> Result<(), String> {
    for e in PV_EXTS {
        let _ = run("reg", &["delete", &format!(r"HKLM\SOFTWARE\Classes\.{e}\OpenWithProgids"), "/v", PV_PROG, "/f"]);
    }
    let _ = run("reg", &["delete", r"HKLM\SOFTWARE\RegisteredApplications", "/v", PV_APP, "/f"]);
    let _ = run("reg", &["delete", &format!(r"HKLM\SOFTWARE\Classes\{PV_PROG}"), "/f"]);
    let _ = run("reg", &["delete", r"HKLM\SOFTWARE\WinRust\PhotoView", "/f"]);
    tool_remove("photoview.exe", &pv_dir(), Some("Photo viewer"));
    Ok(())
}

/// Installing = applying: debloat.exe --apply writes an undo file first, uninstalling plays it back.
fn db_install() -> Result<(), String> {
    tool_install("debloat.exe", &db_dir(), None)?;
    out(&db_dir().join("debloat.exe").display().to_string(), &["--apply"]).map(|_| ())
}

fn db_uninstall() -> Result<(), String> {
    let exe = db_dir().join("debloat.exe");
    if exe.is_file() {
        out(&exe.display().to_string(), &["--undo"])?; // on failure the exe and the undo file stay, so it can be repeated
    }
    tool_remove("debloat.exe", &db_dir(), None);
    Ok(())
}

fn taskbar_search(mode: u32) -> Result<(), String> {
    run("reg", &["add", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Search", "/v", "SearchboxTaskbarMode", "/t", "REG_DWORD", "/d", &mode.to_string(), "/f"])
}

fn install() -> Result<(), String> {
    // originals first: if anything below fails, the rollback restores exactly these
    save_dword("WSearchStart", reg_dword(WS_KEY, "Start"));
    save_dword("WSearchDelayed", Some(reg_dword(WS_KEY, "DelayedAutostart").unwrap_or(0)));
    save_dword("SearchboxMode", reg_dword(SB_KEY, "SearchboxTaskbarMode"));
    with_payload(&["rs.exe", "rs-launcher.exe"], |src| {
        run(src.join("rs.exe").to_str().unwrap(), &["--install"])?; // copies itself to Program Files, registers boot tasks
        let dst = prog_dir().join("rs-launcher.exe"); // must live somewhere stable: --autostart registers its own path
        let _ = run("taskkill", &["/f", "/im", "rs-launcher.exe"]);
        put(&src.join("rs-launcher.exe"), &dst)?;
        run(dst.to_str().unwrap(), &["--autostart"])?;
        // via explorer: it starts the launcher with the desktop user's normal token. Started from here it would
        // inherit admin rights, and every file opened from search would run elevated too.
        let _ = Command::new("explorer.exe").arg(&dst).spawn();
        run("sc.exe", &["config", "WSearch", "start=", "disabled"])?;
        let _ = run("sc.exe", &["stop", "WSearch"]);
        // SearchHost.exe (the Start/Win+S search UI) is not a service, so it gets the same IFEO trick as taskmgr: Windows runs `cmd /c exit` instead.
        run("reg", &["add", SH_KEY, "/v", "Debugger", "/t", "REG_SZ", "/d", "cmd.exe /c exit", "/f"])?;
        let _ = run("taskkill", &["/f", "/im", "SearchHost.exe"]);
        taskbar_search(0) // hide the taskbar search box
    })
}

fn uninstall() -> Result<(), String> {
    with_payload(&["rs.exe"], |src| {
        let _ = run("taskkill", &["/f", "/im", "rs-launcher.exe"]);
        let _ = run("reg", &["delete", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run", "/v", "rs-launcher", "/f"]);
        // run the unpacked copy: an exe cannot delete itself, so the installed one would leave its folder behind
        run(src.join("rs.exe").to_str().unwrap(), &["--uninstall"])?;
        let _ = fs::remove_dir_all(prog_dir());
        let _ = run("reg", &["delete", SH_KEY, "/v", "Debugger", "/f"]);
        // no saved values (installed by an older launcher): the Windows defaults
        let mode = match (reg_dword(STATE, "WSearchStart"), reg_dword(STATE, "WSearchDelayed")) {
            (Some(2), Some(0)) => "auto",
            (Some(3), _) => "demand",
            (Some(4), _) => "disabled",
            _ => "delayed-auto",
        };
        run("sc.exe", &["config", "WSearch", "start=", mode])?;
        if mode.ends_with("auto") {
            let _ = run("sc.exe", &["start", "WSearch"]);
        }
        let r = taskbar_search(reg_dword(STATE, "SearchboxMode").unwrap_or(2));
        if r.is_ok() {
            let _ = run("reg", &["delete", STATE, "/f"]);
        }
        r
    })
}

// Shared by the window and the command line, so both do exactly the same thing.
fn install_plugin(i: usize) -> Result<String, String> {
    let p = &PLUGINS[i];
    let (inst, uninst) = p.actions.ok_or("установка недоступна")?;
    let was = (p.installed)(); // already installed = an update: a failure must not remove the working copy
    WARN.lock().unwrap().clear();
    soft("точка восстановления не создана", checkpoint());
    if let Err(e) = inst() {
        log(&format!("{}: ОШИБКА {e}", p.name));
        if was {
            return Err(e);
        }
        let back = uninst().err().map_or(String::new(), |u| format!("; откат: {u}"));
        return Err(format!("{e} (установка отменена, изменения возвращены{back})"));
    }
    let w = WARN.lock().unwrap().clone();
    log(&format!("{}: {} {}", p.name, if was { "обновлён" } else { "установлен" }, w.join(" | ")));
    let note = if w.is_empty() { String::new() } else { format!(r" Предупреждений: {} (см. %ProgramData%\plugin-launcher\install.log).", w.len()) };
    Ok(format!("{} {}.{note}", p.name, if was { "обновлён" } else { "установлен" }))
}

fn uninstall_plugin(i: usize) -> Result<String, String> {
    let p = &PLUGINS[i];
    let (_, uninst) = p.actions.ok_or("удаление недоступно")?;
    uninst().inspect_err(|e| log(&format!("{}: ОШИБКА удаления {e}", p.name)))?;
    log(&format!("{}: удалён", p.name));
    Ok(format!("{} удалён, система восстановлена.", p.name))
}

fn rollback() -> Result<String, String> {
    // reverse install order: rust-explorer (which switched Explorer off) goes before topbar, else the session is left with no panel and no shell
    let errs: Vec<String> = PLUGINS.iter().rev().filter(|p| (p.installed)()).filter_map(|p| p.actions.and_then(|a| a.1().err().map(|e| format!("{}: {e}", p.name)))).collect();
    if errs.is_empty() { Ok("Все плагины удалены, система восстановлена.".into()) } else { Err(errs.join("; ")) }
}

// ---------- command line ----------
//
//   plugin-launcher list
//   plugin-launcher install <plugin> --yes      (without --yes it only prints the warning: nothing is changed)
//   plugin-launcher uninstall <plugin>
//   plugin-launcher rollback --yes
// Exit code 0 = done, 1 = failed, 2 = bad arguments or --yes missing. Run it from an elevated terminal: the manifest
// demands admin, and from a normal one Windows opens a separate elevated console whose output is lost.

fn cli(args: &[String]) -> i32 {
    // A windows-subsystem exe has no console of its own: use stdout if the caller redirected it (pipe, file),
    // else attach to the parent's console and write to it directly.
    let redirected = unsafe { !GetStdHandle(STD_OUTPUT_HANDLE).is_null() && GetStdHandle(STD_OUTPUT_HANDLE) != INVALID_HANDLE_VALUE };
    let mut out: Box<dyn Write> = if redirected {
        Box::new(std::io::stdout())
    } else {
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
        fs::OpenOptions::new().write(true).open("CONOUT$").map_or_else(|_| Box::new(std::io::sink()) as Box<dyn Write>, |f| Box::new(f))
    };
    let mut say = |s: &str| {
        let _ = writeln!(out, "{s}");
    };
    let yes = args.iter().any(|a| a == "--yes");
    let words: Vec<&str> = args.iter().map(String::as_str).filter(|a| *a != "--yes").collect();
    let find = |n: &str| PLUGINS.iter().position(|p| p.name == n);
    let usage = "usage: plugin-launcher list | install <plugin> --yes | update <plugin> | uninstall <plugin> | rollback --yes";
    let result = match words.as_slice() {
        ["list"] => {
            for p in PLUGINS {
                say(&format!("{:<16}{}", p.name, if p.actions.is_none() { "stub" } else if (p.installed)() { "installed" } else { "not installed" }));
            }
            return 0;
        }
        ["install", n] => match find(n) {
            None => return { say(&format!("unknown plugin: {n}")); 2 },
            Some(i) if !yes => return { say(&format!("{}\nNothing changed. Repeat with --yes to install {n}.", PLUGINS[i].warn)); 2 },
            Some(i) if (PLUGINS[i].installed)() => return { say(&format!("{n} is already installed")); 0 },
            Some(i) => install_plugin(i),
        },
        ["update", n] => match find(n) {
            None => return { say(&format!("unknown plugin: {n}")); 2 },
            Some(i) if !(PLUGINS[i].installed)() => return { say(&format!("{n} is not installed (use install)")); 2 },
            Some(i) => install_plugin(i),
        },
        ["uninstall", n] => match find(n) {
            None => return { say(&format!("unknown plugin: {n}")); 2 },
            Some(i) if !(PLUGINS[i].installed)() => return { say(&format!("{n} is not installed")); 0 },
            Some(i) => uninstall_plugin(i),
        },
        ["rollback"] if yes => rollback(),
        ["rollback"] => return { say("Removes every installed plugin. Repeat with --yes."); 2 },
        _ => return { say(usage); 2 },
    };
    match result {
        Ok(m) => { say(&m); 0 }
        Err(e) => { say(&format!("error: {e}")); 1 }
    }
}

// ---------- drawing ----------
//
// One back buffer per paint: GDI+ for the anti-aliased shapes, then GDI (ClearType) for text on top.

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// COLORREF (0x00BBGGRR) + alpha -> GDI+ ARGB.
const fn argb(c: u32, a: u8) -> u32 {
    (a as u32) << 24 | (c & 255) << 16 | (c >> 8 & 255) << 8 | (c >> 16 & 255)
}

fn lerp(a: u32, b: u32, t: f32) -> u32 {
    let m = |sh: u32| (((a >> sh & 255) as f32) * (1.0 - t) + ((b >> sh & 255) as f32) * t) as u32;
    m(0) | m(8) << 8 | m(16) << 16
}

type Box4 = [f32; 4]; // left, top, right, bottom in logical px

struct Ui {
    dc: HDC,
    g: *mut GpGraphics,
    s: f32,
}
impl Ui {
    fn px(&self, v: f32) -> i32 {
        (v * self.s).round() as i32
    }
    unsafe fn shape(&self, b: Box4, rad: f32, color: u32) {
        let (s, mut path) = (self.s, null_mut::<GpPath>());
        let (x, y, w, h) = (b[0] * s, b[1] * s, (b[2] - b[0]) * s, (b[3] - b[1]) * s);
        let d = (rad * 2.0 * s).min(h).min(w).max(0.01);
        GdipCreatePath(FillModeAlternate, &mut path);
        GdipAddPathArc(path, x, y, d, d, 180.0, 90.0);
        GdipAddPathArc(path, x + w - d, y, d, d, 270.0, 90.0);
        GdipAddPathArc(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
        GdipAddPathArc(path, x, y + h - d, d, d, 90.0, 90.0);
        GdipClosePathFigure(path);
        let mut br = null_mut::<GpSolidFill>();
        GdipCreateSolidFill(color, &mut br);
        GdipFillPath(self.g, br as *mut GpBrush, path);
        GdipDeleteBrush(br as *mut GpBrush);
        GdipDeletePath(path);
    }
    /// Rounded box with a 1 px outline (`line` == `fill` for none).
    unsafe fn round(&self, b: Box4, rad: f32, fill: u32, line: u32) {
        if line != fill {
            self.shape(b, rad, argb(line, 255));
            let i = 1.0 / self.s;
            self.shape([b[0] + i, b[1] + i, b[2] - i, b[3] - i], rad - i, argb(fill, 255));
        } else {
            self.shape(b, rad, argb(fill, 255));
        }
    }
    unsafe fn font(&self, size: f32, weight: i32, face: &str) -> HFONT {
        CreateFontW(-self.px(size), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, wide(face).as_ptr())
    }
    unsafe fn text_in(&self, s: &str, b: Box4, size: f32, weight: i32, color: u32, flags: u32, face: &str) {
        let f = self.font(size, weight, face);
        let of = SelectObject(self.dc, f);
        SetTextColor(self.dc, color);
        SetBkMode(self.dc, TRANSPARENT as i32);
        let mut r = RECT { left: self.px(b[0]), top: self.px(b[1]), right: self.px(b[2]), bottom: self.px(b[3]) };
        DrawTextW(self.dc, wide(s).as_ptr(), -1, &mut r, flags | DT_NOPREFIX);
        SelectObject(self.dc, of);
        DeleteObject(f);
    }
    unsafe fn text(&self, s: &str, b: Box4, size: f32, weight: i32, color: u32, flags: u32) {
        self.text_in(s, b, size, weight, color, flags, "Segoe UI");
    }
    /// Width of a single line of text, logical px.
    unsafe fn width(&self, s: &str, size: f32, weight: i32) -> f32 {
        let f = self.font(size, weight, "Segoe UI");
        let of = SelectObject(self.dc, f);
        let mut r: RECT = zeroed();
        DrawTextW(self.dc, wide(s).as_ptr(), -1, &mut r, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(self.dc, of);
        DeleteObject(f);
        r.right as f32 / self.s
    }
}

// layout, logical px
const WIN_W: f32 = 600.0;
const M: f32 = 24.0; // outer margin
const N_READY: usize = {
    let (mut n, mut i) = (0, 0);
    while i < PLUGINS.len() {
        if PLUGINS[i].actions.is_some() && PLUGINS[i].bulk {
            n += 1;
        }
        i += 1;
    }
    n
};
const LIST_TOP: f32 = 136.0;
const ROW_H: f32 = 62.0;
const LIST_H: f32 = N_READY as f32 * ROW_H + 12.0;
const MAIN_TOP: f32 = LIST_TOP + LIST_H + 20.0;
const MAIN_H: f32 = 54.0;
const FOOT_H: f32 = 72.0;
const WIN_H: f32 = MAIN_TOP + MAIN_H + 16.0 + FOOT_H;
const BTN_W: f32 = 136.0;
const BTN_H: f32 = 36.0;
// clickable things
const ID_MAIN: usize = 0;
const ID_ROLLBACK: usize = 1;
const ID_OK: usize = 2;
const ID_CANCEL: usize = 3;
const ID_SWITCH: usize = 4;
const ID_GEAR: usize = 5;
const STARTING: usize = usize::MAX - 1; // JOB before install_all picks its first component

/// Installable components, in install order (topbar comes before rust-explorer, which needs it).
fn ready() -> impl Iterator<Item = usize> {
    (0..PLUGINS.len()).filter(|&i| PLUGINS[i].actions.is_some() && PLUGINS[i].bulk)
}

fn row_box(k: usize, w: f32) -> Box4 {
    let t = LIST_TOP + 6.0 + k as f32 * ROW_H;
    [M + 6.0, t, w - M - 6.0, t + ROW_H]
}

/// Settings gear at the right end of the row of the plugin that has settings (topbar): (plugin index, box).
fn gear_box(w: f32) -> Option<(usize, Box4)> {
    let (k, i) = ready().enumerate().find(|&(_, i)| PLUGINS[i].settings)?;
    let r = row_box(k, w);
    let t = r[1] + (ROW_H - 32.0) / 2.0;
    Some((i, [r[2] - 14.0 - 32.0, t, r[2] - 14.0, t + 32.0]))
}

fn main_box(w: f32) -> Box4 {
    [M, MAIN_TOP, w - M, MAIN_TOP + MAIN_H]
}

/// On/off switch inside the settings panel row.
fn switch_box(w: f32) -> Box4 {
    let (d, _, _) = dialog_boxes(w);
    [d[2] - 24.0 - 16.0 - 44.0, d[1] + 78.0 + 23.0, d[2] - 24.0 - 16.0, d[1] + 78.0 + 47.0]
}

fn rollback_box(w: f32) -> Box4 {
    let t = WIN_H - FOOT_H + (FOOT_H - BTN_H) / 2.0;
    [w - M - 150.0, t, w - M, t + BTN_H]
}

/// Dialog panel and its (cancel, ok) buttons.
fn dialog_boxes(w: f32) -> (Box4, Box4, Box4) {
    let (dw, dh) = (528.0, 352.0);
    let (l, t) = ((w - dw) / 2.0, (WIN_H - dh) / 2.0);
    let (bt, r) = (t + dh - 24.0 - BTN_H, l + dw - 24.0);
    ([l, t, l + dw, t + dh], [r - BTN_W - 10.0 - 120.0, bt, r - BTN_W - 10.0, bt + BTN_H], [r - BTN_W, bt, r, bt + BTN_H])
}

/// A button: `kind` 0 = primary (accent), 1 = secondary (outline), 2 = danger (outline, red text), 3 = disabled.
unsafe fn button(ui: &Ui, b: Box4, label: &str, kind: u8, hot: bool) {
    let (fill, line, fg) = match kind {
        0 => (ACCENT, ACCENT, rgb(255, 255, 255)),
        1 => (PANEL, BORDER, TEXT),
        2 => (PANEL, BORDER, BAD),
        _ => (PANEL, lerp(PANEL, BORDER, 0.6), lerp(DIM, PANEL, 0.35)),
    };
    let fill = if hot && kind != 3 { lerp(fill, TEXT, if kind == 0 { 0.14 } else { 0.07 }) } else { fill };
    let line = if hot && kind == 2 { BAD } else if kind == 0 { fill } else { line };
    ui.round(b, 8.0, fill, line);
    ui.text(label, b, 13.5, 600, fg, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
}

unsafe fn paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc: RECT = zeroed();
    GetClientRect(hwnd, &mut rc);
    let (cw, ch) = (rc.right, rc.bottom);
    let mem = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, cw, ch);
    let ob = SelectObject(mem, bmp);
    let s = GetDeviceCaps(hdc, LOGPIXELSX as i32) as f32 / 96.0;
    let bgb = CreateSolidBrush(BG);
    FillRect(mem, &rc, bgb);
    DeleteObject(bgb);
    let mut g = null_mut::<GpGraphics>();
    GdipCreateFromHDC(mem, &mut g);
    GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
    let ui = Ui { dc: mem, g, s };
    let w = cw as f32 / s;
    let hover = HOVER.get();
    let dialog = DIALOG.with_borrow(|d| d.as_ref().map(|d| (d.title.clone(), d.text.clone(), d.ok.clone(), d.danger)));
    let hot = |id: usize| hover == Some(id);
    let busy = BUSY.load(Relaxed);
    let job = JOB.load(Relaxed);
    let n = PLUGINS.len();
    let n_inst = ready().filter(|&i| INSTALLED[i].load(Relaxed)).count();

    // header: logo + name
    ui.shape([0.0, 0.0, w, 3.0], 0.0, argb(ACCENT, 255));
    let logo = [M, 30.0, M + 60.0, 90.0];
    ui.round(logo, 16.0, ACCENT, ACCENT);
    let inner = lerp(ACCENT, BG, 0.12);
    ui.round([logo[0] + 4.0, logo[1] + 4.0, logo[2] - 4.0, logo[3] - 4.0], 13.0, inner, inner);
    ui.text("W", [logo[0], logo[1] - 2.0, logo[2], logo[3]], 32.0, 900, rgb(255, 255, 255), DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    let tx = logo[2] + 18.0;
    ui.text("WinRust", [tx, 28.0, w - M, 64.0], 30.0, 700, TEXT, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
    ui.text(&format!("Оболочка Windows на Rust  \u{00B7}  установлено {n_inst} из {N_READY}"), [tx, 64.0, w - M, 88.0], 13.0, 400, DIM, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
    ui.text("ВХОДИТ В СОСТАВ", [M + 4.0, LIST_TOP - 26.0, w - M, LIST_TOP - 6.0], 11.0, 700, DIM, DT_LEFT | DT_SINGLELINE | DT_VCENTER);

    // component list
    ui.round([M, LIST_TOP, w - M, LIST_TOP + LIST_H], 12.0, PANEL, BORDER);
    let pos = ready().position(|i| i == job); // where install_all is now
    let gear = gear_box(w);
    for (k, i) in ready().enumerate() {
        let p = &PLUGINS[i];
        let r = row_box(k, w);
        let installed = INSTALLED[i].load(Relaxed);
        let working = busy && (job == i || (job == n && installed));
        let queued = busy && !installed && (job == STARTING || pos.is_some_and(|q| k > q));
        if k > 0 {
            ui.shape([r[0] + 12.0, r[1], r[2] - 12.0, r[1] + 1.0 / s], 0.0, argb(BORDER, 255));
        }
        if working {
            let t = lerp(PANEL, ACCENT, 0.10);
            ui.round([r[0], r[1] + 3.0, r[2], r[3] - 3.0], 9.0, t, t);
        }
        let tile = [r[0] + 10.0, r[1] + 13.0, r[0] + 46.0, r[1] + 49.0];
        let lit = installed || working;
        let tint = if lit { lerp(PANEL, ACCENT, 0.20) } else { lerp(PANEL, TEXT, 0.06) };
        ui.round(tile, 9.0, tint, tint);
        ui.text_in(p.glyph, tile, 16.0, 400, if lit { ACCENT } else { TEXT }, DT_CENTER | DT_VCENTER | DT_SINGLELINE, "Segoe MDL2 Assets");

        let (pill, pc) = if working {
            (if job == n { "Удаление…" } else { "Установка…" }, ACCENT)
        } else if installed {
            ("Установлен", GOOD)
        } else if queued {
            ("В очереди", DIM)
        } else {
            ("Не установлен", DIM)
        };
        let pr = r[2] - 14.0 - 32.0 - 10.0;
        let pw = ui.width(pill, 11.5, 600) + 18.0;
        let pb = [pr - pw, r[1] + 21.0, pr, r[1] + 41.0];
        let ptint = lerp(PANEL, pc, 0.16);
        ui.round(pb, 10.0, ptint, ptint);
        ui.text(pill, pb, 11.5, 600, pc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);

        let nx = tile[2] + 14.0;
        ui.text(p.name, [nx, r[1] + 11.0, pb[0] - 10.0, r[1] + 32.0], 14.5, 600, TEXT, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
        let short = p.desc.split(". ").next().unwrap_or(p.desc).trim_end_matches('.');
        ui.text(short, [nx, r[1] + 32.0, pb[0] - 10.0, r[3] - 10.0], 12.0, 400, DIM, DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS);

        if let Some((_, gb)) = gear.filter(|&(gi, _)| gi == i && installed) {
            let h = hot(ID_GEAR) && !busy;
            ui.round(gb, 8.0, if h { lerp(PANEL, TEXT, 0.08) } else { PANEL }, BORDER);
            ui.text_in("\u{E713}", gb, 14.0, 400, if busy { DIM } else { TEXT }, DT_CENTER | DT_VCENTER | DT_SINGLELINE, "Segoe MDL2 Assets");
        }
    }

    // the one button: installs everything that is missing (or updates all when nothing is)
    let mb = main_box(w);
    if busy {
        let base = lerp(PANEL, ACCENT, 0.22);
        ui.round(mb, 12.0, base, base);
        let done = if job == n { 0.0 } else { pos.map_or(0.0, |q| (q as f32 + 0.5) / N_READY as f32) };
        if done > 0.0 {
            ui.shape([mb[0], mb[1], mb[0] + (mb[2] - mb[0]) * done, mb[3]], 12.0, argb(ACCENT, 255));
        }
        let label = if job == n {
            "Удаление WinRust…".to_string()
        } else {
            pos.map_or("Подготовка…".into(), |q| format!("Устанавливается {}  ({} из {N_READY})", PLUGINS[job].name, q + 1))
        };
        ui.text(&label, mb, 15.5, 700, TEXT, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    } else {
        let all = n_inst == N_READY;
        let label = if all {
            "Обновить WinRust".to_string()
        } else if n_inst == 0 {
            "Установить WinRust".into()
        } else {
            format!("Доустановить WinRust  ({} из {N_READY})", N_READY - n_inst)
        };
        let (fill, fg) = if all { (PANEL, TEXT) } else { (ACCENT, rgb(255, 255, 255)) };
        let fill = if hot(ID_MAIN) { lerp(fill, TEXT, if all { 0.07 } else { 0.14 }) } else { fill };
        ui.round(mb, 12.0, fill, if all { BORDER } else { fill });
        let tw = ui.width(&label, 16.0, 700);
        let cx = (mb[0] + mb[2]) / 2.0 + 13.0;
        ui.text_in(if all { "\u{E72C}" } else { "\u{E896}" }, [cx - tw / 2.0 - 30.0, mb[1], cx - tw / 2.0 - 6.0, mb[3]], 15.0, 400, fg, DT_CENTER | DT_VCENTER | DT_SINGLELINE, "Segoe MDL2 Assets");
        ui.text(&label, [cx - tw / 2.0, mb[1], cx + tw / 2.0 + 4.0, mb[3]], 16.0, 700, fg, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
    }

    // footer: status on the left, remove-all on the right
    let ft = WIN_H - FOOT_H;
    ui.shape([M, ft, w - M, ft + 1.0 / s], 0.0, argb(BORDER, 255));
    let (m, ok) = MSG.lock().unwrap().clone();
    let (m, dot, fg) = if busy {
        ("Выполняется, не закрывайте окно…".to_string(), ACCENT, DIM)
    } else if m.is_empty() {
        ("Перед установкой создаётся точка восстановления.".into(), DIM, DIM)
    } else if ok {
        (m, GOOD, TEXT)
    } else {
        (m, BAD, BAD)
    };
    let rb = rollback_box(w);
    ui.shape([M, ft + FOOT_H / 2.0 - 4.0, M + 8.0, ft + FOOT_H / 2.0 + 4.0], 4.0, argb(dot, 255));
    let one_line = ui.width(&m, 12.5, 400) < rb[0] - 16.0 - M - 18.0;
    let tb = [M + 18.0, ft + if one_line { 0.0 } else { 12.0 }, rb[0] - 16.0, WIN_H - if one_line { 0.0 } else { 8.0 }];
    ui.text(&m, tb, 12.5, 400, fg, DT_LEFT | DT_END_ELLIPSIS | if one_line { DT_SINGLELINE | DT_VCENTER } else { DT_WORDBREAK });
    button(&ui, rb, "Удалить всё", if n_inst > 0 && !busy { 2 } else { 3 }, hot(ID_ROLLBACK));

    // confirmation dialog over the dimmed window
    if let Some((title, text, ok_label, danger)) = dialog {
        ui.shape([0.0, 0.0, w, WIN_H + 1.0], 0.0, argb(0, 160));
        let (d, cancel, okb) = dialog_boxes(w);
        ui.round(d, 14.0, lerp(PANEL, BG, 0.25), BORDER);
        let accent = if danger { BAD } else { ACCENT };
        let icon = [d[0] + 24.0, d[1] + 24.0, d[0] + 60.0, d[1] + 60.0];
        let tint = lerp(PANEL, accent, 0.18);
        ui.round(icon, 18.0, tint, tint);
        ui.text_in(if danger { "\u{E7BA}" } else { "\u{E946}" }, icon, 16.0, 400, accent, DT_CENTER | DT_VCENTER | DT_SINGLELINE, "Segoe MDL2 Assets");
        ui.text(&title, [icon[2] + 14.0, d[1] + 24.0, d[2] - 24.0, d[1] + 60.0], 17.0, 600, TEXT, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
        ui.text(&text, [d[0] + 24.0, d[1] + 76.0, d[2] - 24.0, cancel[1] - 12.0], 12.5, 400, DIM, DT_LEFT | DT_WORDBREAK | DT_END_ELLIPSIS);
        button(&ui, cancel, "Отмена", 1, hot(ID_CANCEL));
        let okfill = if hot(ID_OK) { lerp(accent, TEXT, 0.14) } else { accent };
        ui.round(okb, 8.0, okfill, okfill);
        ui.text(&ok_label, okb, 13.5, 600, rgb(255, 255, 255), DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    }

    // settings panel of a plugin (topbar: OneDrive switch)
    if let Some(i) = SETTINGS.get() {
        ui.shape([0.0, 0.0, w, WIN_H + 1.0], 0.0, argb(0, 160));
        let (d, _, closeb) = dialog_boxes(w);
        ui.round(d, 14.0, lerp(PANEL, BG, 0.25), BORDER);
        let icon = [d[0] + 24.0, d[1] + 24.0, d[0] + 60.0, d[1] + 60.0];
        let tint = lerp(PANEL, ACCENT, 0.18);
        ui.round(icon, 18.0, tint, tint);
        ui.text_in("\u{E713}", icon, 16.0, 400, ACCENT, DT_CENTER | DT_VCENTER | DT_SINGLELINE, "Segoe MDL2 Assets");
        ui.text(&format!("Настройки {}", PLUGINS[i].name), [icon[2] + 14.0, d[1] + 24.0, d[2] - 24.0, d[1] + 60.0], 17.0, 600, TEXT, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
        let row = [d[0] + 24.0, d[1] + 78.0, d[2] - 24.0, d[1] + 148.0];
        let hid = od_hidden();
        ui.round(row, 10.0, PANEL, BORDER);
        ui.text("Отключение OneDrive", [row[0] + 16.0, row[1] + 10.0, row[2] - 80.0, row[1] + 32.0], 14.0, 600, TEXT, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
        ui.text("Значок OneDrive не показывается в топбаре (по умолчанию). Синхронизация продолжает работать.", [row[0] + 16.0, row[1] + 32.0, row[2] - 80.0, row[3] - 6.0], 12.0, 400, DIM, DT_LEFT | DT_WORDBREAK);
        let sw = switch_box(w);
        let off = lerp(PANEL, TEXT, 0.25);
        let track = if hid { ACCENT } else { off };
        ui.round(sw, 12.0, track, track);
        let kx = if hid { sw[2] - 12.0 } else { sw[0] + 12.0 };
        ui.round([kx - 8.0, sw[1] + 4.0, kx + 8.0, sw[3] - 4.0], 8.0, TEXT, TEXT);
        button(&ui, closeb, "Закрыть", 1, hot(ID_CANCEL));
    }

    GdipDeleteGraphics(g);
    BitBlt(hdc, 0, 0, cw, ch, mem, 0, 0, SRCCOPY);
    SelectObject(mem, ob);
    DeleteObject(bmp);
    DeleteDC(mem);
    EndPaint(hwnd, &ps);
}

// ---------- window ----------

/// What a confirmed dialog runs.
#[derive(Clone, Copy)]
enum Act {
    InstallAll,
    Rollback,
}
struct Dialog {
    title: String,
    text: String,
    ok: String,
    danger: bool,
    act: Act,
}
thread_local! { static DIALOG: std::cell::RefCell<Option<Dialog>> = const { std::cell::RefCell::new(None) }; }

/// Clickable thing under the cursor (ID_*). While a panel is up only its buttons count; while a job runs nothing does.
unsafe fn hit(hwnd: HWND, lp: isize) -> Option<usize> {
    let (x, y) = ((lp & 0xFFFF) as i16 as f32, ((lp >> 16) & 0xFFFF) as i16 as f32);
    let (mut rc, dc): (RECT, _) = (zeroed(), GetDC(hwnd));
    let s = GetDeviceCaps(dc, LOGPIXELSX as i32) as f32 / 96.0;
    ReleaseDC(hwnd, dc);
    GetClientRect(hwnd, &mut rc);
    let w = rc.right as f32 / s;
    let inside = |b: Box4| x >= b[0] * s && x <= b[2] * s && y >= b[1] * s && y <= b[3] * s;
    if SETTINGS.get().is_some() {
        let (_, _, closeb) = dialog_boxes(w);
        return if inside(switch_box(w)) { Some(ID_SWITCH) } else if inside(closeb) { Some(ID_CANCEL) } else { None };
    }
    if DIALOG.with_borrow(|d| d.is_some()) {
        let (_, cancel, ok) = dialog_boxes(w);
        return if inside(ok) { Some(ID_OK) } else if inside(cancel) { Some(ID_CANCEL) } else { None };
    }
    if BUSY.load(Relaxed) {
        return None;
    }
    if gear_box(w).is_some_and(|(i, b)| INSTALLED[i].load(Relaxed) && inside(b)) {
        return Some(ID_GEAR);
    }
    if inside(main_box(w)) {
        return Some(ID_MAIN);
    }
    (INSTALLED.iter().any(|f| f.load(Relaxed)) && inside(rollback_box(w))).then_some(ID_ROLLBACK)
}

/// Runs `job` off the UI thread; its Ok/Err text becomes the status line. `which` = plugin index, n = rollback.
fn start(hwnd: HWND, which: usize, job: impl FnOnce() -> Result<String, String> + Send + 'static) {
    JOB.store(which, Relaxed);
    BUSY.store(true, Relaxed);
    HOVER.set(None);
    unsafe { InvalidateRect(hwnd, null(), 0) };
    let h = hwnd as isize;
    thread::spawn(move || {
        *MSG.lock().unwrap() = match job() {
            Ok(m) => (m, true),
            Err(e) => (format!("Ошибка: {e}"), false),
        };
        refresh();
        BUSY.store(false, Relaxed);
        unsafe { PostMessageW(h as HWND, WM_DONE, 0, 0) };
    });
}

/// One click: every component that is missing, in order (all of them again = update when nothing is missing).
/// Stops at the first failure: rust-explorer must not go on without topbar. `h` = window to repaint between steps.
fn install_all(h: isize) -> Result<String, String> {
    let all = ready().all(|i| (PLUGINS[i].installed)());
    let (mut done, mut warned) = (Vec::new(), false);
    for i in ready() {
        if !all && (PLUGINS[i].installed)() {
            continue;
        }
        JOB.store(i, Relaxed);
        unsafe { InvalidateRect(h as HWND, null(), 0) };
        if let Err(e) = install_plugin(i) {
            let ok = if done.is_empty() { String::new() } else { format!(" Уже готово: {}.", done.join(", ")) };
            return Err(format!("{}: {e}.{ok}", PLUGINS[i].name));
        }
        warned |= !WARN.lock().unwrap().is_empty(); // install_plugin clears the list for each component
        done.push(PLUGINS[i].name);
        refresh();
    }
    let note = if warned { r" Есть предупреждения (см. %ProgramData%\plugin-launcher\install.log)." } else { "" };
    Ok(format!("WinRust {}: {}.{note}", if all { "обновлён" } else { "установлен" }, done.join(", ")))
}

unsafe fn ask(hwnd: HWND, d: Dialog) {
    DIALOG.set(Some(d));
    HOVER.set(None);
    InvalidateRect(hwnd, null(), 0);
}

unsafe fn close_dialog(hwnd: HWND, run: bool) {
    let Some(d) = DIALOG.take() else { return };
    HOVER.set(None);
    InvalidateRect(hwnd, null(), 0);
    if !run {
        return;
    }
    match d.act {
        Act::Rollback => start(hwnd, PLUGINS.len(), rollback),
        Act::InstallAll => {
            let h = hwnd as isize;
            start(hwnd, STARTING, move || install_all(h))
        }
    }
}

unsafe fn click(hwnd: HWND, id: usize) {
    if SETTINGS.get().is_some() {
        if id == ID_SWITCH {
            if let Err(e) = set_od_hidden(!od_hidden()) {
                *MSG.lock().unwrap() = (format!("Ошибка: {e}"), false);
            }
        } else if id == ID_CANCEL {
            SETTINGS.set(None);
        }
        HOVER.set(None);
        InvalidateRect(hwnd, null(), 0);
        return;
    }
    if id == ID_OK || id == ID_CANCEL {
        return close_dialog(hwnd, id == ID_OK);
    }
    if BUSY.load(Relaxed) {
        return;
    }
    match id {
        ID_GEAR => {
            SETTINGS.set(PLUGINS.iter().position(|p| p.settings));
            HOVER.set(None);
            InvalidateRect(hwnd, null(), 0);
        }
        ID_ROLLBACK => ask(hwnd, Dialog {
            title: "Удалить WinRust?".into(),
            text: "Все компоненты будут удалены. Вернутся поиск Windows, стандартный диспетчер задач, панель задач и обычный Проводник.".into(),
            ok: "Удалить всё".into(),
            danger: true,
            act: Act::Rollback,
        }),
        ID_MAIN if ready().all(|i| INSTALLED[i].load(Relaxed)) => ask(hwnd, Dialog {
            title: "Обновить WinRust?".into(),
            text: "Все компоненты будут переустановлены из этой версии установщика. Настройки сохранятся.".into(),
            ok: "Обновить".into(),
            danger: false,
            act: Act::InstallAll,
        }),
        ID_MAIN => ask(hwnd, Dialog {
            title: "Установить WinRust?".into(),
            text: "Компоненты Windows будут заменены:\n\u{2022} поиск Windows отключается, вместо него rust-search (Alt+Space);\n\u{2022} диспетчер задач заменяется на rtaskmgr;\n\u{2022} панель задач заменяется на topbar (внизу экрана);\n\u{2022} Проводник как оболочка отключается: папки и рабочий стол открывает rust-explorer, отключаются службы телеметрии.\n\nСоздаётся точка восстановления, «Удалить всё» вернёт систему. Если экран останется пустым: Ctrl+Alt+Del \u{2192} Диспетчер задач \u{2192} Запустить explorer.exe.".into(),
            ok: "Установить".into(),
            danger: false,
            act: Act::InstallAll,
        }),
        _ => {}
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => paint(hwnd),
        WM_ERASEBKGND => return 1,
        WM_MOUSEMOVE => {
            let h = hit(hwnd, lp);
            if h != HOVER.get() {
                HOVER.set(h);
                InvalidateRect(hwnd, null(), 0);
            }
            let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            TrackMouseEvent(&mut t);
        }
        WM_SETCURSOR if (lp & 0xFFFF) as u32 == HTCLIENT => {
            SetCursor(LoadCursorW(null_mut(), if HOVER.get().is_some() { IDC_HAND } else { IDC_ARROW }));
            return 1;
        }
        WM_MOUSELEAVE => {
            HOVER.set(None);
            InvalidateRect(hwnd, null(), 0);
        }
        WM_LBUTTONUP => {
            if let Some(i) = hit(hwnd, lp) {
                click(hwnd, i);
            }
        }
        WM_KEYDOWN if SETTINGS.get().is_some() && wp as u16 == VK_ESCAPE => {
            SETTINGS.set(None);
            InvalidateRect(hwnd, null(), 0);
        }
        WM_KEYDOWN if DIALOG.with_borrow(|d| d.is_some()) => match wp as u16 {
            VK_ESCAPE => close_dialog(hwnd, false),
            VK_RETURN if DIALOG.with_borrow(|d| d.as_ref().is_some_and(|d| !d.danger)) => close_dialog(hwnd, true), // a dangerous one needs the click

            _ => {}
        },
        WM_DONE => {
            InvalidateRect(hwnd, null(), 0);
        }
        WM_CLOSE if BUSY.load(Relaxed) => {} // closing mid-install would leave the system half-changed
        WM_DESTROY => PostQuitMessage(0),
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    0
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if !args.is_empty() {
        std::process::exit(cli(&args));
    }
    unsafe {
        refresh();
        let (mut token, mut input): (usize, GdiplusStartupInput) = (0, zeroed());
        input.GdiplusVersion = 1;
        GdiplusStartup(&mut token, &input, null_mut());
        let hi = GetModuleHandleW(null());
        let cls = wide("WinRust");
        let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: hi, hCursor: LoadCursorW(null_mut(), IDC_ARROW), lpszClassName: cls.as_ptr(), ..zeroed() };
        RegisterClassW(&wc);
        let dc = GetDC(null_mut());
        let s = GetDeviceCaps(dc, LOGPIXELSX as i32) as f32 / 96.0;
        ReleaseDC(null_mut(), dc);
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut r = RECT { left: 0, top: 0, right: (WIN_W * s) as i32, bottom: (WIN_H * s) as i32 };
        AdjustWindowRect(&mut r, style, 0);
        let title = wide("WinRust");
        let hwnd = CreateWindowExW(0, cls.as_ptr(), title.as_ptr(), style | WS_VISIBLE, CW_USEDEFAULT, CW_USEDEFAULT, r.right - r.left, r.bottom - r.top, null_mut(), null_mut(), hi, null());
        DwmSetWindowAttribute(hwnd, 20, &1i32 as *const i32 as *const _, 4); // dark title bar
        DwmSetWindowAttribute(hwnd, 35, &BG as *const u32 as *const _, 4); // caption colour = window background (Win11; ignored on Win10)
        let mut m: MSG = zeroed();
        while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}
