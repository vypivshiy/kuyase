<#
.SYNOPSIS
    Детектор и очиститель чувствительных данных (ID устройства, Wi-Fi/сеть, токены,
    SSH-ключи и сторонние рут-киты) в дампах прошивок и образах .ext4 (через WSL2).

.EXAMPLE
    .\scripts\sanitize-rootfs.ps1 -Check
    .\scripts\sanitize-rootfs.ps1 -Clean
    .\scripts\sanitize-rootfs.ps1 -Check backups\yandexmini_2026-10-30
#>
param(
    [Parameter(Position = 0)]
    [string]$Target = "",
    [switch]$Check,
    [switch]$Clean
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$WslRepoRoot = ($RepoRoot -replace "\\", "/") -replace "^([A-Za-z]):", { "/mnt/" + $_.Groups[1].Value.ToLower() }

$ModeFlag = "--clean"
if ($Check -and -not $Clean) {
    $ModeFlag = "--check"
}

$WslTarget = ""
if ($Target) {
    $Resolved = Resolve-Path $Target -ErrorAction SilentlyContinue
    if ($Resolved) {
        $WslTarget = ($Resolved.Path -replace "\\", "/") -replace "^([A-Za-z]):", { "/mnt/" + $_.Groups[1].Value.ToLower() }
    } else {
        $WslTarget = ($Target -replace "\\", "/")
    }
}

$Cmd = "cd '$WslRepoRoot' && sed -i 's/\r$//' scripts/sanitize-rootfs.sh && bash ./scripts/sanitize-rootfs.sh $ModeFlag"
if ($WslTarget) {
    $Cmd += " '$WslTarget'"
}

wsl -u root -e bash -c $Cmd
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
