# Rebuilds ONE plugin (and what ships with it), copies the exes to payload\ and runs `plugin-launcher update`: no launcher rebuild.
#   scripts\update-plugin.ps1 rust-search|rtaskmgr-win32|topbar|rust-explorer
# Build + copy need no admin. The update step does: from a normal shell this script asks for it via a UAC prompt (the user clicks Yes),
# then prints the last lines of %ProgramData%\plugin-launcher\install.log (the elevated window's own output is lost).
param([Parameter(Mandatory)][ValidateSet('rust-search', 'rtaskmgr-win32', 'topbar', 'rust-explorer', 'wiztree', 'photoview', 'debloat')][string]$name)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
function Build($dir) { Push-Location $dir; cargo build --release; if ($LASTEXITCODE) { throw "build failed: $dir" }; Pop-Location }
$p = "$root\payload"
if ($name -in 'rust-search', 'rtaskmgr-win32') { & "$PSScriptRoot\sync.ps1" | Out-Null } # these two are edited in their original folders on the desktop
switch ($name) {
    'rust-search' {
        Build "$root\plugins\rust-search"; $r = "$root\plugins\rust-search\target\release"
        Copy-Item "$r\rs.exe", "$r\rs-launcher.exe" $p -Force
    }
    'rtaskmgr-win32' {
        Build "$root\plugins\rtaskmgr-win32"; Build "$root\plugins\rs-du"
        Copy-Item "$root\plugins\rtaskmgr-win32\target\release\rtaskmgr-win32.exe" "$p\Task manager.exe" -Force
        Copy-Item "$root\plugins\rs-du\target\release\rs-du.exe" $p -Force
    }
    'topbar' { # reshell and rs-du are installed together with topbar
        Build "$root\plugins\topbar"; Build "$root\plugins\rs-du"; Build "$root\ReShell"
        Copy-Item "$root\plugins\topbar\target\release\topbar.exe" $p -Force
        Copy-Item "$root\plugins\rs-du\target\release\rs-du.exe" $p -Force
        Copy-Item "$root\ReShell\target\release\reshell.exe" $p -Force
    }
    { $_ -in 'wiztree', 'photoview', 'debloat' } { # stand-alone: one exe each (wiztree reads the MFT through rs-du\src\mft.rs)
        Build "$root\plugins\$name"
        Copy-Item "$root\plugins\$name\target\release\$name.exe" $p -Force
    }
    'rust-explorer' { # rs-shell --overlay starts topbar and the desktop at logon
        Build "$root\plugins\rust-explorer"; Build "$root\plugins\rs-shell"
        Copy-Item "$root\plugins\rust-explorer\target\release\rust-explorer.exe", "$root\plugins\rs-shell\target\release\rs-shell.exe" $p -Force
    }
}
$exe = "$root\target\release\plugin-launcher.exe"
if (-not (Test-Path $exe)) { throw "build the launcher once first: cargo build --release in $root" }
# Started through explorer.exe, never as our child: from a packaged app (the Claude desktop app is MSIX) every child process,
# the elevated launcher included, writes HKCU into the package's private copy of the registry, which the real session
# never sees (Run keys, ReplaceExplorer, folder verbs were silently lost that way). Explorer asks for admin (UAC) itself.
$log = "$env:ProgramData\plugin-launcher\install.log"
$before = if (Test-Path $log) { @(Get-Content $log -Encoding UTF8).Count } else { 0 }
$lnk = "$root\target\update-$name.lnk"
$s = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk); $s.TargetPath = $exe; $s.Arguments = "update $name"; $s.Save()
Start-Process explorer.exe -ArgumentList "`"$lnk`""
# explorer returns at once: wait for the launcher's verdict in the log (the user may take a while to answer UAC)
$deadline = (Get-Date).AddMinutes(5)
do {
    Start-Sleep 2
    $new = if (Test-Path $log) { @(Get-Content $log -Encoding UTF8 | Select-Object -Skip $before) } else { @() }
} until (($new -match "^\[\d+\] ${name}: ") -or (Get-Date) -gt $deadline)
Remove-Item $lnk -ErrorAction SilentlyContinue
if (-not ($new -match "^\[\d+\] ${name}: ")) { Write-Warning "no result in $log after 5 minutes (UAC declined?)" }
$new | Select-Object -Last 4
