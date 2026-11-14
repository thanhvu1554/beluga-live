# Chạy server Beluga Live (exe đã build) và in địa chỉ Tailscale để nhập vào app.
# Dùng:  .\scripts\start-server.ps1            chạy bản release (tự build nếu chưa có)
#        .\scripts\start-server.ps1 -Build     build lại trước khi chạy
#        .\scripts\start-server.ps1 -Mock      chạy với "Claude giả" (không tốn hạn mức)
param(
    [switch]$Build,
    [switch]$Mock,
    [int]$Port = 0
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$serverDir = Join-Path $root 'server'
$exe = Join-Path $serverDir 'target\release\beluga-server.exe'

if ($Build -or -not (Test-Path $exe)) {
    Write-Host 'Đang build bản release (lần đầu mất vài phút)...'
    Push-Location $serverDir
    try {
        cargo build --release
        if ($LASTEXITCODE -ne 0) { throw 'cargo build lỗi' }
    } finally {
        Pop-Location
    }
}

# Địa chỉ Tailscale của máy này (lấy lúc chạy, không lưu vào repo).
$tsExe = (Get-Command tailscale -ErrorAction SilentlyContinue).Source
if (-not $tsExe -and (Test-Path 'C:\Program Files\Tailscale\tailscale.exe')) {
    $tsExe = 'C:\Program Files\Tailscale\tailscale.exe'
}
$ip = $null
if ($tsExe) {
    try { $ip = (& $tsExe ip -4 | Select-Object -First 1) } catch { $ip = $null }
}

$shownPort = $Port
if ($shownPort -eq 0) { $shownPort = 8787 }
Write-Host ''
if ($ip) {
    Write-Host ("Địa chỉ cho app:  {0}:{1}" -f $ip, $shownPort)
} else {
    Write-Host 'Không lấy được địa chỉ Tailscale (tailscale ip -4). Tailscale đã chạy và đăng nhập chưa?'
}
Write-Host 'Mã ghép đôi 6 số sẽ hiện ngay bên dưới (lưu trong server\data\pair.json).'
Write-Host ''

$serverArgs = @()
if ($Mock) { $serverArgs += '--mock' }
if ($Port -ne 0) { $serverArgs += @('--port', "$Port") }

Set-Location $root
& $exe @serverArgs
exit $LASTEXITCODE
