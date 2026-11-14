# Beluga Live — giao thức (v1)

Hợp đồng giữa app iOS (`ios/`), server PC (`server/`) và prompt gia sư (`tutor/`).
Đổi file này trước, rồi mới đổi code hai bên.

## 1. Tổng quan

```
iPhone ──WebSocket (Tailscale, không TLS)──► server (Node, PC) ──stdio stream-json──► claude CLI
  mic/STT, camera, loa/TTS, bảng                parser dòng, ElevenLabs, MCP tool          cwd = tutor/
```

- Một phiên trên điện thoại = một session Claude Code (`sessionId` là UUID dùng chung).
- Claude viết ra **văn bản theo dòng** (mục 2). Server đọc từng dòng ngay khi dòng kết thúc,
  đổi thành sự kiện có số thứ tự `seq` và đẩy xuống điện thoại.
- Điện thoại xếp sự kiện vào một **timeline**: lời nói phát lần lượt, khối bảng chỉ hiện khi
  lời nói đứng trước nó đã phát xong. Đó là cách đồng bộ lời nói với màn hình.
- Chỉ có một việc cần Claude gọi tool: chụp ảnh (`capture_photo`, qua MCP của server).

## 2. Định dạng dòng bài giảng (Claude → server)

Mỗi dòng là một trong hai loại:

| Dòng | Ý nghĩa |
|---|---|
| văn bản thường | Lời nói. Một dòng = một câu ngắn, văn nói. Không markdown, không LaTeX. |
| `@lệnh {json một dòng}` | Chỉ thị cho màn hình. JSON nằm trọn trên một dòng. |

Chỉ thị:

| Chỉ thị | JSON | Tác dụng |
|---|---|---|
| `@title` | `{"text":"Toán · Bất phương trình bậc hai"}` | Đặt tên phiên (server tự thêm ngày). |
| `@step` | `{"title":"Chuyển vế"}` | Mở một bước mới trên bảng. |
| `@show` | một *khối* (mục 3) | Thêm khối vào bước hiện tại. |
| `@ask` | `{"kind":"next\|why\|check","prompt":"Bước tiếp theo là gì?"}` | Điểm dừng "vặn lại". Phải là dòng cuối của lượt. |
| `@done` | `{}` | Bài giảng xong (hiệu ứng hoàn tất). |

Quy tắc parser (server, có test):
- Dòng trống bỏ qua. Dòng `@...` JSON hỏng hoặc lệnh lạ: bỏ, ghi log, không làm hỏng lượt.
- Văn bản thường bị lẫn markdown (`**`, `#`, `` ` ``, `$`) thì gột ký hiệu trước khi đọc.
- Dòng lời nói dài hơn 220 ký tự thì cắt theo dấu câu thành nhiều `say`.
- Văn bản sau `@ask` trong cùng lượt bị bỏ.
- Parser nhận dữ liệu theo mảnh bất kỳ (streaming) và chỉ phát sự kiện khi gặp `\n` hoặc kết thúc lượt.

Ví dụ một lượt:

```
@title {"text":"Toán · Bất phương trình bậc hai"}
Câu này là bất phương trình bậc hai, nhìn màn hình nhé.
@step {"title":"Đưa về dạng chuẩn"}
@show {"type":"math","tex":"x^2 - 5x + 6 > 0"}
Việc đầu tiên là tìm nghiệm của tam thức.
@ask {"kind":"next","prompt":"Cậu tìm nghiệm bằng cách nào?"}
```

## 3. Khối bảng (`@show`)

Trường chung: `type` (bắt buộc), `id` (server tự cấp nếu thiếu), `fallback` (markdown, tùy chọn),
`note` (chú thích nhỏ, tùy chọn).

| `type` | Trường |
|---|---|
| `math` | `tex` (KaTeX, display) |
| `chem` | `tex` (cú pháp mhchem, ví dụ `2H2 + O2 -> 2H2O`) |
| `text` | `md` (markdown, cho phép `$...$` inline) |
| `table` | `head: string[]`, `rows: string[][]` (ô là markdown inline) |
| `code` | `lang`, `code`, `highlight?: number[]` (số dòng) |
| `timeline` | `items: [{"when":"1945","what":"..."}]` |
| `vocab` | `word`, `ipa?`, `pos?`, `meaning`, `example?` |
| `svg` | `svg` (chuỗi SVG, dùng `currentColor`, không script) |
| `plot` | `fn: string[]` (biểu thức theo x, ví dụ `"x^2-5*x+6"`), `x:[min,max]`, `y?:[min,max]`, `points?:[{"x":2,"y":0,"label":"A"}]` |
| `answer` | `md` (kết quả cuối, hiển thị nổi bật bằng độ đậm, không màu) |

**Mở rộng không cần sửa app:** bộ vẽ bảng là một gói web (`shared/board/`) chạy trong WKWebView.
App mang theo một bản; server cũng phục vụ nó ở `GET /board/`. Nếu `ready.boardVersion` lớn hơn
bản trong app thì app tải bảng từ server. Kiểu khối lạ: vẽ `fallback` (markdown); không có
`fallback` thì bỏ qua khối.

Hợp đồng JS của bảng (app gọi qua `evaluateJavaScript`, bảng gọi lại qua
`window.webkit.messageHandlers.board.postMessage`):

```
board.reset()                       xóa hết
board.apply(event)                  event là step/show/ask/done ở mục 4 (nguyên JSON)
board.goto(stepIndex)               xem lại bước cũ (vuốt)
board.setTheme("light"|"dark")
board.setLang("vi"|"en")
→ postMessage {type:"ready", version}            khi tải xong
→ postMessage {type:"height"|"stepChanged", ...} tùy chọn
```

## 4. WebSocket `ws://<pc>:8787/ws`

Mỗi frame là một JSON có `t`. Sự kiện bài giảng có `seq` tăng dần trong phiên và `turn`.

### Điện thoại → server

| `t` | Trường | Ghi chú |
|---|---|---|
| `hello` | `v:1, pairCode?, pairToken?, device, sessionId?, lastSeq?, settings` | `sessionId` rỗng = phiên mới. `lastSeq` để nối lại. |
| `settings` | `lang:"vi"\|"en", model:"sonnet"\|"opus"\|"haiku", rigor:"light"\|"medium"\|"hard"` | Áp dụng từ lượt sau. |
| `user_text` | `text` | Lời người dùng đã nhận dạng trên máy (STT thiết bị). |
| `audio_start` | `mime:"audio/wav", sampleRate` | Chỉ khi `ready.stt == "server"`. Sau đó là các frame nhị phân. |
| `audio_end` | — | Server nhận dạng rồi tự coi như `user_text`. |
| `interrupt` | — | Ngắt lời: dừng lượt đang chạy, bỏ phần chưa phát. |
| `photo` | `requestId?, jpeg (base64), w, h` | Trả lời `capture_request`, hoặc chụp tay (không có `requestId`). |
| `played` | `seq` | Đã phát tới sự kiện này (server dùng khi ngắt lời và khi nối lại). |
| `list_sessions` / `open_session` | `sessionId` | Danh sách và mở lại phiên cũ. |

### Server → điện thoại

| `t` | Trường | Ghi chú |
|---|---|---|
| `ready` | `v, sessionId, title, tts:"device"\|"server", stt:"device"\|"server", audioWaitMs?, boardVersion, pairToken?` | `audioWaitMs`: thời gian tối đa máy chờ frame `audio` của một `say` (mặc định 3000). |
| `state` | `state:"idle"\|"thinking"\|"tool"\|"speaking"\|"done", tool?:"capture"` | "listening" do máy tự biết. |
| `heard` | `text` | Kết quả STT phía server (để hiện phụ đề). |
| `say` | `seq, turn, text, audio:boolean` | `audio:true` thì chờ frame `audio` cùng `seq`. |
| `audio` | `seq, mime:"audio/mpeg", data (base64)` | Quá `ready.audioWaitMs` kể từ lúc tới lượt phát câu đó mà chưa tới thì máy đọc bằng giọng hệ thống. Server tạo tiếng cho các câu song song, nên `audio` có thể tới trước lượt phát. |
| `step` | `seq, turn, index, title` | |
| `show` | `seq, turn, step, block` | |
| `ask` | `seq, turn, kind, prompt` | Sau khi phát xong: máy vào trạng thái chờ trả lời. |
| `done` | `seq, turn` | |
| `turn_end` | `seq, turn, interrupted?:boolean` | |
| `title` | `text` | |
| `capture_request` | `requestId` | Máy chụp một ảnh nét rồi gửi `photo`. Quá 8 giây: tool báo lỗi cho Claude. |
| `snapshot` | `events:[...]` | Sau `ready` khi nối lại: các sự kiện có `seq > lastSeq` (đủ để dựng lại bảng). |
| `sessions` | `items:[{sessionId,title,updatedAt}]` | |
| `notice` | `code, text` | Nhẹ, hiện nhỏ. Ví dụ `tts_fallback`, `reconnected`. |
| `error` | `code, text, fatal` | `bad_pair`, `claude_unavailable`, `rate_limited`, ... |

### Ghép đôi
Server in mã 6 số khi khởi động. Lần đầu app gửi `pairCode`, nhận `pairToken` (lưu Keychain),
các lần sau gửi `pairToken`. Sai: `error{code:"bad_pair", fatal:true}`.

### HTTP
`GET /health` → `{ok, v, boardVersion}` · `GET /board/*` → gói bảng tĩnh.

## 5. Trạng thái pill (máy tự suy ra)

| Trạng thái | Khi nào | Pill |
|---|---|---|
| `idle` | không có gì | vạch mảnh, đứng yên |
| `listening` | mic mở | nhảy theo âm lượng mic thật |
| `thinking` | đã gửi lời, chưa có `say` | nhịp thở chậm; quá 4 giây đọc "Đang suy nghĩ…" |
| `tool` | `state.tool` | vạch quét ngang (chụp ảnh: nháy màn trập) |
| `speaking` | đang phát `say` | nhảy theo âm lượng tiếng phát thật |
| `asking` | vừa phát xong `ask` | đứng chờ, mời nói |
| `done` | sau `done` | khép lại một nhịp rồi về `idle` |
| `offline` | mất kết nối | vạch đứt, tự nối lại |
