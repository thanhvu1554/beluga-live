/* Block renderers (PROTOCOL §3, §6, §14) shared by the live board and the chat.
 * Blocks.render(block, {still, width}) -> HTMLElement | null
 *   still: no entrance animation (history).  width: px available for plots (defaults to window width - 48). */
(function (global) {
  "use strict";
  var LABELS = {
    vi: { answer: "Kết quả", review: "Ôn lại", lastSeen: "Lần cuối", interactive: "Tương tác" },
    en: { answer: "Result", review: "Review", lastSeen: "Last seen", interactive: "Interactive" }
  };
  function L(k) { return (LABELS[Beluga.lang()] || LABELS.vi)[k]; }

  function h(tag, cls, html) {
    var n = document.createElement(tag);
    if (cls) n.className = cls;
    if (html != null) n.innerHTML = html;
    return n;
  }

  function sanitizeSVG(src) {
    var doc = new DOMParser().parseFromString(String(src), "image/svg+xml");
    var svg = doc.documentElement;
    if (!svg || svg.nodeName.toLowerCase() !== "svg") return null;
    var all = svg.querySelectorAll("*");
    [svg].concat(Array.prototype.slice.call(all)).forEach(function (n) {
      var name = n.nodeName.toLowerCase();
      if (name === "script" || name === "foreignobject" || name === "iframe") { n.parentNode && n.parentNode.removeChild(n); return; }
      Array.prototype.slice.call(n.attributes || []).forEach(function (a) {
        if (/^on/i.test(a.name) || /javascript:/i.test(a.value) || (/href$/i.test(a.name) && !/^#/.test(a.value))) n.removeAttribute(a.name);
      });
    });
    return document.importNode(svg, true);
  }

  // "2026-11-12" or an ISO timestamp -> "12 thg 11" / "Nov 12"; unreadable text is shown as it is.
  function fmtDate(raw) {
    if (raw == null || raw === "") return "";
    var d = new Date(typeof raw === "number" ? (raw > 1e11 ? raw : raw * 1000) : String(raw));
    if (isNaN(d.getTime())) return String(raw);
    try { return d.toLocaleDateString(Beluga.lang() === "vi" ? "vi-VN" : "en-US", { day: "numeric", month: "short" }); }
    catch (e) { return String(raw); }
  }

  function fallback(b) {
    if (!b || !b.fallback) return null;
    return h("div", "block text fallback", MD.block(b.fallback));
  }

  /* ---------- interactive: sandboxed iframe, scripts only, no network ---------- */
  var frames = [];
  function frameDoc(html) {
    var pal = Beluga.palette(), vars = "";
    for (var k in pal) if (pal[k]) vars += "--" + k + ":" + pal[k] + ";";
    var prelude =
      '<!doctype html><html data-theme="' + Beluga.theme() + '"><head><meta charset="utf-8">' +
      '<meta http-equiv="Content-Security-Policy" content="default-src \'none\'; script-src \'unsafe-inline\'; style-src \'unsafe-inline\'; img-src data: blob:; font-src data:; media-src data: blob:">' +
      '<meta name="viewport" content="width=device-width,initial-scale=1">' +
      "<style>:root{" + vars + "color-scheme:" + Beluga.theme() + "}html,body{margin:0;padding:0;background:transparent;color:var(--ink);" +
      "font:16px/1.45 -apple-system,system-ui,sans-serif;-webkit-text-size-adjust:100%}" +
      "button{font:inherit;color:var(--ink);background:var(--wash);border:1px solid var(--rule);border-radius:10px;padding:8px 14px}" +
      "input,select{font:inherit;color:var(--ink);accent-color:var(--ink)}svg{color:var(--ink)}</style>" +
      "<script>(function(){var last=0;function send(){var b=document.body;if(!b)return;" +
      "var r=b.getBoundingClientRect();var hh=Math.ceil(Math.max(b.scrollHeight,r.height));if(hh!==last){last=hh;parent.postMessage({beluga:'height',h:hh},'*');}}" +
      "window.addEventListener('load',send);document.addEventListener('DOMContentLoaded',function(){send();" +
      "if(window.ResizeObserver)new ResizeObserver(send).observe(document.body);});setTimeout(send,300);" +
      "window.addEventListener('message',function(e){var d=e.data;if(!d||d.beluga!=='theme')return;var s=document.documentElement;" +
      "for(var k in d.vars)s.style.setProperty('--'+k,d.vars[k]);s.setAttribute('data-theme',d.theme);s.style.colorScheme=d.theme;" +
      "try{window.dispatchEvent(new CustomEvent('belugatheme',{detail:d}))}catch(_){}});})();<\/script></head><body>";
    return prelude + String(html) + "</body></html>";
  }

  function interactive(b) {
    if (!b.html) return fallback(b);
    var n = h("div", "block interactive");
    var f = document.createElement("iframe");
    f.setAttribute("sandbox", "allow-scripts");
    f.setAttribute("referrerpolicy", "no-referrer");
    f.setAttribute("title", b.title || L("interactive"));
    f.setAttribute("loading", "eager");
    var fixed = +b.height > 0;
    var hgt = fixed ? Math.max(40, Math.min(2000, +b.height)) : 160;
    f.style.height = hgt + "px";
    f.srcdoc = frameDoc(b.html);
    n.appendChild(f);
    frames.push({ el: f, auto: !fixed });
    return n;
  }

  global.addEventListener("message", function (e) {
    var d = e.data;
    if (!d || typeof d !== "object") return;
    var hgt = d.beluga === "height" ? d.h : (d.type === "height" ? d.height : null);
    if (hgt == null) return;
    for (var i = 0; i < frames.length; i++) {
      var fr = frames[i];
      if (fr.el.contentWindow === e.source) {
        if (fr.auto) {
          var v = Math.max(40, Math.min(1200, Math.ceil(+hgt) || 0));
          if (Math.abs(v - parseFloat(fr.el.style.height)) > 1) {
            fr.el.style.height = v + "px";
            try { global.dispatchEvent(new CustomEvent("blockresize")); } catch (x) { /* old engines */ }
          }
        }
        return;
      }
    }
  });

  function updateFrames() {
    frames = frames.filter(function (fr) { return fr.el.isConnected; });
    if (!frames.length) return;
    var msg = { beluga: "theme", theme: Beluga.theme(), vars: Beluga.palette() };
    frames.forEach(function (fr) { try { fr.el.contentWindow.postMessage(msg, "*"); } catch (e) { /* gone */ } });
  }
  Beluga.onTheme(function () { setTimeout(updateFrames, 0); });

  /* ---------- the rest ---------- */
  function render(b, opts) {
    opts = opts || {};
    if (!b || !b.type) return null;
    var type = b.type, n;
    switch (type) {
      case "math":
        n = h("div", "block math", MD.tex(b.tex || "", true)); break;
      case "chem":
        n = h("div", "block math chem", MD.tex("\\ce{" + (b.tex || "") + "}", true)); break;
      case "text":
        n = h("div", "block text md", MD.block(b.md)); break;
      case "table": {
        var head = (b.head || []).map(function (c) { return "<th>" + MD.inline(c) + "</th>"; }).join("");
        var rows = (b.rows || []).map(function (r) {
          return "<tr>" + (r || []).map(function (c) { return "<td>" + MD.inline(c) + "</td>"; }).join("") + "</tr>";
        }).join("");
        n = h("div", "block table", "<table>" + (head ? "<thead><tr>" + head + "</tr></thead>" : "") + "<tbody>" + rows + "</tbody></table>");
        break;
      }
      case "code": {
        var hl = b.highlight || [];
        var lines = String(b.code || "").replace(/\n$/, "").split("\n").map(function (l, i) {
          return '<span class="ln' + (hl.indexOf(i + 1) >= 0 ? " hl" : "") + '">' + (MD.esc(l) || " ") + "</span>";
        }).join("");
        n = h("div", "block code", '<div class="code-head"><span class="lang">' + MD.esc(b.lang || "") + "</span>" +
          '<button type="button" class="md-copy" data-copy>' + MD.esc(MD.copyLabel) + "</button></div><pre><code>" + lines + "</code></pre>");
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
        n.appendChild(Plot.render(b, opts.width || (global.innerWidth || 390) - 48));
        break;
      case "review_offer": {
        var when = fmtDate(b.lastSeen);
        var rmeta = [b.subject ? MD.esc(b.subject) : "", when ? L("lastSeen") + " " + MD.esc(when) : ""].filter(Boolean).join(" · ");
        n = h("div", "block review",
          '<span class="label">' + L("review") + "</span>" +
          '<div class="topic">' + MD.inline(b.topic || "") + "</div>" +
          (rmeta ? '<div class="meta">' + rmeta + "</div>" : "") +
          (b.note ? '<div class="why">' + MD.inline(b.note) + "</div>" : ""));
        // The offer carries its own note, so the generic one is not added again.
        if (b.id) n.dataset.id = b.id;
        if (opts.still) n.classList.add("still");
        return n;
      }
      case "answer":
        n = h("div", "block answer", '<span class="label">' + L("answer") + "</span>" + MD.block(b.md)); break;
      case "interactive":
        n = interactive(b);
        if (!n) return null;
        break;
      default:
        return fallback(b);
    }
    if (b.note) n.appendChild(h("div", "note", MD.inline(b.note)));
    if (b.id) n.dataset.id = b.id;
    if (opts.still) n.classList.add("still");
    return n;
  }

  global.Blocks = { render: render, sanitizeSVG: sanitizeSVG, fmtDate: fmtDate, h: h };
})(window);
