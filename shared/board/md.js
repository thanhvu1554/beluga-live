/* Minimal, safe markdown for board blocks: paragraphs, lists, **bold**, *italic*, `code`, $math$. */
(function (global) {
  "use strict";

  function esc(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c];
    });
  }

  function tex(src, display) {
    try {
      return global.katex.renderToString(src, { displayMode: !!display, throwOnError: false, strict: "ignore", trust: false });
    } catch (e) {
      return '<code>' + esc(src) + '</code>';
    }
  }

  // Inline: protect math and code first, then emphasis.
  function inline(s) {
    var slots = [];
    function keep(html) { slots.push(html); return "\u0000" + (slots.length - 1) + "\u0000"; }
    s = String(s);
    s = s.replace(/\$\$([^$]+)\$\$/g, function (_, m) { return keep(tex(m, true)); });
    s = s.replace(/\$([^$\n]+)\$/g, function (_, m) { return keep(tex(m, false)); });
    s = s.replace(/`([^`]+)`/g, function (_, m) { return keep("<code>" + esc(m) + "</code>"); });
    s = esc(s);
    s = s.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
    s = s.replace(/(^|[^*])\*([^*\s][^*]*)\*/g, "$1<em>$2</em>");
    s = s.replace(/\u0000(\d+)\u0000/g, function (_, i) { return slots[+i]; });
    return s;
  }

  function block(md) {
    var lines = String(md || "").replace(/\r/g, "").split("\n");
    var out = [], para = [], list = null;
    function flushPara() { if (para.length) { out.push("<p>" + inline(para.join(" ")) + "</p>"); para = []; } }
    function flushList() { if (list) { out.push("<" + list.tag + ">" + list.items.map(function (i) { return "<li>" + inline(i) + "</li>"; }).join("") + "</" + list.tag + ">"); list = null; } }
    lines.forEach(function (line) {
      var t = line.trim();
      var ul = /^[-*]\s+(.*)$/.exec(t), ol = /^\d+[.)]\s+(.*)$/.exec(t);
      if (!t) { flushPara(); flushList(); return; }
      if (ul || ol) {
        flushPara();
        var tag = ul ? "ul" : "ol";
        if (!list || list.tag !== tag) { flushList(); list = { tag: tag, items: [] }; }
        list.items.push((ul || ol)[1]);
        return;
      }
      flushList();
      para.push(t.replace(/^#+\s*/, ""));
    });
    flushPara(); flushList();
    return out.join("");
  }

  global.MD = { esc: esc, inline: inline, block: block, tex: tex };
})(window);
