import Foundation
import BelugaCore

/// Sample server frames for protocol v2 screens in `-mock 1` (folders, chat, knowledge, review, exams, usage).
enum MockData {
    static func parse(_ s: String) -> JSONValue { (try? JSONValue.parse(s)) ?? .null }

    static let models: JSONValue = parse(#"""
    [{"id":"sonnet","label":"Claude Sonnet","provider":"claude","efforts":["low","medium","high"],"defaultEffort":"medium","vision":true,"tracked":true},
     {"id":"opus","label":"Claude Opus","provider":"claude","efforts":["low","medium","high","xhigh","max"],"defaultEffort":"low","vision":true,"tracked":true},
     {"id":"haiku","label":"Claude Haiku","provider":"claude","efforts":[],"vision":true,"tracked":true},
     {"id":"deepseek","label":"DeepSeek Flash","provider":"anthropic-compatible","efforts":[],"vision":true,"tracked":false}]
    """#)

    static let folders: [JSONValue] = parse(#"""
    [{"id":"f-toan","name":"Toán 9 · Giữa kỳ 1","subject":"toan","kind":"toan","about":"Ôn giữa kỳ 1 theo đề của trường","sessionCount":4,"updatedAt":"2026-11-15T01:20:00Z","hasKnowledge":true},
     {"id":"f-hoa","name":"Hóa 9","subject":"hoa","kind":"hoa","about":"","sessionCount":2,"updatedAt":"2026-11-13T19:42:00Z","hasKnowledge":false},
     {"id":"f-anh","name":"Tiếng Anh trên lớp","subject":"anh-ssat","kind":"anh-ssat","about":"Anh 9 theo sách Global Success, cô hay cho ngữ pháp và viết lại câu","sessionCount":1,"updatedAt":"2026-11-12T21:05:00Z","hasKnowledge":false}]
    """#).array ?? []

    static let sessions: [JSONValue] = parse(#"""
    [{"sessionId":"mock-0001","title":"Toán · Bất phương trình bậc hai","updatedAt":"2026-11-14T20:10:00Z","folderId":"f-toan","mode":"live","preview":"Xét dấu tam thức rồi kết luận tập nghiệm"},
     {"sessionId":"mock-0006","title":"Toán · Rút gọn biểu thức chứa căn","updatedAt":"2026-11-15T01:20:00Z","folderId":"f-toan","mode":"chat","preview":"Điều kiện xác định trước khi rút gọn"},
     {"sessionId":"mock-0007","title":"Toán · Hệ thức Vi-ét","updatedAt":"2026-11-11T20:00:00Z","folderId":"f-toan","mode":"chat","preview":"Tổng và tích hai nghiệm"},
     {"sessionId":"mock-0008","title":"Toán · Ôn nhanh tuần 2","updatedAt":"2026-11-09T19:30:00Z","folderId":"f-toan","mode":"chat","preview":"5 câu, đúng 4"},
     {"sessionId":"mock-0002","title":"Hóa · Cân bằng phương trình phản ứng","updatedAt":"2026-11-13T19:42:00Z","folderId":"f-hoa","mode":"live"},
     {"sessionId":"mock-0009","title":"Hóa · Nồng độ mol","updatedAt":"2026-11-10T18:00:00Z","folderId":"f-hoa","mode":"chat"},
     {"sessionId":"mock-0003","title":"Anh · Thì hiện tại hoàn thành","updatedAt":"2026-11-12T21:05:00Z","folderId":"f-anh","mode":"chat"},
     {"sessionId":"mock-0004","title":"Lý · Chuyển động ném ngang","updatedAt":"2026-11-10T18:30:00Z","mode":"live"},
     {"sessionId":"mock-0005","title":"Sử · Cách mạng tháng Tám 1945","updatedAt":"2026-11-08T20:15:00Z","mode":"chat"}]
    """#).array ?? []

    /// The server's real sample (`server/tests/fixtures/knowledge.sample.json`, copied as `mock-knowledge.json`),
    /// else the small inline one below.
    static let knowledge: JSONValue = {
        if let url = Bundle.main.url(forResource: "mock-knowledge", withExtension: "json"),
           let data = try? Data(contentsOf: url), let v = try? JSONValue.parse(data), v.object != nil { return v }
        return inlineKnowledge
    }()

    static let inlineKnowledge: JSONValue = parse(#"""
    {"v":1,"folderId":"f-toan","subject":"toan","grade":"9","goal":"ôn giữa kỳ 1","updatedAt":"2026-11-15T02:00:00Z",
     "sources":[{"fileId":"a","name":"de-on-1.pdf","kind":"exam","title":"Đề ôn giữa kỳ số 1","pages":2,"questions":18},
                {"fileId":"b","name":"de-on-2.pdf","kind":"exam","title":"Đề ôn giữa kỳ số 2","pages":2,"questions":17},
                {"fileId":"c","name":"phieu-can.jpg","kind":"worksheet","title":"Phiếu bài tập căn thức","pages":1,"questions":12}],
     "curriculum":[
       {"id":"c1","title":"Căn bậc hai, căn bậc ba","topics":[
         {"id":"c1.t1","title":"Rút gọn biểu thức chứa căn","summary":"","count":14,"share":0.18,
          "forms":[{"id":"c1.t1.f1","title":"Rút gọn rồi tính giá trị","method":"","count":6,"levels":{"nb":1,"th":3,"vd":2,"vdc":0}},
                   {"id":"c1.t1.f2","title":"Tìm x để biểu thức nguyên","method":"","count":3,"levels":{"nb":0,"th":0,"vd":1,"vdc":2}}]},
         {"id":"c1.t2","title":"Trục căn thức ở mẫu","summary":"","count":8,"share":0.1,
          "forms":[{"id":"c1.t2.f1","title":"Nhân liên hợp","method":"","count":8,"levels":{"nb":2,"th":4,"vd":2,"vdc":0}}]},
         {"id":"c1.t3","title":"Điều kiện xác định","summary":"","count":9,"share":0.11,
          "forms":[{"id":"c1.t3.f1","title":"Tìm điều kiện của x","method":"","count":9,"levels":{"nb":6,"th":3,"vd":0,"vdc":0}}]}]},
       {"id":"c2","title":"Hàm số bậc nhất","topics":[
         {"id":"c2.t1","title":"Đồ thị và hệ số góc","summary":"","count":11,"share":0.14,
          "forms":[{"id":"c2.t1.f1","title":"Vẽ đồ thị, tìm giao điểm","method":"","count":11,"levels":{"nb":3,"th":5,"vd":3,"vdc":0}}]},
         {"id":"c2.t2","title":"Hai đường thẳng song song, cắt nhau","summary":"","count":7,"share":0.09,
          "forms":[{"id":"c2.t2.f1","title":"Tìm m theo điều kiện","method":"","count":7,"levels":{"nb":1,"th":3,"vd":3,"vdc":0}}]}]},
       {"id":"c3","title":"Hệ thức lượng trong tam giác vuông","topics":[
         {"id":"c3.t1","title":"Tỉ số lượng giác","summary":"","count":12,"share":0.15,
          "forms":[{"id":"c3.t1.f1","title":"Tính cạnh và góc","method":"","count":12,"levels":{"nb":4,"th":5,"vd":2,"vdc":1}}]},
         {"id":"c3.t2","title":"Bài toán thực tế","summary":"","count":6,"share":0.08,
          "forms":[{"id":"c3.t2.f1","title":"Đo chiều cao, khoảng cách","method":"","count":6,"levels":{"nb":0,"th":2,"vd":3,"vdc":1}}]}]}],
     "structures":[
       {"id":"s1","title":"Đề giữa kỳ 1 (trắc nghiệm + tự luận)","basedOn":["a","b"],"durationMin":90,"totalPoints":10,
        "sections":[{"id":"A","title":"Trắc nghiệm","kind":"mcq","count":12,"pointsEach":0.25},{"id":"B","title":"Tự luận","kind":"essay","count":5,"pointsEach":1.4}],
        "matrix":[]},
       {"id":"s2","title":"Kiểm tra 45 phút (tự luận)","basedOn":["c"],"durationMin":45,"totalPoints":10,
        "sections":[{"id":"A","title":"Tự luận","kind":"essay","count":4,"pointsEach":2.5}],"matrix":[]}],
     "schoolProgress":{"coveredTopicIds":["c1.t1","c1.t2","c1.t3","c2.t1"],"currentTopicId":"c2.t2","confidence":0.7,"note":"Suy từ đề ôn mới nhất: lớp đang học hai đường thẳng song song."},
     "stats":{"questions":118,"byLevel":{"nb":30,"th":44,"vd":34,"vdc":10},"byKind":{"mcq":60,"essay":58}}}
    """#)

    static func review(folderId: String, weekly: Bool) -> JSONValue {
        var v = parse(#"""
        {"t":"review","updatedAt":"2026-11-15T02:00:00Z",
         "topics":[{"topicId":"c1.t1","title":"Rút gọn biểu thức chứa căn","chapter":"Căn bậc hai","state":"weak","wrong":3,"right":2,"lastSeen":"2026-11-14"},
                   {"topicId":"c1.t2","title":"Trục căn thức ở mẫu","chapter":"Căn bậc hai","state":"solid","wrong":0,"right":5,"lastSeen":"2026-11-12"},
                   {"topicId":"c1.t3","title":"Điều kiện xác định","chapter":"Căn bậc hai","state":"learning","wrong":1,"right":2,"lastSeen":"2026-11-13"},
                   {"topicId":"c2.t1","title":"Đồ thị và hệ số góc","chapter":"Hàm số bậc nhất","state":"learning","wrong":1,"right":1,"lastSeen":"2026-11-11"},
                   {"topicId":"c2.t2","title":"Hai đường thẳng song song, cắt nhau","chapter":"Hàm số bậc nhất","state":"new","wrong":0,"right":0},
                   {"topicId":"c3.t1","title":"Tỉ số lượng giác","chapter":"Hệ thức lượng","state":"new","wrong":0,"right":0},
                   {"topicId":"c3.t2","title":"Bài toán thực tế","chapter":"Hệ thức lượng","state":"new","wrong":0,"right":0}],
         "weak":[{"topic":"Rút gọn biểu thức chứa căn","note":"Quên điều kiện xác định trước khi rút gọn.","count":3,"lastSeen":"2026-11-14","due":"2026-11-15"},
                 {"topic":"Đồ thị và hệ số góc","note":"Nhầm hệ số góc với tung độ gốc.","count":1,"lastSeen":"2026-11-11","due":"2026-11-18"}],
         "due":[{"topic":"Rút gọn biểu thức chứa căn","note":"Ôn lần 2","due":"2026-11-15"},
                {"topic":"Điều kiện xác định","note":"Ôn lần 1","due":"2026-11-15"}],
         "weekly":null,
         "sessions":[{"sessionId":"mock-0001","title":"Toán · Bất phương trình bậc hai","updatedAt":"2026-11-14T20:10:00Z","mode":"live","steps":4},
                     {"sessionId":"mock-0006","title":"Toán · Rút gọn biểu thức chứa căn","updatedAt":"2026-11-15T01:20:00Z","mode":"chat","steps":0}]}
        """#)
        if case .object(var o) = v {
            o["folderId"] = .string(folderId)
            if weekly {
                o["weekly"] = ["from": "2026-11-09", "to": "2026-11-15",
                               "md": "**Tuần này** cậu học 4 buổi, chủ yếu căn thức.\n\n- Vững: trục căn thức ở mẫu.\n- Còn yếu: rút gọn khi quên điều kiện xác định.\n- Nên ôn: đồ thị hàm số bậc nhất trước buổi kiểm tra."]
            }
            v = .object(o)
        }
        return v
    }

    static let exams: [JSONValue] = parse(#"""
    [{"examId":"e-2","title":"Đề giữa kỳ 1 · Lần 2","structureId":"s1","createdAt":"2026-11-14T21:00:00Z","questions":17},
     {"examId":"e-1","title":"Kiểm tra 45 phút · Căn thức","structureId":"s2","createdAt":"2026-11-12T20:30:00Z","questions":4}]
    """#).array ?? []

    /// The real catalogue (`server/voices.json`), all with preview clips.
    static let voiceItems: JSONValue = parse(#"""
    [{"id":"5yEPTrA0uAhZsBEhHjfi","name":"Chi","gender":"female","accent":"north","desc":"Nữ trẻ miền Bắc, ấm, tự nhiên như đang trò chuyện.","preview":true},
     {"id":"0ggMuQ1r9f9jqBu50nJn","name":"Thắm","gender":"female","accent":"north","desc":"Nữ trẻ miền Bắc, nhẹ nhàng, gần gũi.","preview":true},
     {"id":"qByVAGjXwGlkcRDJoiHg","name":"Kiều Linh","gender":"female","accent":"south","desc":"Nữ trẻ Sài Gòn, rõ ràng, tự tin.","preview":true},
     {"id":"8PEPqUGgvwN1Fk9KR8jV","name":"An Nhiên","gender":"female","accent":"south","desc":"Nữ trẻ miền Nam, tươi sáng, vui vẻ.","preview":true},
     {"id":"K7ewtjKRNtwwt3lKQ6M0","name":"Tony Hoàng","gender":"male","accent":"north","desc":"Nam trẻ miền Bắc, giọng giảng bài rõ ràng.","preview":true},
     {"id":"RxhjHDfpO54FYotYtKpw","name":"Phong","gender":"male","accent":"north","desc":"Nam trẻ miền Bắc, ấm, truyền cảm.","preview":true},
     {"id":"M0rVwr32hdQ5UXpkI3ni","name":"Hào","gender":"male","accent":"south","desc":"Nam trẻ miền Nam, ấm, thân thiện, nhịp vừa.","preview":true},
     {"id":"JxmKvRaNYFidf0N27Vng","name":"Sơn","gender":"male","accent":"central","desc":"Nam trẻ miền Trung, nói chuyện thoải mái.","preview":true}]
    """#)

    /// The v3 catalogue also lists the wait and hello lines (PROTOCOL section 21); every mock voice has them all.
    static func voices(current: String) -> JSONValue {
        let ids: JSONValue = .array(fillers.compactMap { $0["id"] })
        let items: [JSONValue] = (voiceItems.array ?? []).map { v in
            guard case .object(var o) = v else { return v }
            o["fillers"] = ids
            return .object(o)
        }
        return ["t": "voices", "items": .array(items), "current": .string(current), "provider": "genvoice", "fillers": .array(fillers)]
    }

    static let fillers: [JSONValue] = parse(#"""
    [{"id":"short-1","text":"Ừm, để tớ xem nào","when":"short"},
     {"id":"short-2","text":"Chờ tớ một chút nhé","when":"short"},
     {"id":"short-3","text":"Để tớ nghĩ xíu","when":"short"},
     {"id":"long-1","text":"Bài này hơi dài, cậu chờ tớ thêm chút nha","when":"long"},
     {"id":"long-2","text":"Sắp xong rồi đây","when":"long"},
     {"id":"vision-1","text":"Để tớ nhìn kỹ ảnh đã","when":"vision"},
     {"id":"hello-1","text":"A lô, tớ nghe đây","when":"hello"},
     {"id":"hello-2","text":"Tớ đây, mình học gì nào?","when":"hello"}]
    """#).array ?? []

    static let usage: JSONValue = parse(#"""
    {"t":"usage",
     "claude":{"session":{"inputTokens":18400,"outputTokens":2600,"cacheReadTokens":142000,"cacheWriteTokens":9100,"costUsd":0.21,"turns":6},
               "today":{"inputTokens":61000,"outputTokens":9800,"cacheReadTokens":610000,"cacheWriteTokens":32000,"costUsd":0.94,"turns":23},
               "week":{"inputTokens":402000,"outputTokens":61000,"cacheReadTokens":3900000,"cacheWriteTokens":180000,"costUsd":6.12,"turns":141},
               "limit":{"status":"allowed","resetsAt":"2026-11-15T05:00:00Z","type":"five_hour","utilization":0.34}},
     "other":{"today":{"costUsd":0.03,"inputTokens":21000,"outputTokens":4000}},
     "ingest":{"today":{"costUsd":0.08},"total":{"costUsd":0.41}},"exam":{"today":{"costUsd":0.06},"total":{"costUsd":0.22}}}
    """#)

    static let greeting: JSONValue = parse(#"""
    {"t":"greeting","text":"Tối rồi, cùng ôn căn thức nha","sub":"Có 2 mục Toán tới hạn ôn hôm nay","doodle":"cat_walk"}
    """#)

    /// One chat answer, cut into streaming pieces.
    static let chatQuestion = "Giải giúp tớ phương trình x² − 5x + 6 = 0 nhé"
    static let chatAnswer = "Đây là phương trình bậc hai với $a = 1$, $b = -5$, $c = 6$.\n\nTính biệt thức:\n\n$$\\Delta = b^2 - 4ac = 25 - 24 = 1 > 0$$\n\nNên phương trình có hai nghiệm phân biệt:\n\n$$x_1 = \\frac{5 + 1}{2} = 3, \\quad x_2 = \\frac{5 - 1}{2} = 2$$"
    static let chatAnswer2 = "**Kiểm tra lại** bằng Vi-ét: $x_1 + x_2 = 5$ và $x_1 x_2 = 6$, khớp với $-b/a$ và $c/a$. Cậu thử tự giải $x^2 - 7x + 10 = 0$ xem?"
    static let chatBlock: JSONValue = ["type": "math", "tex": "x^2 - 5x + 6 = (x-2)(x-3)", "note": "Phân tích thành nhân tử"]

    static let quizQuestion = "Ôn nhanh nhé! Câu 1/5: rút gọn $A = \\sqrt{12} - 2\\sqrt{3} + \\sqrt{27}$. Cậu viết kết quả cuối thôi là được."

    /// A printable exam page in the shape of `GET /exam/<id>` (no KaTeX offline: formulas in plain text).
    static func examHTML(title: String) -> String {
        """
        <!doctype html><html lang="vi"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width">
        <style>
        body{font-family:-apple-system,"Times New Roman",serif;color:#111;margin:0;font-size:12pt;line-height:1.5}
        h1{font-size:15pt;text-align:center;margin:0 0 2pt}
        .meta{text-align:center;font-size:10pt;color:#444;margin-bottom:12pt}
        h2{font-size:12pt;margin:14pt 0 6pt;border-bottom:1px solid #999;padding-bottom:2pt}
        ol{padding-left:18pt} li{margin:0 0 7pt}
        .opts{display:grid;grid-template-columns:1fr 1fr;gap:2pt 16pt;margin-top:3pt}
        .answers{page-break-before:always}
        table{border-collapse:collapse;width:100%} td,th{border:1px solid #999;padding:3pt 6pt;font-size:10.5pt;text-align:left}
        </style></head><body>
        <h1>\(title)</h1>
        <div class="meta">Môn Toán 9 · Thời gian 90 phút · Thang điểm 10</div>
        <h2>Phần A. Trắc nghiệm (3 điểm)</h2><ol>
        <li>Điều kiện xác định của √(2x − 4) là<div class="opts"><span>A. x ≥ 2</span><span>B. x > 2</span><span>C. x ≤ 2</span><span>D. x ≥ −2</span></div></li>
        <li>Giá trị của √12 − 2√3 + √27 bằng<div class="opts"><span>A. 3√3</span><span>B. 2√3</span><span>C. 5√3</span><span>D. √3</span></div></li>
        <li>Đường thẳng y = 2x − 3 có hệ số góc là<div class="opts"><span>A. −3</span><span>B. 2</span><span>C. 3</span><span>D. −2</span></div></li>
        <li>Trục căn ở mẫu của 1/(√5 − 2) được<div class="opts"><span>A. √5 + 2</span><span>B. √5 − 2</span><span>C. 2 − √5</span><span>D. (√5 + 2)/3</span></div></li>
        <li>Cho tam giác ABC vuông tại A, AB = 3, AC = 4. Khi đó sin B bằng<div class="opts"><span>A. 3/5</span><span>B. 4/5</span><span>C. 3/4</span><span>D. 4/3</span></div></li>
        <li>Hai đường thẳng y = (m − 1)x + 2 và y = 3x − 1 song song khi<div class="opts"><span>A. m = 4</span><span>B. m = 3</span><span>C. m = 2</span><span>D. m = −4</span></div></li>
        </ol>
        <h2>Phần B. Tự luận (7 điểm)</h2><ol>
        <li>(2 điểm) Cho biểu thức P = (√x + 1)/(√x − 1) − 2√x/(x − 1) với x ≥ 0, x ≠ 1. a) Rút gọn P. b) Tìm x để P = 1/2.</li>
        <li>(2 điểm) Cho hàm số y = (m − 2)x + m + 1. a) Tìm m để hàm số đồng biến. b) Tìm m để đồ thị cắt trục tung tại điểm có tung độ bằng 3. c) Vẽ đồ thị với m vừa tìm được.</li>
        <li>(2 điểm) Một cột đèn cao 7 m có bóng trên mặt đất dài 4 m. Tính góc tạo bởi tia nắng và mặt đất (làm tròn đến độ).</li>
        <li>(1 điểm) Tìm các số nguyên x để Q = (√x + 3)/(√x − 2) là số nguyên.</li>
        </ol>
        <div class="answers"><h1>Đáp án và hướng dẫn chấm</h1><div class="meta">\(title)</div>
        <h2>Phần A</h2><table><tr><th>Câu</th><th>1</th><th>2</th><th>3</th><th>4</th><th>5</th><th>6</th></tr>
        <tr><td>Đáp án</td><td>A</td><td>A</td><td>B</td><td>A</td><td>B</td><td>A</td></tr></table>
        <h2>Phần B</h2><ol>
        <li>a) P = (√x − 1)/(√x + 1)… quy đồng mẫu x − 1 rồi rút gọn (1 điểm). b) P = 1/2 ⇔ √x = 3 ⇔ x = 9 (1 điểm).</li>
        <li>a) m &gt; 2. b) m + 1 = 3 ⇔ m = 2, loại vì khi đó hàm không còn bậc nhất; kết luận không có m (cho điểm phần lập luận).</li>
        <li>tan α = 7/4 ⇒ α ≈ 60°.</li>
        <li>Q = 1 + 5/(√x − 2): √x − 2 là ước của 5 ⇒ x ∈ {1, 9, 49}.</li>
        </ol></div>
        <script>document.documentElement.dataset.ready="1";try{window.webkit.messageHandlers.board.postMessage({type:"exam_ready"})}catch(e){}</script>
        </body></html>
        """
    }
}
