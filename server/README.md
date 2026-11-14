# Beluga Live — server PC (Rust)

Cầu nối giữa app iPhone và Claude Code CLI trên máy này. Giao thức: `../docs/PROTOCOL.md`.

## Chạy

```powershell
cd server
cargo run -- --mock        # "Claude giả": phát kịch bản có sẵn, không tốn hạn mức
cargo run                  # Claude Code thật (cần `claude` đã cài và đăng nhập)
cargo test                 # 34 test đơn vị + 12 test tích hợp (WebSocket giả + Claude giả)
cargo build --release      # server\target\release\beluga-server.exe
..\scripts\start-server.ps1   # chạy bản release, in địa chỉ Tailscale; -Mock, -Build, -Port
```

- Cổng mặc định 8787 (`PORT` trong `.env` hoặc `--port`), nghe trên mọi interface.
- Lúc khởi động server in **mã ghép đôi 6 số** (lưu ở `data/pair.json`). App gửi mã đó lần đầu,
  nhận `pairToken`, các lần sau dùng token.
- `GET /health`, `GET /board/*` (phục vụ `../shared/board`), `ws://<pc>:8787/ws`.
- Dữ liệu phiên (danh sách, nhật ký sự kiện) ở `data/` (đã gitignore). Ảnh chụp ở `../tutor/photos/<sessionId>/`.
- Tự chạy khi đăng nhập Windows: `scripts\install-autostart.ps1` (gỡ: `uninstall-autostart.ps1`).

## ElevenLabs (tùy chọn)

Sao chép `.env.example` thành `.env` rồi điền `ELEVENLABS_API_KEY`. Không có key (hoặc key không dùng
được giọng thư viện, ví dụ gói Free trả HTTP 402) thì `ready.tts = "device"` và điện thoại tự đọc bằng
giọng hệ thống. `ELEVENLABS_STT_MODEL` để trống = điện thoại tự nhận dạng giọng nói. Chế độ `--mock`
không gọi ElevenLabs trừ khi đặt `BELUGA_VOICE_IN_MOCK=1`.

## Biến môi trường khác

`BELUGA_CLAUDE_BIN` (đường dẫn `claude`), `BELUGA_EFFORT` (low/medium/high... truyền cho `--effort`),
`BELUGA_MOCK_DELAY_MS`, `BELUGA_ROOT` (gốc repo), `RUST_LOG`.

## Thử với Claude thật (tốn hạn mức, mặc định bị bỏ qua)

```powershell
$env:BELUGA_TEST_JPEG = "C:\path\to\anh.jpg"
cargo test --test real_claude -- --ignored --nocapture
```
