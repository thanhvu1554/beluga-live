# NOTES — quyết định và tiến độ

## Tiến độ
(cập nhật 14/11/2026, 18:15)

- [x] 1. Khung app, giao diện lofi trắng đen (sáng/tối), mock, CI ra IPA unsigned.
      CI xanh (run 43572866046): test BelugaCore, IPA unsigned, 26 screenshot simulator.
- [x] 2. Server PC (Rust) và luồng giọng nói, trình chiếu bài giảng từng bước.
      Server đã chạy với Claude Code thật và GenVoice thật; app mới chạy ở chế độ mock trên simulator.
- [x] 3. Điểm dừng "vặn lại": có ở server (parser, lượt dừng sau `@ask`) và ở app (thẻ "Đến lượt bạn").
- [ ] **Chưa làm lần nào: app thật nói chuyện với server thật trên iPhone.** Đây là việc kế tiếp.
- [ ] Để sau: memory, ôn lại, màn hình tiến độ, module môn học (`tutor/.claude/skills/`).

## iOS (`ios/`, `shared/board/`)

Agent làm phần này dừng giữa chừng vì chạm giới hạn 5 giờ của gói Pro, nên mục này do người điều phối
viết từ những gì kiểm chứng được, không phải báo cáo đầy đủ của agent.

**Đã kiểm chứng**
- CI xanh trên runner `xcode-27`: `swift test` cho BelugaCore, build simulator, build IPA unsigned.
- IPA: chỉ có `Payload/BelugaLive.app`, có `Info.plist`, không có `_CodeSignature` hay provisioning
  profile, có kèm gói bảng (34 file).
- Screenshot đã tự xem (4 trong 26): bài giảng sáng và tối, điểm dừng "vặn lại", cài đặt. Camera
  phía trên, bảng trượt lên khoảng 60%, đồ thị, bảng xét dấu và công thức KaTeX vẽ đúng, điều khiển
  dùng Liquid Glass, không có màu nhấn.

**Chưa kiểm chứng**
- Mọi thứ cần thiết bị thật: camera, micro, nhận dạng giọng nói, phát tiếng, khử vọng, ngắt lời,
  rảnh tay, âm thanh trạng thái, rung, kết nối WebSocket qua Tailscale, ghép đôi, tự nối lại.
- Đường nghe qua server (gửi WAV) và phát mp3 từ server có mã nhưng chưa chạy với server thật.
- 22 screenshot còn lại chưa ai xem kỹ; icon app chưa được xem lại.

**Lỗi giao diện đã thấy, chưa sửa**
- Khi bật phụ đề, khung phụ đề đè lên phần cuối của bảng (che một hàng của bảng xét dấu).
- Nhãn điểm trên đồ thị ("2", "3") trùng với số trên trục và chạm vào đường cong.

## Quyết định đã chọn

- **Bố cục repo:** `ios/` (app), `server/` (Rust trên PC), `shared/board/` (bộ vẽ bảng, dùng chung),
  `tutor/` (thư mục làm việc của Claude Code: prompt gia sư, sau này là skill từng môn và memory),
  `docs/PROTOCOL.md` (hợp đồng giữa các phần).
- **Runner:** GitHub có image Xcode 27 (`xcode-27-arm64`, macOS 27, simulator iOS 27.x). App đặt
  tối thiểu iOS 26.
- **Bài giảng là văn bản theo dòng** (lời nói / `@lệnh {json}`), không dùng tool cho từng bước:
  nhanh hơn vì không tốn một vòng gọi tool cho mỗi khối. Chỉ chụp ảnh mới là tool.
- **Đồng bộ lời nói với bảng** làm ở điện thoại bằng timeline theo `seq`.
- **Bảng là gói web trong WKWebView** (KaTeX + mhchem), server cũng phục vụ gói này, nên thêm kiểu
  khối mới không phải build lại app.
- **Giọng nói:** GenVoice (bên thứ ba bán lại ElevenLabs, API bất đồng bộ) là lựa chọn số 1 vì gói
  Free của ElevenLabs không cho gọi giọng thư viện qua API (HTTP 402). Thứ tự: GenVoice, ElevenLabs
  trực tiếp, cuối cùng là giọng và nhận dạng của iOS. Giọng mặc định: "Chi" (nữ trẻ, giọng Bắc).
- **Địa chỉ PC mặc định không nằm trong repo public:** đưa vào app lúc build qua secret
  `BELUGA_DEFAULT_HOST` của GitHub Actions; sửa được trong cài đặt app.
- **Claude Code không có quyền ghi:** chạy với danh sách tool tối thiểu, cwd là `tutor/`.
- **OpenCode worker:** chưa dùng ở đợt này; ba ưu tiên đầu đều là việc thiết kế và tích hợp, không
  có phần lặp lại đủ lớn để đáng viết brief.

## Server (Rust, `server/`)

**Đã xong và đã kiểm chứng**
- Parser dòng bài giảng streaming (`src/parser.rs`): đủ quy tắc PROTOCOL mục 2, 19 test (mảnh cắt giữa dòng/JSON/UTF-8, CRLF, LaTeX có dấu gạch chéo ngược kép, `@show` trước `@step`, dòng `@` hỏng, bỏ sau `@ask`, cắt câu >220).
- Server axum: `/health`, `/board/*`, `/ws`, ghép đôi (mã 6 số lưu `server/data/pair.json`, đổi lấy `pairToken`), phiên + nhật ký sự kiện (`server/data/sessions/<id>.jsonl`), `snapshot` khi nối lại bằng `lastSeq`, `open_session`/`list_sessions`, `interrupt`, `played`, `capture_request`/`photo`, TTS/STT sau interface (có bản giả).
- Chế độ `--mock`: Claude giả phát kịch bản theo mảnh 9 ký tự qua đúng parser thật.
- Driver Claude thật (`src/brain/claude.rs`) chạy thật với claude 2.1.325: một lượt có gọi `capture_photo` (ảnh JPEG giả vẽ `x^2 - 5x + 6 > 0`, Claude đọc đúng đề và trả lời đúng định dạng dòng), ngắt lời giữa lượt, lượt tiếp sau ngắt trong cùng tiến trình, khởi động lại server rồi `--resume` giữ nguyên ngữ cảnh. Tổng cộng khoảng 5 lượt `haiku` ngắn.
- ElevenLabs trực tiếp: (đợt đầu) gọi thật 1 lần, gói Free trả HTTP 402 `paid_plan_required`, server rơi về giọng thiết bị. Chủ nhà đã bỏ key ElevenLabs, nên nhánh này chỉ còn là dự phòng chưa kiểm chứng.
- Test: `cargo test` = 41 đơn vị + 34 tích hợp (12 luồng chính với Claude giả, 12 GenVoice giả, 10 MCP chung), chạy 3 lần liên tiếp đều xanh, 0 fail; thêm 3 test thật `--ignored` (2 Claude, 1 GenVoice) tốn hạn mức/credit.

**Chưa chạy được / còn dở**
- TTS ElevenLabs thành công (mp3 qua frame `audio`) và STT ElevenLabs (`audio_start`..`audio_end`) CHƯA chạy với key đủ quyền: chỉ có đường lỗi 402 là đã thấy thật. Endpoint (`/v1/text-to-speech/{voice}?output_format=mp3_44100_64`, `/v1/speech-to-text` multipart, `language_code`) viết theo tài liệu từ trí nhớ, cần xác nhận khi có gói trả phí. Phần STT mặc định tắt (`ELEVENLABS_STT_MODEL` trống).
- Câu `say` đã gửi với `audio:true` mà tạo tiếng lỗi/quá hạn thì server gửi `audio_skip{seq}` ngay (máy đọc bằng giọng hệ thống), không để máy chờ hết `audioWaitMs`.
- Chưa thử với app iOS thật; chưa đo độ trễ qua Tailscale.
- `tutor/CLAUDE.md` chưa có phần dạy Claude dùng `Skill` cho module môn học (chưa có skill nào).
- Thời gian tới câu nói đầu tiên với haiku khoảng 4 đến 6 giây (gồm chụp ảnh); chưa tối ưu. Có thể thử `BELUGA_EFFORT=low`.

**Quyết định tự chọn**
- Viết bằng Rust theo yêu cầu mới của chủ nhà (một file exe, không cần Node). Bản Node/TS chưa từng được commit.
- MCP `capture_photo` là route HTTP ngay trong server (`POST /mcp/<sessionId>`, JSON-RPC tối thiểu: initialize, tools/list, tools/call, ping), chỉ nhận từ loopback, trỏ tới bằng `--mcp-config` (file `server/data/mcp/<sid>.json`). Chọn vì không cần tiến trình thứ hai và biết chắc phiên nào; đã xác nhận Claude Code 2.1.325 kết nối được và nhận ảnh qua `image` content. Đường dự phòng `mcp-stdio` không cần dùng nên không viết.
- Ngắt lời dùng `control_request{subtype:"interrupt"}` của stream-json (thử thật: tiến trình sống tiếp, `result` có `terminal_reason:"aborted_streaming"`, phần đã sinh vẫn nằm trong ngữ cảnh). `turn_end{interrupted:true}` gửi ngay, không chờ Claude; nếu sau 6 giây Claude chưa dừng thì kill tiến trình và lượt sau `--resume`. Lượt kế được kèm ghi chú hệ thống nói bạn ấy nghe tới câu nào (`played.seq`) và phần chưa nghe.
- Lượt người dùng đến khi lượt trước còn chạy thì xếp hàng, chạy sau khi Claude kết thúc lượt trước.
- `@title` được server thêm ` · dd/mm`; tên phiên mới là "Phiên mới · dd/mm" cho tới khi có `@title`. Phiên chỉ vào danh sách sau lượt đầu tiên.
- Parser sửa lỗi nhỏ: JSON có `\` đơn không hợp lệ (LaTeX như `\(`) được thử sửa trước khi bỏ; `@ask` có `kind` lạ thành `check`; dòng chỉ có ký hiệu (`---`) bị bỏ; `@show` đầu tiên không có `@step` tự mở step tên rỗng.
- Tiến trình `claude` sống suốt phiên, tự tắt sau 20 phút rảnh rồi `--resume` lại; chết giữa lượt trước khi có chữ nào thì khởi động lại và gửi lại một lần. Đổi `model` trong cài đặt thì khởi động lại tiến trình bằng `--resume`.
- Server xóa `ANTHROPIC_API_KEY`/`ANTHROPIC_AUTH_TOKEN` khỏi môi trường của `claude` để chắc chắn dùng đăng nhập gói Pro, không tính tiền API.
- Mode `--mock` không gọi nhà cung cấp giọng nói nào (GenVoice/ElevenLabs) trừ khi `BELUGA_VOICE_IN_MOCK=1`.
- `Read` và `Skill` được cho phép; `Skill` cho phép cả skill dựng sẵn của Claude Code (design, loop...) vì chưa có skill môn học để lọc theo tên. Khi có module môn học nên thu hẹp lại (ví dụ `Skill(tên-module)`).

**Giọng nói: GenVoice (số 1), ElevenLabs trực tiếp (số 2), giọng thiết bị (dự phòng cuối)**
- Chọn bằng `TTS_PROVIDER` / `STT_PROVIDER` = `auto|genvoice|elevenlabs|device` (mặc định `auto`: có `GENVOICE_API_KEY` thì GenVoice, rồi ElevenLabs, rồi thiết bị). `language_code` theo `settings.lang`. `server/.env` chỉ có `PORT` và `GENVOICE_API_KEY` lúc làm; `.env.example` đã cập nhật. Không in, không commit key.
- GenVoice TTS: `POST /v1/text-to-speech/{voice}` (202) rồi poll `GET /v1/history/{id}` mỗi 220 ms tới `completed`/`failed`, tải `result.audio_url` (gửi `xi-api-key` chỉ khi URL cùng host với API). STT: `POST /v1/speech-to-text` multipart (`language_code`, `timestamps_granularity=none`, `tag_audio_events=false`, `keyterms` lặp lại) rồi poll `GET /v1/speech-to-text/{task_id}`, chữ ở `result.text`. Một `reqwest::Client` dùng chung (keep-alive).
- Độ trễ: mỗi câu `say` được tạo tiếng ngay khi parser phát ra, song song tối đa 4 task (`Semaphore`), frame `audio` gửi ngay khi xong (điện thoại xếp theo `seq`). Ngắt lời hủy (abort) các task TTS đang chờ và bỏ các `audio` muộn của lượt đó.
- Hạn mỗi câu `8000 ms + 60 ms x ký tự` (`BELUGA_TTS_BASE_MS`, `BELUGA_TTS_PER_CHAR_MS`); quá hạn, `failed`, hoặc lỗi HTTP: `audio_skip{seq}` và hủy task. 401/402, hoặc 3 câu lỗi liên tiếp (429, mạng, quá hạn, `failed`; một câu thành công thì đếm lại): `notice{tts_fallback}` một lần, các câu sau `audio:false`. `ready.audioWaitMs`: 20000 với GenVoice, 8000 với ElevenLabs, bỏ khi thiết bị.
- Probe lúc khởi động của GenVoice là `GET /v1/auth/me` (không tốn credit): log `credit_balance`, cảnh báo khi dưới 2000; lỗi thì `ready.tts="device"` và một `notice{tts_fallback}` cho điện thoại đầu tiên.
- STT: ghép frame nhị phân thành WAV (bọc header PCM16 mono nếu chưa có `RIFF`; sampleRate lấy từ `audio_start`), hạn `8000 ms + 1500 ms x số giây âm thanh`; chữ rỗng thì `notice{stt_empty}`, lỗi/quá hạn thì `notice{stt_failed}`, không gọi Claude. Ngôn ngữ STT theo `settings.lang` mới nhất của kết nối. `keyterms`: vài thuật ngữ toán/tiếng Anh (`STT_KEYTERMS` trong `voice.rs`).
- Kiểm chứng: 12 test với dịch vụ GenVoice giả (axum cục bộ, luồng 202 -> poll -> tải): hình dạng request, song song và `audio` tới sai thứ tự, tối đa 4 cùng lúc, ngắt lời hủy task, 401, 3 lỗi liên tiếp, 1 lỗi không tắt TTS, quá hạn, probe lỗi, STT (bọc WAV, không bọc đôi, rỗng/lỗi/quá hạn).
- Chạy THẬT một lần (Claude giả, `BELUGA_VOICE_IN_MOCK=1`, 2 lượt, 4 câu, khoảng 95 credit): probe + `ready` sau 425 ms; từ lúc `say` tới lúc nhận `audio` mp3 (đã tạo + tải xong): 2259 ms (55 ký tự), 1517 ms (41), 2385 ms (27), 2312 ms (48); file 35 đến 53 KB, đúng định dạng mp3. Các câu cách nhau 250 ms vẫn xong gần như cùng lúc (song song). Lưu ý: hai câu 41 và 27 ký tự cho file cùng 35153 byte; chưa nghe lại để xác nhận nội dung (có thể trùng ngẫu nhiên hoặc đệm).
- Chưa kiểm chứng: STT GenVoice thật (chỉ chủ nhà đã đo, tôi chưa gửi âm thanh thật qua server); 429 thật; nhánh ElevenLabs trực tiếp; tải tiếng thật qua Tailscale tới iPhone.

**MCP chung "beluga" (mọi phiên Claude Code trên PC nói chuyện được với iPhone)**
- `POST /mcp` (không sessionId), chỉ nhận từ loopback (hàm `allowed_peer`, có test; không thử được từ IP ngoài trong test tích hợp). Nhắm vào điện thoại kết nối mới nhất còn sống (đóng kết nối thì lùi về kết nối cũ hơn); không có máy nào thì tool trả lỗi chữ tiếng Việt, `phone_status` trả `connected:false`.
- Tools: `phone_status`, `capture_photo`, `speak {text}`, `present {lines}`, `listen {timeout_s?}` (mặc định 30, tối đa 120; câu nói không đến tay gia sư; `listen` thứ hai thay thế `listen` cũ; hết giờ thì trả `heard:null` và câu nói sau đó đi vào gia sư bình thường), `interrupt`.
- `speak`/`present` chạy như một "lượt" riêng (turn mới, `seq` hợp lệ, qua đúng parser, ghi nhật ký, có `turn_end`, có tiếng theo provider hiện tại). `present` trả `{turn, events, dropped}` (dropped là các dòng `@` bị bỏ kèm lý do). Gia sư đang chạy hoặc có lời học sinh chờ xử lý thì trả lỗi "đang bận"; `interrupt` dừng lượt gia sư, hoặc (khi rảnh) phát `turn_end{interrupted:true}` và hủy tiếng đang tạo cho phần `speak`/`present` đã xếp hàng. Điện thoại có dừng phát hay không khi nhận `turn_end` ngắt ở lượt đã kết thúc là chưa kiểm chứng trên app.
- Phiên gia sư giữ nguyên: `--strict-mcp-config` với route theo phiên, chỉ thấy `capture_photo` (có test: gọi tool khác qua route đó bị từ chối).
- Đã xác nhận bằng Claude Code thật không tốn lượt model: `claude -p --strict-mcp-config --mcp-config` trỏ `/mcp`, thông điệp `init` báo `beluga` `connected` và 6 tool `mcp__beluga__*`. `claude mcp list` không kiểm tra được vì server phạm vi project cần duyệt tay, và không tự đăng ký vào cấu hình user. `scripts/register-mcp.ps1` chỉ được tạo, chưa chạy (đã thử cú pháp `claude mcp add --transport http --scope user beluga http://127.0.0.1:8787/mcp` qua `--help`).

**Cờ `claude` đang dùng** (xem `build_args` trong `src/brain/claude.rs`, có test khóa lại):
`-p --input-format stream-json --output-format stream-json --verbose --include-partial-messages --model <sonnet|opus|haiku> (--session-id <uuid> | --resume <uuid>) --setting-sources project --strict-mcp-config --mcp-config <file> --tools Read,Skill --allowedTools Read Skill mcp__beluga__capture_photo --permission-mode dontAsk --permission-prompts none [--effort <mức>]` (mức: sonnet `medium`, opus `low`, haiku không truyền; `BELUGA_EFFORT` ghi đè mọi model; `claude --help` xác nhận `--effort` nhận low, medium, high, xhigh, max; có test khóa), cwd = `tutor/`. `--setting-sources project` vẫn nạp `tutor/CLAUDE.md` nhưng không nạp plugin/MCP/skill cá nhân của người dùng (init báo plugin chỉ có loại builtin, `mcp_servers` rỗng khi không truyền MCP); đăng nhập sẵn có của CLI vẫn dùng được. Không dùng `--bare` vì nó bỏ OAuth.
