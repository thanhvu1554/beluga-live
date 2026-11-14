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
| `ready` | `v, sessionId, title, tts:"device"\|"server", stt:"device"\|"server", audioWaitMs?, boardVersion, pairToken?` | `audioWaitMs`: lưới an toàn, thời gian tối đa máy chờ `audio`/`audio_skip` của một `say` khi đã tới lượt phát (mặc định 3000; GenVoice gửi 20000). Bình thường server tự báo `audio_skip` trước mốc này, với hạn tính theo độ dài câu. |
| `state` | `state:"idle"\|"thinking"\|"tool"\|"speaking"\|"done", tool?:"capture"` | "listening" do máy tự biết. |
| `heard` | `text` | Kết quả STT phía server (để hiện phụ đề). |
| `say` | `seq, turn, text, audio:boolean` | `audio:true` thì chờ frame `audio` cùng `seq`. |
| `audio` | `seq, mime:"audio/mpeg", data (base64)` | Quá `ready.audioWaitMs` kể từ lúc tới lượt phát câu đó mà chưa tới thì máy đọc bằng giọng hệ thống. Server tạo tiếng cho các câu song song, nên `audio` có thể tới trước lượt phát. |
| `audio_skip` | `seq` | Server không tạo được tiếng cho câu này (lỗi hoặc quá hạn phía server): máy đọc ngay bằng giọng hệ thống, không chờ nữa. |
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

## 6. Memory, ôn lại, tiến độ, module môn học

**Lưu trữ** (`tutor/memory/`, đã gitignore, người dùng mở đọc và sửa tay được):
`profile.md` (cách giảng hợp, ghi chú chung) và mỗi môn một file `<môn>.md` (ví dụ `toan.md`). Mỗi mục
là một dòng danh sách theo khuôn cố định để server đọc lại được, dòng nào không theo khuôn thì giữ
nguyên và bỏ qua. Server là bên duy nhất ghi file; Claude không có quyền ghi.

**Tool của phiên gia sư** (cùng route MCP theo phiên với `capture_photo`):

| Tool | Tham số | Tác dụng |
|---|---|---|
| `memory_note` | `subject, kind:"mistake"\|"mastered"\|"style"\|"progress", topic, note` | Ghi một điều đáng nhớ. `mistake` tăng số lần sai và đặt lịch ôn; `mastered` đánh dấu đã vững sau khi trả lời đúng câu ôn; `style` ghi cách giảng hợp vào `profile.md`; `progress` ghi đã học tới đâu. Trả về ngay, không chặn lời nói. |

Server đính một **bản tóm tắt memory** (ngắn, vài dòng: cách giảng hợp, các chỗ yếu của môn liên quan)
vào lượt đầu của mỗi phiên, không tốn thêm lượt Claude.

**Mời ôn đầu phiên:** khi mở phiên mới mà có mục tới hạn ôn, server tự phát (không gọi Claude) một lượt
ngắn gồm `say` + `show` kiểu `review_offer` rồi `turn_end`. Người dùng đồng ý bằng giọng thì câu đó đi
tới Claude kèm ghi chú hệ thống về mục được mời ôn; từ chối hoặc nói chuyện khác thì bỏ qua.
Lịch ôn: 1, 3, 7, 14 ngày sau mỗi lần sai hoặc ôn; đúng hai lần ôn liên tiếp thì coi là đã vững.

Khối bảng mới: `review_offer` `{topic, subject, lastSeen, note?, fallback}` (luôn kèm `fallback`).

**Tiến độ (trang chỉ đọc trong app):**

| Hướng | `t` | Trường |
|---|---|---|
| máy → server | `get_progress` | — |
| server → máy | `progress` | `subjects:[{id, name, weak:[{topic,note,count,lastSeen,due}], mastered:[{topic,date}], recent:[{topic,date}]}], style:[string], updatedAt` |

**Module môn học:** mỗi môn là một skill của Claude Code ở `tutor/.claude/skills/<id>/SKILL.md`
(`toan`, `ly`, `hoa`, `van`, `anh-ssat`, `duc`, `tin-cpp`, `chung`). Gia sư nhận ra môn từ ảnh hoặc lời
nói rồi nạp đúng skill một lần mỗi phiên; môn chưa có skill thì nạp `chung`. Thêm môn = thêm một thư mục.

---

# Giao thức v2 (mở rộng, tương thích ngược với v1)

`hello.v` vẫn nhận 1; app mới gửi `v:2`. Mọi trường mới đều tùy chọn với client cũ.
Viết tắt mức độ câu hỏi: `nb` nhận biết, `th` thông hiểu, `vd` vận dụng, `vdc` vận dụng cao.

## 7. Hai chế độ: `chat` (mặc định) và `live`

`settings.mode: "chat"|"live"`. Đổi được bất kỳ lúc nào trong cùng một phiên (cùng session Claude).
Server đính `[mode: chat]` / `[mode: live]` vào lượt người dùng đầu tiên và mỗi khi mode đổi.

- **live** = v1 nguyên vẹn (camera, giọng nói, bảng từng bước, `say`/`audio`/`step`/`show`).
- **chat** = như Claude Chat: chỉ chữ, **không TTS, không `say`, không `audio`**. Claude viết markdown
  (cho phép `$...$`, `$$...$$`, bảng, code). Dòng bắt đầu bằng `@show`/`@ask`/`@title`/`@done` vẫn là chỉ thị
  (khối `@show` hiện xen giữa chữ); `@step {"title"}` trong chat thành một tiêu đề nhỏ (`chat` với `md:"### title"`).
  `capture_photo` chỉ dùng ở live; ở chat ảnh đến từ tệp đính kèm.

Sự kiện mới (server → máy):

| `t` | Trường | Ghi chú |
|---|---|---|
| `user` | `seq, turn, text, files?:[{fileId,kind:"image"\|"pdf",name}], via:"text"\|"voice"` | Tiếng vọng lượt người dùng, có ghi nhật ký (cả hai mode) để dựng lại lịch sử. |
| `chat_delta` | `turn, text` | Mảnh markdown đang stream. Không `seq`, không ghi nhật ký, không có trong `snapshot`. |
| `chat` | `seq, turn, md` | Một đoạn markdown hoàn chỉnh của trợ lý; thay thế toàn bộ `chat_delta` đã nhận kể từ `chat`/`show` gần nhất của lượt đó. Phát khi gặp chỉ thị `@...` hoặc hết lượt. |
| `show` | `seq, turn, step:-1, block` | Ở chat, `step` là -1: khối nằm xen trong dòng chat. |

Máy → server: `user_text{text, files?:[fileId]}` (tệp đã tải qua `POST /upload`). Ảnh thành image block, PDF thành
document block trong lượt gửi cho Claude (model không có vision: server báo `notice{code:"no_vision"}` và bỏ ảnh).

**Đọc chính tả (chat):** `audio_start{mime, sampleRate, draft:true}` … `audio_end` → `heard{text, raw?, draft:true}`;
server KHÔNG gửi cho Claude, app đặt chữ vào ô soạn để sửa rồi gửi. `user_text{text, draft:true}` (STT thiết bị) cũng
chỉ trả `heard{draft:true}` sau khi sửa.

**Sửa STT bằng model rẻ:** mọi kết quả STT (server hoặc thiết bị, khi `settings.sttFix != false`) được model tiện ích
sửa theo ngữ cảnh (môn của folder, thuật ngữ trong giáo trình, 2 câu gần nhất của trợ lý) trước khi dùng.
`heard{text, raw?}`: `raw` là bản gốc khi có sửa. Quá 2,5 giây thì dùng bản gốc.

## 8. Folder và phiên

Folder do người dùng đặt tên, gắn đúng một môn (`subject` là id skill: `toan`, `ly`, `hoa`, `van`, `anh-ssat`,
`duc`, `tin-cpp`, `chung`). Một folder có nhiều phiên; phiên không folder vẫn hợp lệ.

| Hướng | `t` | Trường |
|---|---|---|
| máy → | `list_folders` | — |
| → máy | `folders` | `items:[{id,name,subject,sessionCount,updatedAt,hasKnowledge:boolean,ingest?:{jobId,state,done,total}}]` |
| máy → | `create_folder` | `name, subject` → server trả `folder_created{id}` rồi `folders` mới |
| máy → | `rename_folder` / `delete_folder` | `id, name?`. Xóa folder: phiên bên trong thành không folder; kiến thức và đề của folder bị xóa. |
| máy → | `move_session` | `sessionId, folderId\|null` |
| máy → | `rename_session` / `delete_session` | `sessionId, title?` |
| máy → | `new_session` | `folderId?, mode?, kickoff?:"review_quiz"` → `ready` của phiên mới (kickoff: server tự mở lượt đầu, xem mục 12) |
| máy → | `list_sessions` | `folderId?` (thiếu = tất cả) |
| → máy | `sessions` | `items:[{sessionId,title,updatedAt,folderId?,mode,preview?}]` |

`hello` thêm `folderId?` (cho phiên mới). `ready` thêm `folderId?`, `mode`, `models`, `model`, `effort` (mục 10).
Phiên cũ được tự xếp vào folder mặc định theo môn nếu suy được từ tên phiên; không suy được thì để trống.

**Tên phiên:** nếu sau lượt đầu chưa có `@title`, server nhờ model tiện ích đặt tên ngắn (<= 40 ký tự,
`Môn · Chủ đề`) rồi phát `title`. `tutor/CLAUDE.md` không còn bắt buộc `@title`.

**Memory theo folder:** phiên trong folder ghi `memory_note` vào `tutor/memory/folders/<folderId>.md` (cùng khuôn với
file môn) và tóm tắt `[memory]` đầu phiên gồm: cách giảng hợp, chỗ yếu của folder, **vị trí đang học trên trường và
các chủ đề đã/chưa học** lấy từ `knowledge.json` (mục 11). Phiên không folder dùng file môn như v1.
`memory_note` thêm `kind:"understood"` (hiểu sau khi giảng lại, kèm cách giảng đã hiệu quả) và `topicId?` (id chủ đề
trong giáo trình folder).

**Compact:** khi ngữ cảnh phiên vượt ngưỡng (`BELUGA_COMPACT_TOKENS`, mặc định 120000 token đầu vào ở lượt gần nhất),
server nhờ model tiện ích tóm tắt nhật ký phiên, mở session Claude mới cho cùng `sessionId`, đính bản tóm tắt vào
lượt kế, phát `notice{code:"compacted"}`.

## 9. Tải tệp

`POST /upload?name=<tên>` · header `Authorization: Bearer <pairToken>`, `Content-Type: image/jpeg|image/png|application/pdf`,
body thô (tối đa 30 MB) → `{fileId, kind:"image"|"pdf", bytes, name}`. Lưu ở `server/data/uploads/`.
`GET /file/<fileId>?token=<pairToken>` trả lại tệp (để hiện ảnh trong lịch sử chat).
App thu nhỏ ảnh về cạnh dài tối đa 2000 px, JPEG chất lượng 0,8 trước khi tải.

## 10. Model, nhà cung cấp, effort, usage

Claude Code là harness; model nào nói được giao thức Anthropic Messages đều chạy được bằng cách đặt
`ANTHROPIC_BASE_URL` + `ANTHROPIC_AUTH_TOKEN` + `--model` cho tiến trình `claude` của phiên.

Danh mục ở `server/models.json` (không có thì dùng mặc định dựng sẵn; mẫu `server/models.example.json`):

```json
[{"id":"sonnet","label":"Claude Sonnet","provider":"claude","model":"sonnet","efforts":["low","medium","high"],"defaultEffort":"medium","vision":true},
 {"id":"deepseek","label":"DeepSeek","provider":"anthropic-compatible","baseUrl":"https://api.deepseek.com/anthropic","keyEnv":"DEEPSEEK_API_KEY","model":"deepseek-chat","efforts":[],"vision":false}]
```

`provider:"claude"` = đăng nhập gói Claude (có theo dõi usage). Mục có `keyEnv` mà thiếu key thì không xuất hiện.
`BELUGA_MODEL_MAP` (v1) vẫn được tôn trọng.

- `ready.models: [{id,label,provider,efforts:[string],defaultEffort?,vision,tracked:boolean}]`, `ready.model`, `ready.effort`.
- `settings{model:<id>, effort?:"low"|"medium"|"high"|"xhigh"|"max"|null, mode, lang, rigor, sttFix?}`. Đổi model/effort áp dụng từ lượt sau.
- `get_usage` → `usage{claude:{session:{inputTokens,outputTokens,cacheReadTokens,cacheWriteTokens,costUsd,turns},today:{...},week:{...},limit?:{status,resetsAt,type}},other:{today:{costUsd,inputTokens,outputTokens}},ingest:{today:{costUsd},total:{costUsd}}}`.
  Server cũng tự phát `usage` sau mỗi `turn_end` của model `tracked`. `costUsd` của gói Claude là quy đổi tham khảo
  (từ `total_cost_usd` trong `result` của stream-json), không phải tiền thật. Lưu ở `server/data/usage.jsonl`.

## 11. Ma trận đề: nạp đề → kiến thức của folder

1. App tải 10–30 ảnh hoặc 1–2 PDF qua `/upload`, rồi gửi `ingest_start{folderId, files:[fileId], goal?:string}`.
2. Server đọc từng lô trang bằng model vision rẻ (OpenRouter, Haiku, reasoning high), rồi gộp thành MỘT file máy đọc
   `server/data/folders/<folderId>/knowledge.json`. Nạp thêm lần sau thì gộp vào file cũ.
3. Trong lúc chạy server phát tiến độ; xong thì phát `knowledge`.

| Hướng | `t` | Trường |
|---|---|---|
| máy → | `ingest_start` | `folderId, files, goal?` (`goal`: mục đích người dùng, ví dụ "ôn giữa kỳ 1", "thi vào 10 chuyên") |
| máy → | `ingest_cancel` | `folderId` |
| → máy | `ingest` | `folderId, jobId, state:"queued"\|"reading"\|"merging"\|"done"\|"error"\|"cancelled", done, total, note, found?:{questions,topics}, costUsd?` (`note`: một dòng tiếng Việt đang làm gì, ví dụ "Đang đọc trang 7/24: Đề ôn giữa kỳ số 3") |
| máy → | `get_knowledge` | `folderId` |
| → máy | `knowledge` | `folderId, data:<knowledge.json>\|null, ingest?:{...như trên}` |

`knowledge.json`:

```json
{"v":1,"folderId":"…","subject":"toan","grade":"9","goal":"ôn giữa kỳ 1","updatedAt":"2026-11-15T02:00:00Z",
 "sources":[{"fileId":"…","name":"de-on-3.pdf","kind":"exam|review|worksheet","title":"Đề ôn giữa kỳ số 3","school":null,"date":null,"pages":2,"questions":18}],
 "curriculum":[{"id":"c1","title":"Căn bậc hai","topics":[
   {"id":"c1.t1","title":"Rút gọn biểu thức chứa căn","summary":"…","count":14,"share":0.12,
    "forms":[{"id":"c1.t1.f1","title":"Rút gọn rồi tính giá trị","method":"Các bước giải ngắn gọn…","count":6,
              "levels":{"nb":1,"th":3,"vd":2,"vdc":0},"example":{"md":"Rút gọn $A=…$"},"refs":[{"fileId":"…","q":"3a"}]}]}]}],
 "structures":[{"id":"s1","title":"Đề giữa kỳ 1 (trắc nghiệm + tự luận)","basedOn":["fileId"],"durationMin":90,"totalPoints":10,
   "sections":[{"id":"A","title":"Trắc nghiệm","kind":"mcq|truefalse|short|essay","count":12,"pointsEach":0.25,"instructions":"…"}],
   "matrix":[{"section":"A","topicId":"c1.t1","formId":"c1.t1.f1","level":"th","count":2,"points":0.5}]}],
 "schoolProgress":{"coveredTopicIds":["c1.t1"],"currentTopicId":"c1.t2","confidence":0.7,"note":"Suy từ đề ôn mới nhất…"},
 "stats":{"questions":118,"byLevel":{"nb":30,"th":44,"vd":34,"vdc":10},"byKind":{"mcq":60,"essay":58}}}
```

## 12. Review của folder

| Hướng | `t` | Trường |
|---|---|---|
| máy → | `get_review` | `folderId` |
| → máy | `review` | `folderId, topics:[{topicId,title,chapter,state:"new"\|"learning"\|"weak"\|"solid",wrong,right,lastSeen?}], weak:[{topic,note,count,lastSeen,due}], due:[{topic,note,due}], weekly:{from,to,md}\|null, sessions:[{sessionId,title,updatedAt,mode,steps}], updatedAt` |
| máy → | `get_weekly` | `folderId` → server nhờ model tiện ích viết tóm tắt tuần (học gì, sai gì, nên ôn gì), lưu lại, phát `review` mới. |

`topics` là chồng memory của folder lên giáo trình (`knowledge.curriculum`); chưa có giáo trình thì suy từ memory.
**Quiz ôn nhanh:** `new_session{folderId, mode, kickoff:"review_quiz"}`: server tự gửi lượt đầu cho Claude
(`[ôn nhanh: <các mục tới hạn và đang yếu>]`), gia sư hỏi từng câu một, chấm, gọi `memory_note`.
**Xem lại bài giảng cũ:** `open_session` rồi dựng lại từ `snapshot`; app cho lướt các bước (mục 14).

## 13. Tạo đề

| Hướng | `t` | Trường |
|---|---|---|
| máy → | `gen_exam` | `folderId, structureId, note?` (`note`: yêu cầu thêm, ví dụ "khó hơn một chút", "tập trung chương 2") |
| → máy | `exam` | `folderId, examId, state:"writing"\|"checking"\|"done"\|"error", note, title?` |
| máy → | `list_exams` | `folderId` |
| → máy | `exams` | `folderId, items:[{examId,title,structureId,createdAt,questions}]` |
| máy → | `delete_exam` | `examId` |

Đề được sinh đúng cấu trúc `structures[structureId]` (số câu, loại câu, mức độ, điểm theo `matrix`), câu hỏi mới
(không chép lại đề gốc), lưu `server/data/folders/<folderId>/exams/<examId>.json`.
`GET /exam/<examId>?token=<pairToken>` trả một trang HTML A4 in được (KaTeX từ `/board/vendor/`), gồm đề rồi
**đáp án ở trang cuối** (`page-break-before`). App mở trang này trong WKWebView ẩn, in ra PDF (A4) và mở bảng chia sẻ.
Khi trang vẽ xong toàn bộ công thức nó đặt `document.documentElement.dataset.ready = "1"` và gọi
`window.webkit?.messageHandlers?.board?.postMessage({type:"exam_ready"})`.

## 14. Gói web (`shared/board/`, `version` 3)

- **Bảng (live), `index.html`:** mỗi bước là một trang, **vuốt ngang trái/phải** để tới/lui (scroll-snap), có chấm vị trí.
  Bước mới tới thì tự trượt tới bước đó, trừ khi người dùng đang xem lại bước cũ (khi đó hiện nút "Bước mới").
  `board.goto(i)`, `board.next()`, `board.prev()`; postMessage `{type:"stepChanged", index, count, following:boolean}`.
- **Chat, `chat.html`:** `chat.reset()`, `chat.apply(event)` với `user|chat_delta|chat|show|ask|done|turn_end|state|notice`,
  `chat.load(events[])` (lịch sử, không animation), `chat.setTheme`, `chat.setLang`, `chat.setFileBase(url, token)`.
  postMessage: `{type:"ready",version}`, `{type:"action", action:"copy"|"retry", turn, text?}`, `{type:"ask_tap", prompt}`.
  Sự kiện live (`say`, `step`, `show` có `step>=0`) nếu lọt vào chat thì vẽ gọn (lời nói thành đoạn chữ, bước thành tiêu đề).
- **Kiến thức, `knowledge.html`:** `knowledge.render(data, review?)` vẽ: cây giáo trình, ma trận nhiệt chủ đề × mức độ,
  cột dạng bài, vị trí đang học trên trường, danh sách cấu trúc đề (nút "Tạo đề"), tiến độ chồng lên (nếu có `review`).
  `knowledge.progress(ingestEvent)` hiện tiến độ nạp đề. postMessage: `{type:"gen_exam", structureId}`,
  `{type:"open_topic", topicId, title}`, `{type:"ready"}`.
- **Khối mới** `interactive` `{html, height?, fallback}`: minh họa/trò chơi tương tác nhỏ, chạy trong `iframe sandbox="allow-scripts"`
  (`srcdoc`), không mạng. Chỉ dùng khi người dùng yêu cầu.
- Mọi trang: `setTheme("light"|"dark")`, `setLang("vi"|"en")`, chữ và nền theo token chung trong `board.css`.

## 15. Lời chào

`get_greeting{localTime:"2026-11-15T01:45", name?}` → `greeting{text, sub?, doodle}`.
`text`: một câu chào ngắn, dễ thương, xưng tớ gọi cậu, theo giờ và tiến độ (<= 60 ký tự); `sub`: gợi ý việc nên làm
(ví dụ "Có 2 mục Toán tới hạn ôn"), tùy chọn. `doodle` ∈ `cat_walk|cat_sleep|whale_swim|bird_hop|snail|paper_plane`
(app vẽ nét SVG chuyển động; tên lạ thì dùng `cat_walk`). Model tiện ích viết, cache 3 giờ; app có sẵn danh sách dự phòng.

