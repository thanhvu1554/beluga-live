/* Beluga live board (PROTOCOL §3, §4, §14): lesson events -> one page per step, swiped horizontally.
 * Paging is native CSS scroll-snap on a horizontal scroller whose pages scroll vertically on their own, so the
 * WebKit scroll machinery decides direction (no JS gesture handling that can lose touches in WKWebView).
 *
 * board.reset()                     clear
 * board.apply(event)                step | show | ask | done (raw JSON from the socket)
 * board.restore(events[])           apply many without animation, keeps the current page logic
 * board.load(events[])              reset + restore + show the last step (history)
 * board.goto(i) / next() / prev()
 * board.setTheme("light"|"dark"|null) / setLang("vi"|"en")
 * board.setInsets(top, bottom)      px kept clear for native chrome (bottom default 128)
 * board.setOptions({dots:boolean})  hide the in-page position dots when the app draws its own
 * -> {type:"ready", version}
 * -> {type:"stepChanged", index, count, following}
 * -> {type:"height", height}        content height of the visible step (on change) */
(function (global) {
  "use strict";
  var VERSION = Beluga.VERSION;
  var LABELS = {
    vi: { step: "Bước", ask: "Đến lượt cậu", done: "Xong bài", newStep: "Bước mới" },
    en: { step: "Step", ask: "Your turn", done: "Lesson done", newStep: "New step" }
  };
  function L(k) { return (LABELS[Beluga.lang()] || LABELS.vi)[k]; }
  var h = Blocks.h;

  document.body.classList.add("board-page");
  var root = document.getElementById("steps");
  var chrome = h("div", "board-chrome");
  var dotsEl = h("div", "dots");
  dotsEl.setAttribute("role", "tablist");
  chrome.appendChild(dotsEl);
  document.body.appendChild(chrome);
  var pill = h("button", "new-pill");
  pill.type = "button";
  pill.innerHTML = '<span class="t"></span><svg viewBox="0 0 12 12" aria-hidden="true"><path d="M4.5 2.5 8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>';
  document.body.appendChild(pill);

  var steps = [];        // [{index, el, body, title, num}]
  var current = 0;       // page shown (position in steps[])
  var following = true;  // newest step is followed automatically
  var restoring = false;
  var lastPosted = "";
  var bottomClear = 128;

  function stepFor(index, title) {
    for (var i = 0; i < steps.length; i++) if (steps[i].index === index) {
      if (title != null && title !== "") { steps[i].title.textContent = title; steps[i].el.classList.remove("untitled"); }
      return steps[i];
    }
    var el = h("section", "step" + (title ? "" : " untitled"));
    var head = h("header", "step-head");
    var num = h("div", "step-num");
    num.textContent = L("step") + " " + (index + 1);
    var t = h("div", "step-title");
    t.textContent = title || "";
    head.appendChild(num); head.appendChild(t);
    var body = h("div", "step-body");
    el.appendChild(head); el.appendChild(body);
    var s = { index: index, el: el, body: body, title: t, num: num };
    var after = null;
    for (var j = 0; j < steps.length; j++) if (steps[j].index > index) { after = steps[j]; break; }
    root.insertBefore(el, after ? after.el : null);
    steps.push(s);
    steps.sort(function (a, b) { return a.index - b.index; });
    // Inserting a page before the one shown shifts it: keep the same page in view.
    if (steps.length > 1 && after && steps.indexOf(s) <= current) { current++; jump(current, false); }
    renderDots();
    return s;
  }

  function lastStep() { return steps.length ? steps[steps.length - 1] : stepFor(0, ""); }
  function pageWidth() { return root.clientWidth || global.innerWidth || 390; }

  function renderDots() {
    var n = steps.length;
    while (dotsEl.children.length < n) {
      var d = h("button", "dot");
      d.type = "button";
      d.setAttribute("role", "tab");
      (function (btn) { btn.addEventListener("click", function () { board.goto(Array.prototype.indexOf.call(dotsEl.children, btn)); }); })(d);
      dotsEl.appendChild(d);
    }
    while (dotsEl.children.length > n) dotsEl.removeChild(dotsEl.lastChild);
    for (var i = 0; i < n; i++) {
      dotsEl.children[i].classList.toggle("on", i === current);
      dotsEl.children[i].setAttribute("aria-label", L("step") + " " + (i + 1));
    }
    dotsEl.classList.toggle("hidden", n < 2);
  }

  function updatePill(hasNew) {
    var show = !following && steps.length > 1 && current < steps.length - 1 && (hasNew || pill.classList.contains("show"));
    pill.classList.toggle("show", !!show);
  }

  function announce() {
    following = current >= steps.length - 1;
    if (following) pill.classList.remove("show");
    renderDots();
    var key = current + "/" + steps.length + "/" + following;
    if (key === lastPosted) return;
    lastPosted = key;
    Beluga.post({ type: "stepChanged", index: current, count: steps.length, following: following });
    postHeight();
  }

  var lastHeight = -1;
  function postHeight() {
    var s = steps[current];
    if (!s) return;
    var hgt = Math.ceil(s.el.scrollHeight);
    if (hgt !== lastHeight) { lastHeight = hgt; Beluga.post({ type: "height", height: hgt }); }
  }

  // Programmatic paging; `smooth` animates, otherwise instant.
  var programmatic = 0;
  function jump(i, smooth) {
    if (!steps.length) return;
    var idx = Math.max(0, Math.min(steps.length - 1, i));
    current = idx;
    var left = idx * pageWidth();
    if (Math.abs(root.scrollLeft - left) > 1) {
      programmatic = Date.now();
      // scroll-snap + smooth scrollTo is honoured by WebKit and Blink.
      try { root.scrollTo({ left: left, behavior: smooth && !restoring ? "smooth" : "auto" }); }
      catch (e) { root.scrollLeft = left; }
    }
    announce();
  }

  // The user's own swipes: read the page from the scroll position once it settles. Native scroll-snap does the
  // paging; if an engine leaves the scroller between pages (seen with synthetic touches), finish the snap here,
  // in the direction of the swipe.
  var settleTimer = 0, touching = false, touchStartPage = 0;
  root.addEventListener("touchstart", function () { touching = true; touchStartPage = current; }, { passive: true });
  function touchDone() { touching = false; scheduleSettle(); }
  root.addEventListener("touchend", touchDone, { passive: true });
  root.addEventListener("touchcancel", touchDone, { passive: true });
  function scheduleSettle() {
    clearTimeout(settleTimer);
    settleTimer = setTimeout(settle, 120);
  }
  function settle() {
    if (touching || !steps.length) return;
    var w = pageWidth(), x = root.scrollLeft;
    var idx = Math.max(0, Math.min(steps.length - 1, Math.round(x / w)));
    if (Math.abs(x - idx * w) > 2) {
      var delta = x - touchStartPage * w;
      if (Math.abs(delta) > w * 0.12) idx = touchStartPage + (delta > 0 ? 1 : -1);
      idx = Math.max(0, Math.min(steps.length - 1, idx));
      touchStartPage = idx;
      jump(idx, true);
      updatePill(false);
      return;
    }
    if (idx !== current) { current = idx; touchStartPage = idx; announce(); updatePill(false); }
    else announce();
  }
  root.addEventListener("scroll", scheduleSettle, { passive: true });

  // Keep the shown page aligned when the web view changes size (rotation, panel resize).
  var lastW = 0;
  function onResize() {
    var w = pageWidth();
    if (w === lastW) return;
    lastW = w;
    root.scrollLeft = current * w;
  }
  global.addEventListener("resize", onResize);
  if (global.ResizeObserver) new ResizeObserver(onResize).observe(root);

  function scrollIntoBlock(stepObj, node) {
    // Newest content stays in view while the page fills up (only on the page being followed).
    var page = stepObj.el;
    var bottom = node.offsetTop + node.offsetHeight + bottomClear;
    if (bottom > page.scrollTop + page.clientHeight) {
      page.scrollTo({ top: bottom - page.clientHeight, behavior: restoring ? "auto" : "smooth" });
    }
  }

  // New content on page `s`: follow it, or tell the user it is there.
  function arrived(s, node) {
    var idx = steps.indexOf(s);
    var newest = idx === steps.length - 1;
    if ((following || restoring) && (newest || idx === current)) {
      if (idx !== current) jump(idx, !restoring);
      if (node) scrollIntoBlock(s, node);
    } else if (idx > current) {
      updatePill(true);
    } else if (idx === current && node) {
      scrollIntoBlock(s, node);
    }
    postHeight();
  }

  pill.addEventListener("click", function () { following = true; jump(steps.length - 1, true); });
  global.addEventListener("blockresize", postHeight);

  function setPillText() { pill.querySelector(".t").textContent = L("newStep"); }
  setPillText();

  var board = {
    version: VERSION,
    reset: function () {
      root.innerHTML = ""; steps = []; current = 0; following = true; lastPosted = ""; lastHeight = -1;
      root.scrollLeft = 0;
      pill.classList.remove("show");
      renderDots();
      Beluga.post({ type: "stepChanged", index: 0, count: 0, following: true });
    },
    apply: function (ev) {
      if (!ev || !ev.t) return;
      switch (ev.t) {
        case "step": {
          var s = stepFor(ev.index | 0, ev.title || "");
          arrived(s, null);
          announce();
          break;
        }
        case "show": {
          var st = typeof ev.step === "number" && ev.step >= 0 ? stepFor(ev.step) : lastStep();
          var node = ev.block ? Blocks.render(ev.block, { still: restoring, width: pageWidth() - 48 }) : null;
          if (!node) return;
          st.body.appendChild(node);
          arrived(st, node);
          break;
        }
        case "ask": {
          var sa = lastStep();
          var q = h("div", "block ask" + (restoring ? " still" : ""), '<span class="label">' + L("ask") + "</span>" + MD.inline(ev.prompt || ""));
          sa.body.appendChild(q);
          arrived(sa, q);
          break;
        }
        case "done": {
          var sd = lastStep();
          var d = h("div", "done-mark", MD.esc(L("done")));
          sd.body.appendChild(d);
          arrived(sd, d);
          break;
        }
      }
    },
    restore: function (events) {
      restoring = true;
      try { (events || []).forEach(function (e) { board.apply(e); }); } finally { restoring = false; }
    },
    load: function (events) {
      board.reset();
      board.restore(events);
      following = true;
      jump(steps.length - 1, false);
    },
    goto: function (i) { jump(i | 0, true); },
    next: function () { jump(current + 1, true); },
    prev: function () { jump(current - 1, true); },
    setTheme: function (t) { Beluga.setTheme(t); },
    setLang: function (l) { Beluga.setLang(l); },
    setInsets: function (top, bottom) {
      var st = document.documentElement.style;
      if (top != null) st.setProperty("--top-inset", Math.max(0, +top || 0) + "px");
      if (bottom != null) { bottomClear = Math.max(0, +bottom || 0); st.setProperty("--bottom-clear", bottomClear + "px"); }
    },
    setOptions: function (o) {
      o = o || {};
      if ("dots" in o) { chrome.classList.toggle("no-dots", !o.dots); document.body.classList.toggle("no-dots", !o.dots); }
    },
    state: function () { return { index: current, count: steps.length, following: following }; }
  };

  Beluga.onLang(function () {
    steps.forEach(function (s) { s.num.textContent = L("step") + " " + (s.index + 1); });
    setPillText();
    renderDots();
  });

  global.board = board;
  Beluga.ready(function () { Beluga.post({ type: "ready", version: VERSION }); });
})(window);
