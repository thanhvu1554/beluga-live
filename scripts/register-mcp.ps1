# Đăng ký MCP "beluga" vào Claude Code của bạn (phạm vi user) để MỌI phiên Claude Code trên máy này
# có thể nói chuyện với iPhone qua server: phone_status, capture_photo, speak, present, listen, interrupt.
# Script này KHÔNG tự chạy; bạn chạy khi muốn bật. Server (start-server.ps1) phải đang chạy để tool dùng được.
# Gỡ:  claude mcp remove --scope user beluga
param(
    [int]$Port = 8787,
    [string]$Name = 'beluga',
    [switch]$DryRun    # chỉ in lệnh, không chạy
)

$ErrorActionPreference = 'Stop'
$url = "http://127.0.0.1:$Port/mcp"
$cmdArgs = @('mcp', 'add', '--transport', 'http', '--scope', 'user', $Name, $url)

Write-Host ('Lệnh: claude ' + ($cmdArgs -join ' '))
if ($DryRun) { exit 0 }

if (-not (Get-Command claude -ErrorAction SilentlyContinue)) {
    Write-Host 'Không tìm thấy lệnh claude trong PATH.'
    exit 1
}

& claude @cmdArgs
if ($LASTEXITCODE -ne 0) {
    Write-Host 'Đăng ký lỗi (nếu đã có sẵn thì gỡ trước: claude mcp remove --scope user beluga).'
    exit $LASTEXITCODE
}
Write-Host ''
Write-Host 'Xong. Mở phiên Claude Code mới rồi gõ /mcp để thấy "beluga". Chỉ nhận kết nối từ chính máy này.'
