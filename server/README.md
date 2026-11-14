# Beluga Live — server PC (Rust)

Cầu nối giữa app iPhone và Claude Code CLI trên máy này. Giao thức: `../docs/PROTOCOL.md`.

## Chạy

```powershell
cd server
cargo run -- --mock        # "Claude giả": phát kịch bản có sẵn, không tốn hạn mức
cargo run                  # Claude Code thật (cần `claude` đã cài và đăng nhập)
cargo test                 # 41 test đơn vị + 34 test tích hợp (WebSocket giả, Claude giả, GenVoice giả)
cargo build --release      # server\target\release\beluga-server.exe
..\scripts\start-server.ps1   # chạy bản release, in địa chỉ Tailscale; -Mock, -Build, -Port
```

- Cổng mặc định 8787 (`PORT` trong `.env` hoặc `--port`), nghe trên mọi interface.
- Lúc khởi động server in **mã ghép đôi 6 số** (lưu ở `data/pair.json`). App gửi mã đó lần đầu,
  nhận `pairToken`, các lần sau dùng token.
- `GET /health`, `GET /board/*` (phục vụ `../shared/board`), `ws://<pc>:8787/ws`.
- Dữ liệu phiên (danh sách, nhật ký sự kiện) ở `data/` (đã gitignore). Ảnh chụp ở `../tutor/photos/<sessionId>/`.
- Tự chạy khi đăng nhập Windows: `scripts\install-autostart.ps1` (gỡ: `uninstall-autostart.ps1`).

## Giọng nói (TTS / STT)

Chọn nhà cung cấp trong `server/.env` (xem `.env.example`; file `.env` không bao giờ được commit hay in ra):

| Biến | Giá trị |
|---|---|
| `TTS_PROVIDER`, `STT_PROVIDER` | `auto` (mặc định) \| `genvoice` \| `elevenlabs` \| `device` |
| `GENVOICE_API_KEY`, `GENVOICE_VOICE_ID`, `GENVOICE_TTS_MODEL` | GenVoice (ưu tiên số 1) |
| `ELEVENLABS_API_KEY`, `ELEVENLABS_VOICE_ID`, `ELEVENLABS_TTS_MODEL`, `ELEVENLABS_STT_MODEL` | ElevenLabs trực tiếp (số 2) |

`auto`: có `GENVOICE_API_KEY` thì GenVoice, không thì có key ElevenLabs thì ElevenLabs, không thì giọng của
điện thoại (`ready.tts = "device"`). Giọng mặc định "Chi" (`5yEPTrA0uAhZsBEhHjfi`), `language_code` theo
`settings.lang` của điện thoại.

- **TTS:** mỗi câu `say` được tạo tiếng ngay khi parser phát ra (song song tối đa 4 câu), frame `audio` gửi ngay khi xong,
  kể cả trước lượt phát. Mỗi câu có hạn `8000 ms + 60 ms x số ký tự` (đổi bằng `BELUGA_TTS_BASE_MS`,
  `BELUGA_TTS_PER_CHAR_MS`); quá hạn hoặc lỗi thì server gửi `audio_skip{seq}` và hủy task. Ngắt lời hủy các task đang chờ.
  Lỗi 401/402, hoặc 3 câu lỗi liên tiếp: `notice{tts_fallback}` một lần rồi các câu sau `audio:false`.
- **STT:** các frame nhị phân giữa `audio_start` và `audio_end` ghép thành WAV (PCM16 mono nếu chưa có header RIFF),
  gửi lên nhà cung cấp, kết quả thành `heard` rồi một lượt như `user_text`. Hạn `8000 ms + 1500 ms x số giây âm thanh`
  (`BELUGA_STT_BASE_MS`, `BELUGA_STT_PER_SEC_MS`). Chữ rỗng: `notice{stt_empty}`; lỗi/quá hạn: `notice{stt_failed}`.
- Khởi động: GenVoice được kiểm tra bằng `GET /v1/auth/me` (không tốn credit, log số dư, cảnh báo dưới 2000).
- Chế độ `--mock` không gọi nhà cung cấp giọng nói nào trừ khi đặt `BELUGA_VOICE_IN_MOCK=1`.

## MCP chung: cho phiên Claude Code bất kỳ nói chuyện với iPhone

Ngoài route riêng của phiên gia sư (`/mcp/<sessionId>`, chỉ có `capture_photo`), server có `POST /mcp`
(chỉ nhận từ localhost) nhắm vào điện thoại kết nối mới nhất còn sống. Tools:

| Tool | Việc |
|---|---|
| `phone_status` | đã kết nối chưa, sessionId, tên phiên, trạng thái (`idle`/`busy`/`listening`), tts/stt đang dùng |
| `capture_photo` | chụp ảnh bằng camera điện thoại, trả image content |
| `speak {text}` | đọc một hay nhiều câu (lỗi "đang bận" nếu gia sư đang giảng) |
| `present {lines}` | văn bản theo định dạng dòng bài giảng (lời nói + `@step/@show/@ask/@done`), trả số sự kiện và các dòng bị bỏ |
| `listen {timeout_s?}` | chờ câu nói kế tiếp (mặc định 30 giây, tối đa 120); câu đó không chuyển cho gia sư |
| `interrupt` | dừng lượt gia sư đang chạy, hoặc cắt phần đã xếp hàng do `speak`/`present` |

Đăng ký vào Claude Code của bạn (một lần; script chỉ chạy khi bạn chạy nó):

```powershell
.\scripts\register-mcp.ps1            # = claude mcp add --transport http --scope user beluga http://127.0.0.1:8787/mcp
.\scripts\register-mcp.ps1 -DryRun    # chỉ in lệnh
claude mcp remove --scope user beluga # gỡ
```

Server phải đang chạy và app đã kết nối thì các tool mới dùng được; không có điện thoại thì tool trả lỗi dạng chữ.

## Claude Code (gia sư)

`--model` theo cài đặt của app (mặc định sonnet), `--effort`: sonnet `medium`, opus `low`, haiku không truyền;
`BELUGA_EFFORT` ghi đè cho mọi model. Khác: `BELUGA_CLAUDE_BIN` (đường dẫn `claude`), `BELUGA_MOCK_DELAY_MS`,
`BELUGA_ROOT` (gốc repo), `RUST_LOG`.

## Thử với dịch vụ thật (tốn hạn mức/credit, mặc định bị bỏ qua)

```powershell
$env:BELUGA_TEST_JPEG = "C:\path\to\anh.jpg"
cargo test --test real_claude -- --ignored --nocapture   # Claude Code thật (haiku)
cargo test --test real_voice  -- --ignored --nocapture   # GenVoice thật + Claude giả, đo độ trễ say -> audio
```
