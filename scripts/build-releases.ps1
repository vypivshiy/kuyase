#Requires -Version 5.1
<#
.SYNOPSIS
    Сборка всех релизных артефактов KUYASE для GitHub Releases (Windows x64 GUI/headless + ARMv7 guest bridge + Linux x64 headless musl + Linux x64 GUI через WSL2 + SHA256SUMS.txt).
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

Write-Host "=== [1/6] Кросс-компиляция гостевого моста qemu-hw-bridge (armv7-unknown-linux-musleabihf) ===" -ForegroundColor Cyan
cargo build --release -p qemu-hw-bridge --target armv7-unknown-linux-musleabihf
if ($LASTEXITCODE -ne 0) { throw "Ошибка сборки qemu-hw-bridge" }

Copy-Item -Force "target/armv7-unknown-linux-musleabihf/release/qemu-hw-bridge" "guest-init/bin/qemu-hw-bridge"
Copy-Item -Force "target/armv7-unknown-linux-musleabihf/release/qemu-hw-bridge" "Releases/qemu-hw-bridge"

Write-Host "=== [2/6] Сборка десктопного приложения и CLI для Windows x64 (kuyase.exe) ===" -ForegroundColor Cyan
$WinBinPath = "target/release/kuyase.exe"
cargo build --release -p kuyase
if ($LASTEXITCODE -ne 0) {
    Write-Host "[kuyase] target/release/kuyase.exe занят запущенным процессом, собираем в target-win-release..." -ForegroundColor Yellow
    cargo build --release -p kuyase --target-dir target-win-release
    if ($LASTEXITCODE -ne 0) { throw "Ошибка сборки kuyase.exe" }
    $WinBinPath = "target-win-release/release/kuyase.exe"
}
Copy-Item -Force $WinBinPath "Releases/kuyase.exe"

Write-Host "=== [3/6] Сборка headless-раннера без GUI для Windows x64 (kuyase-headless.exe) и Linux x64 (x86_64-unknown-linux-musl) ===" -ForegroundColor Cyan
cargo build --release -p kuyase --no-default-features --target-dir target-headless
if ($LASTEXITCODE -ne 0) { throw "Ошибка сборки kuyase-headless.exe" }
Copy-Item -Force "target-headless/release/kuyase.exe" "Releases/kuyase-headless.exe"

$InstalledTargets = rustup target list --installed
if ($InstalledTargets -notcontains "x86_64-unknown-linux-musl") {
    rustup target add x86_64-unknown-linux-musl
    if ($LASTEXITCODE -ne 0) { throw "Не удалось установить rustup-таргет x86_64-unknown-linux-musl" }
}
cargo build --release -p kuyase --no-default-features --target x86_64-unknown-linux-musl
if ($LASTEXITCODE -ne 0) { throw "Ошибка кросс-компиляции kuyase-headless (x86_64-unknown-linux-musl)" }
Copy-Item -Force "target/x86_64-unknown-linux-musl/release/kuyase" "Releases/kuyase-headless"

if (Test-Path "kernel/kuyase-kernel-arm64.img") {
    Copy-Item -Force "kernel/kuyase-kernel-arm64.img" "Releases/kuyase-kernel-arm64.img"
} elseif (Test-Path "kernel/Image") {
    Copy-Item -Force "kernel/Image" "Releases/kuyase-kernel-arm64.img"
    Copy-Item -Force "kernel/Image" "kernel/kuyase-kernel-arm64.img"
} elseif (Test-Path "Releases/kuyase-kernel-arm64.img") {
    Copy-Item -Force "Releases/kuyase-kernel-arm64.img" "kernel/kuyase-kernel-arm64.img"
} elseif (Test-Path "Releases/Image") {
    Copy-Item -Force "Releases/Image" "Releases/kuyase-kernel-arm64.img"
    Copy-Item -Force "Releases/Image" "kernel/kuyase-kernel-arm64.img"
}
if (-not (Test-Path "Releases/kuyase-kernel-arm64.img")) {
    throw "Не найден образ гостевого ядра Releases/kuyase-kernel-arm64.img (или kernel/Image)"
}
Copy-Item -Force "Releases/kuyase-kernel-arm64.img" "kernel/Image"

Write-Host "=== [4/6] Упаковка Releases/kuyase-win64-portable.zip и Releases/kuyase-headless-win64.zip ===" -ForegroundColor Cyan
$StageWin = Join-Path ([System.IO.Path]::GetTempPath()) "kuyase-win64-stage-$PID"
$StageWinHeadless = Join-Path ([System.IO.Path]::GetTempPath()) "kuyase-headless-win64-stage-$PID"
if (Test-Path $StageWin) { Remove-Item -Recurse -Force $StageWin }
if (Test-Path $StageWinHeadless) { Remove-Item -Recurse -Force $StageWinHeadless }
New-Item -ItemType Directory -Force -Path $StageWin, (Join-Path $StageWin "scripts"), $StageWinHeadless | Out-Null

try {
    Copy-Item -Force "Releases/kuyase.exe" (Join-Path $StageWin "kuyase.exe")
    Copy-Item -Force "Releases/kuyase-kernel-arm64.img" (Join-Path $StageWin "kuyase-kernel-arm64.img")
    Copy-Item -Force "Releases/qemu-hw-bridge" (Join-Path $StageWin "qemu-hw-bridge")
    Copy-Item -Force "README.md" (Join-Path $StageWin "README.md")
    Copy-Item -Force "DUMP_GUIDE.md" (Join-Path $StageWin "DUMP_GUIDE.md")
    Copy-Item -Force "scripts/dump-from-adb.ps1", "scripts/dump-from-adb.sh", "scripts/sanitize-rootfs.ps1", "scripts/sanitize-rootfs.sh" (Join-Path $StageWin "scripts/")

    $WinZip = Join-Path $RepoRoot "Releases/kuyase-win64-portable.zip"
    if (Test-Path $WinZip) { Remove-Item -Force $WinZip }
    Compress-Archive -Path (Join-Path $StageWin "*") -DestinationPath $WinZip -Force

    Copy-Item -Force "Releases/kuyase-headless.exe" (Join-Path $StageWinHeadless "kuyase-headless.exe")
    Copy-Item -Force "Releases/kuyase-kernel-arm64.img" (Join-Path $StageWinHeadless "kuyase-kernel-arm64.img")
    Copy-Item -Force "Releases/qemu-hw-bridge" (Join-Path $StageWinHeadless "qemu-hw-bridge")

    $WinHeadlessZip = Join-Path $RepoRoot "Releases/kuyase-headless-win64.zip"
    if (Test-Path $WinHeadlessZip) { Remove-Item -Force $WinHeadlessZip }
    Compress-Archive -Path (Join-Path $StageWinHeadless "*") -DestinationPath $WinHeadlessZip -Force
} finally {
    if (Test-Path $StageWin) { Remove-Item -Recurse -Force $StageWin }
    if (Test-Path $StageWinHeadless) { Remove-Item -Recurse -Force $StageWinHeadless }
}

Write-Host "=== [5/6] Упаковка Releases/kuyase-headless-linux64.tar.gz ===" -ForegroundColor Cyan
@'
import pathlib, tarfile

releases = pathlib.Path("Releases")
out_tar = releases / "kuyase-headless-linux64.tar.gz"
items = [
    ("kuyase-headless", 0o755),
    ("kuyase-kernel-arm64.img", 0o644),
    ("qemu-hw-bridge", 0o755),
]
with tarfile.open(out_tar, "w:gz", format=tarfile.GNU_FORMAT) as tf:
    for name, mode in items:
        src = releases / name
        ti = tf.gettarinfo(str(src), arcname=name)
        ti.mode = mode
        ti.uid = 0
        ti.gid = 0
        ti.uname = "root"
        ti.gname = "root"
        with src.open("rb") as f:
            tf.addfile(ti, f)
'@ | python -
if ($LASTEXITCODE -ne 0) { throw "Ошибка упаковки Releases/kuyase-headless-linux64.tar.gz" }

if (-not $SkipLinux) {
    Write-Host "=== [6/6] Сборка полного Linux x86_64 GUI-бинарника и архива через WSL2 ===" -ForegroundColor Cyan
    $WslRepo = (wsl -e wslpath -u ($RepoRoot -replace '\\', '/')).Trim()
    wsl -e bash -lc "cd '$WslRepo' && SKIP_BRIDGE=1 SKIP_HEADLESS_LINUX=1 CARGO_TARGET_DIR=/tmp/kuyase-linux-target bash scripts/build-releases.sh"
    if ($LASTEXITCODE -ne 0) { throw "Ошибка сборки Linux-релиза в WSL2" }
} else {
    Write-Host "=== [6/6] Пропуск WSL2 GUI-сборки (-SkipLinux); статический kuyase-headless-linux64.tar.gz собран нативно ===" -ForegroundColor Yellow
}

$ReleaseAssets = @(
    "kuyase-win64-portable.zip",
    "kuyase-headless-win64.zip",
    "kuyase-linux64-portable.tar.gz",
    "kuyase-headless-linux64.tar.gz",
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
