/* Mock events for dev.html and the automated browser checks. Not used by the app. */
(function (global) {
  "use strict";
  var seq = 0;
  function S(o) { o.seq = ++seq; o.turn = o.turn || 1; return o; }

  var interactiveHtml =
    '<div style="padding:12px 14px">' +
    '<div style="font-weight:600;margin-bottom:6px">Kéo để đổi hệ số a</div>' +
    '<input id="a" type="range" min="-3" max="3" step="0.5" value="1" style="width:100%">' +
    '<svg id="g" viewBox="0 0 300 150" style="width:100%;height:auto;display:block;margin-top:8px"></svg>' +
    '<div id="t" style="font-size:14px;opacity:.7"></div></div>' +
    '<script>var a=document.getElementById("a"),g=document.getElementById("g"),t=document.getElementById("t");' +
    'function draw(){var k=+a.value,d="";for(var i=0;i<=60;i++){var x=-3+i/10,y=k*(x*x)-1;var px=150+x*45,py=95-y*20;d+=(i?"L":"M")+px.toFixed(1)+" "+py.toFixed(1);}' +
    'g.innerHTML=\'<line x1="0" y1="95" x2="300" y2="95" stroke="currentColor" stroke-opacity=".3"/><line x1="150" y1="0" x2="150" y2="150" stroke="currentColor" stroke-opacity=".3"/><path d="\'+d+\'" fill="none" stroke="currentColor" stroke-width="2.2"/>\';' +
    't.textContent="y = "+k+"x² − 1 · "+(k>0?"bề lõm hướng lên":k<0?"bề lõm hướng xuống":"đường thẳng");}a.oninput=draw;draw();<\/script>';

  var lesson = [
    S({ t: "step", index: 0, title: "Đọc đề" }),
    S({ t: "show", step: 0, block: { type: "text", md: "Giải bất phương trình sau và biểu diễn tập nghiệm trên trục số." } }),
    S({ t: "show", step: 0, block: { type: "math", tex: "x^2 - 5x + 6 > 0" } }),
    S({ t: "show", step: 0, block: { type: "text", md: "Đây là **bất phương trình bậc hai** với $a = 1$, $b = -5$, $c = 6$." } }),
    S({ t: "step", index: 1, title: "Tìm nghiệm của tam thức" }),
    S({ t: "show", step: 1, block: { type: "math", tex: "\\Delta = b^2 - 4ac = 25 - 24 = 1 > 0" } }),
    S({ t: "show", step: 1, block: { type: "math", tex: "x_1 = \\frac{5 - 1}{2} = 2, \\qquad x_2 = \\frac{5 + 1}{2} = 3" }, note: "" }),
    S({ t: "show", step: 1, block: { type: "plot", fn: ["x^2-5*x+6"], x: [-1, 6], y: [-1, 6], points: [{ x: 2, y: 0, label: "2" }, { x: 3, y: 0, label: "3" }] } }),
    S({ t: "step", index: 2, title: "Xét dấu" }),
    S({ t: "show", step: 2, block: { type: "table", head: ["$x$", "$-\\infty$", "", "2", "", "3", "", "$+\\infty$"], rows: [["$f(x)$", "", "+", "0", "−", "0", "+", ""]] } }),
    S({ t: "show", step: 2, block: { type: "text", md: "Quy tắc: **trong trái, ngoài cùng** — giữa hai nghiệm thì trái dấu với $a$, ngoài hai nghiệm thì cùng dấu với $a$.\n\n- $a = 1 > 0$\n- Ngoài khoảng $(2;3)$: $f(x) > 0$\n- Trong khoảng $(2;3)$: $f(x) < 0$" } }),
    S({ t: "show", step: 2, block: { type: "interactive", html: interactiveHtml, fallback: "Thử đổi hệ số $a$ để thấy bề lõm của parabol." } }),
    S({ t: "step", index: 3, title: "Một ví dụ ngoài Toán" }),
    S({ t: "show", step: 3, block: { type: "chem", tex: "2H2 + O2 -> 2H2O" }, note: "" }),
    S({ t: "show", step: 3, block: { type: "code", lang: "cpp", code: "int a = 1, b = -5, c = 6;\ndouble d = b*b - 4*a*c;\nif (d > 0) cout << \"2 nghiem\";", highlight: [2] } }),
    S({ t: "show", step: 3, block: { type: "vocab", word: "inequality", ipa: "/ˌɪnɪˈkwɒləti/", pos: "n.", meaning: "bất đẳng thức, bất phương trình", example: "Solve the inequality $x^2-5x+6>0$." } }),
    S({ t: "step", index: 4, title: "Mốc lịch sử" }),
    S({ t: "show", step: 4, block: { type: "timeline", items: [{ when: "1637", what: "Descartes dùng $x, y$ cho ẩn số" }, { when: "1799", what: "Gauss chứng minh định lý cơ bản của đại số" }] } }),
    S({ t: "show", step: 4, block: { type: "svg", svg: '<svg viewBox="0 0 200 60" xmlns="http://www.w3.org/2000/svg"><line x1="10" y1="30" x2="190" y2="30" stroke="currentColor"/><circle cx="80" cy="30" r="5" fill="none" stroke="currentColor" stroke-width="2"/><circle cx="120" cy="30" r="5" fill="none" stroke="currentColor" stroke-width="2"/><path d="M10 30H75M125 30H190" stroke="currentColor" stroke-width="4"/><text x="80" y="52" font-size="12" text-anchor="middle" fill="currentColor">2</text><text x="120" y="52" font-size="12" text-anchor="middle" fill="currentColor">3</text><script>alert(1)</script></svg>' } }),
    S({ t: "show", step: 4, block: { type: "review_offer", topic: "Xét dấu tam thức bậc hai", subject: "Toán", lastSeen: "2026-11-10", note: "Lần trước cậu nhầm chia hai vế cho $x$.", fallback: "Ôn lại: xét dấu tam thức" } }),
    S({ t: "step", index: 5, title: "Kết luận" }),
    S({ t: "show", step: 5, block: { type: "answer", md: "$S = (-\\infty; 2) \\cup (3; +\\infty)$" } }),
    S({ t: "show", step: 5, block: { type: "unknown_kind", fallback: "Khối lạ thì vẽ *fallback*." } }),
    S({ t: "ask", kind: "check", prompt: "Nếu đổi dấu thành $<$ thì tập nghiệm là gì?" }),
    S({ t: "done" })
  ];

  var longAnswer = [
    "## Giải bất phương trình bậc hai",
    "",
    "Mình cùng làm từng bước nhé. Xét tam thức $f(x) = x^2 - 5x + 6$ với hệ số $a = 1 > 0$.",
    "",
    "### Bước 1: Tìm nghiệm",
    "",
    "Tính biệt thức:",
    "",
    "$$\\Delta = b^2 - 4ac = (-5)^2 - 4 \\cdot 1 \\cdot 6 = 1$$",
    "",
    "Vì $\\Delta > 0$ nên tam thức có hai nghiệm phân biệt $x_1 = 2$ và $x_2 = 3$.",
    "",
    "### Bước 2: Bảng xét dấu",
    "",
    "| $x$ | $(-\\infty; 2)$ | $(2; 3)$ | $(3; +\\infty)$ |",
    "|---|:---:|:---:|:---:|",
    "| $f(x)$ | $+$ | $-$ | $+$ |",
    "",
    "Ghi nhớ quy tắc **\"trong trái, ngoài cùng\"**:",
    "",
    "1. Ngoài khoảng hai nghiệm: $f(x)$ *cùng dấu* với $a$.",
    "2. Trong khoảng hai nghiệm: $f(x)$ *trái dấu* với $a$.",
    "   - Ví dụ $x = 2{,}5$: $f(2{,}5) = -0{,}25 < 0$.",
    "",
    "> Mẹo: vẽ nhanh parabol quay lên rồi nhìn phần nằm trên trục hoành.",
    "",
    "### Bước 3: Kết luận",
    "",
    "Tập nghiệm là $S = (-\\infty; 2) \\cup (3; +\\infty)$.",
    "",
    "Nếu muốn kiểm tra bằng máy, cậu có thể chạy đoạn code sau:",
    "",
    "```python",
    "for x in [1, 2.5, 4]:",
    "    print(x, x*x - 5*x + 6 > 0)",
    "```",
    "",
    "Còn trong Hoá, phương trình đốt cháy hiđro là $\\ce{2H2 + O2 -> 2H2O}$ — cũng cần *cân bằng* như hai vế của bất phương trình vậy."
  ].join("\n");

  // Split into irregular chunks like a real stream (cuts inside $..$, **..** and table rows).
  function chunks(text, seed) {
    var out = [], i = 0, r = seed || 7;
    while (i < text.length) {
      r = (r * 9301 + 49297) % 233280;
      var n = 3 + Math.floor(r / 233280 * 14);
      out.push(text.slice(i, i + n)); i += n;
    }
    return out;
  }

  var chatHistory = [
    { t: "user", seq: 1, turn: 1, text: "Giải giúp tớ bài này với", files: [{ fileId: "demo-img", kind: "image", name: "de-bai.jpg" }, { fileId: "demo-pdf", kind: "pdf", name: "de-on-giua-ky-1.pdf" }], via: "text" },
    { t: "state", state: "thinking" },
    { t: "chat", seq: 2, turn: 1, md: "Đề bài là bất phương trình bậc hai. Tớ vẽ lên đây cho dễ nhìn:" },
    { t: "show", seq: 3, turn: 1, step: -1, block: { type: "math", tex: "x^2 - 5x + 6 > 0" } },
    { t: "chat", seq: 4, turn: 1, md: "Cậu thử tìm nghiệm của tam thức $x^2 - 5x + 6$ trước nhé." },
    { t: "ask", seq: 5, turn: 1, kind: "next", prompt: "Nghiệm của tam thức là gì?" },
    { t: "turn_end", seq: 6, turn: 1 },
    { t: "user", seq: 7, turn: 2, text: "x = 2 và x = 3", via: "voice" },
    { t: "chat", seq: 8, turn: 2, md: "Chính xác! Giờ xét dấu nhé." },
    { t: "show", seq: 9, turn: 2, step: -1, block: { type: "plot", fn: ["x^2-5*x+6"], x: [-1, 6], y: [-1, 6], points: [{ x: 2, y: 0, label: "2" }, { x: 3, y: 0, label: "3" }] } },
    { t: "notice", code: "compacted", text: "Đã thu gọn ngữ cảnh phiên" },
    { t: "turn_end", seq: 10, turn: 2 },
    { t: "user", seq: 11, turn: 3, text: "Cho tớ một trò tương tác để hiểu hệ số a", via: "text" },
    { t: "chat", seq: 12, turn: 3, md: "Đây, cậu kéo thanh trượt xem parabol đổi thế nào:" },
    { t: "show", seq: 13, turn: 3, step: -1, block: { type: "interactive", html: interactiveHtml, fallback: "Minh họa hệ số a" } },
    { t: "show", seq: 14, turn: 3, step: -1, block: { type: "review_offer", topic: "Xét dấu tam thức bậc hai", subject: "Toán", lastSeen: "2026-11-10", fallback: "Ôn lại" } },
    // A live-mode turn leaking into chat: drawn compactly.
    { t: "say", seq: 15, turn: 3, text: "Ở chế độ live tớ sẽ nói câu này." },
    { t: "step", seq: 16, turn: 3, index: 0, title: "Bước từ chế độ live" },
    { t: "show", seq: 17, turn: 3, step: 0, block: { type: "answer", md: "$S = (-\\infty;2)\\cup(3;+\\infty)$" } },
    { t: "done", seq: 18, turn: 3 },
    { t: "turn_end", seq: 19, turn: 3 }
  ];

  function streamTurn(turn) {
    var evs = [{ t: "user", seq: 100 + turn, turn: turn, text: "Giải chi tiết và giải thích quy tắc giúp tớ, thêm một ví dụ code và một câu Hoá nữa nhé. Cảm ơn cậu nhiều!", via: "text" },
      { t: "state", state: "thinking" }];
    chunks(longAnswer, turn).forEach(function (c) { evs.push({ t: "chat_delta", turn: turn, text: c }); });
    evs.push({ t: "chat", seq: 200 + turn, turn: turn, md: longAnswer });
    evs.push({ t: "ask", seq: 201 + turn, turn: turn, kind: "check", prompt: "Đổi dấu thành < thì sao?" });
    evs.push({ t: "turn_end", seq: 202 + turn, turn: turn });
    evs.push({ t: "state", state: "idle" });
    return evs;
  }

  function ingestSteps(folderId) {
    var total = 24, out = [], titles = ["Đề ôn giữa kỳ số 1", "Đề ôn giữa kỳ số 2", "Đề ôn giữa kỳ số 3", "Phiếu bài tập căn bậc hai", "Đề kiểm tra 15 phút"];
    out.push({ t: "ingest", folderId: folderId, jobId: "j1", state: "queued", done: 0, total: total, note: "Đang xếp hàng…" });
    for (var p = 1; p <= total; p++) {
      out.push({ t: "ingest", folderId: folderId, jobId: "j1", state: "reading", done: p, total: total,
        note: "Đang đọc trang " + p + "/" + total + ": " + titles[Math.floor((p - 1) / 5)],
        found: { questions: Math.round(p * 4.9), topics: Math.min(14, 2 + Math.floor(p / 2)) }, costUsd: +(p * 0.0021).toFixed(4) });
    }
    out.push({ t: "ingest", folderId: folderId, jobId: "j1", state: "merging", done: total, total: total, note: "Đang gộp thành giáo trình và ma trận đề…", found: { questions: 118, topics: 14 } });
    out.push({ t: "ingest", folderId: folderId, jobId: "j1", state: "done", done: total, total: total, note: "Xong: 118 câu, 14 chủ đề, 2 cấu trúc đề", found: { questions: 118, topics: 14 }, costUsd: 0.061 });
    return out;
  }

  global.DevData = { lesson: lesson, chatHistory: chatHistory, streamTurn: streamTurn, longAnswer: longAnswer, ingestSteps: ingestSteps, interactiveHtml: interactiveHtml };
})(window);
