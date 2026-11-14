/* Beluga board: renders lesson events (PROTOCOL sections 3 and 4) into swipeable step pages.
 * JS contract: board.reset() / apply(event) / goto(i) / setTheme(t) / setLang(l).
 * Posts {type:"ready", version} and {type:"stepChanged", index, count} to webkit.messageHandlers.board. */
(function (global) {
  "use strict";
  var VERSION = 1;
  var LABELS = {
    vi: { step: "Bước", answer: "Kết quả", ask: "Đến lượt bạn", done: "Xong bài" },
    en: { step: "Step", answer: "Result", ask: "Your turn", done: "Lesson done" }
  };
  var lang = "vi";
  var root = document.getElementById("steps");
  var steps = [];        // [{index, el, body}]
  var current = 0;
  var restoring = false;

  function L(k) { return (LABELS[lang] || LABELS.vi)[k]; }

  function post(msg) {
    try { global.webkit.messageHandlers.board.postMessage(msg); } catch (e) { /* browser preview */ }
  }

  function h(tag, cls, html) {
    var n = document.createElement(tag);
    if (cls) n.className = cls;
    if (html != null) n.innerHTML = html;
    return n;
  }

  function stepFor(index, title) {
    for (var i = 0; i < steps.length; i++) if (steps[i].index === index) {
      if (title != null) steps[i].title.textContent = title;
      return steps[i];
    }
    var el = h("section", "step");
    var head = h("header", "step-head");
    var num = h("div", "step-num");
    num.textContent = L("step") + " " + (index + 1);
    var t = h("div", "step-title");
    t.textContent = title || "";
    head.appendChild(num); head.appendChild(t);
    var body = h("div", "step-body");
    el.appendChild(head); el.appendChild(body);
    var s = { index: index, el: el, body: body, title: t, num: num };
    // Keep pages ordered by index.
    var after = null;
    for (var j = 0; j < steps.length; j++) if (steps[j].index > index) { after = steps[j]; break; }
    root.insertBefore(el, after ? after.el : null);
    steps.push(s);
    steps.sort(function (a, b) { return a.index - b.index; });
    return s;
  }

  function lastStep() { return steps.length ? steps[steps.length - 1] : stepFor(0, ""); }

  function sanitizeSVG(src) {
    var doc = new DOMParser().parseFromString(String(src), "image/svg+xml");
    var svg = doc.documentElement;
    if (!svg || svg.nodeName.toLowerCase() !== "svg") return null;
    var all = svg.querySelectorAll("*");
    [svg].concat(Array.prototype.slice.call(all)).forEach(function (n) {
      var name = n.nodeName.toLowerCase();
      if (name === "script" || name === "foreignobject") { n.parentNode && n.parentNode.removeChild(n); return; }
      Array.prototype.slice.call(n.attributes || []).forEach(function (a) {
        if (/^on/i.test(a.name) || /javascript:/i.test(a.value) || (/href$/i.test(a.name) && !/^#/.test(a.value))) n.removeAttribute(a.name);
      });
    });
    return document.importNode(svg, true);
  }

  function renderBlock(b) {
    var type = b.type, n;
    switch (type) {
      case "math":
        n = h("div", "block math", MD.tex(b.tex || "", true)); break;
      case "chem":
        n = h("div", "block math chem", MD.tex("\\ce{" + (b.tex || "") + "}", true)); break;
      case "text":
        n = h("div", "block text", MD.block(b.md)); break;
      case "table": {
        var head = (b.head || []).map(function (c) { return "<th>" + MD.inline(c) + "</th>"; }).join("");
        var rows = (b.rows || []).map(function (r) {
          return "<tr>" + r.map(function (c) { return "<td>" + MD.inline(c) + "</td>"; }).join("") + "</tr>";
        }).join("");
        n = h("div", "block table", "<table>" + (head ? "<thead><tr>" + head + "</tr></thead>" : "") + "<tbody>" + rows + "</tbody></table>");
        break;
      }
      case "code": {
        var hl = b.highlight || [];
        var lines = String(b.code || "").replace(/\n$/, "").split("\n").map(function (l, i) {
          return '<span class="ln' + (hl.indexOf(i + 1) >= 0 ? " hl" : "") + '">' + (MD.esc(l) || " ") + "</span>";
        }).join("");
        n = h("div", "block code", (b.lang ? '<div class="lang">' + MD.esc(b.lang) + "</div>" : "") + "<pre><code>" + lines + "</code></pre>");
        break;
      }
      case "timeline":
        n = h("div", "block timeline", "<ol>" + (b.items || []).map(function (it) {
          return '<li><div class="when">' + MD.inline(it.when || "") + '</div><div class="what">' + MD.inline(it.what || "") + "</div></li>";
        }).join("") + "</ol>");
        break;
      case "vocab": {
        var meta = [b.ipa, b.pos].filter(Boolean).map(MD.esc).join(" · ");
        n = h("div", "block vocab",
          '<div><span class="word">' + MD.esc(b.word || "") + "</span>" + (meta ? '<span class="meta">' + meta + "</span>" : "") + "</div>" +
          '<div class="meaning">' + MD.inline(b.meaning || "") + "</div>" +
          (b.example ? '<div class="example">' + MD.inline(b.example) + "</div>" : ""));
        break;
      }
      case "svg": {
        var svg = sanitizeSVG(b.svg);
        if (!svg) return fallback(b);
        n = h("div", "block svg"); n.appendChild(svg); break;
      }
      case "plot":
        n = h("div", "block plot");
        n.appendChild(Plot.render(b, (global.innerWidth || 390) - 48));
        break;
      case "answer":
        n = h("div", "block answer", '<span class="label">' + L("answer") + "</span>" + MD.block(b.md)); break;
      default:
        return fallback(b);
    }
    if (b.note) n.appendChild(h("div", "note", MD.inline(b.note)));
    if (b.id) n.dataset.id = b.id;
    return n;
  }

  function fallback(b) {
    if (!b.fallback) return null;
    return h("div", "block text fallback", MD.block(b.fallback));
  }

  function settle(target) {
    current = target;
    post({ type: "stepChanged", index: current, count: steps.length });
  }

  // Paging by transform in vw units: independent of when the web view gets its size.
  function scrollToStep(i, smooth) {
    if (!steps.length) return;
    var idx = Math.max(0, Math.min(steps.length - 1, i));
    if (!smooth) root.classList.add("dragging");
    root.style.transform = "translateX(" + (-idx * 100) + "vw)";
    if (!smooth) { void root.offsetWidth; root.classList.remove("dragging"); }
    if (idx !== current || !smooth) settle(idx);
  }

  function scrollIntoBlock(stepObj, node) {
    // Newest content stays in view while the page fills up.
    var page = stepObj.el;
    var bottom = node.offsetTop + node.offsetHeight + 120;
    if (bottom > page.scrollTop + page.clientHeight) page.scrollTo({ top: bottom - page.clientHeight, behavior: restoring ? "auto" : "smooth" });
  }

  // Horizontal swipe between steps; vertical drags scroll the page.
  var touch = null;
  root.addEventListener("touchstart", function (e) {
    var t = e.touches[0];
    touch = { x: t.clientX, y: t.clientY, dx: 0, horizontal: null };
  }, { passive: true });
  root.addEventListener("touchmove", function (e) {
    if (!touch) return;
    var t = e.touches[0];
    touch.dx = t.clientX - touch.x;
    var dy = t.clientY - touch.y;
    if (touch.horizontal === null && (Math.abs(touch.dx) > 8 || Math.abs(dy) > 8)) touch.horizontal = Math.abs(touch.dx) > Math.abs(dy);
    if (touch.horizontal) {
      e.preventDefault();
      var edge = (current === 0 && touch.dx > 0) || (current === steps.length - 1 && touch.dx < 0);
      root.classList.add("dragging");
      root.style.transform = "translateX(calc(" + (-current * 100) + "vw + " + (edge ? touch.dx / 3 : touch.dx) + "px))";
    }
  }, { passive: false });
  root.addEventListener("touchend", function () {
    if (!touch) return;
    root.classList.remove("dragging");
    var next = current;
    if (touch.horizontal && touch.dx < -50) next = current + 1;
    if (touch.horizontal && touch.dx > 50) next = current - 1;
    touch = null;
    scrollToStep(next, true);
  });

  var board = {
    version: VERSION,
    reset: function () {
      root.innerHTML = ""; steps = []; current = 0; root.style.transform = "";
      post({ type: "stepChanged", index: 0, count: 0 });
    },
    apply: function (ev) {
      if (!ev || !ev.t) return;
      var live = !ev.restore;
      switch (ev.t) {
        case "step": {
          var s = stepFor(ev.index | 0, ev.title || "");
          scrollToStep(steps.indexOf(s), !restoring);
          break;
        }
        case "show": {
          var st = typeof ev.step === "number" ? stepFor(ev.step) : lastStep();
          var node = ev.block ? renderBlock(ev.block) : null;
          if (!node) return;
          st.body.appendChild(node);
          if (steps.indexOf(st) !== current) scrollToStep(steps.indexOf(st), !restoring);
          scrollIntoBlock(st, node);
          break;
        }
        case "ask": {
          var sa = lastStep();
          var q = h("div", "block ask", '<span class="label">' + L("ask") + "</span>" + MD.inline(ev.prompt || ""));
          sa.body.appendChild(q);
          scrollIntoBlock(sa, q);
          break;
        }
        case "done": {
          var sd = lastStep();
          var d = h("div", "done-mark", MD.esc(L("done")));
          sd.body.appendChild(d);
          scrollIntoBlock(sd, d);
          break;
        }
      }
      void live;
    },
    goto: function (i) { scrollToStep(i | 0, true); },
    setTheme: function (t) { document.documentElement.setAttribute("data-theme", t === "dark" ? "dark" : "light"); },
    setLang: function (l) {
      lang = LABELS[l] ? l : "vi";
      document.documentElement.lang = lang;
      steps.forEach(function (s) { s.num.textContent = L("step") + " " + (s.index + 1); });
    },
    /* Apply many events without animation (used for history). */
    restore: function (events) {
      restoring = true;
      document.body.classList.add("restored");
      (events || []).forEach(board.apply);
      restoring = false;
      setTimeout(function () { document.body.classList.remove("restored"); }, 50);
    }
  };

  global.board = board;
  function ready() { post({ type: "ready", version: VERSION }); }
  if (document.fonts && document.fonts.ready) document.fonts.ready.then(ready); else ready();
})(window);
