/* Beluga chat transcript (PROTOCOL §7, §14).
 * chat.reset()
 * chat.apply(event)            user | chat_delta | chat | show | ask | done | turn_end | state | notice
 *                              (+ live events say | step | show with step>=0, drawn compactly)
 * chat.load(events[])          history: reset + apply without animation, scrolled to the end
 * chat.setTheme(t) / setLang(l)
 * chat.setFileBase(url, token) images and PDFs: <url>/file/<fileId>?token=<token>
 * chat.setInsets(top, bottom)  px kept clear for the native title bar and composer
 * chat.scrollToBottom(smooth?)
 * -> {type:"ready", version}
 * -> {type:"action", action:"copy"|"retry", turn, text?}   (copy of a code block also sends what:"code")
 * -> {type:"ask_tap", prompt, kind?, turn?}
 * -> {type:"open_file", fileId, kind, name}               tap on an image thumbnail or PDF chip
 * -> {type:"open_url", url}                                tap on a markdown link */
(function (global) {
  "use strict";
  var VERSION = Beluga.VERSION;
  var LABELS = {
    vi: { thinking: "Đang suy nghĩ", capture: "Đang xem ảnh", tool: "Đang làm", copy: "Chép", copied: "Đã chép", retry: "Thử lại",
          done: "Xong bài", stopped: "Đã dừng", pdf: "Tài liệu PDF", image: "Ảnh", voice: "Nói", bottom: "Xuống cuối" },
    en: { thinking: "Thinking", capture: "Looking at the photo", tool: "Working", copy: "Copy", copied: "Copied", retry: "Retry",
          done: "Lesson done", stopped: "Stopped", pdf: "PDF document", image: "Image", voice: "Spoken", bottom: "Jump to latest" }
  };
  function L(k) { return (LABELS[Beluga.lang()] || LABELS.vi)[k]; }
  var h = Blocks.h;
  var ICON = {
    copy: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"><rect x="7" y="7" width="10" height="10" rx="2.2"/><path d="M13 7V5.2A2.2 2.2 0 0 0 10.8 3H5.2A2.2 2.2 0 0 0 3 5.2v5.6A2.2 2.2 0 0 0 5.2 13H7"/></svg>',
    check: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="m4.5 10.5 3.5 3.5 7.5-8"/></svg>',
    retry: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M16 10a6 6 0 1 1-1.8-4.3"/><path d="M16.2 3.5v3.2H13"/></svg>',
    reply: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M6 4 2.5 7.5 6 11"/><path d="M3 7.5h6.5a4 4 0 0 1 4 4V13"/></svg>',
    pdf: '<svg viewBox="0 0 22 26" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"><path d="M3 1.5h10.5L19.5 7.5V23a1.5 1.5 0 0 1-1.5 1.5H3A1.5 1.5 0 0 1 1.5 23V3A1.5 1.5 0 0 1 3 1.5Z"/><path d="M13.5 1.5V7.5h6"/><path d="M6 14h10M6 18h7" stroke-linecap="round"/></svg>',
    mic: '<svg viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"><rect x="4.5" y="1" width="5" height="8" rx="2.5"/><path d="M2.5 7a4.5 4.5 0 0 0 9 0M7 11.5V13"/></svg>',
    down: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M8 3v10M3.5 8.5 8 13l4.5-4.5"/></svg>'
  };

  document.body.classList.add("chat-page");
  var scroller = h("div", "scroller");
  var log = h("div", "log");
  var thinking = h("div", "thinking", '<span class="dots3"><i></i><i></i><i></i></span><span class="tx"></span>');
  var toBottom = h("button", "to-bottom", ICON.down);
  toBottom.type = "button";
  scroller.appendChild(log);
  document.body.appendChild(scroller);
  document.body.appendChild(toBottom);

  var turns = {};          // turn -> {el, body, parts:[], draft, sayEl, actions, ended}
  var restoring = false;
  var fileBase = "", fileToken = "";
  var stick = true;
  var lastAssistantTurn = null;

  /* ---------- motion ----------
   * One requestAnimationFrame loop drives both the text reveal of streaming answers and the glide that keeps
   * the newest line in view. Text arrives from the socket in bursts; it is shown at an even pace a little
   * behind (about 0.15 s), so lines grow smoothly instead of jumping a chunk at a time, and the view follows
   * by easing toward the end instead of snapping by a line height. Reduce Motion shows everything at once. */
  var reduceMotion = !!(global.matchMedia && global.matchMedia("(prefers-reduced-motion: reduce)").matches);
  var revealing = [];      // drafts being revealed: {el, text, shown, final, turn}
  var gliding = false;
  var loop = 0, lastFrame = 0;
  function kick() {
    if (loop) return;
    lastFrame = 0;
    loop = (global.requestAnimationFrame || function (f) { return setTimeout(function () { f(Date.now()); }, 16); })(frame);
  }
  function frame(now) {
    loop = 0;
    var dt = lastFrame ? Math.min(50, now - lastFrame) : 16;
    lastFrame = now;
    var grew = false;
    for (var i = revealing.length - 1; i >= 0; i--) {
      if (revealStep(revealing[i], dt)) grew = true;
      if (revealing[i].done) revealing.splice(i, 1);
    }
    if (grew) updateToBottom();
    glide(dt);
    if (revealing.length || gliding) { loop = (global.requestAnimationFrame || setTimeout)(frame); }
  }
  function revealStep(d, dt) {
    var len = d.text.length;
    if (d.shown < len) {
      var backlog = len - d.shown;
      var step = Math.max(1, Math.ceil(backlog * Math.min(1, dt / 150)));
      var end = d.shown + step;
      // Prefer ending a frame on a word boundary when one is close.
      if (end < len) { var ws = d.text.slice(end, end + 6).search(/\s/); if (ws >= 0) end += ws; }
      d.shown = Math.min(len, end);
      var complete = d.final && d.shown >= len;
      patch(d.el, MD.block(d.text.slice(0, d.shown), { partial: !complete }));
      if (complete) settleDraft(d);
      if (stick) gliding = true;
      return true;
    }
    if (d.final) { patch(d.el, MD.block(d.text)); settleDraft(d); }
    return false;
  }
  function settleDraft(d) {
    d.done = true;
    if (!d.text.trim() && d.el.parentNode) d.el.parentNode.removeChild(d.el);
    if (d.onDone) d.onDone();
  }
  function glide(dt) {
    if (!gliding) return;
    if (!stick || touching) { gliding = false; return; }
    var target = scroller.scrollHeight - scroller.clientHeight;
    var cur = scroller.scrollTop;
    var diff = target - cur;
    if (diff <= 0.5 || reduceMotion) { if (diff > 0) scroller.scrollTop = target; gliding = revealing.length > 0; return; }
    // Exponential approach (time constant ~90 ms): fast for a big block, gentle for a line.
    scroller.scrollTop = cur + Math.max(0.5, diff * Math.min(1, dt / 90));
  }

  /* ---------- scrolling ---------- */
  function atBottom() { return scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 64; }
  function toEnd(smooth) {
    if (smooth && !reduceMotion) { gliding = true; kick(); return; }
    scroller.scrollTop = scroller.scrollHeight;
  }
  function follow() { if (stick) { gliding = true; kick(); } updateToBottom(); }
  function updateToBottom() {
    toBottom.classList.toggle("show", !stick && scroller.scrollHeight > scroller.clientHeight + 40);
  }
  // Only the reader moves `stick` (a finger, its momentum, a wheel); the glide's own scrolling never does.
  var touching = false, userUntil = 0;
  scroller.addEventListener("touchstart", function () { touching = true; gliding = false; userUntil = Date.now() + 2000; }, { passive: true });
  scroller.addEventListener("touchmove", function () { userUntil = Date.now() + 2000; }, { passive: true });
  scroller.addEventListener("touchend", function () { touching = false; userUntil = Date.now() + 1200; }, { passive: true });
  scroller.addEventListener("touchcancel", function () { touching = false; userUntil = Date.now() + 600; }, { passive: true });
  scroller.addEventListener("scroll", function () {
    if (touching || Date.now() < userUntil) stick = atBottom();
    updateToBottom();
  }, { passive: true });
  scroller.addEventListener("wheel", function (e) { userUntil = Date.now() + 600; if (e.deltaY < 0) stick = false; }, { passive: true });
  toBottom.addEventListener("click", function () { stick = true; toEnd(true); updateToBottom(); });
  if (global.ResizeObserver) new ResizeObserver(function () { follow(); }).observe(log);
  global.addEventListener("resize", follow);
  global.addEventListener("blockresize", follow);

  /* ---------- helpers ---------- */
  function fileUrl(id) {
    if (!fileBase) return "";
    return fileBase.replace(/\/+$/, "") + "/file/" + encodeURIComponent(id) + (fileToken ? "?token=" + encodeURIComponent(fileToken) : "");
  }

  function before(node) { log.insertBefore(node, thinking); }

  function turnFor(n) {
    var key = n == null ? "_" : String(n);
    var t = turns[key];
    if (t) return t;
    var el = h("div", "msg assistant" + (restoring ? " still" : ""));
    if (n != null) el.setAttribute("data-turn", n);
    var body = h("div", "body");
    el.appendChild(body);
    before(el);
    t = turns[key] = { n: n, el: el, body: body, parts: [], draft: null, sayEl: null, actions: null, ended: false };
    if (n != null) lastAssistantTurn = n;
    return t;
  }

  // Replace a container's children with the parsed HTML, touching only the children that changed. A child of
  // the same kind is updated in place (a growing paragraph keeps its element, so its fade-in is not restarted);
  // children that are new get the `fresh` class and fade in.
  function patch(el, html) {
    var tmp = document.createElement("div");
    tmp.innerHTML = html;
    var next = tmp.children, i;
    for (i = 0; i < next.length; i++) {
      var cur = el.children[i], nx = next[i];
      if (cur && cur.tagName === nx.tagName && cur.className.replace(/\s*fresh\b/, "") === nx.className) {
        if (cur.innerHTML !== nx.innerHTML) cur.innerHTML = nx.innerHTML;
        continue;
      }
      var clone = nx.cloneNode(true);
      if (cur) el.replaceChild(clone, cur);
      else { if (!restoring) clone.classList.add("fresh"); el.appendChild(clone); }
    }
    while (el.children.length > next.length) el.removeChild(el.lastChild);
  }

  /* ---------- streaming draft ---------- */
  function scheduleDraft(t) {
    var d = t.draft;
    if (!d) return;
    if (reduceMotion) { d.shown = d.text.length; patch(d.el, MD.block(d.text, { partial: true })); follow(); return; }
    if (revealing.indexOf(d) < 0) revealing.push(d);
    kick();
  }
  function draftFor(t) {
    if (!t.draft) {
      var el = h("div", "seg md");
      t.body.appendChild(el);
      t.draft = { el: el, text: "", shown: 0, final: false, done: false };
      t.sayEl = null;
    }
    return t.draft;
  }
  // The final text of a segment: shown at once when restoring, otherwise the reveal finishes it at its pace
  // (later blocks are appended after this element, so order never changes).
  function finishDraft(t, md) {
    var d = t.draft;
    if (!d && md == null) return;
    if (!d) { d = draftFor(t); }
    var text = md != null ? md : d.text;
    if (text.trim()) t.parts.push(text);
    t.draft = null;
    d.text = text;
    d.final = true;
    if (restoring || reduceMotion || d.shown >= text.length) {
      d.shown = text.length;
      var i = revealing.indexOf(d);
      if (i >= 0) revealing.splice(i, 1);
      patch(d.el, MD.block(text));
      settleDraft(d);
      return;
    }
    if (revealing.indexOf(d) < 0) revealing.push(d);
    kick();
  }

  function setThinking(on, kind) {
    thinking.classList.toggle("on", !!on);
    if (on) thinking.querySelector(".tx").textContent = kind === "capture" ? L("capture") : kind === "tool" ? L("tool") : L("thinking");
    log.appendChild(thinking);
    follow();
  }

  function blockText(b) {
    if (!b) return "";
    if (b.type === "math") return "$$" + (b.tex || "") + "$$";
    if (b.type === "chem") return "$$\\ce{" + (b.tex || "") + "}$$";
    if (b.type === "text" || b.type === "answer") return b.md || "";
    if (b.type === "code") return "```" + (b.lang || "") + "\n" + (b.code || "") + "\n```";
    if (b.type === "table") {
      var head = b.head || [];
      return "| " + head.join(" | ") + " |\n|" + head.map(function () { return "---"; }).join("|") + "|\n" +
        (b.rows || []).map(function (r) { return "| " + r.join(" | ") + " |"; }).join("\n");
    }
    if (b.type === "vocab") return "**" + (b.word || "") + "** " + (b.ipa || "") + " — " + (b.meaning || "");
    return b.fallback || "";
  }

  /* ---------- actions under an assistant message ---------- */
  function addActions(t) {
    if (t.actions || t.n == null) return;
    var row = h("div", "actions");
    var copy = h("button", "act copy", ICON.copy);
    copy.type = "button"; copy.setAttribute("aria-label", L("copy"));
    copy.addEventListener("click", function () {
      var text = t.parts.join("\n\n");
      Beluga.post({ type: "action", action: "copy", turn: t.n, text: text });
      Beluga.copyText(text);
      copy.innerHTML = ICON.check; copy.classList.add("done");
      setTimeout(function () { copy.innerHTML = ICON.copy; copy.classList.remove("done"); }, 1400);
    });
    var retry = h("button", "act retry", ICON.retry);
    retry.type = "button"; retry.setAttribute("aria-label", L("retry"));
    retry.addEventListener("click", function () { Beluga.post({ type: "action", action: "retry", turn: t.n }); });
    row.appendChild(copy); row.appendChild(retry);
    t.el.appendChild(row);
    t.actions = row;
    refreshRetry();
  }
  // Retry only makes sense on the newest answer.
  function refreshRetry() {
    var newest = null;
    Object.keys(turns).forEach(function (k) { var t = turns[k]; if (t.actions && (newest == null || t.n > newest.n)) newest = t; });
    Object.keys(turns).forEach(function (k) {
      var t = turns[k];
      if (t.actions) t.actions.querySelector(".retry").style.display = t === newest ? "" : "none";
    });
  }

  /* ---------- user message ---------- */
  function userMsg(ev) {
    var el = h("div", "msg user" + (restoring ? " still" : ""));
    if (ev.turn != null) el.setAttribute("data-user-turn", ev.turn);
    var files = ev.files || [];
    var imgs = files.filter(function (f) { return f.kind !== "pdf"; });
    var pdfs = files.filter(function (f) { return f.kind === "pdf"; });
    if (imgs.length) {
      var row = h("div", "files" + (imgs.length === 1 ? " one" : ""));
      imgs.forEach(function (f) {
        var b = h("button", "thumb");
        b.type = "button";
        b.setAttribute("aria-label", f.name || L("image"));
        var ph = h("span", "ph"); ph.textContent = f.name || L("image");
        var img = document.createElement("img");
        img.alt = f.name || "";
        img.setAttribute("data-file-id", f.fileId);
        img.addEventListener("load", function () { b.classList.remove("broken"); ph.style.display = "none"; });
        img.addEventListener("error", function () { b.classList.add("broken"); ph.style.display = ""; });
        var src = fileUrl(f.fileId);
        if (src) img.src = src; else b.classList.add("broken");
        b.appendChild(img); b.appendChild(ph);
        b.addEventListener("click", function () { Beluga.post({ type: "open_file", fileId: f.fileId, kind: f.kind || "image", name: f.name || "" }); });
        row.appendChild(b);
      });
      el.appendChild(row);
    }
    if (pdfs.length) {
      var prow = h("div", "files");
      pdfs.forEach(function (f) {
        var b = h("button", "pdf", ICON.pdf + '<span><div class="name"></div><div class="kind"></div></span>');
        b.type = "button";
        b.querySelector(".name").textContent = f.name || "PDF";
        b.querySelector(".kind").textContent = L("pdf");
        b.addEventListener("click", function () { Beluga.post({ type: "open_file", fileId: f.fileId, kind: "pdf", name: f.name || "" }); });
        prow.appendChild(b);
      });
      el.appendChild(prow);
    }
    if (ev.text) {
      var bub = h("div", "bubble");
      if (ev.via === "voice") bub.innerHTML = '<span class="via" title="' + MD.esc(L("voice")) + '">' + ICON.mic + "</span>";
      bub.appendChild(document.createTextNode(ev.text));
      el.appendChild(bub);
    }
    before(el);
    // A new question always brings the view down to it.
    stick = true;
  }

  /* ---------- events ---------- */
  function apply(ev) {
    if (!ev || !ev.t) return;
    var t;
    switch (ev.t) {
      case "user":
        userMsg(ev);
        break;
      case "chat_delta":
        if (restoring) return;
        t = turnFor(ev.turn);
        thinking.classList.remove("on");
        draftFor(t).text += ev.text || "";
        scheduleDraft(t);
        return;
      case "chat":
        t = turnFor(ev.turn);
        thinking.classList.remove("on");
        if (t.draft) finishDraft(t, ev.md || "");
        else if (ev.md) {
          var seg = h("div", "seg md" + (restoring ? "" : " fresh"), MD.block(ev.md));
          t.body.appendChild(seg);
          t.parts.push(ev.md);
        }
        t.sayEl = null;
        break;
      case "say":
        t = turnFor(ev.turn);
        finishDraft(t);
        if (!t.sayEl) { t.sayEl = h("p", "say md"); t.body.appendChild(t.sayEl); t.sayText = ""; t.parts.push(""); }
        t.sayText = (t.sayText ? t.sayText + " " : "") + (ev.text || "");
        t.sayEl.innerHTML = MD.inline(t.sayText);
        t.parts[t.parts.length - 1] = t.sayText;
        thinking.classList.remove("on");
        break;
      case "step":
        t = turnFor(ev.turn);
        finishDraft(t);
        if (ev.title) {
          var hd = h("div", "seg md", '<h3 class="md-h live-step">' + MD.esc(ev.title) + "</h3>");
          t.body.appendChild(hd);
          t.parts.push("### " + ev.title);
        }
        t.sayEl = null;
        break;
      case "show": {
        t = turnFor(ev.turn);
        finishDraft(t);
        thinking.classList.remove("on");
        var node = Blocks.render(ev.block, { still: restoring, width: Math.min(720, log.clientWidth || (global.innerWidth - 40)) });
        if (node) { t.body.appendChild(node); var bt = blockText(ev.block); if (bt) t.parts.push(bt); }
        t.sayEl = null;
        break;
      }
      case "ask": {
        t = turnFor(ev.turn);
        finishDraft(t);
        var chip = h("button", "ask-chip", ICON.reply + '<span class="p"></span>');
        chip.type = "button";
        chip.querySelector(".p").innerHTML = MD.inline(ev.prompt || "");
        chip.addEventListener("click", function () {
          chip.classList.add("used");
          var msg = { type: "ask_tap", prompt: ev.prompt || "" };
          if (ev.kind) msg.kind = ev.kind;
          if (ev.turn != null) msg.turn = ev.turn;
          Beluga.post(msg);
        });
        t.body.appendChild(chip);
        t.parts.push(ev.prompt || "");
        t.sayEl = null;
        break;
      }
      case "done":
        t = turnFor(ev.turn);
        finishDraft(t);
        t.body.appendChild(h("div", "chat-done", MD.esc(L("done"))));
        break;
      case "turn_end":
        t = turns[String(ev.turn)];
        thinking.classList.remove("on");
        if (t) {
          finishDraft(t);
          if (ev.interrupted) t.body.appendChild(h("div", "stopped", MD.esc(L("stopped"))));
          t.ended = true;
          // The copy/retry row arrives when the answer has finished appearing, not before.
          var pending = revealing.filter(function (d) { return !d.done && t.body.contains(d.el); });
          if (pending.length && !restoring) pending[pending.length - 1].onDone = function () { addActions(t); follow(); };
          else addActions(t);
        }
        break;
      case "state":
        if (restoring) return;
        if (ev.state === "thinking" || ev.state === "tool") setThinking(true, ev.state === "tool" ? (ev.tool === "capture" ? "capture" : "tool") : "thinking");
        else setThinking(false);
        return;
      case "notice": {
        var n = h("div", "notice");
        n.textContent = ev.text || ev.code || "";
        before(n);
        break;
      }
      default:
        return;
    }
    follow();
  }

  var chat = {
    version: VERSION,
    reset: function () {
      turns = {}; lastAssistantTurn = null; revealing = []; gliding = false;
      log.innerHTML = "";
      thinking.classList.remove("on");
      log.appendChild(thinking);
      stick = true; updateToBottom();
    },
    apply: apply,
    load: function (events) {
      chat.reset();
      restoring = true;
      log.classList.add("still");
      try {
        (events || []).forEach(apply);
        Object.keys(turns).forEach(function (k) { var t = turns[k]; finishDraft(t); if (t.body.children.length) addActions(t); });
      } finally { restoring = false; log.classList.remove("still"); }
      stick = true;
      toEnd(false);
      updateToBottom();
    },
    setTheme: function (t) { Beluga.setTheme(t); },
    setLang: function (l) { Beluga.setLang(l); },
    setFileBase: function (url, token) {
      fileBase = url || ""; fileToken = token || "";
      Array.prototype.forEach.call(log.querySelectorAll("img[data-file-id]"), function (img) {
        var src = fileUrl(img.getAttribute("data-file-id"));
        if (src) { img.parentNode.classList.remove("broken"); img.src = src; }
      });
    },
    setInsets: function (top, bottom) {
      var st = document.documentElement.style;
      if (top != null) st.setProperty("--chat-top", Math.max(0, +top || 0) + "px");
      if (bottom != null) st.setProperty("--chat-bottom", Math.max(0, +bottom || 0) + "px");
      follow();
    },
    scrollToBottom: function (smooth) { stick = true; toEnd(!!smooth); updateToBottom(); },
    state: function () { return { stick: stick, turns: Object.keys(turns).length, thinking: thinking.classList.contains("on") }; }
  };

  Beluga.onLang(function () { toBottom.setAttribute("aria-label", L("bottom")); });
  chat.reset();
  global.chat = chat;
  Beluga.ready(function () { Beluga.post({ type: "ready", version: VERSION }); });
})(window);
