# Syncs plugins\ from the original project folders, rebuilds every plugin, refreshes payload\ (+ STAMPS.txt),
# then rebuilds the launcher (one self-contained exe). Ends with a stamp check: any mismatch fails the script.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$sync = "$PSScriptRoot\sync.ps1"
& $sync | Out-Null
function Build($dir) { Push-Location "$root\plugins\$dir"; cargo build --release; if ($LASTEXITCODE) { throw "build failed: $dir" }; Pop-Location }
Build 'rust-search'
Build 'rtaskmgr-win32'
Build 'topbar'
Build 'rs-du'
Build 'rust-explorer'
Build 'rs-shell'
foreach ($t in 'wiztree', 'photoview', 'debloat') { Build $t; Copy-Item "$root\plugins\$t\target\release\$t.exe" "$root\payload" -Force }
Push-Location "$root\ReShell"; cargo build --release; if ($LASTEXITCODE) { throw 'build failed: ReShell' }; Pop-Location
Copy-Item "$root\ReShell\target\release\reshell.exe" "$root\payload" -Force
Copy-Item "$root\plugins\topbar\target\release\topbar.exe" "$root\payload" -Force
Copy-Item "$root\plugins\rs-du\target\release\rs-du.exe" "$root\payload" -Force
Copy-Item "$root\plugins\rust-explorer\target\release\rust-explorer.exe", "$root\plugins\rs-shell\target\release\rs-shell.exe" "$root\payload" -Force
$rs = "$root\plugins\rust-search\target\release"
Copy-Item "$rs\rs.exe", "$rs\rs-launcher.exe" "$root\payload" -Force
Copy-Item "$root\plugins\rtaskmgr-win32\target\release\rtaskmgr-win32.exe" "$root\payload\Task manager.exe" -Force
& $sync -Stamp | Out-Null
Push-Location $root; cargo build --release; if ($LASTEXITCODE) { throw 'launcher build failed' }; Pop-Location
& $sync -Check
if ($LASTEXITCODE) { throw 'stamp mismatch' }
Write-Host "done: $root\target\release\plugin-launcher.exe"
