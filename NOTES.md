# NOTES — quyết định và tiến độ

## Tiến độ
(cập nhật 14/11/2026, 18:15)

- [x] 1. Khung app, giao diện lofi trắng đen (sáng/tối), mock, CI ra IPA unsigned.
      CI xanh (run 43572866046): test BelugaCore, IPA unsigned, 26 screenshot simulator.
- [x] 2. Server PC (Rust) và luồng giọng nói, trình chiếu bài giảng từng bước.
      Server đã chạy với Claude Code thật và GenVoice thật; app mới chạy ở chế độ mock trên simulator.
- [x] 3. Điểm dừng "vặn lại": có ở server (parser, lượt dừng sau `@ask`) và ở app (thẻ "Đến lượt bạn").
- [ ] **Chưa làm lần nào: app thật nói chuyện với server thật trên iPhone.** Đây là việc kế tiếp.
- [x] Memory, ôn lại đầu phiên, tám module môn học: xong ở server, đã chạy thử với Claude thật (riêng module Toán; bảy module còn lại mới kiểm cú pháp). Trang tiến độ trong app: xem mục iOS.

## iOS (`ios/`, `shared/board/`)

Cập nhật 14/11/2026. CI xanh ở run 43591800568 (commit f640c8b): `swift test` BelugaCore, IPA unsigned
(đúng cấu trúc, không `_CodeSignature`, không provisioning profile), 30 screenshot simulator, video demo.

**Đã kiểm chứng bằng screenshot và video trên simulator (mock, `-mock 1`)**
- Đã xem cả 24 ảnh cảnh (12 cảnh x sáng/tối), 6 ảnh flow và 3 bản icon.
- Phụ đề nằm ở vùng camera ngay trên mép bảng, không còn che bảng; cuối bảng có đệm 128 px nên không
  khuất sau nút chụp, pill, nút thu bảng (thấy ở cảnh lesson, ask, done và trong video).
- Đồ thị: nhãn điểm "2", "3" đặt cách xa đường cong và trục (tự chọn vị trí thoáng nhất quanh điểm),
  nhãn trục trùng với nhãn điểm đã bỏ.
- Pill kiểu "quầng sáng" (run 43595441711): viên nang 200x64 là Liquid Glass hệ thống nhuộm đen, thêm lớp nền
  tối bên trong nên ở cả chế độ sáng lẫn tối đều là viên nang tối; quầng sáng trắng-xám dâng từ mép dưới, cắt
  theo viên nang, một màu duy nhất `Palette.glow` (đổi một dòng trong `Tokens.swift` để chuyển màu). Chiều cao
  và độ sáng bám mức âm thật đã làm mượt (attack 60 ms, release 250 ms; mức 0 thì tắt hẳn); mép trên gợn nhẹ,
  pha gợn chỉ trôi khi có tiếng. listening = quầng phẳng theo mic; speaking = quầng dạng vòm (giữa cao hơn) theo
  mức đầu ra của engine (cả mp3 lẫn giọng hệ thống đều đi qua engine nên đo thật) + biểu tượng loa; thinking =
  dải mờ thở chậm ở đáy (trạng thái duy nhất chuyển động khi im); tool = vệt sáng quét một lần mỗi 1,4 s;
  asking = đáy sáng nhẹ đứng yên + micro; done = quầng khép xuống một lần + tick; idle = chỉ còn viền;
  offline = viền đứt nét + biểu tượng mất sóng. Reduce Motion: bỏ gợn, thinking/tool đứng yên.
  Trích khung hình video (4 khung/giây) thấy quầng listening và speaking đổi độ cao giữa các khung, có khung
  tắt hẳn ở quãng nghỉ. Mock `-mock 1`: mức âm giả có âm tiết và quãng nghỉ thật; cảnh screenshot giữ mức cố định.
  Chưa đo được: độ mượt 60 fps trên máy thật (Canvas + blur, vẽ lại Path mỗi khung, không tối ưu thêm).
  Nút tròn hai bên 48 pt, pill 64 pt cao: vùng chạm đều trên 44 pt.
- Trang tiến độ (từ danh sách phiên hoặc cài đặt, hoặc `-mockScene progress`): từng môn với Đang yếu
  (chủ đề, ghi chú, số lần sai, ngày ôn hoặc "Đến hạn ôn"), Đã vững, Đã học gần đây, rồi Cách giảng hợp.
  Trạng thái rỗng, đang tải và chưa kết nối viết riêng. Chữ phẳng trên nền hệ thống, không thẻ.
- Khối `review_offer` (thẻ "Ôn lại": chủ đề, môn, lần cuối, ghi chú) và cảnh `-mockScene review`. Gói bảng
  tăng lên `version 2`; server đọc version từ `shared/board/version.json` nên tự báo 2.
- Video `demo-video` (33,6 s, h264, 1206x2622, khoảng 18 MB): trích khung hình mỗi 2 giây thấy đúng
  trình tự idle, nghe, nghĩ, bảng trượt lên từng bước (Đọc đề, Tìm nghiệm, Xét dấu), pill "đến lượt
  bạn", nghe câu trả lời, nghĩ, nói, Kết luận, Xong bài, về idle.
- Icon vẽ lại lần hai: ba khối đặc cùng một màu (thân bo tròn, trán bầu hình tròn, đuôi hai thùy hếch lên) và một
  chấm mắt, không miệng, không vây lưng; bản sáng, tối, tinted xem ở cỡ lớn và 40 px đều đọc là cá voi trắng.
  Nguồn SVG ở `ios/design/icon*.svg`. Vẫn nên để chủ nhà duyệt hình.
- Tương phản: `Ink.tertiary` 0,38 lên 0,55, `Ink.secondary` 0,62 lên 0,68; `--ink-3` của bảng 0,38 lên 0,6
  (trước đó nhãn nhỏ chỉ khoảng 2,7:1).

**Sửa sau khi đọc mã đường chạy thật (chưa chạy được trên thiết bị)**
- Quyền: camera xin lúc mở app; micro và nhận dạng giọng xin ở lần chạm pill đầu tiên (lần đó chỉ xin quyền,
  hiện thông báo "chạm giữ lần nữa"). Bị từ chối thì hiện khung điềm đạm có nút "Mở Cài đặt"; quay lại app
  từ Cài đặt thì tự gỡ khung và bật lại camera.
- `bad_pair` mở thẳng Cài đặt (ô mã ghép đôi); nhập lại cùng mã vẫn nối lại được (trước đó bị bỏ qua).
  Vào foreground thì nối lại ngay, không chờ backoff.
- Ảnh chụp: đặt `videoRotationAngle = 90` cho photo output (app chỉ dọc; trước đó ảnh có thể ra ngang).
  Chụp lỗi thì báo nhẹ trên màn hình thay vì im lặng.
- Chạm pill khi đang nói: dừng tiếng ngay, rồi mở mic (giữ để nói) hoặc giữ nghe rảnh tay đang bật.
- AVAudioEngine: khởi động lại khi đổi route và sau cuộc gọi; đầu vào voice processing được tắt tiếng
  (`isVoiceProcessingInputMuted`) trừ lúc đang nghe.
- Bảng luôn nằm trong cây view và trượt ra ngoài màn hình khi ẩn (cách cũ chèn/gỡ bằng transition làm bảng
  không hiện trong luồng mock). Lịch sử/snapshot vẽ không animation.

**Đã đọc mã và thấy đúng (không đổi)**: `hello` gửi `pairCode`/`pairToken`, `sessionId`, `lastSeq`, `settings`;
`pairToken` lưu Keychain; backoff 0,5 s tới 15 s có nhiễu, gửi `lastSeq`; `played{seq}` gửi sau mỗi câu và mỗi
sự kiện bảng; WAV PCM16 mono 16 kHz header RIFF đủ 44 byte, gửi từng khúc 64 KB; không có tap micro khi
không nghe.

**Chưa kiểm chứng (cần iPhone thật)**
- Camera, micro, nhận dạng giọng, phát tiếng thật, khử vọng, ngắt lời, rảnh tay, âm báo, rung, WebSocket
  qua Tailscale, ghép đôi, tự nối lại, chụp theo `capture_request` trong 8 giây.
- Đường nghe qua server (WAV) và phát mp3 từ server: có mã, chưa chạy với server thật.
- Hộp xin quyền và hành vi sau khi từ chối: chỉ đọc mã.

**Nghi vấn còn mở**
- Mic cam: `AVAudioEngine` với voice processing chạy suốt phiên nên chấm cam/micro của hệ thống có thể sáng
  cả khi không nghe, dù tiếng vào đã tắt. Nếu phiền thì phải dựng lại engine mỗi lần nghe.
- Audio session dùng mode `.default` + `defaultToSpeaker` + voice processing của engine. Nếu tiếng ra loa nhỏ
  hoặc còn vọng, thử đổi sang mode `.voiceChat`.
- `AVSpeechSynthesizer.write` với giọng vi-VN: nếu máy chưa tải giọng thì không có tiếng, watchdog 4 giây
  cho bài giảng đi tiếp (không đọc được câu đó).
- Tiếng mp3 dừng giữa chừng (`fileNode.stop()`): giả định completion handler vẫn được gọi; nếu không thì
  task phát bị treo nhưng không chặn câu sau.
- Nhận dạng giọng trên máy với `requiresOnDeviceRecognition`: vi-VN có thể không hỗ trợ on-device trên máy cũ.
- Phụ đề dài 3 dòng khi bảng đang mở có thể chạm thanh tiêu đề trên iPhone nhỏ (mới xem ở simulator iPhone 17).
- Tiêu đề bước đã cách tên bài 24 pt khi chưa cuộn (header đệm 8 + trang đệm 16) và mờ dần dưới mép trên khi
  cuộn; khi cuộn dở, chữ tiêu đề có thể bị mờ nửa chừng (chủ ý, không cắt gắt).
- Định dạng ngày của `progress` (server gửi `YYYY-MM-DD`) đã khớp mã server; phần còn lại của khung progress
  đọc theo PROTOCOL, chưa thử với server thật.

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
- Test: `cargo test` = 58 đơn vị + 48 tích hợp (12 luồng chính với Claude giả, 13 GenVoice giả, 10 MCP chung, 9 memory/ôn lại/tiến độ, 4 skill), xanh, 0 fail; thêm 3 test thật `--ignored` (2 Claude, 1 GenVoice) tốn hạn mức/credit.
- **Memory, mời ôn, tiến độ, môn học (PROTOCOL mục 6): đã xong, test xanh, và đã chạy thật một lần với Claude thật (xem "Bài thử thật đầu-cuối").** `src/memory.rs`: file `tutor/memory/profile.md` và `<môn>.md` (gitignore, sửa tay được, dòng không theo khuôn giữ nguyên, ghi atomic), `memory_note` trên route MCP theo phiên, lịch ôn thuần (sai: mai ôn; đúng: lên bậc 3, 7, 14 ngày; hai lần đúng liên tiếp: "Đã vững"), tóm tắt `[memory]` (<= 12 dòng) đính vào lượt đầu của phiên một lần (cờ `memory_sent` lưu trong `sessions.json`, không đính lại sau khởi động lại hay `--resume`), lời mời ôn do server tự phát đầu phiên mới (`say` + `step` + `show review_offer` + `turn_end`, có tiếng, một lần mỗi phiên, lượt kế tiếp kèm `[ôn lại: <môn> · <chủ đề> · <ghi chú>]`, còn nhớ sau khởi động lại), `get_progress` -> `progress`. Tám skill môn học ở `tutor/.claude/skills/` (40 đến 80 dòng, `tests/skills.rs` kiểm tra khuôn và mọi ví dụ `@lệnh` đi qua parser thật không bị bỏ).
- **Kiểm tra nghi vấn "gắn nhầm tiếng" (hai câu cùng 35153 byte): không phải lỗi.** Mã gắn tiếng theo task id của từng câu (`seq` và văn bản đóng gói trong task của chính nó; URL kết quả lấy từ đúng task đó), không có chỗ nào dùng thứ tự hoàn thành. Test mới `each_sentence_gets_its_own_audio_when_tasks_finish_out_of_order` (40 câu, dịch vụ giả trả nội dung khác nhau cho từng câu, hoàn thành lệch thứ tự) khóa lại. Đo thật: gọi lại 3 câu hai lần, mỗi lần mp3 khác nhau (md5 khác, URL khác) nhưng cùng một câu hay cho cùng kích thước (36407, 36407; 37661, 37661), nên kích thước do thời lượng giọng đọc quyết định và trùng nhau là chuyện thường; 35153 byte xuất hiện lại cho một câu thứ ba ("Tớ thấy đề rồi, câu a nhé."). Kiểm bằng nhận dạng giọng của chính GenVoice trên 3 mp3 của bài thử thật: mỗi file nói đúng câu của `seq` đó.

**Chưa chạy được / còn dở**
- TTS ElevenLabs thành công (mp3 qua frame `audio`) và STT ElevenLabs (`audio_start`..`audio_end`) CHƯA chạy với key đủ quyền: chỉ có đường lỗi 402 là đã thấy thật. Endpoint (`/v1/text-to-speech/{voice}?output_format=mp3_44100_64`, `/v1/speech-to-text` multipart, `language_code`) viết theo tài liệu từ trí nhớ, cần xác nhận khi có gói trả phí. Phần STT mặc định tắt (`ELEVENLABS_STT_MODEL` trống).
- Câu `say` đã gửi với `audio:true` mà tạo tiếng lỗi/quá hạn thì server gửi `audio_skip{seq}` ngay (máy đọc bằng giọng hệ thống), không để máy chờ hết `audioWaitMs`.
- Chưa thử với app iOS thật; chưa đo độ trễ qua Tailscale.
- Thời gian tới câu nói đầu tiên với sonnet medium là 8 đến 11 giây khi Claude gọi tool trước khi nói (nạp skill `toan` một vòng tool; chụp ảnh; `memory_note`), chỉ 2,6 giây khi không gọi tool nào. Mỗi vòng tool tốn 2 đến 7 giây. Chưa tối ưu: mô tả tool `memory_note` đã được siết ("gọi sau khi đã nói những câu đầu, `progress` chỉ khi sắp `@done`") nhưng chưa kiểm chứng lại với Claude thật. Việc ngoài thư mục của tôi: `tutor/CLAUDE.md` có thể nói rõ "nói trước, ghi sau".
- Chưa kiểm chứng: `Skill` của Claude Code vẫn liệt kê 17 skill dựng sẵn (design, loop...) trong danh sách skill dù chỉ `Skill(<môn>)` được cho phép; chưa thử xem chúng có bị từ chối thật không (chưa cố gọi). Hạn chế đã biết: đọc tiếng Anh/Đức bằng giọng cài `lang=vi` (xem cuối mục).

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
- `Read` được cho phép; `Skill` chỉ cho tám skill môn học qua `--allowedTools Skill(toan) Skill(ly) ...` (không còn `Skill` trần). Xác nhận với claude 2.1.325 thật: `--setting-sources project` nạp skill trong `tutor/.claude/skills/` (cạnh 17 skill dựng sẵn), và `Skill {"skill":"toan"}` chạy được dưới `--permission-mode dontAsk` (hai lần, cả hai bài thử).

**Giọng nói: GenVoice (số 1), ElevenLabs trực tiếp (số 2), giọng thiết bị (dự phòng cuối)**
- Chọn bằng `TTS_PROVIDER` / `STT_PROVIDER` = `auto|genvoice|elevenlabs|device` (mặc định `auto`: có `GENVOICE_API_KEY` thì GenVoice, rồi ElevenLabs, rồi thiết bị). `language_code` theo `settings.lang`. `server/.env` chỉ có `PORT` và `GENVOICE_API_KEY` lúc làm; `.env.example` đã cập nhật. Không in, không commit key.
- GenVoice TTS: `POST /v1/text-to-speech/{voice}` (202) rồi poll `GET /v1/history/{id}` mỗi 220 ms tới `completed`/`failed`, tải `result.audio_url` (gửi `xi-api-key` chỉ khi URL cùng host với API). STT: `POST /v1/speech-to-text` multipart (`language_code`, `timestamps_granularity=none`, `tag_audio_events=false`, `keyterms` lặp lại) rồi poll `GET /v1/speech-to-text/{task_id}`, chữ ở `result.text`. Một `reqwest::Client` dùng chung (keep-alive).
- Độ trễ: mỗi câu `say` được tạo tiếng ngay khi parser phát ra, song song tối đa 4 task (`Semaphore`), frame `audio` gửi ngay khi xong (điện thoại xếp theo `seq`). Ngắt lời hủy (abort) các task TTS đang chờ và bỏ các `audio` muộn của lượt đó.
- Hạn mỗi câu `8000 ms + 60 ms x ký tự` (`BELUGA_TTS_BASE_MS`, `BELUGA_TTS_PER_CHAR_MS`); quá hạn, `failed`, hoặc lỗi HTTP: `audio_skip{seq}` và hủy task. 401/402, hoặc 3 câu lỗi liên tiếp (429, mạng, quá hạn, `failed`; một câu thành công thì đếm lại): `notice{tts_fallback}` một lần, các câu sau `audio:false`. `ready.audioWaitMs`: 20000 với GenVoice, 8000 với ElevenLabs, bỏ khi thiết bị.
- Probe lúc khởi động của GenVoice là `GET /v1/auth/me` (không tốn credit): log `credit_balance`, cảnh báo khi dưới 2000; lỗi thì `ready.tts="device"` và một `notice{tts_fallback}` cho điện thoại đầu tiên.
- STT: ghép frame nhị phân thành WAV (bọc header PCM16 mono nếu chưa có `RIFF`; sampleRate lấy từ `audio_start`), hạn `8000 ms + 1500 ms x số giây âm thanh`; chữ rỗng thì `notice{stt_empty}`, lỗi/quá hạn thì `notice{stt_failed}`, không gọi Claude. Ngôn ngữ STT theo `settings.lang` mới nhất của kết nối. `keyterms`: vài thuật ngữ toán/tiếng Anh (`STT_KEYTERMS` trong `voice.rs`).
- Kiểm chứng: 12 test với dịch vụ GenVoice giả (axum cục bộ, luồng 202 -> poll -> tải): hình dạng request, song song và `audio` tới sai thứ tự, tối đa 4 cùng lúc, ngắt lời hủy task, 401, 3 lỗi liên tiếp, 1 lỗi không tắt TTS, quá hạn, probe lỗi, STT (bọc WAV, không bọc đôi, rỗng/lỗi/quá hạn).
- Chạy THẬT một lần (Claude giả, `BELUGA_VOICE_IN_MOCK=1`, 2 lượt, 4 câu, khoảng 95 credit): probe + `ready` sau 425 ms; từ lúc `say` tới lúc nhận `audio` mp3 (đã tạo + tải xong): 2259 ms (55 ký tự), 1517 ms (41), 2385 ms (27), 2312 ms (48); file 35 đến 53 KB, đúng định dạng mp3. Các câu cách nhau 250 ms vẫn xong gần như cùng lúc (song song). Lưu ý cũ: hai câu 41 và 27 ký tự cho file cùng 35153 byte; đã điều tra, không phải lỗi gắn nhầm (xem trên). Lần đo thứ hai (4 câu: 50618, 41422, 33899, 48110 byte, 128 kbps, 3,16 / 2,59 / 2,12 / 3,00 giây, hash khác nhau; thời lượng tăng theo độ dài câu).
- Chưa kiểm chứng: STT GenVoice thật (chỉ chủ nhà đã đo, tôi chưa gửi âm thanh thật qua server); 429 thật; nhánh ElevenLabs trực tiếp; tải tiếng thật qua Tailscale tới iPhone.

**MCP chung "beluga" (mọi phiên Claude Code trên PC nói chuyện được với iPhone)**
- `POST /mcp` (không sessionId), chỉ nhận từ loopback (hàm `allowed_peer`, có test; không thử được từ IP ngoài trong test tích hợp). Nhắm vào điện thoại kết nối mới nhất còn sống (đóng kết nối thì lùi về kết nối cũ hơn); không có máy nào thì tool trả lỗi chữ tiếng Việt, `phone_status` trả `connected:false`.
- Tools: `phone_status`, `capture_photo`, `speak {text}`, `present {lines}`, `listen {timeout_s?}` (mặc định 30, tối đa 120; câu nói không đến tay gia sư; `listen` thứ hai thay thế `listen` cũ; hết giờ thì trả `heard:null` và câu nói sau đó đi vào gia sư bình thường), `interrupt`.
- `speak`/`present` chạy như một "lượt" riêng (turn mới, `seq` hợp lệ, qua đúng parser, ghi nhật ký, có `turn_end`, có tiếng theo provider hiện tại). `present` trả `{turn, events, dropped}` (dropped là các dòng `@` bị bỏ kèm lý do). Gia sư đang chạy hoặc có lời học sinh chờ xử lý thì trả lỗi "đang bận"; `interrupt` dừng lượt gia sư, hoặc (khi rảnh) phát `turn_end{interrupted:true}` và hủy tiếng đang tạo cho phần `speak`/`present` đã xếp hàng. Điện thoại có dừng phát hay không khi nhận `turn_end` ngắt ở lượt đã kết thúc là chưa kiểm chứng trên app.
- Phiên gia sư giữ nguyên: `--strict-mcp-config` với route theo phiên, chỉ thấy `capture_photo` (có test: gọi tool khác qua route đó bị từ chối).
- Đã xác nhận bằng Claude Code thật không tốn lượt model: `claude -p --strict-mcp-config --mcp-config` trỏ `/mcp`, thông điệp `init` báo `beluga` `connected` và 6 tool `mcp__beluga__*`. `claude mcp list` không kiểm tra được vì server phạm vi project cần duyệt tay, và không tự đăng ký vào cấu hình user. `scripts/register-mcp.ps1` chỉ được tạo, chưa chạy (đã thử cú pháp `claude mcp add --transport http --scope user beluga http://127.0.0.1:8787/mcp` qua `--help`).

**Cờ `claude` đang dùng** (xem `build_args` trong `src/brain/claude.rs`, có test khóa lại):
`-p --input-format stream-json --output-format stream-json --verbose --include-partial-messages --model <sonnet|opus|haiku> (--session-id <uuid> | --resume <uuid>) --setting-sources project --strict-mcp-config --mcp-config <file> --tools Read,Skill --allowedTools Read Skill(toan) Skill(ly) Skill(hoa) Skill(van) Skill(anh-ssat) Skill(duc) Skill(tin-cpp) Skill(chung) mcp__beluga__capture_photo mcp__beluga__memory_note --permission-mode dontAsk --permission-prompts none [--effort <mức>]` (mức: sonnet `medium`, opus `low`, haiku không truyền; `BELUGA_EFFORT` ghi đè mọi model; `claude --help` xác nhận `--effort` nhận low, medium, high, xhigh, max; có test khóa), cwd = `tutor/`. `--setting-sources project` vẫn nạp `tutor/CLAUDE.md` nhưng không nạp plugin/MCP/skill cá nhân của người dùng (init báo plugin chỉ có loại builtin, `mcp_servers` rỗng khi không truyền MCP); đăng nhập sẵn có của CLI vẫn dùng được. Không dùng `--bare` vì nó bỏ OAuth.

**Bài thử thật đầu-cuối (14/11/2026, không cần iPhone)**
`examples/fake_phone.rs` (WebSocket thật, "điện thoại giả") nối vào `beluga-server.exe` bản release chạy Claude Code thật (`sonnet`, effort `medium` mặc định) và GenVoice thật; server chạy với `BELUGA_ROOT` trỏ tới một bản sao tạm của `tutor/` nên không đụng memory thật. Hai lần chạy, mỗi lần hai lượt học sinh, không gặp lỗi hạn mức. Ảnh đề là JPEG 1200x800 dựng bằng PIL (bài 2a: `x² - 5x + 6 > 0`, 2b: `2x² + x - 3 <= 0`).
- *Lần 1 (bài giảng):* "Giảng tớ câu a" -> `capture_request` sau 2,65 s (điện thoại giả trả ảnh sau 700 ms) -> Claude đọc đúng đề, nạp skill `toan`, nói 3 câu, `@step`, `@show math`, `@ask next`. Tới `say` đầu 7,9 s, tới `audio` đầu 9,9 s (hơn `say` đầu 2,0 s), `turn_end` 7,9 s; 17 khung. Rồi trả lời SAI ("chia hai vế cho x ra x lớn hơn năm trừ sáu"): Claude nói sai ở hai chỗ, đưa bảng thử số cụ thể (`table`), chuyển sang cách khác (tìm nghiệm trước), `@ask next`; tới `say` đầu 11,1 s (Claude gọi `memory_note` mistake trước khi nói, mất khoảng 8,6 s), 13 khung. `audio` của lượt 2 không ghi được do một lỗi của chính ví dụ (ping WebSocket làm vòng chờ dừng sớm), đã sửa.
- *Lần 2 (mời ôn):* sửa tay ngày ôn của mục yếu thành đã quá hạn, phiên mới. Server tự phát lời mời 5 ms sau `ready` ("Lần trước cậu hay nhầm chia hai vế cho x thay vì tìm nghiệm tam thức, ôn một câu không?", `step` "Ôn lại", `show review_offer`, `turn_end`), `audio` của lời mời tới sau 4,1 s (câu 87 ký tự). "Ừ, ôn đi" -> Claude nạp skill, ra bài ôn `x^2-7x+10>0` bằng `@show math` + `@ask next` (`say` đầu 10,8 s, `audio` đầu 12,8 s); trả lời đúng hướng nhưng thiếu tập nghiệm -> Claude chấm "đúng hướng, chưa nói tập nghiệm", `@ask check` (`say` đầu 2,6 s, `audio` đầu 4,8 s, không gọi tool); không gọi `memory_note` ở lần này (chưa trả lời đủ).
- Parser bỏ 0 dòng trong cả hai lần (không có cảnh báo "dropped line"). Định dạng của Claude đúng: lời nói một câu mỗi dòng, `@title` tự đặt ("Toán · Bất phương trình bậc hai · 14/11"), không markdown, không `@done` (chưa xong bài).
- Skill nạp: `init` báo 25 skill (8 môn học + 17 dựng sẵn); Claude gọi `Skill {"skill":"toan"}` ở cả hai lần. Tool: `Read, Skill, mcp__beluga__capture_photo, mcp__beluga__memory_note`.
- Lỗi cách dùng `memory_note` thấy ở lần 1: Claude gọi `progress` ở đầu bài ("Bắt đầu bất phương trình bậc hai...") thay vì cuối bài, và gọi tool trước khi nói. Đã siết mô tả tool; chưa kiểm chứng lại.
- File memory sinh ra (`toan.md`): khối chú thích khuôn ở đầu, `## Đã học` chứa `- [học] Bắt đầu bất phương trình bậc hai: x^2-5x+6>0 (Bài 2a) | ngày 2026-11-14` và `## Đang yếu` chứa `- [yếu] Chia hai vế cho x thay vì tìm nghiệm tam thức | sai 1 | đúng 0 | bậc 0 | lần cuối 2026-11-14 | ôn 2026-11-15 | Bài x^2-5x+6>0: chia hai vế cho x, bỏ x ở số hạng 6 ...`.
- mp3: 9 file lưu lại, đều có header mp3 hợp lệ (`ID3` hoặc khung đồng bộ) và khung MPEG-1 layer III 128 kbps, thời lượng 1,2 đến 5,4 giây; 3 file của lần 1 đã đưa qua nhận dạng của GenVoice, mỗi file nói đúng câu của `seq` đó ("Từ thấy đẻ rồi, cầu à nhé" cho "Tớ thấy đề rồi, câu a nhé", ...). Vẫn chưa nghe bằng tai.
- Văn bản các lượt được Claude trả ra theo mảnh nhỏ (token) khi đang nói tiếp, nhưng sau một vòng tool thì nhiều lần cả khối chữ tới một lúc (dưới 10 ms), nên câu đầu tới ngay sau vòng tool chứ không dần dần.
- Chưa kiểm chứng: mọi thứ cần iPhone thật; môn khác ngoài Toán (skill `ly`, `hoa`, `van`, `anh-ssat`, `duc`, `tin-cpp`, `chung` mới chỉ được kiểm tra khuôn và cú pháp ví dụ, chưa chạy với Claude thật); `memory_note` `mastered` và việc hai lần ôn đúng chuyển "Đã vững" mới chỉ có test, chưa thấy Claude thật làm; đọc tiếng Anh/Đức: giọng TTS và nhận dạng giọng nói đều theo `settings.lang` (vi hoặc en), nên giọng Việt đọc câu tiếng Đức và nhận dạng tiếng Việt cho lời nói tiếng Đức sẽ cho chữ lệch (skill `duc` đã dặn Claude nói thật giới hạn này, nhưng cần cân nhắc cho phép `lang` theo từng câu).
