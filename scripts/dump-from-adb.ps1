param(
    [Parameter(Mandatory = $true, Position = 0, HelpMessage = "IP-адрес колонки (например, 10.1.1.56 или 10.1.1.56:5555)")]
    [string]$Target,

    [Parameter(Mandatory = $false, Position = 1)]
    [string]$OutDir = ""
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot
if ((Split-Path -Leaf $PSScriptRoot) -eq "scripts" -and (Test-Path (Join-Path $RepoRoot "guest-init"))) {
    $DefaultBaseDir = Join-Path $RepoRoot "backups"
} else {
    $DefaultBaseDir = (Get-Location).Path
}
$AdbExe = ""
$AdbCandidates = @(
    (Join-Path $PSScriptRoot "adb.exe"),
    ".\adb.exe",
    $(if (Get-Command "adb.exe" -ErrorAction SilentlyContinue) { (Get-Command "adb.exe").Source } else { "" }),
    "$env:LOCALAPPDATA\Microsoft\WinGet\Links\adb.exe",
    "$env:LOCALAPPDATA\Android\Sdk\platform-tools\adb.exe",
    "$env:USERPROFILE\scoop\shims\adb.exe",
    "C:\platform-tools\adb.exe",
    "C:\adb\adb.exe"
)
foreach ($Cand in $AdbCandidates) {
    if ($Cand -and (Test-Path $Cand)) {
        $AdbExe = (Resolve-Path $Cand).Path
        break
    }
}
if (-not $AdbExe) {
    Write-Error "[dump] Утилита adb не найдена. Установите через: winget install Google.PlatformTools (или положите adb.exe рядом со скриптом)"
    exit 1
}

if (-not $Target.Contains(":")) {
    $Target = "${Target}:5555"
}

function Invoke-Adb {
    param([string[]]$ArgsList)
    & $AdbExe -s $Target @ArgsList
}

Write-Host "[1/3] Подключение к колонке по сети ($Target)..." -ForegroundColor Cyan
& $AdbExe connect $Target

$ModelRaw = (Invoke-Adb @("shell", "tr '\0' ' ' < /proc/device-tree/compatible 2>/dev/null; echo; cat /proc/device-tree/model 2>/dev/null")) -join " "
$DevTypeCfg = ((Invoke-Adb @("shell", 'sed -n "s/.*\"deviceType\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" /system/vendor/quasar/quasar.cfg 2>/dev/null | head -n 1')) -join "").Trim()
$Slug = "unknown"
if ($ModelRaw -match "prune") {
    $Slug = "plumnc"
} else {
    switch -Regex ($DevTypeCfg) {
        "^cucumber$" { $Slug = "cucumber"; break }
        "^(bergamot|mini3|yandexmini3|yandexmini_3)$" { $Slug = "bergamot"; break }
        "^(pickle|mini3pro)$" { $Slug = "pickle"; break }
        "^(orion|station3)$" { $Slug = "orion"; break }
        "^(plumnc|prune|lite2nc)$" { $Slug = "plumnc"; break }
        "^(plum|lite2)$" { $Slug = "plum"; break }
        "^(mango|street)$" { $Slug = "mango"; break }
        "^(saturn|hub)$" { $Slug = "saturn"; break }
        "^(station_2|station-midi|yandexmidi|station2)$" { $Slug = "station2"; break }
        "^(yandexmini_2_no_clock|yandexmini2nc|mini2nc)$" { $Slug = "yandexmini2nc"; break }
        "^(yandexmini_2|mini2|yandexmini2)$" { $Slug = "yandexmini2"; break }
        "^(yandexmicro|lite1|micro)$" { $Slug = "yandexmicro"; break }
        "^(yandexmini|mini1)$" { $Slug = "yandexmini"; break }
        "^.+$" { $Slug = $DevTypeCfg; break }
    }
}
if ($Slug -eq "unknown") {
    if ($ModelRaw -match "cucumber") { $Slug = "cucumber" }
    elseif ($ModelRaw -match "bergamot") { $Slug = "bergamot" }
    elseif ($ModelRaw -match "pickle") { $Slug = "pickle" }
    elseif ($ModelRaw -match "orion") { $Slug = "orion" }
    elseif ($ModelRaw -match "prune") { $Slug = "plumnc" }
    elseif ($ModelRaw -match "plum") { $Slug = "plum" }
    elseif ($ModelRaw -match "mango") { $Slug = "mango" }
    elseif ($ModelRaw -match "saturn") { $Slug = "saturn" }
    elseif ($ModelRaw -match "station-midi") { $Slug = "station2" }
    elseif ($ModelRaw -match "yandexmini2nc|mini2nc") { $Slug = "yandexmini2nc" }
    elseif ($ModelRaw -match "yandexmini_2|mini2") { $Slug = "yandexmini2" }
    elseif ($ModelRaw -match "yandexmicro") { $Slug = "yandexmicro" }
    elseif ($ModelRaw -match "axg") { $Slug = "yandexmini" }
}

if (-not $OutDir) {
    $Timestamp = Get-Date -Format "yyyyMMdd_HHmmss"
    $OutDir = Join-Path $DefaultBaseDir "dump-${Slug}-${Timestamp}"
}
New-Item -ItemType Directory -Force $OutDir | Out-Null
Write-Host " -> Определена плата : $Slug" -ForegroundColor Green
Write-Host " -> Папка сохранения : $OutDir" -ForegroundColor Green

Write-Host "[2/3] Сборка системного архива rootfs.tar.gz (без /data, /bkupgrade, /nv, /tee)..." -ForegroundColor Cyan
$ScratchCmd = @'
TMP_FREE=$(df -k /tmp 2>/dev/null | awk 'NR==2 {print $4}')
BK_FREE=$( [ -d /bkupgrade ] && df -k /bkupgrade 2>/dev/null | awk 'NR==2 {print $4}' || echo 0 )
DATA_FREE=$( [ -d /data ] && df -k /data 2>/dev/null | awk 'NR==2 {print $4}' || echo 0 )
ROOT_USED=$(df -k / 2>/dev/null | awk 'NR==2 {print $3}')
NEED_KB=$(( ${ROOT_USED:-80000} * 80 / 100 ))
[ "$NEED_KB" -lt 70000 ] && NEED_KB=70000

SCRATCH="/tmp/quasar_emu_rootfs.tar.gz"
if [ "${TMP_FREE:-0}" -ge "$NEED_KB" ]; then
  SCRATCH="/tmp/quasar_emu_rootfs.tar.gz"
elif [ "${BK_FREE:-0}" -ge "$NEED_KB" ] -a [ "${BK_FREE:-0}" -ge "${DATA_FREE:-0}" ]; then
  SCRATCH="/bkupgrade/quasar_emu_rootfs.tar.gz"
elif [ "${DATA_FREE:-0}" -ge "$NEED_KB" ]; then
  SCRATCH="/data/quasar_emu_rootfs.tar.gz"
elif [ "${BK_FREE:-0}" -gt "${TMP_FREE:-0}" ]; then
  SCRATCH="/bkupgrade/quasar_emu_rootfs.tar.gz"
elif [ "${DATA_FREE:-0}" -gt "${TMP_FREE:-0}" ]; then
  SCRATCH="/data/quasar_emu_rootfs.tar.gz"
fi
rm -f "$SCRATCH"
cd /
DIRS=""
for d in bin sbin lib lib32 lib64 usr etc system boot linuxrc; do
  [ -e "$d" ] && DIRS="$DIRS $d"
done
tar -cf - $DIRS 2>/dev/null | gzip -1 > "$SCRATCH"
echo "SCRATCH_PATH=$SCRATCH"
ls -lh "$SCRATCH"
'@
$ScratchOut = (Invoke-Adb @("shell", ($ScratchCmd -replace "`r`n", "`n"))) -join "`n"
Write-Host $ScratchOut
$ScratchPath = "/tmp/quasar_emu_rootfs.tar.gz"
if ($ScratchOut -match "SCRATCH_PATH=([^\r\n]+)") {
    $ScratchPath = $Matches[1].Trim()
}

Write-Host "[3/3] Скачивание rootfs.tar.gz по сети и очистка временного файла на колонке..." -ForegroundColor Cyan
Invoke-Adb @("pull", $ScratchPath, (Join-Path $OutDir "rootfs.tar.gz"))
Invoke-Adb @("shell", "rm -f $ScratchPath")

Write-Host "==================================================================" -ForegroundColor Green
Write-Host " Дамп успешно сохранён в: $OutDir" -ForegroundColor Green
Get-ChildItem $OutDir | ForEach-Object { Write-Host ("  - {0} ({1:N1} KB)" -f $_.Name, ($_.Length / 1KB)) }
Write-Host "==================================================================" -ForegroundColor Green
