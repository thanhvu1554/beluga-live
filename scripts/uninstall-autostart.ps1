# Gỡ tác vụ tự chạy server Beluga Live (và dừng server nếu đang chạy do tác vụ đó).
param(
    [string]$TaskName = 'BelugaLiveServer'
)

$task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if (-not $task) {
    Write-Host "Không có tác vụ '$TaskName'."
    exit 0
}
Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
Write-Host "Đã gỡ tác vụ '$TaskName'."
