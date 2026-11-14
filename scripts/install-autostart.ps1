# Tạo tác vụ Task Scheduler: tự chạy server Beluga Live khi đăng nhập Windows.
# KHÔNG chạy tự động; chủ máy tự chạy script này khi muốn bật.
# Gỡ bằng scripts\uninstall-autostart.ps1.
param(
    [string]$TaskName = 'BelugaLiveServer',
    [switch]$Visible   # hiện cửa sổ (để thấy mã ghép đôi); mặc định chạy ẩn
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$script = Join-Path $PSScriptRoot 'start-server.ps1'
$exe = Join-Path $root 'server\target\release\beluga-server.exe'

if (-not (Test-Path $exe)) {
    Write-Host 'Chưa có bản release. Build trước:  cd server; cargo build --release'
    exit 1
}

$window = '-WindowStyle Hidden'
if ($Visible) { $window = '' }
$arg = "-NoProfile $window -ExecutionPolicy Bypass -File `"$script`""

$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $arg -WorkingDirectory $root
$user = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -StartWhenAvailable `
    -RestartCount 3 `
    -RestartInterval (New-TimeSpan -Minutes 1) `
    -ExecutionTimeLimit ([TimeSpan]::Zero)
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited

Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger `
    -Settings $settings -Principal $principal -Force | Out-Null

Write-Host "Đã tạo tác vụ '$TaskName' (chạy khi đăng nhập). Mã ghép đôi nằm trong server\data\pair.json."
Write-Host "Chạy thử ngay:  Start-ScheduledTask -TaskName $TaskName"
