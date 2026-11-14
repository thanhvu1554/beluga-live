/* Safe markdown for the board, chat and knowledge pages. Everything is escaped; raw HTML never passes.
 * MD.block(md, {partial})  block markdown: headings, paragraphs, nested lists, tables, fenced code (with a copy
 *                          button), quotes, rules, display math ($$..$$ and \[..\]).
 *                          partial: text is still streaming, so unfinished math/tables/emphasis are held back
 *                          instead of flashing as raw source.
 * MD.inline(s)             **bold**, *italic*, ~~strike~~, `code`, $math$, \(math\), [links](https://…).
 * MD.tex(src, display)     KaTeX (with mhchem), memoised.
 * MD.esc(s)                HTML escape. */
(function (global) {
  "use strict";

  function esc(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c];
    });
  }

  var texCache = new Map();
  function tex(src, display) {
    var key = (display ? "D" : "I") + src;
    var hit = texCache.get(key);
    if (hit) return hit;
    var out;
    try {
      out = global.katex.renderToString(String(src), { displayMode: !!display, throwOnError: false, strict: "ignore", trust: false, output: "html" });
    } catch (e) {
      out = "<code>" + esc(src) + "</code>";
    }
    if (texCache.size > 600) texCache.clear();
    texCache.set(key, out);
    return out;
  }

  function safeUrl(u) {
    u = String(u).trim();
    return /^https?:\/\//i.test(u) ? u : null;
  }

  // Inline: protect math and code first, then emphasis and links.
  function inline(s, opts) {
    var partial = opts && opts.partial;
    var slots = [];
    function keep(html) { slots.push(html); return "\u0000" + (slots.length - 1) + "\u0000"; }
    s = String(s == null ? "" : s);
    s = s.replace(/`([^`\n]+)`/g, function (_, m) { return keep("<code>" + esc(m) + "</code>"); });
    s = s.replace(/\$\$([\s\S]+?)\$\$/g, function (_, m) { return keep(tex(m, true)); });
    s = s.replace(/\\\[([\s\S]+?)\\\]/g, function (_, m) { return keep(tex(m, true)); });
    s = s.replace(/\\\(([\s\S]+?)\\\)/g, function (_, m) { return keep(tex(m, false)); });
    // $x$: no space just inside the dollars, so "5$ và 10$" stays text.
    s = s.replace(/(^|[^\\$])\$(?!\s)([^$\n]*?[^\s\\$])\$(?!\d)/g, function (_, pre, m) { return pre + keep(tex(m, false)); });
    s = s.replace(/(^|[^\\$])\$([^\s$])\$(?!\d)/g, function (_, pre, m) { return pre + keep(tex(m, false)); });
    if (partial) {
      // Still streaming: an opening math/code mark without its end is held back until it closes.
      s = s.replace(/(\$\$|\\\[|\\\(|(^|[^\\])\$(?=\S)|`)[^\u0000]*$/, function (m, mark, pre) { return pre || ""; });
    }
    s = s.replace(/\\\$/g, keep("$"));
    s = esc(s);
    s = s.replace(/\[([^\]\n]+)\]\(([^)\s]+)\)/g, function (all, text, url) {
      var u = safeUrl(url.replace(/&amp;/g, "&"));
      return u ? '<a href="' + esc(u) + '" data-href="' + esc(u) + '">' + text + "</a>" : text;
    });
    s = s.replace(/\*\*([^*\n]+?)\*\*/g, "<strong>$1</strong>");
    s = s.replace(/__([^_\n]+?)__/g, "<strong>$1</strong>");
    s = s.replace(/(^|[^*\w])\*([^*\s][^*\n]*?)\*(?!\w)/g, "$1<em>$2</em>");
    s = s.replace(/(^|[^_\w])_([^_\s][^_\n]*?)_(?!\w)/g, "$1<em>$2</em>");
    s = s.replace(/~~([^~\n]+?)~~/g, "<del>$1</del>");
    if (partial) s = s.replace(/\*\*?$/, "").replace(/\*\*(?=[^*]*$)/, "");
    s = s.replace(/\u0000(\d+)\u0000/g, function (_, i) { return slots[+i]; });
    return s;
  }

  function splitRow(line) {
    var t = line.trim().replace(/^\|/, "").replace(/\|$/, "");
    var cells = [], cur = "", math = false;
    for (var i = 0; i < t.length; i++) {
      var c = t[i];
      if (c === "\\" && t[i + 1] === "|") { cur += "|"; i++; continue; }
      if (c === "$") math = !math;
      if (c === "|" && !math) { cells.push(cur.trim()); cur = ""; continue; }
      cur += c;
    }
    cells.push(cur.trim());
    return cells;
  }
  var SEP = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;

  function codeBlock(lang, code, open) {
    return '<div class="md-code' + (open ? " open" : "") + '"><div class="md-code-head"><span class="md-code-lang">' + esc(lang || "") +
      '</span><button type="button" class="md-copy" data-copy>' + esc(MD.copyLabel) + "</button></div><pre><code>" +
      esc(code) + "</code></pre></div>";
  }

  function block(md, opts) {
    var partial = !!(opts && opts.partial);
    var src = String(md == null ? "" : md).replace(/\r/g, "");
    if (partial) src = trimOpenTail(src);
    var lines = src.split("\n");
    var out = [];
    var i = 0;
    var para = [];
    function il(s) { return inline(s, { partial: false }); }
    function flushPara(last) {
      if (!para.length) return;
      var text = para.join("\n").trim().replace(/\s*\n\s*/g, " ");
      out.push("<p>" + (last && partial ? inline(text, { partial: true }) : il(text)) + "</p>");
      para = [];
    }
    function indentOf(l) { var m = /^( *)/.exec(l.replace(/\t/g, "    ")); return m[1].length; }
    var LIST = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;

    function parseList(start) {
      // Collect the list lines (items, continuations, nested items) and build nested HTML.
      var items = [], j = start, baseIndent = indentOf(lines[start]);
      while (j < lines.length) {
        var l = lines[j];
        if (!l.trim()) {
          // A blank line ends the list unless the next line keeps it going.
          var nx = lines[j + 1];
          if (nx != null && (LIST.test(nx) && indentOf(nx) >= baseIndent || (nx.trim() && indentOf(nx) > baseIndent))) { j++; continue; }
          break;
        }
        var m = LIST.exec(l);
        if (m && indentOf(l) < baseIndent) break;
        if (!m && indentOf(l) <= baseIndent && items.length && !/^\s/.test(l)) {
          // Lazy continuation of the last item only if not a new block construct.
          if (/^(#{1,6}\s|```|>|\$\$|\\\[|\|)/.test(l.trim())) break;
        }
        items.push(l);
        j++;
      }
      return { html: buildList(items), next: j };
    }
    function buildList(ls) {
      var html = "", stack = [];
      function close(toDepth) {
        while (stack.length > toDepth) { var s = stack.pop(); html += "</li></" + s.tag + ">"; }
      }
      ls.forEach(function (l) {
        var m = LIST.exec(l);
        var ind = indentOf(l);
        if (m) {
          var tag = /\d/.test(m[2]) ? "ol" : "ul";
          while (stack.length && ind < stack[stack.length - 1].indent) close(stack.length - 1);
          var top = stack[stack.length - 1];
          if (!top || ind > top.indent + 1) {
            var startAttr = tag === "ol" && parseInt(m[2], 10) !== 1 ? ' start="' + parseInt(m[2], 10) + '"' : "";
            html += "<" + tag + startAttr + "><li>";
            stack.push({ tag: tag, indent: ind });
          } else if (top.tag !== tag) {
            close(stack.length - 1);
            html += "<" + tag + "><li>";
            stack.push({ tag: tag, indent: ind });
          } else {
            html += "</li><li>";
          }
          html += il(m[3]);
        } else {
          html += " " + il(l.trim());
        }
      });
      close(0);
      return html;
    }

    while (i < lines.length) {
      var line = lines[i], t = line.trim();
      if (!t) { flushPara(); i++; continue; }

      var fence = /^(```+|~~~+)\s*([\w+#.-]*)/.exec(t);
      if (fence) {
        flushPara();
        var buf = [], j = i + 1, closed = false;
        while (j < lines.length) {
          if (lines[j].trim().indexOf(fence[1]) === 0) { closed = true; break; }
          buf.push(lines[j]); j++;
        }
        out.push(codeBlock(fence[2], buf.join("\n"), !closed));
        i = closed ? j + 1 : j;
        continue;
      }

      if (/^(\$\$|\\\[)/.test(t)) {
        var open = t.slice(0, 2) === "$$" ? "$$" : "\\[", close = open === "$$" ? "$$" : "\\]";
        var rest = t.slice(2), mbuf = [], k = i, done = false;
        var endAt = rest.indexOf(close);
        if (endAt >= 0) {
          flushPara();
          out.push('<div class="md-math">' + tex(rest.slice(0, endAt), true) + "</div>");
          var after = rest.slice(endAt + 2).trim();
          if (after) para.push(after);
          i++; continue;
        }
        if (rest) mbuf.push(rest);
        k = i + 1;
        while (k < lines.length) {
          var lk = lines[k], e = lk.indexOf(close);
          if (e >= 0) { mbuf.push(lk.slice(0, e)); done = true; break; }
          mbuf.push(lk); k++;
        }
        flushPara();
        if (done) out.push('<div class="md-math">' + tex(mbuf.join("\n"), true) + "</div>");
        else if (!partial) out.push('<div class="md-math">' + tex(mbuf.join("\n"), true) + "</div>");
        else out.push('<div class="md-math pending"></div>');
        i = done ? k + 1 : k;
        continue;
      }

      var hd = /^(#{1,6})\s+(.*)$/.exec(t);
      if (hd) {
        flushPara();
        var lvl = Math.min(4, Math.max(2, hd[1].length + 1));
        out.push("<h" + lvl + ' class="md-h">' + il(hd[2].replace(/\s#+\s*$/, "")) + "</h" + lvl + ">");
        i++; continue;
      }

      if (/^([-*_])(\s*\1){2,}$/.test(t)) { flushPara(); out.push("<hr>"); i++; continue; }

      if (t[0] === "|" || (t.indexOf("|") > 0 && lines[i + 1] && SEP.test(lines[i + 1]))) {
        if (lines[i + 1] != null && SEP.test(lines[i + 1])) {
          flushPara();
          var head = splitRow(t), aligns = splitRow(lines[i + 1]).map(function (c) {
            return /^:-+:$/.test(c) ? "center" : /-+:$/.test(c) ? "right" : "";
          });
          var rows = [], r = i + 2;
          while (r < lines.length && lines[r].trim() && lines[r].indexOf("|") >= 0) { rows.push(splitRow(lines[r])); r++; }
          if (partial && r === lines.length && rows.length) rows.pop(); // last row may still be arriving
          function cell(tag, c, idx) { var a = aligns[idx]; return "<" + tag + (a ? ' style="text-align:' + a + '"' : "") + ">" + il(c) + "</" + tag + ">"; }
          out.push('<div class="md-table"><table><thead><tr>' + head.map(function (c, x) { return cell("th", c, x); }).join("") +
            "</tr></thead><tbody>" + rows.map(function (row) {
              return "<tr>" + head.map(function (_, x) { return cell("td", row[x] || "", x); }).join("") + "</tr>";
            }).join("") + "</tbody></table></div>");
          i = r; continue;
        }
        if (partial && t[0] === "|" && i + 2 >= lines.length) { flushPara(); i = lines.length; break; } // table header still arriving
      }

      if (t[0] === ">") {
        flushPara();
        var q = [];
        while (i < lines.length && lines[i].trim()[0] === ">") { q.push(lines[i].trim().replace(/^>\s?/, "")); i++; }
        out.push("<blockquote>" + block(q.join("\n"), opts) + "</blockquote>");
        continue;
      }

      if (LIST.test(line)) {
        flushPara();
        var res = parseList(i);
        out.push(res.html);
        i = res.next; continue;
      }

      para.push(line);
      i++;
    }
    flushPara(true);
    return out.join("");
  }

  // Streaming: the last line may end inside $..$, \(..\) or `..`; cut it there so raw source never flashes.
  function trimOpenTail(src) {
    var nl = src.lastIndexOf("\n"), head = src.slice(0, nl + 1), last = src.slice(nl + 1);
    if (/^\s*(```|~~~|\$\$|\\\[)/.test(last)) return src;
    var fences = (src.match(/^\s*(```|~~~)/gm) || []).length;
    if (fences % 2 === 1) return src; // inside a code block: shown as it comes
    var cut = -1, i, inMath = false, inCode = false, mathStart = -1, codeStart = -1;
    for (i = 0; i < last.length; i++) {
      var c = last[i];
      if (c === "\\" && !inCode) {
        if (last[i + 1] === "(" && !inMath) { inMath = "("; mathStart = i; i++; continue; }
        if (last[i + 1] === ")" && inMath === "(") { inMath = false; i++; continue; }
        i++; continue;
      }
      if (c === "`" && !inMath) { inCode = !inCode; codeStart = i; continue; }
      if (c === "$" && !inCode) {
        if (last[i + 1] === "$") { if (inMath === "$$") inMath = false; else if (!inMath) { inMath = "$$"; mathStart = i; } i++; continue; }
        if (inMath === "$") inMath = false;
        else if (!inMath && last[i + 1] && /\S/.test(last[i + 1])) { inMath = "$"; mathStart = i; }
      }
    }
    if (inMath) cut = mathStart;
    if (inCode) cut = cut < 0 ? codeStart : Math.min(cut, codeStart);
    if (cut >= 0) last = last.slice(0, cut);
    return head + last;
  }

  var MD = { esc: esc, inline: inline, block: block, tex: tex, copyLabel: "Chép" };
  global.MD = MD;
})(window);
