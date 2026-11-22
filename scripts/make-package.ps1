# Builds a runnable folder: release/dupfinder
# Requires: npm install already done, tools/bin populated (or run download-tools.ps1)
$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$out = Join-Path $root "release\dupfinder"

if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force -Path $out, (Join-Path $out "scripts") | Out-Null

Write-Host "Copying app files..."
Copy-Item (Join-Path $root "package.json") $out
Copy-Item (Join-Path $root "dupfinder.cmd") $out
Copy-Item (Join-Path $root "README.md") $out -ErrorAction SilentlyContinue
Copy-Item (Join-Path $root "src") (Join-Path $out "src") -Recurse
Copy-Item (Join-Path $root "public") (Join-Path $out "public") -Recurse
Copy-Item (Join-Path $root "scripts\download-tools.ps1") (Join-Path $out "scripts\download-tools.ps1") -Force

if (Test-Path (Join-Path $root "tools\bin")) {
  Write-Host "Copying tools/bin (ffmpeg, ffprobe, fpcalc)..."
  New-Item -ItemType Directory -Force -Path (Join-Path $out "tools\bin") | Out-Null
  Copy-Item (Join-Path $root "tools\bin\*") (Join-Path $out "tools\bin") -Force
} else {
  Write-Host "WARNING: tools/bin missing — run scripts/download-tools.ps1 inside the package."
}

Write-Host "Installing production node_modules..."
Push-Location $out
npm install --omit=dev
Pop-Location

@"
Duplicate Finder — portable package
===================================

Requires: Node.js 22.5+ on PATH

Run (each instance needs its own --data folder):

  dupfinder.cmd --data D:\dup-runs\batch-a
  dupfinder.cmd --data D:\dup-runs\batch-b --port 0

--port 0 picks any free port. If you omit --port and 3847 is busy,
the next free port is chosen automatically.

Data folder holds: catalog.sqlite, settings, trash/
App binaries (ffmpeg) stay in tools\bin and are shared by all instances.
"@ | Set-Content -Path (Join-Path $out "RUN.txt") -Encoding UTF8

Write-Host "Package ready: $out"
Get-ChildItem $out | Format-Table Name, Mode -AutoSize
