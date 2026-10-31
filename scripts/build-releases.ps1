#Requires -Version 5.1
<#
.SYNOPSIS
    Сборка всех релизных артефактов KUYASE для GitHub Releases (Windows x64 + ARMv7 guest bridge + Linux x64 через WSL2 + SHA256SUMS.txt).
.EXAMPLE
    pwsh -File scripts/build-releases.ps1
#>
param(
    [switch]$SkipLinux
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $RepoRoot

New-Item -ItemType Directory -Force -Path "guest-init/bin", "Releases", "kernel" | Out-Null

Write-Host "=== [1/4] Кросс-компиляция гостевого моста qemu-hw-bridge (armv7-unknown-linux-musleabihf) ===" -ForegroundColor Cyan
cargo build --release -p qemu-hw-bridge --target armv7-unknown-linux-musleabihf
if ($LASTEXITCODE -ne 0) { throw "Ошибка сборки qemu-hw-bridge" }

Copy-Item -Force "target/armv7-unknown-linux-musleabihf/release/qemu-hw-bridge" "guest-init/bin/qemu-hw-bridge"
Copy-Item -Force "target/armv7-unknown-linux-musleabihf/release/qemu-hw-bridge" "Releases/qemu-hw-bridge"

Write-Host "=== [2/4] Сборка десктопного приложения и CLI для Windows x64 (kuyase.exe) ===" -ForegroundColor Cyan
cargo build --release -p kuyase
if ($LASTEXITCODE -ne 0) { throw "Ошибка сборки kuyase.exe" }

Copy-Item -Force "target/release/kuyase.exe" "Releases/kuyase.exe"

if (Test-Path "kernel/kuyase-kernel-arm64.img") {
    Copy-Item -Force "kernel/kuyase-kernel-arm64.img" "Releases/kuyase-kernel-arm64.img"
} elseif (Test-Path "kernel/Image") {
    Copy-Item -Force "kernel/Image" "Releases/kuyase-kernel-arm64.img"
} elseif ((Test-Path "Releases/Image") -and -not (Test-Path "Releases/kuyase-kernel-arm64.img")) {
    Copy-Item -Force "Releases/Image" "Releases/kuyase-kernel-arm64.img"
}
if (-not (Test-Path "Releases/kuyase-kernel-arm64.img")) {
    throw "Не найден образ гостевого ядра Releases/kuyase-kernel-arm64.img (или kernel/Image)"
}
Copy-Item -Force "Releases/kuyase-kernel-arm64.img" "kernel/kuyase-kernel-arm64.img"
Copy-Item -Force "Releases/kuyase-kernel-arm64.img" "kernel/Image"

Write-Host "=== [3/4] Упаковка Releases/kuyase-win64-portable.zip ===" -ForegroundColor Cyan
$StageWin = Join-Path ([System.IO.Path]::GetTempPath()) "kuyase-win64-stage-$PID"
if (Test-Path $StageWin) { Remove-Item -Recurse -Force $StageWin }
New-Item -ItemType Directory -Force -Path $StageWin, (Join-Path $StageWin "examples"), (Join-Path $StageWin "scripts") | Out-Null

try {
    Copy-Item -Force "Releases/kuyase.exe" (Join-Path $StageWin "kuyase.exe")
    Copy-Item -Force "Releases/kuyase-kernel-arm64.img" (Join-Path $StageWin "kuyase-kernel-arm64.img")
    Copy-Item -Force "Releases/qemu-hw-bridge" (Join-Path $StageWin "qemu-hw-bridge")
    Copy-Item -Force "README.md" (Join-Path $StageWin "README.md")
    Copy-Item -Force "DUMP_GUIDE.md" (Join-Path $StageWin "DUMP_GUIDE.md")
    Copy-Item -Force "examples/README.md" (Join-Path $StageWin "examples/README.md")
    Copy-Item -Force "examples/test_yam_lineup.py" (Join-Path $StageWin "examples/test_yam_lineup.py")
    Copy-Item -Force "scripts/dump-from-adb.ps1", "scripts/dump-from-adb.sh", "scripts/sanitize-rootfs.ps1", "scripts/sanitize-rootfs.sh" (Join-Path $StageWin "scripts/")

    $WinZip = Join-Path $RepoRoot "Releases/kuyase-win64-portable.zip"
    if (Test-Path $WinZip) { Remove-Item -Force $WinZip }
    Compress-Archive -Path (Join-Path $StageWin "*") -DestinationPath $WinZip -Force
} finally {
    if (Test-Path $StageWin) { Remove-Item -Recurse -Force $StageWin }
}

if (-not $SkipLinux) {
    Write-Host "=== [4/4] Сборка Linux x86_64 бинарника и архива через WSL2 ===" -ForegroundColor Cyan
    $WslRepo = (wsl -e wslpath -u ($RepoRoot -replace '\\', '/')).Trim()
    wsl -e bash -lc "cd '$WslRepo' && SKIP_BRIDGE=1 CARGO_TARGET_DIR=/tmp/kuyase-linux-target bash scripts/build-releases.sh"
    if ($LASTEXITCODE -ne 0) { throw "Ошибка сборки Linux-релиза в WSL2" }
}

$ReleaseAssets = @(
    "kuyase-win64-portable.zip",
    "kuyase-linux64-portable.tar.gz",
    "kuyase-kernel-arm64.img"
)
$SumLines = @()
foreach ($fname in $ReleaseAssets) {
    $fpath = Join-Path $RepoRoot "Releases/$fname"
    if (Test-Path $fpath) {
        $hash = (Get-FileHash -Path $fpath -Algorithm SHA256).Hash.ToLowerInvariant()
        $SumLines += "$hash  $fname"
    }
}
$ShaFile = Join-Path $RepoRoot "Releases/SHA256SUMS.txt"
[System.IO.File]::WriteAllLines($ShaFile, $SumLines, (New-Object System.Text.UTF8Encoding($false)))

Write-Host "`n=== Готовые файлы для загрузки в GitHub Releases (Releases/): ===" -ForegroundColor Green
Get-ChildItem "Releases" | Where-Object { $_.Name -in ($ReleaseAssets + @("SHA256SUMS.txt")) } |
    Select-Object Name, @{Name="SizeMB";Expression={[math]::Round($_.Length / 1MB, 2)}}, LastWriteTime |
    Format-Table -AutoSize

Write-Host "=== SHA256SUMS.txt ===" -ForegroundColor Yellow
Get-Content $ShaFile
