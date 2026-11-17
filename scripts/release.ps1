# Publishes a GitHub release with a fresh WinRust.exe.
#   release.ps1 v0.2.0 [-Notes "text"]
# Rebuilds everything (build-all.ps1), pushes pending changes, tags, uploads the exe.
param([Parameter(Mandatory)][string]$Version, [string]$Notes)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
Set-Location $root
if ($Version -notmatch '^v\d+\.\d+\.\d+$') { throw 'version must look like v0.2.0' }
& "$PSScriptRoot\build-all.ps1"
& "$PSScriptRoot\publish.ps1" -Message "Release $Version"
$exe = Join-Path $env:TEMP 'WinRust.exe'
Copy-Item "$root\target\release\plugin-launcher.exe" $exe -Force
git tag $Version
git push origin $Version
if ($Notes) { gh release create $Version $exe --repo Lemxy/WinRust --title "WinRust $Version" --notes $Notes }
else { gh release create $Version $exe --repo Lemxy/WinRust --title "WinRust $Version" --generate-notes }
