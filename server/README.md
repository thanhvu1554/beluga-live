# Beluga Live — server PC (Rust)

Cầu nối giữa app iPhone và Claude Code CLI trên máy này. Giao thức: `../docs/PROTOCOL.md`.

## Chạy

```powershell
cd server
cargo run -- --mock        # "Claude giả": phát kịch bản có sẵn, không tốn hạn mức
cargo run                  # Claude Code thật (cần `claude` đã cài và đăng nhập)
cargo test                 # 58 test đơn vị + 48 test tích hợp (WebSocket giả, Claude giả, GenVoice giả, memory, skill)
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

## Trí nhớ, ôn lại, tiến độ, môn học

Hợp đồng ở `../docs/PROTOCOL.md` mục 6. Phần server:

- **File memory** (`../tutor/memory/`, đã gitignore, mở đọc và sửa tay được): `profile.md` (cách giảng hợp) và `<môn>.md`
  (`toan`, `ly`, `hoa`, `van`, `anh-ssat`, `duc`, `tin-cpp`, `chung`). Mỗi mục một dòng theo khuôn, có tiêu đề nhóm
  (Đang yếu, Đã vững, Đã học, Cách giảng hợp):

  ```
  - [yếu] Xét dấu tam thức khi a âm | sai 3 | đúng 1 | bậc 1 | lần cuối 2026-11-14 | ôn 2026-11-17 | hay quên đổi chiều
  - [vững] Giải phương trình bậc hai | ngày 2026-11-20 | ghi chú
  - [học] Bất phương trình bậc hai | ngày 2026-11-14 | ghi chú
  - [cách] Hiểu nhanh hơn khi có ví dụ số | môn toan | ngày 2026-11-14
  ```

  Dòng không theo khuôn (chữ tự viết, dòng hỏng) được giữ nguyên khi server ghi lại. Ghi file kiểu atomic (ghi tạm rồi đổi tên).
  Chủ đề trùng (không phân biệt hoa thường, bỏ dấu cách thừa) thì cập nhật mục cũ.
- **Tool `memory_note`** trên route MCP theo phiên (`/mcp/<sessionId>`, cạnh `capture_photo`; `--allowedTools` có
  `mcp__beluga__memory_note`). `subject` được chuẩn hóa về id môn ("Toán", "vật lý", "C++"... ; môn lạ như Sinh, Sử thành `chung`).
- **Lịch ôn** (`src/memory.rs`, hàm thuần nhận ngày hiện tại): sai thì ôn lại sau 1 ngày; đúng một lần ôn thì lên một bậc
  (3, 7, 14 ngày, bậc giữ nguyên qua lần sai); đúng hai lần ôn liên tiếp thì chuyển sang "Đã vững"; sai lại thì mở lại thành "Đang yếu".
- **Bản tóm tắt `[memory]`** (tối đa 12 dòng: cách giảng hợp, chỗ yếu, bài học gần đây) đính vào lượt đầu của mỗi phiên;
  cờ `memory_sent` trong `data/sessions.json` giữ cho nó không đính lại sau khi khởi động lại hay `--resume`.
- **Mời ôn đầu phiên**: phiên mới (chưa có sự kiện nào) mà có mục tới hạn thì server tự phát `say` + `step` + `show review_offer`
  + `turn_end` ngay sau `ready`, không gọi Claude (có tiếng như mọi `say`; theo `settings.lang`; chọn mục trễ hạn lâu nhất; mỗi phiên
  một lần). Lượt người dùng kế tiếp kèm `[ôn lại: <môn> · <chủ đề> · <ghi chú>]` (còn nhớ sau khi khởi động lại).
  Phiên chỉ có lời mời chưa vào danh sách phiên cho tới khi người dùng nói.
- **`get_progress`** (WS) trả `progress` (môn có dữ liệu, mục yếu xếp theo hạn ôn, đã vững, đã học gần đây, cách giảng, `updatedAt`).
- **Môn học**: skill ở `../tutor/.claude/skills/<id>/SKILL.md`, nạp với `--setting-sources project`. `--allowedTools` chỉ cho
  `Skill(toan)`, `Skill(ly)`... (tám skill này), không cho `Skill` chung. `tests/skills.rs` kiểm tra khuôn file và mọi ví dụ `@lệnh`
  trong skill đều qua được parser thật.

## Claude Code (gia sư)

`--model` theo cài đặt của app (mặc định sonnet), `--effort`: sonnet `medium`, opus `low`, haiku không truyền;
`BELUGA_EFFORT` ghi đè cho mọi model. `BELUGA_MODEL_MAP` đổi hướng tạm một mức trong app sang model khác, dạng
`opus=ds/deepseek-flash` (nhiều cặp cách bằng dấu phẩy); model không phải sonnet/opus/haiku được coi là model
của gateway nên server **giữ** `ANTHROPIC_AUTH_TOKEN`/`ANTHROPIC_API_KEY` cho `claude` (model built-in thì vẫn xóa
để dùng đăng nhập Pro). Khác: `BELUGA_CLAUDE_BIN` (đường dẫn `claude`), `BELUGA_MOCK_DELAY_MS`,
`BELUGA_ROOT` (gốc repo), `RUST_LOG`.

## Thử với dịch vụ thật (tốn hạn mức/credit, mặc định bị bỏ qua)

Một bài đầu-cuối "điện thoại giả" (Claude thật, GenVoice thật, tối đa hai lượt học sinh; dừng ngay khi gặp `rate_limited`):

```powershell
# 1) server thật, tách khỏi dữ liệu thật: BELUGA_ROOT trỏ tới thư mục có tutor\ (bản sao CLAUDE.md và .claude) để memory không lẫn
#    (GENVOICE_API_KEY lấy từ môi trường hoặc từ server\.env của BELUGA_ROOT)
$env:BELUGA_ROOT = "$env:TEMP\beluga-e2e"; .\target\release\beluga-server.exe --port 8799 --data "$env:TEMP\beluga-e2e\data"
# 2) điện thoại giả; ghi transcript.txt, events.jsonl và các mp3 vào --out
cargo run --example fake_phone -- --addr 127.0.0.1:8799 --code <mã ghép đôi> --jpeg de.jpg --out out
#    --wait-offer --first "Ừ, ôn đi" --second "..." : để server mở đầu bằng lời mời ôn rồi mới trả lời
```

```powershell
$env:BELUGA_TEST_JPEG = "C:\path\to\anh.jpg"
cargo test --test real_claude -- --ignored --nocapture   # Claude Code thật (haiku)
cargo test --test real_voice  -- --ignored --nocapture   # GenVoice thật + Claude giả, đo độ trễ say -> audio
                                                            # (BELUGA_SAVE_AUDIO=<thư mục> giữ mp3 và in kích thước, bitrate, thời lượng, hash)
```
