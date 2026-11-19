// debloat              show what would change (nothing is modified)
// debloat --apply      apply (admin); the old value of every change is written to the undo file BEFORE the change
// debloat --undo       put every value and service back exactly as it was
//
// Only reversible things: a few services and registry policies. Not touched on purpose: Windows Update, Defender, Print Spooler,
// Xbox/Game services, SysMain, and installed apps (removing an AppX package cannot be undone from here).
// Side effect worth knowing: Edge shows "managed by your organization" while its two policies below are set.
use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    process::Command,
};

const SERVICES: &[(&str, &str)] = &[
    ("DiagTrack", "Connected User Experiences and Telemetry"),
    ("dmwappushservice", "WAP push message routing (telemetry helper)"),
    ("MapsBroker", "downloaded maps"),
    ("RetailDemo", "retail demo mode"),
    ("WerSvc", "Windows Error Reporting"),
    ("Fax", "Fax"),
];

// (key, value, DWORD data, what it does)
const REGS: &[(&str, &str, u32, &str)] = &[
    (r"HKLM\SOFTWARE\Policies\Microsoft\Windows\DataCollection", "AllowTelemetry", 0, "telemetry to the minimum"),
    (r"HKLM\SOFTWARE\Policies\Microsoft\Windows\DataCollection", "DoNotShowFeedbackNotifications", 1, "no feedback pop-ups"),
    (r"HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsCopilot", "TurnOffWindowsCopilot", 1, "Copilot off"),
    (r"HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsAI", "DisableAIDataAnalysis", 1, "Recall: no snapshots of your screen"),
    (r"HKLM\SOFTWARE\Policies\Microsoft\Windows\CloudContent", "DisableWindowsConsumerFeatures", 1, "no suggested/promoted apps"),
    (r"HKLM\SOFTWARE\Policies\Microsoft\Windows\CloudContent", "DisableCloudOptimizedContent", 1, "no cloud-driven content"),
    (r"HKLM\SOFTWARE\Policies\Microsoft\Windows\CloudContent", "DisableSoftLanding", 1, "no Windows tips and tricks"),
    (r"HKLM\SOFTWARE\Policies\Microsoft\Windows\AdvertisingInfo", "DisabledByGroupPolicy", 1, "no advertising ID"),
    (r"HKLM\SOFTWARE\Policies\Microsoft\Windows\System", "EnableActivityFeed", 0, "no activity history (Timeline)"),
    (r"HKLM\SOFTWARE\Policies\Microsoft\Windows\System", "PublishUserActivities", 0, "activity history stays local"),
    (r"HKLM\SOFTWARE\Policies\Microsoft\Windows\GameDVR", "AllowGameDVR", 0, "Game Bar background capture off"),
    (r"HKLM\SOFTWARE\Policies\Microsoft\Edge", "StartupBoostEnabled", 0, "Edge does not preload at sign-in"),
    (r"HKLM\SOFTWARE\Policies\Microsoft\Edge", "BackgroundModeEnabled", 0, "Edge does not run after it is closed"),
    (r"HKCU\Software\Policies\Microsoft\Windows\Explorer", "DisableSearchBoxSuggestions", 1, "no web results in search"),
    (r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager", "SilentInstalledAppsEnabled", 0, "no apps installed behind your back"),
    (r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager", "PreInstalledAppsEnabled", 0, "no pre-installed promoted apps"),
    (r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager", "OemPreInstalledAppsEnabled", 0, "no OEM promoted apps"),
    (r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager", "SystemPaneSuggestionsEnabled", 0, "no ads in Start"),
    (r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager", "SubscribedContent-338388Value", 0, "no suggestions in Start"),
    (r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager", "SubscribedContent-338389Value", 0, "no tips and suggestions"),
    (r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager", "SubscribedContent-353694Value", 0, "no suggestions in Settings"),
    (r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager", "SubscribedContent-353696Value", 0, "no suggestions in Settings"),
    (r"HKCU\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager", "RotatingLockScreenOverlayEnabled", 0, "no ads on the lock screen"),
];

fn run(prog: &str, args: &[&str]) -> (bool, String) {
    // HKCU = the signed-in user's when the installer was elevated as another administrator (WINRUST_SID)
    let args: Vec<String> = if prog == "reg" { winrust_common::reg::user_args(args) } else { args.iter().map(|a| a.to_string()).collect() };
    match Command::new(prog).args(&args).output() {
        Ok(o) => (o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned()),
        Err(e) => (false, e.to_string()),
    }
}

/// Current start mode as `sc config` spells it. None = service missing, or boot/system driver (never touched).
fn start_mode(svc: &str) -> Option<&'static str> {
    let (ok, out) = run("sc.exe", &["qc", svc]);
    let line = out.lines().find(|l| ok && l.contains("START_TYPE"))?;
    Some(if line.contains("DISABLED") {
        "disabled"
    } else if line.contains("DEMAND_START") {
        "demand"
    } else if line.contains("AUTO_START") {
        if line.contains("DELAYED") { "delayed-auto" } else { "auto" }
    } else {
        return None;
    })
}

#[derive(Debug, PartialEq)]
enum Val {
    Missing,
    Dword(u32),
    Other, // a value of another type: not ours to overwrite
}

/// Reads `reg query /v` output.
fn parse_val(ok: bool, out: &str) -> Val {
    if !ok {
        return Val::Missing;
    }
    match winrust_common::reg::value(out) {
        Some(_) => winrust_common::reg::dword(out).map_or(Val::Other, Val::Dword),
        None => Val::Missing,
    }
}

fn reg_val(key: &str, value: &str) -> Val {
    let (ok, out) = run("reg.exe", &["query", key, "/v", value]);
    parse_val(ok, &out)
}

fn undo_path() -> PathBuf {
    PathBuf::from(env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into())).join("winrust-debloat").join("undo.txt")
}

fn plan() {
    println!("Dry run: nothing is changed. Add --apply (admin) to do it, --undo to revert.\n");
    for (svc, why) in SERVICES {
        match start_mode(svc) {
            None => println!("  service {svc:<18} not present"),
            Some("disabled") => println!("  service {svc:<18} already disabled"),
            Some(m) => println!("  service {svc:<18} {m} -> disabled   ({why})"),
        }
    }
    for (key, value, data, why) in REGS {
        let state = match reg_val(key, value) {
            Val::Dword(d) if d == *data => "already set".to_string(),
            Val::Dword(d) => format!("{d} -> {data}"),
            Val::Missing => format!("(none) -> {data}"),
            Val::Other => "not a DWORD, left alone".into(),
        };
        println!("  policy  {value:<34} {state}   ({why})");
    }
}

fn apply() -> Result<(), String> {
    let path = undo_path();
    let dir = path.parent().unwrap();
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e} (run as admin)", dir.display()))?;
    let mut log = OpenOptions::new().create(true).append(true).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut failed = 0;
    for (svc, _) in SERVICES {
        match start_mode(svc) {
            None => println!("skip   {svc}: not present"),
            Some("disabled") => println!("skip   {svc}: already disabled"),
            Some(mode) => {
                writeln!(log, "svc|{svc}|{mode}").map_err(|e| e.to_string())?; // log BEFORE changing
                let ok = run("sc.exe", &["config", svc, "start=", "disabled"]).0;
                let _ = run("sc.exe", &["stop", svc]);
                failed += !ok as u32;
                println!("{}  {svc}: {mode} -> disabled", if ok { "done  " } else { "FAILED" });
            }
        }
    }
    for (key, value, data, _) in REGS {
        let old = match reg_val(key, value) {
            Val::Dword(d) if d == *data => {
                println!("skip   {value}: already {data}");
                continue;
            }
            Val::Other => {
                println!("skip   {value}: not a DWORD, left alone");
                continue;
            }
            Val::Dword(d) => d.to_string(),
            Val::Missing => "-".into(),
        };
        writeln!(log, "reg|{key}|{value}|{old}").map_err(|e| e.to_string())?;
        let ok = run("reg.exe", &["add", key, "/v", value, "/t", "REG_DWORD", "/d", &data.to_string(), "/f"]).0;
        failed += !ok as u32;
        println!("{}  {value} = {data}", if ok { "done  " } else { "FAILED" });
    }
    println!("\nUndo information saved to {}. Some changes need a reboot or sign-out.", path.display());
    if failed > 0 { Err(format!("{failed} change(s) failed (run as administrator)")) } else { Ok(()) }
}

fn undo() -> Result<(), String> {
    let path = undo_path();
    let Ok(text) = fs::read_to_string(&path) else {
        println!("nothing to undo (no undo file)");
        return Ok(());
    };
    let mut failed = 0;
    // the first line per item holds the ORIGINAL value (an earlier --apply may have logged the same item again)
    for line in text.lines().rev() {
        let f: Vec<&str> = line.split('|').collect();
        let ok = match f.as_slice() {
            ["svc", name, mode] => run("sc.exe", &["config", name, "start=", mode]).0,
            ["reg", key, value, "-"] => run("reg.exe", &["delete", key, "/v", value, "/f"]).0 || reg_val(key, value) == Val::Missing, // never written (the change itself failed): nothing to remove
            ["reg", key, value, old] => run("reg.exe", &["add", key, "/v", value, "/t", "REG_DWORD", "/d", old, "/f"]).0,
            _ => continue,
        };
        failed += !ok as u32;
        println!("{}  {line}", if ok { "restored" } else { "FAILED  " });
    }
    if failed > 0 {
        return Err(format!("{failed} item(s) not restored; the undo file is kept at {}", path.display()));
    }
    fs::remove_file(&path).map_err(|e| e.to_string())?;
    println!("\nServices were restored to their old start mode (start them manually or reboot).");
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let r = if args.iter().any(|a| a == "--apply") {
        apply()
    } else if args.iter().any(|a| a == "--undo") {
        undo()
    } else {
        plan();
        Ok(())
    };
    if let Err(e) = r {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_reg_query_output() {
        let dw = "\r\nHKEY_LOCAL_MACHINE\\X\r\n    AllowTelemetry    REG_DWORD    0x1\r\n\r\n";
        assert_eq!(parse_val(true, dw), Val::Dword(1));
        assert_eq!(parse_val(true, "    V    REG_SZ    hello\r\n"), Val::Other);
        assert_eq!(parse_val(false, "ERROR: The system was unable to find the specified registry key or value."), Val::Missing);
    }

    #[test]
    fn every_registry_item_is_hkcu_or_hklm_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for (k, v, _, _) in REGS {
            assert!(k.starts_with("HKLM\\") || k.starts_with("HKCU\\"), "{k}");
            assert!(seen.insert((k, v)), "duplicate {k} {v}");
        }
    }
}
