# Keeps plugins\<name> identical to the original project folder and stamps what the payload exes were built from.
#   sync.ps1          copy originals -> plugins\ (originals win), then report
#   sync.ps1 -Check   only report; exit 1 on any mismatch
# A stamp = short SHA-256 over every source file (path + content), target\ and .git\ excluded.
#   sync.ps1 -Stamp   (used by build-all.ps1) record what the payload exes were just built from
param([switch]$Check, [switch]$Stamp)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$desk = Split-Path $root
$orig = @{ 'rust-search' = "$desk\rust-search"; 'rtaskmgr-win32' = "$desk\rtaskmgr-win32" }
$stampFile = "$root\payload\STAMPS.txt"

function Stamp($dir) {
    $parts = Get-ChildItem $dir -Recurse -File | Where-Object { $_.FullName -notmatch '\\(target|dist|backup-v1|\.git)\\' } | Sort-Object FullName |
        ForEach-Object { $_.FullName.Substring($dir.Length) + ':' + (Get-FileHash $_.FullName -Algorithm SHA256).Hash }
    $sha = [Security.Cryptography.SHA256]::Create()
    -join ($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($parts -join "`n")) | Select-Object -First 6 | ForEach-Object { $_.ToString('x2') })
}
function Built($name) {
    if (Test-Path $stampFile) { foreach ($l in Get-Content $stampFile) { $n, $s = $l -split ' '; if ($n -eq $name) { return $s } } }
    '-'
}

if (-not $Check) {
    foreach ($n in $orig.Keys) {
        robocopy $orig[$n] "$root\plugins\$n" /MIR /XD target dist backup-v1 .git /NFL /NDL /NJH /NJS /NP | Out-Null
        if ($LASTEXITCODE -ge 8) { throw "robocopy failed for $n ($LASTEXITCODE)" }
    }
}
if ($Stamp) { ($orig.Keys | ForEach-Object { "$_ $(Stamp "$root\plugins\$_")" }) | Set-Content $stampFile }
$bad = 0
foreach ($n in $orig.Keys) {
    $o = Stamp $orig[$n]; $c = Stamp "$root\plugins\$n"; $b = Built $n
    $state = if ($o -ne $c) { $bad++; 'COPY STALE: run scripts\sync.ps1' } elseif ($c -ne $b) { $bad++; 'PAYLOAD STALE: run scripts\build-all.ps1' } else { 'ok' }
    '{0,-15} original {1}  plugins\ {2}  payload built from {3}  {4}' -f $n, $o, $c, $b, $state
}
if ($Check -and $bad) { exit 1 }
