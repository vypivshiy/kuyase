param(
    [Parameter(Position = 0)]
    [string]$Board = "",

    [ValidateSet("stub", "full")]
    [string]$Mode = "stub",

    [Parameter(HelpMessage = "Путь к папке дампа (например, backups\dump-yandexmini-20261029) или файлу rootfs.tar.gz / ubi0_0_rootfs.img / system.img")]
    [string]$DumpPath = "",

    [switch]$RebuildImage,

    [int]$BridgePort = 18091,
    [int]$SdkPort = 18090,
    [int]$AdbPort = 15555
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot
$KernelImg = Join-Path $RepoRoot "kernel\Image"
$WorkDir = Join-Path $RepoRoot "work"
$LastBoardFile = Join-Path $WorkDir ".last-board"

$QemuExe = ""
if (Get-Command "qemu-system-aarch64.exe" -ErrorAction SilentlyContinue) {
    $QemuExe = (Get-Command "qemu-system-aarch64.exe").Source
} elseif (Test-Path "C:\Program Files\qemu\qemu-system-aarch64.exe") {
    $QemuExe = "C:\Program Files\qemu\qemu-system-aarch64.exe"
}

if (-not (Test-Path $KernelImg)) {
    $ReleaseImg = Join-Path $RepoRoot "Releases\Image"
    if (Test-Path $ReleaseImg) {
        $KernelImg = $ReleaseImg
    }
}

if (-not (Test-Path $KernelImg)) {
    Write-Error "Ядро $KernelImg не найдено. Соберите ядро внутри WSL2 командой: ./kernel/build-kernel.sh"
}

$WslRepoRoot = ($RepoRoot -replace "\\", "/") -replace "^([A-Za-z]):", { "/mnt/" + $_.Groups[1].Value.ToLower() }
$WslPrepScript = "$WslRepoRoot/scripts/prepare-rootfs.sh"

New-Item -ItemType Directory -Force $WorkDir | Out-Null

if ($DumpPath) {
    if (-not (Test-Path $DumpPath)) {
        Write-Error "Указанный путь к дампу не найден: $DumpPath"
    }
    $ReqBoard = if ($Board) { $Board } else { "auto" }
    $ResolvedDump = (Resolve-Path $DumpPath).Path
    $WslDump = ($ResolvedDump -replace "\\", "/") -replace "^([A-Za-z]):", { "/mnt/" + $_.Groups[1].Value.ToLower() }
    Write-Host "[kuyase] Сборка образа .ext4 из дампа: $ResolvedDump ..." -ForegroundColor Cyan
    wsl -u root -e bash -c "sed -i 's/\r$//' '$WslPrepScript' && bash '$WslPrepScript' '$ReqBoard' '$WslDump'"
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
    if (-not $Board -and (Test-Path $LastBoardFile)) {
        $Board = (Get-Content $LastBoardFile -Raw).Trim()
    }
}

if (-not $Board) {
    if (Test-Path $LastBoardFile) {
        $Candidate = (Get-Content $LastBoardFile -Raw).Trim()
        if ($Candidate -and (Test-Path (Join-Path $WorkDir "$Candidate-system.ext4"))) {
            $Board = $Candidate
        }
    }
    if (-not $Board) {
        $ExistingImages = @(Get-ChildItem -Path $WorkDir -Filter "*-system.ext4" -File -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending)
        if ($ExistingImages.Count -gt 0) {
            $Board = $ExistingImages[0].Name -replace "-system\.ext4$", ""
        }
    }
}

if (-not $Board) {
    Write-Host "[kuyase] В папке work/ ещё нет собранных образов. Проверяем папку backups/..." -ForegroundColor Cyan
    wsl -u root -e bash -c "sed -i 's/\r$//' '$WslPrepScript' && bash '$WslPrepScript'"
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
    if (Test-Path $LastBoardFile) {
        $Board = (Get-Content $LastBoardFile -Raw).Trim()
    }
}

switch ($Board) {
    { $_ -in @("station-lite", "lite", "lite1", "micro") } { $Board = "yandexmicro"; break }
    { $_ -in @("mini", "mini1") } { $Board = "yandexmini"; break }
    { $_ -in @("mini2", "yandexmini_2") } { $Board = "yandexmini2"; break }
    "mini3" { $Board = "bergamot"; break }
    "mini3pro" { $Board = "pickle"; break }
    "lite2" { $Board = "plum"; break }
    "midi" { $Board = "cucumber"; break }
    "station3" { $Board = "orion"; break }
    { $_ -in @("station_2", "station-midi", "yandexmidi") } { $Board = "station2"; break }
    "street" { $Board = "mango"; break }
    "hub" { $Board = "saturn"; break }
}

$DriveImg = Join-Path $WorkDir "$Board-system.ext4"
$DriveTgz = Join-Path $WorkDir "$Board-system.tar.gz"
$OrigImg = Join-Path $WorkDir "$Board-system-orig.ext4"

if (-not (Test-Path $DriveImg) -and (Test-Path $DriveTgz)) {
    Write-Host "[kuyase] Распаковка готового образа из $DriveTgz..." -ForegroundColor Cyan
    & C:\Windows\System32\tar.exe -xzf $DriveTgz -C $WorkDir
}

if ($RebuildImage -and -not $DumpPath) {
    Write-Host "[kuyase] Пересборка образа $Board-system.ext4 через WSL..." -ForegroundColor Cyan
    wsl -u root -e bash -c "sed -i 's/\r$//' '$WslPrepScript' && bash '$WslPrepScript' '$Board'"
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}

if (-not (Test-Path $DriveImg)) {
    Write-Host "==================================================================" -ForegroundColor Yellow
    Write-Host " [kuyase] Образ '$DriveImg' не найден!" -ForegroundColor Yellow
    Write-Host " Дампы прошивок не поставляются в комплекте с репозиторием." -ForegroundColor Yellow
    Write-Host ""
    Write-Host " 1) Снимите дамп со своей колонки по сетевому ADB (см. DUMP_GUIDE.md):" -ForegroundColor Cyan
    Write-Host "    .\scripts\dump-from-adb.ps1 <IP_колонки>"
    Write-Host ""
    Write-Host " 2) Или укажите путь к самостоятельно снятому / скачанному на профильных" -ForegroundColor Cyan
    Write-Host "    ресурсах дампу (папка, rootfs.tar.gz, ubi0_0_rootfs.img, system.img):" -ForegroundColor Cyan
    Write-Host "    .\scripts\run-qemu.ps1 -DumpPath .\backups\my-speaker-dump"
    Write-Host "==================================================================" -ForegroundColor Yellow
    exit 1
}

$HostFwd = "hostfwd=tcp:127.0.0.1:${BridgePort}-:8091,hostfwd=tcp:127.0.0.1:${AdbPort}-:5555,hostfwd=tcp:127.0.0.1:${SdkPort}-:8090"

$CargoBridge = Join-Path $RepoRoot "hw-bridge\target\armv7-unknown-linux-musleabihf\release\qemu-hw-bridge"
$InitBridge = Join-Path $RepoRoot "guest-init\bin\qemu-hw-bridge"
$RelBridge = Join-Path $RepoRoot "Releases\qemu-hw-bridge"
$BridgeBin = ""
if (Test-Path $CargoBridge) {
    New-Item -ItemType Directory -Force (Split-Path -Parent $InitBridge) | Out-Null
    Copy-Item -Force $CargoBridge $InitBridge -ErrorAction SilentlyContinue
    $BridgeBin = $CargoBridge
} elseif (Test-Path $InitBridge) {
    $BridgeBin = $InitBridge
} elseif (Test-Path $RelBridge) {
    $BridgeBin = $RelBridge
}

Write-Host "==================================================================" -ForegroundColor Green
Write-Host " К.У.Я.С.Э. (KUYASE) — Кустарный Универсальный Яндекс Станции Эмулятор" -ForegroundColor Green
Write-Host " Board: $Board | maind_mode: $Mode | Image: work\$Board-system.ext4" -ForegroundColor Green
Write-Host " -> Virtual Speaker Twin (2D UI) : http://127.0.0.1:$BridgePort" -ForegroundColor Cyan
Write-Host " -> ADB Access                   : adb connect 127.0.0.1:$AdbPort" -ForegroundColor Cyan
Write-Host " -> Forwarded Guest SDK Port     : 127.0.0.1:$SdkPort -> guest :8090" -ForegroundColor Cyan
Write-Host " -> Stop Emulator                : Press Ctrl+C or Ctrl+D (or Enter for root shell)" -ForegroundColor Yellow
Write-Host "==================================================================" -ForegroundColor Green

if ($QemuExe) {
    $QemuArgs = @(
        "-M", "virt", "-cpu", "cortex-a53", "-smp", "2", "-m", "512M",
        "-kernel", $KernelImg,
        "-append", "console=ttyAMA0 root=/dev/vda rw ip=dhcp init=/quasar-qemu-init.sh quasar.board=$Board quasar.mode=$Mode panic=-1",
        "-drive", "file=$DriveImg,if=virtio,format=raw"
    )
    if ($BridgeBin) {
        $QemuArgs += @("-drive", "file=$BridgeBin,if=virtio,format=raw,readonly=on")
    }
    $QemuArgs += @(
        "-netdev", "user,id=net0,$HostFwd",
        "-device", "virtio-net-pci,netdev=net0",
        "-nographic"
    )
    & $QemuExe @QemuArgs
} else {
    Write-Host "[kuyase] Windows QEMU не найден в C:\Program Files\qemu — запускаем qemu-system-aarch64 внутри WSL2..." -ForegroundColor Yellow
    $WslKernel = "$WslRepoRoot/kernel/Image"
    $WslDrive = "$WslRepoRoot/work/$Board-system.ext4"
    $WslBridgeArgs = @()
    if ($BridgeBin) {
        $WslBridge = ($BridgeBin -replace "\\", "/") -replace "^([A-Za-z]):", { "/mnt/" + $_.Groups[1].Value.ToLower() }
        $WslBridgeArgs = @("-drive", "file=$WslBridge,if=virtio,format=raw,readonly=on")
    }
    wsl -e qemu-system-aarch64 `
        -M virt -cpu cortex-a53 -smp 2 -m 512M `
        -kernel "$WslKernel" `
        -append "console=ttyAMA0 root=/dev/vda rw ip=dhcp init=/quasar-qemu-init.sh quasar.board=$Board quasar.mode=$Mode panic=-1" `
        -drive "file=$WslDrive,if=virtio,format=raw" `
        @WslBridgeArgs `
        -netdev "user,id=net0,$HostFwd" `
        -device virtio-net-pci,netdev=net0 `
        -nographic
}
