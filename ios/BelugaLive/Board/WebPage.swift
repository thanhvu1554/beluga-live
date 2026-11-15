import SwiftUI
import WebKit
import BelugaCore

/// A page of the shared web bundle (`shared/board/<name>.html`): `chat`, `knowledge` (PROTOCOL section 14).
/// Calls are queued until the page posts `{type:"ready"}`. When the app bundle has no such page, `available`
/// is false and the screen shows its native fallback instead.
@MainActor @Observable
final class WebPage: NSObject, @preconcurrency WKScriptMessageHandler {
    let name: String
    private(set) var ready = false
    private(set) var available: Bool
    let webView: WKWebView
    @ObservationIgnored var onMessage: (([String: Any]) -> Void)?
    @ObservationIgnored private var queue: [String] = []
    @ObservationIgnored private var theme = "light"
    @ObservationIgnored private var lang = "vi"
    @ObservationIgnored private var loadedRemote = false

    init(name: String) {
        self.name = name
        let config = WKWebViewConfiguration()
        let controller = WKUserContentController()
        config.userContentController = controller
        webView = WKWebView(frame: .zero, configuration: config)
        let bundled = Bundle.main.url(forResource: name, withExtension: "html", subdirectory: "board")
        available = bundled != nil
        super.init()
        for handler in ["board", name] { controller.add(WeakPageHandler(self), name: handler) }
        webView.isOpaque = false
        webView.backgroundColor = .clear
        webView.scrollView.backgroundColor = .clear
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        #if DEBUG
        webView.isInspectable = true
        #endif
        if let bundled { webView.loadFileURL(bundled, allowingReadAccessTo: bundled.deletingLastPathComponent()) }
    }

    /// The bundled app has no copy of this page (or an older bundle): load it from the server's `/board/`.
    func loadRemote(board: URL) {
        guard !loadedRemote else { return }
        loadedRemote = true
        available = true
        ready = false
        webView.load(URLRequest(url: board.appendingPathComponent("\(name).html"),
                                cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 8))
    }

    /// `object.method(args...)` where every argument is JSON.
    func invoke(_ method: String, _ args: JSONValue...) {
        call("\(name).\(method)(\(args.map(\.jsonString).joined(separator: ",")))")
    }

    func setTheme(_ dark: Bool) {
        theme = dark ? "dark" : "light"
        call("\(name).setTheme(\"\(theme)\")")
    }

    func setLang(_ l: Lang) {
        lang = l.rawValue
        call("\(name).setLang(\"\(lang)\")")
    }

    /// Drops queued calls (a reset makes them moot).
    func clearQueue() { queue.removeAll() }

    func call(_ js: String) {
        guard ready else { queue.append(js); return }
        webView.evaluateJavaScript(js, completionHandler: nil)
    }

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let body = message.body as? [String: Any] else { return }
        if body["type"] as? String == "ready" {
            ready = true
            let pending = queue
            queue.removeAll()
            let n = name
            webView.evaluateJavaScript("\(n).setTheme(\"\(theme)\");\(n).setLang(\"\(lang)\")", completionHandler: nil)
            for js in pending { webView.evaluateJavaScript(js, completionHandler: nil) }
        }
        onMessage?(body)
    }
}

@MainActor
private final class WeakPageHandler: NSObject, @preconcurrency WKScriptMessageHandler {
    weak var target: (any WKScriptMessageHandler)?
    init(_ t: any WKScriptMessageHandler) { target = t }
    func userContentController(_ c: WKUserContentController, didReceive m: WKScriptMessage) {
        target?.userContentController(c, didReceive: m)
    }
}

struct WebPageView: UIViewRepresentable {
    let page: WebPage
    func makeUIView(context: Context) -> WKWebView { page.webView }
    func updateUIView(_ uiView: WKWebView, context: Context) {}
}
