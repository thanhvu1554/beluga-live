/* Function plots drawn as plain SVG (no library). */
(function (global) {
  "use strict";
  var NS = "http://www.w3.org/2000/svg";
  var FUNCS = { sin: 1, cos: 1, tan: 1, sqrt: 1, abs: 1, log: 1, ln: 1, exp: 1, asin: 1, acos: 1, atan: 1, floor: 1, ceil: 1 };

  // Compile "x^2-5*x+6" into a JS function, allowing only numbers, x, operators and known functions.
  function compile(expr) {
    var src = String(expr).replace(/\s+/g, "").replace(/−/g, "-");
    if (!/^[0-9x.+\-*/^()a-z,]*$/i.test(src)) return null;
    var words = src.match(/[a-z]+/gi) || [];
    for (var i = 0; i < words.length; i++) {
      var w = words[i].toLowerCase();
      if (w !== "x" && w !== "pi" && w !== "e" && !FUNCS[w]) return null;
    }
    src = src
      .replace(/(\d)(x|\()/gi, "$1*$2")
      .replace(/\)(\(|x|\d)/gi, ")*$1")
      .replace(/x\(/gi, "x*(")
      .replace(/\^/g, "**")
      .replace(/\bln\(/g, "Math.log(")
      .replace(/\blog\(/g, "Math.log10(")
      .replace(/\b(sin|cos|tan|sqrt|abs|exp|asin|acos|atan|floor|ceil)\(/g, "Math.$1(")
      .replace(/\bpi\b/gi, "Math.PI")
      .replace(/\be\b/g, "Math.E");
    try {
      var f = new Function("x", "return (" + src + ");");
      f(1);
      return f;
    } catch (e) { return null; }
  }

  function el(name, attrs, parent) {
    var n = document.createElementNS(NS, name);
    for (var k in attrs) n.setAttribute(k, attrs[k]);
    if (parent) parent.appendChild(n);
    return n;
  }

  function niceStep(span) {
    var raw = span / 6, mag = Math.pow(10, Math.floor(Math.log10(raw))), n = raw / mag;
    return (n < 1.5 ? 1 : n < 3 ? 2 : n < 7 ? 5 : 10) * mag;
  }

  function fmt(v) { return Math.abs(v) < 1e-9 ? "0" : String(+v.toFixed(2)).replace("-", "−"); }

  function render(block, width) {
    var W = Math.max(280, width || 340), H = Math.round(W * 0.56), pad = 24;
    var fns = (block.fn || []).map(compile).filter(Boolean);
    var x0 = (block.x || [-5, 5])[0], x1 = (block.x || [-5, 5])[1];
    var y0, y1;
    if (block.y) { y0 = block.y[0]; y1 = block.y[1]; }
    else {
      y0 = Infinity; y1 = -Infinity;
      fns.forEach(function (f) {
        for (var i = 0; i <= 200; i++) {
          var v = f(x0 + (x1 - x0) * i / 200);
          if (isFinite(v)) { y0 = Math.min(y0, v); y1 = Math.max(y1, v); }
        }
      });
      if (!isFinite(y0)) { y0 = -5; y1 = 5; }
      var m = (y1 - y0) * 0.1 || 1; y0 -= m; y1 += m;
    }
    function sx(x) { return pad + (x - x0) / (x1 - x0) * (W - 2 * pad); }
    function sy(y) { return H - pad - (y - y0) / (y1 - y0) * (H - 2 * pad); }

    var svg = el("svg", { viewBox: "0 0 " + W + " " + H, role: "img" });
    var g = el("g", {}, svg);
    var stepX = niceStep(x1 - x0), stepY = niceStep(y1 - y0), v;
    for (v = Math.ceil(x0 / stepX) * stepX; v <= x1 + 1e-9; v += stepX) el("line", { "class": "grid", x1: sx(v), x2: sx(v), y1: pad, y2: H - pad }, g);
    for (v = Math.ceil(y0 / stepY) * stepY; v <= y1 + 1e-9; v += stepY) el("line", { "class": "grid", y1: sy(v), y2: sy(v), x1: pad, x2: W - pad }, g);

    var ax = Math.min(Math.max(0, y0), y1), ay = Math.min(Math.max(0, x0), x1);
    el("line", { "class": "axis", x1: pad, x2: W - pad, y1: sy(ax), y2: sy(ax) }, g);
    el("line", { "class": "axis", y1: pad, y2: H - pad, x1: sx(ay), x2: sx(ay) }, g);

    // Everything a point label must stay clear of, as pixel samples.
    var obstacles = [];
    var px;
    for (px = pad; px <= W - pad; px += 6) obstacles.push([px, sy(ax)]);
    for (px = pad; px <= H - pad; px += 6) obstacles.push([sx(ay), px]);

    // A tick whose number is also shown as a point label on that axis is dropped.
    var labelled = (block.points || []).filter(function (p) { return p.label; });
    function dupX(val) {
      return labelled.some(function (p) { return Math.abs(p.y - ax) < 1e-9 && Math.abs(p.x - val) < 1e-9; });
    }
    function dupY(val) {
      return labelled.some(function (p) { return Math.abs(p.x - ay) < 1e-9 && Math.abs(p.y - val) < 1e-9; });
    }
    for (v = Math.ceil(x0 / stepX) * stepX; v <= x1 + 1e-9; v += stepX) {
      if (Math.abs(v) < 1e-9 || dupX(v)) continue;
      var t = el("text", { "class": "tick", x: sx(v), y: sy(ax) + 14, "text-anchor": "middle" }, g); t.textContent = fmt(v);
      obstacles.push([sx(v), sy(ax) + 10], [sx(v) - 6, sy(ax) + 10], [sx(v) + 6, sy(ax) + 10]);
    }
    for (v = Math.ceil(y0 / stepY) * stepY; v <= y1 + 1e-9; v += stepY) {
      if (Math.abs(v) < 1e-9 || dupY(v)) continue;
      var t2 = el("text", { "class": "tick", x: sx(ay) - 6, y: sy(v) + 4, "text-anchor": "end" }, g); t2.textContent = fmt(v);
      obstacles.push([sx(ay) - 12, sy(v)], [sx(ay) - 6, sy(v)]);
    }

    var clip = "c" + Math.random().toString(36).slice(2);
    var defs = el("defs", {}, svg);
    var cp = el("clipPath", { id: clip }, defs);
    el("rect", { x: pad, y: pad, width: W - 2 * pad, height: H - 2 * pad }, cp);

    fns.forEach(function (f, idx) {
      var d = "", pen = false, N = 400;
      for (var i = 0; i <= N; i++) {
        var x = x0 + (x1 - x0) * i / N, y = f(x);
        if (!isFinite(y) || Math.abs(y) > 1e6) { pen = false; continue; }
        d += (pen ? "L" : "M") + sx(x).toFixed(1) + " " + sy(y).toFixed(1);
        if (i % 2 === 0 && sy(y) >= pad - 4 && sy(y) <= H - pad + 4) obstacles.push([sx(x), sy(y)]);
        pen = true;
      }
      el("path", { "class": "curve" + (idx > 0 ? " alt" : ""), d: d, "clip-path": "url(#" + clip + ")" }, svg);
    });

    // Label position: the spot around the point that is farthest from curves, axes and numbers.
    function placeLabel(cx, cy, text) {
      var hw = text.length * 3.6 + 3, hh = 8, best = null, a, r;
      for (r = 14; r <= 22; r += 8) {
        for (a = 0; a < 360; a += 30) {
          var rad = a * Math.PI / 180, lx = cx + r * Math.cos(rad), ly = cy + r * Math.sin(rad);
          if (lx - hw < pad || lx + hw > W - pad || ly - hh < pad || ly + hh > H - pad) continue;
          var clear = 1e9;
          for (var k = 0; k < obstacles.length; k++) {
            var dx = Math.max(0, Math.abs(obstacles[k][0] - lx) - hw), dy = Math.max(0, Math.abs(obstacles[k][1] - ly) - hh);
            var dist = Math.sqrt(dx * dx + dy * dy);
            if (dist < clear) clear = dist;
          }
          // Prefer clear space, then closer to the point, then above it.
          var score = Math.min(clear, 12) * 10 - (r - 14) * 0.5 + (a > 180 ? 1 : 0);
          if (!best || score > best.score) best = { x: lx, y: ly, score: score };
        }
      }
      return best || { x: cx + 12, y: cy - 12 };
    }

    (block.points || []).forEach(function (p) {
      obstacles.push([sx(p.x), sy(p.y)]);
      for (var q = 0; q < 8; q++) obstacles.push([sx(p.x) + 8 * Math.cos(q * Math.PI / 4), sy(p.y) + 8 * Math.sin(q * Math.PI / 4)]);
    });
    (block.points || []).forEach(function (p) {
      el("circle", { "class": "pt-ring", cx: sx(p.x), cy: sy(p.y), r: 7 }, svg);
      el("circle", { "class": "pt", cx: sx(p.x), cy: sy(p.y), r: 3.5 }, svg);
      if (p.label) {
        var text = String(p.label), pos = placeLabel(sx(p.x), sy(p.y), text);
        var lt = el("text", { "class": "pt-label", x: pos.x, y: pos.y + 4, "text-anchor": "middle" }, svg);
        lt.textContent = text;
        obstacles.push([pos.x, pos.y]);
      }
    });
    return svg;
  }

  global.Plot = { render: render, compile: compile };
})(window);
