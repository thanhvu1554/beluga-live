/* Shared plumbing for every page of the web bundle: messages to the app, theme, language, labels.
 * Beluga.post(msg)    -> window.webkit.messageHandlers.board.postMessage(msg) (one handler name for all pages);
 *                        in a browser also window.parent.postMessage({beluga:msg}) and window.__posted for the dev harness.
 * Beluga.setTheme(t)  "light" | "dark" | anything else = follow prefers-color-scheme.
 * Beluga.setLang(l)   "vi" | "en"; fires listeners registered with Beluga.onLang(fn). */
(function (global) {
  "use strict";
  var VERSION = 3;
  var langListeners = [], themeListeners = [];
  var state = { lang: "vi", theme: null };

  var native = !!(global.webkit && global.webkit.messageHandlers && global.webkit.messageHandlers.board);
  // Inside the app the native view paints the background; in a browser the page paints its own.
  document.documentElement.classList.toggle("native", native);

  global.__posted = [];
  function post(msg) {
    global.__posted.push(msg);
    if (global.__posted.length > 500) global.__posted.shift();
    try { global.webkit.messageHandlers.board.postMessage(msg); return; } catch (e) { /* not in the app */ }
    try { if (global.parent && global.parent !== global) global.parent.postMessage({ beluga: msg }, "*"); } catch (e2) { /* ignore */ }
  }

  function effectiveTheme() {
    if (state.theme) return state.theme;
    return global.matchMedia && global.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }

  function setTheme(t) {
    state.theme = t === "dark" || t === "light" ? t : null;
    if (state.theme) document.documentElement.setAttribute("data-theme", state.theme);
    else document.documentElement.removeAttribute("data-theme");
    var eff = effectiveTheme();
    themeListeners.forEach(function (fn) { try { fn(eff); } catch (e) { /* keep going */ } });
  }

  function setLang(l) {
    state.lang = l === "en" ? "en" : "vi";
    document.documentElement.lang = state.lang;
    if (global.MD) global.MD.copyLabel = state.lang === "en" ? "Copy" : "Chép";
    langListeners.forEach(function (fn) { try { fn(state.lang); } catch (e) { /* keep going */ } });
  }

  if (global.matchMedia) {
    var mq = global.matchMedia("(prefers-color-scheme: dark)");
    var onChange = function () { if (!state.theme) setTheme(null); };
    if (mq.addEventListener) mq.addEventListener("change", onChange); else if (mq.addListener) mq.addListener(onChange);
  }

  // Theme colours as plain values (for iframes and canvases that cannot read our CSS variables).
  function palette() {
    var cs = getComputedStyle(document.documentElement);
    var names = ["bg", "ink", "ink-2", "ink-3", "rule", "wash", "wash-2", "card"];
    var out = {};
    names.forEach(function (n) { out[n] = cs.getPropertyValue("--" + n).trim(); });
    return out;
  }

  function ready(fn) {
    var go = function () { setTimeout(fn, 0); };
    if (document.fonts && document.fonts.ready) document.fonts.ready.then(go, go); else go();
  }

  // Links never navigate the web view away; the app decides what to do with them.
  document.addEventListener("click", function (e) {
    var a = e.target.closest && e.target.closest("a[href]");
    if (!a) return;
    e.preventDefault();
    var url = a.getAttribute("data-href") || a.getAttribute("href");
    if (/^https?:/i.test(url)) post({ type: "open_url", url: url });
  });

  // Copy buttons on code blocks (markdown fences and `code` blocks).
  function copyText(text) {
    try { if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(text).catch(function () {}); } catch (e) { /* app copies */ }
  }
  document.addEventListener("click", function (e) {
    var btn = e.target.closest && e.target.closest("[data-copy]");
    if (!btn) return;
    var box = btn.closest(".md-code, .block.code");
    var pre = box && box.querySelector("pre");
    if (!pre) return;
    var text = Array.prototype.map.call(pre.querySelectorAll(".ln"), function (l) { return l.textContent; }).join("\n") || pre.textContent;
    var turnEl = btn.closest("[data-turn]");
    var msg = { type: "action", action: "copy", what: "code", text: text };
    if (turnEl) msg.turn = +turnEl.getAttribute("data-turn");
    post(msg);
    copyText(text);
    var old = btn.textContent;
    btn.textContent = state.lang === "en" ? "Copied" : "Đã chép";
    btn.classList.add("done");
    setTimeout(function () { btn.textContent = old; btn.classList.remove("done"); }, 1400);
  });

  global.Beluga = {
    copyText: copyText,
    VERSION: VERSION,
    native: native,
    post: post,
    setTheme: setTheme,
    setLang: setLang,
    theme: effectiveTheme,
    lang: function () { return state.lang; },
    onLang: function (fn) { langListeners.push(fn); },
    onTheme: function (fn) { themeListeners.push(fn); },
    palette: palette,
    ready: ready
  };
})(window);
