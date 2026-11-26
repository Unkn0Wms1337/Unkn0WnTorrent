$ErrorActionPreference = "Stop"

Write-Host "Building Unkn0WnTorrent (release, single exe)..." -ForegroundColor Cyan

# cargo пишет прогресс в stderr — PowerShell считает это ошибкой, поэтому
# временно не останавливаемся на выводе нативной команды.
$ErrorActionPreference = "Continue"
& cargo build --release
$code = $LASTEXITCODE
$ErrorActionPreference = "Stop"

if ($code -ne 0) {
    throw "cargo build failed with exit code $code"
}

$src = Join-Path $PSScriptRoot "target\release\Unkn0WnTorrent.exe"
if (-not (Test-Path $src)) {
    throw "Build failed: $src not found"
}

$dist = Join-Path $PSScriptRoot "dist"
New-Item -ItemType Directory -Force -Path $dist | Out-Null
$dst = Join-Path $dist "Unkn0WnTorrent.exe"
Copy-Item -Path $src -Destination $dst -Force

$sizeMb = [math]::Round((Get-Item $dst).Length / 1MB, 1)
Write-Host "Done: $dst ($sizeMb MB)" -ForegroundColor Green
