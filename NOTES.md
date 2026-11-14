# NOTES — quyết định và tiến độ

## Tiến độ
(cập nhật ở cuối mỗi giai đoạn)

- [ ] 1. Khung app, giao diện lofi trắng đen (sáng/tối), mock, CI ra IPA unsigned
- [ ] 2. Server PC, luồng giọng nói (mock), trình chiếu bài giảng từng bước
- [ ] 3. Điểm dừng "vặn lại"
- [ ] Để sau: memory, ôn lại, màn hình tiến độ, module môn học

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
- **Chưa có key ElevenLabs:** nghe bằng nhận dạng giọng nói của iOS, đọc bằng giọng hệ thống iOS.
  Điền key vào `server/.env` thì server chuyển sang ElevenLabs (chưa chạy thử với key thật).
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
- ElevenLabs: gọi thật đúng 1 lần (probe lúc khởi động): gói Free trả HTTP 402 `paid_plan_required` với giọng thư viện, server rơi về `ready.tts="device"` và gửi `notice{tts_fallback}` một lần cho điện thoại đầu tiên.
- Test: `cargo test` = 34 đơn vị + 12 tích hợp (WS giả) đều xanh; thêm 2 test thật `--ignored` (tốn hạn mức).

**Chưa chạy được / còn dở**
- TTS ElevenLabs thành công (mp3 qua frame `audio`) và STT ElevenLabs (`audio_start`..`audio_end`) CHƯA chạy với key đủ quyền: chỉ có đường lỗi 402 là đã thấy thật. Endpoint (`/v1/text-to-speech/{voice}?output_format=mp3_44100_64`, `/v1/speech-to-text` multipart, `language_code`) viết theo tài liệu từ trí nhớ, cần xác nhận khi có gói trả phí. Phần STT mặc định tắt (`ELEVENLABS_STT_MODEL` trống).
- Sau khi TTS lỗi giữa chừng, các `say` đã gửi với `audio:true` sẽ chờ 3 giây (theo PROTOCOL) rồi điện thoại tự đọc; các `say` sau là `audio:false`.
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
- Mode `--mock` không bao giờ gọi ElevenLabs (trừ khi `BELUGA_VOICE_IN_MOCK=1`).
- `Read` và `Skill` được cho phép; `Skill` cho phép cả skill dựng sẵn của Claude Code (design, loop...) vì chưa có skill môn học để lọc theo tên. Khi có module môn học nên thu hẹp lại (ví dụ `Skill(tên-module)`).

**Cờ `claude` đang dùng** (xem `build_args` trong `src/brain/claude.rs`, có test khóa lại):
`-p --input-format stream-json --output-format stream-json --verbose --include-partial-messages --model <sonnet|opus|haiku> (--session-id <uuid> | --resume <uuid>) --setting-sources project --strict-mcp-config --mcp-config <file> --tools Read,Skill --allowedTools Read Skill mcp__beluga__capture_photo --permission-mode dontAsk --permission-prompts none [--effort <BELUGA_EFFORT>]`, cwd = `tutor/`. `--setting-sources project` vẫn nạp `tutor/CLAUDE.md` nhưng không nạp plugin/MCP/skill cá nhân của người dùng (init báo plugin chỉ có loại builtin, `mcp_servers` rỗng khi không truyền MCP); đăng nhập sẵn có của CLI vẫn dùng được. Không dùng `--bare` vì nó bỏ OAuth.
