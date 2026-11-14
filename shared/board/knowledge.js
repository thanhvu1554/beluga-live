/* Folder knowledge page (PROTOCOL §11, §12, §14).
 * knowledge.render(data|null, review?)   data = knowledge.json; review = the `review` event (or {topics:[...]})
 * knowledge.progress(ingestEvent)        live ingest panel (state queued|reading|merging|done|error|cancelled)
 * knowledge.setTheme(t) / setLang(l) / setInsets(top, bottom) / show(sectionId)
 * -> {type:"ready", version}
 * -> {type:"gen_exam", structureId}
 * -> {type:"open_topic", topicId, title}
 * -> {type:"ingest_cancel"}             cancel button on a running ingest
 * -> {type:"add_exams"}                 button in the empty state */
(function (global) {
  "use strict";
  var VERSION = Beluga.VERSION;
  var LV = ["nb", "th", "vd", "vdc"];
  var LABELS = {
    vi: {
      tabs: { ov: "Tổng quan", cur: "Giáo trình", heat: "Ma trận", forms: "Dạng bài", school: "Trên trường", exam: "Cấu trúc đề" },
      lv: { nb: "Nhận biết", th: "Thông hiểu", vd: "Vận dụng", vdc: "Vận dụng cao" },
      lvs: { nb: "NB", th: "TH", vd: "VD", vdc: "VDC" },
      grade: "Lớp", questions: "câu hỏi", topics: "chủ đề", forms: "dạng bài", sources: "tài liệu",
      byLevel: "Theo mức độ", byKind: "Theo loại câu", srcTitle: "Tài liệu đã nạp", pages: "trang", q: "câu",
      kind: { exam: "Đề", review: "Đề cương", worksheet: "Phiếu bài tập", mcq: "Trắc nghiệm", truefalse: "Đúng/Sai", short: "Trả lời ngắn", essay: "Tự luận" },
      curSub: "Chương, chủ đề và các dạng bài gặp trong đề. Chạm để mở.",
      chapter: "Chương", method: "Cách làm", example: "Ví dụ", openTopic: "Học chủ đề này",
      heatSub: "Số câu theo chủ đề và mức độ. Ô càng đậm càng nhiều câu.", few: "ít", many: "nhiều",
      formsSub: "Các dạng bài gặp nhiều nhất trong đề đã nạp.", showAll: "Xem tất cả", showLess: "Thu gọn",
      schoolSub: "Vị trí đang học trên trường, suy ra từ đề và đề cương.", covered: "Đã học", current: "Đang học", yet: "Chưa học",
      now: "Đang học", confidence: "Độ tin cậy",
      examSub: "Khung đề rút ra từ các đề đã nạp. Tạo đề mới đúng khung này.",
      min: "phút", pts: "điểm", section: "Phần", count: "Số câu", points: "Điểm", gen: "Tạo đề",
      state: { new: "Chưa học", learning: "Đang học", weak: "Còn yếu", solid: "Vững" },
      reviewTitle: "Tiến độ của cậu",
      ingest: { queued: "Đang chờ", reading: "Đang đọc đề", merging: "Đang gộp", done: "Đã nạp xong", error: "Có lỗi", cancelled: "Đã huỷ" },
      found: "câu tìm thấy", foundT: "chủ đề", cancel: "Huỷ", cost: "Chi phí",
      emptyT: "Chưa có đề nào", emptyS: "Thêm ảnh hoặc PDF đề kiểm tra, đề cương của trường. Beluga sẽ đọc và dựng giáo trình, ma trận đề cho folder này.",
      add: "Thêm đề", waitHint: "Giáo trình sẽ hiện ở đây khi đọc xong."
    },
    en: {
      tabs: { ov: "Overview", cur: "Curriculum", heat: "Matrix", forms: "Question types", school: "At school", exam: "Exam formats" },
      lv: { nb: "Recall", th: "Understand", vd: "Apply", vdc: "Advanced" },
      lvs: { nb: "NB", th: "TH", vd: "VD", vdc: "VDC" },
      grade: "Grade", questions: "questions", topics: "topics", forms: "question types", sources: "sources",
      byLevel: "By level", byKind: "By format", srcTitle: "Sources", pages: "pages", q: "q",
      kind: { exam: "Exam", review: "Review sheet", worksheet: "Worksheet", mcq: "Multiple choice", truefalse: "True/false", short: "Short answer", essay: "Written" },
      curSub: "Chapters, topics and the question types seen in the exams. Tap to open.",
      chapter: "Chapter", method: "Method", example: "Example", openTopic: "Study this topic",
      heatSub: "Questions per topic and level. Darker means more.", few: "few", many: "many",
      formsSub: "The most frequent question types in the exams.", showAll: "Show all", showLess: "Show less",
      schoolSub: "Where the class is now, inferred from exams and review sheets.", covered: "Covered", current: "Current", yet: "Not yet",
      now: "Now", confidence: "Confidence",
      examSub: "Exam formats derived from the exams. Generate a new exam in the same format.",
      min: "min", pts: "points", section: "Part", count: "Questions", points: "Points", gen: "Generate exam",
      state: { new: "New", learning: "Learning", weak: "Weak", solid: "Solid" },
      reviewTitle: "Your progress",
      ingest: { queued: "Queued", reading: "Reading exams", merging: "Merging", done: "Done", error: "Error", cancelled: "Cancelled" },
      found: "questions found", foundT: "topics", cancel: "Cancel", cost: "Cost",
      emptyT: "No exams yet", emptyS: "Add photos or PDFs of school tests and review sheets. Beluga reads them and builds the curriculum and exam matrix for this folder.",
      add: "Add exams", waitHint: "The curriculum will appear here when reading is done."
    }
  };
  var SUBJECTS = {
    vi: { toan: "Toán", ly: "Vật lý", hoa: "Hoá học", van: "Ngữ văn", "anh-ssat": "Tiếng Anh", duc: "Tiếng Đức", "tin-cpp": "Tin học", chung: "Tổng hợp" },
    en: { toan: "Maths", ly: "Physics", hoa: "Chemistry", van: "Literature", "anh-ssat": "English", duc: "German", "tin-cpp": "Computer science", chung: "General" }
  };
  function L() { return LABELS[Beluga.lang()] || LABELS.vi; }
  var h = Blocks.h;
  var esc = MD.esc;

  var MARK = {
    new: '<svg viewBox="0 0 14 14"><circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.5"/></svg>',
    learning: '<svg viewBox="0 0 14 14"><circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M7 2a5 5 0 0 1 0 10Z" fill="currentColor"/></svg>',
    weak: '<svg viewBox="0 0 14 14"><path d="M7 1.6 12.8 12H1.2Z" fill="currentColor"/><path d="M7 5.4v3" stroke="var(--bg)" stroke-width="1.5" stroke-linecap="round"/><circle cx="7" cy="10.2" r=".85" fill="var(--bg)"/></svg>',
    solid: '<svg viewBox="0 0 14 14"><circle cx="7" cy="7" r="5.6" fill="currentColor"/></svg>'
  };
  var CHEV = '<svg class="chev" viewBox="0 0 12 12" aria-hidden="true"><path d="M4.5 2.5 8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>';
  var DOC = '<svg viewBox="0 0 18 22" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"><path d="M2.5 1h9l5 5v14a1 1 0 0 1-1 1h-13a1 1 0 0 1-1-1V2a1 1 0 0 1 1-1Z"/><path d="M11.5 1v5h5M5 11h8M5 14.5h8M5 18h5" stroke-linecap="round"/></svg>';

  document.body.classList.add("k-page");
  var tabs = h("nav", "k-tabs", '<div class="k-tabs-in"></div>');
  var wrap = h("main", "k-wrap");
  document.body.appendChild(tabs);
  document.body.appendChild(wrap);
  var ingestPanel = null;
  var lastData = null, lastReview = null, lastIngest = null, ingestLog = [];
  var showAllForms = false;

  function mark(state) {
    if (!MARK[state]) return "";
    return '<span class="mark" title="' + esc(L().state[state]) + '">' + MARK[state] + "</span>";
  }
  function num(n) { return (n == null ? 0 : n).toLocaleString(Beluga.lang() === "vi" ? "vi-VN" : "en-US"); }
  function fmtPts(p) { return String(+(+p).toFixed(2)).replace(".", Beluga.lang() === "vi" ? "," : "."); }

  function reviewMap(review) {
    var m = {};
    var list = review && (review.topics || (Array.isArray(review) ? review : null));
    (list || []).forEach(function (t) { if (t && t.topicId) m[t.topicId] = t; });
    return m;
  }

  function levelsOf(obj) {
    var o = { nb: 0, th: 0, vd: 0, vdc: 0 };
    if (obj && obj.levels) LV.forEach(function (k) { o[k] += +obj.levels[k] || 0; });
    return o;
  }
  function topicLevels(t) {
    var o = { nb: 0, th: 0, vd: 0, vdc: 0 };
    (t.forms || []).forEach(function (f) { var l = levelsOf(f); LV.forEach(function (k) { o[k] += l[k]; }); });
    return o;
  }
  function stack(levels) {
    var total = LV.reduce(function (s, k) { return s + (levels[k] || 0); }, 0) || 1;
    return '<div class="stack">' + LV.map(function (k) {
      var v = levels[k] || 0;
      return v ? '<i class="lv-' + k + '" style="width:' + (v / total * 100).toFixed(2) + '%"></i>' : "";
    }).join("") + "</div>";
  }

  /* ---------- sections ---------- */
  function secOverview(d, rv) {
    var lb = L();
    var cur = d.curriculum || [];
    var nTopics = 0, nForms = 0;
    cur.forEach(function (c) { (c.topics || []).forEach(function (t) { nTopics++; nForms += (t.forms || []).length; }); });
    var st = d.stats || {};
    var byLevel = st.byLevel || (function () {
      var o = { nb: 0, th: 0, vd: 0, vdc: 0 };
      cur.forEach(function (c) { (c.topics || []).forEach(function (t) { var l = topicLevels(t); LV.forEach(function (k) { o[k] += l[k]; }); }); });
      return o;
    })();
    var nQ = st.questions != null ? st.questions : LV.reduce(function (s, k) { return s + (byLevel[k] || 0); }, 0);
    var subj = (SUBJECTS[Beluga.lang()] || SUBJECTS.vi)[d.subject] || d.subject || "";
    var title = d.grade ? (subj ? subj + (Beluga.lang() === "vi" ? " " + d.grade : " · " + lb.grade + " " + d.grade) : lb.grade + " " + d.grade) : subj;
    var html = '<section class="k-sec k-hero" id="ov">' +
      '<div class="k-eyebrow">' + esc(lb.tabs.ov) + "</div>" +
      '<h1 class="k-title">' + esc(title || lb.tabs.ov) + "</h1>" +
      (d.goal ? '<div class="k-goal">' + esc(d.goal) + "</div>" : "") +
      '<div class="k-stats">' +
      '<div class="k-stat"><div class="n">' + num(nQ) + '</div><div class="l">' + lb.questions + "</div></div>" +
      '<div class="k-stat"><div class="n">' + num(nTopics) + '</div><div class="l">' + lb.topics + "</div></div>" +
      '<div class="k-stat"><div class="n">' + num(nForms) + '</div><div class="l">' + lb.forms + "</div></div>" +
      '<div class="k-stat"><div class="n">' + num((d.sources || []).length) + '</div><div class="l">' + lb.sources + "</div></div>" +
      "</div>";
    var lvTotal = LV.reduce(function (s, k) { return s + (byLevel[k] || 0); }, 0) || 1;
    html += '<div class="k-eyebrow">' + lb.byLevel + "</div>" + stack(byLevel) +
      '<div class="legend">' + LV.map(function (k) {
        return '<span><i class="sw lv-' + k + '"></i>' + lb.lv[k] + " <b>" + num(byLevel[k] || 0) + "</b> · " + Math.round((byLevel[k] || 0) / lvTotal * 100) + "%</span>";
      }).join("") + "</div>";
    var byKind = st.byKind || {};
    var kinds = Object.keys(byKind).filter(function (k) { return byKind[k] > 0; });
    if (kinds.length) {
      var kMax = Math.max.apply(null, kinds.map(function (k) { return byKind[k]; }));
      html += '<div class="k-kinds"><div class="k-eyebrow">' + lb.byKind + "</div>" + kinds.sort(function (a, b) { return byKind[b] - byKind[a]; }).map(function (k) {
        return '<div class="k-row"><span>' + esc(lb.kind[k] || k) + '</span><span class="v">' + num(byKind[k]) + '</span><div class="bar"><i style="width:' + (byKind[k] / kMax * 100).toFixed(1) + '%"></i></div></div>';
      }).join("") + "</div>";
    }
    if (rv) {
      var counts = { solid: 0, learning: 0, weak: 0, new: 0 };
      Object.keys(rv).forEach(function (id) { if (counts[rv[id].state] != null) counts[rv[id].state]++; });
      html += '<div class="k-kinds"><div class="k-eyebrow">' + lb.reviewTitle + '</div><div class="k-review-sum">' +
        ["solid", "learning", "weak", "new"].map(function (s) { return "<span>" + mark(s) + lb.state[s] + " <b>" + counts[s] + "</b></span>"; }).join("") + "</div></div>";
    }
    if ((d.sources || []).length) {
      html += '<div class="k-kinds"><div class="k-eyebrow">' + lb.srcTitle + '</div><div class="k-sources">' + d.sources.map(function (s) {
        var meta = [lb.kind[s.kind] || s.kind, s.school, s.date ? Blocks.fmtDate(s.date) : "", s.pages ? s.pages + " " + lb.pages : "", s.questions ? s.questions + " " + lb.q : ""].filter(Boolean).map(esc).join(" · ");
        return '<div class="k-src">' + DOC + '<div><div class="t">' + esc(s.title || s.name || "") + '</div><div class="m">' + meta + "</div></div></div>";
      }).join("") + "</div></div>";
    }
    return html + "</section>";
  }

  function stateLegend() {
    var lb = L();
    return '<div class="k-legend-row">' + ["new", "learning", "weak", "solid"].map(function (s) { return "<span>" + mark(s) + esc(lb.state[s]) + "</span>"; }).join("") + "</div>";
  }

  function secCurriculum(d, rv) {
    var lb = L();
    var html = '<section class="k-sec" id="cur"><h2 class="k-h">' + lb.tabs.cur + '</h2><p class="k-sub">' + lb.curSub + "</p>";
    if (rv) html += stateLegend();
    (d.curriculum || []).forEach(function (c, ci) {
      var cCount = (c.topics || []).reduce(function (s, t) { return s + (t.count || 0); }, 0);
      html += '<details class="k-ch"' + (ci === 0 ? " open" : "") + "><summary>" + CHEV + '<div><div class="no">' + lb.chapter + " " + (ci + 1) + '</div><div class="tt">' + MD.inline(c.title || "") + '</div></div><span class="c">' + num(cCount) + " " + lb.q + "</span></summary>";
      (c.topics || []).forEach(function (t) {
        var r = rv && rv[t.id];
        html += '<details class="k-topic" data-topic="' + esc(t.id) + '"><summary>' + (r ? mark(r.state) : CHEV) +
          '<span class="tt">' + MD.inline(t.title || "") + '</span><span class="c">' + num(t.count || 0) + "</span>" + (r ? CHEV : "") + '</summary><div class="in">' +
          (t.summary ? '<div class="sum md">' + MD.inline(t.summary) + "</div>" : "");
        (t.forms || []).forEach(function (f) {
          var lv = levelsOf(f);
          html += '<div class="k-form"><div class="ft">' + MD.inline(f.title || "") + "</div>" +
            '<div class="lv"><span class="chip total"><span>' + num(f.count || 0) + " " + lb.q + "</span></span>" + LV.map(function (k) {
              return '<span class="chip lv-' + k + (lv[k] ? "" : " zero") + '"><span>' + lb.lvs[k] + " " + lv[k] + "</span></span>";
            }).join("") + "</div>" +
            (f.method ? '<div class="meth md">' + MD.block(f.method) + "</div>" : "") +
            (f.example && f.example.md ? '<div class="ex md"><span class="lbl">' + lb.example + "</span>" + MD.block(f.example.md) + "</div>" : "") +
            "</div>";
        });
        html += '<button type="button" class="k-open" data-open="' + esc(t.id) + '">' + lb.openTopic + "</button></div></details>";
      });
      html += "</details>";
    });
    return html + "</section>";
  }

  function secHeat(d, rv) {
    var lb = L();
    var max = 1;
    (d.curriculum || []).forEach(function (c) { (c.topics || []).forEach(function (t) { var l = topicLevels(t); LV.forEach(function (k) { max = Math.max(max, l[k]); }); }); });
    var html = '<section class="k-sec" id="heat"><h2 class="k-h">' + lb.tabs.heat + '</h2><p class="k-sub">' + lb.heatSub + "</p>";
    if (rv) html += stateLegend();
    html += '<table class="heat"><colgroup>' + (rv ? '<col class="cm">' : "") + '<col><col class="cn"><col class="cn"><col class="cn"><col class="cn"></colgroup><thead><tr>' +
      (rv ? "<th></th>" : "") + '<th class="tl"></th>' + LV.map(function (k) { return "<th>" + lb.lvs[k] + "</th>"; }).join("") + "</tr></thead><tbody>";
    (d.curriculum || []).forEach(function (c, ci) {
      html += '<tr class="chap"><td colspan="' + (rv ? 6 : 5) + '">' + lb.chapter + " " + (ci + 1) + "</td></tr>";
      (c.topics || []).forEach(function (t) {
        var l = topicLevels(t), r = rv && rv[t.id];
        html += '<tr class="topic" data-open="' + esc(t.id) + '">' + (rv ? '<td class="mk">' + (r ? mark(r.state) : "") + "</td>" : "") +
          '<td class="name"><span>' + MD.inline(t.title || "") + "</span></td>" + LV.map(function (k) {
            var v = l[k], a = v ? 0.12 + 0.88 * (v / max) : 0;
            return '<td class="cell' + (a > 0.55 ? " dark" : "") + (v ? "" : " zero") + '" style="--a:' + a.toFixed(3) + '"><span>' + (v || "·") + "</span></td>";
          }).join("") + "</tr>";
      });
    });
    html += '</tbody></table><div class="heat-scale">' + lb.few + '<span class="ramp">' + [0.15, 0.35, 0.55, 0.78, 1].map(function (a) { return '<i style="--a:' + a + '"></i>'; }).join("") + "</span>" + lb.many + "</div>";
    return html + "</section>";
  }

  function secForms(d) {
    var lb = L();
    var list = [];
    (d.curriculum || []).forEach(function (c) { (c.topics || []).forEach(function (t) { (t.forms || []).forEach(function (f) { list.push({ f: f, t: t }); }); }); });
    list.sort(function (a, b) { return (b.f.count || 0) - (a.f.count || 0); });
    var max = list.length ? (list[0].f.count || 1) : 1;
    var shown = showAllForms ? list : list.slice(0, 10);
    var html = '<section class="k-sec" id="forms"><h2 class="k-h">' + lb.tabs.forms + '</h2><p class="k-sub">' + lb.formsSub + "</p>";
    shown.forEach(function (x) {
      var lv = levelsOf(x.f);
      html += '<div class="fbar"><div class="t"><span class="name">' + MD.inline(x.f.title || "") + '<span class="topic">' + MD.inline(x.t.title || "") + '</span></span><span class="n">' + num(x.f.count || 0) + "</span></div>" +
        '<div style="width:' + Math.max(4, (x.f.count || 0) / max * 100).toFixed(1) + '%">' + stack(lv) + "</div></div>";
    });
    html += '<div class="legend" style="margin-bottom:var(--s4)">' + LV.map(function (k) { return '<span><i class="sw lv-' + k + '"></i>' + lb.lv[k] + "</span>"; }).join("") + "</div>";
    if (list.length > 10) html += '<button type="button" class="k-more" data-more>' + (showAllForms ? lb.showLess : lb.showAll + " (" + list.length + ")") + "</button>";
    return html + "</section>";
  }

  function secSchool(d) {
    var lb = L(), sp = d.schoolProgress;
    if (!sp) return "";
    var covered = {};
    (sp.coveredTopicIds || []).forEach(function (id) { covered[id] = 1; });
    var html = '<section class="k-sec" id="school"><h2 class="k-h">' + lb.tabs.school + '</h2><p class="k-sub">' + lb.schoolSub +
      (sp.confidence != null ? " " + lb.confidence + " " + Math.round(sp.confidence * 100) + "%." : "") + "</p>" +
      '<div class="k-legend-row"><span><i class="lg-dot covered"></i>' + lb.covered + '</span><span><i class="lg-dot current"></i>' + lb.current + '</span><span><i class="lg-dot yet"></i>' + lb.yet + "</span></div>" +
      (sp.note ? '<p class="k-note">' + MD.inline(sp.note) + "</p>" : "") + '<div class="track">';
    var all = [];
    (d.curriculum || []).forEach(function (c, ci) { (c.topics || []).forEach(function (t, ti) { all.push({ t: t, c: c, ci: ci, first: ti === 0 }); }); });
    all.forEach(function (x, i) {
      var cls = x.t.id === sp.currentTopicId ? "current" : covered[x.t.id] ? "covered" : "yet";
      if (x.first) html += '<div class="trk-ch">' + lb.chapter + " " + (x.ci + 1) + " · " + MD.inline(x.c.title || "") + "</div>";
      html += '<div class="trk ' + cls + (i === all.length - 1 ? " last" : "") + '"><span class="pt"><i></i></span><span class="tx">' + MD.inline(x.t.title || "") +
        (cls === "current" ? '<span class="now">' + lb.now + "</span>" : "") + "</span></div>";
    });
    return html + "</div></section>";
  }

  function secExams(d) {
    var lb = L();
    var list = d.structures || [];
    if (!list.length) return "";
    var html = '<section class="k-sec" id="exam"><h2 class="k-h">' + lb.tabs.exam + '</h2><p class="k-sub">' + lb.examSub + "</p>";
    list.forEach(function (s) {
      var nq = (s.sections || []).reduce(function (a, x) { return a + (x.count || 0); }, 0);
      var meta = [s.durationMin ? s.durationMin + " " + lb.min : "", s.totalPoints != null ? fmtPts(s.totalPoints) + " " + lb.pts : "", nq ? nq + " " + lb.q : ""].filter(Boolean).join(" · ");
      var lv = { nb: 0, th: 0, vd: 0, vdc: 0 };
      (s.matrix || []).forEach(function (m) { if (lv[m.level] != null) lv[m.level] += +m.points || +m.count || 0; });
      var hasLv = LV.some(function (k) { return lv[k] > 0; });
      html += '<div class="k-card"><div class="ct">' + MD.inline(s.title || "") + '</div><div class="cm">' + esc(meta) + "</div>" +
        "<table><thead><tr><th>" + lb.section + '</th><th class="num">' + lb.count + '</th><th class="num">' + lb.points + "</th></tr></thead><tbody>" +
        (s.sections || []).map(function (x) {
          return '<tr><td><span class="sid">' + esc(x.id || "") + "</span>" + MD.inline(x.title || "") + '<span class="kd">' + esc(lb.kind[x.kind] || x.kind || "") + (x.instructions ? " · " + esc(x.instructions) : "") + "</span></td>" +
            '<td class="num">' + (x.count || 0) + '</td><td class="num">' + fmtPts((x.count || 0) * (x.pointsEach || 0)) + "</td></tr>";
        }).join("") + "</tbody></table>" +
        (hasLv ? '<div class="lvlbl">' + lb.byLevel + " (" + lb.pts + ")</div>" + stack(lv) + '<div class="legend">' + LV.filter(function (k) { return lv[k] > 0; }).map(function (k) {
          return '<span><i class="sw lv-' + k + '"></i>' + lb.lvs[k] + " <b>" + fmtPts(lv[k]) + "</b></span>";
        }).join("") + "</div>" : "") +
        '<button type="button" class="k-gen" data-gen="' + esc(s.id) + '">' + lb.gen + "</button></div>";
    });
    return html + "</section>";
  }

  function emptyState() {
    var lb = L();
    return '<div class="k-empty"><svg class="art" viewBox="0 0 120 90" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round" stroke-linecap="round">' +
      '<path d="M30 14h40l14 14v52H30Z"/><path d="M70 14v14h14"/><path d="M40 40h34M40 50h34M40 60h22"/>' +
      '<path d="M18 24h8M14 34h10M92 52h12M94 62h8" opacity=".5"/></svg>' +
      '<div class="t">' + lb.emptyT + '</div><div class="s">' + lb.emptyS + '</div><button type="button" class="k-add" data-add>' + lb.add + "</button></div>";
  }

  /* ---------- tabs ---------- */
  var sectionIds = [];
  function buildTabs() {
    var lb = L(), inner = tabs.querySelector(".k-tabs-in");
    sectionIds = Array.prototype.map.call(wrap.querySelectorAll("section.k-sec"), function (s) { return s.id; });
    inner.innerHTML = sectionIds.map(function (id, i) {
      return '<button type="button" class="k-tab' + (i === 0 ? " on" : "") + '" data-tab="' + id + '">' + esc(lb.tabs[id] || id) + "</button>";
    }).join("");
    tabs.style.display = sectionIds.length > 1 ? "" : "none";
  }
  var spyLock = 0;
  function setTab(id) {
    Array.prototype.forEach.call(tabs.querySelectorAll(".k-tab"), function (b) {
      var on = b.getAttribute("data-tab") === id;
      b.classList.toggle("on", on);
      if (on) {
        var p = b.parentNode, l = b.offsetLeft - 20, r = b.offsetLeft + b.offsetWidth + 20;
        if (l < p.scrollLeft || r > p.scrollLeft + p.clientWidth) p.scrollTo({ left: Math.max(0, l), behavior: "smooth" });
      }
    });
  }
  function spy() {
    if (Date.now() < spyLock) return;
    var y = tabs.getBoundingClientRect().bottom + 24, cur = sectionIds[0];
    sectionIds.forEach(function (id) { var s = document.getElementById(id); if (s && s.getBoundingClientRect().top <= y) cur = id; });
    if ((global.innerHeight + global.scrollY) >= document.documentElement.scrollHeight - 4) cur = sectionIds[sectionIds.length - 1];
    if (cur) setTab(cur);
  }
  global.addEventListener("scroll", spy, { passive: true });
  function show(id, smooth) {
    var s = document.getElementById(id);
    if (!s) return;
    spyLock = Date.now() + 700;
    setTab(id);
    var top = s.getBoundingClientRect().top + global.scrollY - tabs.offsetHeight + 1;
    global.scrollTo({ top: Math.max(0, top), behavior: smooth === false ? "auto" : "smooth" });
  }

  /* ---------- clicks ---------- */
  document.addEventListener("click", function (e) {
    var t = e.target.closest ? e.target : e.target.parentNode;
    var b;
    if ((b = t.closest("[data-tab]"))) { show(b.getAttribute("data-tab")); return; }
    if ((b = t.closest("[data-gen]"))) { Beluga.post({ type: "gen_exam", structureId: b.getAttribute("data-gen") }); return; }
    if ((b = t.closest("[data-open]"))) {
      var id = b.getAttribute("data-open");
      Beluga.post({ type: "open_topic", topicId: id, title: topicTitle(id) });
      return;
    }
    if ((b = t.closest("[data-more]"))) {
      showAllForms = !showAllForms;
      var old = document.getElementById("forms");
      var tmp = document.createElement("div"); tmp.innerHTML = secForms(lastData);
      old.parentNode.replaceChild(tmp.firstChild, old);
      if (!showAllForms) show("forms", false);
      return;
    }
    if (t.closest("[data-add]")) { Beluga.post({ type: "add_exams" }); return; }
    if (t.closest("[data-cancel]")) { Beluga.post({ type: "ingest_cancel" }); }
  });
  function topicTitle(id) {
    var out = "";
    ((lastData && lastData.curriculum) || []).forEach(function (c) { (c.topics || []).forEach(function (t) { if (t.id === id) out = t.title || ""; }); });
    return out;
  }

  /* ---------- ingest panel ---------- */
  function ensurePanel() {
    if (ingestPanel && ingestPanel.isConnected) return ingestPanel;
    ingestPanel = h("div", "k-ingest", '<div class="hd"><span class="st"></span><span class="pc"></span></div><div class="pbar"><i></i></div>' +
      '<div class="note"></div><div class="cnt"><div><b class="nq">0</b><span class="lq"></span></div><div><b class="nt">0</b><span class="lt"></span></div></div>' +
      '<ul class="lg"></ul><div class="ft"><span class="cost"></span><button type="button" class="k-cancel" data-cancel></button></div>');
    ingestPanel.hidden = true;
    wrap.insertBefore(ingestPanel, wrap.firstChild);
    return ingestPanel;
  }
  function paintIngest() {
    var ev = lastIngest, lb = L();
    if (!ev) { if (ingestPanel) ingestPanel.hidden = true; return; }
    var p = ensurePanel();
    p.hidden = false;
    var busy = ev.state === "queued" || ev.state === "reading" || ev.state === "merging";
    p.classList.toggle("busy", busy);
    p.querySelector(".st").textContent = lb.ingest[ev.state] || ev.state || "";
    var total = +ev.total || 0, done = +ev.done || 0;
    var frac = ev.state === "done" ? 1 : total ? Math.min(1, done / total) : 0;
    var bar = p.querySelector(".pbar");
    bar.classList.toggle("indef", ev.state === "queued" || (busy && !total));
    bar.querySelector("i").style.width = (frac * 100).toFixed(1) + "%";
    p.querySelector(".pc").textContent = total ? done + "/" + total : "";
    p.querySelector(".note").textContent = ev.note || "";
    var f = ev.found || {};
    p.querySelector(".nq").textContent = num(f.questions || 0);
    p.querySelector(".nt").textContent = num(f.topics || 0);
    p.querySelector(".lq").textContent = lb.found;
    p.querySelector(".lt").textContent = lb.foundT;
    p.querySelector(".cost").textContent = ev.costUsd != null ? lb.cost + " $" + (+ev.costUsd).toFixed(3) : "";
    var cancel = p.querySelector(".k-cancel");
    cancel.textContent = lb.cancel;
    cancel.style.visibility = busy ? "" : "hidden";
    var ul = p.querySelector(".lg");
    // Earlier notes, newest first; the current one is already on the line above.
    var past = ingestLog.slice(0, ingestLog[ingestLog.length - 1] === ev.note ? -1 : undefined).reverse().slice(0, 5);
    ul.innerHTML = past.map(function (n) { return "<li>" + esc(n) + "</li>"; }).join("");
    ul.style.display = past.length ? "" : "none";
  }

  /* ---------- api ---------- */
  function render(data, review) {
    lastData = data || null;
    lastReview = review || null;
    var rv = review ? reviewMap(review) : null;
    if (rv && !Object.keys(rv).length) rv = null;
    var y = global.scrollY;
    if (ingestPanel) ingestPanel.parentNode && ingestPanel.parentNode.removeChild(ingestPanel);
    var html;
    if (!data || !(data.curriculum || []).length) {
      html = lastIngest && lastIngest.state !== "done" && lastIngest.state !== "cancelled" && lastIngest.state !== "error"
        ? '<div class="k-empty" style="padding-top:var(--s8)"><div class="s">' + L().waitHint + "</div></div>" : emptyState();
    } else {
      html = secOverview(data, rv) + secCurriculum(data, rv) + secHeat(data, rv) + secForms(data) + secSchool(data) + secExams(data);
    }
    wrap.innerHTML = html;
    if (ingestPanel) wrap.insertBefore(ingestPanel, wrap.firstChild);
    buildTabs();
    paintIngest();
    if (y) global.scrollTo(0, y);
    spy();
  }

  var knowledge = {
    version: VERSION,
    render: render,
    progress: function (ev) {
      if (!ev) { lastIngest = null; ingestLog = []; paintIngest(); return; }
      if (lastIngest && ev.jobId && lastIngest.jobId && ev.jobId !== lastIngest.jobId) ingestLog = [];
      lastIngest = ev;
      if (ev.note && ingestLog[ingestLog.length - 1] !== ev.note) ingestLog.push(ev.note);
      if (ingestLog.length > 40) ingestLog.shift();
      if (!lastData && !wrap.querySelector(".k-empty .s")) render(null);
      // Waiting screen instead of the "add exams" invitation while a job runs.
      if (!lastData && wrap.querySelector("[data-add]") && ev.state !== "done" && ev.state !== "error" && ev.state !== "cancelled") render(null);
      paintIngest();
    },
    setTheme: function (t) { Beluga.setTheme(t); },
    setLang: function (l) { Beluga.setLang(l); },
    setInsets: function (top, bottom) {
      var st = document.documentElement.style;
      if (top != null) st.setProperty("--k-top", Math.max(0, +top || 0) + "px");
      if (bottom != null) st.setProperty("--k-bottom", Math.max(0, +bottom || 0) + "px");
    },
    show: function (id) { show(id); }
  };

  Beluga.onLang(function () { if (lastData || wrap.children.length) render(lastData, lastReview); });
  render(null);
  global.knowledge = knowledge;
  Beluga.ready(function () { Beluga.post({ type: "ready", version: VERSION }); });
})(window);
