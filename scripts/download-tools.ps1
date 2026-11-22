# Downloads ffmpeg, ffprobe, and fpcalc into tools/bin
$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$tools = Join-Path $root "tools"
$tmp = Join-Path $tools "_tmp"
$bin = Join-Path $tools "bin"
New-Item -ItemType Directory -Force -Path $tmp, $bin | Out-Null

$ffZip = Join-Path $tmp "ffmpeg.zip"
$fpZip = Join-Path $tmp "fpcalc.zip"

Write-Host "Downloading ffmpeg essentials..."
curl.exe -L --retry 3 --connect-timeout 30 --progress-bar -o $ffZip `
  "https://github.com/GyanD/codexffmpeg/releases/download/9.1/ffmpeg-9.1-essentials_build.zip"
if ($LASTEXITCODE -ne 0) { throw "ffmpeg download failed" }

Write-Host "Downloading chromaprint fpcalc..."
curl.exe -L --retry 3 --connect-timeout 30 --progress-bar -o $fpZip `
  "https://github.com/acoustid/chromaprint/releases/download/v1.6.2/chromaprint-fpcalc-1.6.2-windows-x86_64.zip"
if ($LASTEXITCODE -ne 0) { throw "fpcalc download failed" }

Expand-Archive -Path $ffZip -DestinationPath (Join-Path $tmp "ffmpeg") -Force
Expand-Archive -Path $fpZip -DestinationPath (Join-Path $tmp "chromaprint") -Force

$ffBin = Get-ChildItem (Join-Path $tmp "ffmpeg") -Recurse -Filter "ffmpeg.exe" | Select-Object -First 1
$probe = Get-ChildItem (Join-Path $tmp "ffmpeg") -Recurse -Filter "ffprobe.exe" | Select-Object -First 1
$fpcalc = Get-ChildItem (Join-Path $tmp "chromaprint") -Recurse -Filter "fpcalc.exe" | Select-Object -First 1

Copy-Item $ffBin.FullName (Join-Path $bin "ffmpeg.exe") -Force
Copy-Item $probe.FullName (Join-Path $bin "ffprobe.exe") -Force
Copy-Item $fpcalc.FullName (Join-Path $bin "fpcalc.exe") -Force
Get-ChildItem $fpcalc.Directory -Filter "*.dll" -ErrorAction SilentlyContinue | ForEach-Object {
  Copy-Item $_.FullName $bin -Force
}

Remove-Item -Recurse -Force $tmp
Write-Host "Installed to $bin"
Get-ChildItem $bin | Format-Table Name, Length -AutoSize
